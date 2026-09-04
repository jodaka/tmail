//! Responsive layout computation (plan §18).
//!
//! Reference size is 152×40 (mockup title bar). Three modes:
//! - **Full** (≥120 cols): sidebar + primary and secondary metadata.
//! - **Compact** (90–119 cols): sidebar hidden, snippet/tags hidden.
//! - **Too small**: a clear minimum-size message instead of overlapping
//!   widgets.

use ratatui::layout::{Constraint, Layout, Rect};

use crate::config::ViewMode;

/// Minimum width for the full layout.
pub const FULL_MIN_WIDTH: u16 = 120;
/// Minimum width for the compact layout.
pub const COMPACT_MIN_WIDTH: u16 = 90;
/// Minimum heights (the 152×40 reference leaves ample list rows).
pub const FULL_MIN_HEIGHT: u16 = 24;
pub const COMPACT_MIN_HEIGHT: u16 = 20;
/// Sidebar width (mockup: 232 px ≈ 24 terminal columns).
pub const SIDEBAR_WIDTH: u16 = 24;

/// Heights of the fixed chrome regions.
pub const TOPBAR_HEIGHT: u16 = 4;
pub const STATUSBAR_HEIGHT: u16 = 3;
pub const LIST_HEAD_HEIGHT: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    Full,
    Compact,
    TooSmall,
}

pub fn mode_for(width: u16, height: u16) -> LayoutMode {
    if width >= FULL_MIN_WIDTH && height >= FULL_MIN_HEIGHT {
        LayoutMode::Full
    } else if width >= COMPACT_MIN_WIDTH && height >= COMPACT_MIN_HEIGHT {
        LayoutMode::Compact
    } else {
        LayoutMode::TooSmall
    }
}

/// Vertical chrome split: topbar / body / statusbar.
pub fn split_vertical(area: Rect) -> (Rect, Rect, Rect) {
    let [topbar, body, statusbar] = Layout::vertical([
        Constraint::Length(TOPBAR_HEIGHT),
        Constraint::Min(0),
        Constraint::Length(STATUSBAR_HEIGHT),
    ])
    .areas(area);
    (topbar, body, statusbar)
}

/// Horizontal body split: sidebar / list. `sidebar` is `None` in compact.
pub fn split_body(mode: LayoutMode, area: Rect) -> (Option<Rect>, Rect) {
    match mode {
        LayoutMode::Full => {
            let [sidebar, list] =
                Layout::horizontal([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(0)])
                    .areas(area);
            (Some(sidebar), list)
        }
        _ => (None, area),
    }
}

/// List split: header / rows.
pub fn split_list(area: Rect) -> (Rect, Rect) {
    let [head, rows] =
        Layout::vertical([Constraint::Length(LIST_HEAD_HEIGHT), Constraint::Min(0)]).areas(area);
    (head, rows)
}

/// Number of messages the list area shows for a terminal `size` under a
/// view mode, computed with the exact same layout functions the renderer
/// uses. The reducer consumes this to keep the selection on screen across
/// movement, page loads, and resize (Phase 2 acceptance) and to size
/// auto-sized pages (ticket kjfq); `0` means the list is not drawn at all.
/// Comfortable view mode interleaves a faint separator under every row, so
/// each message costs [`ViewMode::row_height`] lines and fewer fit.
pub fn messages_visible(size: (u16, u16), view_mode: ViewMode) -> usize {
    let area = Rect {
        x: 0,
        y: 0,
        width: size.0,
        height: size.1,
    };
    let mode = mode_for(area.width, area.height);
    if mode == LayoutMode::TooSmall {
        return 0;
    }
    let (_, body, _) = split_vertical(area);
    let (_, list) = split_body(mode, body);
    let (_, rows) = split_list(list);
    rows.height as usize / view_mode.row_height()
}

/// Height of the reader viewport (the whole body area: the reader document
/// replaces list head and rows, mockup `viewer.html` `.reader`). The
/// reducer's scroll clamp uses this with the reader's content line count.
pub fn reader_rows_visible(size: (u16, u16)) -> usize {
    let area = Rect {
        x: 0,
        y: 0,
        width: size.0,
        height: size.1,
    };
    let mode = mode_for(area.width, area.height);
    if mode == LayoutMode::TooSmall {
        return 0;
    }
    let (_, body, _) = split_vertical(area);
    let (_, list) = split_body(mode, body);
    list.height as usize
}

/// Width of the reader viewport (body area minus the sidebar in full mode).
/// The reader's content function and renderer must agree on this.
pub fn reader_width(size: (u16, u16)) -> usize {
    let area = Rect {
        x: 0,
        y: 0,
        width: size.0,
        height: size.1,
    };
    let mode = mode_for(area.width, area.height);
    if mode == LayoutMode::TooSmall {
        return 0;
    }
    let (_, body, _) = split_vertical(area);
    let (_, list) = split_body(mode, body);
    list.width as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_boundaries() {
        // Reference full size.
        assert_eq!(mode_for(152, 40), LayoutMode::Full);
        assert_eq!(mode_for(120, 24), LayoutMode::Full);
        // Compact band; height below the full floor drops to compact first.
        assert_eq!(mode_for(119, 40), LayoutMode::Compact);
        assert_eq!(mode_for(90, 20), LayoutMode::Compact);
        assert_eq!(mode_for(100, 30), LayoutMode::Compact);
        assert_eq!(mode_for(120, 23), LayoutMode::Compact);
        // Too small: width or height below the compact floor.
        assert_eq!(mode_for(89, 40), LayoutMode::TooSmall);
        assert_eq!(mode_for(152, 19), LayoutMode::TooSmall);
        assert_eq!(mode_for(80, 15), LayoutMode::TooSmall);
        assert_eq!(mode_for(0, 0), LayoutMode::TooSmall);
    }

    #[test]
    fn messages_visible_matches_chrome_budget() {
        use crate::config::ViewMode;
        // 40 rows − topbar 4 − statusbar 3 − list head 2 = 31.
        assert_eq!(messages_visible((152, 40), ViewMode::Compact), 31);
        // Mode does not change the row height, only the sidebar width.
        assert_eq!(messages_visible((100, 30), ViewMode::Compact), 21);
        // Too small renders no list.
        assert_eq!(messages_visible((80, 15), ViewMode::Compact), 0);
    }

    #[test]
    fn comfortable_view_mode_halves_the_visible_messages() {
        use crate::config::ViewMode;
        // Each comfortable message costs content + separator = 2 lines, so
        // a 31-line list shows 15 messages (the trailing line stays blank
        // rather than clipping a row block).
        assert_eq!(messages_visible((152, 40), ViewMode::Comfortable), 15);
        assert_eq!(messages_visible((100, 30), ViewMode::Comfortable), 10);
        assert_eq!(messages_visible((80, 15), ViewMode::Comfortable), 0);
    }
}
