//! Sidebar: Compose affordance + mailbox list (mockup `list.html`).
//!
//! No labels block and no storage meter: both are v1 overrides / out of
//! scope (plan §3, §4).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::focus::Focus;
use crate::app::state::AppState;
use crate::ui::text;
use crate::ui::theme::Theme;

/// Render the sidebar into `area` (width 24 in full mode).
pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let rows = LayoutRows::new(area);

    // Compose affordance: bordered well with a `c` hint (plan §10: `c` =
    // compose). Not a focus target until the composer exists (Phase 6).
    let compose_focused = false;
    let compose_style = if compose_focused {
        theme.hover()
    } else {
        Style::new().bg(theme.surface)
    };
    let compose = Line::from(vec![
        Span::styled("+ ", Style::new().fg(theme.accent)),
        Span::styled(
            "Compose",
            Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ".repeat(rows.compose.width.saturating_sub(13) as usize)),
        Span::styled(" c", Style::new().fg(theme.dim)),
    ])
    .style(compose_style);
    frame.render_widget(
        Paragraph::new(compose).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(theme.border)),
        ),
        rows.compose,
    );

    // Mailbox list.
    if let Some(mailboxes) = state.mailboxes.as_loaded() {
        let active_id = state.active_route().and_then(|r| r.mailbox_id().cloned());
        let bottom = area.y + area.height;
        for (y, (i, mailbox)) in (rows.folders.y..).zip(mailboxes.iter().enumerate()) {
            if y >= bottom {
                break;
            }
            let is_active = Some(&mailbox.id) == active_id.as_ref();
            let cursor = state.focus == Focus::Sidebar && i == state.mailbox_selection;
            let row_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            };
            render_folder_row(frame, row_area, state, theme, mailbox, is_active, cursor);
        }
    }
}

fn render_folder_row(
    frame: &mut Frame<'_>,
    area: Rect,
    _state: &AppState,
    theme: &Theme,
    mailbox: &crate::domain::Mailbox,
    is_active: bool,
    cursor: bool,
) {
    let width = area.width as usize;
    // Row anatomy: 1 marker col + 2 left pad + name + gap + count + 1 right pad.
    let count = match mailbox.unread_count {
        Some(n) if n > 0 => n.to_string(),
        _ => String::new(),
    };
    let name_budget = width.saturating_sub(4 + count.width()).max(1);
    let name = text::clip(&mailbox.name, name_budget);
    let gap = width.saturating_sub(4 + name.width() + count.width());

    let row_bg = if is_active {
        theme.accent_bg
    } else if cursor {
        theme.surface2
    } else {
        theme.background
    };
    let pad = Style::new().bg(row_bg);
    let name_style = if is_active {
        Style::new().fg(theme.text).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.text_soft)
    }
    .bg(row_bg);
    let count_style = if is_active {
        Style::new().fg(theme.text_soft)
    } else {
        Style::new().fg(theme.dim)
    }
    .bg(row_bg);
    // Inset accent bar on the active folder (mockup `.folder.active`).
    let marker = if is_active { "▏" } else { " " };
    let marker_style = if is_active {
        Style::new().fg(theme.accent)
    } else {
        pad
    }
    .bg(row_bg);
    let spans = vec![
        Span::styled(marker, marker_style),
        Span::styled("  ", pad),
        Span::styled(name, name_style),
        Span::styled(" ".repeat(gap), pad),
        Span::styled(count, count_style),
        Span::styled(" ", pad),
    ];
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Regions inside the sidebar.
struct LayoutRows {
    compose: Rect,
    folders: Rect,
}

impl LayoutRows {
    fn new(area: Rect) -> Self {
        let compose_height = area.height.min(3);
        let compose = Rect {
            x: area.x.saturating_add(1),
            y: area.y.saturating_add(1),
            width: area.width.saturating_sub(2),
            height: compose_height,
        };
        let folders_y = area.y.saturating_add(compose_height + 2);
        let folders = Rect {
            x: area.x,
            y: folders_y,
            width: area.width,
            height: area.height.saturating_sub(folders_y - area.y),
        };
        Self { compose, folders }
    }
}
