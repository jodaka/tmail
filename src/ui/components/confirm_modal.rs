//! The composer's confirm-discard dialog (plan §14): deleting local and
//! remote draft state happens only after explicit confirmation. `Keep` is
//! the safe default button; the draft preview shows where the deletion
//! would land.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::action::ClickTarget;
use crate::app::overlay::{ConfirmButton, Overlay};
use crate::input::mouse::HitMap;
use crate::ui::chrome;
use crate::ui::text;
use crate::ui::theme::Theme;

/// Dialog geometry for one terminal size; centered like the error modal.
/// The height budgets the borders and the one-line margins `modal_frame`
/// keeps around the six content rows.
fn layout(size: (u16, u16)) -> Rect {
    chrome::centered(size, 52, 10)
}

/// Render the dialog, when open, above everything already drawn.
pub fn render(
    frame: &mut Frame<'_>,
    state: &crate::app::state::AppState,
    theme: &Theme,
    hits: &mut HitMap,
) {
    let Some(Overlay::ConfirmDiscard(dialog)) = &state.session.overlay else {
        return;
    };
    let area = layout(state.session.size);
    if area.width < 6 || area.height < 4 {
        return;
    }
    let Some(inner) = chrome::modal_frame(
        frame,
        area,
        &Span::raw(" Discard draft? "),
        theme.error,
        theme,
    ) else {
        return;
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
    // The hint sits in the label slot one row below the content rect —
    // separated from it by the rect's last (blank) row, adjacent to the
    // bottom border.
    frame.render_widget(
        Paragraph::new(Span::styled(
            text::clip(
                "Tab switch · ↵ confirm · Esc keep draft",
                inner.width as usize,
            ),
            Style::new().fg(theme.dim),
        )),
        Rect {
            x: inner.x,
            y: inner.y + inner.height,
            width: inner.width,
            height: 1,
        },
    );
    // Click targets for the two buttons when their row is drawn (plan
    // §10/§14): Tab + Enter reaches the same states. `Keep` stays the safe
    // default — clicking outside the buttons does nothing.
    if inner.height > 4 {
        let discard_w = chrome::labeled_button_label("Discard").width() as u16;
        let keep_w = chrome::labeled_button_label("Keep editing").width() as u16;
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
/// when focused); the unfocused buttons stay quiet. Both labels go through
/// [`chrome::labeled_button`], which the click-target widths share.
fn button_spans<'a>(button: ConfirmButton, theme: &'a Theme) -> Vec<Span<'a>> {
    vec![
        chrome::labeled_button(
            "Discard",
            button == ConfirmButton::Discard,
            theme.error,
            theme,
        ),
        Span::raw(" "),
        chrome::labeled_button(
            "Keep editing",
            button == ConfirmButton::Keep,
            theme.accent,
            theme,
        ),
    ]
}
