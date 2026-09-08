//! The account configuration wizard (ADR 0003): state machine and
//! reducer slice.
//!
//! The wizard is one route owning a small step machine
//! (`Email → Discovery → Identity → Credentials → Testing → Confirm →
//! Saved`). All transitions run through the same I/O-free reducer
//! contract as the rest of the app: side effects travel as
//! `OperationKind::DiscoverConfig` / `TestAccount` / `SaveAccount`
//! effects for the operation manager, and results arrive as
//! `Action::BackendCompleted`. Secrets live only here and in the boxed
//! `DraftAccountConfig` payload — never in logs, never in display
//! strings (the UI renders the raw password masked).
//!
//! `Esc` always steps back; from the first step it cancels the wizard
//! (which quits the app: without an account there is nothing to show).
//! While the wizard is active the reducer suppresses mailbox warmup and
//! swallows every mailbox-key action.

use std::path::PathBuf;

use crate::app::action::{Action, DialogEdit};
use crate::app::effect::Effect;
use crate::app::operation::{OperationId, OperationKind, OperationOutcome, OperationResult};
use crate::app::state::AppState;
use crate::config::write::{DraftAccount, SecretStorage};
use crate::discovery::{
    ConfigSource, DiscoveredService, Provider, Security, ServerEndpoint, derive_aliases,
};

/// The wizard draft the operations carry (ADR 0003 §3.7): the writer's
/// account shape plus nothing else — secrets ride boxed and are
/// redacted in any `Debug` output via [`SecretStorage`].
pub type DraftAccountConfig = DraftAccount;

/// One wizard screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    /// W1: enter the email address.
    Email,
    /// W2: ranked discovery results (+ the manual-override sub-form).
    Discovery,
    /// W3: display name and a recap of the chosen servers.
    Identity,
    /// W4: username and password storage mode.
    Credentials,
    /// W5: the credential test is running.
    Testing,
    /// W6: derived aliases and the save confirmation.
    Confirm,
    /// W7: the account was saved.
    Saved,
}

/// How the account secret is stored (ADR 0003 §3.2 W4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageMode {
    /// Store the password itself (`password.raw`, 0600 file).
    Raw,
    /// Store a command producing the secret (`password.cmd`).
    Command,
}

/// A single-line text field with an explicit caret (the same edit
/// semantics as the dialog fields).
#[derive(Clone, PartialEq, Eq, Default)]
pub struct TextField {
    pub value: String,
    pub cursor: usize,
    /// The field holds a secret: `Debug` redacts the value so the
    /// wizard state can never leak it through a log line (ADR 0003
    /// §3.7: tracing never logs secrets).
    secret: bool,
}

impl std::fmt::Debug for TextField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.secret && !self.value.is_empty() {
            f.debug_struct("TextField")
                .field("value", &"███")
                .field("cursor", &self.cursor)
                .finish()
        } else {
            f.debug_struct("TextField")
                .field("value", &self.value)
                .field("cursor", &self.cursor)
                .finish()
        }
    }
}

impl TextField {
    fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.chars().count();
        Self {
            value,
            cursor,
            secret: false,
        }
    }

    /// A password-style field: rendered masked and Debug-redacted.
    fn secret(value: impl Into<String>) -> Self {
        let mut field = Self::new(value);
        field.secret = true;
        field
    }

    fn apply(&mut self, edit: &DialogEdit) {
        match edit {
            DialogEdit::Char(c) => {
                let byte = self
                    .value
                    .char_indices()
                    .nth(self.cursor)
                    .map(|(byte, _)| byte)
                    .unwrap_or(self.value.len());
                self.value.insert(byte, *c);
                self.cursor += 1;
            }
            DialogEdit::Backspace => {
                if self.cursor > 0 {
                    let byte = self
                        .value
                        .char_indices()
                        .nth(self.cursor - 1)
                        .map(|(byte, _)| byte)
                        .unwrap_or(0);
                    let end = self
                        .value
                        .char_indices()
                        .nth(self.cursor)
                        .map(|(byte, _)| byte)
                        .unwrap_or(self.value.len());
                    self.value.replace_range(byte..end, "");
                    self.cursor -= 1;
                }
            }
            DialogEdit::Delete => {
                let start = self
                    .value
                    .char_indices()
                    .nth(self.cursor)
                    .map(|(byte, _)| byte)
                    .unwrap_or(self.value.len());
                let end = self
                    .value
                    .char_indices()
                    .nth(self.cursor + 1)
                    .map(|(byte, _)| byte)
                    .unwrap_or(self.value.len());
                self.value.replace_range(start..end, "");
            }
            DialogEdit::CursorLeft => self.cursor = self.cursor.saturating_sub(1),
            DialogEdit::CursorRight => {
                let len = self.value.chars().count();
                self.cursor = (self.cursor + 1).min(len);
            }
        }
    }
}

