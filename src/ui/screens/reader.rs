//! Reader screen: exactly one message (mockup `viewer.html`, plan §19
//! Phase 4). v1 overrides applied (plan §4): no thread count, no collapsed
//! messages, no thread navigation, no label tags. The action buttons are
//! gone (ticket 3rt5): every action stays bound to its hotkey, only the
//! clickable row is gone.
//!
//! The document splits into a *fixed header* — subject, meta block,
//! hairline, each field padded two symbols in from the panel edges — and a
//! *scrollable body* (message text and attachments, ticket 6864): the
//! header stays pinned at the top of the area on every frame while a long
//! message scrolls beneath it, with a vertical
//! scrollbar that appears only when the body overflows the viewport.
//! The document model itself lives in `app::reader`, shared with the
//! reducer's scroll clamp, so what is drawn and what the clamp allows can
//! never disagree.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::action::ClickTarget;
use crate::app::reader::{LinkRuns, ReaderDoc, ReaderLine, Tone, header_lines, scroll_document};
use crate::app::state::{AppState, Loadable, ReaderFocus};
use crate::input::mouse::HitMap;
use crate::ui::chrome;
use crate::ui::theme::Theme;
use crate::view::rich::RichStyle;
use crate::view::text;

/// Render the reader into `area` (the body area right of the sidebar). The
/// fixed header draws from the top of `area` on every frame (ticket 6864);
/// the body scrolls beneath it from the reducer-maintained `reader_scroll`
/// anchor, with a vertical scrollbar only while it overflows the viewport.
/// The wrap width derives from the *terminal* size via `reader_width` —
/// the same call the reducer's scroll clamp makes — never from the sub-rect
/// alone, whose dimensions would re-run mode selection on the wrong frame
/// (caught by the Phase 5 corpus below 120 columns).
pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    hits: &mut HitMap,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let width = crate::ui::layout::reader_width(state.session.size);
    let header = header_lines(state, width);
    let doc = scroll_document(state, width);

    let body_area = render_header(frame, area, theme, &header);
    if body_area.height == 0 {
        return;
    }
    // While the message loads the body carries nothing: the centered pane
    // spinner stands in for it (ticket m3by).
    if matches!(state.open_message, Loadable::Loading) {
        crate::ui::components::spinner::render_centered(
            frame,
            body_area,
            theme,
            crate::ui::components::spinner::pane_millis(state),
        );
        return;
    }
    render_body(frame, body_area, theme, &doc, state, hits);
}

/// The fixed header: subject, meta block, hairline, each field padded two
/// symbols in from the panel edges (ticket 6864). It never scrolls.
fn render_header(frame: &mut Frame<'_>, area: Rect, theme: &Theme, header: &[ReaderLine]) -> Rect {
    let header_h = header.len().min(area.height as usize) as u16;
    // The fixed header carries no links and no focus cursor; the empty
    // run map keeps the shared span builder total.
    let no_links = LinkRuns::default();
    for (i, line) in header.iter().take(header_h as usize).enumerate() {
        let row = Rect {
            x: area.x,
            y: area.y + i as u16,
            width: area.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(reader_spans(
                line,
                theme,
                area.width as usize,
                i,
                &no_links,
                None,
            )),
            row,
        );
    }
    Rect {
        x: area.x,
        y: area.y + header_h,
        width: area.width,
        height: area.height - header_h,
    }
}

/// The scrollable body viewport with its scrollbar (appears only when the
/// body overflows the viewport, ticket 6864). Drawing and hit-map
/// recording happen together here: both walk the same [`LinkRuns`] map, so
/// a click can never land on a different link than the one drawn (ticket
/// hc9n).
fn render_body(
    frame: &mut Frame<'_>,
    body_area: Rect,
    theme: &Theme,
    doc: &ReaderDoc,
    state: &AppState,
    hits: &mut HitMap,
) {
    let body = &doc.lines;
    let viewport = body_area.height as usize;
    let total = body.len();
    let start = state.reader_scroll.min(total.saturating_sub(1));
    let scrolling = total > viewport;
    // A visible scrollbar reserves its column: body text clips one column
    // short so text and scrollbar never overlap. When the message fits,
    // the full width is used and no scrollbar is drawn. The document
    // itself wraps `SCROLLBAR_RESERVE` (rail + padding, ticket ytqd) short
    // of the panel width, so a rail never hides the last symbol of a long
    // line — the clip below is only a safety net.
    let text_width = chrome::scrollbar_content_width(body_area.width, total, viewport) as usize;
    let links = &doc.links;
    let focus = state.reader_focus;
    let visible: Vec<Line<'_>> = body
        .iter()
        .enumerate()
        .skip(start)
        .take(viewport)
        .map(|(offset, line)| reader_spans(line, theme, text_width, offset, links, focus))
        .collect();
    let text_area = if scrolling {
        Rect {
            width: body_area.width - 1,
            ..body_area
        }
    } else {
        body_area
    };
    frame.render_widget(Paragraph::new(visible), text_area);
    if scrolling {
        chrome::render_scrollbar(frame, body_area, theme, total, start);
    }
    push_body_targets(hits, body, text_area, start, viewport, links);
}

