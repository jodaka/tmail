//! Navigation and input: focus cycling, list movement and pagination,
//! reader scrolling, page requests and their cache-completion handlers,
//! activation (open reader / composer / attachment dialog), and message
//! opening including the reader route.
use super::actions::activate_reader_item;
use super::composer_flow::{draft_save_effect, open_composer_screen, switch_mailbox};
use super::message_results::{
    apply_mailbox_listing, apply_page, cycle_reader_focus, visible_mailbox_page,
};
use super::reduce;
use super::results::unexpected_payload;
use super::seeding::install_composer_draft;
use crate::app::action::Action;
use crate::app::composer::ComposerField;
use crate::app::effect::Effect;
use crate::app::focus::Focus;
use crate::app::operation::{OperationKind, OperationOutcome, OperationResult};
use crate::app::overlay::{AttachmentFileDialog, Overlay};
use crate::app::route::{MessageRoute, Route};
use crate::app::state::{AppState, Loadable};
use crate::domain::{
    MailboxId, MailboxRole, MessageLocator, PageRequest, SearchRequest, bare_message_id,
};

// ── Navigation and input ─────────────────────────────────────────────────

/// Tab/Shift+Tab focus cycling (plan §10). While composing, the folder
/// list joins the composer's cycle — Tab past the last control (Shift+Tab
/// before the first) steps out to the sidebar, and the next step returns
/// into the composer's first (last) control — so a draft can be parked on
/// any mailbox without leaving the composer: switching preserves the draft
/// (plan §14). The mailbox screen's other focusable controls (search
/// field, message list) are off screen while the
/// composer replaces the list, so the cycle skips them.
pub(crate) fn focus_step(state: &mut AppState, delta: i64) -> Vec<Effect> {
    match state.session.focus {
        Focus::Composer => {
            let Some(composer) = state.session.composer.as_mut() else {
                return Vec::new();
            };
            // The sidebar sits just past the cycle's ends: Tab leaves from
            // the last control, Shift+Tab from the first.
            let step_out = (delta > 0 && composer.field == ComposerField::Discard)
                || (delta < 0 && composer.field == ComposerField::To);
            if step_out {
                state.session.focus = Focus::Sidebar;
            } else if delta > 0 {
                composer.focus_next();
            } else {
                composer.focus_previous();
            }
            Vec::new()
        }
        // Back into the composer: Tab re-enters at the first control,
        // Shift+Tab at the last — one linear cycle.
        Focus::Sidebar if matches!(state.active_route(), Some(Route::Composer)) => {
            state.session.focus = Focus::Composer;
            let field = if delta > 0 {
                ComposerField::To
            } else {
                ComposerField::Discard
            };
            if let Some(composer) = state.session.composer.as_mut() {
                composer.focus_field(field);
            }
            Vec::new()
        }
        // In the reader Tab walks the body's links and the attachment
        // chips (plan §15, tickets 1fnh/hc9n): the selection the Enter and
        // save/open keys act on.
        Focus::Reader => {
            cycle_reader_focus(state, delta);
            Vec::new()
        }
        _ => {
            state.session.focus = if delta > 0 {
                state.session.focus.next()
            } else {
                state.session.focus.previous()
            };
            Vec::new()
        }
    }
}

pub(crate) fn move_selection(state: &mut AppState, delta: i64) -> Vec<Effect> {
    match state.session.focus {
        Focus::MessageList => {
            let len = state.messages.items.len();
            if len == 0 {
                return Vec::new();
            }
            let next = state.selection as i64 + delta;
            state.selection = next.clamp(0, len as i64 - 1) as usize;
            keep_selection_visible(state);
        }
        Focus::Sidebar => {
            let len = state.mailboxes.as_loaded().map(Vec::len).unwrap_or(0);
            if len == 0 {
                return Vec::new();
            }
            let next = state.mailbox_selection as i64 + delta;
            state.mailbox_selection = next.clamp(0, len as i64 - 1) as usize;
        }
        Focus::SearchField => {
            // Cursor movement inside the field is a render concern for now;
            // the query is edited append/backspace only (Phase 1).
        }
        Focus::Reader => scroll_reader(state, delta),
        // Wizard input never reaches the mailbox navigation (the wizard
        // intercepts everything first, ADR 0003).
        Focus::Composer
        | Focus::Dialog
        | Focus::ThemePicker
        | Focus::Help
        | Focus::ErrorModal
        | Focus::Wizard => {}
    }
    Vec::new()
}

