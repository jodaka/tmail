//! The theme picker dialog (ticket k5ba): a small centered list of every
//! available palette. The highlighted row is already applied — the dialog
//! renders with the palette it is previewing, so navigating recolors the
//! whole screen live. Enter keeps the preview; Esc restores the palette
//! the picker opened with (that logic lives in the reducer). A list longer
//! than the dialog scrolls, with a scrollbar like the message list's.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::overlay::Overlay;
use crate::ui::chrome;
use crate::ui::text;
use crate::ui::theme::Theme;

pub use crate::view::overlay::picker_layout as layout;
pub use crate::view::overlay::picker_max_scroll as max_scroll;
pub use crate::view::overlay::picker_visible_rows as visible_rows;

/// Render the picker, when open, above everything already drawn.
pub fn render(frame: &mut Frame<'_>, state: &crate::app::state::AppState, theme: &Theme) {
    let Some(Overlay::ThemePicker(dialog)) = &state.session.overlay else {
        return;
    };
    let layout = layout(state.session.size, state.settings.themes.len());
    if layout.area.width < 6 || layout.area.height < 3 {
        return;
    }
    let Some(inner) = chrome::modal_frame(
        frame,
        layout.area,
        &Span::raw(" Theme "),
        theme.accent,
        theme,
    ) else {
        return;
    };
    // The hint occupies the last inner row; theme rows fill the rest,
    // windowed by the reducer-maintained scroll offset. When the theme
    // list outgrows the window, a scrollbar takes the last inner column —
    // the same anatomy (and look) as the message list's scrollbar — so
    // row text and fills end one column short of it. The visible-row
    // count comes from the shared `picker_layout`, so the drawn window
    // and the reducer's clamp cannot drift apart.
    let rows_height = layout.visible_rows as u16;
    let visible = rows_height as usize;
    let scrolling = state.settings.themes.len() > visible;
    let row_width =
        chrome::scrollbar_content_width(inner.width, state.settings.themes.len(), visible);
    for (drawn, (index, (name, _))) in state
        .settings
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
                "▎",
                Style::new()
                    .fg(theme.marker_bar)
                    .bg(theme.marker)
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
        chrome::render_scrollbar(
            frame,
            Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: rows_height,
            },
            theme,
            state.settings.themes.len(),
            dialog.scroll,
        );
    }
    // The hint sits in the label slot one row below the rows — separated
    // from them by the content rect's last (blank) row, adjacent to the
    // bottom border.
    let hint = Rect {
        x: inner.x,
        y: inner.y + rows_height + 1,
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
