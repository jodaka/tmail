//! Composer state: fields, focus cycling, and text editing (plan §10/§14).
//!
//! Plain data mutated only by the reducer. The body is a
//! `ratatui_textarea::TextArea` (plan §5: "ratatui-textarea for the
//! built-in body editor") so multi-line editing and wrapping come from the
//! library; single-line fields are plain strings with an explicit
//! char-index cursor so editing stays deterministic and unit-testable.
//!
//! Field navigation follows plan §10: Tab/Shift+Tab and Up/Down move
//! between To/Cc/Bcc/Subject/body and the action row; Left/Right and
//! Up/Down move the caret inside the focused field, crossing into the
//! neighbouring field at the body's top/bottom edge.

use ratatui_textarea::{CursorMove, TextArea};

use crate::app::action::ComposerEdit;
use crate::domain::Draft;

/// A focusable composer control (mockup `new-mail.html`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerField {
    To,
    /// The `Cc` button on the To row, present only while the Cc field is
    /// hidden. Enter reveals the field (mockup `.field-extra button`).
    CcToggle,
    Cc,
    /// The `Bcc` button on the To row, present only while the Bcc field is
    /// hidden.
    BccToggle,
    Bcc,
    Subject,
    Body,
    /// One attached-file chip, by position in the draft (mockup `.att`).
    /// Enter removes it (plan §15: "allow removal before send").
    Attachment(usize),
    /// The `+ attach` control (mockup `.att.add`); Enter opens the
    /// path-entry overlay (plan §15: no file browser in v1).
    Attach,
    /// The Send button (plan §10: `Ctrl+Enter` sends; Enter activates).
    Send,
    /// The explicit Discard action (plan §14; confirmation lands in 6.6).
    Discard,
}

impl ComposerField {
    /// Whether text edits land in this control; chips and buttons take
    /// none (plan §10: shortcut rows never receive composed text).
    pub fn accepts_text(self) -> bool {
        matches!(
            self,
            ComposerField::To
                | ComposerField::Cc
                | ComposerField::Bcc
                | ComposerField::Subject
                | ComposerField::Body
        )
    }
}

/// Everything visible in the composer. Lives in [`crate::app::state::AppState`]
/// for as long as a draft exists; leaving the composer keeps it so the
/// draft can be reopened (plan §14: "Leaving returns to the prior route
/// and preserves the draft").
#[derive(Debug, Clone)]
pub struct ComposerState {
    /// The focused control.
    pub field: ComposerField,
    /// Caret offset in chars inside the focused single-line field.
    pub cursor: usize,
    pub show_cc: bool,
    pub show_bcc: bool,
    /// The body editor. `TextArea<'static>`: it owns its styling, so no
    /// borrowed UI state outlives a frame. Its lines are synced into
    /// [`ComposerState::draft`] on every content edit.
    pub body: TextArea<'static>,
    /// The draft being composed: field text, revision tracking, and the
    /// autosave state machine (plan §14).
    pub draft: Draft,
    /// A send of this draft is in flight (Phase 7.6): content edits are
    /// frozen so the bytes on the wire stay exactly what the user saw,
    /// and a second send cannot start.
    pub sending: bool,
    /// The external editor owns the body file (plan §14 step 5, Phase
    /// 11.5): background autosave is suppressed until it exits, no matter
    /// how long the user writes.
    pub external_editing: bool,
}

impl Default for ComposerState {
    fn default() -> Self {
        Self::new()
    }
}

impl ComposerState {
    pub fn new() -> Self {
        Self {
            field: ComposerField::To,
            cursor: 0,
            show_cc: false,
            show_bcc: false,
            body: TextArea::from([""]),
            draft: Draft::default(),
            sending: false,
            external_editing: false,
        }
    }

