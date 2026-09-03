//! The centered, scrollable Retry/Dismiss error modal (plan §12).
//!
//! Shows a human-readable operation summary, the Himalaya exit status, and
//! the full sanitized causal detail — scrollable when it overflows — plus
//! an ambiguity warning slot and the two buttons. The reducer scrolls and
//! switches buttons through the same layout math this module exposes, so
//! the clamp the reducer computes always matches what is drawn.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::overlay::{ErrorDialog, ModalButton, Overlay};
use crate::ui::text::wrap;
use crate::ui::theme::Theme;

/// Geometry of the modal for one terminal size and dialog shape. Shared by
/// the renderer and the reducer's scroll clamping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModalLayout {
    /// Outer bordered rectangle.
    pub area: Rect,
    /// Display width available to wrapped detail lines.
    pub detail_width: usize,
    /// Number of detail lines visible at once.
    pub viewport_lines: usize,
}

/// Compute the modal geometry. The modal always fits: it shrinks to the
/// terminal and keeps at least one detail row even in tiny terminals.
pub fn layout(size: (u16, u16), code: Option<i32>, ambiguous: bool) -> ModalLayout {
    let width = size.0.saturating_sub(10).clamp(24, 76).min(size.0.max(1));
    let height = size.1.saturating_sub(4).clamp(7, 18).min(size.1.max(1));
    let area = Rect {
        x: size.0.saturating_sub(width) / 2,
        y: size.1.saturating_sub(height) / 2,
        width,
        height,
    };
    // Inside the borders: [code?] [warning?] [detail…] [buttons] [hint].
    let inner_height = height.saturating_sub(2) as usize;
    let fixed = 2 + usize::from(code.is_some()) + usize::from(ambiguous);
    let viewport_lines = inner_height.saturating_sub(fixed).max(1);
    let detail_width = width.saturating_sub(4).max(1) as usize;
    ModalLayout {
        area,
        detail_width,
        viewport_lines,
    }
}

/// Lines of wrapped detail (width from the layout), for tests and the
/// reducer.
fn detail_lines(detail: &str, size: (u16, u16), code: Option<i32>, ambiguous: bool) -> Vec<String> {
    let layout = layout(size, code, ambiguous);
    wrap(detail, layout.detail_width)
}

/// Largest valid scroll offset for `dialog` at `size` (what the reducer
/// clamps against).
pub fn max_scroll(dialog: &ErrorDialog, size: (u16, u16)) -> usize {
    let lines = detail_lines(&dialog.detail, size, dialog.code, dialog.ambiguous);
    let layout = layout(size, dialog.code, dialog.ambiguous);
    lines.len().saturating_sub(layout.viewport_lines)
}

