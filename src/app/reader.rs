//! Reader document model and the reducer-shared scroll bookkeeping.
//!
//! `AppState` caches the scrollable document ([`CachedReaderDoc`]) and the
//! reducer clamps scrolling against the same line counts the renderer
//! draws, so the two can never disagree. The renderer consumes these
//! builders from `ui::screens::reader`.

use std::rc::Rc;

use crate::app::state::{AppState, Loadable};
use crate::view::dates;
use crate::view::rich::{RichLine, RichSpan, RichStyle};
use crate::view::text;

/// Visual tone of one chrome reader line; the renderer maps tones to theme
/// styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    /// Subject line (mockup `.thread-subject`).
    Strong,
    /// Meta labels, hairlines, placeholders, attachment chips.
    Dim,
}

/// One line of the reader document: either tone-styled chrome (subject,
/// meta, attachments) or a rich body line whose spans carry the
/// HTML semantics mapped by `ui::rich` (plan §13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReaderLine {
    Chrome {
        tone: Tone,
        text: String,
    },
    /// One attachment chip (mockup `.att`). `selected` marks the chip the
    /// save/open keys act on; it renders with the cursor marker and the
    /// accent style.
    Chip {
        text: String,
        selected: bool,
    },
    Rich(RichLine),
}

impl ReaderLine {
    fn chrome(tone: Tone, text: impl Into<String>) -> Self {
        ReaderLine::Chrome {
            tone,
            text: text.into(),
        }
    }

    /// Full display text (tests).
    #[cfg(test)]
    fn text(&self) -> String {
        match self {
            ReaderLine::Chrome { text, .. } => text.clone(),
            ReaderLine::Chip { text, .. } => text.clone(),
            ReaderLine::Rich(line) => line.text(),
        }
    }
}

/// Fixed-header side padding (ticket 3rt5): every fixed field sits two
/// symbols in from the left and right edges of the panel.
const HEADER_PAD: &str = "  ";

/// Body indent (mockup `.m-text` `padding-left: 2ch`).
const INDENT: &str = "  ";

/// Wrap a fixed-header field in the panel's side padding, truncating the
/// content to keep the whole line inside the panel width.
fn padded_field(content: &str, width: usize) -> String {
    let inner = text::truncate(content, width.saturating_sub(2 * HEADER_PAD.len()));
    format!("{HEADER_PAD}{inner}{HEADER_PAD}")
}

/// Prepend the body indent to a rich line as a plain leading span.
fn indented(line: RichLine) -> ReaderLine {
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(RichSpan {
        text: String::from(INDENT),
        style: RichStyle::default(),
        link_target: None,
    });
    spans.extend(line.spans);
    ReaderLine::Rich(RichLine { spans })
}

/// The fixed header of the reader document (ticket 6864): subject, meta
/// block, and the hairline that separates them from the body. Every field
/// is padded two symbols in from the panel edges (ticket 3rt5).
/// Deterministic and I/O-free; never scrolls.
pub(crate) fn header_lines(state: &AppState, width: usize) -> Vec<ReaderLine> {
    let Some(summary) = state.open_summary() else {
        return Vec::new();
    };
    let w = width.max(10);
    let loaded = state.open_message.as_loaded();
    let mut lines = Vec::new();

    // Subject; a missing header renders as an explicit placeholder (plan
    // §19 Phase 4 acceptance: reader handles missing subject). Once the
    // full message is loaded, its headers are authoritative; the summary
    // snapshot fills anything the fetch does not provide.
    let subject = loaded
        .and_then(|m| (!m.headers.subject.is_empty()).then_some(m.headers.subject.clone()))
        .unwrap_or_else(|| summary.subject.clone());
    let subject = if subject.is_empty() {
        "(no subject)"
    } else {
        subject.as_str()
    };
    lines.push(ReaderLine::chrome(Tone::Strong, padded_field(subject, w)));

    // Meta block (mockup `.msg-meta`): From / To / Cc / Date. From and To
    // come from the summary snapshot (available before the fetch lands);
    // Cc only once the full message is here. Missing senders and dates
    // degrade to dim placeholders, never empty rows.
    let from = loaded
        .and_then(|m| (!m.headers.from.is_empty()).then_some(m.headers.from.clone()))
        .unwrap_or_else(|| summary.from.clone());
    push_meta(
        &mut lines,
        "From",
        &address_list_or(&from, "(unknown sender)"),
        w,
    );
    let to = loaded
        .and_then(|m| (!m.headers.to.is_empty()).then_some(m.headers.to.clone()))
        .unwrap_or_else(|| summary.to.clone());
    push_meta(
        &mut lines,
        "To",
        &address_list_or(&to, "(no recipients)"),
        w,
    );
    if let Some(message) = loaded
        && !message.headers.cc.is_empty()
    {
        push_meta(
            &mut lines,
            "Cc",
            &address_list_label(&message.headers.cc),
            w,
        );
    }
    // The loaded message's Date header wins; the envelope timestamp is the
    // fallback (it is the epoch sentinel when the header was missing).
    let date = loaded
        .and_then(|m| m.headers.date)
        .unwrap_or(summary.timestamp);
    push_meta(&mut lines, "Date", &date_label(date), w);

    lines.push(hairline(w));
    lines
}

