//! Reply / forward seeding (plan §14, Phase 7.3): fetch the source
//! message, turn it into a seeded draft, and open the composer — from the
//! reader and from the list.
use super::navigation::composer_open;
use crate::app::composer::ComposerState;
use crate::app::effect::Effect;
use crate::app::focus::Focus;
use crate::app::operation::OperationKind;
use crate::app::route::Route;
use crate::app::state::AppState;

// ── Reply / forward seeding (plan §14, Phase 7.3) ────────────────────────

/// The one-composer-rule status issued wherever seeding (or a draft
/// install) would clobber a composer already in progress.
const DRAFT_ALREADY_OPEN: &str = "A draft is already open — send or discard it first";

/// Enforce the one-composer rule at a seeding site: reports the shared
/// status and returns true when a composer is already open.
fn refuse_open_composer(state: &mut AppState) -> bool {
    if !composer_open(state) {
        return false;
    }
    state.set_status(DRAFT_ALREADY_OPEN);
    true
}

/// Turn a fetched message into its seeded draft plus the completion
/// status — the single kind→seed table shared by the reader path and the
/// list path (plan §14: one behavior, two entry points).
fn seed_for(
    message: &crate::domain::Message,
    kind: crate::app::operation::SeedKind,
    own_account: Option<&str>,
) -> (crate::domain::reply::Seed, &'static str) {
    use crate::app::operation::SeedKind;
    use crate::domain::ReplyKind;
    match kind {
        SeedKind::Reply => (
            crate::domain::reply::seed_reply(message, ReplyKind::Reply, None),
            "Reply draft ready",
        ),
        SeedKind::ReplyAll => (
            crate::domain::reply::seed_reply(message, ReplyKind::ReplyAll, own_account),
            "Reply-all draft ready",
        ),
        SeedKind::Forward => (
            crate::domain::reply::seed_forward(message),
            "Forward draft ready",
        ),
    }
}

/// Seed a reply draft from the open message (reader). Reply acts on full
/// message data — headers, threading ids, and the quotable body — so it
/// requires a loaded reader; a draft already in the composer is never
/// clobbered (plan §14: one composer at a time).
pub(crate) fn open_reply(state: &mut AppState) -> Vec<Effect> {
    match state.open_message.as_loaded() {
        Some(message) => {
            let (seed, status) = seed_for(message, crate::app::operation::SeedKind::Reply, None);
            if refuse_open_composer(state) {
                return Vec::new();
            }
            open_seeded_composer(state, seed, status)
        }
        // Per user request: reply works from the list too — fetch the
        // selected message, then seed on arrival (`SeedComposer` result arm).
        None => seed_from_list(state, crate::app::operation::SeedKind::Reply),
    }
}

/// Seed a reply-all draft (Phase 7.5): recipients merged, deduplicated,
/// and the configured account address excluded.
pub(crate) fn open_reply_all(state: &mut AppState) -> Vec<Effect> {
    match state.open_message.as_loaded() {
        Some(message) => {
            let own = state.settings.account_email.clone();
            let (seed, status) = seed_for(
                message,
                crate::app::operation::SeedKind::ReplyAll,
                own.as_deref(),
            );
            if refuse_open_composer(state) {
                return Vec::new();
            }
            open_seeded_composer(state, seed, status)
        }
        None => seed_from_list(state, crate::app::operation::SeedKind::ReplyAll),
    }
}

/// Seed a forward draft from the open message (reader).
pub(crate) fn open_forward(state: &mut AppState) -> Vec<Effect> {
    match state.open_message.as_loaded() {
        Some(message) => {
            let (seed, status) = seed_for(message, crate::app::operation::SeedKind::Forward, None);
            if refuse_open_composer(state) {
                return Vec::new();
            }
            open_seeded_composer(state, seed, status)
        }
        None => seed_from_list(state, crate::app::operation::SeedKind::Forward),
    }
}
/// List-initiated reply/forward (user request): the summaries do not carry
/// a body, so fetch the message first (`SeedComposer`) and seed the
/// composer when the result lands. Requires a list/reader-targeted
/// message.
pub(crate) fn seed_from_list(
    state: &mut AppState,
    kind: crate::app::operation::SeedKind,
) -> Vec<Effect> {
    // Same target rule as every list/reader message action.
    let locator = match state.session.focus {
        Focus::MessageList | Focus::Reader => state.action_target(),
        _ => None,
    };
    let Some(locator) = locator else {
        return Vec::new();
    };
    if refuse_open_composer(state) {
        return Vec::new();
    }
    state.set_status(match kind {
        crate::app::operation::SeedKind::Reply => "Loading message for reply…",
        crate::app::operation::SeedKind::ReplyAll => "Loading message for reply-all…",
        crate::app::operation::SeedKind::Forward => "Loading message for forward…",
    });
    vec![
        state
            .session
            .operations
            .start(OperationKind::SeedComposer { locator, kind }),
    ]
}

/// The fetched message arrives for a list-initiated seed: convert it into
/// a composer draft with the same domain seeding the reader uses, one
/// composer rule intact (an older fetch whose composer already opened by
/// a newer seed is superseded away operation-wise, so this runs once).
pub(crate) fn install_seed(
    state: &mut AppState,
    message: crate::domain::Message,
    kind: crate::app::operation::SeedKind,
) -> Vec<Effect> {
    if refuse_open_composer(state) {
        return Vec::new();
    }
    let (seed, status) = seed_for(&message, kind, state.settings.account_email.as_deref());
    open_seeded_composer(state, seed, status)
}

/// Install a seeded draft in the composer, pushing the composer route on
/// top of the current one (reply from the reader returns to the reader on
/// Esc). The seeded draft starts clean: autosave engages on the first
/// edit.
pub(crate) fn open_seeded_composer(
    state: &mut AppState,
    seed: crate::domain::reply::Seed,
    status: &str,
) -> Vec<Effect> {
    let draft = crate::domain::Draft {
        to: seed.to,
        cc: seed.cc,
        bcc: String::new(),
        subject: seed.subject,
        body: seed.body,
        in_reply_to: seed.in_reply_to,
        references: seed.references,
        ..crate::domain::Draft::default()
    };
    install_composer_draft(state, draft, status);
    Vec::new()
}

/// Put a ready draft into the composer and open the composer screen over
/// the current route. The one-composer rule lives with the callers: this
/// installs unconditionally.
pub(crate) fn install_composer_draft(
    state: &mut AppState,
    draft: crate::domain::Draft,
    status: &str,
) {
    state.session.composer = Some(ComposerState::from_draft(draft));
    if !matches!(state.active_route(), Some(Route::Composer)) {
        state.session.routes.push(Route::Composer);
    }
    state.session.focus = Focus::Composer;
    state.set_status(status);
}
