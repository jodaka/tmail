//! Status bar: key hints, foreground-work spinner, environment info
//! (mockup `list.html`).
//!
//! v1 overrides applied (plan §4): no `j`/`k` move hints, no `?` help. The
//! mockup's mode badge was dropped: it named the screen the user is already
//! looking at, so it carried no information.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::action::{BulkOp, ClickTarget};
use crate::app::route::Route;
use crate::app::state::AppState;
use crate::input::keymap::Context;
use crate::input::mouse::HitMap;
use crate::ui::chrome::{self, HairlineSide};
use crate::ui::layout::{LayoutMode, SIDEBAR_WIDTH};
use crate::ui::theme::Theme;

/// Render the status bar into `area` (height 3: hairline + content row).
/// The foreground-work loader sits in the left corner (two spaces in from
/// the border, ticket m3by's scanner moved here from under the top-bar
/// logo); the hints start where the sidebar ends, lining the panel up
/// with the message list pane — full mode only (ticket c1d7: compact has
/// no sidebar, so there the hints start at the pane's left edge).
#[allow(clippy::too_many_arguments)]
pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    mode: LayoutMode,
    state: &AppState,
    theme: &Theme,
    loader_millis: u64,
    hits: &mut HitMap,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    // Panel fill, same as the sidebar: hints, hairline, and the blank
    // padding row all sit on it.
    frame.render_widget(Block::new().style(Style::new().bg(theme.sidebar_bg)), area);

    let hairline = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    chrome::hairline(frame, hairline, HairlineSide::Top, theme);

    let row = Rect {
        x: area.x,
        y: area.y.saturating_add(1),
        width: area.width,
        height: 1,
    };

    // Foreground work: the Knight Rider scanner in the left corner, two
    // spaces in from the border (the slot the top-bar logo's row used to
    // carry). The hints start at the sidebar's right edge, so the two
    // never meet.
    if state.session.operations.foreground().is_some() {
        super::spinner::render_blocks(
            frame,
            Rect {
                x: area.x.saturating_add(2),
                y: row.y,
                width: super::spinner::KR_WIDTH as u16,
                height: 1,
            },
            theme.accent3,
            theme.sidebar_bg,
            loader_millis,
        );
    }

    // Key hints follow the input contract (plan §10) and the active screen
    // (mockup `.statusbar` per layout). Deliberately no j/k, no help.
    // Selection mode replaces the hint row with the bulk-operation buttons
    // (ticket p0s3): the count and the four actions, each clickable.
    // Everything starts at the sidebar's right edge (aligned with the
    // message list pane); compact mode has no sidebar, so the offset
    // applies only in full mode (ticket c1d7).
    let hints_x = match mode {
        LayoutMode::Full => area.x.saturating_add(SIDEBAR_WIDTH),
        _ => area.x,
    };
    let row = Rect {
        x: hints_x,
        width: area.x + area.width.saturating_sub(hints_x),
        ..row
    };
    let reader = matches!(state.active_route(), Some(Route::Message(_)));
    let composer = matches!(state.active_route(), Some(Route::Composer));
    let selection_mode = state.selection_active() && !reader && !composer;
    let mut spans: Vec<Span<'_>> = Vec::new();
    if selection_mode {
        let count = format!(
            "  {} selected:",
            state.visible_selected_count().max(state.selected.len())
        );
        let mut cursor_x = row.x + count.width() as u16;
        spans.push(Span::styled(
            count,
            Style::new()
                .fg(theme.accent)
                .bg(theme.sidebar_bg)
                .add_modifier(ratatui::style::Modifier::BOLD),
        ));
        for (op, label) in [
            (BulkOp::Trash, "[delete]"),
            (BulkOp::Archive, "[archive]"),
            (BulkOp::MarkRead, "[read]"),
            (BulkOp::MarkUnread, "[unread]"),
        ] {
            let width = label.width() as u16 + 1;
            if cursor_x + width > area.x + area.width {
                break;
            }
            spans.push(Span::styled(
                format!(" {label}"),
                Style::new().fg(theme.text_soft).bg(theme.sidebar_bg),
            ));
            hits.push(
                Rect {
                    x: cursor_x,
                    y: row.y,
                    width,
                    height: 1,
                },
                ClickTarget::BulkAction(op),
            );
            cursor_x += width;
        }
        // The clear hint reads the `cancel` binding: rebinding it in the
        // config keeps this row honest (and an unbound cancel hides it).
        if let Some(clear) = state.settings.keymap.hint(None, "cancel") {
            spans.push(Span::styled(
                format!("  {clear} clear"),
                Style::new().fg(theme.muted).bg(theme.sidebar_bg),
            ));
        }
    } else if composer {
        // The composer's editing keys are not configurable (typing must
        // type); Tab and Esc are the keymap's focus/cancel bindings, so
        // their hints read from it. Send stays the fixed Ctrl+Enter chord.
        let hints: Vec<(Option<String>, &str)> = vec![
            (
                state
                    .settings
                    .keymap
                    .hint(None, "focus_next")
                    .map(String::from),
                "next field",
            ),
            (
                state.settings.keymap.hint(None, "cancel").map(String::from),
                "save & leave",
            ),
            (Some(String::from("^↵")), "send"),
        ];
        push_hints(theme, &mut spans, &hints);
    } else if reader {
        let context = Some(Context::Reader);
        let mut hints: Vec<(Option<String>, &str)> = vec![
            (state.settings.keymap.move_hint(context), "scroll"),
            (
                state
                    .settings
                    .keymap
                    .hint(context, "cancel")
                    .map(String::from),
                "back",
            ),
            (
                state
                    .settings
                    .keymap
                    .hint(context, "reply")
                    .map(String::from),
                "reply",
            ),
            (
                state
                    .settings
                    .keymap
                    .hint(context, "forward")
                    .map(String::from),
                "forward",
            ),
            (
                state
                    .settings
                    .keymap
                    .hint(context, "archive")
                    .map(String::from),
                "archive",
            ),
            (
                state
                    .settings
                    .keymap
                    .hint(context, "star")
                    .map(String::from),
                "star",
            ),
            (
                state
                    .settings
                    .keymap
                    .hint(context, "trash")
                    .map(String::from),
                "delete",
            ),
            (
                state
                    .settings
                    .keymap
                    .hint(context, "open_help")
                    .map(String::from),
                "shortcuts",
            ),
        ];
        // Attachment actions advertise only when the open message carries
        // attachments (plan §15, ticket 61qx): the chips are the target of
        // S/o and Enter, and an inert-looking button must say so.
        if state
            .open_message
            .as_loaded()
            .is_some_and(|message| !message.attachments.is_empty())
        {
            hints.push((
                state
                    .settings
                    .keymap
                    .hint(context, "save_attachment")
                    .map(String::from),
                "save",
            ));
            hints.push((
                state
                    .settings
                    .keymap
                    .hint(context, "open_attachment")
                    .map(String::from),
                "open",
            ));
        }
        push_hints(theme, &mut spans, &hints);
    } else {
        let context = Some(Context::List);
        let hints: Vec<(Option<String>, &str)> = vec![
            (state.settings.keymap.move_hint(context), "move"),
            (
                state
                    .settings
                    .keymap
                    .hint(context, "activate")
                    .map(String::from),
                "open",
            ),
            (
                state
                    .settings
                    .keymap
                    .hint(context, "toggle_selected")
                    .map(String::from),
                "select",
            ),
            (
                state
                    .settings
                    .keymap
                    .hint(context, "star")
                    .map(String::from),
                "star",
            ),
            (
                state
                    .settings
                    .keymap
                    .hint(context, "archive")
                    .map(String::from),
                "archive",
            ),
            (
                state
                    .settings
                    .keymap
                    .hint(context, "trash")
                    .map(String::from),
                "delete",
            ),
            (
                state
                    .settings
                    .keymap
                    .hint(context, "compose")
                    .map(String::from),
                "compose",
            ),
            (
                state
                    .settings
                    .keymap
                    .hint(context, "open_search")
                    .map(String::from),
                "search",
            ),
            (
                state
                    .settings
                    .keymap
                    .hint(context, "open_help")
                    .map(String::from),
                "shortcuts",
            ),
        ];
        push_hints(theme, &mut spans, &hints);
    }
    // Foreground work is announced by the loader in this bar's left
    // corner; the status message sits top-right in the top bar.
    frame.render_widget(Paragraph::new(Line::from(spans)), row);
}

/// Render one hint per action, skipping unbound ones: an action with an
/// empty binding list in the config disappears from the row instead of
/// lying about a key. Hint text is cloned into owned spans so nothing
/// borrowed from `hints` flows into the frame's lifetime.
fn push_hints(theme: &Theme, spans: &mut Vec<Span<'_>>, hints: &[(Option<String>, &str)]) {
    for (key, label) in hints {
        let Some(key) = key else { continue };
        spans.push(Span::styled(
            String::from("  "),
            Style::new().bg(theme.sidebar_bg),
        ));
        spans.push(Span::styled(
            key.clone(),
            Style::new().fg(theme.text_soft).bg(theme.sidebar_bg),
        ));
        spans.push(Span::styled(
            format!(" {label}"),
            Style::new().fg(theme.label_dim).bg(theme.sidebar_bg),
        ));
    }
}