/// Left/Right: pages in the message list, viewport steps in the reader.
pub(crate) fn page_step(state: &mut AppState, delta: i64) -> Vec<Effect> {
    match state.session.focus {
        Focus::Reader => {
            let (viewport, _) = reader_scroll_bounds(state);
            scroll_reader(state, delta * viewport);
            Vec::new()
        }
        _ => change_page(state, delta),
    }
}

/// Scroll the reader body (Up/Down in reader focus, plan §10: "scroll
/// focused area"). The budget comes from the same pure content functions
/// the renderer draws, so the reducer's clamp always matches the frame.
/// The fixed header never scrolls (ticket 6864): the viewport is the rows
/// under it and the clamp tracks the body alone.
pub(crate) fn scroll_reader(state: &mut AppState, delta: i64) {
    let (viewport, total) = reader_scroll_bounds(state);
    let max = (total - viewport).max(0);
    let next = (state.reader_scroll as i64 + delta).clamp(0, max);
    state.reader_scroll = next as usize;
}

/// The reader's scrollable viewport (body rows under the fixed header) and
/// the scrollable body length, from the same pure functions the renderer
/// draws — reducer and frame can never disagree (ticket 6864).
pub(crate) fn reader_scroll_bounds(state: &AppState) -> (i64, i64) {
    let width = crate::view::layout::reader_width(state.session.size).max(10);
    let viewport = crate::view::layout::reader_rows_visible(state.session.size)
        .saturating_sub(crate::app::reader::header_line_count(state, width))
        .max(1) as i64;
    let total = crate::app::reader::scroll_line_count(state, width) as i64;
    (viewport, total)
}

/// Reflow on terminal resize (plan §13/§19 Phase 5): the document re-wraps
/// at the new reader width inside the renderer, and the scroll anchor is
/// clamped to the re-flowed body length so the viewport can never point
/// past the end of the document (the fixed header never scrolls, ticket
/// 6864).
pub(crate) fn clamp_reader_scroll(state: &mut AppState) {
    if !matches!(state.active_route(), Some(Route::Message(_))) {
        return;
    }
    let (viewport, total) = reader_scroll_bounds(state);
    let max = (total - viewport).max(0);
    state.reader_scroll = (state.reader_scroll as i64).clamp(0, max) as usize;
}

/// Shift `list_scroll` so the selection stays on screen. Row geometry comes
/// from the same pure layout math the renderer uses (`messages_visible`,
/// view-mode aware: comfortable rows cost two lines), keeping the
/// bookkeeping consistent with what is actually drawn.
pub(crate) fn keep_selection_visible(state: &mut AppState) {
    let visible =
        crate::view::layout::messages_visible(state.session.size, state.settings.view_mode).max(1);
    if state.selection < state.list_scroll {
        state.list_scroll = state.selection;
    } else if state.selection >= state.list_scroll + visible {
        state.list_scroll = state.selection + 1 - visible;
    }
    // A shrunken or replaced page must never leave the window past the end.
    state.list_scroll = state
        .list_scroll
        .min(state.messages.items.len().saturating_sub(1));
}