/// The manual-override form fields (ADR 0003 §3.2 W2): IMAP URL, SMTP
/// URL, username — in Tab order.
pub const OVERRIDE_FIELDS: usize = 3;

/// The W4 credential fields in Tab order: username, then the
/// storage-mode toggle, then the secret field (feedback: the storage
/// choice comes before the secret it governs).
pub const CREDENTIAL_FIELDS: usize = 3;

/// How the W6 confirm screen resolves an account-name collision
/// (ADR 0003 §3.6): replace the existing block or pick `-2`, `-3`….
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameChoice {
    Replace,
    Suffix(String),
}

/// The wizard's whole state. Lives in `AppState.wizard` while active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WizardState {
    pub step: WizardStep,
    /// Manual `--configure` mode (ADR 0003 §3.1): completion and cancel
    /// exit the app instead of continuing into the mailbox UI.
    pub manual: bool,
    /// The resolved save target, shown on the confirm screen.
    pub save_path: Option<PathBuf>,
    /// Account names already in the config file (snapshotted at wizard
    /// startup; only the wizard writes the file during the session).
    pub existing_names: Vec<String>,
    /// The name of the account holding `default = true` in the file, if
    /// any (snapshot; see `will_set_default`).
    pub existing_default_name: Option<String>,
    /// Whether the existing file is group/world-readable (warning when
    /// a raw password is about to join it).
    pub existing_shared_readable: bool,

    // W1
    pub email: TextField,
    // W2
    pub services: Vec<DiscoveredService>,
    pub service_index: usize,
    pub discovering: bool,
    pub override_open: bool,
    pub override_fields: [TextField; OVERRIDE_FIELDS],
    pub override_index: usize,
    // W3
    pub display_name: TextField,
    // W4
    pub username: TextField,
    pub storage_mode: StorageMode,
    pub password: TextField,
    pub command: TextField,
    pub credentials_index: usize,
    /// The sanitized failure of the last discovery/test attempt.
    pub last_error: Option<String>,
    /// The Gmail app-password hint applies (gmail.com/googlemail.com
    /// domain or a Google provider tag).
    pub gmail_hint: bool,
    // W5
    // (testing is implied by `step == Testing`; the in-flight id is here)
    // W6
    pub mailbox_names: Vec<String>,
    pub aliases: Vec<(String, String)>,
    pub account_name: String,
    pub name_choice: Option<NameChoice>,
    pub name_choice_index: usize,
    // W7
    pub saved_path: Option<PathBuf>,
    pub saved_created: bool,
    /// The writer's warning for the just-saved file (shared-readable
    /// config + `password.raw`), shown on W7.
    pub permissions_warning: Option<String>,
    /// The id of the operation currently in flight (discovery, test, or
    /// save) — `Esc` cancels it.
    pub in_flight: Option<OperationId>,
    // Terminal outcomes.
    pub cancelled: bool,
    pub completed: bool,
}

impl WizardState {
    /// A fresh wizard on the email screen. `save_path` and the file
    /// snapshots come from startup (I/O stays out of the reducer).
    pub fn new(
        manual: bool,
        save_path: Option<PathBuf>,
        existing_names: Vec<String>,
        existing_default_name: Option<String>,
        existing_shared_readable: bool,
    ) -> Self {
        Self {
            step: WizardStep::Email,
            manual,
            save_path,
            existing_names,
            existing_default_name,
            existing_shared_readable,
            email: TextField::default(),
            services: Vec::new(),
            service_index: 0,
            discovering: false,
            override_open: false,
            override_fields: Default::default(),
            override_index: 0,
            display_name: TextField::default(),
            username: TextField::default(),
            storage_mode: StorageMode::Raw,
            password: TextField::secret(""),
            command: TextField::default(),
            credentials_index: 0,
            last_error: None,
            gmail_hint: false,
            mailbox_names: Vec::new(),
            aliases: Vec::new(),
            account_name: String::new(),
            name_choice: None,
            name_choice_index: 0,
            saved_path: None,
            saved_created: false,
            permissions_warning: None,
            in_flight: None,
            cancelled: false,
            completed: false,
        }
    }

    /// The chosen discovery candidate, if any.
    pub fn selected_service(&self) -> Option<&DiscoveredService> {
        self.services.get(self.service_index)
    }

    /// The provider the final account will carry (manual override
    /// included): host-derived tags included.
    pub fn provider(&self) -> Option<Provider> {
        self.selected_service().and_then(|service| service.provider)
    }