/// Render the modal, when open, above everything already drawn.
pub fn render(frame: &mut Frame<'_>, state: &crate::app::state::AppState, theme: &Theme) {
    let Some(Overlay::Error(dialog)) = &state.overlay else {
        return;
    };
    let area = frame.area();
    let size = (area.width, area.height);
    let layout = layout(size, dialog.code, dialog.ambiguous);
    if layout.area.width < 3 || layout.area.height < 3 {
        return;
    }
    frame.render_widget(Clear, layout.area);

    let title = dialog
        .retry
        .as_ref()
        .map(|spec| spec.kind.summary())
        .unwrap_or("Operation");
    // Plan §12: an ambiguous outcome never claims definite failure — the
    // message may already be out (e.g. a send that died mid-DATA).
    let title_text = if dialog.ambiguous {
        format!(" {title} — outcome unclear ")
    } else {
        format!(" {title} failed ")
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            title_text,
            Style::new()
                .fg(theme.background)
                .bg(theme.error)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::new().fg(theme.error))
        .style(theme.on_background());
    frame.render_widget(block, layout.area);

    // Rows are laid out top to bottom inside the border, matching `layout`:
    // [code?] [warning?] [detail viewport] [buttons] [hint].
    let inner_x = layout.area.x + 2;
    let inner_w = layout.area.width.saturating_sub(4);
    let mut y = layout.area.y + 1;
    let row = |y: u16, height: u16| Rect {
        x: inner_x,
        y,
        width: inner_w,
        height,
    };
    if let Some(code) = dialog.code {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("himalaya exited with code {code}"),
                Style::new().fg(theme.warning),
            ))),
            row(y, 1),
        );
        y += 1;
    }
    if dialog.ambiguous {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Warning: the outcome is unclear — retrying may send a duplicate.",
                Style::new().fg(theme.warning),
            ))),
            row(y, 1),
        );
        y += 1;
    }

    // Detail viewport: skip `scroll` wrapped lines, draw at most
    // `viewport_lines`, with a subtle continuation marker.
    let lines = detail_lines(&dialog.detail, size, dialog.code, dialog.ambiguous);
    let max_scroll = lines.len().saturating_sub(layout.viewport_lines);
    let scroll = dialog.scroll.min(max_scroll);
    let visible: Vec<Line<'_>> = lines
        .iter()
        .skip(scroll)
        .take(layout.viewport_lines)
        .map(|line| Line::from(Span::styled(line.clone(), Style::new().fg(theme.text_soft))))
        .collect();
    frame.render_widget(
        Paragraph::new(visible),
        row(y, layout.viewport_lines as u16),
    );
    y += layout.viewport_lines as u16;

    // Buttons: [ Retry ]  [ Dismiss ]; the focused one gets the accent fill.
    let button_style = |focused: bool| {
        if focused {
            Style::new()
                .fg(theme.background)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.muted)
        }
    };
    let buttons = Line::from(vec![
        Span::styled(
            "[ Retry ]",
            button_style(dialog.button == ModalButton::Retry && dialog.retry.is_some()),
        ),
        Span::raw("   "),
        Span::styled(
            "[ Dismiss ]",
            button_style(dialog.button == ModalButton::Dismiss),
        ),
    ]);
    frame.render_widget(Paragraph::new(buttons), row(y, 1));
    y += 1;

    let hint = "↑↓ scroll · Tab switch · ↵ confirm · Esc dismiss";
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            crate::ui::text::clip(hint, inner_w as usize),
            Style::new().fg(theme.dim),
        ))),
        row(y, 1),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog(detail: &str) -> ErrorDialog {
        ErrorDialog {
            code: Some(1),
            detail: String::from(detail),
            retry: None,
            ambiguous: false,
            scroll: 0,
            button: ModalButton::Dismiss,
            previous_focus: crate::app::focus::Focus::MessageList,
        }
    }

    #[test]
    fn modal_fits_reference_size() {
        let geom = layout((152, 40), Some(1), false);
        assert_eq!(geom.area.width, 76);
        // Height caps at 18 rows; fixed rows: code + detail + buttons + hint.
        assert_eq!(geom.area.height, 18);
        assert_eq!(geom.viewport_lines, 18 - 2 - 3);
        assert_eq!(geom.detail_width, 72);
        // Centered.
        assert_eq!(geom.area.x, (152 - 76) / 2);
    }

    #[test]
    fn modal_shrinks_for_small_terminals_without_panicking() {
        let geom = layout((0, 0), None, false);
        assert_eq!(geom.area.width.min(geom.area.height), 1);
        assert_eq!(geom.viewport_lines, 1);
        let geom = layout((40, 10), Some(1), true);
        // Height clamps to 7 → inner 5; minus code/warning/buttons/hint.
        assert_eq!(geom.viewport_lines, 1);
    }

    #[test]
    fn max_scroll_is_zero_when_detail_fits() {
        assert_eq!(max_scroll(&dialog("short"), (152, 40),), 0);
    }

    #[test]
    fn max_scroll_counts_wrapped_lines() {
        let detail = "word ".repeat(200);
        let over = max_scroll(&dialog(detail.trim_end()), (152, 40));
        assert!(over > 0);
    }
}
