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
    // Foreground work never blocks input, but it is announced here so the
    // user knows what `Esc` would cancel (plan §11).
    if let Some(operation) = state.operations.foreground() {
        spans.push(Span::styled("  ", Style::new().bg(theme.background)));
        spans.push(Span::styled(
            super::spinner::frame(state.ticks),
            Style::new().fg(theme.accent).bg(theme.background),
        ));
        spans.push(Span::styled(
            format!(" {}", operation.kind.summary()),
            Style::new().fg(theme.muted).bg(theme.background),
        ));
    }
    if let Some(message) = &state.status.message {
        spans.push(Span::styled(
            format!("  · {message}"),
            Style::new().fg(theme.accent).bg(theme.background),
        ));
    }

    let env = format!("UTF-8 · {}×{}", state.size.0, state.size.1);
    let env_w = env.width() as u16;
    if env_w + 2 < area.width {
        let env_area = Rect {
            x: area.x + area.width - env_w - 2,
            y: row.y,
            width: env_w + 2,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" ", Style::new().bg(theme.background)),
                Span::styled(env, Style::new().fg(theme.dim).bg(theme.background)),
            ])),
            env_area,
        );
        // Trim the hint row so it never overlaps the env info.
        let hint_budget = (area.width - env_w - 4) as usize;
        let mut used = 0usize;
        spans.retain(|span| {
            let w = span.content.width();
            if used + w <= hint_budget {
                used += w;
                true
            } else {
                false
            }
        });
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), row);
}
