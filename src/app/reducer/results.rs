//! Backend results (plan §11): `backend_completed` dispatch and every
//! completion handler. Results apply only while their operation is still
//! registered; stale, cancelled, or superseded results never win.
use super::actions::attachment_saved;
use super::composer_flow::draft_save_effect;
use super::message_results::{
    FlagChange, apply_flag, apply_page, complete_cache_preview_load, mailboxes_loaded,
    message_loaded, message_moved, preview_loaded, visible_mailbox_page,
};
use super::modals::open_error_modal;
use super::navigation::{
    complete_cache_list_load, complete_cache_mailboxes_load, complete_cache_message_load,
    draft_message_loaded, request_visible_page_background,
};
use super::seeding::install_seed;
use super::send::send_completed;
use crate::app::composer::{ComposerField, ComposerState};
use crate::app::effect::Effect;
use crate::app::operation::{
    DraftRemovalReason, NotifyRequest, OperationFailure, OperationId, OperationKind,
    OperationOrigin, OperationOutcome, OperationResult,
};
use crate::app::overlay::Overlay;
use crate::app::route::Route;
use crate::app::state::{AppState, Loadable};
use crate::config::Notifications;
use crate::domain::{MessageLocator, MessageSummary, Page, PageRequest, SearchRequest};

// ── Backend results (plan §11) ───────────────────────────────────────────

/// Failure handling for list-shaped results (mailbox pages and searches,
/// Phase 9): a *foreground* failure opens the Retry/Dismiss modal; a
/// *background* (timer) refresh failure never interrupts the user — the
/// first failure lands in the status line, and repeated identical failures
/// are suppressed until a success or a manual refresh clears the record
/// (Phase 9.6).
pub(crate) fn list_failure(
    state: &mut AppState,
    failure: &OperationFailure,
    origin: OperationOrigin,
) {
    if origin == OperationOrigin::Background {
        if state.session.last_background_error.as_deref() != Some(failure.detail.as_str()) {
            tracing::info!(detail = %failure.detail, "background refresh failed");
            state.set_status("Refresh failed — the timer will retry");
            state.session.last_background_error = Some(failure.detail.clone());
        } else {
            tracing::debug!("identical background refresh failure; status unchanged");
        }
        return;
    }
    // The last coherent page stays visible; the modal offers Retry/Dismiss
    // (plan §12).
    open_error_modal(state, failure);
}

/// Finish a successful *background* page update for notifications (ticket
/// b28p): the page becomes the new clean baseline, and the leading run of
/// ids the previous baseline did not contain is the mail that arrived
/// since the last boundary. With notifications enabled and the terminal
/// unfocused, that run turns into a bell or desktop-notification effect;
/// otherwise the update stays silent. The manager delivers the
/// notification off-thread, so nothing here (or there) blocks the UI.
pub(crate) fn notify_new_messages(
    state: &mut AppState,
    page: &Page<MessageSummary>,
) -> Vec<Effect> {
    let arrived: Vec<MessageSummary> = state
        .session
        .notifications
        .new_messages(page)
        .into_iter()
        .cloned()
        .collect();
    // The finish boundary: whatever the update found is clean now.
    state.session.notifications.mark_clean(page);
    if arrived.is_empty()
        || state.settings.notifications == Notifications::Off
        || state.session.terminal_focused
    {
        return Vec::new();
    }
    let request = match state.settings.notifications {
        Notifications::Off => unreachable!("filtered above"),
        Notifications::Bell => NotifyRequest::Bell,
        Notifications::On => desktop_request(&arrived),
    };
    vec![
        state
            .session
            .operations
            .start_background(OperationKind::Notify { request }),
    ]
}

/// The desktop notification for `arrived` (ticket b28p): a single message
/// names its sender and subject; several list the count.
pub(crate) fn desktop_request(arrived: &[MessageSummary]) -> NotifyRequest {
    match arrived {
        [message] => NotifyRequest::Desktop {
            summary: message.from_display().to_owned(),
            body: message.subject.clone(),
        },
        _ => NotifyRequest::Desktop {
            summary: String::from("Tmail"),
            body: format!("{} new messages", arrived.len()),
        },
    }
}