    /// The focus cycle for the current toggle state, in visual order.
    /// Attachment chips sit between the body and the `+ attach` control
    /// (mockup `.attach-row`).
    fn cycle(&self) -> Vec<ComposerField> {
        let mut fields = vec![ComposerField::To];
        if self.show_cc {
            fields.push(ComposerField::Cc);
        } else {
            fields.push(ComposerField::CcToggle);
        }
        if self.show_bcc {
            fields.push(ComposerField::Bcc);
        } else {
            fields.push(ComposerField::BccToggle);
        }
        fields.push(ComposerField::Subject);
        fields.push(ComposerField::Body);
        let chip_count = self.draft.attachments.len();
        fields.extend((0..chip_count).map(ComposerField::Attachment));
        fields.extend([
            ComposerField::Attach,
            ComposerField::Send,
            ComposerField::Discard,
        ]);
        fields
    }

    /// Step to the next control (Tab, plan §10), placing the caret.
    pub fn focus_next(&mut self) {
        let cycle = self.cycle();
        let index = cycle.iter().position(|f| *f == self.field).unwrap_or(0);
        let next = cycle[(index + 1) % cycle.len()];
        self.enter_field(next);
    }

    /// Step to the previous control (Shift+Tab), placing the caret.
    pub fn focus_previous(&mut self) {
        let cycle = self.cycle();
        let index = cycle.iter().position(|f| *f == self.field).unwrap_or(0);
        let previous = cycle[(index + cycle.len() - 1) % cycle.len()];
        self.enter_field(previous);
    }

    /// Focus `field` and put the caret where editing would continue: at the
    /// end of single-line fields, wherever it was in the body. Returns
    /// `false` when the field is not part of the current cycle (e.g. a
    /// hidden Cc row), leaving the focus untouched — the guard the mouse
    /// click path relies on (Phase 10.2).
    pub fn focus_field(&mut self, field: ComposerField) -> bool {
        if !self.cycle().contains(&field) {
            return false;
        }
        self.enter_field(field);
        true
    }

    /// Focus `field` and put the caret where editing would continue: at the
    /// end of single-line fields, wherever it was in the body.
    fn enter_field(&mut self, field: ComposerField) {
        self.field = field;
        if field != ComposerField::Body {
            self.cursor = self.focused_text().chars().count();
        }
    }

    /// Rebuild composer UI state around a draft (startup restore from the
    /// journal, or reopening a preserved draft).
    pub fn from_draft(draft: Draft) -> Self {
        let body = TextArea::from(if draft.body.is_empty() {
            vec![String::new()]
        } else {
            draft
                .body
                .split('\n')
                .map(str::to_owned)
                .collect::<Vec<_>>()
        });
        Self {
            field: ComposerField::To,
            cursor: draft.to.chars().count(),
            show_cc: !draft.cc.is_empty(),
            show_bcc: !draft.bcc.is_empty(),
            body,
            draft,
            sending: false,
            external_editing: false,
        }
    }

    /// Reveal and focus the Cc field (Enter on the Cc toggle).
    pub fn enter_cc(&mut self) {
        self.show_cc = true;
        self.enter_field(ComposerField::Cc);
    }

    /// Reveal and focus the Bcc field (Enter on the Bcc toggle).
    pub fn enter_bcc(&mut self) {
        self.show_bcc = true;
        self.enter_field(ComposerField::Bcc);
    }

    /// Attach a validated file (plan §15). Same-path entries are ignored —
    /// the deterministic duplicate rule is one chip per file — while two
    /// files that merely share a basename both attach, in insertion order.
    /// Returns whether the file was added.
    pub fn add_attachment(&mut self, attachment: crate::domain::DraftAttachment) -> bool {
        if self
            .draft
            .attachments
            .iter()
            .any(|a| a.path == attachment.path)
        {
            return false;
        }
        self.draft.attachments.push(attachment);
        true
    }