    /// Whether the wizard is waiting on an operation it can cancel.
    pub fn is_busy(&self) -> bool {
        self.discovering || self.step == WizardStep::Testing || self.in_flight.is_some()
    }

    /// The `default = true` decision the confirm screen previews
    /// (ADR 0003 §3.6): set only when no other account already has it.
    /// Replacing the file's default account keeps it default. (The
    /// writer recomputes the authoritative decision at save time; this
    /// preview never contradicts it.)
    pub fn will_set_default(&self) -> bool {
        match &self.existing_default_name {
            None => true,
            Some(name) => name == &self.account_name,
        }
    }

    /// Builds the draft account from the current fields (ADR 0003
    /// §3.4): used both for the credential test and the final save.
    pub fn build_draft(&self) -> Option<DraftAccountConfig> {
        let service = self.selected_service()?;
        let email = self.email.value.trim().to_string();
        if email.is_empty() {
            return None;
        }
        let display_name = self.display_name.value.trim().to_string();
        let secret = match self.storage_mode {
            StorageMode::Raw => SecretStorage::Raw(self.password.value.clone()),
            StorageMode::Command => SecretStorage::Command(self.command.value.clone()),
        };
        Some(DraftAccountConfig {
            name: self.account_name.clone(),
            email,
            display_name: (!display_name.is_empty()).then_some(display_name),
            imap_server: service.imap.url.clone(),
            imap_starttls: service.imap.security == Security::StartTls,
            smtp_server: service.smtp.as_ref()?.url.clone(),
            smtp_starttls: service
                .smtp
                .as_ref()
                .is_some_and(|smtp| smtp.security == Security::StartTls),
            username: self.username.value.clone(),
            secret,
            aliases: self.aliases.clone(),
        })
    }
}

/// Wizard input (ADR 0003 §3.7). Text edits reuse the dialog edit
/// vocabulary; Enter-derived submissions and `Esc` step-backs map to
/// the dedicated variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WizardAction {
    /// Edit the focused text field (email, display name, credentials,
    /// or the override form).
    Edit(DialogEdit),
    /// `↑/↓`: move the discovery selection, override field, credential
    /// field, or name-choice row.
    MoveUp,
    MoveDown,
    /// `Tab`/`Shift+Tab` cycle fields within the current screen.
    FocusNext,
    FocusPrevious,
    /// W1 `Enter`: validate the address and start discovery.
    SubmitEmail,
    /// W2 `Enter`: accept the selected candidate (or the highlighted
    /// override field's submit).
    SelectService,
    /// W2 `r` (or a fresh submit on an empty result): run discovery
    /// again for the same address.
    RerunDiscovery,
    /// W2 `e`: open the manual-override form.
    OverrideServers,
    /// W2 override form `Enter` on the last field: use the typed
    /// servers.
    SubmitOverride,
    /// W2 override form `Esc`: close the form, keep the results list.
    CancelOverride,
    /// W3 `Enter` / W4 `Enter`: advance (W4 validates and tests).
    SubmitCredentials,
    /// W4: flip raw ↔ command storage.
    ToggleStorageMode,
    /// W6 `Enter`: save (honoring the highlighted collision choice).
    ConfirmSave,
    /// W7 `Enter`/`Esc`: finish — continue into the app (first-run) or
    /// exit (manual).
    DismissSaved,
    /// `Esc`: step back one screen; from W1 cancel the wizard.
    Cancel,
}