/// The scrollable part of the reader document (ticket 6864): the message
/// body and the attachment chips. Rendered from the reducer-maintained
/// `reader_scroll` anchor in the viewport under the fixed header.
pub(crate) fn scroll_lines(state: &AppState, width: usize) -> Vec<ReaderLine> {
    if state.open_summary().is_none() {
        return Vec::new();
    }
    let w = width.max(10);
    let mut lines = Vec::new();

    // Body (plan §13: HTML-preferred selection, rich semantic rendering,
    // reflow at the current width; Unicode-safe via ui::text and html2text).
    // Missing bodies render explicit placeholders (plan §19 Phase 4).
    // While the message loads there is nothing to lay out: the pane
    // spinner takes over (ticket m3by), so the document length stays 0.
    let body: Vec<RichLine> = match &state.open_message {
        Loadable::Loaded(message) => {
            crate::view::rich::body_lines(message, w.saturating_sub(INDENT.len()))
        }
        Loadable::Failed(detail) => vec![RichLine::from_plain(format!(
            "(message could not be loaded — {})",
            first_line(detail)
        ))],
        // `Idle` is defensive: a reader route without its load lifecycle.
        Loadable::Idle => vec![RichLine::from_plain("(no message loaded)")],
        Loadable::Loading => Vec::new(),
    };
    for line in body {
        lines.push(indented(line));
    }

    // Attachments (mockup `.attachments`): metadata chips — filename, MIME
    // type when known, and size. Tab cycles the cursor; `d`/`o` save/open
    // the selected chip (plan §15).
    if let Some(message) = state.open_message.as_loaded()
        && !message.attachments.is_empty()
    {
        lines.push(ReaderLine::chrome(Tone::Dim, ""));
        let count = message.attachments.len();
        lines.push(ReaderLine::chrome(
            Tone::Dim,
            format!(
                "{INDENT}{} attachment{}",
                count,
                if count == 1 { "" } else { "s" }
            ),
        ));
        let selected = state
            .reader_attachment
            .unwrap_or(0)
            .min(message.attachments.len() - 1);
        for (index, attachment) in message.attachments.iter().enumerate() {
            let name = attachment.name.as_deref().unwrap_or("(unnamed attachment)");
            let size = attachment
                .size
                .map(crate::view::text::human_size)
                .unwrap_or_else(|| String::from("unknown size"));
            let mime = attachment.mime_type.as_deref().unwrap_or("unknown type");
            let marker = if index == selected { "▸ " } else { "  " };
            lines.push(ReaderLine::Chip {
                text: text::truncate(&format!("{INDENT}{marker}[ {name} · {mime} · {size} ]"), w),
                selected: index == selected,
            });
        }
    }
    lines
}

/// Whole document: fixed header followed by the scrollable body (tests and
/// length bookkeeping compose the two pure halves).
#[cfg(test)]
pub(crate) fn content(state: &AppState, width: usize) -> Vec<ReaderLine> {
    let mut lines = header_lines(state, width);
    lines.extend(scroll_document(state, width).iter().cloned());
    lines
}