/// Apply a backend result. Results for unknown, cancelled, or superseded
/// operation ids never mutate state: `finish` removes the operation, and a
/// superseded operation was already cancelled and removed when its
/// replacement started.
/// A result payload that cannot belong to this operation kind: wiring
/// bugs of the manager/reducer contract, never user-facing — log and
/// keep the state untouched.
pub(crate) fn unexpected_payload(id: OperationId, kind: &str) -> Vec<Effect> {
    tracing::warn!(id = %id, "unexpected payload for a {kind} operation");
    Vec::new()
}

pub(crate) fn backend_completed(state: &mut AppState, result: &OperationResult) -> Vec<Effect> {
    let Some(op) = state.session.operations.get(result.id) else {
        tracing::debug!(id = %result.id, "dropping result for unknown or cancelled operation");
        return Vec::new();
    };
    let kind = op.kind.clone();
    let origin = op.origin;
    // The result is consumed exactly once before dispatch: a handler's
    // currency check reads only routes and state, and the operation must
    // not count as in flight while its outcome is applied.
    state.session.operations.finish(result.id);
    match &kind {
        OperationKind::LoadMailboxes => complete_load_mailboxes(state, origin, result),
        OperationKind::LoadPage(request) => complete_load_page(state, request, origin, result),
        OperationKind::Search(request) => complete_search(state, request, origin, result),
        OperationKind::LoadMessage(locator) => {
            complete_load_message(state, locator, origin, result)
        }
        OperationKind::OpenDraft(locator) => complete_open_draft(state, locator, result),
        OperationKind::Preview(_) => complete_preview(state, result),
        OperationKind::SeedComposer { kind, .. } => complete_seed_composer(state, result, *kind),
        OperationKind::SetRead { locator, read } => {
            complete_flag(state, result, locator, FlagChange::Read(*read))
        }
        OperationKind::SetStarred { locator, starred } => {
            complete_flag(state, result, locator, FlagChange::Starred(*starred))
        }
        OperationKind::Archive(locator) | OperationKind::Trash(locator) => {
            complete_move(state, result, locator)
        }
        OperationKind::SaveDraft { draft } => save_draft_completed(state, draft, result),
        OperationKind::LoadDrafts => complete_load_drafts(state, result),
        OperationKind::DeleteDraft { reason, .. } => complete_delete_draft(state, result, reason),
        OperationKind::Send { message } => complete_send(state, result, message),
        OperationKind::ReadAttachment { path } => attachment_validated(state, path, result),
        OperationKind::ListAttachmentFiles { .. } => attachment_listing_ready(state, result),
        OperationKind::SaveAttachment { open_after, .. } => {
            complete_save_attachment(state, result, *open_after)
        }
        OperationKind::OpenPath { .. } => complete_open_path(state, result),
        OperationKind::OpenUrl { .. } => complete_open_url(state, result),
        // Notifications are best-effort side effects (ticket b28p): the
        // manager reports the attempt, state has nothing to apply.
        OperationKind::Notify { .. } => {
            tracing::debug!(id = %result.id, "notification delivered");
            Vec::new()
        }
        // The external editor completes through `Action::EditorFinished`,
        // not the result channel (Phase 11: it runs on the terminal owner,
        // not in the manager). A result arriving here would be a routing
        // bug; the operation is consumed above so the registry cannot leak.
        OperationKind::EditExternally { .. } => {
            tracing::warn!(id = %result.id, "result for an external-editor operation");
            Vec::new()
        }
        // Wizard operations complete through the wizard slice, which
        // intercepts `BackendCompleted` first (ADR 0003 §3.7). Reaching
        // this arm would be a routing bug; the operation is consumed above
        // so nothing leaks.
        OperationKind::DiscoverConfig { .. }
        | OperationKind::TestAccount { .. }
        | OperationKind::SaveAccount { .. } => {
            tracing::warn!(id = %result.id, "wizard result reached the main backend path");
            Vec::new()
        }
        // ── Summary/message cache (ticket haeb) ─────────────────────────
        //
        // The cache lives in the operation manager; the reducer only
        // applies what a read served and emits writes as effects.
        OperationKind::CacheListLoad {
            mailbox,
            query,
            offset,
            limit,
            fresh_background_on_hit,
        } => complete_cache_list_load(
            state,
            mailbox,
            query.as_deref(),
            *offset,
            *limit,
            *fresh_background_on_hit,
            result,
        ),
        OperationKind::CacheMailboxesLoad => complete_cache_mailboxes_load(state, result),
        OperationKind::CacheMessageLoad { locator } => {
            complete_cache_message_load(state, locator, result)
        }
        OperationKind::CachePreviewLoad { locator } => {
            complete_cache_preview_load(state, locator, result)
        }
        // Writes are best-effort side effects: the manager logs failures
        // inside the cache, and a dropped write can never lose mail —
        // only warmth.
        OperationKind::CacheListStore { .. }
        | OperationKind::CacheMailboxesStore { .. }
        | OperationKind::CacheMessageStore { .. } => {
            if let Err(failure) = &result.outcome {
                tracing::debug!(id = %result.id, detail = %failure.detail, "cache write failed");
            }
            Vec::new()
        }
    }
}