    /// Remove the attachment chip at `index` (no-op when out of range) and
    /// refocus: the next chip at the same position, the last chip when the
    /// row shrank, or the `+ attach` control when none remain.
    pub fn remove_attachment(&mut self, index: usize) {
        if index >= self.draft.attachments.len() {
            return;
        }
        self.draft.attachments.remove(index);
        self.field = if self.draft.attachments.is_empty() {
            ComposerField::Attach
        } else {
            ComposerField::Attachment(index.min(self.draft.attachments.len() - 1))
        };
    }

    /// The text of the focused single-line field; the body is not a string.
    fn focused_text(&self) -> &str {
        match self.field {
            ComposerField::To => &self.draft.to,
            ComposerField::Cc => &self.draft.cc,
            ComposerField::Bcc => &self.draft.bcc,
            ComposerField::Subject => &self.draft.subject,
            _ => "",
        }
    }

    fn focused_text_mut(&mut self) -> Option<&mut String> {
        match self.field {
            ComposerField::To => Some(&mut self.draft.to),
            ComposerField::Cc => Some(&mut self.draft.cc),
            ComposerField::Bcc => Some(&mut self.draft.bcc),
            ComposerField::Subject => Some(&mut self.draft.subject),
            _ => None,
        }
    }

    /// Sync the editor surface into the draft and record a content edit:
    /// bumps the revision and re-arms the two-second autosave debounce
    /// (plan §14). Called by the reducer after every content edit, with
    /// the injected clock (caret moves skip this entirely).
    pub fn sync_draft(&mut self, now: Option<chrono::DateTime<chrono::FixedOffset>>) {
        self.draft.body = self.body.lines().join("\n");
        self.draft.note_edit(now);
    }

    /// Import the external editor's text (plan §14 step 6, Phase 11.6):
    /// replace the body wholesale. Content identical to what the editor
    /// was handed imports as a no-op — no revision bump, no save. The
    /// changed case records exactly one edit (step 8: mark dirty once);
    /// `now` is the injected clock for the debounce bookkeeping.
    pub fn import_body(
        &mut self,
        content: &str,
        now: Option<chrono::DateTime<chrono::FixedOffset>>,
    ) -> bool {
        if content == self.draft.body {
            return false;
        }
        let lines: Vec<String> = content.lines().map(str::to_owned).collect();
        self.body = if lines.is_empty() {
            TextArea::from([""])
        } else {
            TextArea::from(lines)
        };
        self.draft.body = String::from(content);
        self.draft.note_edit(now);
        true
    }

    /// Apply one character-level edit to the focused field (plan §10:
    /// composer keys). Arrow navigation crosses field boundaries at the
    /// body's top/bottom edge and between single-line fields. Content
    /// edits are inert on chips and buttons (plan §10).
    pub fn apply(&mut self, edit: &ComposerEdit) {
        if edit.is_content_edit() && !self.field.accepts_text() {
            return;
        }
        match edit {
            ComposerEdit::Char(c) => {
                let cursor = self.cursor;
                if let Some(text) = self.focused_text_mut() {
                    let offset = char_offset(text, cursor);
                    text.insert(offset, *c);
                    self.cursor = cursor + 1;
                } else {
                    self.body.insert_char(*c);
                }
            }
            ComposerEdit::Backspace => {
                let cursor = self.cursor;
                if let Some(text) = self.focused_text_mut() {
                    if cursor > 0 {
                        let offset = char_offset(text, cursor - 1);
                        text.remove(offset);
                        self.cursor = cursor - 1;
                    }
                } else {
                    self.body.delete_char();
                }
            }
            ComposerEdit::Delete => {
                let cursor = self.cursor;
                if let Some(text) = self.focused_text_mut() {
                    if cursor < text.chars().count() {
                        let offset = char_offset(text, cursor);
                        text.remove(offset);
                    }
                } else {
                    self.body.delete_next_char();
                }
            }
            ComposerEdit::Newline => {
                // Enter is a newline only in the body (plan §10); elsewhere
                // it activates, which the reducer routes separately.
                if self.field == ComposerField::Body {
                    self.body.insert_newline();
                }
            }
            ComposerEdit::CursorLeft => {
                if self.field == ComposerField::Body {
                    self.body.move_cursor(CursorMove::Back);
                } else {
                    self.cursor = self.cursor.saturating_sub(1);
                }
            }
            ComposerEdit::CursorRight => {
                if self.field == ComposerField::Body {
                    self.body.move_cursor(CursorMove::Forward);
                } else {
                    self.cursor = self
                        .cursor
                        .saturating_add(1)
                        .min(self.focused_text().chars().count());
                }
            }
            ComposerEdit::CursorUp => {
                if self.field == ComposerField::Body && self.body.cursor().0 > 0 {
                    self.body.move_cursor(CursorMove::Up);
                } else {
                    self.focus_previous();
                }
            }
            ComposerEdit::CursorDown => {
                if self.field == ComposerField::Body {
                    let last = self.body.lines().len().saturating_sub(1);
                    if self.body.cursor().0 < last {
                        self.body.move_cursor(CursorMove::Down);
                    } else {
                        self.focus_next();
                    }
                } else {
                    self.focus_next();
                }
            }
        }
    }
}

