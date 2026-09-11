//! Status bar: key hints, foreground-work spinner, environment info
//! (mockup `list.html`).
//!
//! v1 overrides applied (plan §4): no `j`/`k` move hints, no `?` help. The
//! mockup's mode badge was dropped: it named the screen the user is already
//! looking at, so it carried no information.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::action::{BulkOp, ClickTarget};
use crate::app::route::Route;
use crate::app::state::AppState;
use crate::input::keymap::Context;
use crate::input::mouse::HitMap;
use crate::ui::chrome::{self, HairlineSide};
use crate::ui::theme::Theme;

/// Status-bar fill, matching the sidebar panel color (temporary
/// experiment): rgb(21, 24, 32).
const STATUSBAR_BG: Color = Color::Rgb(0x15, 0x18, 0x20);
/// Key glyphs on the hint row: rgb(192, 189, 183).
const KEY_COLOR: Color = Color::Rgb(0xC0, 0xBD, 0xB7);
/// Key labels on the hint row: rgb(87, 94, 113).
const LABEL_COLOR: Color = Color::Rgb(0x57, 0x5E, 0x71);

/// Render the status bar into `area` (height 3: hairline + content row).
pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    hits: &mut HitMap,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    // Panel fill, same as the sidebar: hints, hairline, and the blank
    // padding row all sit on it.
    frame.render_widget(Block::new().style(Style::new().bg(STATUSBAR_BG)), area);

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

    // Key hints follow the input contract (plan §10) and the active screen
    // (mockup `.statusbar` per layout). Deliberately no j/k, no help.
    // Selection mode replaces the hint row with the bulk-operation buttons
    // (ticket p0s3): the count and the four actions, each clickable.
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
                .bg(STATUSBAR_BG)
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
                Style::new().fg(theme.text_soft).bg(STATUSBAR_BG),
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
                Style::new().fg(theme.muted).bg(STATUSBAR_BG),
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
        push_hints(&mut spans, &hints);
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
        push_hints(&mut spans, &hints);
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
        push_hints(&mut spans, &hints);
    }
    // Foreground work is announced by the loader under the top-bar logo
    // (ticket m3by); the status message sits top-right in the top bar.
    frame.render_widget(Paragraph::new(Line::from(spans)), row);
}

/// Render one hint per action, skipping unbound ones: an action with an
/// empty binding list in the config disappears from the row instead of
/// lying about a key. Hint text is cloned into owned spans so nothing
/// borrowed from `hints` flows into the frame's lifetime.
fn push_hints(spans: &mut Vec<Span<'_>>, hints: &[(Option<String>, &str)]) {
    for (key, label) in hints {
        let Some(key) = key else { continue };
        spans.push(Span::styled(
            String::from("  "),
            Style::new().bg(STATUSBAR_BG),
        ));
        spans.push(Span::styled(
            key.clone(),
            Style::new().fg(KEY_COLOR).bg(STATUSBAR_BG),
        ));
        spans.push(Span::styled(
            format!(" {label}"),
            Style::new().fg(LABEL_COLOR).bg(STATUSBAR_BG),
        ));
    }
}