/// Apply a finished mailbox listing: refresh the cached sidebar and hand
/// the listing to `mailboxes_loaded`. A *background* refresh (the
/// discard-sweep recount and friends) never erases a healthy sidebar: a
/// hiccup just keeps what the user already sees, silently.
pub(crate) fn complete_load_mailboxes(
    state: &mut AppState,
    origin: OperationOrigin,
    result: &OperationResult,
) -> Vec<Effect> {
    match &result.outcome {
        Ok(OperationOutcome::Mailboxes(mailboxes)) => {
            // Ticket haeb: every successful listing refreshes the cached
            // sidebar — as an effect, so the write never blocks the
            // reducer (the manager owns the cache and the disk).
            let mut effects = mailboxes_loaded(state, mailboxes.clone());
            effects.push(state.session.operations.start_background(
                OperationKind::CacheMailboxesStore {
                    mailboxes: mailboxes.clone(),
                },
            ));
            effects
        }
        Ok(OperationOutcome::Page(_)) => {
            tracing::warn!(id = %result.id, "page payload for a mailbox operation");
            Vec::new()
        }
        Ok(_) => unexpected_payload(result.id, "mailbox"),
        Err(failure) => {
            // A background refresh failure never erases a healthy sidebar:
            // what the user sees is good, the hiccup is logged. When
            // nothing was loaded yet (a cold start), the sidebar carries
            // the failure note — the user's retry is `Ctrl+R`; no modal
            // interrupts background work.
            if origin == OperationOrigin::Background
                && matches!(state.mailboxes, Loadable::Loaded(_))
            {
                tracing::debug!(
                    id = %result.id,
                    detail = %failure.detail,
                    "background mailbox refresh failed; sidebar untouched"
                );
                return Vec::new();
            }
            // The sidebar keeps a dim failed note; the modal carries the
            // full sanitized detail and the retry intent.
            state.mailboxes = Loadable::Failed(failure.detail.clone());
            if origin == OperationOrigin::Background {
                return Vec::new();
            }
            open_error_modal(state, failure)
        }
    }
}

