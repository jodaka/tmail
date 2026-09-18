//! The attachment file chooser (plan §15, ticket 95x0): a centered modal
//! hosting a `ratatui_explorer` listing of the working directory. The
//! explorer widget draws its own list — themed here from the app's
//! palette — while the dialog adds the chrome, the current-directory
//! line, and the status row: a listing in flight, the detail of a failed
//! listing or validation, or the submit hint.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{FrameExt, Paragraph};

use crate::app::overlay::Overlay;
use crate::ui::chrome::{self, centered};
use crate::ui::text;
use crate::ui::theme::Theme;

/// Dialog geometry for one terminal size: generous — a proper chooser —
/// but always inside the terminal with a margin. The width half is shared
/// with `view::overlay` (the error viewport's wrap width, ticket 6t30).
fn layout(size: (u16, u16)) -> Rect {
    let width = crate::view::overlay::attachment_dialog_width(size);
    let height = (size.1 * 3 / 4).clamp(12, 30).min(size.1.max(1));
    centered(size, width, height)
}

/// Render the dialog, when open, above everything already drawn.
pub fn render(frame: &mut Frame<'_>, state: &crate::app::state::AppState, theme: &Theme) {
    let Some(Overlay::AttachmentExplorer(dialog)) = &state.session.overlay else {
        return;
    };
    let area = layout(state.session.size);
    if area.width < 8 || area.height < 5 {
        return;
    }
    let Some(inner) = chrome::modal_frame(
        frame,
        area,
        &Span::raw(" Attach file "),
        theme.accent,
        theme,
    ) else {
        return;
    };
    let inner_w = inner.width as usize;

    // The directory being listed, clipped to one line.
    let cwd = dialog
        .explorer
        .as_ref()
        .map(|explorer| explorer.cwd().display().to_string())
        .unwrap_or_else(|| String::from("…"));
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text::clip(&cwd, inner_w),
            Style::new().fg(theme.dim),
        ))),
        Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        },
    );

    // The listing itself, between the directory line and the bottom rows:
    // one line for cwd, then the status strip — one row for the
    // listing/hint, the error's wrapped viewport when one is showing.
    let status_rows = if dialog.error.is_some() {
        crate::view::overlay::attachment_error_viewport(state.session.size) as u16
    } else {
        1
    };
    let list = Rect {
        x: inner.x,
        y: inner.y + 1,
        width: inner.width,
        height: inner.height.saturating_sub(2 + status_rows).max(1),
    };
    if let Some(explorer) = dialog.explorer.as_ref() {
        frame.render_widget_ref(explorer.widget(), list);
    } else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                text::clip("Listing…", list.width as usize),
                Style::new().fg(theme.dim),
            ))),
            list,
        );
    }

    // Status strip: the last failure, the in-flight listing, or the hint
    // that Enter attaches the selected file. An error grows the strip
    // into a scrollable viewport (ticket 6t30: a wrapped detail used to
    // lose every line after the first) — the clamp is the shared math,
    // the same offset the reducer moves.
    let status_row = inner.height.saturating_sub(1 + status_rows);
    if let Some(detail) = &dialog.error {
        let lines = crate::view::overlay::attachment_error_lines(detail, state.session.size);
        let scroll = dialog
            .error_scroll
            .min(crate::view::overlay::attachment_error_max_scroll(
                detail,
                state.session.size,
            ));
        let visible: Vec<Line<'_>> = lines
            .into_iter()
            .skip(scroll)
            .take(status_rows as usize)
            .map(|line| Line::from(Span::styled(line, Style::new().fg(theme.warning))))
            .collect();
        frame.render_widget(
            Paragraph::new(visible),
            Rect {
                x: inner.x,
                y: inner.y + status_row,
                width: inner.width,
                height: status_rows,
            },
        );
    } else {
        let (status, style) = match dialog.listing {
            true => (String::from("Listing…"), Style::new().fg(theme.dim)),
            false => (
                String::from("(↵ attaches the selected file)"),
                Style::new().fg(theme.dim),
            ),
        };
        render_status_line(frame, inner, inner_w, status_row as usize, status, style);
    }

    // The hint sits in the label slot one row below the content rect —
    // separated from it by the rect's last (blank) row, adjacent to the
    // bottom border.
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text::clip("↑↓ select · ← up · → open · ↵ attach · Esc cancel", inner_w),
            Style::new().fg(theme.dim),
        ))),
        Rect {
            x: inner.x,
            y: inner.y + inner.height,
            width: inner.width,
            height: 1,
        },
    );
}

fn render_status_line(
    frame: &mut Frame<'_>,
    inner: Rect,
    inner_w: usize,
    row: usize,
    content: String,
    style: Style,
) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text::clip(&content, inner_w),
            style,
        ))),
        Rect {
            x: inner.x,
            y: inner.y + row as u16,
            width: inner.width,
            height: 1,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_generous_and_centered() {
        let area = layout((152, 40));
        assert_eq!(area.width, 96);
        assert_eq!(area.height, 30);
        assert_eq!(area.x, (152 - 96) / 2);
        assert_eq!(area.y, (40 - 30) / 2);
        // Small terminals shrink the dialog instead of overlapping.
        let area = layout((40, 10));
        assert_eq!(area.width, 40);
        assert_eq!(area.height, 10);
    }

    #[test]
    fn explorer_theme_uses_the_palette_tokens() {
        let theme = Theme::default_dark();
        let explorer = theme.explorer_theme();
        assert_eq!(explorer.item_style().fg, Some(theme.text));
        assert_eq!(explorer.highlight_item_style().bg, Some(theme.accent_bg));
        assert_eq!(explorer.dir_style().fg, Some(theme.text_soft));
        assert!(explorer.block().is_none(), "the dialog owns the chrome");
    }
}