/// Wizard slice of the reducer: every action while
/// `AppState.wizard` is `Some`. Warmup and mailbox keys never reach it.
pub fn wizard_reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    let Action::Wizard(w) = action else {
        return match action {
            // Results, clock, size, and quit stay live during the wizard;
            // everything else (mailbox navigation, composer, modals) is
            // swallowed so no mailbox key can leak into the wizard.
            Action::BackendCompleted(result) => wizard_completed(state, result),
            Action::Tick { .. } | Action::Resize { .. } | Action::Quit => {
                crate::app::reducer::reduce_unwizarded(state, action)
            }
            // Structural keys arrive as keymap actions (Esc/Enter/Tab stay
            // rebindable — ADR 0003 §3.2: keymap defaults apply inside the
            // wizard context); translate them into the wizard vocabulary.
            Action::BackOrCancel => wizard_action(state, &WizardAction::Cancel),
            Action::Activate => {
                let Some(current) = state.wizard.as_ref() else {
                    return Vec::new();
                };
                let enter = match current.step {
                    WizardStep::Email => WizardAction::SubmitEmail,
                    WizardStep::Discovery if current.override_open => WizardAction::SubmitOverride,
                    WizardStep::Discovery => WizardAction::SelectService,
                    WizardStep::Identity => WizardAction::SubmitCredentials,
                    // Enter activates the focused control (ADR 0003 §3.2):
                    // on the storage-mode row that is the toggle, not the
                    // credential test.
                    WizardStep::Credentials if current.credentials_index == 1 => {
                        WizardAction::ToggleStorageMode
                    }
                    WizardStep::Credentials => WizardAction::SubmitCredentials,
                    WizardStep::Testing => return Vec::new(),
                    WizardStep::Confirm => WizardAction::ConfirmSave,
                    WizardStep::Saved => WizardAction::DismissSaved,
                };
                wizard_action(state, &enter)
            }
            Action::FocusNext => wizard_action(state, &WizardAction::FocusNext),
            Action::FocusPrevious => wizard_action(state, &WizardAction::FocusPrevious),
            Action::MoveUp => wizard_action(state, &WizardAction::MoveUp),
            Action::MoveDown => wizard_action(state, &WizardAction::MoveDown),
            // Warmup suppression (ADR 0003 §3.1): no account yet, so the
            // timer's Refresh and the startup LoadDrafts are inert.
            Action::Refresh | Action::LoadDrafts => Vec::new(),
            _ => Vec::new(),
        };
    };
    wizard_action(state, w)
}

fn wizard_action(state: &mut AppState, action: &WizardAction) -> Vec<Effect> {
    // The wizard is the only writer of its own state; take it out to
    // satisfy the borrow checker across the step transitions.
    let mut wizard = state.wizard.take().expect("wizard active");

    let effects = match action {
        WizardAction::Edit(edit) => {
            edit_wizard_field(&mut wizard, edit);
            Vec::new()
        }
        WizardAction::MoveUp => {
            move_wizard_selection(&mut wizard, -1);
            Vec::new()
        }
        WizardAction::MoveDown => {
            move_wizard_selection(&mut wizard, 1);
            Vec::new()
        }
        WizardAction::FocusNext => {
            cycle_wizard_field(&mut wizard, 1);
            Vec::new()
        }
        WizardAction::FocusPrevious => {
            cycle_wizard_field(&mut wizard, -1);
            Vec::new()
        }
        WizardAction::SubmitEmail => submit_email(&mut wizard, state),
        WizardAction::SelectService => {
            if wizard.override_open {
                submit_override(&mut wizard)
            } else {
                select_service(&mut wizard)
            }
        }
        WizardAction::OverrideServers => {
            if wizard.step == WizardStep::Discovery && !wizard.override_open {
                open_override(&mut wizard);
                Vec::new()
            } else {
                // The key arrived on a text-entry step: it is a typed
                // character, not the override shortcut.
                edit_wizard_field(&mut wizard, &DialogEdit::Char('e'));
                Vec::new()
            }
        }
        WizardAction::RerunDiscovery => {
            if wizard.step == WizardStep::Discovery && !wizard.override_open {
                wizard.last_error = None;
                wizard.discovering = true;
                wizard.services.clear();
                wizard.service_index = 0;
                let email = wizard.email.value.trim().to_string();
                let effect = state
                    .operations
                    .start(OperationKind::DiscoverConfig { email });
                wizard.in_flight = Some(effect.id);
                vec![effect]
            } else {
                edit_wizard_field(&mut wizard, &DialogEdit::Char('r'));
                Vec::new()
            }
        }
        WizardAction::SubmitOverride => submit_override(&mut wizard),
        WizardAction::CancelOverride => {
            wizard.override_open = false;
            Vec::new()
        }
        WizardAction::SubmitCredentials => submit_credentials(&mut wizard, state),
        WizardAction::ToggleStorageMode => {
            if wizard.step == WizardStep::Credentials && wizard.credentials_index == 1 {
                // The storage-mode row is focused: flip the mode.
                wizard.storage_mode = match wizard.storage_mode {
                    StorageMode::Raw => StorageMode::Command,
                    StorageMode::Command => StorageMode::Raw,
                };
            } else if wizard_text_entry_focused(&wizard) {
                // The key arrived on a text-entry step (Space is the
                // checkbox convention, but it is a typed character
                // everywhere else — commands and display names may
                // contain spaces).
                edit_wizard_field(&mut wizard, &DialogEdit::Char(' '));
            }
            Vec::new()
        }
        WizardAction::ConfirmSave => confirm_save(&mut wizard, state),
        WizardAction::DismissSaved => dismiss_saved(&mut wizard, state),
        WizardAction::Cancel => cancel_wizard(&mut wizard, state),
    };

    state.wizard = Some(wizard);
    effects
}