/// Record the clickable rectangles of the body: link segments (ticket
/// hc9n) and attachment chips (plan §15). Rectangles are indexed across
/// the whole body, so scrolling never shifts the identity of the visible
/// items; their rects follow the scroll offset.
fn push_body_targets(
    hits: &mut HitMap,
    body: &[ReaderLine],
    area: Rect,
    start: usize,
    viewport: usize,
    links: &LinkRuns,
) {
    let width = area.width as usize;
    let mut chip_index = 0usize;
    for (offset, line) in body.iter().enumerate() {
        let visible = offset >= start && offset < start + viewport;
        match line {
            ReaderLine::Chip { .. } => {
                if visible {
                    hits.push(
                        Rect {
                            x: area.x,
                            y: area.y + (offset - start) as u16,
                            width: area.width,
                            height: 1,
                        },
                        ClickTarget::ReaderAttachment(chip_index),
                    );
                }
                chip_index += 1;
            }
            // The body indent and every span travel through the same
            // `text::clip` the renderer uses, so the recorded x offsets are
            // exactly the drawn columns.
            ReaderLine::Rich(rich) if visible => {
                let mut x = area.x;
                for (span_index, span) in rich.spans.iter().enumerate() {
                    let span_width = text::clip(&span.text, width).width() as u16;
                    if let Some(run) = links.run_of(offset, span_index)
                        && span_width > 0
                    {
                        hits.push(
                            Rect {
                                x,
                                y: area.y + (offset - start) as u16,
                                width: span_width,
                                height: 1,
                            },
                            ClickTarget::ReaderLink(run),
                        );
                    }
                    x = x.saturating_add(span_width);
                }
            }
            _ => {}
        }
    }
}

/// Style for one rich span (plan §13 semantic table → theme tokens; no
/// literal colors here). Flags compose: a link inside a blockquote keeps
/// its accent so it stays discoverable; the focused link (ticket hc9n)
/// gets the stronger cursor style.
fn rich_span_style(style: RichStyle, theme: &Theme, focused: bool) -> Style {
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
        out = if focused {
            theme.link_focused()
        } else {
            theme.link()
        };
    }
    out
}

/// Convert one document line into Ratatui spans, clipped to `width` as the
/// final safety net (html2text and `ui::text` already wrap; code/pre lines
/// keep their whitespace). `links` resolves which span carries the focused
/// link run, so the cursor style is drawn without rebuilding the document.
fn reader_spans<'a>(
    line: &'a ReaderLine,
    theme: &'a Theme,
    width: usize,
    line_index: usize,
    links: &LinkRuns,
    focus: Option<ReaderFocus>,
) -> Line<'a> {
    match line {
        ReaderLine::Chrome { tone, text } => {
            let style = match tone {
                Tone::Strong => Style::new()
                    .fg(theme.text)
                    .bg(theme.background)
                    .add_modifier(Modifier::BOLD),
                Tone::Dim => Style::new().fg(theme.dim).bg(theme.background),
            };
            Line::from(Span::styled(text::clip(text, width), style))
        }
        ReaderLine::Chip {
            text,
            selected,
            focused,
        } => {
            let style = if *focused {
                // Button fill (the composer's focused controls carry the
                // same badge): accent text alone is not a visible cursor.
                theme.mode_badge()
            } else if *selected {
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
                .enumerate()
                .map(|(span_index, span)| {
                    let focused = matches!(
                        focus,
                        Some(ReaderFocus::Link(run))
                            if links.run_of(line_index, span_index) == Some(run)
                    );
                    Span::styled(
                        text::clip(&span.text, width),
                        rich_span_style(span.style, theme, focused),
                    )
                })
                .collect::<Vec<_>>(),
        ),
    }
}
