//! Mailbox screen: list header + message rows (mockup `list.html`).
//!
//! One row per message (the mockup's single-line grid). Unread rows get the
//! `unread` modifier; the selected row gets the accent fill. The same list
//! renders search results (Phase 9): the head then names the query, and an
//! empty result set is a valid, explicit state. No thread count, no labels
//! column, no tags (plan §4 overrides; labels carry no backend meaning —
//! ADR 0001).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use unicode_width::UnicodeWidthStr;

use crate::app::action::ClickTarget;
use crate::app::focus::Focus;
use crate::app::route::Route;
use crate::app::state::AppState;
use crate::domain::MessageSummary;
use crate::input::mouse::HitMap;
use crate::ui::dates;
use crate::ui::layout::LayoutMode;
use crate::ui::text;
use crate::ui::theme::Theme;

/// Render the message list into `area` (already split off from the sidebar).
#[allow(clippy::too_many_arguments)]
pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    mode: LayoutMode,
    theme: &Theme,
    now: chrono::DateTime<chrono::FixedOffset>,
    hits: &mut HitMap,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let (head, rows) = crate::ui::layout::split_list(area);
    render_head(frame, head, state, theme, hits);

    // Draw from the reducer-maintained scroll anchor so the selected row is
    // always on screen regardless of movement, page loads, or resize
    // (Phase 2 acceptance). When the page holds more rows than fit, a
    // vertical scrollbar takes the last column (ticket kjfq) and the rows
    // clip one column short.
    let visible_rows = rows.height as usize;
    let scrolling = state.messages.items.len() > visible_rows;
    let row_width = if scrolling {
        rows.width.saturating_sub(1)
    } else {
        rows.width
    };
    let bottom = area.y + area.height;
    let mut drew_any_row = false;
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
        drew_any_row = true;
        let selected = i == state.selection;
        let bulk_selected = state.selected.contains(&message.id);
        let row_area = Rect {
            x: rows.x,
            y,
            width: row_width,
            height: 1,
        };
        let spans = message_spans(
            message,
            row_width as usize,
            mode,
            theme,
            now,
            selected,
            bulk_selected,
            state.focus == Focus::MessageList,
        );
        frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
        // Clicking a row selects it; clicking the selected row opens it
        // (arrows + Enter, plan §10).
        hits.push(row_area, ClickTarget::MessageRow(i));
    }
    if scrolling {
        let mut scrollbar_state =
            ScrollbarState::new(state.messages.items.len()).position(state.list_scroll);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .track_style(Style::new().fg(theme.border).bg(theme.background))
                .thumb_style(Style::new().fg(theme.dim).bg(theme.background)),
            rows,
            &mut scrollbar_state,
        );
    }
    // An empty search result set is a valid state, not an error — say so
    // once the request is no longer in flight (Phase 9.3). While the list
    // is empty and a load is in flight, show the pane spinner instead
    // (ticket m3by: one loader look everywhere, centered in the panel).
    if !drew_any_row && list_load_in_flight(state) {
        crate::ui::components::spinner::render_centered(frame, rows, theme, state.ticks);
    } else if !drew_any_row
        && matches!(state.active_route(), Some(Route::Search(_)))
        && state.operations.foreground().is_none()
    {
        render_note(frame, rows, theme, "(no results)");
    }
}

/// Whether the visible list is waiting for its first rows: a page or
/// search load in flight for the active route (ticket m3by).
fn list_load_in_flight(state: &AppState) -> bool {
    match state.active_route() {
        Some(Route::Search(route)) => state
            .operations
            .search_in_flight(&route.mailbox_id)
            .is_some(),
        Some(route) => route
            .mailbox_id()
            .is_some_and(|id| state.operations.page_in_flight(id).is_some()),
        None => false,
    }
}

/// A dim one-line note in the list area (Phase 9: empty search results).
fn render_note(frame: &mut Frame<'_>, rows: Rect, theme: &Theme, note: &str) {
    if rows.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Span::styled(
            text::clip(note, rows.width as usize),
            Style::new().fg(theme.dim),
        )),
        Rect {
            x: rows.x,
            y: rows.y,
            width: rows.width,
            height: 1,
        },
    );
}

