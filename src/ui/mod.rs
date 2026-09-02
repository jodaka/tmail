//! Rendering entry point: theme + responsive layout + chrome components.

pub mod components;
pub mod dates;
pub mod layout;
pub mod screens;
pub mod text;
pub mod theme;

pub use theme::Theme;

use chrono::{DateTime, FixedOffset};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::state::AppState;
use crate::ui::layout::LayoutMode;

/// Values injected at render time so neither the reducer nor snapshots
/// depend on the wall clock (plan §20).
pub struct RenderContext<'a> {
    /// Current time for the clock and relative dates.
    pub now: DateTime<FixedOffset>,
    /// Preformatted clock label (`Wed Sep 2 · 10:47`).
    pub clock: String,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> RenderContext<'a> {
    pub fn new(now: DateTime<FixedOffset>, clock: String) -> Self {
        Self {
            now,
            clock,
            _marker: std::marker::PhantomData,
        }
    }
}

/// Render one frame.
pub fn render(frame: &mut Frame<'_>, state: &AppState, theme: &Theme, ctx: &RenderContext<'_>) {
    let area = frame.area();
    frame.render_widget(
        ratatui::widgets::Block::new().style(theme.on_background()),
        area,
    );
    let mode = layout::mode_for(state.size.0, state.size.1);
    if mode == LayoutMode::TooSmall {
        render_too_small(frame, area, state, theme);
        return;
    }
    let (topbar, body, statusbar) = layout::split_vertical(area);
    components::topbar::render(frame, topbar, state, theme, &ctx.clock);
    let (sidebar, list) = layout::split_body(mode, body);
    if let Some(sidebar) = sidebar {
        components::sidebar::render(frame, sidebar, state, theme);
    }
    screens::mailbox::render(frame, list, state, mode, theme, ctx.now);
    components::statusbar::render(frame, statusbar, state, theme);
}

/// Too-small mode: a clear centered message, nothing overlapping (plan §18).
fn render_too_small(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let message = format!(
        "Terminal too small ({}×{})\npost needs at least {}×{} columns/rows.\nEnlarge the window or press Esc to quit.",
        state.size.0,
        state.size.1,
        layout::COMPACT_MIN_WIDTH,
        layout::COMPACT_MIN_HEIGHT,
    );
    let lines: Vec<Line<'_>> = message
        .lines()
        .map(|l| {
            Line::from(Span::styled(
                l,
                Style::new().fg(theme.text).bg(theme.background),
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).centered(), area);
}