/// Byte offset of char index `index` (cursor offsets are char counts).
fn char_offset(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map(|(offset, _)| offset)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::DraftAttachment;
    use std::path::PathBuf;

    fn composer() -> ComposerState {
        ComposerState::new()
    }

    #[test]
    fn cycle_covers_toggles_and_actions() {
        let c = composer();
        assert_eq!(
            c.cycle(),
            vec![
                ComposerField::To,
                ComposerField::CcToggle,
                ComposerField::BccToggle,
                ComposerField::Subject,
                ComposerField::Body,
                ComposerField::Attach,
                ComposerField::Send,
                ComposerField::Discard,
            ]
        );
    }

    #[test]
    fn revealed_fields_replace_their_toggles() {
        let mut c = composer();
        c.show_cc = true;
        c.show_bcc = true;
        assert_eq!(
            c.cycle(),
            vec![
                ComposerField::To,
                ComposerField::Cc,
                ComposerField::Bcc,
                ComposerField::Subject,
                ComposerField::Body,
                ComposerField::Attach,
                ComposerField::Send,
                ComposerField::Discard,
            ]
        );
    }

    #[test]
    fn attachment_chips_sit_between_body_and_attach() {
        let mut c = composer();
        assert!(c.add_attachment(DraftAttachment {
            path: PathBuf::from("/tmp/a.pdf"),
            name: String::from("a.pdf"),
            size: 1,
        }));
        assert!(c.add_attachment(DraftAttachment {
            path: PathBuf::from("/tmp/b.png"),
            name: String::from("b.png"),
            size: 2,
        }));
        assert_eq!(
            c.cycle(),
            vec![
                ComposerField::To,
                ComposerField::CcToggle,
                ComposerField::BccToggle,
                ComposerField::Subject,
                ComposerField::Body,
                ComposerField::Attachment(0),
                ComposerField::Attachment(1),
                ComposerField::Attach,
                ComposerField::Send,
                ComposerField::Discard,
            ]
        );
    }

    #[test]
    fn tab_and_shift_tab_walk_the_whole_cycle() {
        let mut c = composer();
        assert_eq!(c.field, ComposerField::To);
        for expected in [
            ComposerField::CcToggle,
            ComposerField::BccToggle,
            ComposerField::Subject,
            ComposerField::Body,
            ComposerField::Attach,
            ComposerField::Send,
            ComposerField::Discard,
            ComposerField::To,
        ] {
            c.focus_next();
            assert_eq!(c.field, expected);
        }
        c.focus_previous();
        assert_eq!(c.field, ComposerField::Discard);
    }

    #[test]
    fn entering_a_field_puts_the_caret_at_the_end() {
        let mut c = composer();
        c.draft.to = String::from("ab");
        c.field = ComposerField::Subject;
        c.focus_previous(); // BccToggle
        c.focus_previous(); // CcToggle
        c.focus_previous(); // To — caret at the end of "ab"
        assert_eq!(c.field, ComposerField::To);
        assert_eq!(c.cursor, 2);
    }

    #[test]
    fn chars_land_in_the_focused_field() {
        let mut c = composer();
        for ch in "max@post".chars() {
            c.apply(&ComposerEdit::Char(ch));
        }
        assert_eq!(c.draft.to, "max@post");
        assert_eq!(c.cursor, 8);
        c.focus_next(); // CcToggle
        c.apply(&ComposerEdit::Char('x'));
        assert_eq!(c.draft.to, "max@post", "toggle rows take no text");
    }

    #[test]
    fn backspace_and_delete_edit_around_the_caret() {
        let mut c = composer();
        c.draft.to = String::from("abc");
        c.cursor = 2;
        c.apply(&ComposerEdit::Backspace);
        assert_eq!(c.draft.to, "ac");
        assert_eq!(c.cursor, 1);
        c.apply(&ComposerEdit::Delete);
        assert_eq!(c.draft.to, "a");
        assert_eq!(c.cursor, 1);
        // At the end of the text Delete is a no-op…
        c.apply(&ComposerEdit::Delete);
        assert_eq!(c.draft.to, "a");
        // …and Backspace still removes the character before the caret.
        c.apply(&ComposerEdit::Backspace);
        assert_eq!(c.draft.to, "");
        assert_eq!(c.cursor, 0);
        // Backspace at the very start is a no-op.
        c.apply(&ComposerEdit::Backspace);
        assert_eq!(c.draft.to, "");
    }

    #[test]
    fn caret_moves_within_single_line_bounds() {
        let mut c = composer();
        c.draft.to = String::from("ab");
        c.apply(&ComposerEdit::CursorLeft);
        c.apply(&ComposerEdit::CursorLeft);
        c.apply(&ComposerEdit::CursorLeft); // clamps at 0
        assert_eq!(c.cursor, 0);
        c.apply(&ComposerEdit::CursorRight);
        c.apply(&ComposerEdit::CursorRight);
        c.apply(&ComposerEdit::CursorRight); // clamps at len
        assert_eq!(c.cursor, 2);
    }

    #[test]
    fn enter_inserts_newline_only_in_body() {
        let mut c = composer();
        c.field = ComposerField::Subject;
        c.apply(&ComposerEdit::Newline);
        assert_eq!(c.draft.subject, "");
        c.focus_next(); // Body
        c.apply(&ComposerEdit::Char('a'));
        c.apply(&ComposerEdit::Newline);
        c.apply(&ComposerEdit::Char('b'));
        assert_eq!(c.body.lines(), ["a", "b"]);
    }

    #[test]
    fn body_caret_moves_and_stays_in_bounds() {
        let mut c = composer();
        // Walk to the body: CcToggle, BccToggle, Subject, then Body.
        for _ in 0..4 {
            c.focus_next();
        }
        assert_eq!(c.field, ComposerField::Body);
        c.apply(&ComposerEdit::Char('a'));
        c.apply(&ComposerEdit::Newline);
        c.apply(&ComposerEdit::Char('b'));
        c.apply(&ComposerEdit::CursorUp);
        assert_eq!(c.body.cursor().0, 0, "first row");
        c.apply(&ComposerEdit::CursorDown);
        assert_eq!(c.body.cursor().0, 1);
        // Down on the last row moves to the next field instead.
        c.apply(&ComposerEdit::CursorDown);
        assert_eq!(c.field, ComposerField::Attach);
        // Returning to the body keeps the body caret where it was (row 1),
        // so Up first moves within the body, then crosses into Subject.
        c.apply(&ComposerEdit::CursorUp); // Attach -> Body
        assert_eq!(c.field, ComposerField::Body);
        c.apply(&ComposerEdit::CursorUp); // row 1 -> row 0, still in the body
        assert_eq!(c.body.cursor().0, 0);
        c.apply(&ComposerEdit::CursorUp); // top edge -> previous field
        assert_eq!(c.field, ComposerField::Subject);
    }

    #[test]
    fn add_attachment_dedupes_by_path_and_keeps_order() {
        let mut c = composer();
        let pdf = DraftAttachment {
            path: PathBuf::from("/tmp/report.pdf"),
            name: String::from("report.pdf"),
            size: 10,
        };
        let other = DraftAttachment {
            path: PathBuf::from("/other/report.pdf"),
            name: String::from("report.pdf"),
            size: 20,
        };
        assert!(c.add_attachment(pdf.clone()));
        assert!(!c.add_attachment(pdf), "same path attaches once");
        assert!(c.add_attachment(other), "same basename, other path: kept");
        assert_eq!(c.draft.attachments.len(), 2);
        assert_eq!(
            c.draft.attachments[0].path,
            PathBuf::from("/tmp/report.pdf")
        );
        assert_eq!(c.draft.attachments[1].size, 20, "insertion order");
    }

    #[test]
    fn remove_attachment_refocuses_sensibly() {
        let mut c = composer();
        for name in ["a.pdf", "b.pdf", "c.pdf"] {
            assert!(c.add_attachment(DraftAttachment {
                path: PathBuf::from(format!("/tmp/{name}")),
                name: String::from(name),
                size: 1,
            }));
        }
        c.field = ComposerField::Attachment(1);
        c.remove_attachment(1);
        assert_eq!(c.draft.attachments.len(), 2);
        assert_eq!(c.field, ComposerField::Attachment(1), "next chip");
        c.remove_attachment(1);
        assert_eq!(c.field, ComposerField::Attachment(0), "clamped to last");
        c.remove_attachment(0);
        assert_eq!(c.field, ComposerField::Attach, "row empty again");
        // Out-of-range removals are inert.
        c.remove_attachment(5);
        assert_eq!(c.field, ComposerField::Attach);
    }

    #[test]
    fn chips_and_buttons_take_no_text_edits() {
        let mut c = composer();
        c.field = ComposerField::Body;
        c.apply(&ComposerEdit::Char('b'));
        c.apply(&ComposerEdit::Newline);
        c.apply(&ComposerEdit::Char('x'));
        c.field = ComposerField::Attach;
        c.apply(&ComposerEdit::Char('a'));
        c.apply(&ComposerEdit::Backspace);
        c.apply(&ComposerEdit::Delete);
        c.apply(&ComposerEdit::Newline);
        assert_eq!(c.body.lines(), ["b", "x"], "body untouched");
        assert_eq!(c.draft.attachments.len(), 0);
        c.field = ComposerField::Attachment(0);
        c.apply(&ComposerEdit::Char('a'));
        c.apply(&ComposerEdit::Backspace);
        assert_eq!(c.body.lines(), ["b", "x"], "chips take no text either");
    }

    #[test]
    fn toggle_rows_take_no_text_edits() {
        let mut c = composer();
        c.field = ComposerField::CcToggle;
        c.apply(&ComposerEdit::Char('c'));
        c.apply(&ComposerEdit::Backspace);
        c.apply(&ComposerEdit::Newline);
        assert_eq!(c.draft.to, "");
        assert_eq!(c.body.lines(), [String::new()]);
    }

    #[test]
    fn char_offset_handles_unicode() {
        assert_eq!(char_offset("héllo", 0), 0);
        assert_eq!(char_offset("héllo", 2), 3, "é is one char, two bytes");
        assert_eq!(char_offset("héllo", 5), 6);
        assert_eq!(char_offset("héllo", 99), 6, "clamps to the end");
    }
}