/// Apply a finished mailbox page. Currency check: the request must still
/// target the mailbox whose page the visible list shows. A newer request
/// for the same mailbox superseded this operation, so its id would already
/// be unknown; this guard drops results that raced a mailbox switch or a
/// search taking over the list. Reader and composer routes are overlays: a
/// load finishing while the user reads or composes still updates the list
/// behind them — routes, focus, and selection untouched (ticket sazy).
pub(crate) fn complete_load_page(
    state: &mut AppState,
    request: &PageRequest,
    origin: OperationOrigin,
    result: &OperationResult,
) -> Vec<Effect> {
    let current = visible_mailbox_page(state).is_some_and(|id| *id == request.mailbox_id);
    if !current {
        tracing::debug!(
            mailbox = %request.mailbox_id.0,
            offset = request.offset,
            "dropping page result for inactive mailbox"
        );
        return Vec::new();
    }
    match &result.outcome {
        Ok(OperationOutcome::Page(page)) => {
            state.session.last_background_error = None;
            let mut effects = Vec::new();
            // Timer refreshes report new mail (ticket b28p); foreground
            // loads are user-driven and stay silent.
            if origin == OperationOrigin::Background {
                effects.extend(notify_new_messages(state, page));
            }
            effects.extend(apply_page(state, page.clone()));
            // Ticket haeb: every successful load refreshes the cached page —
            // with the previews applied (ticket wxtx), so the next cold
            // start renders rows without re-fetching anything. The write
            // travels as an effect: the manager owns the cache and the disk.
            effects.push(state.session.operations.start_background(
                OperationKind::CacheListStore {
                    mailbox: request.mailbox_id.clone(),
                    query: None,
                    page: Box::new(state.messages.clone()),
                },
            ));
            effects
        }
        Ok(OperationOutcome::Mailboxes(_)) => {
            tracing::warn!(id = %result.id, "mailbox payload for a page operation");
            Vec::new()
        }
        Err(failure) => {
            // Foreground failures open the Retry/Dismiss modal; background
            // (timer) refresh failures never interrupt the user (Phase 9.6).
            list_failure(state, failure, origin);
            Vec::new()
        }
        _ => unexpected_payload(result.id, "page"),
    }
}

/// Apply a finished search page. Currency check: the results must belong
/// to the open search — same query and mailbox (a re-submit supersedes the
/// older operation, so only races with navigation land here).
pub(crate) fn complete_search(
    state: &mut AppState,
    request: &SearchRequest,
    origin: OperationOrigin,
    result: &OperationResult,
) -> Vec<Effect> {
    let current = matches!(
        state.active_route(),
        Some(Route::Search(route))
            if route.mailbox_id == request.mailbox_id && route.query == request.query
    );
    if !current {
        tracing::debug!(
            id = %result.id,
            mailbox = %request.mailbox_id.0,
            "dropping search result for a closed or changed search"
        );
        return Vec::new();
    }
    match &result.outcome {
        Ok(OperationOutcome::Page(page)) => {
            state.session.last_background_error = None;
            let mut effects = Vec::new();
            // A timer refresh of an open search reports matching new mail
            // like a mailbox refresh does (ticket b28p).
            if origin == OperationOrigin::Background {
                effects.extend(notify_new_messages(state, page));
            }
            effects.extend(apply_page(state, page.clone()));
            // Ticket haeb: search results cache under their query — with
            // the previews applied (ticket wxtx). The write travels as an
            // effect: the manager owns the cache and the disk.
            effects.push(state.session.operations.start_background(
                OperationKind::CacheListStore {
                    mailbox: request.mailbox_id.clone(),
                    query: Some(request.query.clone()),
                    page: Box::new(state.messages.clone()),
                },
            ));
            effects
        }
        Ok(_) => unexpected_payload(result.id, "search"),
        Err(failure) => {
            list_failure(state, failure, origin);
            Vec::new()
        }
    }
}

/// Apply a finished message fetch. Currency check: the reader must still
/// show this message.
pub(crate) fn complete_load_message(
    state: &mut AppState,
    locator: &MessageLocator,
    origin: OperationOrigin,
    result: &OperationResult,
) -> Vec<Effect> {
    let current = matches!(
        state.active_route(),
        Some(Route::Message(route))
            if route.mailbox_id == locator.mailbox && route.summary.id == locator.id
    );
    if !current {
        tracing::debug!(
            id = %result.id,
            mailbox = %locator.mailbox.0,
            "dropping message result for a closed reader"
        );
        return Vec::new();
    }
    match &result.outcome {
        Ok(OperationOutcome::Message(message)) => message_loaded(state, (**message).clone()),
        Ok(_) => unexpected_payload(result.id, "message"),
        Err(failure) => {
            // A background convergence fetch of an already-cached message
            // runs silently: the reader already shows good content, so a
            // remote hiccup (or a stale maildir id drift) just keeps the
            // cached copy instead of interrupting with a modal.
            if origin == OperationOrigin::Background {
                tracing::debug!(
                    id = %result.id,
                    detail = %failure.detail,
                    "silent convergence fetch failed; cached copy stays"
                );
                return Vec::new();
            }
            // The reader shows a failure placeholder; the modal carries
            // Retry/Dismiss (plan §12). Coherent state.
            state.open_message = Loadable::Failed(failure.detail.clone());
            open_error_modal(state, failure)
        }
    }
}

