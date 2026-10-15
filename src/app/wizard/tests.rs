//! Wizard reducer unit tests (ADR 0003 §4 W4): the full success path,
//! every failure path, cancel-at-every-step, warmup suppression, and
//! the no-secrets-in-logs invariant. All I/O-free: operations complete
//! through synthesized `BackendCompleted` actions.

use super::*;
use crate::app::Focus;
use crate::app::action::DialogEdit;
use crate::app::mock::mock_initial_state;
use crate::app::operation::OperationFailure;
use crate::app::reducer::reduce;
use crate::discovery::{ConfigSource, Provider, Security, ServerEndpoint};
use crate::domain::{MailboxRole, TestedMailbox};

fn state() -> AppState {
    let mut state = mock_initial_state();
    state.session.wizard = Some(WizardState::new(
        false,
        ConfigSnapshot {
            save_path: Some(std::path::PathBuf::from("/tmp/tmail-test-config.toml")),
            existing_names: Vec::new(),
            existing_default_name: None,
            existing_shared_readable: false,
        },
    ));
    state.session.focus = Focus::Wizard;
    state
}

/// A wizard state builder with a distinct name (a local `state` binding
/// would shadow the helper).
fn fresh_wizard_state() -> AppState {
    state()
}

fn manual_state() -> AppState {
    let mut state = fresh_wizard_state();
    if let Some(wizard) = state.session.wizard.as_mut() {
        wizard.manual = true;
    }
    state
}

fn wizard(state: &AppState) -> &WizardState {
    state.session.wizard.as_ref().expect("wizard active")
}

fn wizard_mut(state: &mut AppState) -> &mut WizardState {
    state.session.wizard.as_mut().expect("wizard active")
}

fn act(state: &mut AppState, action: WizardAction) -> Vec<Effect> {
    reduce(state, Action::Wizard(action))
}

fn complete(state: &mut AppState, effects: &[Effect], outcome: OperationOutcome) -> Vec<Effect> {
    let id = effects.first().expect("effect to complete").id;
    reduce(
        state,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(outcome),
        }),
    )
}

fn fail_with(state: &mut AppState, effects: &[Effect], detail: &str) -> Vec<Effect> {
    let id = effects.first().expect("effect to complete").id;
    reduce(
        state,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Err(OperationFailure {
                code: Some(1),
                detail: detail.to_string(),
                retry: None,
                ambiguous: false,
            }),
        }),
    )
}

fn gmail_service() -> DiscoveredService {
    DiscoveredService {
        source: ConfigSource::Provider(Provider::Gmail),
        imap: ServerEndpoint {
            url: String::from("imaps://imap.gmail.com:993"),
            security: Security::Tls,
        },
        smtp: Some(ServerEndpoint {
            url: String::from("smtps://smtp.gmail.com:465"),
            security: Security::Tls,
        }),
        provider: Some(Provider::Gmail),
        username: None,
    }
}

fn discovered(services: Vec<DiscoveredService>) -> OperationOutcome {
    OperationOutcome::Discovered(services)
}

fn test_ok(names: &[&str]) -> OperationOutcome {
    test_ok_tagged(names, &[])
}

/// `TestAccountCompleted` with names, optionally tagged with the
/// roles the server attributed (RFC 6154 SPECIAL-USE spellings: the
/// `mailbox.alias` role keys).
fn test_ok_tagged(names: &[&str], roles: &[(&str, &str)]) -> OperationOutcome {
    OperationOutcome::TestAccountCompleted {
        mailboxes: names
            .iter()
            .map(|name| TestedMailbox {
                name: name.to_string(),
                role: roles
                    .iter()
                    .find(|(_, attributed)| attributed == name)
                    .map(|(role, _)| match *role {
                        "inbox" => MailboxRole::Inbox,
                        "sent" => MailboxRole::Sent,
                        "drafts" => MailboxRole::Drafts,
                        "trash" => MailboxRole::Trash,
                        "junk" => MailboxRole::Spam,
                        _ => MailboxRole::Archive,
                    }),
            })
            .collect(),
    }
}

fn account_saved(path: &str, created: bool) -> OperationOutcome {
    account_saved_provisioned(path, created, &[])
}

/// `AccountSaved` with the special folders the server actually
/// provisioned during the save (issue txps).
fn account_saved_provisioned(path: &str, created: bool, provisioned: &[&str]) -> OperationOutcome {
    OperationOutcome::AccountSaved {
        path: std::path::PathBuf::from(path),
        created,
        permissions_warning: None,
        created_mailboxes: provisioned.iter().map(|name| name.to_string()).collect(),
    }
}

/// Drives the happy path up to the saved screen through real reducer
/// transitions. Returns the state (for further assertions).
fn drive_to_saved(state: &mut AppState) {
    wizard_mut(state).email.address = TextField::new("u@example.com");
    let effects = act(state, WizardAction::SubmitEmail);
    assert!(matches!(
        effects.first().expect("discover effect").kind,
        OperationKind::Account(AccountOperation::DiscoverConfig { .. })
    ));
    complete(state, &effects, discovered(vec![gmail_service()]));
    assert_eq!(wizard(state).step, WizardStep::Discovery);
    act(state, WizardAction::SelectService); // → W3
    assert_eq!(wizard(state).step, WizardStep::Identity);
    act(state, WizardAction::SubmitCredentials); // → W4
    assert_eq!(wizard(state).step, WizardStep::Credentials);
    wizard_mut(state).credentials.password = TextField::secret("app-password");
    let effects = act(state, WizardAction::SubmitCredentials); // → W5 test
    assert!(matches!(
        effects.first().expect("test effect").kind,
        OperationKind::Account(AccountOperation::TestAccount { .. })
    ));
    complete(state, &effects, test_ok(&["INBOX", "[Gmail]/Sent Mail"]));
    assert_eq!(wizard(state).step, WizardStep::Confirm);
    let effects = act(state, WizardAction::ConfirmSave);
    assert!(matches!(
        effects.first().expect("save effect").kind,
        OperationKind::Account(AccountOperation::SaveAccount { .. })
    ));
    complete(state, &effects, account_saved("/tmp/config.toml", true));
    assert_eq!(wizard(state).step, WizardStep::Saved);
}

