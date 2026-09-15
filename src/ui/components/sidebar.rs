//! Sidebar: mailbox list (mockup `list.html`, minus the Compose
//! affordance).
//!
//! The listing splits into system folders, one blank row, then user
//! labels (a blank separator instead of the mockup's `LABELS` caption —
//! user preference); no storage meter: out of scope (plan §3, §4).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::action::ClickTarget;
use crate::app::focus::Focus;
use crate::app::state::AppState;
use crate::input::mouse::HitMap;
use crate::ui::chrome;
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
    // Panel fill: covers the folder rows' color gaps (e.g. the blank
    // separator row) and any folders-free expanse.
    frame.render_widget(Block::new().style(Style::new().bg(theme.sidebar_bg)), area);
    // The last sidebar column is a right margin (temporary experiment):
    // folder rows and notes never draw into it, only the fill does.
    let rows = Rect {
        x: area.x,
        y: area.y,
        width: area.width.saturating_sub(1),
        height: area.height,
    };

    // Mailbox list: backend-driven since Phase 2, so all loadable states
    // render safely (plan §16: empty results are valid). The backend hands
    // the listing folders-first, so the first label follows the blank
    // separator row; visual rows and state indices diverge there, so `y`
    // is counted separately from `i`.
    match &state.mailboxes {
        crate::app::state::Loadable::Loaded(mailboxes) if !mailboxes.is_empty() => {
            // While composing, the Drafts folder is the active one (the
            // composer writes drafts there); otherwise the displayed
            // mailbox.
            let active_id = state.sidebar_active_mailbox_id();
            let bottom = rows.y + rows.height;
            let first_label = mailboxes.iter().position(|m| m.is_label());
            let mut y = rows.y;
            for (i, mailbox) in mailboxes.iter().enumerate() {
                if Some(i) == first_label && i > 0 {
                    // The blank separator row: the sidebar's panel fill
                    // already covers it, so nothing is drawn — the row is
                    // simply skipped and stays unclickable.
                    if y >= bottom {
                        break;
                    }
                    y += 1;
                }
                if y >= bottom {
                    break;
                }
                let is_active = Some(&mailbox.id) == active_id;
                let cursor = state.session.focus == Focus::Sidebar && i == state.mailbox_selection;
                let row_area = Rect {
                    x: rows.x,
                    y,
                    width: rows.width,
                    height: 1,
                };
                render_folder_row(
                    frame,
                    row_area,
                    theme,
                    mailbox,
                    is_active,
                    cursor,
                    theme.sidebar_bg,
                );
                // Clicking a folder selects it; clicking the selected one
                // switches (arrows + Enter, plan §10).
                hits.push(row_area, ClickTarget::Mailbox(i));
                y += 1;
            }
        }
        crate::app::state::Loadable::Loaded(_) => {
            chrome::render_note(frame, rows, theme, "(no mailboxes)")
        }
        // Loading: one centered spinner, like every other pane (ticket
        // m3by). `Idle` cannot occur for the sidebar slot, but the
        // fallback keeps the exhaustive match honest.
        crate::app::state::Loadable::Loading | crate::app::state::Loadable::Idle => {
            super::spinner::render_centered(frame, rows, theme, super::spinner::pane_millis(state));
        }
        crate::app::state::Loadable::Failed(_) => {
            chrome::render_note(frame, rows, theme, "mailboxes unavailable")
        }
    }
}

/// View-only nicety (permanent): Gmail exposes its special folders as
/// `[GMAIL]/Drafts` and friends; the sidebar shows the bare folder name.
/// Purely cosmetic — storage, routing, and the reducer keep the full
/// IMAP name everywhere else.
fn display_name(imap_name: &str) -> &str {
    match imap_name.get(..8) {
        Some(prefix) if prefix.eq_ignore_ascii_case("[GMAIL]/") => {
            imap_name.get(8..).unwrap_or(imap_name)
        }
        _ => imap_name,
    }
}

/// One sidebar folder row: marker column + name + ` (N)` unread counter.
/// Shared with the Mailboxes popup (issue brnw) so the popup draws the
/// sidebar's rows exactly — `panel_bg` is the color plain (inactive,
/// unfocused) rows sit on: the sidebar panel there, the popup's page
/// background in the popup.
pub(crate) fn render_folder_row(
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
    panel_bg: ratatui::style::Color,
) {
    let width = area.width as usize;
    // Row anatomy (ticket ye28): 1 marker col + 1 left pad + name +
    // ` (N)` when the folder holds unread mail + right pad. The counter
    // reads as part of the folder name, exactly as written: `Inbox (4)`.
    // The name budget keeps one column free on the right, so the text
    // carries a one-cell margin on both sides of the row.
    let suffix = match mailbox.unread_count {
        Some(n) if n > 0 => format!(" ({n})"),
        // Drafts are invisible to unread mail: the counter is the folder
        // content — how many drafts there are (Gmail reports drafts as
        // unread 0, so unread alone would leave the row counterless).
        _ if mailbox.role == Some(crate::domain::MailboxRole::Drafts) => {
            match mailbox.total_count {
                Some(n) if n > 0 => format!(" ({n})"),
                _ => String::new(),
            }
        }
        _ => String::new(),
    };
    let name_budget = width.saturating_sub(3 + suffix.width()).max(1);
    let name = text::clip(display_name(&mailbox.name), name_budget);
    let right_pad = width.saturating_sub(2 + name.width() + suffix.width());

    let row_bg = if is_active {
        // Active folder fill: the `marker` gold, the same highlight the
        // message list's cursor row carries — `accent_bg` sat one shade
        // off the sidebar panel and read as no fill at all.
        theme.marker
    } else if cursor {
        // Focused-control selection fill: the sidebar cursor row is the
        // one place the `selection` token shows (ticket e6wn removed the
        // hover-only `surface2`).
        theme.selection
    } else {
        // Plain folder rows sit on the caller's panel color: the sidebar
        // panel in the sidebar, the page background in the Mailboxes
        // popup (issue brnw).
        panel_bg
    };
    let pad = Style::new().bg(row_bg);
    let name_style = if is_active {
        // Dark text on the marker fill (the mode-badge convention).
        Style::new()
            .fg(theme.background)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.text_soft)
    }
    .bg(row_bg);
    let count_style = if is_active {
        Style::new().fg(theme.background)
    } else {
        Style::new().fg(theme.dim)
    }
    .bg(row_bg);
    // Inset left edge bar: white on the active folder and on the cursor
    // row while the sidebar holds focus (mockup `.folder.active`).
    let marker = if cursor || is_active { "▎" } else { " " };
    let marker_style = if cursor || is_active {
        Style::new().fg(theme.marker_bar)
    } else {
        pad
    }
    .bg(row_bg);
    let spans = vec![
        Span::styled(marker, marker_style),
        Span::styled(" ", pad),
        Span::styled(name, name_style),
        Span::styled(suffix, count_style),
        Span::styled(" ".repeat(right_pad), pad),
    ];
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