pub(crate) fn change_page(state: &mut AppState, delta: i64) -> Vec<Effect> {
    if state.session.focus != Focus::MessageList {
        return Vec::new();
    }
    let limit = state.messages.limit.max(1) as i64;
    // Base on the in-flight request when there is one, so repeated keys
    // before results arrive keep advancing instead of re-requesting the
    // same next page. The in-flight kind follows the visible context
    // (mailbox page vs search results, Phase 9).
    let base = match state.active_route() {
        Some(Route::Search(route)) => state
            .session
            .operations
            .search_in_flight(&route.mailbox_id)
            .map(|pending| pending.offset as i64),
        Some(route) => route
            .mailbox_id()
            .and_then(|id| state.session.operations.page_in_flight(id))
            .map(|pending| pending.offset as i64),
        None => None,
    }
    .unwrap_or(state.messages.offset as i64);
    let target = base + delta * limit;
    if target < 0 {
        return Vec::new();
    }
    // Known total: never issue a request past the end (plan §16/§19).
    if state
        .messages
        .total
        .is_some_and(|total| target >= total as i64)
    {
        return Vec::new();
    }
    // Unknown total (maildir, search): a short page is the last one, so
    // there is nothing valid to request.
    if state.messages.total.is_none() && state.messages.items.len() < state.messages.limit {
        return Vec::new();
    }
    // The selection keeps pointing at the current page until the result
    // applies; `apply_page` re-resolves it by identity.
    request_visible_page(state, target as usize)
}

/// Request one page of whatever list the active route shows (Phase 9): the
/// active mailbox's page, or the open search's results. Both share the
/// list state, selection, and scroll machinery (Phase 9.1).
pub(crate) fn request_visible_page(state: &mut AppState, offset: usize) -> Vec<Effect> {
    match state.active_route() {
        Some(Route::Search(route)) => {
            let request = SearchRequest {
                mailbox_id: route.mailbox_id.clone(),
                query: route.query.clone(),
                offset,
                limit: state.messages.limit.max(1),
            };
            // Ticket haeb: in cold contexts (empty list) the cached page
            // for this exact query renders immediately — a move re-sync
            // has a non-empty list, so it can never resurrect moved rows
            // from the cache. The read runs off-thread; its completion
            // applies the page and starts the fresh load.
            let mut effects = Vec::new();
            if state.messages.items.is_empty() {
                effects.push(state.session.operations.start_background(
                    OperationKind::CacheListLoad {
                        mailbox: request.mailbox_id.clone(),
                        query: Some(request.query.clone()),
                        offset: request.offset,
                        limit: request.limit,
                        fresh_background_on_hit: false,
                    },
                ));
            } else {
                effects.push(
                    state
                        .session
                        .operations
                        .start(OperationKind::Search(request)),
                );
            }
            effects
        }
        Some(route) => match route.mailbox_id().cloned() {
            Some(mailbox_id) => {
                let request = PageRequest {
                    mailbox_id,
                    offset,
                    limit: state.messages.limit.max(1),
                };
                // Ticket haeb: cached summaries render instantly in cold
                // contexts (empty list: startup, mailbox switch) — a move
                // re-sync has a non-empty list, so it can never resurrect
                // moved rows from the cache. The read runs off-thread; its
                // completion applies the page and starts the fresh load.
                let mut effects = Vec::new();
                if state.messages.items.is_empty() {
                    effects.push(state.session.operations.start_background(
                        OperationKind::CacheListLoad {
                            mailbox: request.mailbox_id.clone(),
                            query: None,
                            offset: request.offset,
                            limit: request.limit,
                            fresh_background_on_hit: false,
                        },
                    ));
                } else {
                    effects.push(
                        state
                            .session
                            .operations
                            .start(OperationKind::LoadPage(request)),
                    );
                }
                effects
            }
            None => Vec::new(),
        },
        None => Vec::new(),
    }
}

