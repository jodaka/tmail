//! The Mailboxes popup (issue brnw): compact mode's sidebar stand-in —
//! the mailbox listing the sidebar draws, in a centered modal. Rows reuse
//! the sidebar's folder-row look (active folder on the marker fill, the
//! popup's cursor on the selection fill). Arrows move the cursor, Enter
//! switches (or closes on the displayed mailbox), Esc closes. Like the
//! other pickers the popup is keyboard-first: no click targets.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use crate::app::overlay::Overlay;
use crate::app::state::{AppState, Loadable};
use crate::ui::chrome;
use crate::ui::components::sidebar;
use crate::ui::text;
use crate::ui::theme::Theme;

pub use crate::view::overlay::mailboxes_layout as layout;
pub use crate::view::overlay::mailboxes_max_scroll as max_scroll;
pub use crate::view::overlay::mailboxes_visible_rows as visible_rows;

/// Render the popup, when open, above everything already drawn.
pub fn render(frame: &mut Frame<'_>, state: &AppState, theme: &Theme) {
    let Some(Overlay::Mailboxes(dialog)) = &state.session.overlay else {
        return;
    };
    let count = state.mailboxes.as_loaded().map(Vec::len).unwrap_or(0);
    let layout = layout(state.session.size, count);
    if layout.area.width < 6 || layout.area.height < 3 {
        return;
    }
    let Some(inner) = chrome::modal_frame(
        frame,
        layout.area,
        &Span::raw(" Mailboxes "),
        theme.accent,
        theme,
    ) else {
        return;
    };
    let rows_height = layout.visible_rows as u16;
    // Pending and empty states mirror the sidebar's (plan §16: empty
    // results are valid, every pane loads with the same spinner look).
    // Nothing is choosable yet, so the hint stays off until rows exist.
    let Some(mailboxes) = state.mailboxes.as_loaded() else {
        match &state.mailboxes {
            Loadable::Failed(_) => {
                chrome::render_note(frame, inner, theme, "mailboxes unavailable");
            }
            _ => super::spinner::render_centered(
                frame,
                inner,
                theme,
                super::spinner::pane_millis(state),
            ),
        }
        return;
    };
    if mailboxes.is_empty() {
        chrome::render_note(frame, inner, theme, "(no mailboxes)");
        return;
    }
    // The mailbox the sidebar marks active: the popup marks it the same
    // way, so Enter on it reads as "close, nothing to do".
    let active_id = state.sidebar_active_mailbox_id();
    let visible = layout.visible_rows;
    let scrolling = mailboxes.len() > visible;
    let row_width = chrome::scrollbar_content_width(inner.width, mailboxes.len(), visible);
    let bottom = inner.y + rows_height;
    for (y, (index, mailbox)) in (inner.y..).zip(
        mailboxes
            .iter()
            .enumerate()
            .skip(dialog.scroll)
            .take(visible),
    ) {
        if y >= bottom {
            break;
        }
        let is_active = Some(&mailbox.id) == active_id;
        let cursor = index == dialog.cursor;
        let row_area = Rect {
            x: inner.x,
            y,
            width: row_width,
            height: 1,
        };
        // The rows are the sidebar's, on the popup's page background
        // (the modal frame already cleared and filled it).
        sidebar::render_folder_row(
            frame,
            row_area,
            theme,
            mailbox,
            is_active,
            cursor,
            theme.background,
        );
    }
    if scrolling {
        // The thumb tracks the scroll window the reducer keeps centered
        // on the cursor (position = first visible row).
        chrome::render_scrollbar(
            frame,
            Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: rows_height,
            },
            theme,
            mailboxes.len(),
            dialog.scroll,
        );
    }
    // The hint sits in the label slot one row below the rows — separated
    // from them by the content rect's last (blank) row, adjacent to the
    // bottom border (the shared modal anatomy).
    let hint = Rect {
        x: inner.x,
        y: inner.y + rows_height + 1,
        width: inner.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            text::clip("↑/↓ choose · ↵ switch · Esc cancel", inner.width as usize),
            Style::new().fg(theme.dim),
        )),
        hint,
    );
}
