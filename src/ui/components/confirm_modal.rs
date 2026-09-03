//! The composer's confirm-discard dialog (plan §14): deleting local and
//! remote draft state happens only after explicit confirmation. `Keep` is
//! the safe default button; the draft preview shows where the deletion
//! would land.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::action::ClickTarget;
use crate::app::overlay::{ConfirmButton, Overlay};
use crate::input::mouse::HitMap;
use crate::ui::text;
use crate::ui::theme::Theme;

/// Dialog geometry for one terminal size; centered like the error modal.
fn layout(size: (u16, u16)) -> Rect {
    let width = 52u16.min(size.0.max(1));
    let height = 8u16.min(size.1.max(1));
    Rect {
        x: size.0.saturating_sub(width) / 2,
        y: size.1.saturating_sub(height) / 2,
        width,
        height,
    }
}

/// Render the dialog, when open, above everything already drawn.
pub fn render(
    frame: &mut Frame<'_>,
    state: &crate::app::state::AppState,
    theme: &Theme,
    hits: &mut HitMap,
) {
    let Some(Overlay::ConfirmDiscard(dialog)) = &state.overlay else {
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
            " Discard draft? ",
            Style::new()
                .fg(theme.background)
                .bg(theme.error)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::new().fg(theme.error))
        .style(theme.on_background());
    frame.render_widget(block, area);

    let inner = Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    };
    let subject = match dialog.draft.subject.trim() {
        "" => String::from("(no subject)"),
        subject => text::clip(subject, inner.width as usize),
    };
    let body_lines = [
        Line::from(Span::styled(subject, Style::new().fg(theme.text))),
        Line::from(Span::styled(
            "The draft, its journal entry, and its saved copy",
            Style::new().fg(theme.text_soft),
        )),
        Line::from(Span::styled(
            "will be deleted permanently.",
            Style::new().fg(theme.text_soft),
        )),
        Line::from(Span::raw("")),
        Line::from(button_spans(dialog.button, theme)),
        Line::from(Span::styled(
            text::clip(
                "Tab switch · ↵ confirm · Esc keep draft",
                inner.width as usize,
            ),
            Style::new().fg(theme.dim),
        )),
    ];
    for (index, line) in body_lines
        .into_iter()
        .take(inner.height as usize)
        .enumerate()
    {
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
    // Click targets for the two buttons when their row is drawn (plan
    // §10/§14): Tab + Enter reaches the same states. `Keep` stays the safe
    // default — clicking outside the buttons does nothing.
    if inner.height > 4 {
        let discard_w = " [ Discard ] ".width() as u16;
        let keep_w = " [ Keep editing ] ".width() as u16;
        hits.push(
            Rect {
                x: inner.x,
                y: inner.y + 4,
                width: discard_w,
                height: 1,
            },
            ClickTarget::ConfirmButton(ConfirmButton::Discard),
        );
        hits.push(
            Rect {
                x: inner.x + discard_w + 1,
                y: inner.y + 4,
                width: keep_w,
                height: 1,
            },
            ClickTarget::ConfirmButton(ConfirmButton::Keep),
        );
    }
}

/// [ Discard ] (warning fill when focused) / [ Keep editing ] (accent fill
/// when focused); the unfocused buttons stay quiet.
fn button_spans<'a>(button: ConfirmButton, theme: &'a Theme) -> Vec<Span<'a>> {
    let discard = if button == ConfirmButton::Discard {
        Span::styled(
            " [ Discard ] ",
            Style::new()
                .fg(theme.background)
                .bg(theme.error)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(" [ Discard ] ", Style::new().fg(theme.muted))
    };
    let keep = if button == ConfirmButton::Keep {
        Span::styled(
            " [ Keep editing ] ",
            Style::new()
                .fg(theme.background)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(" [ Keep editing ] ", Style::new().fg(theme.muted))
    };
    vec![discard, Span::raw(" "), keep]
}