/// Background variant of [`request_visible_page`] (Phase 9.4): same
/// request, but failures are handled as background work (Phase 9.6). The
/// visible page is marked clean first — the start boundary of the
/// new-mail notification bookkeeping (ticket b28p): only ids the result
/// adds are new arrivals (a foreground refresh or navigation between
/// timer firings is already counted clean).
pub(crate) fn request_visible_page_background(state: &mut AppState, offset: usize) -> Vec<Effect> {
    state.session.notifications.mark_clean(&state.messages);
    match state.active_route() {
        Some(Route::Search(route)) => {
            let request = SearchRequest {
                mailbox_id: route.mailbox_id.clone(),
                query: route.query.clone(),
                offset,
                limit: state.messages.limit.max(1),
            };
            vec![
                state
                    .session
                    .operations
                    .start_background(OperationKind::Search(request)),
            ]
        }
        Some(route) => match route.mailbox_id().cloned() {
            Some(mailbox_id) => {
                let request = PageRequest {
                    mailbox_id,
                    offset,
                    limit: state.messages.limit.max(1),
                };
                vec![
                    state
                        .session
                        .operations
                        .start_background(OperationKind::LoadPage(request)),
                ]
            }
            None => Vec::new(),
        },
        None => Vec::new(),
    }
}

/// Start a page load for the active route's mailbox and return its effect.
/// The registry supersedes any page request still in flight for the same
/// mailbox (and cancels it), so only the newest result can win.
pub(crate) fn request_page(state: &mut AppState, offset: usize) -> Vec<Effect> {
    let Some(mailbox_id) = state.active_route().and_then(Route::mailbox_id).cloned() else {
        return Vec::new();
    };
    let request = PageRequest {
        mailbox_id,
        offset,
        limit: state.messages.limit.max(1),
    };
    // Ticket haeb: in cold contexts (empty list — startup, mailbox switch)
    // the cached page renders instantly; the fresh load below still runs
    // and its result overwrites the cache and the page. When the cache
    // served the visible rows, the fresh load is *consequence* work: it
    // runs in the background so `Esc` can never cancel it into "cancelled"
    // noise, and its result converges read/unread state and remote drift.
    // The read itself runs off-thread (the manager owns the cache); the
    // completion decides the fresh load's origin.
    if state.messages.items.is_empty() {
        vec![
            state
                .session
                .operations
                .start_background(OperationKind::CacheListLoad {
                    mailbox: request.mailbox_id.clone(),
                    query: None,
                    offset: request.offset,
                    limit: request.limit,
                    fresh_background_on_hit: true,
                }),
        ]
    } else {
        vec![
            state
                .session
                .operations
                .start(OperationKind::LoadPage(request)),
        ]
    }
}

/// Apply a served page-cache read (ticket haeb): in a still-cold context a
/// hit renders the cached rows instantly and starts the fresh load
/// (background when the read was the cold-start path, foreground
/// otherwise); a miss starts the fresh load in the foreground. A result
/// for a context that warmed up meanwhile is dropped — whatever filled the
/// list already owns it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn complete_cache_list_load(
    state: &mut AppState,
    mailbox: &MailboxId,
    query: Option<&str>,
    offset: usize,
    limit: usize,
    fresh_background_on_hit: bool,
    result: OperationResult,
) -> Vec<Effect> {
    let id = result.id;
    // Currency: the visible list must still belong to this identity (a
    // mailbox page for `query: None`, the open search otherwise) and stay
    // cold — the cache only ever serves startup and mailbox switches.
    let current = match query {
        Some(query) => matches!(
            state.active_route(),
            Some(Route::Search(route))
                if route.mailbox_id == *mailbox && route.query == query
        ),
        None => visible_mailbox_page(state).is_some_and(|id| *id == *mailbox),
    };
    if !current || !state.messages.items.is_empty() {
        tracing::debug!(
            id = %id,
            mailbox = %mailbox.0,
            "dropping cache read for a warmed or switched context"
        );
        return Vec::new();
    }
    match result.outcome {
        Ok(OperationOutcome::CachedPage(page)) => {
            let mut effects = apply_page(state, page);
            let load = match query {
                Some(query) => state.session.operations.start(OperationKind::Search(
                    crate::domain::SearchRequest {
                        mailbox_id: mailbox.clone(),
                        query: String::from(query),
                        offset,
                        limit,
                    },
                )),
                None => {
                    let request = PageRequest {
                        mailbox_id: mailbox.clone(),
                        offset,
                        limit,
                    };
                    if fresh_background_on_hit {
                        state
                            .session
                            .operations
                            .start_background(OperationKind::LoadPage(request))
                    } else {
                        state
                            .session
                            .operations
                            .start(OperationKind::LoadPage(request))
                    }
                }
            };
            effects.push(load);
            effects
        }
        Ok(OperationOutcome::CacheMiss) => {
            let load = match query {
                Some(query) => state.session.operations.start(OperationKind::Search(
                    crate::domain::SearchRequest {
                        mailbox_id: mailbox.clone(),
                        query: String::from(query),
                        offset,
                        limit,
                    },
                )),
                None => state
                    .session
                    .operations
                    .start(OperationKind::LoadPage(PageRequest {
                        mailbox_id: mailbox.clone(),
                        offset,
                        limit,
                    })),
            };
            vec![load]
        }
        Ok(_) => unexpected_payload(id, "cached page"),
        Err(_) => Vec::new(),
    }
}

