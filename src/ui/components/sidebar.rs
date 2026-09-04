//! Sidebar: Compose affordance + mailbox list (mockup `list.html`).
//!
//! No labels block and no storage meter: both are v1 overrides / out of
//! scope (plan §3, §4).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::action::ClickTarget;
use crate::app::focus::Focus;
use crate::app::state::AppState;
use crate::input::mouse::HitMap;
use crate::ui::text;
use crate::ui::theme::Theme;

/// Render the sidebar into `area` (width 24 in full mode).
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
    let rows = LayoutRows::new(area);

    // Compose affordance: bordered well (mockup `new-mail.html` button
    // shape: rounded, like the search field). No inline `c` hint — the
    // status bar advertises the shortcut. The well sits on the page
    // background (ticket e6wn).
    let compose_style = Style::new().bg(theme.background);
    let compose = Line::from(vec![
        Span::styled("+ ", Style::new().fg(theme.accent)),
        Span::styled(
            "Compose",
            Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
    ])
    .style(compose_style);
    frame.render_widget(
        Paragraph::new(compose).block(
            Block::default()
                .borders(Borders::ALL)
                .border_set(symbols::border::ROUNDED)
                .border_style(Style::new().fg(theme.border)),
        ),
        rows.compose,
    );
    // Clicking the affordance composes (`c`'s job, plan §10).
    hits.push(rows.compose, ClickTarget::ComposeButton);

    // Mailbox list: backend-driven since Phase 2, so all loadable states
    // render safely (plan §16: empty results are valid).
    match &state.mailboxes {
        crate::app::state::Loadable::Loaded(mailboxes) if !mailboxes.is_empty() => {
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
                render_folder_row(frame, row_area, theme, mailbox, is_active, cursor);
                // Clicking a folder selects it; clicking the selected one
                // switches (arrows + Enter, plan §10).
                hits.push(row_area, ClickTarget::Mailbox(i));
            }
        }
        crate::app::state::Loadable::Loaded(_) => {
            render_note(frame, rows.folders, theme, "(no mailboxes)")
        }
        // Loading: one centered spinner, like every other pane (ticket
        // m3by). `Idle` cannot occur for the sidebar slot, but the
        // fallback keeps the exhaustive match honest.
        crate::app::state::Loadable::Loading | crate::app::state::Loadable::Idle => {
            super::spinner::render_centered(frame, rows.folders, theme, state.ticks);
        }
        crate::app::state::Loadable::Failed(_) => {
            render_note(frame, rows.folders, theme, "mailboxes unavailable")
        }
    }
}

/// A dim one-line placeholder for the mailbox list area.
fn render_note(frame: &mut Frame<'_>, area: Rect, theme: &Theme, note: &str) {
    if area.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Span::styled(
            text::clip(note, area.width as usize),
            Style::new().fg(theme.dim),
        )),
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        },
    );
}

fn render_folder_row(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    mailbox: &crate::domain::Mailbox,
    is_active: bool,
    // Sidebar-focused cursor on this row: the only state in which a folder
    // row shows the accent marker. The marker is the focus indicator, so
    // the message list (or search field) holding focus leaves no folder
    // marked, even the active one.
    cursor: bool,
) {
    let width = area.width as usize;
    // Row anatomy (ticket ye28): 1 marker col + 2 left pad + name +
    // ` (N)` when the folder holds unread mail + right pad. The counter
    // reads as part of the folder name, exactly as written: `Inbox (4)`.
    let suffix = match mailbox.unread_count {
        Some(n) if n > 0 => format!(" ({n})"),
        _ => String::new(),
    };
    let name_budget = width.saturating_sub(3 + suffix.width()).max(1);
    let name = text::clip(&mailbox.name, name_budget);
    let right_pad = width.saturating_sub(3 + name.width() + suffix.width());

    let row_bg = if is_active {
        theme.accent_bg
    } else if cursor {
        // Focused-control selection fill: the sidebar cursor row is the
        // one place the `selection` token shows (ticket e6wn removed the
        // hover-only `surface2`).
        theme.selection
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
    // Inset accent bar marking the focused row (mockup `.folder.active`):
    // the cursor row while the sidebar holds focus.
    let marker = if cursor { "▏" } else { " " };
    let marker_style = if cursor {
        Style::new().fg(theme.accent)
    } else {
        pad
    }
    .bg(row_bg);
    let spans = vec![
        Span::styled(marker, marker_style),
        Span::styled("  ", pad),
        Span::styled(name, name_style),
        Span::styled(suffix, count_style),
        Span::styled(" ".repeat(right_pad), pad),
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
