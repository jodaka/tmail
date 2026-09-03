//! Reader screen: exactly one message (mockup `viewer.html`, plan §19
//! Phase 4). v1 overrides applied (plan §4): no thread count, no collapsed
//! messages, no thread navigation, no label tags.
//!
//! The whole reader is one scrollable document — header block, action row,
//! hairline, body, attachments — mirroring the mockup's `.reader` scroll
//! container. Content is built as tone-tagged lines by a pure function
//! shared with the reducer's scroll clamp, so what is drawn and what the
//! clamp allows can never disagree.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::state::{AppState, Loadable};
use crate::ui::dates;
use crate::ui::rich::{RichLine, RichSpan, RichStyle};
use crate::ui::text;
use crate::ui::theme::Theme;

/// Visual tone of one chrome reader line; the renderer maps tones to theme
/// styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    /// Subject line (mockup `.thread-subject`).
    Strong,
    /// Body text (mockup `.m-text`) and the action row.
    Body,
    /// Meta labels, hairlines, placeholders, attachment chips.
    Dim,
}

/// One line of the reader document: either tone-styled chrome (subject,
/// meta, actions, attachments) or a rich body line whose spans carry the
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

/// Body indent (mockup `.m-text` `padding-left: 2ch`).
const INDENT: &str = "  ";

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

