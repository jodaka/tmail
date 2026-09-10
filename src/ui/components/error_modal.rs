//! The centered, scrollable Retry/Dismiss error modal (plan §12).
//!
//! Shows a human-readable operation summary, the Himalaya exit status, and
//! the full sanitized causal detail — scrollable when it overflows — plus
//! an ambiguity warning slot and the two buttons. The reducer scrolls and
//! switches buttons through the same layout math this module exposes, so
//! the clamp the reducer computes always matches what is drawn.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::action::ClickTarget;
use crate::app::overlay::{ErrorDialog, ModalButton, Overlay};
use crate::input::mouse::HitMap;
use crate::ui::chrome;
use crate::ui::theme::Theme;

pub use crate::view::overlay::error_detail_lines as detail_lines;
pub use crate::view::overlay::error_modal_layout as layout;

/// Largest valid scroll offset for `dialog` at `size` (what the reducer
/// clamps against); the shared geometry lives in `view::overlay`.
pub fn max_scroll(dialog: &ErrorDialog, size: (u16, u16)) -> usize {
    crate::view::overlay::error_modal_max_scroll(
        &dialog.detail,
        dialog.code,
        dialog.ambiguous,
        size,
    )
}

/// Render the modal, when open, above everything already drawn.
pub fn render(
    frame: &mut Frame<'_>,
    state: &crate::app::state::AppState,
    theme: &Theme,
    hits: &mut HitMap,
) {
    let Some(Overlay::Error(dialog)) = &state.overlay else {
        return;
    };
    let area = frame.area();
    let size = (area.width, area.height);
    let layout = layout(size, dialog.code, dialog.ambiguous);
    if layout.area.width < 3 || layout.area.height < 3 {
        return;
    }
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
    let Some(area) = chrome::modal_frame(
        frame,
        layout.area,
        &Span::raw(title_text),
        theme.error,
        theme,
    ) else {
        return;
    };

    // Rows are laid out top to bottom inside the border, matching `layout`:
    // [code?] [warning?] [detail viewport] [buttons] [hint].
    let inner_x = area.x;
    let inner_w = area.width;
    let mut y = area.y;
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
    let button_style = |focused: bool| theme.button_style(focused, theme.accent);
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
    // Click targets for the two buttons (plan §10): Tab + Enter reaches
    // the same states. The click path ignores Retry when the failure
    // carries no retry intent (the dimmed button).
    hits.push(
        Rect {
            x: inner_x,
            y,
            width: "[ Retry ]".width() as u16,
            height: 1,
        },
        ClickTarget::ErrorButton(ModalButton::Retry),
    );
    hits.push(
        Rect {
            x: inner_x + ("[ Retry ]".width() + 3) as u16,
            y,
            width: "[ Dismiss ]".width() as u16,
            height: 1,
        },
        ClickTarget::ErrorButton(ModalButton::Dismiss),
    );
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