/// Apply a served mailbox-listing cache read (ticket haeb): a non-empty
/// hit renders the sidebar instantly and roots the route stack, then the
/// fresh listing still loads in the background (uncancellable — at startup
/// the user has no intent to interrupt it, and a cached sidebar has
/// already given them something interactive). A miss (or an empty cached
/// listing) goes straight to the fresh background load.
pub(crate) fn complete_cache_mailboxes_load(
    state: &mut AppState,
    result: OperationResult,
) -> Vec<Effect> {
    // Currency: the sidebar must still be unloaded — nothing else loads it
    // between this read's start and its result.
    if matches!(state.mailboxes, Loadable::Loaded(_)) {
        return Vec::new();
    }
    match result.outcome {
        Ok(OperationOutcome::CachedMailboxes(mailboxes)) if !mailboxes.is_empty() => {
            state.mailboxes = Loadable::Loaded(mailboxes.clone());
            let mut effects = apply_mailbox_listing(state, mailboxes);
            effects.push(
                state
                    .session
                    .operations
                    .start_background(OperationKind::LoadMailboxes),
            );
            effects
        }
        Ok(_) => vec![
            state
                .session
                .operations
                .start_background(OperationKind::LoadMailboxes),
        ],
        Err(_) => Vec::new(),
    }
}

/// Apply a served message-cache read for the reader (ticket haeb): a hit
/// renders the cached body instantly and starts the silent background
/// convergence fetch (it reconciles read state and remote drift; it never
/// takes the loader slot, so `Esc` cannot cancel it into "cancelled"
/// noise). A miss keeps the spinner and loads in the foreground. A result
/// for a closed or switched reader is dropped.
pub(crate) fn complete_cache_message_load(
    state: &mut AppState,
    locator: &MessageLocator,
    result: OperationResult,
) -> Vec<Effect> {
    let id = result.id;
    let current = matches!(
        state.active_route(),
        Some(Route::Message(route))
            if route.mailbox_id == locator.mailbox && route.summary.id == locator.id
    );
    if !current {
        tracing::debug!(
            id = %id,
            mailbox = %locator.mailbox.0,
            "dropping message cache read for a closed reader"
        );
        return Vec::new();
    }
    match result.outcome {
        Ok(OperationOutcome::CachedMessage(message)) => {
            // The cached body renders immediately; the convergence fetch
            // still runs silently in the background (never cancellable,
            // no loader slot): its result converges read state, fills the
            // list snippet, and reconciles the attachment flag — exactly
            // what a fresh load would do (ticket haeb).
            state.open_message = Loadable::Loaded(*message);
            vec![
                state
                    .session
                    .operations
                    .start_background(OperationKind::LoadMessage(locator.clone())),
            ]
        }
        Ok(OperationOutcome::CacheMiss) => vec![
            state
                .session
                .operations
                .start(OperationKind::LoadMessage(locator.clone())),
        ],
        Ok(_) => unexpected_payload(id, "cached message"),
        Err(_) => Vec::new(),
    }
}

