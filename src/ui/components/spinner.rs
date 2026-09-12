//! Loaders (ticket m3by): one loader implementation everywhere — the
//! opencode Knight Rider scanner, "blocks" style. The top bar renders it
//! on the row below the program name/version while foreground work is in
//! flight; pane loaders (mailboxes, message list, message body) render
//! the very same scanner centered in the pane. All phases are wall-clock
//! driven, so the pace never drifts with the tick counter.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::state::AppState;
use crate::ui::theme::Theme;

// ── Knight Rider scanner (opencode TUI loader, "blocks" style) ────────────
//
// A bidirectional scan: a bright leading block sweeps left-to-right, brie
// ffly holds, sweeps back, and holds at the start. The trail decays in
// alpha behind the head; inactive blocks render as faint dots. Ported from
// opencode `packages/tui/src/ui/spinner.ts` (`createFrames` with
// `style: "blocks"`).

/// Scanner width in blocks.
const KR_WIDTH: i32 = 8;
/// Trail colors behind the head.
const KR_TRAIL_STEPS: usize = 6;
const KR_MIN_ALPHA: f64 = 0.3;
const KR_INACTIVE_FACTOR: f64 = 0.6;
/// Hold lengths. opencode holds for 30/9 frames at 40 ms apiece; tmail
/// ticks at 250 ms, so the holds are scaled down to keep the same tempo.
const KR_HOLD_START: i32 = 4;
const KR_HOLD_END: i32 = 2;

/// Total frames in one bidirectional cycle.
const KR_FRAMES: u64 = (KR_WIDTH + KR_HOLD_END + (KR_WIDTH - 1) + KR_HOLD_START) as u64;

/// One scanner position: where the lead block is, how far into a hold it
/// is, and the sweep direction (opencode `getScannerState`).
struct ScanState {
    pos: i32,
    holding: bool,
    hold_progress: i32,
    hold_total: i32,
    forward: bool,
    movement_progress: i32,
    movement_total: i32,
}

fn scanner_state(frame_index: i32) -> ScanState {
    let forward_frames = KR_WIDTH;
    let if_holding = |pos: i32, progress: i32, forward: bool| ScanState {
        pos,
        holding: true,
        hold_progress: progress,
        hold_total: if forward { KR_HOLD_END } else { KR_HOLD_START },
        forward,
        movement_progress: 0,
        movement_total: 0,
    };
    if frame_index < forward_frames {
        ScanState {
            pos: frame_index,
            holding: false,
            hold_progress: 0,
            hold_total: 0,
            forward: true,
            movement_progress: frame_index,
            movement_total: forward_frames,
        }
    } else if frame_index < forward_frames + KR_HOLD_END {
        if_holding(KR_WIDTH - 1, frame_index - forward_frames, true)
    } else if frame_index < forward_frames + KR_HOLD_END + KR_WIDTH - 1 {
        let backward_index = frame_index - forward_frames - KR_HOLD_END;
        ScanState {
            pos: KR_WIDTH - 2 - backward_index,
            holding: false,
            hold_progress: 0,
            hold_total: 0,
            forward: false,
            movement_progress: backward_index,
            movement_total: KR_WIDTH - 1,
        }
    } else {
        let progress = frame_index - forward_frames - KR_HOLD_END - (KR_WIDTH - 1);
        if_holding(0, progress, false)
    }
}

/// Color index for one cell (`-1` = inactive, opencode
/// `calculateColorIndex`).
fn color_index(cell: i32, state: &ScanState) -> i32 {
    let distance = if state.forward {
        state.pos - cell
    } else {
        cell - state.pos
    };
    if state.holding {
        distance + state.hold_progress
    } else if distance == 0 {
        0
    } else if distance > 0 && (distance as usize) < KR_TRAIL_STEPS {
        distance
    } else {
        -1
    }
}

/// Global fade factor for the inactive dots (opencode's movement/hold
/// fading): trail fades out while holding, fades back in while moving.
fn inactive_fade(state: &ScanState) -> f64 {
    if state.holding && state.hold_total > 0 {
        let progress = (state.hold_progress as f64 / state.hold_total as f64).min(1.0);
        (1.0 - progress * (1.0 - KR_MIN_ALPHA)).max(KR_MIN_ALPHA)
    } else if !state.holding && state.movement_total > 0 {
        let progress = state.movement_progress as f64 / (state.movement_total - 1).max(1) as f64;
        KR_MIN_ALPHA + progress * (1.0 - KR_MIN_ALPHA)
    } else {
        1.0
    }
}

