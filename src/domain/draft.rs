//! Draft model and autosave state machine (plan §14).
//!
//! One draft holds the composer fields plus the revision bookkeeping that
//! makes autosave safe:
//!
//! ```text
//! edit -> increment revision -> dirty -> debounce 2 seconds -> saving
//!    -> success for latest revision -> saved(timestamp)
//!    -> success for older revision -> remain dirty and save again
//!    -> failure -> error modal + unsaved state
//! ```
//!
//! "Dirty" is always derivable (`revision > saved_revision`), so a save of
//! revision N can never mark a later revision N+1 clean: [`Draft::confirm_saved`]
//! only advances `saved_revision` and the state machine re-saves while a
//! newer revision exists.
//!
//! Time enters only through [`Draft::note_edit`] / [`Draft::autosave_due`]
//! arguments supplied by the reducer from `Action::Tick { now }`, keeping
//! the model deterministic and unit-testable.

use chrono::{DateTime, Duration, FixedOffset};
use serde::{Deserialize, Serialize};

use crate::domain::MessageId;

/// Two-second autosave debounce (plan §14: "debounce 2 seconds").
pub const AUTOSAVE_DEBOUNCE_SECS: i64 = 2;

/// Locally unique draft identity; doubles as the journal file stem
/// (ADR 0002 §D.1). Minted once, before the first save, from reducer-supplied
/// wall-clock time so ids are unique across sessions without the reducer
/// touching a clock.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DraftId(pub String);

/// The autosave phase of one draft (plan §14 state machine).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DraftSaveState {
    /// `revision == saved_revision`: nothing outstanding.
    #[default]
    Saved,
    /// Edited; waiting out the debounce window.
    Debouncing,
    /// A remote save is in flight.
    Saving,
    /// The last save attempt failed; the content is retained.
    Failed,
}

/// One draft: fields, revision tracking, and save state. Plain data; the
/// reducer is its only writer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Draft {
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body: String,
    /// Bumped on every content edit; the identity of "what is on screen".
    pub revision: u64,
    /// The newest revision confirmed saved (journal + remote).
    pub saved_revision: u64,
    /// When the newest successful save completed (`Draft saved · HH:MM`).
    pub saved_at: Option<DateTime<FixedOffset>>,
    /// When the newest revision was edited (drives the debounce).
    pub last_edit_at: Option<DateTime<FixedOffset>>,
    pub save: DraftSaveState,
    /// Minted before the first save; `None` for a never-saved draft.
    pub local_id: Option<DraftId>,
    /// RFC `Message-ID` header, stable across revisions (ADR 0002 §D.6);
    /// what makes add-then-delete replacement and reconciliation possible.
    pub message_id: Option<String>,
    /// Backend id of the last confirmed remote copy; the next save replaces
    /// it (add-then-delete, ADR 0002 §D.3).
    pub remote_id: Option<MessageId>,
}

/// The serializable payload of one save operation (plan §12: retry intents
/// must be serializable). Everything the backend needs to journal and push
/// one revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftSnapshot {
    pub local_id: DraftId,
    pub message_id: Option<String>,
    /// Backend id of the previous confirmed remote copy, if any.
    pub remote_id: Option<MessageId>,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body: String,
    /// The revision this snapshot carries.
    pub revision: u64,
}

/// A draft restored from the journal at startup (ADR 0002 §D.5): the
/// newest recorded revision plus how far the remote had confirmed before
/// the interruption. A gap between them means the restored draft re-pushes
/// itself (self-healing autosave) once the composer reopens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredDraft {
    pub draft: DraftSnapshot,
    pub saved_revision: u64,
}

impl Draft {
    /// Whether unsaved edits exist (`revision > saved_revision`).
    pub fn is_dirty(&self) -> bool {
        self.revision > self.saved_revision
    }

    /// Record a content edit: bump the revision and (re)arm the debounce.
    /// An edit during an in-flight save re-dirties the draft, so the state
    /// machine always follows with another save (plan §14 acceptance).
    pub fn note_edit(&mut self, now: Option<DateTime<FixedOffset>>) {
        self.revision += 1;
        if now.is_some() {
            self.last_edit_at = now;
        }
        self.save = DraftSaveState::Debouncing;
    }

