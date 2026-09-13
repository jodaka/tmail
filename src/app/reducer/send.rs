//! Send (plan §14, Phase 7.6/7.7): confirming the composer, the
//! in-flight state, and the classified outcome handling.
use super::modals::open_error_modal;
use crate::app::effect::Effect;
use crate::app::focus::Focus;
use crate::app::operation::{DraftRemovalReason, OperationFailure, OperationKind};
use crate::app::route::Route;
use crate::app::sanitize::sanitize;
use crate::app::state::AppState;

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
    let Some(composer) = state.session.composer.as_ref() else {
        return Vec::new();
    };
    if composer.sending {
        tracing::debug!("send ignored: one is already in flight");
        return Vec::new();
    }
    if state.session.operations.is_sending() {
        state.set_status("A send is already in progress");
        return Vec::new();
    }
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
    if let Some(composer) = state.session.composer.as_mut() {
        composer.sending = true;
    }
    state.set_status("Sending…");
    vec![state.session.operations.start(OperationKind::Send {
        message: Box::new(message),
    })]
}

/// Apply a classified send outcome (plan §12). Only [`SendOutcome::Sent`]
/// is definitive; every other outcome opens the modal and keeps the draft
/// (plan §19 Phase 7: failed send keeps the draft intact). `message` is
/// the frozen payload, replayed verbatim by retries.
pub(crate) fn send_completed(
    state: &mut AppState,
    outcome: &crate::domain::SendOutcome,
    message: &crate::domain::OutboundMessage,
) -> Vec<Effect> {
    use crate::domain::SendOutcome;
    match outcome {
        SendOutcome::Sent => confirm_send(state),
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
                    OperationKind::Send {
                        message: Box::new(message.clone()),
                    }
                    .retry_spec(),
                ),
                ambiguous: other.is_ambiguous(),
            };
            // The specific send status must survive the modal opening
            // (the modal sets the generic "Operation failed" first).
            let effects = open_error_modal(state, &failure);
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
pub(crate) fn confirm_send(state: &mut AppState) -> Vec<Effect> {
    state.set_status("Message sent");
    // The composer may have been left mid-send (Esc saves/leaves); the
    // draft data — with its stable ids — is what gets resolved here.
    let snapshot = state
        .session
        .composer
        .take()
        .map(|composer| composer.draft.snapshot());
    if matches!(state.active_route(), Some(Route::Composer)) {
        state.session.routes.pop();
        state.session.focus = Focus::MessageList;
    }
    match snapshot {
        Some(snapshot) => vec![state.session.operations.start(OperationKind::DeleteDraft {
            draft: Box::new(snapshot),
            reason: DraftRemovalReason::Sent,
        })],
        None => {
            tracing::debug!("send confirmed without a draft to resolve");
            Vec::new()
        }
    }
}
