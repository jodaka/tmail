//! Send (plan §14, Phase 7.6/7.7): confirming the composer, the
//! in-flight state, and the classified outcome handling.
use std::sync::Arc;

use super::composer_flow::close_composer_route;
use super::modals::open_error_modal;
use crate::app::effect::Effect;
use crate::app::operation::{DraftOperation, DraftRemovalReason, OperationFailure, OperationKind};
use crate::app::route::Route;
use crate::app::state::AppState;
use crate::domain::sanitize::sanitize;

// ── Send (plan §14, Phase 7.6/7.7) ───────────────────────────────────────

/// Ctrl+Enter / Send button (plan §10). Sends only from the composer: the
/// route gate makes the action inert anywhere else. Refusals (invalid or
/// missing recipients, a send already in flight) surface on the status
/// line — the problem is visible input, not an operational failure — and
/// start nothing, so the draft is untouched.
pub(crate) fn send_from_composer(state: &mut AppState) -> Vec<Effect> {
    if !matches!(state.active_route(), Some(Route::Composer)) {
        // Ctrl+Enter sends only from the composer (plan §19 Phase 7).
        tracing::debug!("send ignored outside the composer");
        return Vec::new();
    }
    if state.session.composer.as_ref().is_none() {
        return Vec::new();
    }
    if state
        .session
        .composer
        .as_ref()
        .is_some_and(|composer| composer.sending)
    {
        tracing::debug!("send ignored: one is already in flight");
        return Vec::new();
    }
    if state.session.operations.is_sending() {
        state.set_status("A send is already in progress");
        return Vec::new();
    }
    // A send mints the draft's stable identity when it has none yet
    // (ADR 0002 §D.6): the sent bytes then carry the draft's Message-ID,
    // so the confirmed-send cleanup can sweep every copy by envelope —
    // including one a leave-time forced save pushes after the send
    // started, under the same identity.
    if let (Some(composer), Some(now)) = (state.session.composer.as_mut(), state.session.clock) {
        composer.draft.mint_identities(now);
    }
    let Some(composer) = state.session.composer.as_ref() else {
        return Vec::new();
    };
    let draft = &composer.draft;
    // Attached files ride through by path (plan §15, Phase 8.2): the
    // backend reads the bytes when it serializes the MIME.
    let attachments = draft
        .attachments
        .iter()
        .map(|att| crate::domain::OutboundAttachment {
            name: att.name.clone(),
            path: att.path.clone(),
        })
        .collect();
    let message = crate::domain::OutboundMessage::with_attachments(
        &draft.to,
        &draft.cc,
        &draft.bcc,
        crate::domain::OutgoingContent {
            subject: draft.subject.clone(),
            body: draft.body.clone(),
            in_reply_to: draft.in_reply_to.clone(),
            references: draft.references.clone(),
        },
        draft.message_id.clone(),
        attachments,
    );
    let message = match message {
        Ok(message) => message,
        Err(blocker) => {
            state.set_status(blocker.to_string());
            return Vec::new();
        }
    };
    // The send's cleanup resolves the draft that was sent, by the
    // identity frozen here — never whatever the composer slot holds when
    // the outcome lands (the user may have left and opened a different
    // draft in between).
    let sent_draft = Arc::new(draft.snapshot());
    if let Some(composer) = state.session.composer.as_mut() {
        composer.sending = true;
    }
    state.set_status("Sending…");
    vec![
        state
            .session
            .operations
            .start(OperationKind::Draft(DraftOperation::Send {
                message: Arc::new(message),
                draft: sent_draft,
            })),
    ]
}

/// Apply a classified send outcome (plan §12). Only [`SendOutcome::Sent`]
/// is definitive; every other outcome opens the modal and keeps the draft
/// (plan §19 Phase 7: failed send keeps the draft intact). `message` is
/// the frozen payload, replayed verbatim by retries; `sent_draft` is the
/// sent draft's frozen identity, what the confirmation resolves.
pub(crate) fn send_completed(
    state: &mut AppState,
    outcome: &crate::domain::SendOutcome,
    message: Arc<crate::domain::OutboundMessage>,
    sent_draft: Arc<crate::domain::DraftSnapshot>,
) -> Vec<Effect> {
    use crate::domain::SendOutcome;
    match outcome {
        SendOutcome::Sent => confirm_send(state, &message, sent_draft),
        other => {
            // The draft stays exactly as it was, editable again; the modal
            // carries the typed retry intent.
            if let Some(composer) = state.session.composer.as_mut() {
                composer.sending = false;
            }
            let failure = OperationFailure {
                code: other.code(),
                detail: {
                    let detail = sanitize(other.detail());
                    if detail.is_empty() {
                        String::from("the send outcome could not be determined")
                    } else {
                        detail
                    }
                },
                retry: Some(
                    OperationKind::Draft(DraftOperation::Send {
                        message,
                        draft: sent_draft,
                    })
                    .retry_spec(),
                ),
                ambiguous: other.is_ambiguous(),
            };
            // The specific send status must survive the modal opening
            // (the modal sets the generic "Operation failed" first).
            let effects = open_error_modal(state, failure);
            state.set_status(if other.is_ambiguous() {
                "Send outcome unclear"
            } else {
                "Send failed"
            });
            effects
        }
    }
}

/// A confirmed send (plan §19 Phase 7, Phase 7.6): leave the composer,
/// return to the prior route, and resolve the draft — journal entry and
/// remote copies removed through the same backend sweep a discard uses
/// (ADR 0002 §D.4). That cleanup is best-effort (`DraftRemovalReason::Sent`):
/// delivery is already confirmed, so a leftover copy must never claim a
/// failure afterwards.
///
/// The composer resolves only when it still holds the sent draft itself:
/// the user may have left mid-send and opened a different draft, and that
/// draft must never be deleted by another send's completion. Either way
/// the cleanup runs on the sent draft's own frozen identity.
pub(crate) fn confirm_send(
    state: &mut AppState,
    message: &crate::domain::OutboundMessage,
    sent_draft: Arc<crate::domain::DraftSnapshot>,
) -> Vec<Effect> {
    use crate::domain::sent_draft_is;
    state.set_status("Message sent");
    if state
        .session
        .composer
        .as_ref()
        .is_some_and(|composer| sent_draft_is(&composer.draft, message))
    {
        state.session.composer = None;
        close_composer_route(state);
    } else {
        tracing::debug!(
            message_id = ?message.message_id,
            "send confirmed; the composer holds a different draft and stays"
        );
    }
    vec![
        state
            .session
            .operations
            .start(OperationKind::Draft(DraftOperation::DeleteDraft {
                draft: sent_draft,
                reason: DraftRemovalReason::Sent,
            })),
    ]
}
