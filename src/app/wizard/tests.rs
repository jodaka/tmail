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

fn state() -> AppState {
    let mut state = mock_initial_state();
    state.wizard = Some(WizardState::new(
        false,
        Some(std::path::PathBuf::from("/tmp/tmail-test-config.toml")),
        Vec::new(),
        None,
        false,
    ));
    state.focus = Focus::Wizard;
    state
}

/// A wizard state builder with a distinct name (a local `state` binding
/// would shadow the helper).
fn fresh_wizard_state() -> AppState {
    state()
}

fn manual_state() -> AppState {
    let mut state = fresh_wizard_state();
    if let Some(wizard) = state.wizard.as_mut() {
        wizard.manual = true;
    }
    state
}

fn wizard(state: &AppState) -> &WizardState {
    state.wizard.as_ref().expect("wizard active")
}

fn wizard_mut(state: &mut AppState) -> &mut WizardState {
    state.wizard.as_mut().expect("wizard active")
}

fn act(state: &mut AppState, action: WizardAction) -> Vec<Effect> {
    reduce(state, &Action::Wizard(action))
}

fn complete(state: &mut AppState, effects: &[Effect], outcome: OperationOutcome) -> Vec<Effect> {
    let id = effects.first().expect("effect to complete").id;
    reduce(
        state,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(outcome),
        }),
    )
}

