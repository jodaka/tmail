//! Loaders (ticket m3by): one spinner everywhere. Pane loaders (mailboxes,
//! message list, message body) render the braille frame centered in the
//! pane; the top bar renders it in place of the program name/version while
//! work is in flight. Animated from the reducer's tick counter, so
//! snapshots and tests stay deterministic (plan §20: no wall-clock
//! dependence).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use crate::ui::theme::Theme;

/// Braille dot frames: ordinary Unicode, no Nerd Font dependency (plan §18).
const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The spinner frame for the given tick counter.
pub fn frame(ticks: u64) -> &'static str {
    FRAMES[(ticks % FRAMES.len() as u64) as usize]
}

/// Render the spinner centered in `area` (ticket m3by: every pane loader
/// looks the same — one accent glyph in the middle of the panel).
pub fn render_centered(frame: &mut Frame<'_>, area: Rect, theme: &Theme, ticks: u64) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let cell = Rect {
        x: area.x + area.width.saturating_sub(1) / 2,
        y: area.y + area.height.saturating_sub(1) / 2,
        width: 1,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            // Fully qualified: the `frame` parameter shadows the frame
            // function inside this scope.
            self::frame(ticks),
            Style::new().fg(theme.accent).bg(theme.background),
        )),
        cell,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_cycle_deterministically() {
        assert_eq!(frame(0), "⠋");
        assert_eq!(frame(1), "⠙");
        assert_eq!(frame(10), frame(0), "wraps at the frame count");
        assert_eq!(frame(23), frame(3));
    }
}