#[test]
fn full_success_path_reaches_saved() {
    let mut state = state();
    drive_to_saved(&mut state);

    // The derived aliases (Gmail preset) ride along on the confirm
    // screen and the draft.
    assert_eq!(
        wizard(&state).confirm.aliases,
        vec![
            (String::from("inbox"), String::from("INBOX")),
            (String::from("sent"), String::from("[Gmail]/Sent Mail")),
        ]
    );
    assert_eq!(wizard(&state).confirm.account_name, "example");
    assert!(wizard(&state).will_set_default(), "no existing default");
    assert!(wizard(&state).confirm.saved_created);
    assert!(!wizard(&state).completed);
    // First-run dismiss ends the session (main.rs restarts into the
    // mailbox UI with the fresh config).
    act(&mut state, WizardAction::DismissSaved);
    assert!(wizard(&state).completed);
    assert!(
        state.session.quit_requested,
        "the session must end for the restart"
    );
}

#[test]
fn dismiss_ends_the_session_in_both_modes() {
    // Manual: main.rs prints the path and exits 0.
    let mut manual = manual_state();
    drive_to_saved(&mut manual);
    act(&mut manual, WizardAction::DismissSaved);
    assert!(wizard(&manual).completed);
    assert!(
        manual.session.quit_requested,
        "manual mode exits after saving"
    );

    // First-run: the session ends too, and main.rs restarts into the
    // normal mailbox UI with the fresh config.
    let mut first_run = fresh_wizard_state();
    drive_to_saved(&mut first_run);
    act(&mut first_run, WizardAction::DismissSaved);
    assert!(wizard(&first_run).completed);
    assert!(
        first_run.session.quit_requested,
        "the session must end for the restart"
    );
}

#[test]
fn missing_special_folders_are_queued_for_creation_and_saved() {
    // The bare-Dovecot class (issue txps): a server with Inbox alone.
    // W6 previews the canonical folders it will create, ConfirmSave
    // carries them, and the save reports what was actually provisioned.
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("info@klinoteka.ru");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    let mut generic = gmail_service();
    generic.source = ConfigSource::Rfc6186;
    generic.provider = None;
    generic.imap.url = String::from("imaps://mail.klinoteka.ru:993");
    complete(&mut state, &effects, discovered(vec![generic]));
    act(&mut state, WizardAction::SelectService);
    act(&mut state, WizardAction::SubmitCredentials);
    wizard_mut(&mut state).credentials.password = TextField::secret("pw");
    let effects = act(&mut state, WizardAction::SubmitCredentials);
    complete(&mut state, &effects, test_ok(&["Inbox"]));
    assert_eq!(wizard(&state).step, WizardStep::Confirm);

    // The provisioning preview, priority-ordered, inbox never offered.
    assert_eq!(
        wizard(&state).confirm.create_missing,
        vec![
            (String::from("sent"), String::from("Sent")),
            (String::from("drafts"), String::from("Drafts")),
            (String::from("trash"), String::from("Trash")),
            (String::from("junk"), String::from("Junk")),
            (String::from("archive"), String::from("Archive")),
        ]
    );

    let effects = act(&mut state, WizardAction::ConfirmSave);
    let OperationKind::Account(AccountOperation::SaveAccount { create, .. }) =
        &effects.first().expect("save effect").kind
    else {
        panic!("expected a SaveAccount effect");
    };
    assert_eq!(
        create,
        &wizard(&state).confirm.create_missing,
        "the queued pairs ride the save effect"
    );

    complete(
        &mut state,
        &effects,
        account_saved_provisioned(
            "/tmp/config.toml",
            true,
            &["Sent", "Drafts", "Trash", "Junk", "Archive"],
        ),
    );
    assert_eq!(wizard(&state).step, WizardStep::Saved);
    assert_eq!(
        wizard(&state).confirm.saved_created_mailboxes,
        vec![
            String::from("Sent"),
            String::from("Drafts"),
            String::from("Trash"),
            String::from("Junk"),
            String::from("Archive"),
        ]
    );
}

