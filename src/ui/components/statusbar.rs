//! Status bar: key hints, foreground-work spinner, environment info
//! (mockup `list.html`).
//!
//! v1 overrides applied (plan §4): no `j`/`k` move hints, no `?` help. The
//! mockup's mode badge was dropped: it named the screen the user is already
//! looking at, so it carried no information.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::action::{BulkOp, ClickTarget};
use crate::app::route::Route;
use crate::app::state::AppState;
use crate::input::mouse::HitMap;
use crate::ui::text;
use crate::ui::theme::Theme;

/// Render the status bar into `area` (height 3: hairline + content row).
pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    hits: &mut HitMap,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let hairline = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    frame.render_widget(
        Block::default()
            .borders(Borders::TOP)
            .border_style(theme.hairline()),
        hairline,
    );

    let row = Rect {
        x: area.x,
        y: area.y.saturating_add(1),
        width: area.width,
        height: 1,
    };

    // Key hints follow the input contract (plan §10) and the active screen
    // (mockup `.statusbar` per layout). Deliberately no j/k, no help.
    // Selection mode replaces the hint row with the bulk-operation buttons
    // (ticket p0s3): the count and the four actions, each clickable.
    let reader = matches!(state.active_route(), Some(Route::Message(_)));
    let composer = matches!(state.active_route(), Some(Route::Composer));
    let selection_mode = state.selection_active() && !reader && !composer;
    let mut spans: Vec<Span<'_>> = Vec::new();
    if selection_mode {
        let count = format!(
            "  {} selected:",
            state.visible_selected_count().max(state.selected.len())
        );
        let mut cursor_x = row.x + count.width() as u16;
        spans.push(Span::styled(
            count,
            Style::new()
                .fg(theme.accent)
                .bg(theme.background)
                .add_modifier(ratatui::style::Modifier::BOLD),
        ));
        for (op, label) in [
            (BulkOp::Trash, "[delete]"),
            (BulkOp::Archive, "[archive]"),
            (BulkOp::MarkRead, "[read]"),
            (BulkOp::MarkUnread, "[unread]"),
        ] {
            let width = label.width() as u16 + 1;
            if cursor_x + width > area.x + area.width {
                break;
            }
            spans.push(Span::styled(
                format!(" {label}"),
                Style::new().fg(theme.text_soft).bg(theme.background),
            ));
            hits.push(
                Rect {
                    x: cursor_x,
                    y: row.y,
                    width,
                    height: 1,
                },
                ClickTarget::BulkAction(op),
            );
            cursor_x += width;
        }
        spans.push(Span::styled(
            "  esc clear",
            Style::new().fg(theme.muted).bg(theme.background),
        ));
    } else {
        let hints: &[(&str, &str)] = if composer {
            &[
                ("tab", "next field"),
                ("esc", "save & leave"),
                ("^↵", "send"),
            ]
        } else if reader {
            &[
                ("↑↓", "scroll"),
                ("esc", "back"),
                ("r", "reply"),
                ("f", "forward"),
                ("e", "archive"),
                ("s", "star"),
                ("d", "delete"),
            ]
        } else {
            &[
                ("↑↓", "move"),
                ("↵", "open"),
                ("space", "select"),
                ("s", "star"),
                ("e", "archive"),
                // `d` deletes (trash) — the focused row, or the whole
                // selection when bulk-selection mode is on (ticket h1m2).
                ("d", "delete"),
                ("c", "compose"),
                ("/", "search"),
            ]
        };
        for (key, label) in hints {
            spans.push(Span::styled("  ", Style::new().bg(theme.background)));
            spans.push(Span::styled(
                *key,
                Style::new().fg(theme.text_soft).bg(theme.background),
            ));
            spans.push(Span::styled(
                format!(" {label}"),
                Style::new().fg(theme.muted).bg(theme.background),
            ));
        }
    }
    // Foreground work is announced by the loader in the top bar (in place
    // of the program name, ticket m3by); the status bar keeps hints and
    // the status message only.
    // Status messages sit in the bottom-right corner (ticket en85): the
    // encoding/size readout they replaced carried nothing the user could
    // act on. Clipped to the space left of the hints so they never
    // overlap, with one column of padding off the right border
    // (ticket h1d7).
    if let Some(message) = &state.status.message {
        let left_used: usize = spans.iter().map(|s| s.content.width()).sum();
        // 3 columns of existing slack plus the 1 padding column.
        let budget = (area.width as usize)
            .saturating_sub(left_used)
            .saturating_sub(4)
            .max(10);
        let message = text::truncate(message, budget);
        let width = message.width() as u16;
        if width > 0 {
            frame.render_widget(
                Paragraph::new(Span::styled(message, status_message_style(theme, state))),
                Rect {
                    x: (area.x + area.width)
                        .saturating_sub(width)
                        .saturating_sub(1),
                    y: row.y,
                    width,
                    height: 1,
                },
            );
        }
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), row);
}

/// Closing seconds of the timeout window over which the status message
/// fades into the background (ticket h1d7).
const STATUS_FADE_SECONDS: f64 = 0.3;

/// Status-message style (ticket h1d7): accent on the page background.
/// With `[post].status_timeout > 0` the message fades into the background
/// over the closing [`STATUS_FADE_SECONDS`] of its window; the reducer
/// clears it when the window elapses. The fade interpolates the two
/// colors, so the monochrome theme (terminal defaults) renders at full
/// strength instead.
fn status_message_style(theme: &Theme, state: &AppState) -> Style {
    let Some(fg) = as_rgb(theme.accent) else {
        return Style::new().fg(theme.accent).bg(theme.background);
    };
    let Some(bg) = as_rgb(theme.background) else {
        return Style::new().fg(theme.accent).bg(theme.background);
    };
    let alpha = status_alpha(state);
    Style::new().fg(lerp(fg, bg, alpha)).bg(theme.background)
}

/// Remaining visibility of the current status message as an opacity in
/// `0.0..=1.0`: `1.0` until the fade window opens, then linearly to `0.0`
/// as the timeout elapses.
fn status_alpha(state: &AppState) -> f64 {
    if state.status_timeout_seconds == 0 {
        return 1.0;
    }
    let (Some(now), Some(shown_at)) = (state.clock, state.status.shown_at) else {
        return 1.0;
    };
    let elapsed = (now - shown_at).num_milliseconds().max(0) as f64 / 1000.0;
    let remaining = state.status_timeout_seconds as f64 - elapsed;
    (remaining / STATUS_FADE_SECONDS).clamp(0.0, 1.0)
}

fn as_rgb(color: ratatui::style::Color) -> Option<(u8, u8, u8)> {
    match color {
        ratatui::style::Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

/// Linear interpolation `from` → `to` at `alpha` (1.0 keeps `from`).
fn lerp(from: (u8, u8, u8), to: (u8, u8, u8), alpha: f64) -> ratatui::style::Color {
    let mix = |a: u8, b: u8| (a as f64 * alpha + b as f64 * (1.0 - alpha)).round() as u8;
    ratatui::style::Color::Rgb(mix(from.0, to.0), mix(from.1, to.1), mix(from.2, to.2))
}