fn render_head(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    hits: &mut HitMap,
) {
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

    // Search results headline the query; a mailbox names itself (Phase 9).
    let (title, unread) = match state.active_route() {
        Some(Route::Search(route)) => (format!("SEARCH — {}", route.query), String::new()),
        _ => {
            let title = state
                .active_mailbox_name()
                .unwrap_or("Mailbox")
                .to_uppercase();
            let unread = state
                .active_mailbox_unread()
                .map(|n| format!("{n} unread"))
                .unwrap_or_default();
            (title, unread)
        }
    };
    let range = state.messages.range_label();

    // Right-aligned range (mockup `.pane-range`).
    let range_w = range.width();
    let left_budget = width.saturating_sub(range_w + 2).max(10);
    // The select-all toggle (ticket p0s3): `[X]` while every visible row
    // carries the bulk mark, `[ ]` otherwise. Clicking it (or Ctrl+A, or
    // Enter while it holds focus) flips the whole visible set.
    let checkbox = if state.all_visible_selected() {
        "[X]"
    } else {
        "[ ]"
    };
    let toggle_style = if state.focus == Focus::SelectAllToggle {
        Style::new()
            .fg(theme.accent)
            .bg(theme.background)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new()
            .fg(theme.text)
            .bg(theme.background)
            .add_modifier(Modifier::BOLD)
    };
    let title = text::clip(&format!("  {title}"), left_budget.saturating_sub(3));
    let title_width = title.width();
    let mut spans = vec![Span::styled(checkbox, toggle_style)];
    if !title.is_empty() {
        spans.push(Span::styled(title, toggle_style));
    }
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

    // The checkbox (plus its label) is the select-all click target.
    let toggle_w = (checkbox.width() + title_width) as u16;
    if toggle_w > 0 {
        hits.push(
            Rect {
                x: area.x,
                y: row.y,
                width: toggle_w.min(area.width),
                height: 1,
            },
            ClickTarget::SelectAllToggle,
        );
    }

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
    // Space-marked row (ticket p0s3): the bulk highlight fill. The cursor
    // row keeps its accent fill; both may apply to one row (cursor wins).
    bulk_selected: bool,
    // The message list holds focus: the selected row shows the accent
    // marker (the same bar the sidebar's focused folder carries). Selection
    // alone keeps the fill but never the marker, so focus stays readable.
    focused: bool,
) -> Vec<Span<'a>> {
    let full = mode == LayoutMode::Full;
    let compact = mode == LayoutMode::Compact;

    // Column anatomy: marker 1 + icon 2 (star/checkbox + trailing space,
    // ticket cvc4) + from + gap 2 + subject(+snippet) + gap 2 + date 8,
    // summing to the full row width so the date lands on the right edge
    // (mockup grid).
    let date = dates::format_relative(now, message.timestamp);
    let from_w = if compact { 14 } else { 18 };
    let subject_w = width.saturating_sub(3 + from_w + 2 + 2 + 8).max(10);

    let bg = if selected {
        theme.accent_bg
    } else if bulk_selected {
        theme.bulk_selected_bg
    } else {
        theme.background
    };
    let base = if selected {
        theme.row_selected()
    } else if bulk_selected {
        theme.row_bulk_selected()
    } else {
        Style::new().bg(theme.background)
    };

    // Icon column (ticket cvc4): a bulk-selected row shows the checkbox
    // whatever its star state; otherwise the star, or a blank. The cell is
    // always two columns — symbol + trailing space.
    let (symbol, style) = if bulk_selected {
        ("☑", theme.accent_fg().bg(bg))
    } else if message.is_starred {
        ("*", theme.star().bg(bg))
    } else {
        (" ", base)
    };
    let icon = Span::styled(format!("{symbol} "), style);
    // Accent bar in the marker column (mockup `.folder.active` bar): marks
    // the focused row while the list holds focus.
    let marker = if selected && focused {
        Span::styled("▏", Style::new().fg(theme.accent).bg(bg))
    } else {
        Span::styled(" ", base)
    };

    let from_style = if selected || bulk_selected {
        base.fg(theme.text)
    } else if message.is_read {
        theme.read_text().bg(theme.background)
    } else {
        theme.unread_text().bg(theme.background)
    };
    let subject_style = from_style;
    // The faded preview (ticket wxtx): dimmer than any subject state, so
    // the Gmail-style body preview reads as context. It keeps the row's
    // fill (accent fill included).
    let snippet_style = Style::new().fg(theme.snippet).bg(bg);

    // Subject plus (full mode only) the faded body preview, composed so
    // the combined cell fills exactly the column width: the subject stays
    // whole when it fits, the preview takes what is left and ends in `…`
    // when the body text is longer (ticket wxtx). Column anatomy keeps the
    // mockup grid: marker, icon, from, subject(+preview), date.
    let mut spans = vec![marker, icon];
    spans.push(Span::styled(
        text::fit_left(message.from_display(), from_w),
        from_style,
    ));
    spans.push(Span::styled("  ", base));
    if full {
        let sep = " — ";
        let subject_cell = text::truncate(&message.subject, subject_w);
        let subject_alone = message.subject.width() > subject_w;
        let preview_cell = if subject_alone {
            String::new()
        } else {
            message
                .snippet
                .as_ref()
                .map(|snippet| {
                    let budget = subject_w
                        .saturating_sub(message.subject.width())
                        .saturating_sub(sep.width());
                    if budget > 0 {
                        format!("{sep}{}", text::truncate(snippet, budget))
                    } else {
                        String::new()
                    }
                })
                .unwrap_or_default()
        };
        let used = subject_cell.width() + preview_cell.width();
        spans.push(Span::styled(subject_cell, subject_style));
        if !preview_cell.is_empty() {
            spans.push(Span::styled(preview_cell, snippet_style));
        }
        // Pad the cell to its exact column width so the date stays on the
        // right edge (mockup grid).
        spans.push(Span::styled(
            " ".repeat(subject_w.saturating_sub(used)),
            base,
        ));
    } else {
        spans.push(Span::styled(
            text::fit_left(&message.subject, subject_w),
            subject_style,
        ));
    }
    let date_style = if message.is_read && !selected {
        Style::new().fg(theme.dim).bg(bg)
    } else {
        from_style.bg(bg)
    };
    spans.push(Span::styled("  ", base));
    spans.push(Span::styled(text::fit_left(&date, 8), date_style));
    spans
}