    /// Whether the debounce window has elapsed and an autosave is due.
    pub fn autosave_due(&self, now: DateTime<FixedOffset>) -> bool {
        self.save == DraftSaveState::Debouncing
            && self
                .last_edit_at
                .is_some_and(|at| now - at >= Duration::seconds(AUTOSAVE_DEBOUNCE_SECS))
    }

    /// Begin a save of the current revision: mint the stable identities on
    /// first use and freeze a snapshot for the backend.
    pub fn start_save(&mut self, now: DateTime<FixedOffset>) -> DraftSnapshot {
        self.save = DraftSaveState::Saving;
        if self.local_id.is_none() {
            let stamp = now
                .timestamp_nanos_opt()
                .unwrap_or(now.timestamp_millis() * 1_000_000);
            self.local_id = Some(DraftId(format!("local-{stamp}")));
            self.message_id = Some(format!("<{stamp}.draft@post.local>"));
        }
        self.snapshot()
    }

    /// Apply a confirmed save for `revision`. Returns `true` when a newer
    /// revision exists and another save must follow immediately (plan §14:
    /// "success for older revision -> remain dirty and save again"); the
    /// save state re-enters debouncing so exactly one follow-up save is
    /// issued. `at` stamps the saved time from the reducer's last tick.
    pub fn confirm_saved(
        &mut self,
        revision: u64,
        remote_id: MessageId,
        at: Option<DateTime<FixedOffset>>,
    ) -> bool {
        self.remote_id = Some(remote_id);
        self.saved_revision = self.saved_revision.max(revision);
        if at.is_some() {
            self.saved_at = at;
        }
        if self.saved_revision >= self.revision {
            self.save = DraftSaveState::Saved;
            false
        } else {
            self.save = DraftSaveState::Debouncing;
            true
        }
    }

    /// Record a failed save: content retained, unsaved state (plan §14:
    /// "failure -> error modal + unsaved state"). Only meaningful for the
    /// newest revision; a stale failure while newer edits exist leaves the
    /// debounce armed (the scheduled save supersedes the failure).
    pub fn mark_failed(&mut self) {
        self.save = DraftSaveState::Failed;
    }