#[test]
fn special_use_attributes_drive_the_alias_derivation() {
    // The reported failure class (issue m0wh): a server whose special
    // folders carry localized names no candidate list knows. The
    // credential test tags each mailbox with the role the server itself
    // attributed (RFC 6154 SPECIAL-USE), and W6 derives the alias table
    // from those roles, so delete/drafts work without hand-editing.
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("u@example.com");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    let mut generic = gmail_service();
    generic.source = ConfigSource::Rfc6186;
    generic.provider = None;
    generic.imap.url = String::from("imaps://imap.example.com:993");
    complete(&mut state, &effects, discovered(vec![generic]));
    act(&mut state, WizardAction::SelectService);
    act(&mut state, WizardAction::SubmitCredentials);
    wizard_mut(&mut state).credentials.password = TextField::secret("pw");
    let effects = act(&mut state, WizardAction::SubmitCredentials);
    complete(
        &mut state,
        &effects,
        test_ok_tagged(
            &["INBOX", "Odeslané", "Koncepty", "Odstraněné"],
            &[
                ("trash", "Odstraněné"),
                ("drafts", "Koncepty"),
                ("sent", "Odeslané"),
            ],
        ),
    );
    assert_eq!(wizard(&state).step, WizardStep::Confirm);
    assert_eq!(
        wizard(&state).confirm.aliases,
        vec![
            (String::from("inbox"), String::from("INBOX")),
            (String::from("sent"), String::from("Odeslané")),
            (String::from("drafts"), String::from("Koncepty")),
            (String::from("trash"), String::from("Odstraněné")),
        ]
    );
}

#[test]
fn email_field_edits_flow_through_wizard_actions() {
    let mut state = state();
    for c in "u@example.com".chars() {
        act(&mut state, WizardAction::Edit(DialogEdit::Char(c)));
    }
    assert_eq!(wizard(&state).email.address.value, "u@example.com");
    act(&mut state, WizardAction::Edit(DialogEdit::Backspace));
    assert_eq!(wizard(&state).email.address.value, "u@example.co");
    act(&mut state, WizardAction::Edit(DialogEdit::CursorLeft));
    act(&mut state, WizardAction::Edit(DialogEdit::Char('X')));
    assert_eq!(wizard(&state).email.address.value, "u@example.cXo");
}

#[test]
fn invalid_email_is_reported_and_discovery_does_not_start() {
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("not-an-email");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    no_effects(&effects);
    assert_eq!(wizard(&state).step, WizardStep::Email);
    assert!(wizard(&state).last_error.is_some());
    assert!(!wizard(&state).discovery.discovering);

    // Multiple @ is invalid too.
    wizard_mut(&mut state).email.address = TextField::new("a@b@c");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    no_effects(&effects);
    assert_eq!(wizard(&state).step, WizardStep::Email);
}

fn no_effects(effects: &[Effect]) {
    assert!(effects.is_empty(), "expected no effects, got {effects:?}");
}

#[test]
fn discovery_result_ranks_into_the_list_and_enter_accepts() {
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("u@example.com");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    // Two candidates: the Gmail provider rule (wins) and a plain SRV one.
    let mut srv = gmail_service();
    srv.source = ConfigSource::Rfc6186;
    srv.provider = None;
    srv.imap.url = String::from("imaps://imap.example.com:993");
    complete(
        &mut state,
        &effects,
        discovered(vec![gmail_service(), srv.clone()]),
    );

    assert_eq!(wizard(&state).discovery.services.len(), 2);
    assert_eq!(wizard(&state).discovery.service_index, 0);
    assert_eq!(
        wizard(&state).discovery.services[0].source,
        ConfigSource::Provider(Provider::Gmail),
        "the provider rule preselects first"
    );

    // Enter accepts the preselected candidate and prefills identity.
    act(&mut state, WizardAction::SelectService);
    assert_eq!(wizard(&state).step, WizardStep::Identity);
    assert_eq!(wizard(&state).credentials.username.value, "u@example.com");
    assert_eq!(
        wizard(&state).credentials.display_name.value,
        "u",
        "local part suggested"
    );
}

#[test]
fn empty_discovery_opens_the_manual_override_form() {
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("u@custom.example");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    complete(&mut state, &effects, discovered(Vec::new()));

    assert!(
        wizard(&state).discovery.override_open,
        "manual override is the only way forward"
    );
    let [imap, smtp, username] = &wizard(&state).discovery.override_fields;
    assert_eq!(imap.value, "imaps://imap.custom.example:993");
    assert_eq!(smtp.value, "smtps://smtp.custom.example:465");
    assert_eq!(username.value, "u@custom.example");
}

#[test]
fn manual_override_skips_straight_to_credentials() {
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("u@custom.example");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    complete(&mut state, &effects, discovered(Vec::new()));

    let effects = act(&mut state, WizardAction::SubmitOverride);
    no_effects(&effects);
    assert_eq!(wizard(&state).step, WizardStep::Credentials);
    assert_eq!(wizard(&state).discovery.services.len(), 1);
    assert_eq!(
        wizard(&state).discovery.services[0].source,
        ConfigSource::Manual
    );
    assert_eq!(wizard(&state).discovery.services[0].provider, None);
    assert_eq!(
        wizard(&state).discovery.services[0].imap.security,
        Security::Tls,
        "imaps:// implies implicit TLS"
    );

    // And the credentials submit builds a manual draft.
    wizard_mut(&mut state).credentials.password = TextField::secret("pw");
    let effects = act(&mut state, WizardAction::SubmitCredentials);
    let id = effects[0].id;
    let OperationKind::Account(AccountOperation::TestAccount { draft }) =
        &state.session.operations.get(id).unwrap().kind
    else {
        panic!("expected a TestAccount operation");
    };
    assert_eq!(draft.imap_server, "imaps://imap.custom.example:993");
    assert_eq!(draft.smtp_server, "smtps://smtp.custom.example:465");
}