/// Apply a fetched draft copy. Currency check: the drafts mailbox must
/// still be displayed (a switch, a reader, or a composer opened meanwhile
/// drops the result — Enter again refetches).
pub(crate) fn complete_open_draft(
    state: &mut AppState,
    locator: &MessageLocator,
    result: &OperationResult,
) -> Vec<Effect> {
    let current = match state.active_route() {
        Some(Route::Mailbox(route)) => route.mailbox_id == locator.mailbox,
        Some(Route::Search(route)) => route.mailbox_id == locator.mailbox,
        _ => false,
    };
    if !current {
        tracing::debug!(
            id = %result.id,
            mailbox = %locator.mailbox.0,
            "dropping draft result for a closed context"
        );
        return Vec::new();
    }
    match &result.outcome {
        Ok(OperationOutcome::Message(message)) => draft_message_loaded(state, (**message).clone()),
        Ok(_) => unexpected_payload(result.id, "draft"),
        Err(failure) => open_error_modal(state, failure),
    }
}

/// Apply a finished preview fetch. A preview is decorative background
/// context (ticket wxtx): its failure never interrupts the user and is
/// never retried — the row simply keeps no snippet.
pub(crate) fn complete_preview(state: &mut AppState, result: &OperationResult) -> Vec<Effect> {
    match &result.outcome {
        Ok(OperationOutcome::Message(message)) => preview_loaded(state, (**message).clone()),
        Ok(_) => unexpected_payload(result.id, "preview"),
        Err(failure) => {
            tracing::debug!(
                id = %result.id,
                detail = %failure.detail,
                "preview fetch failed; row stays without a snippet"
            );
            Vec::new()
        }
    }
}

/// Apply a seed fetch (reply/reply-all/forward from the list): the fetched
/// message becomes the composer draft.
pub(crate) fn complete_seed_composer(
    state: &mut AppState,
    result: &OperationResult,
    kind: crate::app::operation::SeedKind,
) -> Vec<Effect> {
    match &result.outcome {
        Ok(OperationOutcome::Message(message)) => install_seed(state, (**message).clone(), kind),
        Ok(_) => unexpected_payload(result.id, "composer seed"),
        Err(failure) => open_error_modal(state, failure),
    }
}

/// Apply one confirmed flag change. The flag commands echo affected flags,
/// not resulting state (ADR 0001 finding 6): the confirmed request is the
/// state.
pub(crate) fn complete_flag(
    state: &mut AppState,
    result: &OperationResult,
    locator: &MessageLocator,
    change: FlagChange,
) -> Vec<Effect> {
    match &result.outcome {
        Ok(OperationOutcome::Done) => {
            apply_flag(state, locator, change);
        }
        Ok(_) => {
            unexpected_payload(result.id, "flag");
        }
        Err(failure) => {
            open_error_modal(state, failure);
        }
    }
    Vec::new()
}

/// Apply a confirmed archive/trash move. Nothing is removed locally on
/// failure: the list/reader still show the message (plan §12 coherent
/// failure state).
pub(crate) fn complete_move(
    state: &mut AppState,
    result: &OperationResult,
    locator: &MessageLocator,
) -> Vec<Effect> {
    match &result.outcome {
        Ok(OperationOutcome::Done) => message_moved(state, locator),
        Ok(_) => unexpected_payload(result.id, "move"),
        Err(failure) => open_error_modal(state, failure),
    }
}

/// Apply the journal restore result. The journal is the crash-safety net;
/// a failure to read it must be visible (plan §12) even though mail
/// browsing can continue without drafts.
pub(crate) fn complete_load_drafts(state: &mut AppState, result: &OperationResult) -> Vec<Effect> {
    match &result.outcome {
        Ok(OperationOutcome::Drafts(drafts)) => drafts_restored(state, drafts),
        Ok(_) => unexpected_payload(result.id, "draft restore"),
        Err(failure) => open_error_modal(state, failure),
    }
}

