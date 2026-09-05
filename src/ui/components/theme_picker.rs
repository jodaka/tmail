//! The theme picker dialog (ticket k5ba): a small centered list of every
//! available palette. The highlighted row is already applied — the dialog
//! renders with the palette it is previewing, so navigating recolors the
//! whole screen live. Enter keeps the preview; Esc restores the palette
//! the picker opened with (that logic lives in the reducer). A list longer
//! than the dialog scrolls, with a scrollbar like the message list's.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::overlay::Overlay;
use crate::ui::text;
use crate::ui::theme::Theme;

/// Rows the list shows before it starts scrolling.
const MAX_VISIBLE_ROWS: usize = 10;

/// Geometry of the picker for one terminal size and theme count. Shared by
/// the renderer and the reducer's cursor/scroll clamping, so the clamp the
/// reducer computes always matches what is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickerLayout {
    /// Outer bordered rectangle.
    pub area: Rect,
    /// Number of theme rows visible at once.
    pub visible_rows: usize,
}

/// Compute the picker geometry. The dialog always fits: it shrinks to the
/// terminal and keeps at least one theme row even in tiny terminals.
pub fn layout(size: (u16, u16), theme_count: usize) -> PickerLayout {
    let width = 34u16.min(size.0.max(1));
    // Inside the borders: the theme rows plus one hint line.
    let height = ((theme_count.min(MAX_VISIBLE_ROWS) as u16) + 3).min(size.1.max(1));
    let visible_rows = height.saturating_sub(3).max(1) as usize;
    PickerLayout {
        area: Rect {
            x: size.0.saturating_sub(width) / 2,
            y: size.1.saturating_sub(height) / 2,
            width,
            height,
        },
        visible_rows,
    }
}

/// Rows visible in the dialog at `size` (what the reducer keeps the cursor
/// and scroll inside), independent of the theme count cap.
pub fn visible_rows(size: (u16, u16)) -> usize {
    layout(size, usize::MAX).visible_rows
}

/// Largest valid scroll offset for the theme list at `size` (what the
/// reducer clamps against).
pub fn max_scroll(theme_count: usize, size: (u16, u16)) -> usize {
    theme_count.saturating_sub(visible_rows(size))
}

/// Render the picker, when open, above everything already drawn.
pub fn render(frame: &mut Frame<'_>, state: &crate::app::state::AppState, theme: &Theme) {
    let Some(Overlay::ThemePicker(dialog)) = &state.overlay else {
        return;
    };
    let layout = layout(state.size, state.themes.len());
    if layout.area.width < 6 || layout.area.height < 3 {
        return;
    }
    frame.render_widget(Clear, layout.area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " Theme ",
            Style::new()
                .fg(theme.background)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::new().fg(theme.accent))
        .style(theme.on_background());
    frame.render_widget(block, layout.area);

    let inner = Rect {
        x: layout.area.x + 2,
        y: layout.area.y + 1,
        width: layout.area.width.saturating_sub(4),
        height: layout.area.height.saturating_sub(2),
    };
    // The hint occupies the last inner row; theme rows fill the rest,
    // windowed by the reducer-maintained scroll offset. When the theme
    // list outgrows the window, a scrollbar takes the last inner column —
    // the same anatomy (and look) as the message list's scrollbar — so
    // row text and fills end one column short of it.
    let rows_height = inner.height.saturating_sub(1);
    let visible = rows_height as usize;
    let scrolling = state.themes.len() > visible;
    let row_width = if scrolling {
        inner.width.saturating_sub(1)
    } else {
        inner.width
    };
    for (drawn, (index, (name, _))) in state
        .themes
        .iter()
        .enumerate()
        .skip(dialog.scroll)
        .take(visible)
        .enumerate()
    {
        let selected = index == dialog.cursor;
        // The cursor row carries the same accent bar + fill the message
        // list's focused row uses, so "the highlighted entry" reads the
        // same way in both lists.
        let (marker, row_style) = if selected {
            (
                "▏",
                Style::new()
                    .fg(theme.accent)
                    .bg(theme.accent_bg)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            (" ", Style::new().fg(theme.text_soft).bg(theme.background))
        };
        let row = Rect {
            x: inner.x,
            y: inner.y + drawn as u16,
            width: row_width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Span::styled(row_text(marker, name, row.width), row_style)),
            row,
        );
    }
    if scrolling {
        // The thumb tracks the scroll window the reducer keeps centered on
        // the cursor (position = first visible row, like the message list).
        let mut scrollbar_state = ScrollbarState::new(state.themes.len()).position(dialog.scroll);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .track_style(Style::new().fg(theme.border).bg(theme.background))
                .thumb_style(Style::new().fg(theme.dim).bg(theme.background)),
            Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: rows_height,
            },
            &mut scrollbar_state,
        );
    }
    let hint = Rect {
        x: inner.x,
        y: inner.y + rows_height,
        width: inner.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            text::clip("↑/↓ preview · ↵ confirm · Esc cancel", inner.width as usize),
            Style::new().fg(theme.dim),
        )),
        hint,
    );
}

/// One list row: marker, theme name, padded with the row fill to the full
/// dialog width so the highlight reads as a bar, not a halo.
fn row_text(marker: &str, name: &str, width: u16) -> String {
    let mut label = format!("{marker} {name}");
    let label_width = label.width() as u16;
    if label_width < width {
        label.push_str(&" ".repeat((width - label_width) as usize));
    } else {
        label = text::clip(&label, width as usize);
    }
    label
}
