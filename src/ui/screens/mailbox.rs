//! Mailbox screen: list header + message rows (mockup `list.html`).
//!
//! One row per message (the mockup's single-line grid). Unread rows get the
//! `unread` modifier; the selected row gets the accent fill. No thread
//! count, no labels column, no tags (plan §4 overrides; labels carry no
//! backend meaning — ADR 0001).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::focus::Focus;
use crate::app::state::AppState;
use crate::domain::MessageSummary;
use crate::ui::dates;
use crate::ui::layout::LayoutMode;
use crate::ui::text;
use crate::ui::theme::Theme;

/// Render the message list into `area` (already split off from the sidebar).
pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    mode: LayoutMode,
    theme: &Theme,
    now: chrono::DateTime<chrono::FixedOffset>,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let (head, rows) = crate::ui::layout::split_list(area);
    render_head(frame, head, state, theme);

    // Draw from the reducer-maintained scroll anchor so the selected row is
    // always on screen regardless of movement, page loads, or resize
    // (Phase 2 acceptance).
    let bottom = area.y + area.height;
    for (y, (i, message)) in (rows.y..).zip(
        state
            .messages
            .items
            .iter()
            .enumerate()
            .skip(state.list_scroll),
    ) {
        if y >= bottom {
            break;
        }
        let selected = i == state.selection;
        let row_area = Rect {
            x: rows.x,
            y,
            width: rows.width,
            height: 1,
        };
        let spans = message_spans(
            message,
            rows.width as usize,
            mode,
            theme,
            now,
            selected,
            state.focus == Focus::MessageList,
        );
        frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
    }
}

fn render_head(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let row = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    let width = area.width as usize;

    let title = state
        .active_mailbox_name()
        .unwrap_or("Mailbox")
        .to_uppercase();
    let unread = state
        .active_mailbox_unread()
        .map(|n| format!("{n} unread"))
        .unwrap_or_default();
    let range = state.messages.range_label();

    // Right-aligned range (mockup `.pane-range`).
    let range_w = range.width();
    let left_budget = width.saturating_sub(range_w + 2).max(10);
    let left = format!("[ ]  {title}");
    let left = text::clip(&left, left_budget);
    let mut spans = vec![Span::styled(
        left,
        Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
    )];
    if !unread.is_empty() {
        spans.push(Span::styled(
            format!("  {unread}"),
            Style::new().fg(theme.dim),
        ));
    }
    if !range.is_empty() {
        let range_area = Rect {
            x: area.x + area.width - range_w as u16,
            y: row.y,
            width: range_w as u16,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Span::styled(range, Style::new().fg(theme.dim))),
            range_area,
        );
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), row);

    // Hairline under the header.
    let hairline = Rect {
        x: area.x,
        y: area.y.saturating_add(1),
        width: area.width,
        height: 1,
    };
    frame.render_widget(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(theme.hairline()),
        hairline,
    );
}

#[allow(clippy::too_many_arguments)]
fn message_spans<'a>(
    message: &'a MessageSummary,
    width: usize,
    mode: LayoutMode,
    theme: &'a Theme,
    now: chrono::DateTime<chrono::FixedOffset>,
    selected: bool,
    // The message list holds focus: the selected row shows the accent
    // marker (the same bar the sidebar's focused folder carries). Selection
    // alone keeps the fill but never the marker, so focus stays readable.
    focused: bool,
) -> Vec<Span<'a>> {
    let full = mode == LayoutMode::Full;
    let compact = mode == LayoutMode::Compact;

    // Column anatomy: marker 1 + star 1 + from + gap 2 + subject(+snippet) +
    // gap 2 + date 8, summing to the full row width so the date lands on
    // the right edge (mockup grid).
    let date = dates::format_relative(now, message.timestamp);
    let from_w = if compact { 14 } else { 18 };
    let subject_w = width.saturating_sub(2 + from_w + 2 + 2 + 8).max(10);

    let bg = if selected {
        theme.accent_bg
    } else {
        theme.background
    };
    let base = if selected {
        theme.row_selected()
    } else {
        Style::new().bg(theme.background)
    };

    let star = if message.is_starred {
        Span::styled("*", theme.star().bg(bg))
    } else {
        Span::styled(" ", base)
    };
    // Accent bar in the marker column (mockup `.folder.active` bar): marks
    // the focused row while the list holds focus.
    let marker = if selected && focused {
        Span::styled("▏", Style::new().fg(theme.accent).bg(bg))
    } else {
        Span::styled(" ", base)
    };

    let from_style = if selected {
        base.fg(theme.text)
    } else if message.is_read {
        theme.read_text().bg(theme.background)
    } else {
        theme.unread_text().bg(theme.background)
    };
    let subject_style = from_style;

    // Subject plus (full mode only) a dim snippet, composed first so the
    // combined cell can be padded to the exact column width.
    let combined = if full {
        match &message.snippet {
            Some(snippet) => format!("{} — {snippet}", message.subject),
            None => message.subject.clone(),
        }
    } else {
        message.subject.clone()
    };
    let combined = text::clip(&combined, subject_w);

    let mut spans = vec![
        marker,
        star,
        Span::styled(text::fit_left(message.from_display(), from_w), from_style),
        Span::styled("  ", base),
        Span::styled(text::fit_left(&combined, subject_w), subject_style),
    ];
    let date_style = if message.is_read && !selected {
        Style::new().fg(theme.dim).bg(bg)
    } else {
        from_style.bg(bg)
    };
    spans.push(Span::styled("  ", base));
    spans.push(Span::styled(text::fit_left(&date, 8), date_style));
    spans
}
