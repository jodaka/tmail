//! Rendering entry point: theme + responsive layout + chrome components.

pub mod chrome;
pub mod components;
pub use crate::view::dates;
pub use crate::view::layout;
pub mod screens;
pub use crate::view::text;
pub use crate::view::theme;

pub use theme::Theme;

use chrono::{DateTime, FixedOffset};
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::route::Route;
use crate::app::state::AppState;
use crate::input::mouse::HitMap;
use crate::ui::layout::LayoutMode;

/// Values injected at render time so neither the reducer nor snapshots
/// depend on the wall clock (plan §20).
pub struct RenderContext {
    /// Current time for the clock and relative dates.
    pub now: DateTime<FixedOffset>,
    /// Preformatted clock label (`Wed Sep 2 · 10:47`).
    pub clock: String,
}

impl RenderContext {
    pub fn new(now: DateTime<FixedOffset>, clock: String) -> Self {
        Self { now, clock }
    }
}

/// Render one frame. `hits` collects the interactive widget rectangles
/// (plan §10, Phase 10.1): the mouse layer hit-tests the next click
/// against the map of the frame currently on screen.
pub fn render(
    frame: &mut Frame<'_>,
    state: &AppState,
    theme: &Theme,
    ctx: &RenderContext,
    hits: &mut HitMap,
) {
    let area = frame.area();
    frame.render_widget(
        ratatui::widgets::Block::new().style(theme.on_background()),
        area,
    );
    // The account configuration wizard replaces the whole chrome (ADR 0003
    // §3.7): first run has no data for the shell to show, and the wizard
    // owns every key while active. No modals can be open underneath it.
    if state.session.wizard.is_some() {
        screens::wizard::render(frame, area, state, theme);
        return;
    }
    let mode = layout::mode_for(state.session.size.0, state.session.size.1);
    if mode == LayoutMode::TooSmall {
        render_too_small(frame, area, state, theme);
        // The modals still open even in too-small terminals: failures and
        // confirmations must stay visible and recoverable (plan §12/§14).
        render_modals(frame, state, theme, hits);
        return;
    }
    let (topbar, body, statusbar) = layout::split_vertical(area);
    // The loader animation reads elapsed wall clock (a
    // `DateTime::timestamp_millis()`), so the scanner phase does not
    // lag behind the tick cadence.
    let loader_millis = ctx.now.timestamp_millis().max(0) as u64;
    components::topbar::render(frame, topbar, state, theme, &ctx.clock, hits);
    let (sidebar, list) = layout::split_body(mode, body);
    if let Some(sidebar) = sidebar {
        // No hairline divider between the sidebar and the list (temporary
        // experiment): the sidebar paints its full width, the arrowless
        // border column included, and the list area is untouched.
        components::sidebar::render(frame, sidebar, state, theme, hits);
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
    components::statusbar::render(frame, statusbar, mode, state, theme, loader_millis, hits);
    render_modals(frame, state, theme, hits);
    components::help::render(frame, state, theme);
}

/// The always-on modal stack drawn above every screen (plan §12/§14): the
/// error modal, the confirm dialog, the attachment chooser, and the theme
/// picker. Help is normal-mode only and draws on top separately.
fn render_modals(frame: &mut Frame<'_>, state: &AppState, theme: &Theme, hits: &mut HitMap) {
    components::error_modal::render(frame, state, theme, hits);
    components::confirm_modal::render(frame, state, theme, hits);
    components::attachment_dialog::render(frame, state, theme);
    components::theme_picker::render(frame, state, theme);
}

/// Too-small mode: a clear centered message, nothing overlapping (plan §18).
fn render_too_small(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    chrome::render_too_small(
        frame,
        area,
        state.session.size,
        &format!(
            "tmail needs at least {}×{} columns/rows.",
            layout::COMPACT_MIN_WIDTH,
            layout::COMPACT_MIN_HEIGHT
        ),
        "Enlarge the window or press Esc to quit.",
        theme,
    );
}