// ── Field editing / focus ────────────────────────────────────────────────

/// Whether the current screen holds a focused text field that would
/// receive a typed character.
fn wizard_text_entry_focused(wizard: &WizardState) -> bool {
    match wizard.step {
        WizardStep::Email => true,
        WizardStep::Discovery => wizard.override_open,
        WizardStep::Identity => true,
        WizardStep::Credentials => wizard.credentials_index != 1,
        WizardStep::Testing | WizardStep::Confirm | WizardStep::Saved => false,
    }
}

fn edit_wizard_field(wizard: &mut WizardState, edit: &DialogEdit) {
    match wizard.step {
        WizardStep::Email => wizard.email.apply(edit),
        WizardStep::Discovery if wizard.override_open => {
            wizard.override_fields[wizard.override_index].apply(edit)
        }
        WizardStep::Identity => wizard.display_name.apply(edit),
        WizardStep::Credentials => match wizard.credentials_index {
            0 => wizard.username.apply(edit),
            // The storage toggle (1) is not a text field.
            2 => match wizard.storage_mode {
                StorageMode::Raw => wizard.password.apply(edit),
                StorageMode::Command => wizard.command.apply(edit),
            },
            _ => {}
        },
        _ => {}
    }
}

fn cycle_wizard_field(wizard: &mut WizardState, delta: i64) {
    let cycle = |index: &mut usize, len: usize| {
        *index = (*index as i64 + delta).rem_euclid(len as i64) as usize;
    };
    match wizard.step {
        WizardStep::Discovery if wizard.override_open => {
            cycle(&mut wizard.override_index, OVERRIDE_FIELDS)
        }
        WizardStep::Credentials => cycle(&mut wizard.credentials_index, CREDENTIAL_FIELDS),
        _ => {}
    }
}

fn move_wizard_selection(wizard: &mut WizardState, delta: i64) {
    match wizard.step {
        WizardStep::Discovery if !wizard.services.is_empty() && !wizard.override_open => {
            let len = wizard.services.len();
            wizard.service_index =
                (wizard.service_index as i64 + delta).rem_euclid(len as i64) as usize;
        }
        WizardStep::Confirm if wizard.name_choice.is_some() => {
            // Two rows only: replace ↔ suffix.
            wizard.name_choice_index =
                (wizard.name_choice_index as i64 + delta).rem_euclid(2) as usize;
        }
        _ => {}
    }
}

// ── Step transitions ─────────────────────────────────────────────────────

/// W1 validation (ADR 0003 §3.2): non-empty, exactly one `@`, non-empty
/// local part and domain. Full RFC parsing is overkill.
pub fn validate_email(email: &str) -> Result<(), String> {
    if email.is_empty() {
        return Err(String::from("enter an email address"));
    }
    let parts: Vec<&str> = email.split('@').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(format!("“{email}” is not a valid email address"));
    }
    Ok(())
}

fn submit_email(wizard: &mut WizardState, state: &mut AppState) -> Vec<Effect> {
    if wizard.step != WizardStep::Email {
        return Vec::new();
    }
    let email = wizard.email.value.trim().to_string();
    if let Err(message) = validate_email(&email) {
        wizard.last_error = Some(message);
        return Vec::new();
    }
    wizard.last_error = None;
    wizard.gmail_hint = is_google_domain(&email);
    wizard.step = WizardStep::Discovery;
    wizard.discovering = true;
    wizard.services.clear();
    wizard.service_index = 0;
    let effect = state.operations.start(OperationKind::DiscoverConfig {
        email: email.clone(),
    });
    wizard.in_flight = Some(effect.id);
    vec![effect]
}

fn is_google_domain(email: &str) -> bool {
    email
        .split('@')
        .nth(1)
        .map(|domain| {
            domain.eq_ignore_ascii_case("gmail.com")
                || domain.eq_ignore_ascii_case("googlemail.com")
        })
        .unwrap_or(false)
}

/// W2 `Enter`: accept the selected candidate into W3 (or, on the
/// empty-result screen, the manual form's data was already folded in).
fn select_service(wizard: &mut WizardState) -> Vec<Effect> {
    if wizard.step != WizardStep::Discovery
        || wizard.override_open
        || wizard.discovering
        || wizard.services.is_empty()
    {
        return Vec::new();
    }
    // Prefill the username from discovery's advertised login, else the
    // address (ADR 0003 §3.2 W4).
    let username = wizard
        .selected_service()
        .and_then(|service| service.username.clone())
        .unwrap_or_else(|| wizard.email.value.trim().to_string());
    wizard.username = TextField::new(username);
    // Default suggestion for the display name: the local part.
    let local = wizard
        .email
        .value
        .trim()
        .split('@')
        .next()
        .unwrap_or("")
        .to_string();
    wizard.display_name = TextField::new(local);
    wizard.step = WizardStep::Identity;
    Vec::new()
}

