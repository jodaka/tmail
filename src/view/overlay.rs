//! Shared overlay geometry (plan §12/§18): the error modal's and theme
//! picker's sizes and scroll clamps. The reducer and the renderers both
//! consume these, so a clamp can never disagree with what is drawn.

use ratatui::layout::Rect;

use crate::view::layout::centered;
use crate::view::text::wrap;

/// Rows the theme picker shows before it starts scrolling.
const MAX_VISIBLE_ROWS: usize = 10;

/// Geometry of the error modal for one terminal size and dialog shape.
/// Shared by the renderer and the reducer's scroll clamping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModalLayout {
    /// Outer bordered rectangle.
    pub area: Rect,
    /// Display width available to wrapped detail lines.
    pub detail_width: usize,
    /// Number of detail lines visible at once.
    pub viewport_lines: usize,
}

/// Compute the modal geometry. The modal always fits: it shrinks to the
/// terminal and keeps at least one detail row even in tiny terminals.
pub fn error_modal_layout(size: (u16, u16), code: Option<i32>, ambiguous: bool) -> ModalLayout {
    let width = size.0.saturating_sub(10).clamp(24, 76).min(size.0.max(1));
    let height = size.1.saturating_sub(4).clamp(7, 18).min(size.1.max(1));
    let area = centered(size, width, height);
    // Inside the borders: [code?] [warning?] [detail…] [buttons] [hint].
    let inner_height = height.saturating_sub(2) as usize;
    let fixed = 2 + usize::from(code.is_some()) + usize::from(ambiguous);
    let viewport_lines = inner_height.saturating_sub(fixed).max(1);
    let detail_width = width.saturating_sub(4).max(1) as usize;
    ModalLayout {
        area,
        detail_width,
        viewport_lines,
    }
}

/// Lines of wrapped detail (width from the layout), for the renderer and
/// the reducer.
pub fn error_detail_lines(
    detail: &str,
    size: (u16, u16),
    code: Option<i32>,
    ambiguous: bool,
) -> Vec<String> {
    let layout = error_modal_layout(size, code, ambiguous);
    wrap(detail, layout.detail_width)
}

/// Largest valid scroll offset for one error detail at `size` (what the
/// reducer clamps against).
pub fn error_modal_max_scroll(
    detail: &str,
    code: Option<i32>,
    ambiguous: bool,
    size: (u16, u16),
) -> usize {
    let lines = error_detail_lines(detail, size, code, ambiguous);
    let layout = error_modal_layout(size, code, ambiguous);
    lines.len().saturating_sub(layout.viewport_lines)
}

/// Geometry of the picker for one terminal size and theme count. Shared by
/// the renderer and the reducer's cursor/scroll clamping, so the clamp the
/// reducer computes always matches what is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickerLayout {
    /// Outer bordered rectangle.
    pub area: Rect,
    /// Number of theme rows visible at once.
    pub visible_rows: usize,
}

/// Compute the picker geometry. The dialog always fits: it shrinks to the
/// terminal and keeps at least one theme row even in tiny terminals.
pub fn picker_layout(size: (u16, u16), theme_count: usize) -> PickerLayout {
    let width = 34u16.min(size.0.max(1));
    // Inside the borders: the theme rows plus one hint line.
    let height = ((theme_count.min(MAX_VISIBLE_ROWS) as u16) + 3).min(size.1.max(1));
    let visible_rows = height.saturating_sub(3).max(1) as usize;
    PickerLayout {
        area: centered(size, width, height),
        visible_rows,
    }
}

/// Rows visible in the picker at `size` (what the reducer keeps the cursor
/// and scroll inside), independent of the theme count cap.
pub fn picker_visible_rows(size: (u16, u16)) -> usize {
    picker_layout(size, usize::MAX).visible_rows
}

/// Largest valid scroll offset for the theme list at `size` (what the
/// reducer clamps against).
pub fn picker_max_scroll(theme_count: usize, size: (u16, u16)) -> usize {
    theme_count.saturating_sub(picker_visible_rows(size))
}
