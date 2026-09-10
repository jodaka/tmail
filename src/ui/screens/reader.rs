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
use ratatui::widgets::{Paragraph, ScrollbarState};

use crate::app::action::ClickTarget;
use crate::app::reader::{ReaderLine, Tone, header_lines, scroll_document};
use crate::app::state::{AppState, Loadable};
use crate::input::mouse::HitMap;
use crate::ui::chrome;
use crate::ui::theme::Theme;
use crate::view::rich::RichStyle;
use crate::view::text;

pub use crate::app::reader::{header_line_count, scroll_line_count};

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
    let width = crate::ui::layout::reader_width(state.size).max(10);
    let header = header_lines(state, width);
    let body = scroll_document(state, width);

    let body_area = render_header(frame, area, theme, &header);
    if body_area.height == 0 {
        return;
    }
    // While the message loads the body carries nothing: the centered pane
    // spinner stands in for it (ticket m3by).
    if matches!(state.open_message, Loadable::Loading) {
        crate::ui::components::spinner::render_centered(frame, body_area, theme, state.ticks);
        return;
    }
    render_body(frame, body_area, theme, &body, state.reader_scroll);
    push_chip_targets(hits, &body, body_area, state);
}

/// The fixed header: subject, meta block, hairline, each field padded two
/// symbols in from the panel edges (ticket 6864). It never scrolls.
fn render_header(frame: &mut Frame<'_>, area: Rect, theme: &Theme, header: &[ReaderLine]) -> Rect {
    let header_h = header.len().min(area.height as usize) as u16;
    for (i, line) in header.iter().take(header_h as usize).enumerate() {
        let row = Rect {
            x: area.x,
            y: area.y + i as u16,
            width: area.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(reader_spans(line, theme, area.width as usize)),
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
/// body overflows the viewport, ticket 6864).
fn render_body(
    frame: &mut Frame<'_>,
    body_area: Rect,
    theme: &Theme,
    body: &[ReaderLine],
    reader_scroll: usize,
) {
    let viewport = body_area.height as usize;
    let total = body.len();
    let start = reader_scroll.min(total.saturating_sub(1));
    let scrolling = total > viewport;
    // A visible scrollbar reserves its column: body text clips one column
    // short so text and scrollbar never overlap. When the message fits,
    // the full width is used and no scrollbar is drawn.
    let text_width = if scrolling {
        (body_area.width as usize).saturating_sub(1)
    } else {
        body_area.width as usize
    };
    let visible: Vec<Line<'_>> = body
        .iter()
        .skip(start)
        .take(viewport)
        .map(|line| reader_spans(line, theme, text_width))
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
        let mut scrollbar_state = ScrollbarState::new(total).position(start);
        frame.render_stateful_widget(chrome::scrollbar(theme), body_area, &mut scrollbar_state);
    }
}

/// Clickable attachment chips indexed across the whole body, so scrolling
/// never shifts the identity of the visible chips; their rects follow the
/// scroll offset.
fn push_chip_targets(hits: &mut HitMap, body: &[ReaderLine], body_area: Rect, state: &AppState) {
    let viewport = body_area.height as usize;
    let start = state.reader_scroll.min(body.len().saturating_sub(1));
    let width = body_area.width;
    let mut chip_index = 0usize;
    for (offset, line) in body.iter().enumerate() {
        if matches!(line, ReaderLine::Chip { .. }) {
            let visible = offset >= start && offset < start + viewport;
            if visible {
                hits.push(
                    Rect {
                        x: body_area.x,
                        y: body_area.y + (offset - start) as u16,
                        width,
                        height: 1,
                    },
                    ClickTarget::ReaderAttachment(chip_index),
                );
            }
            chip_index += 1;
        }
    }
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