/// W2 `e`: open the manual-override form with guessed defaults
/// (ADR 0003 §3.2): `imaps://imap.<domain>:993`, `smtps://smtp.<domain>:465`,
/// username = the full address.
fn open_override(wizard: &mut WizardState) {
    let email = wizard.email.value.trim().to_string();
    let domain = email.split('@').nth(1).unwrap_or("example.com").to_string();
    wizard.override_open = true;
    wizard.override_index = 0;
    wizard.override_fields = [
        TextField::new(format!("imaps://imap.{domain}:993")),
        TextField::new(format!("smtps://smtp.{domain}:465")),
        TextField::new(email),
    ];
    wizard.last_error = None;
}

/// W2 override submit: the typed servers become a `source = manual`
/// candidate and skip straight to W4 (ADR 0003 §3.2).
fn submit_override(wizard: &mut WizardState) -> Vec<Effect> {
    if wizard.step != WizardStep::Discovery || !wizard.override_open {
        return Vec::new();
    }
    let [imap, smtp, username] = &wizard.override_fields;
    let (imap_url, imap_security) = match parse_server_url(&imap.value) {
        Ok(parsed) => parsed,
        Err(message) => {
            wizard.last_error = Some(message);
            return Vec::new();
        }
    };
    let (smtp_url, smtp_security) = match parse_server_url(&smtp.value) {
        Ok(parsed) => parsed,
        Err(message) => {
            wizard.last_error = Some(message);
            return Vec::new();
        }
    };
    if username.value.trim().is_empty() {
        wizard.last_error = Some(String::from("enter the login username"));
        return Vec::new();
    }
    let provider = host_provider(&imap_url);
    wizard.services = vec![DiscoveredService {
        source: ConfigSource::Manual,
        imap: ServerEndpoint {
            url: imap_url,
            security: imap_security,
        },
        smtp: Some(ServerEndpoint {
            url: smtp_url,
            security: smtp_security,
        }),
        provider,
        username: Some(username.value.trim().to_string()),
    }];
    wizard.service_index = 0;
    wizard.override_open = false;
    // Manual override skips W3 (ADR 0003 §3.2): the display name stays
    // empty and is omitted from the saved account.
    wizard.username = TextField::new(username.value.trim().to_string());
    wizard.display_name = TextField::new("");
    wizard.step = WizardStep::Credentials;
    Vec::new()
}

/// Parses a himalaya server URL: `scheme://host:port` where scheme is
/// `imap(s)` / `smtp(s)`. Returns the normalized URL and the security
/// it implies.
fn parse_server_url(raw: &str) -> Result<(String, Security), String> {
    let raw = raw.trim();
    let (scheme, rest) = raw
        .split_once("://")
        .ok_or_else(|| format!("“{raw}” needs a scheme like imaps://host:993"))?;
    let security = match scheme {
        "imaps" | "smtps" => Security::Tls,
        "imap" | "smtp" => Security::StartTls,
        other => return Err(format!("unknown scheme “{other}” (use imap(s) or smtp(s))")),
    };
    if rest.is_empty() || rest.contains('/') || !rest.contains(':') {
        return Err(format!(
            "“{raw}” needs host and port, e.g. {scheme}://host:993"
        ));
    }
    Ok((format!("{scheme}://{rest}"), security))
}

/// Infers the provider from a server URL's host (manual override).
fn host_provider(url: &str) -> Option<Provider> {
    let host = url.split("://").nth(1)?;
    let host = host.split(':').next().unwrap_or(host);
    Provider::from_host(host)
}