/// Apply a draft removal. A discard applied its local half optimistically
/// when the confirm dialog was accepted (Phase 6.6); a send removed the
/// composer on confirmation (Phase 7.6). Failures follow the removal
/// reason: discards open the modal, tmail-send cleanup is best-effort
/// (ADR 0002) and never claims a failed send.
pub(crate) fn complete_delete_draft(
    state: &mut AppState,
    result: &OperationResult,
    reason: &DraftRemovalReason,
) -> Vec<Effect> {
    match &result.outcome {
        Ok(OperationOutcome::Done) => match reason {
            // Confirmed discard: the sweep removed the draft from the
            // backing store — refresh what the user sees: the visible
            // page (the deleted draft's row leaves the list) and the
            // mailbox listing (folder counts, e.g. `Drafts (6)`). The
            // scoped backend delete awaits its sweep, so both listings
            // now see a clean Drafts mailbox.
            DraftRemovalReason::Discard => {
                // Both refreshes run in the background: the discard is
                // done and its cleanup work must never sit in the
                // foreground slot where `Esc` would cancel it into
                // "loading mailboxes — cancelled".
                let mut effects = vec![
                    state
                        .session
                        .operations
                        .start_background(OperationKind::LoadMailboxes),
                ];
                effects.extend(request_visible_page_background(
                    state,
                    state.messages.offset,
                ));
                effects
            }
            DraftRemovalReason::Sent => Vec::new(),
        },
        Ok(_) => unexpected_payload(result.id, "draft removal"),
        Err(failure) => match reason {
            DraftRemovalReason::Discard => open_error_modal(state, failure),
            DraftRemovalReason::Sent => {
                tracing::warn!(
                    detail = %failure.detail,
                    "the sent message's draft copy could not be removed"
                );
                Vec::new()
            }
        },
    }
}

/// Apply a classified send result. A structural refusal (missing identity,
/// spawn I/O) means nothing was transmitted: the draft stays intact and
/// editable (plan §19 Phase 7: failed send keeps it).
pub(crate) fn complete_send(
    state: &mut AppState,
    result: &OperationResult,
    message: &crate::domain::OutboundMessage,
) -> Vec<Effect> {
    match &result.outcome {
        Ok(OperationOutcome::SendOutcome(outcome)) => send_completed(state, outcome, message),
        Ok(_) => unexpected_payload(result.id, "send"),
        Err(failure) => {
            if let Some(composer) = state.session.composer.as_mut() {
                composer.sending = false;
            }
            state.set_status("Send failed");
            open_error_modal(state, failure)
        }
    }
}

/// Apply a finished attachment save; `open_after` chains the platform
/// opener on the confirmed path — which may be a collision-renamed name,
/// so the chain uses exactly what was written (Phase 8.5).
pub(crate) fn complete_save_attachment(
    state: &mut AppState,
    result: &OperationResult,
    open_after: bool,
) -> Vec<Effect> {
    match &result.outcome {
        Ok(OperationOutcome::SavedPath(path)) => {
            attachment_saved(state, path);
            if open_after {
                return vec![
                    state
                        .session
                        .operations
                        .start(OperationKind::OpenPath { path: path.clone() }),
                ];
            }
            Vec::new()
        }
        Ok(_) => unexpected_payload(result.id, "attachment save"),
        Err(failure) => open_error_modal(state, failure),
    }
}

/// Apply a finished `open` spawn.
pub(crate) fn complete_open_path(state: &mut AppState, result: &OperationResult) -> Vec<Effect> {
    match &result.outcome {
        Ok(OperationOutcome::Done) => {
            state.set_status("Opened");
            Vec::new()
        }
        Ok(_) => unexpected_payload(result.id, "open"),
        Err(failure) => open_error_modal(state, failure),
    }
}

