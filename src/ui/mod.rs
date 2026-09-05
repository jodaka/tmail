//! Rendering entry point: theme + responsive layout + chrome components.

pub mod components;
pub mod dates;
pub mod layout;
pub(crate) mod rich;
pub mod screens;
pub mod text;
pub mod theme;

pub use theme::Theme;

use chrono::{DateTime, FixedOffset};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::route::Route;
use crate::app::state::AppState;
use crate::input::mouse::HitMap;
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

/// Render one frame. `hits` collects the interactive widget rectangles
/// (plan §10, Phase 10.1): the mouse layer hit-tests the next click
/// against the map of the frame currently on screen.
pub fn render(
    frame: &mut Frame<'_>,
    state: &AppState,
    theme: &Theme,
    ctx: &RenderContext<'_>,
    hits: &mut HitMap,
) {
    let area = frame.area();
    frame.render_widget(
        ratatui::widgets::Block::new().style(theme.on_background()),
        area,
    );
    let mode = layout::mode_for(state.size.0, state.size.1);
    if mode == LayoutMode::TooSmall {
        render_too_small(frame, area, state, theme);
        // The modals still open even in too-small terminals: failures and
        // confirmations must stay visible and recoverable (plan §12/§14).
        components::error_modal::render(frame, state, theme, hits);
        components::confirm_modal::render(frame, state, theme, hits);
        components::attachment_dialog::render(frame, state, theme);
        components::theme_picker::render(frame, state, theme);
        return;
    }
    let (topbar, body, statusbar) = layout::split_vertical(area);
    components::topbar::render(frame, topbar, state, theme, &ctx.clock, hits);
    let (sidebar, list) = layout::split_body(mode, body);
    if let Some(sidebar) = sidebar {
        // The mockup's `.sidebar` carries a `border-right` hairline (list,
        // viewer, and new-mail alike). The divider column is shaved off the
        // sidebar so folder rows never draw under it; the list area (and
        // with it every width computation the reducer relies on) is
        // untouched.
        let divider = Rect {
            x: sidebar.x + sidebar.width - 1,
            y: sidebar.y,
            width: 1,
            height: sidebar.height,
        };
        let mut inset = sidebar;
        inset.width -= 1;
        components::sidebar::render(frame, inset, state, theme, hits);
        frame.render_widget(
            Block::default()
                .borders(Borders::RIGHT)
                .border_style(theme.hairline()),
            divider,
        );
    }
    // The reader screen replaces the message list; the sidebar and topbar
    // chrome stay (mockup `viewer.html`). The composer likewise replaces
    // the list (mockup `new-mail.html`).
    if matches!(state.active_route(), Some(Route::Message(_))) {
        screens::reader::render(frame, list, state, theme, hits);
    } else if matches!(state.active_route(), Some(Route::Composer)) {
        screens::composer::render(frame, list, state, theme, hits);
    } else {
        screens::mailbox::render(frame, list, state, mode, theme, ctx.now, hits);
    }
    components::statusbar::render(frame, statusbar, state, theme, hits);
    components::error_modal::render(frame, state, theme, hits);
    components::confirm_modal::render(frame, state, theme, hits);
    components::attachment_dialog::render(frame, state, theme);
    components::theme_picker::render(frame, state, theme);
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
