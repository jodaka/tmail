//! Shared overlay geometry (plan §12/§18): the error modal's and theme
//! picker's sizes and scroll clamps. The reducer and the renderers both
//! consume these, so a clamp can never disagree with what is drawn.

use ratatui::layout::Rect;

use crate::view::layout::centered;
use crate::view::text::wrap;

/// Rows the theme picker shows before it starts scrolling.
const MAX_VISIBLE_ROWS: usize = 10;

/// Rows the account switcher shows before it starts scrolling.
const MAX_SWITCHER_ROWS: usize = 10;

/// Rows the Mailboxes popup shows before it starts scrolling.
const MAX_MAILBOXES_ROWS: usize = 10;

/// Error-modal width: columns shaved off the terminal before the
/// min/max clamp (a wide margin so the dialog never touches the edges),
/// then the size window. The minimum keeps the buttons readable in tiny
/// terminals; the maximum keeps long details wrapped instead of running
/// the full width of a huge window.
const ERROR_MODAL_WIDTH_MARGIN: u16 = 10;
const ERROR_MODAL_MIN_WIDTH: u16 = 24;
const ERROR_MODAL_MAX_WIDTH: u16 = 76;

/// Error-modal height: rows shaved off the terminal before the min/max
/// clamp — the modal floats with room above and below, grows with the
/// terminal up to the cap (long details scroll, they never stretch the
/// dialog), and the minimum keeps title, one detail row, buttons, and
/// hint visible.
const ERROR_MODAL_HEIGHT_MARGIN: u16 = 6;
const ERROR_MODAL_MIN_HEIGHT: u16 = 9;
const ERROR_MODAL_MAX_HEIGHT: u16 = 18;

/// Theme picker dialog width: fits the longest theme label and its
/// preview swatch with room for the border (mockup).
const PICKER_WIDTH: u16 = 34;

/// Account switcher dialog width (ticket c0n0): wider than the theme
/// picker because rows carry the account's email.
const SWITCHER_WIDTH: u16 = 46;

/// Mailboxes popup width (issue brnw): folder row (name + unread
/// counter) plus border, inside the 90-column compact terminal floor.
const MAILBOXES_POPUP_WIDTH: u16 = 40;

/// Shortcuts help dialog width: fits the longest label/key pair without
/// clipping (the help is read, not navigated).
const HELP_WIDTH: u16 = 46;

/// Account-switch confirm dialog width (ticket c0n0): fits its longest
/// prose line without wrapping.
const SWITCH_CONFIRM_WIDTH: u16 = 56;

/// Attachment chooser error viewport: rows shaved off the dialog height
/// before the min/max clamp (cwd row, list floor, blank separator, and
/// the borders/margins; see [`attachment_error_viewport`]).
const ATTACHMENT_ERROR_VIEWPORT_MARGIN: usize = 7;
const ATTACHMENT_ERROR_MIN_HEIGHT: u16 = 12;
const ATTACHMENT_ERROR_MAX_HEIGHT: u16 = 30;

/// Attachment chooser dialog width: three quarters of the terminal,
/// clamped so the file table stays readable on both ends (ticket 6t30).
const ATTACHMENT_DIALOG_MIN_WIDTH: u16 = 46;
const ATTACHMENT_DIALOG_MAX_WIDTH: u16 = 96;

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
/// The height budgets the border and the one-line margins `modal_frame`
/// keeps above and below the content. `more_failures` is the queued
/// "and N more failed" count (issue 8859); any nonzero count adds one
/// fixed warning row between the detail and the buttons.
pub fn error_modal_layout(
    size: (u16, u16),
    code: Option<i32>,
    ambiguous: bool,
    more_failures: usize,
) -> ModalLayout {
    let width = size
        .0
        .saturating_sub(ERROR_MODAL_WIDTH_MARGIN)
        .clamp(ERROR_MODAL_MIN_WIDTH, ERROR_MODAL_MAX_WIDTH)
        .min(size.0.max(1));
    let height = size
        .1
        .saturating_sub(ERROR_MODAL_HEIGHT_MARGIN)
        .clamp(ERROR_MODAL_MIN_HEIGHT, ERROR_MODAL_MAX_HEIGHT)
        .min(size.1.max(1));
    let area = centered(size, width, height);
    // Inside borders+margins: [code?] [warning?] [more?] [detail…] [buttons] [hint].
    let content_height = height.saturating_sub(4) as usize;
    let fixed =
        2 + usize::from(code.is_some()) + usize::from(ambiguous) + usize::from(more_failures > 0);
    let viewport_lines = content_height.saturating_sub(fixed).max(1);
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
    more_failures: usize,
) -> Vec<String> {
    let layout = error_modal_layout(size, code, ambiguous, more_failures);
    wrap(detail, layout.detail_width)
}