/// Apply a finished link-open spawn (ticket hc9n). A failure (no browser
/// handler, missing opener) surfaces in the Retry/Dismiss modal with the
/// URL named, like every other open.
pub(crate) fn complete_open_url(state: &mut AppState, result: &OperationResult) -> Vec<Effect> {
    match &result.outcome {
        Ok(OperationOutcome::Done) => {
            state.set_status("Opened link");
            Vec::new()
        }
        Ok(_) => unexpected_payload(result.id, "open link"),
        Err(failure) => open_error_modal(state, failure),
    }
}

/// Apply the journal restore (ADR 0002 §D.5): rebuild the newest draft
/// into the composer so composing after a crash continues it. Never
/// clobbers a live composer. A gap between the recorded and
/// remote-confirmed revisions leaves the draft dirty, so the autosave
/// self-heals the gap once composing resumes.
pub(crate) fn drafts_restored(
    state: &mut AppState,
    drafts: &[crate::domain::RestoredDraft],
) -> Vec<Effect> {
    let Some(restored) = drafts.iter().max_by_key(|entry| entry.draft.revision) else {
        return Vec::new();
    };
    if state.session.composer.is_some() {
        tracing::debug!("draft restore skipped: a composer draft already exists");
        return Vec::new();
    }
    let snapshot = restored.draft.clone();
    // A gap between the recorded and remote-confirmed revisions means the
    // crash interrupted a push: restore the draft dirty so the autosave
    // self-heals it once the window elapses.
    let save = if snapshot.revision > restored.saved_revision {
        crate::domain::DraftSaveState::Debouncing
    } else {
        crate::domain::DraftSaveState::Saved
    };
    let draft = crate::domain::Draft {
        to: snapshot.to,
        cc: snapshot.cc,
        bcc: snapshot.bcc,
        subject: snapshot.subject,
        body: snapshot.body,
        attachments: snapshot.attachments,
        revision: snapshot.revision,
        saved_revision: restored.saved_revision,
        saved_at: None,
        last_edit_at: state.session.clock,
        save,
        local_id: Some(snapshot.local_id),
        message_id: snapshot.message_id,
        in_reply_to: snapshot.in_reply_to,
        references: snapshot.references,
        remote_id: snapshot.remote_id,
    };
    tracing::info!(
        local_id = %draft.local_id.as_ref().map(|id| id.0.as_str()).unwrap_or("?"),
        revision = draft.revision,
        saved_revision = draft.saved_revision,
        "draft restored from journal"
    );
    state.session.composer = Some(ComposerState::from_draft(draft));
    Vec::new()
}

/// Apply a confirmed draft save (plan §14). Currency check: the draft must
/// still be the one in the composer (a discarded draft is gone; a draft
/// swapped out by opening another one is no longer in the slot — the
/// backend already has the pushed copy, so the local confirm is dropped).
/// Success for the newest revision marks the draft saved; a stale success
/// (edits happened meanwhile) immediately chains another save so revision
/// N+1 is never left unpushed.
pub(crate) fn save_draft_completed(
    state: &mut AppState,
    snapshot: &crate::domain::DraftSnapshot,
    result: &OperationResult,
) -> Vec<Effect> {
    let current = state
        .session
        .composer
        .as_ref()
        .is_some_and(|c| c.draft.local_id.as_ref() == Some(&snapshot.local_id));
    if !current {
        tracing::debug!(
            local_id = %snapshot.local_id.0,
            "dropping draft result for a discarded or replaced draft"
        );
        return Vec::new();
    }
    let composer = state.session.composer.as_mut().expect("checked above");
    match &result.outcome {
        Ok(OperationOutcome::DraftSaved { remote_id }) => {
            let chain = composer.draft.confirm_saved(
                snapshot.revision,
                remote_id.clone(),
                state.session.clock,
            );
            if chain {
                // Remain dirty and save again (plan §14): one follow-up
                // save covering the newest revision. It supersedes nothing
                // in flight — the previous save just completed.
                draft_save_effect(state).into_iter().collect()
            } else {
                Vec::new()
            }
        }
        Err(failure) => {
            // Unsaved state + Retry/Dismiss modal; content retained
            // (plan §14 acceptance). A newer revision debounce outranks
            // the stale failure — its scheduled save retries anyway.
            if snapshot.revision >= composer.draft.revision {
                composer.draft.mark_failed();
            }
            open_error_modal(state, failure);
            state.set_status("Draft save failed");
            Vec::new()
        }
        Ok(_) => unexpected_payload(result.id, "draft save"),
    }
}

