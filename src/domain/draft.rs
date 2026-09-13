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

use std::path::PathBuf;

use chrono::{DateTime, Duration, FixedOffset};
use serde::{Deserialize, Serialize};

use crate::domain::MessageId;
use crate::domain::address::to_field_list;
use crate::domain::message::{Message, bracketed_message_id};

/// Default autosave debounce (plan §14: "debounce 2 seconds"). The
/// `[tmail.composer].autosave_delay_ms` setting overrides it within the
/// bounds the config layer validates (Phase 10.4).
pub const DEFAULT_AUTOSAVE_DELAY_MS: u64 = 2_000;

/// Locally unique draft identity; doubles as the journal file stem
/// (ADR 0002 §D.1). Minted once, before the first save, from
/// reducer-supplied wall-clock time plus a random suffix (ticket tfc3)
/// so ids are unique across sessions without the reducer touching a
/// clock.
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

/// One file attached to an outgoing draft (plan §15). Metadata only: the
/// bytes are read from `path` when the message is serialized for sending,
/// so neither app state nor the journal ever hold attachment payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftAttachment {
    /// Absolute, `~`-expanded source path; validated when attached
    /// (exists, regular, readable, within the size limit).
    pub path: PathBuf,
    /// File name shown on the chip and used as the MIME filename.
    pub name: String,
    /// Size in bytes at attach time (chip display only).
    pub size: u64,
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
    /// Files attached for the outgoing message (plan §15). Paths and names
    /// only, never bytes; included in the journal so a crash cannot lose
    /// the attachment list.
    pub attachments: Vec<DraftAttachment>,
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
    /// Bare `In-Reply-To` id when this draft replies to a message (plan
    /// §14, Phase 7.4); serialized into the outgoing message.
    pub in_reply_to: Option<String>,
    /// Bare, whitespace-separated `References` chain for replies.
    pub references: Option<String>,
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
    /// Reply threading headers (Phase 7.4); `default` keeps journals from
    /// earlier revisions loadable.
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub references: Option<String>,
    /// Backend id of the previous confirmed remote copy, if any.
    pub remote_id: Option<MessageId>,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body: String,
    /// Attached files (plan §15): paths and names only. `default` keeps
    /// journals from earlier revisions loadable.
    #[serde(default)]
    pub attachments: Vec<DraftAttachment>,
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

    /// Whether the draft has nothing in it: no recipients, subject, body,
    /// attachments, reply threading, identity, or unsaved edits. A blank
    /// draft is the `c` artifact (ticket v5x8) — it is not real work, so
    /// it never counts as "a draft is open" (ticket pmbz): the next draft
    /// replaces it without ceremony.
    pub fn is_blank(&self) -> bool {
        !self.is_dirty()
            && self.to.is_empty()
            && self.cc.is_empty()
            && self.bcc.is_empty()
            && self.subject.is_empty()
            && self.body.is_empty()
            && self.attachments.is_empty()
            && self.in_reply_to.is_none()
            && self.references.is_none()
            && self.local_id.is_none()
            && self.message_id.is_none()
            && self.remote_id.is_none()
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
    /// `delay_ms` is the configured `[tmail.composer].autosave_delay_ms`.
    pub fn autosave_due(&self, now: DateTime<FixedOffset>, delay_ms: u64) -> bool {
        let delay = Duration::milliseconds(delay_ms.max(1) as i64);
        self.save == DraftSaveState::Debouncing
            && self.last_edit_at.is_some_and(|at| now - at >= delay)
    }

    /// Begin a save of the current revision: mint the stable identities on
    /// first use and freeze a snapshot for the backend. A draft reopened
    /// from the Drafts mailbox already carries the copy's `Message-ID` —
    /// it is kept, so the save replaces that copy instead of adding a
    /// second one (ADR 0002 §D.3).
    pub fn start_save(&mut self, now: DateTime<FixedOffset>) -> DraftSnapshot {
        self.save = DraftSaveState::Saving;
        if self.local_id.is_none() {
            // Ticket tfc3: a wall-clock stamp alone can collide — two
            // drafts started in the same nanosecond, or a clock rewind
            // across sessions landing on a recorded stamp (silently
            // overwriting the other draft's journal file). Mix in a
            // random 32-bit suffix; the stamp keeps ids sortable by
            // creation time.
            let stamp = now
                .timestamp_nanos_opt()
                .unwrap_or(now.timestamp_millis() * 1_000_000);
            let salt = fastrand::u32(..);
            self.local_id = Some(DraftId(format!("local-{stamp}-{salt:08x}")));
            if self.message_id.is_none() {
                self.message_id = Some(format!("<{stamp}.{salt:08x}.draft@tmail.local>"));
            }
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
            in_reply_to: self.in_reply_to.clone(),
            references: self.references.clone(),
            remote_id: self.remote_id.clone(),
            to: self.to.clone(),
            cc: self.cc.clone(),
            bcc: self.bcc.clone(),
            subject: self.subject.clone(),
            body: self.body.clone(),
            attachments: self.attachments.clone(),
            revision: self.revision,
        }
    }
}