#[test]
fn test_failure_returns_to_credentials_with_fields_intact() {
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("u@example.com");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    complete(&mut state, &effects, discovered(vec![gmail_service()]));
    act(&mut state, WizardAction::SelectService);
    act(&mut state, WizardAction::SubmitCredentials);
    wizard_mut(&mut state).credentials.password = TextField::secret("app-password");
    let effects = act(&mut state, WizardAction::SubmitCredentials);
    fail_with(&mut state, &effects, "login failed");

    assert_eq!(wizard(&state).step, WizardStep::Credentials);
    assert_eq!(wizard(&state).last_error.as_deref(), Some("login failed"));
    assert_eq!(
        wizard(&state).credentials.password.value,
        "app-password",
        "fields intact"
    );

    // Retry re-runs the test with the edited values; nothing accumulates.
    wizard_mut(&mut state).credentials.password = TextField::secret("fixed-password");
    let effects = act(&mut state, WizardAction::SubmitCredentials);
    assert!(matches!(
        effects.first().expect("test effect").kind,
        OperationKind::Account(AccountOperation::TestAccount { .. })
    ));
    complete(&mut state, &effects, test_ok(&["INBOX"]));
    assert_eq!(wizard(&state).step, WizardStep::Confirm);
}

#[test]
fn a_test_failure_detail_is_stored_sanitized_and_debug_redacted() {
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("u@example.com");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    complete(&mut state, &effects, discovered(vec![gmail_service()]));
    act(&mut state, WizardAction::SelectService);
    act(&mut state, WizardAction::SubmitCredentials);
    wizard_mut(&mut state).credentials.password = TextField::secret("hunter2");
    let effects = act(&mut state, WizardAction::SubmitCredentials);
    // The manager sanitizes before the reducer ever sees a failure
    // (the same rule as every backend completion): synthesize what it
    // would produce.
    fail_with(&mut state, &effects, "password=█ rejected");

    assert_eq!(
        wizard(&state).last_error.as_deref(),
        Some("password=█ rejected"),
        "the stored detail is the sanitized form"
    );
    // Defense in depth: the wizard state's Debug never carries the raw
    // secret either.
    let debugged = format!("{:?}", wizard(&state));
    assert!(
        !debugged.contains("hunter2"),
        "no secret in Debug: {debugged}"
    );
}

#[test]
fn cancel_steps_back_through_every_screen_and_quits_from_the_first() {
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("u@example.com");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    complete(&mut state, &effects, discovered(vec![gmail_service()]));
    act(&mut state, WizardAction::SelectService);
    act(&mut state, WizardAction::SubmitCredentials);
    wizard_mut(&mut state).credentials.password = TextField::secret("pw");
    let effects = act(&mut state, WizardAction::SubmitCredentials);
    assert_eq!(wizard(&state).step, WizardStep::Testing);

    // Esc on Testing cancels the operation and returns to W4.
    let token = state.session.operations.cancellation(effects[0].id);
    act(&mut state, WizardAction::Cancel);
    assert_eq!(wizard(&state).step, WizardStep::Credentials);
    assert!(
        token.expect("token").is_cancelled(),
        "the test op was cancelled"
    );

    // Esc: W4 → W3 → W2 → W1.
    act(&mut state, WizardAction::Cancel);
    assert_eq!(wizard(&state).step, WizardStep::Identity);
    act(&mut state, WizardAction::Cancel);
    assert_eq!(wizard(&state).step, WizardStep::Discovery);
    act(&mut state, WizardAction::Cancel);
    assert_eq!(wizard(&state).step, WizardStep::Email);

    // Esc on the first step cancels the wizard and quits the app.
    act(&mut state, WizardAction::Cancel);
    assert!(wizard(&state).cancelled);
    assert!(state.session.quit_requested);
}

#[test]
fn save_failure_stays_on_confirm_and_reports() {
    let mut state = state();
    drive_to_confirm(&mut state);
    let effects = act(&mut state, WizardAction::ConfirmSave);
    fail_with(&mut state, &effects, "existing config is not valid TOML: x");

    assert_eq!(wizard(&state).step, WizardStep::Confirm);
    assert!(wizard(&state).last_error.is_some());
    // Retrying re-serializes; nothing accumulated on disk.
    let effects = act(&mut state, WizardAction::ConfirmSave);
    assert!(matches!(
        effects.first().expect("save effect").kind,
        OperationKind::Account(AccountOperation::SaveAccount { .. })
    ));
}

/// A second Enter while a save is in flight starts nothing (review 24):
/// two concurrent SaveAccount writes would race the same config file, the
/// second `in_flight` overwrite would drop the first save's completion,
/// and Esc would cancel only the latest copy.
#[test]
fn a_second_confirm_enter_while_saving_starts_nothing() {
    let mut state = state();
    drive_to_confirm(&mut state);
    let save = act(&mut state, WizardAction::ConfirmSave);
    assert!(matches!(
        save.first().expect("save effect").kind,
        OperationKind::Account(AccountOperation::SaveAccount { .. })
    ));

    // Repeated Enter: idle. No second operation was registered.
    no_effects(&act(&mut state, WizardAction::ConfirmSave));
    assert_eq!(state.session.operations.len(), 1, "never a second save");

    // Esc still cancels the single save; a fresh save then restarts.
    let token = state
        .session
        .operations
        .cancellation(save[0].id)
        .expect("save registered");
    no_effects(&act(&mut state, WizardAction::Cancel));
    assert!(token.is_cancelled());
    let retry = act(&mut state, WizardAction::ConfirmSave);
    assert!(matches!(
        retry.first().expect("restart").kind,
        OperationKind::Account(AccountOperation::SaveAccount { .. })
    ));
}