fn fail_with(state: &mut AppState, effects: &[Effect], detail: &str) -> Vec<Effect> {
    let id = effects.first().expect("effect to complete").id;
    reduce(
        state,
        &Action::BackendCompleted(OperationResult {
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
    OperationOutcome::TestAccountCompleted {
        mailboxes: names.iter().map(|name| name.to_string()).collect(),
    }
}

fn account_saved(path: &str, created: bool) -> OperationOutcome {
    OperationOutcome::AccountSaved {
        path: std::path::PathBuf::from(path),
        created,
        permissions_warning: None,
    }
}

/// Drives the happy path up to the saved screen through real reducer
/// transitions. Returns the state (for further assertions).
fn drive_to_saved(state: &mut AppState) {
    wizard_mut(state).email = TextField::new("u@example.com");
    let effects = act(state, WizardAction::SubmitEmail);
    assert!(matches!(
        effects.first().expect("discover effect").kind,
        OperationKind::DiscoverConfig { .. }
    ));
    complete(state, &effects, discovered(vec![gmail_service()]));
    assert_eq!(wizard(state).step, WizardStep::Discovery);
    act(state, WizardAction::SelectService); // → W3
    assert_eq!(wizard(state).step, WizardStep::Identity);
    act(state, WizardAction::SubmitCredentials); // → W4
    assert_eq!(wizard(state).step, WizardStep::Credentials);
    wizard_mut(state).password = TextField::secret("app-password");
    let effects = act(state, WizardAction::SubmitCredentials); // → W5 test
    assert!(matches!(
        effects.first().expect("test effect").kind,
        OperationKind::TestAccount { .. }
    ));
    complete(state, &effects, test_ok(&["INBOX", "[Gmail]/Sent Mail"]));
    assert_eq!(wizard(state).step, WizardStep::Confirm);
    let effects = act(state, WizardAction::ConfirmSave);
    assert!(matches!(
        effects.first().expect("save effect").kind,
        OperationKind::SaveAccount { .. }
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
        wizard(&state).aliases,
        vec![
            (String::from("inbox"), String::from("INBOX")),
            (String::from("sent"), String::from("[Gmail]/Sent Mail")),
        ]
    );
    assert_eq!(wizard(&state).account_name, "example");
    assert!(wizard(&state).will_set_default(), "no existing default");
    assert!(wizard(&state).saved_created);
    assert!(!wizard(&state).completed);
    // First-run dismiss ends the session (main.rs restarts into the
    // mailbox UI with the fresh config).
    act(&mut state, WizardAction::DismissSaved);
    assert!(wizard(&state).completed);
    assert!(state.quit_requested, "the session must end for the restart");
}

#[test]
fn dismiss_ends_the_session_in_both_modes() {
    // Manual: main.rs prints the path and exits 0.
    let mut manual = manual_state();
    drive_to_saved(&mut manual);
    act(&mut manual, WizardAction::DismissSaved);
    assert!(wizard(&manual).completed);
    assert!(manual.quit_requested, "manual mode exits after saving");

    // First-run: the session ends too, and main.rs restarts into the
    // normal mailbox UI with the fresh config.
    let mut first_run = fresh_wizard_state();
    drive_to_saved(&mut first_run);
    act(&mut first_run, WizardAction::DismissSaved);
    assert!(wizard(&first_run).completed);
    assert!(
        first_run.quit_requested,
        "the session must end for the restart"
    );
}

#[test]
fn email_field_edits_flow_through_wizard_actions() {
    let mut state = state();
    for c in "u@example.com".chars() {
        act(&mut state, WizardAction::Edit(DialogEdit::Char(c)));
    }
    assert_eq!(wizard(&state).email.value, "u@example.com");
    act(&mut state, WizardAction::Edit(DialogEdit::Backspace));
    assert_eq!(wizard(&state).email.value, "u@example.co");
    act(&mut state, WizardAction::Edit(DialogEdit::CursorLeft));
    act(&mut state, WizardAction::Edit(DialogEdit::Char('X')));
    assert_eq!(wizard(&state).email.value, "u@example.cXo");
}

#[test]
fn invalid_email_is_reported_and_discovery_does_not_start() {
    let mut state = state();
    wizard_mut(&mut state).email = TextField::new("not-an-email");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    no_effects(&effects);
    assert_eq!(wizard(&state).step, WizardStep::Email);
    assert!(wizard(&state).last_error.is_some());
    assert!(!wizard(&state).discovering);

    // Multiple @ is invalid too.
    wizard_mut(&mut state).email = TextField::new("a@b@c");
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
    wizard_mut(&mut state).email = TextField::new("u@example.com");
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

    assert_eq!(wizard(&state).services.len(), 2);
    assert_eq!(wizard(&state).service_index, 0);
    assert_eq!(
        wizard(&state).services[0].source,
        ConfigSource::Provider(Provider::Gmail),
        "the provider rule preselects first"
    );

    // Enter accepts the preselected candidate and prefills identity.
    act(&mut state, WizardAction::SelectService);
    assert_eq!(wizard(&state).step, WizardStep::Identity);
    assert_eq!(wizard(&state).username.value, "u@example.com");
    assert_eq!(
        wizard(&state).display_name.value,
        "u",
        "local part suggested"
    );
}

#[test]
fn empty_discovery_opens_the_manual_override_form() {
    let mut state = state();
    wizard_mut(&mut state).email = TextField::new("u@custom.example");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    complete(&mut state, &effects, discovered(Vec::new()));

    assert!(
        wizard(&state).override_open,
        "manual override is the only way forward"
    );
    let [imap, smtp, username] = &wizard(&state).override_fields;
    assert_eq!(imap.value, "imaps://imap.custom.example:993");
    assert_eq!(smtp.value, "smtps://smtp.custom.example:465");
    assert_eq!(username.value, "u@custom.example");
}

#[test]
fn manual_override_skips_straight_to_credentials() {
    let mut state = state();
    wizard_mut(&mut state).email = TextField::new("u@custom.example");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    complete(&mut state, &effects, discovered(Vec::new()));

    let effects = act(&mut state, WizardAction::SubmitOverride);
    no_effects(&effects);
    assert_eq!(wizard(&state).step, WizardStep::Credentials);
    assert_eq!(wizard(&state).services.len(), 1);
    assert_eq!(wizard(&state).services[0].source, ConfigSource::Manual);
    assert_eq!(wizard(&state).services[0].provider, None);
    assert_eq!(
        wizard(&state).services[0].imap.security,
        Security::Tls,
        "imaps:// implies implicit TLS"
    );

    // And the credentials submit builds a manual draft.
    wizard_mut(&mut state).password = TextField::secret("pw");
    let effects = act(&mut state, WizardAction::SubmitCredentials);
    let id = effects[0].id;
    let OperationKind::TestAccount { draft } = &state.operations.get(id).unwrap().kind else {
        panic!("expected a TestAccount operation");
    };
    assert_eq!(draft.imap_server, "imaps://imap.custom.example:993");
    assert_eq!(draft.smtp_server, "smtps://smtp.custom.example:465");
}

#[test]
fn test_failure_returns_to_credentials_with_fields_intact() {
    let mut state = state();
    wizard_mut(&mut state).email = TextField::new("u@example.com");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    complete(&mut state, &effects, discovered(vec![gmail_service()]));
    act(&mut state, WizardAction::SelectService);
    act(&mut state, WizardAction::SubmitCredentials);
    wizard_mut(&mut state).password = TextField::secret("app-password");
    let effects = act(&mut state, WizardAction::SubmitCredentials);
    fail_with(&mut state, &effects, "login failed");

    assert_eq!(wizard(&state).step, WizardStep::Credentials);
    assert_eq!(wizard(&state).last_error.as_deref(), Some("login failed"));
    assert_eq!(
        wizard(&state).password.value,
        "app-password",
        "fields intact"
    );

    // Retry re-runs the test with the edited values; nothing accumulates.
    wizard_mut(&mut state).password = TextField::secret("fixed-password");
    let effects = act(&mut state, WizardAction::SubmitCredentials);
    assert!(matches!(
        effects.first().expect("test effect").kind,
        OperationKind::TestAccount { .. }
    ));
    complete(&mut state, &effects, test_ok(&["INBOX"]));
    assert_eq!(wizard(&state).step, WizardStep::Confirm);
}

#[test]
fn a_test_failure_detail_is_stored_sanitized_and_debug_redacted() {
    let mut state = state();
    wizard_mut(&mut state).email = TextField::new("u@example.com");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    complete(&mut state, &effects, discovered(vec![gmail_service()]));
    act(&mut state, WizardAction::SelectService);
    act(&mut state, WizardAction::SubmitCredentials);
    wizard_mut(&mut state).password = TextField::secret("hunter2");
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
    wizard_mut(&mut state).email = TextField::new("u@example.com");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    complete(&mut state, &effects, discovered(vec![gmail_service()]));
    act(&mut state, WizardAction::SelectService);
    act(&mut state, WizardAction::SubmitCredentials);
    wizard_mut(&mut state).password = TextField::secret("pw");
    let effects = act(&mut state, WizardAction::SubmitCredentials);
    assert_eq!(wizard(&state).step, WizardStep::Testing);

    // Esc on Testing cancels the operation and returns to W4.
    let token = state.operations.cancellation(effects[0].id);
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
    assert!(state.quit_requested);
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
        OperationKind::SaveAccount { .. }
    ));
}

fn drive_to_confirm(state: &mut AppState) {
    wizard_mut(state).email = TextField::new("u@example.com");
    let effects = act(state, WizardAction::SubmitEmail);
    complete(state, &effects, discovered(vec![gmail_service()]));
    act(state, WizardAction::SelectService);
    act(state, WizardAction::SubmitCredentials);
    wizard_mut(state).password = TextField::secret("pw");
    let effects = act(state, WizardAction::SubmitCredentials);
    complete(state, &effects, test_ok(&["INBOX"]));
}

#[test]
fn warmup_is_suppressed_while_the_wizard_is_active() {
    let mut state = state();
    // The startup warmup actions and the auto-refresh timer's Refresh
    // must be inert (ADR 0003 §3.1): no account to load yet.
    no_effects(&reduce(&mut state, &Action::Refresh));
    no_effects(&reduce(&mut state, &Action::LoadDrafts));
    // Mailbox keys leak nothing: they are swallowed wholesale.
    no_effects(&reduce(&mut state, &Action::MoveDown));
    no_effects(&reduce(&mut state, &Action::Activate));
    no_effects(&reduce(&mut state, &Action::Compose));
    no_effects(&reduce(&mut state, &Action::OpenSearch));
}

#[test]
fn unknown_and_stale_results_are_rejected() {
    let mut state = state();
    wizard_mut(&mut state).email = TextField::new("u@example.com");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    let id = effects[0].id;

    // An unknown id never touches the wizard.
    reduce(
        &mut state,
        &Action::BackendCompleted(OperationResult {
            id: OperationId(9999),
            outcome: Ok(discovered(vec![gmail_service()])),
        }),
    );
    assert!(wizard(&state).discovering);

    // The real one applies.
    complete(&mut state, &effects, discovered(vec![gmail_service()]));
    assert!(!wizard(&state).discovering);

    // A replay of the same (now-finished) id is rejected.
    let before = wizard(&state).services.len();
    reduce(
        &mut state,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(discovered(Vec::new())),
        }),
    );
    assert_eq!(wizard(&state).services.len(), before);
}

