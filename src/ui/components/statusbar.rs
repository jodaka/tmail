//! Status bar: mode badge, key hints, environment info (mockup `list.html`).
//!
//! v1 overrides applied (plan §4): no `j`/`k` move hints, no `?` help.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::state::AppState;
use crate::ui::theme::Theme;

/// Render the status bar into `area` (height 3: hairline + content row).
pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
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

    // Key hints follow the input contract (plan §10). Arrows move, Enter
    // opens, single letters act. Deliberately no j/k, no help.
    let hints = [
        ("↑↓", "move"),
        ("↵", "open"),
        ("s", "star"),
        ("e", "archive"),
        ("c", "compose"),
        ("/", "search"),
    ];
    let mut spans: Vec<Span<'_>> = vec![Span::styled(
        " NORMAL ",
        theme.mode_badge().add_modifier(Modifier::empty()),
    )];
    for (key, label) in hints {
        spans.push(Span::styled("  ", Style::new().bg(theme.background)));
        spans.push(Span::styled(
            key,
            Style::new().fg(theme.text_soft).bg(theme.background),
        ));
        spans.push(Span::styled(
            format!(" {label}"),
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
