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
        let shown = text::truncate(&account, account_area.width as usize);
        frame.render_widget(
            Paragraph::new(Span::styled(
                shown.clone(),
                Style::new().fg(theme.label_dim),
            )),
            account_area,
        );
        // The line is a button: clicking it opens the account switcher,
        // `Ctrl+G`'s job. The hit covers only the label drawn (truncated
        // to the widget's width), so clicks past the text fall through
        // to whatever sits beneath.
        let label_w = shown.width() as u16;
        if label_w > 0 {
            hits.push(
                Rect {
                    x: account_area.x,
                    y: account_area.y,
                    width: label_w,
                    height: 1,
                },
                ClickTarget::AccountButton,
            );
        }
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
    // front of it, never over it. The budget prefers 10 columns but is
    // clipped to the row (minus the placement's one-column right pad) so
    // the right-aligned arithmetic below can never underflow — narrow
    // bars stay safe even though the too-small screen currently keeps
    // this bar at 90+ columns (ticket 6t30).
    let message_budget = area
        .width
        .saturating_sub(search_x - area.x + search_width + 4)
        .max(10)
        .min(area.width.saturating_sub(1)) as usize;
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
    } else if !clock.is_empty() && clock.width() + 2 <= area.width as usize {
        // The placement subtracts a two-column right pad; the guard must
        // require it or `x` underflows on narrow bars (ticket 6t30).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::mock::mock_initial_state;
    use crate::input::mouse::HitMap;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn draw(width: u16, clock: &str, message: Option<&str>) -> ratatui::buffer::Buffer {
        draw_hits(width, clock, message).0
    }

    fn draw_hits(
        width: u16,
        clock: &str,
        message: Option<&str>,
    ) -> (ratatui::buffer::Buffer, HitMap) {
        let mut state = mock_initial_state();
        state.session.status.message = message.map(String::from);
        let backend = TestBackend::new(width, 4);
        let mut terminal = Terminal::new(backend).expect("test backend");
        let mut hits = HitMap::default();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    frame.area(),
                    &state,
                    &Theme::default_dark(),
                    clock,
                    &mut hits,
                )
            })
            .expect("draw");
        (terminal.backend().buffer().clone(), hits)
    }

    #[test]
    fn narrow_bars_never_panic_on_the_right_slots() {
        // The message placement subtracts `width + 1` from the right edge;
        // a budget claiming more than the row underflowed before the clip
        // (ticket 6t30 — currently unreachable because the too-small screen
        // keeps this bar at 90+ columns, hardened anyway).
        let _ = draw(3, "", Some("a very wide status message"));
        let _ = draw(10, "", Some("a very wide status message"));
        let _ = draw(60, "", Some("a very wide status message"));
    }

    #[test]
    fn clock_requires_its_two_column_right_pad() {
        // One column of slack: the clock is skipped, not underflowed.
        let _ = draw(6, "10:47", None);
        // At the boundary it draws, right-aligned.
        let buffer = draw(7, "10:47", None);
        let row: String = (0..7)
            .map(|x| buffer[(x, 1)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(row.contains("10:47"), "clock drawn at the boundary: {row}");
    }

    #[test]
    fn the_account_line_click_opens_the_switcher() {
        // The mock fixture carries no accounts; give it one so the
        // label renders an account button (row 2 is the account line).
        let (buffer, hits) = {
            let mut state = mock_initial_state();
            state.settings.accounts = vec![crate::config::AccountEntry {
                name: String::from("personal"),
                email: Some(String::from("personal@example.org")),
                display_name: None,
                is_default: true,
            }];
            state.settings.account_name = Some(String::from("personal"));
            let backend = TestBackend::new(80, 4);
            let mut terminal = Terminal::new(backend).expect("test backend");
            let mut hits = HitMap::default();
            terminal
                .draw(|frame| {
                    render(
                        frame,
                        frame.area(),
                        &state,
                        &Theme::default_dark(),
                        "",
                        &mut hits,
                    )
                })
                .expect("draw");
            (terminal.backend().buffer().clone(), hits)
        };
        // The email label renders where expected and is clickable.
        let account_line: String = (0..22)
            .map(|x| buffer[(x, 2)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            account_line.contains("personal@example.org"),
            "account label drawn: {account_line:?}"
        );
        assert_eq!(hits.hit_test(2, 2, false), Some(ClickTarget::AccountButton));
        // Past the label's width the click is no longer the account
        // button: the real content beneath (the 3-row-tall search well)
        // takes the click.
        assert_eq!(hits.hit_test(25, 2, false), Some(ClickTarget::SearchField));
    }
}