/// A cached scrollable reader document (perf): building it re-parses the
/// whole message HTML (html2text) and re-wraps every line, and until the
/// cache existed both the reducer's scroll clamp and every frame draw did
/// exactly that — a touchpad momentum burst re-parsed the body hundreds
/// of times, each parse a frame of lag, and key presses queued behind the
/// burst went unanswered for seconds. Keyed by everything the document
/// depends on; a hit serves the shared `Rc` (pointer clone) to both the
/// clamp and the renderer, so what is drawn and what the clamp allows can
/// never disagree (both go through [`scroll_document`]).
#[derive(Debug, Clone)]
pub(crate) struct CachedReaderDoc {
    /// The open message the lines were built from.
    pub message_id: crate::domain::MessageId,
    /// Wrap width the lines were reflowed at (a resize rebuilds).
    pub width: usize,
    /// Attachment-chip cursor baked into the chip markers (tab rebuilds).
    pub selected_chip: Option<usize>,
    /// Cheap content fingerprint — html/plain body lengths and attachment
    /// count — so a same-id body swap cannot serve stale lines.
    pub fingerprint: (usize, usize, usize),
    /// The shared lines; a cache hit is one `Rc` clone.
    pub lines: Rc<Vec<ReaderLine>>,
}

/// The scrollable reader document for `state`, served from the cache on
/// [`AppState`] when the open message, wrap width, chip cursor, and body
/// fingerprint are unchanged (see [`CachedReaderDoc`]). Misses rebuild
/// through [`scroll_lines`]. Not cached while the message loads or fails:
/// those documents are at most a placeholder line.
pub(crate) fn scroll_document(state: &AppState, width: usize) -> Rc<Vec<ReaderLine>> {
    let width = width.max(10);
    let Loadable::Loaded(message) = &state.open_message else {
        return Rc::new(scroll_lines(state, width));
    };
    let fingerprint = (
        message.html_body.as_ref().map_or(0, String::len),
        message.plain_body.as_ref().map_or(0, String::len),
        message.attachments.len(),
    );
    let selected_chip = state.reader_attachment;
    let cached_hit = {
        let cache = state.caches.reader_doc.borrow();
        cache.as_ref().is_some_and(|cached| {
            cached.message_id == message.id
                && cached.width == width
                && cached.selected_chip == selected_chip
                && cached.fingerprint == fingerprint
        })
    };
    if cached_hit {
        let lines = state
            .caches
            .reader_doc
            .borrow()
            .as_ref()
            .map(|cached| Rc::clone(&cached.lines))
            .expect("cache present after a hit check");
        return lines;
    }
    let lines = Rc::new(scroll_lines(state, width));
    *state.caches.reader_doc.borrow_mut() = Some(CachedReaderDoc {
        message_id: message.id.clone(),
        width,
        selected_chip,
        fingerprint,
        lines: Rc::clone(&lines),
    });
    lines
}

/// Number of fixed header lines (ticket 6864): the reducer subtracts it
/// from the viewport so the scroll clamp tracks the body alone.
pub fn header_line_count(state: &AppState, width: usize) -> usize {
    header_lines(state, width).len()
}

/// Number of scrollable body lines — the length the reducer's scroll clamp
/// clamps against (the fixed header never scrolls).
pub fn scroll_line_count(state: &AppState, width: usize) -> usize {
    scroll_document(state, width).len()
}

fn push_meta(lines: &mut Vec<ReaderLine>, label: &str, value: &str, width: usize) {
    lines.push(ReaderLine::chrome(
        Tone::Dim,
        padded_field(&format!("{label:<5} {value}"), width),
    ));
}

fn hairline(width: usize) -> ReaderLine {
    ReaderLine::chrome(Tone::Dim, "─".repeat(width))
}

/// `Name <email>` per address, or the bare email; the list uses the same
/// display rule (`Address::display`).
fn address_label(address: &crate::domain::Address) -> String {
    match &address.name {
        Some(name) if !name.is_empty() => format!("{name} <{}>", address.email),
        _ => address.email.clone(),
    }
}

