//! The shortcuts help popup (user request): a centered modal listing the
//! active bindings for the screen underneath — the keymap's translation
//! tables rendered as data on the user's terminal, one row per action
//! with all of its keys joined (`Trash   d, Del, ⌫`).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::state::AppState;
use crate::ui::chrome;
use crate::ui::text;
use crate::ui::theme::Theme;

/// Render the popup, when open, above everything already drawn.
pub fn render(frame: &mut Frame<'_>, state: &AppState, theme: &Theme) {
    let Some(crate::app::overlay::Overlay::Help(dialog)) = &state.session.overlay else {
        return;
    };
    // The screen underneath decides what is relevant, not the popup's own
    // focus slot.
    let entries = state.settings.keymap.help_entries(dialog.previous_focus);
    // Viewport from the shared layout math: the reducer's scroll clamp
    // and the drawn window cannot disagree (ticket 6t30).
    let layout = crate::view::overlay::help_layout(state.session.size, entries.len());
    let scroll = dialog.scroll.min(crate::view::overlay::help_max_scroll(
        entries.len(),
        state.session.size,
    ));
    let Some(inner) = chrome::modal_frame(
        frame,
        layout.area,
        &Span::raw(" Shortcuts "),
        theme.accent,
        theme,
    ) else {
        return;
    };
    let inner_w = inner.width as usize;
    // Entries fill the body; the hint occupies the last inner row.
    for (index, (label, key)) in entries
        .iter()
        .skip(scroll)
        .take(layout.visible_rows)
        .enumerate()
    {
        // Label left, key right in a separated column (mockup
        // `.help-row`): the wide gap keeps the table readable without
        // drawn rules.
        let track = inner_w
            .saturating_sub(2 + key.width())
            .max(label.width() + 1);
        let clipped = text::clip(label, track);
        let row = Rect {
            x: inner.x,
            y: inner.y + index as u16,
            width: inner.width,
            height: 1,
        };
        let spans = vec![
            Span::styled(clipped.clone(), Style::new().fg(theme.text_soft)),
            Span::raw(" ".repeat(track.saturating_sub(clipped.width()))),
            Span::styled(
                format!("{key} "),
                Style::new()
                    .fg(theme.accent)
                    .bg(theme.background)
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        frame.render_widget(Paragraph::new(Line::from(spans)), row);
    }
    // The hint sits in the label slot one row below the content rect —
    // separated from the entries by the rect's last (blank) row, adjacent
    // to the bottom border.
    let hint = Rect {
        x: inner.x,
        y: inner.y + inner.height,
        width: inner.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            // Short on purpose: the popup clips a long hint (the config
            // pointer does not fit at the dialog's width).
            "↑↓ scroll · Esc or ? closes",
            Style::new().fg(theme.dim),
        )),
        hint,
    );
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    #[test]
    fn dialog_geometry_fits_and_clamps() {
        // The 27-entry table fits the reference terminal (27 rows + hint +
        // borders + margins).
        assert_eq!(layout_area((152, 40), 27).height, 32);
        // Tiny terminals clamp instead of overlapping.
        assert_eq!(layout_area((40, 10), 27).height, 10);
        assert_eq!(layout_area((0, 0), 0).height, 1);
    }

    #[test]
    fn overflow_scrolls_instead_of_silent_clipping() {
        // The reference terminal shows the whole table: scroll stays a
        // no-op budget (27 visible, nothing to scroll).
        assert_eq!(
            crate::view::overlay::help_max_scroll(27, (152, 40)),
            0,
            "the table fits without scrolling"
        );
        // A short terminal clamps the dialog; the overflow scrolls.
        assert_eq!(crate::view::overlay::help_visible_rows((152, 20)), 15);
        assert_eq!(
            crate::view::overlay::help_max_scroll(27, (152, 20)),
            12,
            "the last window shows the tail"
        );
    }

    /// The dialog's outer rectangle for one size and entry count (the
    /// former private `layout`, now shared with the reducer in
    /// `view::overlay`).
    fn layout_area(size: (u16, u16), entries: usize) -> Rect {
        crate::view::overlay::help_layout(size, entries).area
    }
}