#[test]
fn storage_mode_toggle_switches_the_secret_field_and_validates_the_command() {
    let mut state = state();
    drive_to_confirm(&mut state);
    // Back to W4 for the toggle behavior.
    act(&mut state, WizardAction::Cancel);
    assert_eq!(wizard(&state).step, WizardStep::Credentials);

    // The raw password is required in raw mode.
    wizard_mut(&mut state).storage_mode = StorageMode::Raw;
    wizard_mut(&mut state).password = TextField::secret("");
    let effects = act(&mut state, WizardAction::SubmitCredentials);
    no_effects(&effects);
    assert!(wizard(&state).last_error.is_some());

    // Command mode validates the program exists and rejects shell
    // metacharacters (Tmail never spawns a shell).
    wizard_mut(&mut state).storage_mode = StorageMode::Command;
    wizard_mut(&mut state).command = TextField::new("pass show mail/gmail; rm -rf /");
    let effects = act(&mut state, WizardAction::SubmitCredentials);
    no_effects(&effects);
    assert!(
        wizard(&state)
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("without shell metacharacters")
    );

    // A missing program is reported the same way the editor is.
    wizard_mut(&mut state).command = TextField::new("definitely-not-a-program-xyz");
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

#[test]
fn collision_suffix_is_offered_and_selected() {
    let mut state = state();
    // The config already holds an `example` account.
    state.wizard = Some(WizardState::new(
        false,
        Some(std::path::PathBuf::from("/tmp/tmail-test-config.toml")),
        vec![String::from("example")],
        Some(String::from("example")),
        false,
    ));
    drive_to_confirm(&mut state);

    let choice = wizard(&state)
        .name_choice
        .as_ref()
        .expect("collision detected");
    match choice {
        NameChoice::Suffix(suffix) => assert_eq!(suffix, "example-2"),
        NameChoice::Replace => panic!("default choice must be replace, with -2 offered"),
    }
    // Replace is highlighted first (the default), the suffix is one ↓ away.
    assert_eq!(wizard(&state).name_choice_index, 0);
    assert!(
        wizard(&state).will_set_default(),
        "replacing the file's default account keeps it default"
    );

    // Choosing the suffix renames the saved account.
    act(&mut state, WizardAction::MoveDown);
    assert_eq!(wizard(&state).name_choice_index, 1);
    let effects = act(&mut state, WizardAction::ConfirmSave);
    let id = effects[0].id;
    let OperationKind::SaveAccount { draft, .. } = &state.operations.get(id).unwrap().kind else {
        panic!("expected a SaveAccount operation");
    };
    assert_eq!(draft.name, "example-2");
}

#[test]
fn gmail_domain_and_provider_flag_the_app_password_hint() {
    let mut state = state();
    wizard_mut(&mut state).email = TextField::new("u@gmail.com");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    complete(&mut state, &effects, discovered(vec![gmail_service()]));
    act(&mut state, WizardAction::SelectService);
    assert!(wizard(&state).gmail_hint, "domain rule");
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
    wizard_mut(&mut state).email = TextField::new("u@example.com");
    let mut smtpless = gmail_service();
    smtpless.smtp = None;
    let effects = act(&mut state, WizardAction::SubmitEmail);
    complete(&mut state, &effects, discovered(vec![smtpless]));
    act(&mut state, WizardAction::SelectService);
    act(&mut state, WizardAction::SubmitCredentials);
    wizard_mut(&mut state).password = TextField::secret("pw");
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
}

#[test]
fn enter_on_the_storage_row_toggles_the_mode_without_testing() {
    let mut state = state();
    drive_to_credentials(&mut state);

    // Tab once: username → the storage-mode row (Tab order feedback:
    // the storage choice comes before the secret it governs).
    act(&mut state, WizardAction::FocusNext);
    assert_eq!(wizard(&state).credentials_index, 1);

    // Enter activates the focused control: the mode flips, no test runs.
    let effects = reduce(&mut state, &Action::Activate);
    no_effects(&effects);
    assert_eq!(wizard(&state).step, WizardStep::Credentials);
    assert_eq!(wizard(&state).storage_mode, StorageMode::Command);
    assert!(
        state.operations.is_empty(),
        "no credential test may start from the toggle row"
    );

    // Enter again flips back to raw.
    reduce(&mut state, &Action::Activate);
    assert_eq!(wizard(&state).storage_mode, StorageMode::Raw);
}

#[test]
fn space_on_the_storage_row_toggles_and_space_types_everywhere_else() {
    let mut state = state();
    drive_to_credentials(&mut state);

    // Space while the username row is focused types a space into the
    // username, it does not toggle.
    act(&mut state, WizardAction::ToggleStorageMode);
    assert_eq!(wizard(&state).storage_mode, StorageMode::Raw);
    assert!(wizard(&state).username.value.ends_with(' '));

    // Tab once: the storage row is next in the cycle — space toggles.
    act(&mut state, WizardAction::FocusNext);
    assert_eq!(wizard(&state).credentials_index, 1);
    act(&mut state, WizardAction::ToggleStorageMode);
    assert_eq!(wizard(&state).storage_mode, StorageMode::Command);

    // Tab again: the secret row — space types there (passwords and
    // commands may contain spaces).
    act(&mut state, WizardAction::FocusNext);
    act(&mut state, WizardAction::ToggleStorageMode);
    assert_eq!(wizard(&state).storage_mode, StorageMode::Command);

    // And spaces typed into the command field survive (the fold-back):
    // the focus is already on the secret row in command mode, so the
    // space above was the command field's first character.
    assert_eq!(wizard(&state).credentials_index, 2);
    act(&mut state, WizardAction::Edit(DialogEdit::Char('p')));
    act(&mut state, WizardAction::Edit(DialogEdit::Char(' ')));
    act(&mut state, WizardAction::Edit(DialogEdit::Char('a')));
    assert_eq!(wizard(&state).command.value, " p a");
}

#[test]
fn a_display_name_may_contain_spaces() {
    let mut state = state();
    wizard_mut(&mut state).email = TextField::new("u@example.com");
    let effects = act(&mut state, WizardAction::SubmitEmail);
    complete(&mut state, &effects, discovered(vec![gmail_service()]));
    act(&mut state, WizardAction::SelectService);

    // Space on the identity screen types into the display name (which
    // SelectService prefilled with the local part "u").
    act(&mut state, WizardAction::Edit(DialogEdit::Char('A')));
    act(&mut state, WizardAction::ToggleStorageMode);
    act(&mut state, WizardAction::Edit(DialogEdit::Char('B')));
    assert_eq!(wizard(&state).display_name.value, "uA B");
}

/// Drives the wizard to the credentials screen (fields focused).
fn drive_to_credentials(state: &mut AppState) {
    wizard_mut(state).email = TextField::new("u@example.com");
    let effects = act(state, WizardAction::SubmitEmail);
    complete(state, &effects, discovered(vec![gmail_service()]));
    act(state, WizardAction::SelectService);
    act(state, WizardAction::SubmitCredentials); // identity → credentials
}