/// W4 `Enter`: validate the fields and run the credential test
/// (ADR 0003 §3.4) against a temporary config.
fn submit_credentials(wizard: &mut WizardState, state: &mut AppState) -> Vec<Effect> {
    if wizard.step == WizardStep::Identity {
        // W3 `Enter`: the display name is set; advance to the
        // credentials screen.
        wizard.step = WizardStep::Credentials;
        return Vec::new();
    }
    if wizard.step != WizardStep::Credentials {
        return Vec::new();
    }
    if wizard.username.value.trim().is_empty() {
        wizard.last_error = Some(String::from("enter the login username"));
        return Vec::new();
    }
    match &wizard.storage_mode {
        StorageMode::Raw if wizard.password.value.is_empty() => {
            wizard.last_error = Some(String::from("enter the password, or switch to a command"));
            return Vec::new();
        }
        StorageMode::Command => {
            let command = wizard.command.value.trim().to_string();
            if command.is_empty() {
                wizard.last_error =
                    Some(String::from("enter the command that prints the password"));
                return Vec::new();
            }
            if let Err(message) = validate_command(&command) {
                wizard.last_error = Some(message);
                return Vec::new();
            }
        }
        StorageMode::Raw => {}
    }

    // The account id is fixed before the test so the temp config and
    // the final save carry the same name.
    let email = wizard.email.value.trim().to_string();
    let domain = email.split('@').nth(1).unwrap_or("").to_string();
    wizard.account_name = crate::config::write::sanitize_account_name(&domain);
    wizard.aliases.clear();

    let Some(draft) = wizard.build_draft() else {
        wizard.last_error = Some(String::from(
            "the candidate has no SMTP server; edit it (e)",
        ));
        return Vec::new();
    };

    wizard.last_error = None;
    wizard.step = WizardStep::Testing;
    let effect = state.operations.start(OperationKind::TestAccount {
        draft: Box::new(draft),
    });
    wizard.in_flight = Some(effect.id);
    vec![effect]
}

/// `password.cmd` validation, mirroring the editor command rule
/// (ADR 0003 §3.2 W4): Tmail never spawns a shell, so shell
/// metacharacters are rejected; the program must exist.
fn validate_command(command: &str) -> Result<(), String> {
    const FORBIDDEN: [char; 10] = ['|', '&', ';', '<', '>', '`', '$', '\\', '"', '\''];
    if command.chars().any(|c| FORBIDDEN.contains(&c)) {
        return Err(String::from(
            "the command must be a plain command without shell metacharacters",
        ));
    }
    let Some(program) = command.split_whitespace().next() else {
        return Err(String::from("the command is empty"));
    };
    if crate::config::program_exists(program) {
        Ok(())
    } else if program.contains('/') {
        Err(format!("program {program:?} does not exist"))
    } else {
        Err(format!("program {program:?} was not found on PATH"))
    }
}

/// W6 `Enter`: save the account into the resolved config file.
fn confirm_save(wizard: &mut WizardState, state: &mut AppState) -> Vec<Effect> {
    if wizard.step != WizardStep::Confirm {
        return Vec::new();
    }
    // The collision choice may rename the account (ADR 0003 §3.6).
    if wizard.name_choice_index == 1
        && let Some(NameChoice::Suffix(suffix)) = wizard.name_choice.clone()
    {
        wizard.account_name = suffix;
    }
    let Some(path) = wizard.save_path.clone() else {
        wizard.last_error = Some(String::from("no config file path could be resolved"));
        return Vec::new();
    };
    let Some(mut draft) = wizard.build_draft() else {
        wizard.last_error = Some(String::from("the draft is incomplete; go back and retry"));
        return Vec::new();
    };
    draft.name = wizard.account_name.clone();
    draft.aliases = wizard.aliases.clone();

    wizard.last_error = None;
    let effect = state.operations.start(OperationKind::SaveAccount {
        path,
        draft: Box::new(draft),
    });
    wizard.in_flight = Some(effect.id);
    vec![effect]
}

/// W7 `Enter`/`Esc`: finish — continue into the mailbox UI (first-run)
/// or exit (manual mode). Either way the session loop ends: main.rs
/// decides between exit and an in-process restart with the fresh config.
fn dismiss_saved(wizard: &mut WizardState, state: &mut AppState) -> Vec<Effect> {
    if wizard.step != WizardStep::Saved {
        return Vec::new();
    }
    wizard.completed = true;
    state.quit_requested = true;
    Vec::new()
}

/// `Esc`: step back one screen; from the first step cancel the wizard
/// (ADR 0003 §3.2). A busy operation is cancelled first.
fn cancel_wizard(wizard: &mut WizardState, state: &mut AppState) -> Vec<Effect> {
    // Override form first: Esc closes the form, keeping the results.
    if wizard.step == WizardStep::Discovery && wizard.override_open {
        wizard.override_open = false;
        return Vec::new();
    }
    match wizard.step {
        WizardStep::Email => {
            wizard.cancelled = true;
            state.quit_requested = true;
        }
        WizardStep::Discovery => {
            wizard.step = WizardStep::Email;
            wizard.last_error = None;
        }
        WizardStep::Identity => {
            wizard.step = WizardStep::Discovery;
        }
        WizardStep::Credentials => {
            wizard.step = if wizard.services.is_empty() {
                // Nothing to go back to: the only way forward was the
                // manual override, entered from discovery.
                WizardStep::Discovery
            } else {
                WizardStep::Identity
            };
            wizard.last_error = None;
        }
        WizardStep::Testing => {
            // Cancel the test and return to the credentials screen with
            // the fields intact (ADR 0003 §3.2 W5).
            if let Some(id) = wizard.in_flight.take() {
                state.operations.cancel(id);
            }
            wizard.step = WizardStep::Credentials;
        }
        WizardStep::Confirm if wizard.in_flight.is_some() => {
            // The save is in flight: Esc cancels it, staying on the
            // confirm screen.
            if let Some(id) = wizard.in_flight.take() {
                state.operations.cancel(id);
            }
        }
        WizardStep::Confirm => {
            wizard.step = WizardStep::Credentials;
            wizard.last_error = None;
        }
        WizardStep::Saved => {
            dismiss_saved(wizard, state);
        }
    }
    Vec::new()
}