/// Blend `color` over `bg` at `alpha` (1.0 keeps `color`).
fn blend(color: (u8, u8, u8), alpha: f64, bg: (u8, u8, u8)) -> Color {
    let mix = |c: u8, b: u8| {
        (c as f64 * alpha + b as f64 * (1.0 - alpha))
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color::Rgb(mix(color.0, bg.0), mix(color.1, bg.1), mix(color.2, bg.2))
}

/// Brightness-scale an RGB triple by `factor` (opencode's bloom: the second
/// trail step brightens the base color while slightly reducing opacity).
fn brighten(color: (u8, u8, u8), factor: f64) -> (u8, u8, u8) {
    let scale = |c: u8| (c as f64 * factor).min(255.0) as u8;
    (scale(color.0), scale(color.1), scale(color.2))
}

/// The trail colors behind the head (opencode `deriveTrailColors`)
/// composited over `bg`: full color at the head, a 1.15× brightened bloom
/// at step 1, then exponential 0.65 alpha decay.
fn trail_colors(base: (u8, u8, u8), bg: (u8, u8, u8)) -> Vec<Color> {
    let alphas: [f64; KR_TRAIL_STEPS] = core::array::from_fn(|i| {
        if i == 0 {
            1.0
        } else if i == 1 {
            0.9
        } else {
            0.65_f64.powi(i as i32 - 1)
        }
    });
    (0..KR_TRAIL_STEPS)
        .map(|i| {
            let color = if i == 1 { brighten(base, 1.15) } else { base };
            blend(color, alphas[i], bg)
        })
        .collect()
}

/// One scanner frame every 80 ms (opencode's original prompt spinner runs
/// 40 ms; this reads at half of that — quick but not frantic).
const KR_FRAME_MS: u64 = 80;

/// Render the scanner into `area` (opencode prompt spinner: width 8,
/// blocks style, lead + alpha-faded trail over faint dots). The phase is
/// wall-clock driven: `now_millis` (a `DateTime::timestamp_millis()`) is
/// divided by the frame duration, so the animation runs at opencode's
/// tempo regardless of the tick counter's cadence.
///
/// The trail is blended toward `bg`, so the caller passes the fill that
/// actually sits behind the scanner (`sidebar_bg` under the logo, the
/// page background in the panes). Palettes that cannot name a color (the
/// no-color theme's `Color::Reset`) skip the blending entirely and render
/// plain glyphs — the terminal default foreground, dimmed — so the
/// animation survives without inventing colors.
pub fn render_blocks(frame: &mut Frame<'_>, area: Rect, base: Color, bg: Color, now_millis: u64) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let as_rgb = |color: Color| match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    };
    // The trail math needs real RGB on both ends; anything else (the
    // no-color theme) renders fg-only instead.
    let palette = match (as_rgb(base), as_rgb(bg)) {
        (Some(base), Some(bg)) => Some((base, bg, trail_colors(base, bg))),
        _ => None,
    };
    // Wall-clock phase: the tick counter only drives how often the frame
    // redraws, the elapsed milliseconds pick the scanner frame.
    let state = scanner_state((now_millis / KR_FRAME_MS % KR_FRAMES) as i32);
    let inactive_alpha = KR_INACTIVE_FACTOR * inactive_fade(&state);
    let mut spans: Vec<Span<'_>> = Vec::with_capacity(KR_WIDTH as usize);
    for cell in 0..KR_WIDTH {
        let index = color_index(cell, &state);
        let (glyph, style) = match &palette {
            // Colored trail: full composite colors, painted over the
            // caller's background.
            Some((base, bg, trail)) if index >= 0 && (index as usize) < KR_TRAIL_STEPS => (
                "■",
                Style::new()
                    .fg(trail[index as usize])
                    .bg(Color::Rgb(bg.0, bg.1, bg.2)),
            ),
            Some((base, bg, _)) => (
                "⬝",
                Style::new()
                    .fg(blend(*base, inactive_alpha, *bg))
                    .bg(Color::Rgb(bg.0, bg.1, bg.2)),
            ),
            // No-color palette: plain glyphs, dimmed trail and dots —
            // default foreground carries the animation by shape.
            None if index >= 0 => {
                if index == 0 {
                    ("■", Style::new())
                } else {
                    ("▪", Style::new().add_modifier(Modifier::DIM))
                }
            }
            None => ("⬝", Style::new().add_modifier(Modifier::DIM)),
        };
        spans.push(Span::styled(glyph, style));
    }
    let loader_area = Rect {
        y: area.y,
        width: area.width.min(KR_WIDTH as u16),
        height: area.height.min(1),
        ..area
    };
    frame.render_widget(Paragraph::new(Line::from(spans)), loader_area);
}

/// Wall-clock milliseconds from the reducer's tick clock: what the pane
/// loaders animate against.
pub fn pane_millis(state: &AppState) -> u64 {
    state
        .session
        .clock
        .map(|now| now.timestamp_millis().max(0) as u64)
        .unwrap_or(0)
}

/// Render the loader centered in `area` (ticket m3by: every pane loader
/// looks the same — the scanner, centered in the panel on the panel's own
/// colors). The top bar renders the very same scanner (see
/// [`render_blocks`]); this is only geometry on top of it.
pub fn render_centered(frame: &mut Frame<'_>, area: Rect, theme: &Theme, now_millis: u64) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let width = area.width.min(KR_WIDTH as u16);
    let height = area.height.min(1);
    let cell = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    render_blocks(frame, cell, theme.accent, theme.background, now_millis);
}