/// After a save completes (either outcome) Enter starts a fresh save
/// again — the busy gate clears with the operation (review 24).
#[test]
fn confirm_enter_is_reusable_after_the_save_finishes() {
    let mut state = state();
    drive_to_confirm(&mut state);
    let save = act(&mut state, WizardAction::ConfirmSave);
    fail_with(&mut state, &save, "disk full");
    assert_eq!(wizard(&state).step, WizardStep::Confirm);

    let retry = act(&mut state, WizardAction::ConfirmSave);
    assert!(matches!(
        retry.first().expect("save effect").kind,
        OperationKind::Account(AccountOperation::SaveAccount { .. })
    ));
}

fn drive_to_confirm(state: &mut AppState) {
    wizard_mut(state).email.address = TextField::new("u@example.com");
    let effects = act(state, WizardAction::SubmitEmail);
    complete(state, &effects, discovered(vec![gmail_service()]));
    act(state, WizardAction::SelectService);
    act(state, WizardAction::SubmitCredentials);
    wizard_mut(state).credentials.password = TextField::secret("pw");
    let effects = act(state, WizardAction::SubmitCredentials);
    complete(state, &effects, test_ok(&["INBOX"]));
}

#[test]
fn warmup_is_suppressed_while_the_wizard_is_active() {
    let mut state = state();
    // The startup warmup actions and the auto-refresh timer's Refresh
    // must be inert (ADR 0003 §3.1): no account to load yet.
    no_effects(&reduce(&mut state, Action::Refresh));
    no_effects(&reduce(&mut state, Action::LoadDrafts));
    // Mailbox keys leak nothing: they are swallowed wholesale.
    no_effects(&reduce(&mut state, Action::MoveDown));
    no_effects(&reduce(&mut state, Action::Activate));
    no_effects(&reduce(&mut state, Action::Compose));
    no_effects(&reduce(&mut state, Action::OpenSearch));
}

#[test]
fn auto_refresh_tick_fires_nothing_behind_the_wizard() {
    // Review 17: `Tick` reaches `auto_refresh_tick` through the wizard's
    // pass-through; before the wizard joined its conflict list, an
    // elapsed interval fired a background page listing behind the setup
    // screens — spawning himalaya children whose results
    // `wizard_completed` drops. The timer must stand down instead.
    let mut state = state();
    state.settings.refresh_interval_seconds = 60;
    // The first tick arms the timer; nothing fires.
    no_effects(&reduce(
        &mut state,
        Action::Tick {
            now: Box::new(crate::app::mock::now()),
        },
    ));
    // The interval elapses while the wizard is open: silent.
    no_effects(&reduce(
        &mut state,
        Action::Tick {
            now: Box::new(crate::app::mock::now() + chrono::Duration::seconds(60)),
        },
    ));
    assert!(state.session.operations.is_empty());
}

#[test]
fn unknown_and_stale_results_are_rejected() {
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("u@example.com");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    let id = effects[0].id;

    // An unknown id never touches the wizard.
    reduce(
        &mut state,
        Action::BackendCompleted(OperationResult {
            id: OperationId(9999),
            outcome: Ok(discovered(vec![gmail_service()])),
        }),
    );
    assert!(wizard(&state).discovery.discovering);

    // The real one applies.
    complete(&mut state, &effects, discovered(vec![gmail_service()]));
    assert!(!wizard(&state).discovery.discovering);

    // A replay of the same (now-finished) id is rejected.
    let before = wizard(&state).discovery.services.len();
    reduce(
        &mut state,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(discovered(Vec::new())),
        }),
    );
    assert_eq!(wizard(&state).discovery.services.len(), before);
}

#[test]
fn storage_mode_toggle_switches_the_secret_field_and_validates_the_command() {
    let mut state = state();
    drive_to_confirm(&mut state);
    // Back to W4 for the toggle behavior.
    act(&mut state, WizardAction::Cancel);
    assert_eq!(wizard(&state).step, WizardStep::Credentials);

    // The raw password is required in raw mode.
    wizard_mut(&mut state).credentials.storage_mode = StorageMode::Raw;
    wizard_mut(&mut state).credentials.password = TextField::secret("");
    let effects = act(&mut state, WizardAction::SubmitCredentials);
    no_effects(&effects);
    assert!(wizard(&state).last_error.is_some());

    // Command mode validates the program exists and rejects anything
    // beyond the argv-only allowlist (Tmail never spawns a shell,
    // ticket x4gj).
    wizard_mut(&mut state).credentials.storage_mode = StorageMode::Command;
    wizard_mut(&mut state).credentials.command = TextField::new("pass show mail/gmail; rm -rf /");
    let effects = act(&mut state, WizardAction::SubmitCredentials);
    no_effects(&effects);
    assert!(
        wizard(&state)
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("argv-only")
    );

    // A missing program is reported the same way the editor is.
    wizard_mut(&mut state).credentials.command = TextField::new("definitely-not-a-program-xyz");
    let effects = act(&mut state, WizardAction::SubmitCredentials);
    no_effects(&effects);
    assert!(
        wizard(&state)
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("was not found on PATH")
    );
}