// ── Operation results ────────────────────────────────────────────────────

fn wizard_completed(state: &mut AppState, result: &OperationResult) -> Vec<Effect> {
    // Reject unknown, cancelled, or superseded ids (plan §11): the
    // operation is finished out of the registry first.
    let Some(op) = state.operations.finish(result.id) else {
        return Vec::new();
    };
    let kind = op.kind;
    let Some(wizard) = state.wizard.as_mut() else {
        return Vec::new();
    };
    if wizard.in_flight != Some(result.id) {
        return Vec::new();
    }
    wizard.in_flight = None;
    match (&kind, &result.outcome) {
        (OperationKind::DiscoverConfig { .. }, Ok(OperationOutcome::Discovered(services))) => {
            wizard.discovering = false;
            wizard.services = services.clone();
            wizard.service_index = 0;
            if wizard.services.is_empty() {
                // Nothing found: the manual-override form is the only
                // way forward (ADR 0003 §3.2 W2).
                wizard.last_error = None;
                open_override(wizard);
            }
            Vec::new()
        }
        (OperationKind::DiscoverConfig { .. }, Err(failure)) => {
            wizard.discovering = false;
            wizard.last_error = Some(failure.detail.clone());
            Vec::new()
        }
        (
            OperationKind::TestAccount { .. },
            Ok(OperationOutcome::TestAccountCompleted { mailboxes }),
        ) => {
            wizard.mailbox_names = mailboxes.clone();
            wizard.aliases = derive_aliases(&wizard.mailbox_names, wizard.provider());
            prepare_confirm(wizard);
            wizard.step = WizardStep::Confirm;
            Vec::new()
        }
        (OperationKind::TestAccount { .. }, Err(failure)) => {
            // Back to W4 with the sanitized error, fields intact
            // (ADR 0003 §3.2 W5).
            wizard.step = WizardStep::Credentials;
            wizard.last_error = Some(failure.detail.clone());
            Vec::new()
        }
        (
            OperationKind::SaveAccount { .. },
            Ok(OperationOutcome::AccountSaved {
                path,
                created,
                permissions_warning,
            }),
        ) => {
            wizard.saved_path = Some(path.clone());
            wizard.saved_created = *created;
            wizard.permissions_warning = permissions_warning.clone();
            wizard.step = WizardStep::Saved;
            Vec::new()
        }
        (OperationKind::SaveAccount { .. }, Err(failure)) => {
            wizard.step = WizardStep::Confirm;
            wizard.last_error = Some(failure.detail.clone());
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// Computes the W6 confirmation data (ADR 0003 §3.6): the account name
/// (collision-suffix suggestion), the derived alias preview, and the
/// `default` decision.
fn prepare_confirm(wizard: &mut WizardState) {
    if wizard.account_name.is_empty() {
        let email = wizard.email.value.trim().to_string();
        let domain = email.split('@').nth(1).unwrap_or("");
        wizard.account_name = crate::config::write::sanitize_account_name(domain);
    }
    let collides = wizard.existing_names.contains(&wizard.account_name);
    wizard.name_choice = if collides {
        let suffix = next_free_name(&wizard.existing_names, &wizard.account_name);
        wizard.name_choice_index = 0;
        Some(NameChoice::Suffix(suffix))
    } else {
        wizard.name_choice_index = 0;
        None
    };
}

/// The wizard's collision rule (ADR 0003 §3.6): `base`, then `base-2`,
/// `base-3`, …
pub fn next_free_name(existing: &[String], base: &str) -> String {
    if !existing.iter().any(|name| name == base) {
        return base.to_string();
    }
    for suffix in 2.. {
        let candidate = format!("{base}-{suffix}");
        if !existing.iter().any(|name| name == &candidate) {
            return candidate;
        }
    }
    unreachable!("suffix loop always returns")
}

#[cfg(test)]
mod tests;
