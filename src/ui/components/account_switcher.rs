//! The runtime account switcher (ticket c0n0): a small centered list of
//! every `[accounts.<name>]` in the config file, and its confirmation
//! dialog. The highlighted row is the cursor only — nothing applies until
//! Enter — so unlike the theme picker the screen keeps its palette. Enter
//! on another account either switches at once (nothing in flight, clean
//! composer) or opens the confirm dialog first; Esc closes.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::action::ClickTarget;
use crate::app::overlay::{ConfirmButton, Overlay};
use crate::input::mouse::HitMap;
use crate::ui::chrome;
use crate::ui::text;
use crate::ui::theme::Theme;

pub use crate::view::overlay::switch_confirm_layout as confirm_layout;
pub use crate::view::overlay::switcher_layout as layout;
pub use crate::view::overlay::switcher_max_scroll as max_scroll;
pub use crate::view::overlay::switcher_visible_rows as visible_rows;

/// Render the switcher, when open, above everything already drawn.
pub fn render(frame: &mut Frame<'_>, state: &crate::app::state::AppState, theme: &Theme) {
    let Some(Overlay::AccountSwitcher(dialog)) = &state.session.overlay else {
        return;
    };
    let layout = layout(state.session.size, state.settings.accounts.len());
    if layout.area.width < 6 || layout.area.height < 3 {
        return;
    }
    let Some(inner) = chrome::modal_frame(
        frame,
        layout.area,
        &Span::raw(" Switch account "),
        theme.accent,
        theme,
    ) else {
        return;
    };
    // The hint occupies the last inner row; account rows fill the rest,
    // windowed by the reducer-maintained scroll offset — the same
    // anatomy (and scrollbar) as the theme picker's, from the shared
    // `switcher_layout`.
    let rows_height = layout.visible_rows as u16;
    let visible = rows_height as usize;
    let scrolling = state.settings.accounts.len() > visible;
    let row_width =
        chrome::scrollbar_content_width(inner.width, state.settings.accounts.len(), visible);
    for (drawn, (index, account)) in state
        .settings
        .accounts
        .iter()
        .enumerate()
        .skip(dialog.scroll)
        .take(visible)
        .enumerate()
    {
        let selected = index == dialog.cursor;
        let current = Some(account.name.as_str()) == state.settings.account_name.as_deref();
        // The cursor row carries the same accent bar + fill the message
        // list's focused row uses (the theme picker's look); dim extras
        // (display name, the current marker) sit on the same fill.
        let (marker, row_style) = if selected {
            (
                "▎",
                Style::new()
                    .fg(theme.marker_bar)
                    .bg(theme.marker)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            (" ", Style::new().fg(theme.text_soft).bg(theme.background))
        };
        let fill_bg = if selected {
            theme.marker
        } else {
            theme.background
        };
        let prefix = format!("{marker} {}", account.label());
        let mut suffix = String::new();
        // The label is the email (the unique identity — display names are
        // commonly shared between accounts, user request); a configured
        // display name shows dimly behind it so the identity stays
        // recognizable. The account the session drives is marked, so
        // Enter on it reads as "close, nothing to do".
        if let Some(display_name) = account.display_name.as_deref()
            && display_name != account.label()
        {
            suffix.push_str(&format!(" {display_name}"));
        }
        if current {
            suffix.push_str(" · current");
        }
        let prefix_width = prefix.width() as u16;
        let suffix_width = suffix.width() as u16;
        let spans: Vec<Span<'_>> = if prefix_width + suffix_width <= row_width {
            let mut spans = vec![Span::styled(prefix, row_style)];
            if suffix_width > 0 {
                spans.push(Span::styled(suffix, Style::new().fg(theme.dim).bg(fill_bg)));
            }
            if prefix_width + suffix_width < row_width {
                spans.push(Span::styled(
                    " ".repeat((row_width - prefix_width - suffix_width) as usize),
                    row_style,
                ));
            }
            spans
        } else {
            // The plain label alone does not fit either: clip it hard and
            // drop the suffixes.
            vec![Span::styled(
                text::clip(&prefix, row_width as usize),
                row_style,
            )]
        };
        let row = Rect {
            x: inner.x,
            y: inner.y + drawn as u16,
            width: row_width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(Line::from(spans)), row);
    }
    if scrolling {
        // The thumb tracks the scroll window the reducer keeps centered
        // on the cursor (position = first visible row, like the theme
        // picker).
        chrome::render_scrollbar(
            frame,
            Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: rows_height,
            },
            theme,
            state.settings.accounts.len(),
            dialog.scroll,
        );
    }
    // The hint sits in the label slot one row below the rows — separated
    // from them by the content rect's last (blank) row, adjacent to the
    // bottom border.
    let hint = Rect {
        x: inner.x,
        y: inner.y + rows_height + 1,
        width: inner.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            text::clip("↑/↓ choose · ↵ switch · Esc cancel", inner.width as usize),
            Style::new().fg(theme.dim),
        )),
        hint,
    );
}