fn address_list_label(addresses: &[crate::domain::Address]) -> String {
    address_list_or(addresses, "")
}

/// Joined address labels, or the given placeholder when there are none.
fn address_list_or(addresses: &[crate::domain::Address], placeholder: &str) -> String {
    if addresses.is_empty() {
        return String::from(placeholder);
    }
    addresses
        .iter()
        .map(address_label)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Reader meta date. The envelope fallback timestamp is the Unix epoch
/// (ADR 0001 finding 2: missing/unparseable dates); render that as an
/// explicit unknown instead of 1970.
fn date_label(timestamp: chrono::DateTime<chrono::FixedOffset>) -> String {
    if timestamp.timestamp() == 0 {
        String::from("unknown date")
    } else {
        dates::format_absolute(timestamp)
    }
}

fn first_line(detail: &str) -> &str {
    detail.lines().next().unwrap_or("no detail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::mock;
    use crate::app::route::{MessageRoute, Route};
    use crate::app::state::AppState;
    use crate::domain::{
        Address, Attachment, MailboxId, Message, MessageHeaders, MessageId, MessageSummary,
    };
    use unicode_width::UnicodeWidthStr;

    fn address() -> Address {
        Address {
            name: Some(String::from("Bob")),
            email: String::from("bob@example.org"),
        }
    }

    fn summary() -> MessageSummary {
        MessageSummary {
            id: MessageId(String::from("m1")),
            mailbox_id: MailboxId(String::from("inbox")),
            message_id: Some(String::from("1@tmail.local")),
            from: vec![address()],
            to: vec![Address {
                name: None,
                email: String::from("probe@tmail.local"),
            }],
            subject: String::from("Welcome to Tmail"),
            snippet: None,
            timestamp: mock::now(),
            is_read: false,
            is_starred: false,
            has_attachments: false,
        }
    }

    fn loaded_state() -> AppState {
        let mut state = mock::mock_initial_state();
        let summary = summary();
        let message = Message {
            id: summary.id.clone(),
            mailbox_id: summary.mailbox_id.clone(),
            headers: MessageHeaders::default(),
            plain_body: Some(String::from(
                "First line.\n\nSecond paragraph with a long line that will need wrapping to fit the viewport width.\n",
            )),
            html_body: None,
            attachments: vec![Attachment {
                name: Some(String::from("report.pdf")),
                mime_type: Some(String::from("application/pdf")),
                size: Some(1_200_000),
                part_id: 2,
            }],
        };
        state.session.routes.push(Route::Message(MessageRoute {
            mailbox_id: MailboxId(String::from("inbox")),
            summary,
        }));
        state.open_message = Loadable::Loaded(message);
        state.session.focus = crate::app::focus::Focus::Reader;
        state
    }

    #[test]
    fn document_contains_header_body_and_attachments() {
        let state = loaded_state();
        let lines = content(&state, 100);
        let text: Vec<String> = lines.iter().map(ReaderLine::text).collect();
        assert!(text.iter().any(|t| t.contains("Welcome to Tmail")));
        assert!(
            text.iter()
                .any(|t| t.contains("From") && t.contains("Bob <bob@example.org>"))
        );
        assert!(
            text.iter()
                .any(|t| t.contains("To") && t.contains("probe@tmail.local"))
        );
        assert!(
            text.iter()
                .any(|t| t.contains("Date") && t.contains("10:47"))
        );
        // The action buttons are gone (ticket 3rt5); the actions live on
        // their hotkeys only.
        assert!(!text.iter().any(|t| t.contains("Reply r")));
        assert!(!text.iter().any(|t| t.contains("Archive e")));
        assert!(text.iter().any(|t| t.contains("First line.")));
        assert!(text.iter().any(|t| t.contains("report.pdf")));
        assert!(text.iter().any(|t| t.contains("1.1 MB")));
        // The hairline spans the width.
        assert!(
            text.iter()
                .any(|t| t.starts_with('─') && t.chars().all(|c| c == '─'))
        );
    }

    /// Every fixed field (subject, meta) sits two symbols in from the left
    /// and right panel edges (ticket 3rt5).
    #[test]
    fn header_fields_are_padded_two_symbols_from_both_edges() {
        let state = loaded_state();
        let header = header_lines(&state, 100);
        let text: Vec<String> = header.iter().map(ReaderLine::text).collect();
        let subject = &text[0];
        assert!(subject.starts_with("  Welcome to Tmail"), "{subject:?}");
        assert!(subject.ends_with("  "), "{subject:?}");
        let from = text
            .iter()
            .find(|t| t.contains("Bob <bob@example.org>"))
            .expect("From row");
        assert!(from.starts_with("  From "), "{from:?}");
        assert!(from.ends_with("  "), "{from:?}");
        // Padded lines never exceed the panel width.
        assert!(text.iter().all(|t| t.width() <= 100));
    }

    #[test]
    fn missing_fields_render_placeholders() {
        let mut state = mock::mock_initial_state();
        let mut summary = summary();
        summary.subject = String::new();
        summary.from = Vec::new();
        summary.to = Vec::new();
        summary.timestamp = fixed_epoch();
        let message = Message {
            id: summary.id.clone(),
            mailbox_id: summary.mailbox_id.clone(),
            headers: MessageHeaders::default(),
            plain_body: None,
            html_body: None,
            attachments: Vec::new(),
        };
        state.session.routes.push(Route::Message(MessageRoute {
            mailbox_id: MailboxId(String::from("inbox")),
            summary,
        }));
        state.open_message = Loadable::Loaded(message);
        let lines = content(&state, 100);
        let text: Vec<String> = lines.iter().map(ReaderLine::text).collect();
        assert!(text.iter().any(|t| t.contains("(no subject)")));
        assert!(text.iter().any(|t| t.contains("(unknown sender)")));
        assert!(text.iter().any(|t| t.contains("(no recipients)")));
        assert!(text.iter().any(|t| t.contains("unknown date")));
        assert!(text.iter().any(|t| t.contains("(no content)")));
    }

    #[test]
    fn html_only_mail_renders_semantics() {
        let mut state = mock::mock_initial_state();
        let summary = summary();
        let message = Message {
            id: summary.id.clone(),
            mailbox_id: summary.mailbox_id.clone(),
            headers: MessageHeaders::default(),
            plain_body: None,
            html_body: Some(String::from(
                "<html><body><h1>Release</h1><p>see <a href=\"https://example.org\">the notes</a></p></body></html>",
            )),
            attachments: Vec::new(),
        };
        state.session.routes.push(Route::Message(MessageRoute {
            mailbox_id: MailboxId(String::from("inbox")),
            summary,
        }));
        state.open_message = Loadable::Loaded(message);
        let lines = content(&state, 100);
        let text: Vec<String> = lines.iter().map(ReaderLine::text).collect();
        // The markup renders as content, not as a degrade note or raw HTML.
        assert!(text.iter().any(|t| t.contains("Release")));
        assert!(text.iter().any(|t| t.contains("the notes")));
        assert!(!text.iter().any(|t| t.contains("<html>")));
        // The heading line is bold, the link span carries its target.
        let heading = lines
            .iter()
            .find(|l| l.text().contains("Release"))
            .expect("heading line");
        let ReaderLine::Rich(rich) = heading else {
            panic!("body line must be rich");
        };
        assert!(
            rich.spans
                .iter()
                .filter(|s| s.text.contains("Release"))
                .all(|s| s.style.heading && s.style.bold)
        );
        let link = lines
            .iter()
            .flat_map(|l| match l {
                ReaderLine::Rich(r) => r.spans.clone(),
                _ => Vec::new(),
            })
            .find(|s| s.text.contains("the notes"))
            .expect("link span");
        assert_eq!(link.link_target.as_deref(), Some("https://example.org"));
    }

    #[test]
    fn blank_html_falls_back_to_plain() {
        let mut state = mock::mock_initial_state();
        let summary = summary();
        let message = Message {
            id: summary.id.clone(),
            mailbox_id: summary.mailbox_id.clone(),
            headers: MessageHeaders::default(),
            plain_body: Some(String::from("plain fallback")),
            html_body: Some(String::from("<div>   </div>")),
            attachments: Vec::new(),
        };
        state.session.routes.push(Route::Message(MessageRoute {
            mailbox_id: MailboxId(String::from("inbox")),
            summary,
        }));
        state.open_message = Loadable::Loaded(message);
        let lines = content(&state, 100);
        assert!(lines.iter().any(|l| l.text().contains("plain fallback")));
    }

    #[test]
    fn unrenderable_html_without_plain_degrades_to_a_note() {
        let mut state = mock::mock_initial_state();
        let summary = summary();
        let message = Message {
            id: summary.id.clone(),
            mailbox_id: summary.mailbox_id.clone(),
            headers: MessageHeaders::default(),
            plain_body: None,
            html_body: Some(String::from("<div>   </div>")),
            attachments: Vec::new(),
        };
        state.session.routes.push(Route::Message(MessageRoute {
            mailbox_id: MailboxId(String::from("inbox")),
            summary,
        }));
        state.open_message = Loadable::Loaded(message);
        let lines = content(&state, 100);
        assert!(
            lines
                .iter()
                .any(|l| l.text().contains("(HTML message could not be rendered)"))
        );
    }

    #[test]
    fn loading_and_failed_states_render() {
        let mut state = mock::mock_initial_state();
        state.session.routes.push(Route::Message(MessageRoute {
            mailbox_id: MailboxId(String::from("inbox")),
            summary: summary(),
        }));
        // Loading: the body lays out empty — the pane spinner stands in
        // for it (ticket m3by), so nothing but the header is in the
        // document.
        state.open_message = Loadable::Loading;
        assert!(scroll_line_count(&state, 100) == 0, "no body while loading");

        state.open_message = Loadable::Failed(String::from("boom\nmore"));
        let lines = content(&state, 100);
        assert!(
            lines
                .iter()
                .any(|l| l.text().contains("message could not be loaded")
                    && l.text().contains("boom"))
        );
    }

    #[test]
    fn body_wraps_at_viewport_width() {
        let state = loaded_state();
        let lines = content(&state, 40);
        let body: Vec<&ReaderLine> = lines
            .iter()
            .filter(|l| l.text().starts_with(INDENT) && !l.text().contains('['))
            .collect();
        assert!(
            body.iter().all(|l| l.text().width() <= 40),
            "body lines must respect the width: {body:?}"
        );
    }

    #[test]
    fn content_count_matches_document() {
        let state = loaded_state();
        // The two counters split the document exactly (ticket 6864): the
        // reducer clamps with header + scroll lengths, the renderer draws
        // the same halves.
        assert_eq!(
            header_line_count(&state, 100) + scroll_line_count(&state, 100),
            content(&state, 100).len()
        );
        assert!(scroll_line_count(&state, 100) > 5);
    }

    // ── Document cache (perf: the doc must not rebuild per event) ────────

    /// A cache hit serves the same shared lines: two calls in a row share
    /// one `Rc` allocation.
    #[test]
    fn repeated_counts_hit_the_cache() {
        let state = loaded_state();
        let first = scroll_document(&state, 100);
        let second = scroll_document(&state, 100);
        assert!(Rc::ptr_eq(&first, &second), "second call must hit cache");
        assert_eq!(scroll_line_count(&state, 100), first.len());
    }

    /// A width change reflows: the cache re-keys instead of serving stale
    /// geometry.
    #[test]
    fn a_width_change_rebuilds() {
        let state = loaded_state();
        let wide = scroll_document(&state, 100);
        let narrow = scroll_document(&state, 30);
        assert!(!Rc::ptr_eq(&wide, &narrow));
        assert!(narrow.len() >= wide.len(), "narrower wrap adds lines");
        // The new key sticks.
        assert!(Rc::ptr_eq(&narrow, &scroll_document(&state, 30)));
    }

    /// Tabbing the attachment cursor rebuilds (chip markers are baked
    /// into the lines).
    #[test]
    fn a_chip_cursor_change_rebuilds() {
        let mut state = loaded_state();
        let before = scroll_document(&state, 100);
        state.reader_attachment = Some(0);
        let after = scroll_document(&state, 100);
        assert!(!Rc::ptr_eq(&before, &after));
        assert_eq!(before.len(), after.len());
    }

    /// A same-id body swap (e.g. a refetch) must not serve stale lines:
    /// the fingerprint catches it.
    #[test]
    fn a_body_change_rebuilds() {
        let mut state = loaded_state();
        scroll_document(&state, 100);
        if let Loadable::Loaded(message) = &mut state.open_message {
            message.plain_body = Some(String::from(
                "First line.\n\nSecond paragraph.\n\nThird paragraph with another long line that will need wrapping to fit the viewport width.\n",
            ));
        }
        let rebuilt = scroll_document(&state, 100);
        let text: Vec<String> = rebuilt.iter().map(ReaderLine::text).collect();
        assert!(text.iter().any(|t| t.contains("Third paragraph")));
    }

    /// Loading/failed documents are placeholders, not cached bodies.
    #[test]
    fn unloaded_phases_are_not_cached() {
        let mut state = loaded_state();
        scroll_document(&state, 100);
        state.open_message = Loadable::Loading;
        assert!(scroll_line_count(&state, 100) == 0);
        assert!(scroll_document(&state, 100).is_empty());
        state.open_message = Loadable::Idle;
        assert_eq!(scroll_line_count(&state, 100), 1);
    }

    #[test]
    fn header_is_fixed_and_body_scrolls() {
        let state = loaded_state();
        let header = header_lines(&state, 100);
        let body = scroll_lines(&state, 100);
        // The header carries exactly the pinned chrome: subject, meta, and
        // the hairline — never body content, no action row (ticket 3rt5).
        let header_text: Vec<String> = header.iter().map(ReaderLine::text).collect();
        assert!(header_text[0].contains("Welcome to Tmail"));
        assert!(
            header_text
                .last()
                .is_some_and(|t| t.starts_with('─') && t.chars().all(|c| c == '─'))
        );
        assert!(!header_text.iter().any(|t| t.contains("First line.")));
        // The body carries the message text and attachments, none of the
        // pinned chrome.
        let body_text: Vec<String> = body.iter().map(ReaderLine::text).collect();
        assert!(body_text.iter().any(|t| t.contains("First line.")));
        assert!(body_text.iter().any(|t| t.contains("report.pdf")));
    }

    #[test]
    fn selected_chip_carries_the_cursor_marker() {
        let mut state = loaded_state();
        if let Loadable::Loaded(message) = &mut state.open_message {
            message.attachments.push(Attachment {
                name: Some(String::from("second.png")),
                mime_type: Some(String::from("image/png")),
                size: Some(2_048),
                part_id: 3,
            });
        }
        // Default selection: the first chip.
        let lines = content(&state, 100);
        let chips: Vec<&str> = lines
            .iter()
            .filter_map(|l| match l {
                ReaderLine::Chip { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(chips.len(), 2);
        assert!(chips[0].starts_with("  ▸ [ report.pdf"), "{:?}", chips);
        assert!(chips[1].starts_with("    [ second.png"), "{:?}", chips);

        // The cursor moves to the second chip; both stay single lines.
        state.reader_attachment = Some(1);
        let lines = content(&state, 100);
        let chips: Vec<&str> = lines
            .iter()
            .filter_map(|l| match l {
                ReaderLine::Chip { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(chips[0].starts_with("    [ report.pdf"), "{:?}", chips);
        assert!(chips[1].starts_with("  ▸ [ second.png"), "{:?}", chips);
    }

    /// The envelope fallback timestamp (missing/unparseable Date) renders
    /// as an explicit unknown instead of 1970.
    fn fixed_epoch() -> chrono::DateTime<chrono::FixedOffset> {
        chrono::DateTime::from_timestamp(0, 0)
            .expect("epoch")
            .with_timezone(&chrono::FixedOffset::east_opt(0).expect("utc"))
    }
}
