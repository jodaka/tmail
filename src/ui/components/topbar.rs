//! Top bar: brand, search field, clock (mockup `list.html` topbar).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::action::ClickTarget;
use crate::app::state::AppState;
use crate::input::mouse::HitMap;
use crate::ui::chrome::{self, HairlineSide};
use crate::ui::text;
use crate::ui::theme::Theme;

/// Render the topbar into `area` (height 4: 3 content rows + hairline).
#[allow(clippy::too_many_arguments)]
pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    clock: &str,
    hits: &mut HitMap,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    // The topbar carries the `sidebar_bg` panel color, like the sidebar
    // and status bar: the whole strip (brand, search field, clock) sits
    // on it, not on the page background.
    frame.render_widget(Block::new().style(Style::new().bg(theme.sidebar_bg)), area);

    // Brand row: "• tmail" bold + version dim, always rendered (the loader
    // no longer replaces it — it drops one row below instead). Animated
    // from the tick counter (plan §20).
    let brand_area = Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_add(1),
        width: area.width.min(22),
        height: 1,
    };
    let brand = Line::from(vec![
        Span::styled("•", Style::new().fg(theme.accent2)),
        Span::raw(" "),
        Span::styled(
            "t",
            Style::new()
                .fg(theme.label_dim)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "mail",
            Style::new()
                .fg(theme.text_soft)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" v{}", env!("CARGO_PKG_VERSION")),
            Style::new().fg(theme.label_dim),
        ),
    ]);
    frame.render_widget(Paragraph::new(brand), brand_area);

    // Account line under the brand (ticket c0n0): the identity the
    // backend drives — the email (the unique identity; display names are
    // commonly shared, user request), else the display name, else the
    // raw account name. Nothing renders when no account is resolved
    // (multi-account configs without a default; the account switcher is
    // the way to pick one there).
    if let Some(account) = state.current_account_label() {
        let account_area = Rect {
            x: area.x.saturating_add(2),
            y: area.y.saturating_add(2),
            width: area.width.min(22),
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                text::truncate(&account, account_area.width as usize),
                Style::new().fg(theme.label_dim),
            )),
            account_area,
        );
    }

    // Search field: bordered well, `/` prompt, query or placeholder.
    // `search_x` is absolute; the width budget is computed relative to
    // `area` (both offsets must not double-count the screen origin).
    let search_width = area.width.saturating_sub(28).clamp(12, 60);
    let search_x = area.x.saturating_add(24);
    let search_area = Rect {
        x: search_x,
        y: area.y,
        width: search_width.min(area.width.saturating_sub(24 + 18)),
        height: 3,
    };
    if search_area.width >= 4 {
        let focused = state.session.focus == crate::app::focus::Focus::SearchField;
        let border_style = if focused {
            Style::new().fg(theme.accent)
        } else {
            Style::new().fg(theme.border)
        };
        // The search well sits on the panel color; focus shows through the
        // accent border and the cursor, never a fill change.
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(symbols::border::ROUNDED)
            .border_style(border_style)
            .style(Style::new().bg(theme.sidebar_bg));
        let query = &state.session.search_query;
        let prompt = Span::styled("/", Style::new().fg(theme.dim));
        let text = if query.is_empty() && !focused {
            Span::styled(" Search mail", Style::new().fg(theme.dim))
        } else {
            Span::styled(format!(" {query}"), Style::new().fg(theme.text))
        };
        let cursor = if focused {
            Span::styled("▏", Style::new().fg(theme.accent))
        } else {
            Span::raw("")
        };
        let field = Paragraph::new(Line::from(vec![prompt, text, cursor])).block(block);
        frame.render_widget(field, search_area);
        // Clicking the field focuses it (`/`'s job, plan §10).
        hits.push(search_area, ClickTarget::SearchField);
    }

    // Top-right slot on the brand row: a status message when one is
    // showing (the shortcut-label color — the same faded tone the status
    // bar's hint labels carry — one column of padding off the right
    // edge), otherwise the clock (`[tmail.ui].clock`, ticket w7f5: off by
    // default). Both would collide, so the message wins its row.
    let status_message = state.session.status.message.as_deref();
    // The search well may end deep into the row: the message clips in
    // front of it, never over it.
    let message_budget = area
        .width
        .saturating_sub(search_x - area.x + search_width + 4)
        .max(10) as usize;
    if let Some(message) = status_message {
        let message = text::truncate(message, message_budget);
        let width = message.width() as u16;
        if width > 0 {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    message,
                    Style::new().fg(theme.label_dim).bg(theme.sidebar_bg),
                )),
                Rect {
                    x: area.x + area.width - width - 1,
                    y: area.y.saturating_add(1),
                    width,
                    height: 1,
                },
            );
        }
    } else if !clock.is_empty() && clock.width() < area.width as usize {
        let clock_area = Rect {
            x: area.x + area.width - clock.width() as u16 - 2,
            y: area.y.saturating_add(1),
            width: clock.width() as u16,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Span::styled(clock, Style::new().fg(theme.dim))),
            clock_area,
        );
    }

    // Bottom hairline.
    let hairline_area = Rect {
        x: area.x,
        y: area.y + area.height - 1,
        width: area.width,
        height: 1,
    };
    chrome::hairline(frame, hairline_area, HairlineSide::Bottom, theme);
}