/// Rebuild a composer draft from a fetched Drafts-mailbox message (the
/// Drafts list's Enter action): headers become the composer fields, the
/// decoded `text/plain` body becomes the body, and the copy's stable
/// identities carry over — the `Message-ID` (bracketed snapshot form) and
/// the backend id as `remote_id` — so the next save *replaces* this copy
/// instead of adding a second one (ADR 0002 §D.3 add-then-delete matches
/// on both). The revision counter restarts at the fetched (last-pushed)
/// state; attachment chips are path references recorded only in the
/// journal, so a remote copy carries none.
pub fn draft_from_message(message: &Message) -> Draft {
    Draft {
        to: to_field_list(&message.headers.to),
        cc: to_field_list(&message.headers.cc),
        bcc: to_field_list(&message.headers.bcc),
        subject: message.headers.subject.clone(),
        body: message.plain_body.clone().unwrap_or_default(),
        in_reply_to: message.headers.in_reply_to.clone(),
        references: message.headers.references.clone(),
        message_id: message
            .headers
            .message_id
            .as_deref()
            .map(bracketed_message_id),
        remote_id: Some(message.id.clone()),
        ..Draft::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Address, MailboxId, MessageHeaders};

    fn at(secs: i64) -> DateTime<FixedOffset> {
        DateTime::from_timestamp(1_788_335_220 + secs, 0)
            .expect("valid epoch")
            .with_timezone(&FixedOffset::east_opt(0).expect("utc"))
    }

    /// A draft copy as `message read` maps it: tmail-addressed fields,
    /// bare `Message-ID` (the parser strips brackets), hidden recipients.
    fn draft_copy() -> Message {
        Message {
            id: MessageId(String::from("copy-7")),
            mailbox_id: MailboxId(String::from("drafts")),
            headers: MessageHeaders {
                subject: String::from("Quarterly notes"),
                from: vec![Address {
                    name: None,
                    email: String::from("me@tmail.local"),
                }],
                to: vec![Address {
                    name: Some(String::from("Dest")),
                    email: String::from("dest@example.com"),
                }],
                cc: vec![Address {
                    name: None,
                    email: String::from("cc@example.com"),
                }],
                bcc: vec![Address {
                    name: None,
                    email: String::from("bcc@example.com"),
                }],
                date: None,
                message_id: Some(String::from("1778.draft@tmail.local")),
                in_reply_to: Some(String::from("orig@tmail.local")),
                references: Some(String::from("root@tmail.local orig@tmail.local")),
            },
            plain_body: Some(String::from("draft body")),
            html_body: None,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn draft_from_message_maps_fields_and_identities() {
        let draft = draft_from_message(&draft_copy());
        assert_eq!(draft.to, "Dest <dest@example.com>");
        assert_eq!(draft.cc, "cc@example.com");
        assert_eq!(draft.bcc, "bcc@example.com");
        assert_eq!(draft.subject, "Quarterly notes");
        assert_eq!(draft.body, "draft body");
        // The copy's identities carry over: the next save replaces it
        // (bracketed snapshot form, ADR 0002 §D.3).
        assert_eq!(
            draft.message_id.as_deref(),
            Some("<1778.draft@tmail.local>")
        );
        assert_eq!(draft.remote_id, Some(MessageId(String::from("copy-7"))));
        // Threading headers survive the round trip (reply drafts).
        assert_eq!(draft.in_reply_to.as_deref(), Some("orig@tmail.local"));
        assert_eq!(
            draft.references.as_deref(),
            Some("root@tmail.local orig@tmail.local")
        );
        // The fetched copy is the last pushed state: clean, ids minted on
        // the first edit-save.
        assert!(!draft.is_dirty());
        assert!(draft.local_id.is_none());
    }

    #[test]
    fn draft_from_message_tolerates_missing_data() {
        let mut copy = draft_copy();
        copy.headers.message_id = None;
        copy.plain_body = None;
        copy.headers.to.clear();
        let draft = draft_from_message(&copy);
        assert_eq!(draft.message_id, None, "minted at the first save instead");
        assert_eq!(draft.body, "");
        assert_eq!(draft.to, "");
        assert_eq!(draft.remote_id, Some(copy.id));
    }

    #[test]
    fn start_save_keeps_a_recovered_message_id() {
        let mut draft = draft_from_message(&draft_copy());
        let snapshot = draft.start_save(at(2));
        assert!(snapshot.local_id.0.starts_with("local-"), "minted fresh");
        assert_eq!(
            snapshot.message_id.as_deref(),
            Some("<1778.draft@tmail.local>"),
            "the copy's Message-ID is kept, not re-minted"
        );
    }

    #[test]
    fn minted_ids_carry_a_random_suffix_beyond_the_clock() {
        // Ticket tfc3: two drafts minted at the same instant (or across a
        // clock rewind) must not share a journal file. The random suffix
        // makes an exact collision overwhelmingly unlikely, and the
        // Message-ID keeps the same salt so add-then-delete matching
        // stays coherent.
        let mut first = Draft::default();
        let first = first.start_save(at(2));
        let mut second = Draft::default();
        let second = second.start_save(at(2));
        assert_ne!(
            first.local_id, second.local_id,
            "same-instant drafts must not collide"
        );
        assert_ne!(first.message_id, second.message_id);
        // Shape: local-<stamp>-<8 hex chars>.
        let id = first.local_id.0.clone();
        let salt = id.rsplit('-').next().expect("salt");
        assert_eq!(salt.len(), 8, "8 hex chars: {id}");
        assert!(salt.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
        assert!(
            first
                .message_id
                .as_deref()
                .is_some_and(|mid| mid.ends_with(".draft@tmail.local>"))
        );
    }

    #[test]
    fn fresh_draft_is_clean_and_never_due() {
        let d = Draft::default();
        assert!(!d.is_dirty());
        assert_eq!(d.save, DraftSaveState::Saved);
        assert!(!d.autosave_due(at(100), DEFAULT_AUTOSAVE_DELAY_MS));
    }

    #[test]
    fn edit_dirties_and_arms_the_debounce() {
        let mut d = Draft::default();
        d.note_edit(Some(at(10)));
        assert!(d.is_dirty());
        assert_eq!(d.save, DraftSaveState::Debouncing);
        assert_eq!(d.revision, 1);
        // Too early: the window has not elapsed.
        assert!(!d.autosave_due(at(11), DEFAULT_AUTOSAVE_DELAY_MS));
        // At exactly 2 s the save is due.
        assert!(d.autosave_due(at(12), DEFAULT_AUTOSAVE_DELAY_MS));
        assert!(d.autosave_due(at(100), DEFAULT_AUTOSAVE_DELAY_MS));
    }

    #[test]
    fn edits_during_the_window_rearm_the_debounce() {
        let mut d = Draft::default();
        d.note_edit(Some(at(0)));
        d.note_edit(Some(at(1)));
        d.note_edit(Some(at(2)));
        // 2 s after the *last* edit, not the first.
        assert!(!d.autosave_due(at(3), DEFAULT_AUTOSAVE_DELAY_MS));
        assert!(d.autosave_due(at(4), DEFAULT_AUTOSAVE_DELAY_MS));
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
                .is_some_and(|m| m.starts_with('<') && m.ends_with("@tmail.local>"))
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
        assert!(d.autosave_due(at(32), DEFAULT_AUTOSAVE_DELAY_MS));
    }

    #[test]
    fn snapshot_carries_fields_and_revision() {
        let mut d = Draft {
            to: String::from("a@b.c"),
            body: String::from("hello"),
            in_reply_to: Some(String::from("1@x")),
            references: Some(String::from("0@x 1@x")),
            ..Draft::default()
        };
        d.note_edit(Some(at(0)));
        let snap = d.start_save(at(2));
        assert_eq!(snap.to, "a@b.c");
        assert_eq!(snap.body, "hello");
        assert_eq!(snap.revision, 1);
        assert_eq!(snap.in_reply_to.as_deref(), Some("1@x"));
        assert_eq!(snap.references.as_deref(), Some("0@x 1@x"));
        // Serializable retry intents (plan §12).
        let json = serde_json::to_string(&snap).expect("snapshot json");
        let back: DraftSnapshot = serde_json::from_str(&json).expect("round trip");
        assert_eq!(back, snap);
    }

    #[test]
    fn snapshots_from_older_journals_load_without_reply_headers() {
        // Journal entries written before Phase 7.4 carry no threading
        // fields; serde defaults keep them loadable (ADR 0002 versioning).
        let legacy = r#"{
            "local_id": "local-1",
            "message_id": "<1@tmail.local>",
            "remote_id": "remote-1",
            "to": "a@b.c",
            "cc": "",
            "bcc": "",
            "subject": "s",
            "body": "b",
            "revision": 3
        }"#;
        let snap: DraftSnapshot = serde_json::from_str(legacy).expect("legacy journal entry");
        assert_eq!(snap.in_reply_to, None);
        assert_eq!(snap.references, None);
        assert!(
            snap.attachments.is_empty(),
            "pre-Phase-8 journals have none"
        );
    }

    #[test]
    fn snapshots_carry_attachments_without_bytes() {
        let mut d = Draft {
            body: String::from("see attached"),
            ..Draft::default()
        };
        d.attachments.push(DraftAttachment {
            path: PathBuf::from("/tmp/report final.pdf"),
            name: String::from("report final.pdf"),
            size: 1234,
        });
        d.note_edit(Some(at(0)));
        let snap = d.start_save(at(2));
        assert_eq!(snap.attachments.len(), 1);
        assert_eq!(snap.attachments[0].name, "report final.pdf");
        // Serializable for the crash-safe journal (ADR 0002 §D.1).
        let json = serde_json::to_string(&snap).expect("snapshot json");
        let back: DraftSnapshot = serde_json::from_str(&json).expect("round trip");
        assert_eq!(back.attachments, snap.attachments);
    }
}