/// Largest valid scroll offset for one error detail at `size` (what the
/// reducer clamps against).
pub fn error_modal_max_scroll(
    detail: &str,
    code: Option<i32>,
    ambiguous: bool,
    more_failures: usize,
    size: (u16, u16),
) -> usize {
    let lines = error_detail_lines(detail, size, code, ambiguous, more_failures);
    let layout = error_modal_layout(size, code, ambiguous, more_failures);
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
/// terminal and keeps at least one theme row even in tiny terminals. The
/// height budgets the borders, the one-line margins `modal_frame` keeps
/// above and below the content, and the hint row.
pub fn picker_layout(size: (u16, u16), theme_count: usize) -> PickerLayout {
    let width = PICKER_WIDTH.min(size.0.max(1));
    let height = ((theme_count.min(MAX_VISIBLE_ROWS) as u16) + 5).min(size.1.max(1));
    let visible_rows = height.saturating_sub(5).max(1) as usize;
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

/// Geometry of the account switcher (ticket c0n0) for one terminal size
/// and account count. Same anatomy as the theme picker — shared
/// `PickerLayout`, so the reducer's clamp and the drawn window cannot
/// drift apart — but a wider dialog: rows carry the account's email.
/// Height budgets the borders, the one-line margins, and the hint row.
pub fn switcher_layout(size: (u16, u16), account_count: usize) -> PickerLayout {
    let width = SWITCHER_WIDTH.min(size.0.max(1));
    let height = ((account_count.min(MAX_SWITCHER_ROWS) as u16) + 5).min(size.1.max(1));
    let visible_rows = height.saturating_sub(5).max(1) as usize;
    PickerLayout {
        area: centered(size, width, height),
        visible_rows,
    }
}

/// Rows visible in the account switcher at `size` (what the reducer keeps
/// the cursor and scroll inside), independent of the account count cap.
pub fn switcher_visible_rows(size: (u16, u16)) -> usize {
    switcher_layout(size, usize::MAX).visible_rows
}

/// Largest valid scroll offset for the account list at `size` (what the
/// reducer clamps against).
pub fn switcher_max_scroll(account_count: usize, size: (u16, u16)) -> usize {
    account_count.saturating_sub(switcher_visible_rows(size))
}

/// Geometry of the Mailboxes popup (issue brnw) for one terminal size and
/// mailbox count. Same anatomy as the account switcher — shared
/// `PickerLayout`, so the reducer's clamp and the drawn window cannot
/// drift apart — but a narrower dialog: rows are the sidebar's folder
/// rows (name + unread counter), and the compact terminal it serves is
/// at least 90 columns wide. Height budgets the borders, the one-line
/// margins, and the hint row.
pub fn mailboxes_layout(size: (u16, u16), mailbox_count: usize) -> PickerLayout {
    let width = MAILBOXES_POPUP_WIDTH.min(size.0.max(1));
    let height = ((mailbox_count.min(MAX_MAILBOXES_ROWS) as u16) + 5).min(size.1.max(1));
    let visible_rows = height.saturating_sub(5).max(1) as usize;
    PickerLayout {
        area: centered(size, width, height),
        visible_rows,
    }
}

/// Rows visible in the Mailboxes popup at `size` (what the reducer keeps
/// the cursor and scroll inside), independent of the mailbox count cap.
pub fn mailboxes_visible_rows(size: (u16, u16)) -> usize {
    mailboxes_layout(size, usize::MAX).visible_rows
}

/// Largest valid scroll offset for the mailbox list at `size` (what the
/// reducer clamps against).
pub fn mailboxes_max_scroll(mailbox_count: usize, size: (u16, u16)) -> usize {
    mailbox_count.saturating_sub(mailboxes_visible_rows(size))
}

/// Width of the attachment chooser for one terminal size: the width half
/// of the dialog's `layout`, shared so the wrapped error detail the
/// reducer scrolls is measured at exactly the width the renderer clips
/// to (ticket 6t30).
pub fn attachment_dialog_width(size: (u16, u16)) -> u16 {
    (size.0 * 3 / 4)
        .clamp(ATTACHMENT_DIALOG_MIN_WIDTH, ATTACHMENT_DIALOG_MAX_WIDTH)
        .min(size.0.max(1))
}

/// Rows the attachment chooser's error viewport shows at once (the status
/// strip grows from one row to this while an error is displayed; the
/// list above shrinks by the same amount and never drops below two
/// rows). Ticket 6t30: a wrapped detail used to lose every line after
/// the first.
pub fn attachment_error_viewport(size: (u16, u16)) -> usize {
    const MAX_ERROR_LINES: usize = 4;
    let height = (size.1 * 3 / 4)
        .clamp(ATTACHMENT_ERROR_MIN_HEIGHT, ATTACHMENT_ERROR_MAX_HEIGHT)
        .min(size.1.max(1)) as usize;
    // Inside borders + margins: cwd row + [list] + error rows + blank row.
    // At least one line shows even in a degenerate window; a larger
    // terminal shows no more than MAX_ERROR_LINES at once.
    height
        .saturating_sub(ATTACHMENT_ERROR_VIEWPORT_MARGIN)
        .clamp(1, MAX_ERROR_LINES)
}

/// Wrapped lines of the chooser's error detail (the same width the
/// renderer clips to), for the renderer and the reducer.
pub fn attachment_error_lines(detail: &str, size: (u16, u16)) -> Vec<String> {
    wrap(
        detail,
        attachment_dialog_width(size).saturating_sub(4).max(1) as usize,
    )
}

/// Largest valid `error_scroll` for one attachment detail at `size` (what
/// the reducer clamps against).
pub fn attachment_error_max_scroll(detail: &str, size: (u16, u16)) -> usize {
    let viewport = attachment_error_viewport(size);
    attachment_error_lines(detail, size)
        .len()
        .saturating_sub(viewport)
}

/// Geometry of the shortcuts help popup (user request) for one terminal
/// size and entry count. Same anatomy as the theme picker — height
/// budgets the borders, the one-line margins, and the hint slot — but
/// sized to the full entry table when it fits (the help is read, not
/// navigated); a short terminal clamps the dialog and the overflow
/// scrolls.
pub fn help_layout(size: (u16, u16), entry_count: usize) -> PickerLayout {
    let width = HELP_WIDTH.min(size.0.max(1));
    let height = (entry_count.min(1000) as u16)
        .saturating_add(5)
        .min(size.1.max(1));
    let visible_rows = height.saturating_sub(5).max(1) as usize;
    PickerLayout {
        area: centered(size, width, height),
        visible_rows,
    }
}

/// Rows visible in the shortcuts help at `size` (what the reducer keeps
/// the scroll offset inside), independent of the entry count.
pub fn help_visible_rows(size: (u16, u16)) -> usize {
    let height = size.1.max(1);
    height.saturating_sub(5).max(1) as usize
}

/// Largest valid `scroll` for the help entry table at `size` (what the
/// reducer clamps against).
pub fn help_max_scroll(entry_count: usize, size: (u16, u16)) -> usize {
    entry_count.saturating_sub(help_visible_rows(size))
}

/// Geometry of the account-switch confirm dialog (ticket c0n0): sized to
/// its content, centered, never exceeding the terminal — like the error
/// modal. `operations`/`unsaved` say which optional lines the dialog
/// carries (the frozen op summaries, the composer warning). The height
/// budgets the borders and the one-line margins.
pub fn switch_confirm_layout(size: (u16, u16), operations: bool, unsaved: bool) -> Rect {
    let width = SWITCH_CONFIRM_WIDTH.min(size.0.max(1));
    // Content: target line, [ops line], [unsaved line], blank, buttons,
    // hint; outer adds the borders and the margins.
    let inner = 4 + u16::from(operations) + u16::from(unsaved);
    let height = (inner + 4).min(size.1.max(1));
    centered(size, width, height)
}
