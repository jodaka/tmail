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
                ("e", "archive"),
                ("s", "star"),
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
    // overlap.
    if let Some(message) = &state.status.message {
        let left_used: usize = spans.iter().map(|s| s.content.width()).sum();
        let budget = (area.width as usize)
            .saturating_sub(left_used)
            .saturating_sub(3)
            .max(10);
        let message = text::truncate(message, budget);
        let width = message.width() as u16;
        if width > 0 {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    message,
                    Style::new().fg(theme.accent).bg(theme.background),
                )),
                Rect {
                    x: area.x + area.width - width,
                    y: row.y,
                    width,
                    height: 1,
                },
            );
        }
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), row);
}
