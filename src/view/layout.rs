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
/// Sidebar width (mockup: 232 px ≈ 24 terminal columns, plus one
/// right-margin column the folder rows never draw into).
pub const SIDEBAR_WIDTH: u16 = 25;

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

/// A horizontally centered dialog rectangle: the shared anatomy of the
/// modal `layout()` functions (`x = (size.0 - width) / 2`, and so on).
/// The rectangle never exceeds the terminal: oversized dimensions clamp to
/// the terminal first, so callers may pass raw caps.
pub fn centered(size: (u16, u16), width: u16, height: u16) -> Rect {
    let width = width.min(size.0.max(1));
    let height = height.min(size.1.max(1));
    Rect {
        x: size.0.saturating_sub(width) / 2,
        y: size.1.saturating_sub(height) / 2,
        width,
        height,
    }
}

/// The list pane's rectangle for a terminal `size`: the shared prologue of
/// the three visible-viewport helpers — origin rect, mode selection, and
/// the TooSmall bail (`None` means the pane is not drawn at all); then the
/// vertical chrome split and the sidebar split.
fn list_rect(size: (u16, u16)) -> Option<(Rect, LayoutMode)> {
    let area = Rect {
        x: 0,
        y: 0,
        width: size.0,
        height: size.1,
    };
    let mode = mode_for(area.width, area.height);
    if mode == LayoutMode::TooSmall {
        return None;
    }
    let (_, body, _) = split_vertical(area);
    let (_, list) = split_body(mode, body);
    Some((list, mode))
}

/// Number of messages the list area shows for a terminal `size` under a
/// view mode, computed with the exact same layout functions the renderer
/// uses. The reducer consumes this to keep the selection on screen across
/// movement, page loads, and resize (Phase 2 acceptance) and to size
/// auto-sized pages (ticket kjfq); `0` means the list is not drawn at all.
/// Comfortable view mode interleaves a faint separator under every row, so
/// each message costs [`ViewMode::row_height`] lines and fewer fit.
pub fn messages_visible(size: (u16, u16), view_mode: ViewMode) -> usize {
    let Some((list, _)) = list_rect(size) else {
        return 0;
    };
    let (_, rows) = split_list(list);
    rows.height as usize / view_mode.row_height()
}

/// Height of the reader viewport (the whole body area: the reader document
/// replaces list head and rows, mockup `viewer.html` `.reader`). The
/// reducer's scroll clamp uses this with the reader's content line count.
pub fn reader_rows_visible(size: (u16, u16)) -> usize {
    list_rect(size).map_or(0, |(list, _)| list.height as usize)
}

/// Width of the reader viewport (body area minus the sidebar in full mode).
/// The reader's content function and renderer must agree on this. Clamped
/// to at least 10 columns so a degenerate-width terminal still yields a
/// usable document wrap width — the single policy (was spread as a
/// `.max(10)` over every call site).
pub fn reader_width(size: (u16, u16)) -> usize {
    const MIN_READER_WIDTH: usize = 10;
    list_rect(size)
        .map_or(0, |(list, _)| list.width as usize)
        .max(MIN_READER_WIDTH)
}

/// Rows visible in the sidebar pane at `size` (the body region under the
/// top bar, above the status bar — the sidebar and the list pane share
/// the same body rectangle). The blank separator row before the label
/// group is one of them: the sidebar's scroll window counts visual rows,
/// separator included, so the bookkeeping matches what is drawn (ticket
/// 6t30). `0` means the sidebar is not drawn (compact/too small).
pub fn sidebar_visible_rows(size: (u16, u16)) -> usize {
    list_rect(size)
        .filter(|(_, mode)| *mode == LayoutMode::Full)
        .map_or(0, |(list, _)| list.height as usize)
}

/// The visual row a mailbox occupies: the blank separator row the label
/// group reserves sits in the visual sequence before the first label, so
/// mailboxes from there on shift down by one (the renderer's window and
/// the reducer's cursor bookkeeping share this, ticket 6t30).
pub fn sidebar_visual_row(mailbox_index: usize, first_label: Option<usize>) -> usize {
    match first_label {
        Some(first) if first > 0 && mailbox_index >= first => mailbox_index + 1,
        _ => mailbox_index,
    }
}

/// Total visual rows the sidebar draws for `mailbox_count` mailboxes:
/// every mailbox plus the one separator row, when a label group follows
/// other rows.
pub fn sidebar_total_rows(mailbox_count: usize, first_label: Option<usize>) -> usize {
    if mailbox_count == 0 {
        return 0;
    }
    sidebar_visual_row(mailbox_count - 1, first_label) + 1
}

/// Largest valid `sidebar_scroll` for `mailbox_count` at `size` (what the
/// reducer clamps against): the last visual window of the row sequence.
/// `0` when the sidebar is not drawn.
pub fn sidebar_max_scroll(
    mailbox_count: usize,
    first_label: Option<usize>,
    size: (u16, u16),
) -> usize {
    let visible = sidebar_visible_rows(size);
    if visible == 0 {
        return 0;
    }
    sidebar_total_rows(mailbox_count, first_label).saturating_sub(visible)
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

    #[test]
    fn sidebar_rows_count_the_separator() {
        // Reference terminal: 40 − topbar 4 − statusbar 3 = 33 rows.
        assert_eq!(sidebar_visible_rows((152, 40)), 33);
        // No sidebar in compact/too-small sizes.
        assert_eq!(sidebar_visible_rows((100, 30)), 0);
        assert_eq!(sidebar_visible_rows((80, 15)), 0);
    }

    #[test]
    fn sidebar_visual_rows_shift_after_the_separator() {
        // Folders first (indices 0..2), first label at index 2: the
        // separator occupies visual row 2, labels shift down by one.
        let first_label = Some(2);
        assert_eq!(sidebar_visual_row(0, first_label), 0);
        assert_eq!(sidebar_visual_row(1, first_label), 1);
        assert_eq!(sidebar_visual_row(2, first_label), 3);
        assert_eq!(sidebar_visual_row(4, first_label), 5);
        // The separator only exists when a label group follows other
        // rows: a first-label mailbox at index 0 draws no separator.
        assert_eq!(sidebar_visual_row(0, Some(0)), 0);
        assert_eq!(sidebar_total_rows(3, Some(0)), 3);
        // Labels only, or no labels at all, keep mailboxes contiguous.
        assert_eq!(sidebar_total_rows(3, None), 3);
        assert_eq!(sidebar_total_rows(4, first_label), 5);
        assert_eq!(sidebar_total_rows(0, None), 0);
    }

    #[test]
    fn sidebar_max_scroll_walks_the_visual_rows() {
        let first_label = Some(2);
        // 4 mailboxes → 5 visual rows; 33 visible → no scrolling.
        assert_eq!(sidebar_max_scroll(4, first_label, (152, 40)), 0);
        // 40 mailboxes → 41 visual rows; the last window starts at 8.
        assert_eq!(sidebar_max_scroll(40, first_label, (152, 40)), 8);
        // Degenerate sizes clamp to 0.
        assert_eq!(sidebar_max_scroll(4, first_label, (80, 15)), 0);
    }
}
