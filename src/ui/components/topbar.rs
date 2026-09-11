//! Top bar: brand, search field, clock (mockup `list.html` topbar).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
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

/// Topbar fill, distinct from the page background (temporary experiment):
/// rgb(21, 24, 32).
const TOPBAR_BG: Color = Color::Rgb(0x15, 0x18, 0x20);
/// Brand bullet: rgb(131, 189, 99).
const BRAND_DOT: Color = Color::Rgb(0x83, 0xBD, 0x63);
/// Brand "t" glyph and the version: rgb(87, 94, 113).
const BRAND_DIM: Color = Color::Rgb(0x57, 0x5E, 0x71);
/// Brand "mail" glyph: rgb(191, 189, 183).
const BRAND_TEXT: Color = Color::Rgb(0xBF, 0xBD, 0xB7);
/// Loader scanner color: rgb(117, 192, 249).
const LOADER_COLOR: Color = Color::Rgb(0x75, 0xC0, 0xF9);

/// Render the topbar into `area` (height 4: 3 content rows + hairline).
#[allow(clippy::too_many_arguments)]
pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    clock: &str,
    loader_millis: u64,
    hits: &mut HitMap,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    // The topbar carries its own panel color: the whole strip (brand,
    // search field, clock) sits on it, not on the page background.
    frame.render_widget(Block::new().style(Style::new().bg(TOPBAR_BG)), area);

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
        Span::styled("•", Style::new().fg(BRAND_DOT)),
        Span::raw(" "),
        Span::styled("t", Style::new().fg(BRAND_DIM).add_modifier(Modifier::BOLD)),
        Span::styled(
            "mail",
            Style::new().fg(BRAND_TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" v{}", env!("CARGO_PKG_VERSION")),
            Style::new().fg(BRAND_DIM),
        ),
    ]);
    frame.render_widget(Paragraph::new(brand), brand_area);

    // Loader (opencode's TUI scanner, "blocks" style): a Knight Rider
    // sweep rendered on the row right below the logo while foreground work
    // is in flight.
    if state.session.operations.foreground().is_some() {
        super::spinner::render_blocks(
            frame,
            Rect {
                x: area.x.saturating_add(2),
                y: area.y.saturating_add(2),
                width: area.width,
                height: 1,
            },
            LOADER_COLOR,
            TOPBAR_BG,
            loader_millis,
        );
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
        let focused = state.session.focus == crate::app::focus::Focus::SearchField;
        let border_style = if focused {
            Style::new().fg(theme.accent)
        } else {
            Style::new().fg(theme.border)
        };
        // The well sits on the topbar panel color; focus shows through the
        // accent border and the cursor, never a fill change.
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(symbols::border::ROUNDED)
            .border_style(border_style)
            .style(Style::new().bg(TOPBAR_BG));
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
    // showing (faded color, one column of padding off the right edge),
    // otherwise the clock (`[tmail.ui].clock`, ticket w7f5: off by
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
                    Style::new().fg(theme.muted).bg(TOPBAR_BG),
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
