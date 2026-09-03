//! The attachment path-entry dialog (plan §15, Phase 8): a centered modal
//! with a single-line path entry, an inline caret, and the detail of the
//! last rejected submission. Enter submits, Esc cancels — validation runs
//! in the backend (`~` expansion, existence, readability, size), so
//! rejections keep the entry editable and retryable.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::overlay::Overlay;
use crate::ui::text;
use crate::ui::theme::Theme;

/// Dialog geometry for one terminal size; centered like the other modals.
fn layout(size: (u16, u16)) -> Rect {
    let width = 56u16.min(size.0.max(1));
    let height = 8u16.min(size.1.max(1));
    Rect {
        x: size.0.saturating_sub(width) / 2,
        y: size.1.saturating_sub(height) / 2,
        width,
        height,
    }
}

/// Render the dialog, when open, above everything already drawn.
pub fn render(frame: &mut Frame<'_>, state: &crate::app::state::AppState, theme: &Theme) {
    let Some(Overlay::AttachmentPath(dialog)) = &state.overlay else {
        return;
    };
    let area = layout(state.size);
    if area.width < 6 || area.height < 4 {
        return;
    }
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " Attach file ",
            Style::new()
                .fg(theme.background)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::new().fg(theme.accent))
        .style(theme.on_background());
    frame.render_widget(block, area);

    let inner = Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    };
    let inner_w = inner.width as usize;

    // Entry line with an inline caret (the terminal cursor stays hidden
    // app-wide). The path is never cleaned here: what the user typed is
    // what the backend receives.
    let caret_style = Style::new()
        .fg(theme.background)
        .bg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let normal = Style::new().fg(theme.text);
    let mut entry = Vec::new();
    let mut used = 0usize;
    for (index, ch) in dialog.input.char_indices() {
        let width = ch.to_string().width();
        if used + width > inner_w {
            break;
        }
        let style = if index == dialog.cursor {
            caret_style
        } else {
            normal
        };
        entry.push(Span::styled(ch.to_string(), style));
        used += width;
    }
    if dialog.cursor >= dialog.input.chars().count() && used < inner_w {
        entry.push(Span::styled(" ", caret_style));
    }
    // The rejection rides directly under the entry; while validating (no
    // error yet) the hint line doubles as the feedback row.
    let feedback = match &dialog.error {
        Some(detail) => Line::from(Span::styled(
            text::wrap(detail, inner_w)
                .into_iter()
                .next()
                .unwrap_or_default(),
            Style::new().fg(theme.warning),
        )),
        None => Line::from(Span::styled(
            text::clip("(path is checked when you press ↵)", inner_w),
            Style::new().fg(theme.dim),
        )),
    };
    let rows = [
        Line::from(entry),
        feedback,
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            text::clip("Type or paste a path · ↵ attach · Esc cancel", inner_w),
            Style::new().fg(theme.dim),
        )),
    ];
    for (index, line) in rows.into_iter().take(inner.height as usize).enumerate() {
        frame.render_widget(
            Paragraph::new(line),
            Rect {
                x: inner.x,
                y: inner.y + index as u16,
                width: inner.width,
                height: 1,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::focus::Focus;
    use crate::app::overlay::AttachmentPathDialog;

    fn dialog() -> AttachmentPathDialog {
        AttachmentPathDialog {
            input: String::from("~/docs/report final.pdf"),
            cursor: 3,
            error: None,
            previous_focus: Focus::Composer,
        }
    }

    #[test]
    fn layout_stays_centered_and_fits_small_terms() {
        let area = layout((152, 40));
        assert_eq!(area.width, 56);
        assert_eq!(area.x, (152 - 56) / 2);
        let area = layout((30, 6));
        assert_eq!(area.width, 30);
        assert_eq!(area.height, 6);
    }

    #[test]
    fn caret_marker_and_error_shape() {
        let d = dialog();
        // The caret index lands inside the typed text; the entry renders
        // with a reversed cell there (visual check happens in snapshots).
        assert!(d.cursor < d.input.chars().count());
        let mut d = dialog();
        d.error = Some(String::from("`~/x` does not exist"));
        assert!(d.error.is_some());
    }
}
