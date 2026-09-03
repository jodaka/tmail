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
use crate::ui::theme::Theme;

/// Render the topbar into `area` (height 4: 3 content rows + hairline).
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

    // Brand row: "post" bold + version dim — or, while foreground work is
    // in flight, the loader in its place (ticket m3by: the status spinner
    // lives on top of the program name; the brand returns when loading
    // finishes). Animated from the tick counter (plan §20).
    let brand_area = Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_add(1),
        width: area.width.min(22),
        height: 1,
    };
    if state.operations.foreground().is_some() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                super::spinner::frame(state.ticks),
                Style::new().fg(theme.accent),
            )),
            brand_area,
        );
    } else {
        let brand = Line::from(vec![
            Span::styled(
                "post",
                Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" v{}", env!("CARGO_PKG_VERSION")),
                Style::new().fg(theme.dim),
            ),
        ]);
        frame.render_widget(Paragraph::new(brand), brand_area);
    }

    // Search field: bordered well, `/` prompt, query or placeholder.
    let search_width = area.width.saturating_sub(28).clamp(12, 60);
    let search_x = area.x.saturating_add(24);
    let search_area = Rect {
        x: search_x,
        y: area.y,
        width: search_width.min(area.width.saturating_sub(search_x + area.x + 18)),
        height: 3,
    };
    if search_area.width >= 4 {
        let focused = state.focus == crate::app::focus::Focus::SearchField;
        let border_style = if focused {
            Style::new().fg(theme.accent)
        } else {
            Style::new().fg(theme.border)
        };
        let fill = if focused {
            theme.surface2
        } else {
            theme.surface
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(symbols::border::ROUNDED)
            .border_style(border_style)
            .style(Style::new().bg(fill));
        let query = &state.search_query;
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

    // Clock, right-aligned on the middle row. Empty string = disabled
    // (`[post.ui].clock`, ticket w7f5: off by default).
    if !clock.is_empty() && clock.len() < area.width as usize {
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
    frame.render_widget(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(theme.hairline()),
        hairline_area,
    );
}