/// Apply a finished attachment validation (plan §15, ticket 95x0).
/// Currency: the dialog must still be open, and the chooser's selection
/// must still be the validated file — navigation in the meantime drops
/// the result. A validated file becomes a chip and a content edit
/// (autosave carries the attachment list into the journal); a rejection
/// keeps the chooser open with the detail inline, retryable by pressing
/// Enter on the file again.
pub(crate) fn attachment_validated(
    state: &mut AppState,
    path: &std::path::Path,
    result: &OperationResult,
) -> Vec<Effect> {
    let Some(Overlay::AttachmentExplorer(dialog)) = state.session.overlay.as_ref() else {
        tracing::debug!(id = %result.id, "dropping attachment validation for a closed dialog");
        return Vec::new();
    };
    let selection_matches = dialog
        .selected_file()
        .is_some_and(|selected| selected == path);
    if !selection_matches {
        tracing::debug!(id = %result.id, "dropping attachment validation for a moved selection");
        return Vec::new();
    }
    match &result.outcome {
        Ok(OperationOutcome::Attachment(att)) => {
            let att = att.clone();
            let focus = dialog.previous_focus;
            state.session.overlay = None;
            state.session.focus = focus;
            let Some(composer) = state.session.composer.as_mut() else {
                tracing::debug!("validated attachment ignored: no composer");
                return Vec::new();
            };
            let name = att.name.clone();
            let size = att.size;
            if composer.add_attachment(att) {
                let last = composer.draft.attachments.len() - 1;
                composer.field = ComposerField::Attachment(last);
                // Attaching is a content edit: the revision bumps and the
                // autosave journal carries the new attachment list.
                composer.draft.note_edit(state.session.clock);
                state.set_status(format!(
                    "Attached {name} ({})",
                    crate::view::text::human_size(size)
                ));
            } else {
                state.set_status(format!("{name} is already attached"));
            }
            Vec::new()
        }
        Err(failure) => {
            // Detailed and retryable in place: the chooser stays open with
            // the same selection (plan §15 acceptance, ticket 95x0).
            let detail = failure.detail.clone();
            if let Some(Overlay::AttachmentExplorer(dialog)) = state.session.overlay.as_mut() {
                dialog.error = Some(detail);
            }
            Vec::new()
        }
        Ok(_) => unexpected_payload(result.id, "attachment validation"),
    }
}

/// Apply a finished directory listing (ticket 95x0). Currency: the
/// dialog must still be open — an Esc in the meantime drops the result.
/// A landed listing replaces the chooser's explorer state; a failed one
/// keeps the dialog open with the detail inline (navigation stays free,
/// so the user can head elsewhere or Esc).
pub(crate) fn attachment_listing_ready(
    state: &mut AppState,
    result: &OperationResult,
) -> Vec<Effect> {
    if !matches!(state.session.overlay, Some(Overlay::AttachmentExplorer(_))) {
        tracing::debug!(id = %result.id, "dropping directory listing for a closed dialog");
        return Vec::new();
    }
    match &result.outcome {
        Ok(OperationOutcome::Explorer(explorer)) => {
            let mut explorer = explorer.as_ref().clone();
            // The chooser renders with the active palette; theme changes
            // cannot happen while a modal is up, so this sticks.
            let theme = state.active_theme();
            explorer.set_theme(theme.explorer_theme());
            if let Some(Overlay::AttachmentExplorer(dialog)) = state.session.overlay.as_mut() {
                dialog.explorer = Some(explorer);
                dialog.listing = false;
                dialog.error = None;
            }
            Vec::new()
        }
        Err(failure) => {
            let detail = failure.detail.clone();
            if let Some(Overlay::AttachmentExplorer(dialog)) = state.session.overlay.as_mut() {
                dialog.listing = false;
                dialog.error = Some(detail);
            }
            Vec::new()
        }
        Ok(_) => unexpected_payload(result.id, "directory listing"),
    }
}
