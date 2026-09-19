//! Reducer tests: the runtime account switcher (ticket c0n0) — opening,
//! navigation, the confirm stage for in-flight work and dirty drafts, and
//! the confirmed switch's teardown.

use super::*;

fn accounts(names: &[&str]) -> Vec<crate::config::AccountEntry> {
    names
        .iter()
        .map(|name| crate::config::AccountEntry {
            name: name.to_string(),
            email: Some(format!("{name}@example.org")),
            display_name: None,
            is_default: false,
        })
        .collect()
}

fn seeded(names: &[&str]) -> AppState {
    let mut s = state();
    s.settings.accounts = accounts(names);
    s.settings.account_name = Some(String::from(names[0]));
    s
}

#[test]
fn ctrl_g_opens_the_switcher_on_the_current_account() {
    let mut s = seeded(&["personal", "work"]);
    no_effects(&reduce(&mut s, Action::OpenAccountSwitcher));
    let Some(Overlay::AccountSwitcher(dialog)) = &s.session.overlay else {
        panic!("switcher did not open: {:?}", s.session.overlay);
    };
    assert_eq!(dialog.cursor, 0, "cursor starts on the current account");
    assert_eq!(s.session.focus, Focus::AccountSwitcher);
}

#[test]
fn switcher_is_inert_without_accounts_or_with_the_wizard() {
    let mut s = state();
    s.settings.accounts = Vec::new();
    no_effects(&reduce(&mut s, Action::OpenAccountSwitcher));
    assert_eq!(s.session.overlay, None);

    let mut s = seeded(&["a", "b"]);
    s.session.wizard = Some(crate::app::wizard::WizardState::new(
        true,
        crate::app::wizard::ConfigSnapshot {
            save_path: None,
            existing_names: Vec::new(),
            existing_default_name: None,
            existing_shared_readable: false,
        },
    ));
    no_effects(&reduce(&mut s, Action::OpenAccountSwitcher));
    assert!(s.session.overlay.is_none(), "the wizard owns the screen");
}

#[test]
fn switcher_arrows_move_the_cursor_clamped() {
    let mut s = seeded(&["a", "b"]);
    reduce(&mut s, Action::OpenAccountSwitcher);
    reduce(&mut s, Action::MoveDown);
    let Some(Overlay::AccountSwitcher(dialog)) = &s.session.overlay else {
        panic!("switcher closed unexpectedly");
    };
    assert_eq!(dialog.cursor, 1);
    // Clamped at the last row.
    reduce(&mut s, Action::MoveDown);
    let Some(Overlay::AccountSwitcher(dialog)) = &s.session.overlay else {
        panic!("switcher closed");
    };
    assert_eq!(dialog.cursor, 1);
    reduce(&mut s, Action::MoveUp);
    reduce(&mut s, Action::MoveUp);
    let Some(Overlay::AccountSwitcher(dialog)) = &s.session.overlay else {
        panic!("switcher closed");
    };
    assert_eq!(dialog.cursor, 0);
}

#[test]
fn switcher_esc_closes_and_restores_focus() {
    let mut s = seeded(&["a", "b"]);
    reduce(&mut s, Action::OpenAccountSwitcher);
    no_effects(&reduce(&mut s, Action::BackOrCancel));
    assert_eq!(s.session.overlay, None);
    assert_eq!(s.session.focus, Focus::MessageList);
    assert!(s.session.switch_requested.is_none(), "Esc never switches");
}

#[test]
fn enter_on_the_current_account_just_closes() {
    let mut s = seeded(&["a", "b"]);
    reduce(&mut s, Action::OpenAccountSwitcher);
    no_effects(&reduce(&mut s, Action::Activate));
    assert_eq!(s.session.overlay, None);
    assert_eq!(s.session.focus, Focus::MessageList);
    assert!(s.session.switch_requested.is_none());
}

#[test]
fn enter_on_another_account_switches_when_nothing_is_in_flight() {
    let mut s = seeded(&["a", "b"]);
    reduce(&mut s, Action::OpenAccountSwitcher);
    reduce(&mut s, Action::MoveDown);
    no_effects(&reduce(&mut s, Action::Activate));
    assert_eq!(s.session.overlay, None, "no confirm needed");
    assert_eq!(
        s.session.switch_requested.as_deref(),
        Some("b"),
        "the switch is armed"
    );
    assert!(s.session.composer.is_none());
}

#[test]
fn switch_with_in_flight_work_confirms_first() {
    let mut s = seeded(&["a", "b"]);
    // One foreground page load in flight.
    reduce(&mut s, Action::PageNext);
    reduce(&mut s, Action::OpenAccountSwitcher);
    reduce(&mut s, Action::MoveDown);
    no_effects(&reduce(&mut s, Action::Activate));
    let Some(Overlay::SwitchConfirm(dialog)) = &s.session.overlay else {
        panic!("confirm dialog did not open: {:?}", s.session.overlay);
    };
    assert_eq!(dialog.target, "b");
    assert_eq!(dialog.operations, vec![String::from("Loading messages")]);
    assert!(!dialog.unsaved_draft);
    assert_eq!(dialog.button, ConfirmButton::Keep, "safe default button");
    assert!(
        s.session.switch_requested.is_none(),
        "nothing confirmed yet"
    );
}