pub(crate) fn activate(state: &mut AppState) -> Vec<Effect> {
    match state.session.focus {
        Focus::Sidebar => match state.selected_mailbox().cloned() {
            Some(mailbox) => switch_mailbox(state, &mailbox.id),
            None => Vec::new(),
        },
        Focus::MessageList => match state.selected_message().cloned() {
            Some(summary) => open_selected(state, summary),
            None => Vec::new(),
        },
        Focus::Composer => activate_composer(state),
        // The dialogs intercept Enter themselves; unreachable in practice.
        // Enter on the reader activates the focused item (tickets 61qx,
        // hc9n): a focused link opens in the platform browser; otherwise
        // the selected attachment chip opens (`o`'s path — a session save
        // is reused, otherwise the chip is saved first and the opener
        // chains on the confirmed path). Inert without a loaded message.
        Focus::Reader => activate_reader_item(state),
        Focus::Dialog
        | Focus::ThemePicker
        | Focus::Help
        | Focus::SearchField
        | Focus::ErrorModal
        | Focus::Wizard => {
            if state.session.focus == Focus::SearchField {
                reduce(state, Action::SubmitSearch)
            } else {
                Vec::new()
            }
        }
    }
}

/// Enter on a composer control (plan §10): newline in the body, reveal a
/// hidden Cc/Bcc field, open the attachment path dialog (plan §15), remove
/// the focused attachment chip, or activate the focused action. Enter on
/// To/Cc/Bcc/Subject itself does nothing (single-line fields have no
/// activation).
pub(crate) fn activate_composer(state: &mut AppState) -> Vec<Effect> {
    let Some(field) = state.session.composer.as_ref().map(|c| c.field) else {
        return Vec::new();
    };
    match field {
        ComposerField::Body => {
            if let Some(composer) = state.session.composer.as_mut() {
                composer.apply(&crate::app::action::ComposerEdit::Newline);
            }
            Vec::new()
        }
        ComposerField::CcToggle => {
            if let Some(composer) = state.session.composer.as_mut() {
                composer.show_cc = true;
                composer.enter_cc();
            }
            Vec::new()
        }
        ComposerField::BccToggle => {
            if let Some(composer) = state.session.composer.as_mut() {
                composer.show_bcc = true;
                composer.enter_bcc();
            }
            Vec::new()
        }
        ComposerField::Attach => open_attachment_dialog(state),
        ComposerField::Attachment(index) => remove_attachment(state, index),
        ComposerField::Send => reduce(state, Action::Send),
        ComposerField::Discard => reduce(state, Action::DiscardDraft),
        ComposerField::To | ComposerField::Cc | ComposerField::Bcc | ComposerField::Subject => {
            Vec::new()
        }
    }
}

/// Open the attachment file chooser (plan §15, ticket 95x0): a
/// `ratatui_explorer` listing over the user's home directory. The
/// listing itself is backend work — the dialog opens immediately in its
/// pending state and the explorer arrives with the result. No-op without
/// a composer.
pub(crate) fn open_attachment_dialog(state: &mut AppState) -> Vec<Effect> {
    if state.session.composer.is_none() {
        return Vec::new();
    }
    state.session.overlay = Some(Overlay::AttachmentExplorer(Box::new(
        AttachmentFileDialog {
            explorer: None,
            listing: true,
            error: None,
            previous_focus: state.session.focus,
        },
    )));
    state.session.focus = Focus::Dialog;
    vec![
        state
            .session
            .operations
            .start(OperationKind::ListAttachmentFiles { path: None }),
    ]
}

/// Remove the focused attachment chip (plan §15: "allow removal before
/// send"). Removal is a content edit: the revision bumps and autosave
/// journals the shorter attachment list.
pub(crate) fn remove_attachment(state: &mut AppState, index: usize) -> Vec<Effect> {
    let Some(composer) = state.session.composer.as_mut() else {
        return Vec::new();
    };
    let removed = composer
        .draft
        .attachments
        .get(index)
        .map(|att| att.name.clone());
    composer.remove_attachment(index);
    if let Some(name) = removed {
        composer.draft.note_edit(state.session.clock);
        state.set_status(format!("Removed {name}"));
    }
    Vec::new()
}

