//! Shared chrome the screens and components draw the same way: centered
//! dialog geometry, hairline rules, scrollbar construction, modal frames,
//! and dim one-line notes. One implementation per look, so a palette or
//! anatomy change lands everywhere at once.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation};

use crate::ui::text;
use crate::ui::theme::Theme;

/// One side of a hairline rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HairlineSide {
    /// Rule on the bottom edge of `area`.
    Bottom,
    /// Rule on the top edge of `area`.
    Top,
}

/// A horizontally centered dialog rectangle: the shared anatomy of the
/// modal `layout()` functions (`x = (size.0 - width) / 2`, and so on).
/// The rectangle never exceeds the terminal: oversized dimensions clamp to
/// the terminal first, so callers may pass raw caps.
pub use crate::view::layout::centered;

/// A hairline rule on one edge of `area` (mockup `border-bottom: 1px solid
/// var(--border)`): one-color border block, the only difference between
/// the five drawn sites being the edge and rectangle.
pub fn hairline(frame: &mut Frame<'_>, area: Rect, side: HairlineSide, theme: &Theme) {
    let borders = match side {
        HairlineSide::Bottom => Borders::BOTTOM,
        HairlineSide::Top => Borders::TOP,
    };
    frame.render_widget(
        Block::default()
            .borders(borders)
            .border_style(theme.hairline()),
        area,
    );
}

/// The dimensionless part of the shared scrollbar look (full-height
/// vertical right rail, no end symbols, `│` track); styles ride the active
/// palette. Callers keep their stateful position math.
pub fn scrollbar(theme: &Theme) -> Scrollbar<'_> {
    Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .track_style(Style::new().fg(theme.border).bg(theme.background))
        .thumb_style(Style::new().fg(theme.dim).bg(theme.background))
}

/// The shared modal scaffold: clear the area, draw the bordered block with
/// an inverse-filled title on `fill`, and return the inner content rect.
/// One anatomy for all four dialogs; `Some` means the area was large
/// enough to open (callers keep their own bigger minimums).
pub fn modal_frame(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &Span<'_>,
    fill: ratatui::style::Color,
    theme: &Theme,
) -> Option<Rect> {
    if area.width < 4 || area.height < 3 {
        return None;
    }
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            title.content.clone(),
            Style::new()
                .fg(theme.background)
                .bg(fill)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::new().fg(fill))
        .style(theme.on_background());
    frame.render_widget(block, area);
    Some(Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    })
}

/// A dim one-line note clipped to `area`'s width (empty states in panes).
pub fn render_note(frame: &mut Frame<'_>, area: Rect, theme: &Theme, note: &str) {
    if area.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Span::styled(
            text::clip(note, area.width as usize),
            Style::new().fg(theme.dim),
        )),
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(elems: (u16, u16, u16, u16)) -> Rect {
        Rect {
            x: elems.0,
            y: elems.1,
            width: elems.2,
            height: elems.3,
        }
    }

    #[test]
    fn centered_dialog_geometry() {
        assert_eq!(centered((152, 40), 76, 18), rect((38, 11, 76, 18)));
        // Clamped inside tiny terminals.
        assert_eq!(centered((40, 10), 96, 30), rect((0, 0, 40, 10)));
        // Degenerate terminals still keep a 1x1 rect (u16 floor).
        assert_eq!(centered((0, 0), 52, 8), rect((0, 0, 1, 1)));
    }
}