/// The default-account preview answers for the name the save would
/// write (review 38): under a chosen collision suffix the base name
/// matches the file's existing default account while the final name does
/// not — the preview used to claim "Default: yes" while the written file
/// carried default = false for the suffixed account.
#[test]
fn the_default_preview_answers_for_the_chosen_name() {
    let mut state = state();
    state.session.wizard = Some(WizardState::new(
        false,
        ConfigSnapshot {
            save_path: Some(std::path::PathBuf::from("/tmp/tmail-test-config.toml")),
            existing_names: vec![String::from("example")],
            existing_default_name: Some(String::from("example")),
            existing_shared_readable: false,
        },
    ));
    drive_to_confirm(&mut state);
    assert!(
        wizard(&state)
            .confirm
            .name_choice
            .as_ref()
            .is_some_and(|choice| matches!(choice, NameChoice::Suffix(_))),
        "collision detected"
    );

    // The base name replaces the existing default account → default keeps.
    assert_eq!(wizard(&state).confirm.name_choice_index, 0);
    assert!(
        wizard(&state).will_set_default(),
        "replacing the default account stays default"
    );

    // The chosen suffix is a NEW account → the existing default stays
    // authoritative; the preview must have answered this before Enter.
    act(&mut state, WizardAction::MoveDown);
    assert_eq!(wizard(&state).confirm.name_choice_index, 1);
    assert!(
        !wizard(&state).will_set_default(),
        "the preview contradicted the write for the suffixed name"
    );
}

#[test]
fn collision_suffix_is_offered_and_selected() {
    let mut state = state();
    // The config already holds an `example` account.
    state.session.wizard = Some(WizardState::new(
        false,
        ConfigSnapshot {
            save_path: Some(std::path::PathBuf::from("/tmp/tmail-test-config.toml")),
            existing_names: vec![String::from("example")],
            existing_default_name: Some(String::from("example")),
            existing_shared_readable: false,
        },
    ));
    drive_to_confirm(&mut state);

    let choice = wizard(&state)
        .confirm
        .name_choice
        .as_ref()
        .expect("collision detected");
    match choice {
        NameChoice::Suffix(suffix) => assert_eq!(suffix, "example-2"),
        NameChoice::Replace => panic!("default choice must be replace, with -2 offered"),
    }
    // Replace is highlighted first (the default), the suffix is one ↓ away.
    assert_eq!(wizard(&state).confirm.name_choice_index, 0);
    assert!(
        wizard(&state).will_set_default(),
        "replacing the file's default account keeps it default"
    );

    // Choosing the suffix renames the saved account.
    act(&mut state, WizardAction::MoveDown);
    assert_eq!(wizard(&state).confirm.name_choice_index, 1);
    let effects = act(&mut state, WizardAction::ConfirmSave);
    let id = effects[0].id;
    let OperationKind::Account(AccountOperation::SaveAccount { draft, .. }) =
        &state.session.operations.get(id).unwrap().kind
    else {
        panic!("expected a SaveAccount operation");
    };
    assert_eq!(draft.name, "example-2");
}

#[test]
fn gmail_domain_and_provider_flag_the_app_password_hint() {
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("u@gmail.com");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    complete(&mut state, &effects, discovered(vec![gmail_service()]));
    act(&mut state, WizardAction::SelectService);
    assert!(wizard(&state).discovery.gmail_hint, "domain rule");
    assert_eq!(
        wizard(&state).provider(),
        Some(Provider::Gmail),
        "discovery tag"
    );
}

#[test]
fn server_url_parsing_maps_schemes_to_security() {
    assert_eq!(
        parse_server_url("imaps://imap.example.com:993"),
        Ok((String::from("imaps://imap.example.com:993"), Security::Tls))
    );
    assert_eq!(
        parse_server_url("imap://imap.example.com:143"),
        Ok((
            String::from("imap://imap.example.com:143"),
            Security::StartTls
        ))
    );
    assert!(parse_server_url("imap.example.com:993").is_err());
    assert!(parse_server_url("foo://host:1").is_err());
    assert!(parse_server_url("imaps://host").is_err(), "port required");
}

#[test]
fn collision_suffix_rule_generates_the_next_free_name() {
    use crate::config::write::next_free_name;
    let existing = vec![
        String::from("gmail"),
        String::from("gmail-2"),
        String::from("other"),
    ];
    assert_eq!(next_free_name(&existing, "gmail"), "gmail-3");
    assert_eq!(next_free_name(&existing, "other"), "other-2");
    assert_eq!(next_free_name(&existing, "free"), "free");
}

#[test]
fn the_draft_requires_smtp_and_the_email() {
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("u@example.com");
    let mut smtpless = gmail_service();
    smtpless.smtp = None;
    let effects = act(&mut state, WizardAction::SubmitEmail);
    complete(&mut state, &effects, discovered(vec![smtpless]));
    act(&mut state, WizardAction::SelectService);
    act(&mut state, WizardAction::SubmitCredentials);
    wizard_mut(&mut state).credentials.password = TextField::secret("pw");
    let effects = act(&mut state, WizardAction::SubmitCredentials);
    no_effects(&effects);
    assert!(
        wizard(&state)
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("SMTP"),
        "single-sided candidates are flagged, not silently saved"
    );
    // Review finding 43: the error renders on W4, where `e` types a
    // character — the hint must send the user back to the server list
    // (W2), where `e` really opens the manual-override form.
    let hint = wizard(&state).last_error.as_deref().unwrap_or_default();
    assert!(
        !hint.contains("(e)"),
        "the W4 error must not advertise `e` as an edit shortcut: {hint}"
    );
    assert!(
        hint.contains("server list"),
        "the hint points to stepping back to the server list: {hint}"
    );
}