/// Enter (or a second click) on a selected list row. A draft in the
/// mailbox with the `Drafts` role reopens in the composer instead of the
/// reader — the one context where Enter composes (plan §14: reopening
/// continues the draft); every other message opens the reader.
pub(crate) fn open_selected(
    state: &mut AppState,
    summary: crate::domain::MessageSummary,
) -> Vec<Effect> {
    if state.active_mailbox_role() == Some(MailboxRole::Drafts) {
        return open_draft_message(state, summary);
    }
    open_message(state, summary)
}

/// Whether the in-memory draft and the listed message are the same draft:
/// matched on the stable RFC `Message-ID` (envelope listings carry bare
/// ids, snapshots the bracketed form) or on the backend id of the last
/// confirmed remote copy.
pub(crate) fn is_same_draft(
    draft: &crate::domain::Draft,
    summary: &crate::domain::MessageSummary,
) -> bool {
    let ids_match = match (&draft.message_id, &summary.message_id) {
        (Some(draft_id), Some(summary_id)) => {
            bare_message_id(draft_id) == bare_message_id(summary_id)
        }
        _ => false,
    };
    ids_match || draft.remote_id.as_ref() == Some(&summary.id)
}

/// Whether a composer screen is open right now (the one-composer rule,
/// plan §14): a draft the user is editing — or parked with Tab while
/// picking a mailbox — is never clobbered. A draft left behind (`Esc`
/// saved it) stays in `AppState.session.composer` for the Drafts list, but it no
/// longer blocks a reply/forward (ticket 61qx): the seed replaces it, and
/// its possibly in-flight save result is dropped by the local_id currency
/// check in `save_draft_completed`.
pub(crate) fn composer_open(state: &AppState) -> bool {
    matches!(state.active_route(), Some(Route::Composer))
}

/// Secure the draft parked in the composer slot before it is replaced
/// (ticket sazy): a forced save of its newest revision, unless a save of
/// exactly that revision is already in flight (it carries the same
/// content). Drafts are durable in the Drafts mailbox, so a secured parked
/// draft never blocks another one from opening.
pub(crate) enum Secured {
    /// Nothing to do: no parked draft, it is clean, or its newest
    /// revision is already being pushed.
    Nothing,
    /// The save effect to launch before replacing the slot.
    Save(Effect),
    /// The parked draft is dirty but cannot be secured yet (no clock —
    /// before the first tick): the caller must not replace it.
    Cannot,
}

pub(crate) fn secure_parked_draft(state: &mut AppState) -> Secured {
    let Some(composer) = state.session.composer.as_ref() else {
        return Secured::Nothing;
    };
    if !composer.draft.is_dirty() {
        return Secured::Nothing;
    }
    let in_flight = composer.draft.local_id.as_ref().is_some_and(|local_id| {
        state
            .session
            .operations
            .is_saving_draft(local_id, composer.draft.revision)
    });
    if in_flight {
        return Secured::Nothing;
    }
    match draft_save_effect(state) {
        Some(effect) => Secured::Save(effect),
        None => Secured::Cannot,
    }
}