#[test]
fn switch_with_a_dirty_composer_confirms_first() {
    let mut s = seeded(&["a", "b"]);
    // Open the composer with typed content (a dirty draft).
    reduce(&mut s, Action::Compose);
    reduce(&mut s, Action::ComposerEdit(ComposerEdit::Char('h')));
    reduce(&mut s, Action::BackOrCancel); // park the draft, back to the list
    assert!(s.session.composer.is_some(), "the draft stays parked");
    reduce(&mut s, Action::OpenAccountSwitcher);
    reduce(&mut s, Action::MoveDown);
    no_effects(&reduce(&mut s, Action::Activate));
    let Some(Overlay::SwitchConfirm(dialog)) = &s.session.overlay else {
        panic!("confirm dialog did not open: {:?}", s.session.overlay);
    };
    assert!(dialog.unsaved_draft, "the dirty draft must be listed");
    assert!(dialog.operations.is_empty());
}

#[test]
fn switch_confirm_keep_aborts_the_switch() {
    let mut s = seeded(&["a", "b"]);
    reduce(&mut s, Action::PageNext);
    reduce(&mut s, Action::OpenAccountSwitcher);
    reduce(&mut s, Action::MoveDown);
    reduce(&mut s, Action::Activate);
    // The default button is Keep working: Enter aborts.
    no_effects(&reduce(&mut s, Action::Activate));
    assert_eq!(s.session.overlay, None);
    assert_eq!(s.session.focus, Focus::MessageList);
    assert!(s.session.switch_requested.is_none());
    assert!(
        s.session.operations.len() == 1,
        "the in-flight operation survived the abort"
    );
}

#[test]
fn switch_confirm_tab_moves_the_button_and_confirm_proceeds() {
    let mut s = seeded(&["a", "b"]);
    reduce(&mut s, Action::PageNext);
    let (id, _) = {
        let op = s.session.operations.foreground().unwrap();
        (op.id, op.kind.clone())
    };
    let token = s.session.operations.cancellation(id).unwrap();
    reduce(&mut s, Action::OpenAccountSwitcher);
    reduce(&mut s, Action::MoveDown);
    reduce(&mut s, Action::Activate);
    // Tab to "Switch anyway", Enter: the switch proceeds.
    reduce(&mut s, Action::FocusNext);
    no_effects(&reduce(&mut s, Action::Activate));
    assert!(token.is_cancelled(), "the in-flight op's token fired");
    assert!(s.session.operations.get(id).is_none(), "registry cleared");
    assert!(s.session.overlay.is_none());
    assert!(s.session.composer.is_none());
    assert_eq!(s.session.switch_requested.as_deref(), Some("b"));
}

#[test]
fn switch_confirm_esc_aborts_and_restores_focus() {
    let mut s = seeded(&["a", "b"]);
    reduce(&mut s, Action::PageNext);
    reduce(&mut s, Action::OpenAccountSwitcher);
    reduce(&mut s, Action::MoveDown);
    reduce(&mut s, Action::Activate);
    no_effects(&reduce(&mut s, Action::BackOrCancel));
    assert_eq!(s.session.overlay, None, "both dialogs close");
    assert_eq!(s.session.focus, Focus::MessageList);
    assert!(s.session.switch_requested.is_none());
}

#[test]
fn backend_results_land_while_the_switcher_is_open() {
    let mut s = seeded(&["a", "b"]);
    let (id, req) = expect_page(&reduce(&mut s, Action::PageNext));
    reduce(&mut s, Action::OpenAccountSwitcher);
    // The page result applies even under the popup.
    complete_page_ok(&mut s, id, &req, req.offset);
    assert!(s.session.operations.get(id).is_none());
    let Some(Overlay::AccountSwitcher(_)) = &s.session.overlay else {
        panic!("the switcher must stay open");
    };
}

#[test]
fn clicking_the_account_line_opens_the_switcher() {
    // The topbar's account line records ClickTarget::AccountButton; the
    // click lands on the same path Ctrl+G feeds, so the mouse user gets
    // the same popup with the cursor on the current account.
    let mut s = seeded(&["personal", "work"]);
    no_effects(&reduce(
        &mut s,
        Action::Click(crate::app::action::ClickTarget::AccountButton),
    ));
    let Some(Overlay::AccountSwitcher(dialog)) = &s.session.overlay else {
        panic!("switcher did not open: {:?}", s.session.overlay);
    };
    assert_eq!(dialog.cursor, 0);
    assert_eq!(s.session.focus, Focus::AccountSwitcher);
}