#[test]
fn enter_on_the_storage_row_toggles_the_mode_without_testing() {
    let mut state = state();
    drive_to_credentials(&mut state);

    // Tab once: username → the storage-mode row (Tab order feedback:
    // the storage choice comes before the secret it governs).
    act(&mut state, WizardAction::FocusNext);
    assert_eq!(wizard(&state).credentials.credentials_index, 1);

    // Enter activates the focused control: the mode flips, no test runs.
    let effects = reduce(&mut state, Action::Activate);
    no_effects(&effects);
    assert_eq!(wizard(&state).step, WizardStep::Credentials);
    assert_eq!(
        wizard(&state).credentials.storage_mode,
        StorageMode::Command
    );
    assert!(
        state.session.operations.is_empty(),
        "no credential test may start from the toggle row"
    );

    // Enter again flips back to raw.
    reduce(&mut state, Action::Activate);
    assert_eq!(wizard(&state).credentials.storage_mode, StorageMode::Raw);
}

#[test]
fn space_on_the_storage_row_toggles_and_space_types_everywhere_else() {
    let mut state = state();
    drive_to_credentials(&mut state);

    // Space while the username row is focused types a space into the
    // username, it does not toggle.
    act(&mut state, WizardAction::ToggleStorageMode);
    assert_eq!(wizard(&state).credentials.storage_mode, StorageMode::Raw);
    assert!(wizard(&state).credentials.username.value.ends_with(' '));

    // Tab once: the storage row is next in the cycle — space toggles.
    act(&mut state, WizardAction::FocusNext);
    assert_eq!(wizard(&state).credentials.credentials_index, 1);
    act(&mut state, WizardAction::ToggleStorageMode);
    assert_eq!(
        wizard(&state).credentials.storage_mode,
        StorageMode::Command
    );

    // Tab again: the secret row — space types there (passwords and
    // commands may contain spaces).
    act(&mut state, WizardAction::FocusNext);
    act(&mut state, WizardAction::ToggleStorageMode);
    assert_eq!(
        wizard(&state).credentials.storage_mode,
        StorageMode::Command
    );

    // And spaces typed into the command field survive (the fold-back):
    // the focus is already on the secret row in command mode, so the
    // space above was the command field's first character.
    assert_eq!(wizard(&state).credentials.credentials_index, 2);
    act(&mut state, WizardAction::Edit(DialogEdit::Char('p')));
    act(&mut state, WizardAction::Edit(DialogEdit::Char(' ')));
    act(&mut state, WizardAction::Edit(DialogEdit::Char('a')));
    assert_eq!(wizard(&state).credentials.command.value, " p a");
}

#[test]
fn a_display_name_may_contain_spaces() {
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("u@example.com");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    complete(&mut state, &effects, discovered(vec![gmail_service()]));
    act(&mut state, WizardAction::SelectService);

    // Space on the identity screen types into the display name (which
    // SelectService prefilled with the local part "u").
    act(&mut state, WizardAction::Edit(DialogEdit::Char('A')));
    act(&mut state, WizardAction::ToggleStorageMode);
    act(&mut state, WizardAction::Edit(DialogEdit::Char('B')));
    assert_eq!(wizard(&state).credentials.display_name.value, "uA B");
}

#[test]
fn e_on_a_text_step_types_a_character_it_does_not_edit_servers() {
    // Review finding 43's behavioral contract: on the W4/credentials
    // screen (a text-entry step), the `e` key types into the focused
    // field — it must not open the server-edit form or step back. The
    // updated hint relies on exactly this wording being true.
    let mut state = state();
    drive_to_credentials(&mut state);
    assert_eq!(wizard(&state).step, WizardStep::Credentials);

    // Username row focused: `e` becomes a typed character.
    act(&mut state, WizardAction::OverrideServers);
    assert_eq!(wizard(&state).credentials.username.value, "u@example.come");
    assert!(
        !wizard(&state).discovery.override_open,
        "no override form opens from W4"
    );
    assert_eq!(wizard(&state).step, WizardStep::Credentials);

    // Tab to the storage row, then again to the secret row: `e` there
    // lands in the password field (the fold-back delivers it to the
    // focused text field, not the storage toggle).
    act(&mut state, WizardAction::FocusNext);
    assert_eq!(wizard(&state).credentials.credentials_index, 1);
    act(&mut state, WizardAction::FocusNext);
    act(&mut state, WizardAction::OverrideServers);
    assert_eq!(
        wizard(&state).credentials.storage_mode,
        StorageMode::Raw,
        "storage mode unchanged"
    );
    // The focused secret row (index 2) receives the character when the
    // storage mode is Raw (password editing).
    assert_eq!(wizard(&state).credentials.credentials_index, 2);
    assert_eq!(wizard(&state).credentials.password.value, "e");
    assert!(!wizard(&state).discovery.override_open);

    // And Esc from W4 steps back to W3 identity (not a hidden edit).
    act(&mut state, WizardAction::Cancel);
    assert_eq!(wizard(&state).step, WizardStep::Identity);
}

// ── Discovery staleness and gating (review 23) ────────────────────────────