/// Render the switch confirmation dialog, when open, above everything
/// already drawn. Lists what confirming cancels (in-flight operations, an
/// in-flight send may already be delivered) and what it drops (unsaved
/// composer edits); `Keep working` is the safe default button.
pub fn render_confirm(
    frame: &mut Frame<'_>,
    state: &crate::app::state::AppState,
    theme: &Theme,
    hits: &mut HitMap,
) {
    let Some(Overlay::SwitchConfirm(dialog)) = &state.session.overlay else {
        return;
    };
    let area = confirm_layout(
        state.session.size,
        !dialog.operations.is_empty(),
        dialog.unsaved_draft,
    );
    if area.width < 6 || area.height < 4 {
        return;
    }
    let Some(inner) = chrome::modal_frame(
        frame,
        area,
        &Span::raw(" Switch account? "),
        theme.accent,
        theme,
    ) else {
        return;
    };
    let target = text::truncate(&dialog.target, inner.width as usize);
    let mut body_lines: Vec<Line<'_>> = vec![Line::from(Span::styled(
        format!("Switch to {target}."),
        Style::new().fg(theme.text),
    ))];
    if !dialog.operations.is_empty() {
        let list = text::truncate(&dialog.operations.join(", "), inner.width as usize);
        body_lines.push(Line::from(Span::styled(
            format!("Cancels now: {list}"),
            Style::new().fg(theme.text_soft),
        )));
    }
    if dialog.unsaved_draft {
        body_lines.push(Line::from(Span::styled(
            "The composer has unsaved changes; they will be lost.",
            Style::new().fg(theme.warning),
        )));
    }
    body_lines.push(Line::from(Span::raw("")));
    body_lines.push(Line::from(button_spans(dialog.button, theme)));
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
            text::clip("Tab switch · ↵ confirm · Esc cancel", inner.width as usize),
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
    // §10): the same `ConfirmButton` vocabulary the discard dialog uses —
    // the shared click path focuses the button, then runs Enter's path.
    if inner.height > 3 {
        let confirm_w = chrome::labeled_button_label("Switch anyway").width() as u16;
        let keep_w = chrome::labeled_button_label("Keep working").width() as u16;
        hits.push(
            Rect {
                x: inner.x,
                y: inner.y + inner.height - 2,
                width: confirm_w,
                height: 1,
            },
            ClickTarget::ConfirmButton(ConfirmButton::Discard),
        );
        hits.push(
            Rect {
                x: inner.x + confirm_w + 1,
                y: inner.y + inner.height - 2,
                width: keep_w,
                height: 1,
            },
            ClickTarget::ConfirmButton(ConfirmButton::Keep),
        );
    }
}

/// ` [ Switch anyway ] ` (warning fill when focused — confirming cancels
/// work and drops edits) / ` [ Keep working ] ` (accent fill when
/// focused); the unfocused buttons stay quiet. The widths the click
/// targets use come from the same [`chrome::labeled_button_label`].
fn button_spans<'a>(button: ConfirmButton, theme: &'a Theme) -> Vec<Span<'a>> {
    vec![
        chrome::labeled_button(
            "Switch anyway",
            button == ConfirmButton::Discard,
            theme.error,
            theme,
        ),
        Span::raw(" "),
        chrome::labeled_button(
            "Keep working",
            button == ConfirmButton::Keep,
            theme.accent,
            theme,
        ),
    ]
}