/// Enter on a message in the Drafts mailbox (plan §14). The in-memory
/// draft (left open earlier, or restored at startup) is the newest known
/// state of itself — reopening continues it without a backend round-trip.
/// A *different* parked draft never blocks (ticket sazy): it is secured
/// with a forced save and the fetched draft takes the composer slot when
/// it lands. The only remaining refusal is a dirty draft with no clock
/// yet — before the first tick — where the save cannot be stamped.
pub(crate) fn open_draft_message(
    state: &mut AppState,
    summary: crate::domain::MessageSummary,
) -> Vec<Effect> {
    let same = state
        .session
        .composer
        .as_ref()
        .is_some_and(|c| is_same_draft(&c.draft, &summary));
    if same {
        open_composer_screen(state);
        return Vec::new();
    }
    let mut effects = Vec::new();
    match secure_parked_draft(state) {
        Secured::Save(effect) => effects.push(effect),
        Secured::Cannot => {
            state.set_status("Still starting up — try again in a moment");
            return Vec::new();
        }
        Secured::Nothing => {}
    }
    let locator = summary.into_locator();
    state.set_status("Opening draft…");
    effects.push(
        state
            .session
            .operations
            .start(OperationKind::OpenDraft(locator)),
    );
    effects
}

/// Apply a fetched draft copy (the `OpenDraft` result): turn it into a
/// composer draft and open the composer, replacing the draft parked in
/// the slot. The parked copy is secured the same way the intent secured
/// it (`secure_parked_draft`) — a draft composed or restored while the
/// fetch ran is saved to the Drafts mailbox, never clobbered — and its
/// late save result drops on the local_id mismatch (the fetched copy
/// carries a fresh identity). Currency checks upstream: the drafts
/// mailbox must still be displayed. The selected row must still be this
/// draft, so the draft that opens is the one the cursor is on (a moved
/// selection drops the stale fetch).
pub(crate) fn draft_message_loaded(
    state: &mut AppState,
    message: crate::domain::Message,
) -> Vec<Effect> {
    let selected = state
        .selected_message()
        .is_some_and(|row| row.id == message.id);
    if !selected {
        tracing::debug!(
            id = %message.id.0,
            "draft fetch dropped: the selection moved on"
        );
        return Vec::new();
    }
    let mut effects = Vec::new();
    match secure_parked_draft(state) {
        Secured::Save(effect) => effects.push(effect),
        Secured::Cannot => {
            tracing::debug!(
                id = %message.id.0,
                "draft fetch dropped: the parked draft cannot be secured yet"
            );
            return Vec::new();
        }
        Secured::Nothing => {}
    }
    let draft = crate::domain::draft_from_message(&message);
    install_composer_draft(state, draft, "Draft opened");
    effects
}

/// Open the selected message: push the reader route, snapshot the summary,
/// and show the cached copy if one exists (with a silent background
/// convergence fetch); otherwise fetch the full message in the foreground.
/// The list page, selection, and scroll stay untouched so `Esc` restores
/// them exactly (plan §19 Phase 4).
pub(crate) fn open_message(
    state: &mut AppState,
    summary: crate::domain::MessageSummary,
) -> Vec<Effect> {
    let mailbox_id = summary.mailbox_id.clone();
    let locator = summary.into_locator();
    state.session.routes.push(Route::Message(MessageRoute {
        mailbox_id,
        summary,
    }));
    state.session.focus = Focus::Reader;
    state.reader_scroll = 0;
    state.reader_focus = None;
    // The cached copy renders instantly and the convergence fetch runs
    // silently in the background (never cancellable, no loader slot): its
    // result converges read/unread state and any remote drift (ticket
    // haeb). Missing from the cache is the ordinary path: a foreground
    // load with the centered spinner. The read itself runs off-thread
    // (the manager owns the cache); the spinner shows until its result
    // lands, then either the cached body or the foreground load takes
    // over.
    state.open_message = Loadable::Loading;
    vec![
        state
            .session
            .operations
            .start_background(OperationKind::CacheMessageLoad { locator }),
    ]
}

/// Close the reader if it is open: pop its route and drop its data. The
/// mailbox route underneath was never mutated, so page, selection, focus,
/// and scroll are restored by construction.
pub(crate) fn close_reader(state: &mut AppState) {
    if matches!(state.active_route(), Some(Route::Message(_))) {
        state.session.routes.pop();
        state.open_message = Loadable::Idle;
        state.reader_scroll = 0;
        state.reader_focus = None;
        state.session.focus = Focus::MessageList;
    }
}