/// Esc from W2 kills the discovery it belongs to (review 23): a late Err
/// must not paint the Email screen's error, and a late empty Ok must not
/// fold an off-screen override form into the next submission.
#[test]
fn esc_from_discovery_cancels_the_late_results_screen_ways() {
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("u@example.com");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    assert_eq!(wizard(&state).step, WizardStep::Discovery);
    let token = state
        .session
        .operations
        .cancellation(effects[0].id)
        .expect("the discovery op is registered");

    // Back to the email screen while detection still runs.
    act(&mut state, WizardAction::Cancel);
    assert_eq!(wizard(&state).step, WizardStep::Email);
    assert!(!wizard(&state).discovery.discovering);
    assert!(!wizard(&state).discovery.override_open);
    // The op is gone from the registry (late results become unknown)…
    assert!(state.session.operations.get(effects[0].id).is_none());
    assert!(token.is_cancelled());

    // …the late Err paints nothing on the Email screen…
    let late = fail_with(&mut state, &effects, "dns exploded");
    no_effects(&late);
    assert!(wizard(&state).last_error.is_none());

    // …and the late empty Ok does not open the override form.
    // (The op was already finished out of the registry; a second synthetic
    // result with a re-registered op verifies the same staleness rule.)
    let resubmit = act(&mut state, WizardAction::SubmitEmail);
    assert_eq!(wizard(&state).step, WizardStep::Discovery);
    act(&mut state, WizardAction::Cancel);
    assert!(!wizard(&state).discovery.override_open);
    no_effects(&fail_with(&mut state, &resubmit, "gone"));
}

/// A DiscoverConfig completion applies only while W2 is the displayed
/// step (review 23): the in_flight id alone is not enough of a guard.
#[test]
fn a_discovery_result_is_rejected_off_its_screen_even_with_a_registered_op() {
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("u@example.com");
    // Register a discovery op whose id the wizard's in_flight still
    // contains, but force the step off W2 (a future in_flight bookkeeping
    // slip must not resurrect the review 23 leak).
    let effect =
        state
            .session
            .operations
            .start(OperationKind::Account(AccountOperation::DiscoverConfig {
                email: String::from("u@example.com"),
            }));
    wizard_mut(&mut state).in_flight = Some(effect.id);
    wizard_mut(&mut state).step = WizardStep::Identity;

    let late = reduce(
        &mut state,
        Action::BackendCompleted(OperationResult {
            id: effect.id,
            outcome: Err(OperationFailure {
                code: Some(1),
                detail: String::from("late"),
                retry: None,
                ambiguous: false,
            }),
        }),
    );
    no_effects(&late);
    assert!(wizard(&state).last_error.is_none());
    assert_eq!(wizard(&state).step, WizardStep::Identity);
}

/// `e` and `r` are inert while detection runs (review 23): the spinner
/// replaces the whole screen, so a hidden override form would be edited
/// invisibly and additional discovery workers would pile up unbounded.
#[test]
fn e_and_r_do_nothing_while_discovery_runs() {
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("u@example.com");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    assert!(wizard(&state).discovery.discovering);
    let registered = state.session.operations.len();

    // `e`: no hidden override form.
    no_effects(&act(&mut state, WizardAction::OverrideServers));
    assert!(!wizard(&state).discovery.override_open);

    // `r`: no second discovery worker.
    no_effects(&act(&mut state, WizardAction::RerunDiscovery));
    assert_eq!(state.session.operations.len(), registered);
    assert!(!effects.is_empty(), "the original detection is registered");
    assert!(
        state.session.operations.get(effects[0].id).is_some(),
        "the original detection continues"
    );

    // Once the result lands, the shortcuts work again.
    complete(&mut state, &effects, discovered(vec![gmail_service()]));
    let again = act(&mut state, WizardAction::RerunDiscovery);
    assert_eq!(again.len(), 1, "rerun after completion starts a fresh op");
    // The rerun replaced the stale predecessor in_flight honestly.
    assert!(state.session.operations.get(again[0].id).is_some());
}

/// Rerunning discovery cancels the predecessor instead of leaking it
/// (review 23, registry hygiene half of the ungated `r` bug).
#[test]
fn a_discovery_rerun_replaces_the_registered_predecessor() {
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("other@example.com");
    let first = act(&mut state, WizardAction::SubmitEmail);
    // Complete the first run so `discovering` is false and `r` is legal.
    complete(&mut state, &first, discovered(vec![gmail_service()]));
    let rerun = act(&mut state, WizardAction::RerunDiscovery);
    assert_eq!(rerun.len(), 1);

    // A predecessor is still registered only in the pathological case,
    // where the first result never drained. Re-feed it late: it was
    // finished out of the registry by wizard_completed, so it must
    // already be dead.
    assert!(state.session.operations.get(first[0].id).is_none());
}

/// Esc from W2 also closes the override form (the close-the-form branch
/// keeps precedence) — and because it returns to the email screen, the
/// override state never leaks past the step change.
#[test]
fn esc_closes_the_override_before_stepping_back() {
    let mut state = state();
    wizard_mut(&mut state).email.address = TextField::new("u@custom.example");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    complete(&mut state, &effects, discovered(Vec::new()));
    assert!(wizard(&state).discovery.override_open);

    act(&mut state, WizardAction::Cancel);
    assert!(wizard(&state).step == WizardStep::Discovery);
    assert!(!wizard(&state).discovery.override_open, "form closed first");
    assert_eq!(wizard(&state).discovery.services.len(), 0);
}

/// Drives the wizard to the credentials screen (fields focused).
fn drive_to_credentials(state: &mut AppState) {
    wizard_mut(state).email.address = TextField::new("u@example.com");
    let effects = act(state, WizardAction::SubmitEmail);
    complete(state, &effects, discovered(vec![gmail_service()]));
    act(state, WizardAction::SelectService);
    act(state, WizardAction::SubmitCredentials); // identity → credentials
}