/// Build the whole reader document for `width` display columns.
/// Deterministic and I/O-free; the reducer uses only its length for the
/// scroll clamp (Phase 3 modal pattern).
pub(crate) fn content(state: &AppState, width: usize) -> Vec<ReaderLine> {
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
    lines.push(ReaderLine::chrome(Tone::Strong, text::truncate(subject, w)));

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

    // Action row (mockup `.thread-actions`): keyboard-first, plan §10 keys.
    // Save/open hints appear only when attachments exist (plan §15).
    let actions = match state.open_message.as_loaded().map(|m| m.attachments.len()) {
        Some(count) if count > 0 => String::from(
            "Reply r · Forward f · Archive e · Star s · Unread u · Delete ⌫ · Save d · Open o · Tab chip",
        ),
        _ => String::from("Reply r · Forward f · Archive e · Star s · Unread u · Delete ⌫"),
    };
    lines.push(ReaderLine::chrome(Tone::Body, actions));
    lines.push(hairline(w));

    // Body (plan §13: HTML-preferred selection, rich semantic rendering,
    // reflow at the current width; Unicode-safe via ui::text and html2text).
    // Missing bodies render explicit placeholders (plan §19 Phase 4).
    let body: Vec<RichLine> = match &state.open_message {
        Loadable::Loaded(message) => {
            crate::ui::rich::body_lines(message, w.saturating_sub(INDENT.len()))
        }
        Loadable::Failed(detail) => vec![RichLine::from_plain(format!(
            "(message could not be loaded — {})",
            first_line(detail)
        ))],
        // `Idle` is defensive: a reader route without its load lifecycle.
        Loadable::Idle => vec![RichLine::from_plain("(no message loaded)")],
        Loadable::Loading => vec![RichLine::from_plain("(loading message…)")],
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
                .map(crate::ui::text::human_size)
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

/// Total document length, the number the reducer's scroll clamp uses.
pub fn content_line_count(state: &AppState, width: usize) -> usize {
    content(state, width).len()
}

/// Render the reader into `area` (the body area right of the sidebar). The
/// document scrolls from the reducer-maintained `reader_scroll` anchor.
/// The wrap width derives from the *terminal* size via `reader_width` —
/// the same call the reducer's scroll clamp makes — never from the sub-rect
/// alone, whose dimensions would re-run mode selection on the wrong frame
/// (caught by the Phase 5 corpus below 120 columns).
pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let width = crate::ui::layout::reader_width(state.size).max(10);
    let lines = content(state, width);
    let viewport = area.height as usize;
    let start = state.reader_scroll.min(lines.len().saturating_sub(1));
    let visible: Vec<Line<'_>> = lines
        .iter()
        .skip(start)
        .take(viewport)
        .map(|line| reader_spans(line, theme, area.width as usize))
        .collect();
    frame.render_widget(Paragraph::new(visible), area);
}

/// Style for one rich span (plan §13 semantic table → theme tokens; no
/// literal colors here). Flags compose: a link inside a blockquote keeps
/// its accent so it stays discoverable.
fn rich_span_style(style: RichStyle, theme: &Theme) -> Style {
    let mut out = Style::new().fg(theme.text_soft).bg(theme.background);
    if style.blockquote {
        out = theme.blockquote();
    }
    if style.code {
        out = theme.code();
    }
    if style.bold || style.heading {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.heading {
        out = out.fg(theme.text);
    }
    if style.italic {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if style.link {
        out = theme.link();
    }
    out
}

/// Convert one document line into Ratatui spans, clipped to `width` as the
/// final safety net (html2text and `ui::text` already wrap; code/pre lines
/// keep their whitespace).
fn reader_spans<'a>(line: &'a ReaderLine, theme: &'a Theme, width: usize) -> Line<'a> {
    match line {
        ReaderLine::Chrome { tone, text } => {
            let style = match tone {
                Tone::Strong => Style::new()
                    .fg(theme.text)
                    .bg(theme.background)
                    .add_modifier(Modifier::BOLD),
                Tone::Body => Style::new().fg(theme.text_soft).bg(theme.background),
                Tone::Dim => Style::new().fg(theme.dim).bg(theme.background),
            };
            Line::from(Span::styled(text::clip(text, width), style))
        }
        ReaderLine::Chip { text, selected } => {
            let style = if *selected {
                Style::new()
                    .fg(theme.accent)
                    .bg(theme.background)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(theme.dim).bg(theme.background)
            };
            Line::from(Span::styled(text::clip(text, width), style))
        }
        ReaderLine::Rich(rich) => Line::from(
            rich.spans
                .iter()
                .map(|span| {
                    Span::styled(
                        text::clip(&span.text, width),
                        rich_span_style(span.style, theme),
                    )
                })
                .collect::<Vec<_>>(),
        ),
    }
}

fn push_meta(lines: &mut Vec<ReaderLine>, label: &str, value: &str, width: usize) {
    lines.push(ReaderLine::chrome(
        Tone::Dim,
        text::truncate(&format!("{label:<5} {value}"), width),
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
            message_id: Some(String::from("1@post.local")),
            from: vec![address()],
            to: vec![Address {
                name: None,
                email: String::from("probe@post.local"),
            }],
            subject: String::from("Welcome to Post"),
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
        state.routes.push(Route::Message(MessageRoute {
            mailbox_id: MailboxId(String::from("inbox")),
            summary,
        }));
        state.open_message = Loadable::Loaded(message);
        state.focus = crate::app::focus::Focus::Reader;
        state
    }

    #[test]
    fn document_contains_header_body_actions_attachments() {
        let state = loaded_state();
        let lines = content(&state, 100);
        let text: Vec<String> = lines.iter().map(ReaderLine::text).collect();
        assert!(text.iter().any(|t| t.contains("Welcome to Post")));
        assert!(
            text.iter()
                .any(|t| t.contains("From") && t.contains("Bob <bob@example.org>"))
        );
        assert!(
            text.iter()
                .any(|t| t.contains("To") && t.contains("probe@post.local"))
        );
        assert!(
            text.iter()
                .any(|t| t.contains("Date") && t.contains("10:47"))
        );
        assert!(text.iter().any(|t| t.contains("Archive e")));
        assert!(text.iter().any(|t| t.contains("First line.")));
        assert!(text.iter().any(|t| t.contains("report.pdf")));
        assert!(text.iter().any(|t| t.contains("1.1 MB")));
        // The hairline spans the width.
        assert!(
            text.iter()
                .any(|t| t.starts_with('─') && t.chars().all(|c| c == '─'))
        );
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
        state.routes.push(Route::Message(MessageRoute {
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
        state.routes.push(Route::Message(MessageRoute {
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
        state.routes.push(Route::Message(MessageRoute {
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
        state.routes.push(Route::Message(MessageRoute {
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
        state.routes.push(Route::Message(MessageRoute {
            mailbox_id: MailboxId(String::from("inbox")),
            summary: summary(),
        }));
        state.open_message = Loadable::Loading;
        let lines = content(&state, 100);
        assert!(
            lines
                .iter()
                .any(|l| l.text().contains("(loading message…)"))
        );

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
        let count = content_line_count(&state, 100);
        assert_eq!(count, content(&state, 100).len());
        assert!(count > 5);
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