    /// Freeze the current revision for the backend.
    pub fn snapshot(&self) -> DraftSnapshot {
        DraftSnapshot {
            local_id: self
                .local_id
                .clone()
                .unwrap_or_else(|| DraftId(String::from("local-unsaved"))),
            message_id: self.message_id.clone(),
            remote_id: self.remote_id.clone(),
            to: self.to.clone(),
            cc: self.cc.clone(),
            bcc: self.bcc.clone(),
            subject: self.subject.clone(),
            body: self.body.clone(),
            revision: self.revision,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> DateTime<FixedOffset> {
        DateTime::from_timestamp(1_788_335_220 + secs, 0)
            .expect("valid epoch")
            .with_timezone(&FixedOffset::east_opt(0).expect("utc"))
    }

    #[test]
    fn fresh_draft_is_clean_and_never_due() {
        let d = Draft::default();
        assert!(!d.is_dirty());
        assert_eq!(d.save, DraftSaveState::Saved);
        assert!(!d.autosave_due(at(100)));
    }

    #[test]
    fn edit_dirties_and_arms_the_debounce() {
        let mut d = Draft::default();
        d.note_edit(Some(at(10)));
        assert!(d.is_dirty());
        assert_eq!(d.save, DraftSaveState::Debouncing);
        assert_eq!(d.revision, 1);
        // Too early: the window has not elapsed.
        assert!(!d.autosave_due(at(11)));
        // At exactly 2 s the save is due.
        assert!(d.autosave_due(at(12)));
        assert!(d.autosave_due(at(100)));
    }

    #[test]
    fn edits_during_the_window_rearm_the_debounce() {
        let mut d = Draft::default();
        d.note_edit(Some(at(0)));
        d.note_edit(Some(at(1)));
        d.note_edit(Some(at(2)));
        // 2 s after the *last* edit, not the first.
        assert!(!d.autosave_due(at(3)));
        assert!(d.autosave_due(at(4)));
    }

    #[test]
    fn start_save_mints_stable_ids_once() {
        let mut d = Draft::default();
        d.note_edit(Some(at(0)));
        let snap = d.start_save(at(2));
        let local = snap.local_id.clone();
        let message_id = snap.message_id.clone();
        assert!(local.0.starts_with("local-"));
        assert!(
            message_id
                .as_deref()
                .is_some_and(|m| m.starts_with('<') && m.ends_with("@post.local>"))
        );
        // A later save keeps both ids stable (ADR 0002 §D.6).
        d.note_edit(Some(at(10)));
        let snap2 = d.start_save(at(12));
        assert_eq!(snap2.local_id, local);
        assert_eq!(snap2.message_id, message_id);
        assert_eq!(d.save, DraftSaveState::Saving);
    }

    #[test]
    fn confirm_latest_revision_is_saved_and_clean() {
        let mut d = Draft::default();
        d.note_edit(Some(at(0)));
        let snap = d.start_save(at(2));
        let chain = d.confirm_saved(
            snap.revision,
            MessageId(String::from("remote-1")),
            Some(at(3)),
        );
        assert!(!chain, "latest revision: no follow-up save");
        assert!(!d.is_dirty());
        assert_eq!(d.save, DraftSaveState::Saved);
        assert_eq!(d.saved_at, Some(at(3)));
        assert_eq!(d.remote_id, Some(MessageId(String::from("remote-1"))));
    }

    #[test]
    fn edits_during_a_save_force_another_save() {
        let mut d = Draft::default();
        d.note_edit(Some(at(0)));
        let snap1 = d.start_save(at(2));
        // The user keeps typing while revision 1 saves.
        d.note_edit(Some(at(2)));
        let chain = d.confirm_saved(
            snap1.revision,
            MessageId(String::from("remote-1")),
            Some(at(3)),
        );
        assert!(chain, "stale success must trigger another save");
        assert!(d.is_dirty(), "revision N+1 must stay dirty");
        // The chained save covers the newer revision and cleans up.
        let snap2 = d.start_save(at(3));
        assert_eq!(snap2.revision, 2);
        let chain = d.confirm_saved(
            snap2.revision,
            MessageId(String::from("remote-2")),
            Some(at(4)),
        );
        assert!(!chain);
        assert!(!d.is_dirty());
    }

    #[test]
    fn out_of_order_confirmations_never_lower_saved_revision() {
        let mut d = Draft::default();
        d.note_edit(Some(at(0)));
        d.note_edit(Some(at(1)));
        let snap2 = d.start_save(at(3));
        assert_eq!(snap2.revision, 2);
        // A confirmation for revision 1 arrives after 2 started.
        let chain = d.confirm_saved(1, MessageId(String::from("remote-1")), Some(at(3)));
        assert!(chain);
        assert_eq!(d.saved_revision, 1);
        let chain = d.confirm_saved(2, MessageId(String::from("remote-2")), Some(at(4)));
        assert!(!chain);
        assert_eq!(d.saved_revision, 2);
    }

    #[test]
    fn failure_keeps_content_and_unsaved_state() {
        let mut d = Draft {
            to: String::from("x@example.com"),
            ..Draft::default()
        };
        d.note_edit(Some(at(0)));
        let _ = d.start_save(at(2));
        d.mark_failed();
        assert_eq!(d.save, DraftSaveState::Failed);
        assert!(d.is_dirty());
        assert_eq!(d.to, "x@example.com", "content retained");
        // The next edit re-arms autosave.
        d.note_edit(Some(at(30)));
        assert_eq!(d.save, DraftSaveState::Debouncing);
        assert!(d.autosave_due(at(32)));
    }

    #[test]
    fn snapshot_carries_fields_and_revision() {
        let mut d = Draft {
            to: String::from("a@b.c"),
            body: String::from("hello"),
            ..Draft::default()
        };
        d.note_edit(Some(at(0)));
        let snap = d.start_save(at(2));
        assert_eq!(snap.to, "a@b.c");
        assert_eq!(snap.body, "hello");
        assert_eq!(snap.revision, 1);
        // Serializable retry intents (plan §12).
        let json = serde_json::to_string(&snap).expect("snapshot json");
        let back: DraftSnapshot = serde_json::from_str(&json).expect("round trip");
        assert_eq!(back, snap);
    }
}
