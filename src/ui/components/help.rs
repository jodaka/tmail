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

/// Geometry of the dialog for one terminal size and entry count: wide
/// enough for "Action label — key" rows, enough rows for the entries plus
/// the hint, clamped inside the terminal.
fn layout(size: (u16, u16), rows: usize) -> Rect {
    let width = 46u16.min(size.0.max(1));
    let height = rows as u16 + 3; // entry rows + borders + the hint row
    chrome::centered(size, width, height)
}

/// Render the popup, when open, above everything already drawn.
pub fn render(frame: &mut Frame<'_>, state: &AppState, theme: &Theme) {
    let Some(crate::app::overlay::Overlay::Help(dialog)) = &state.session.overlay else {
        return;
    };
    // The screen underneath decides what is relevant, not the popup's own
    // focus slot.
    let entries = state.settings.keymap.help_entries(dialog.previous_focus);
    let area = layout(state.session.size, entries.len());
    let Some(inner) =
        chrome::modal_frame(frame, area, &Span::raw(" Shortcuts "), theme.accent, theme)
    else {
        return;
    };
    let inner_w = inner.width as usize;
    // Entries fill the body; the hint occupies the last inner row.
    for (index, (label, key)) in entries
        .iter()
        .take(inner.height.saturating_sub(1) as usize)
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
    let hint = Rect {
        x: inner.x,
        y: inner.y + inner.height.saturating_sub(1),
        width: inner.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            text::clip(
                "Esc or ? closes · customizable via [tmail.keybindings]",
                inner_w,
            ),
            Style::new().fg(theme.dim),
        )),
        hint,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_geometry_fits_and_clamps() {
        // The 27-entry table fits the reference terminal.
        assert_eq!(layout((152, 40), 27).height, 30);
        // Tiny terminals clamp instead of overlapping.
        assert_eq!(layout((40, 10), 27).height, 10);
        assert_eq!(layout((0, 0), 0).height, 1);
    }
}
