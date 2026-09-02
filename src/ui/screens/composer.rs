//! Composer screen (mockup `new-mail.html`, plan §14).
//!
//! Header ("New message"), one row per address/subject field with the
//! mockup's 8-column right-aligned labels and hairline rules, the body
//! editor (`ratatui-textarea`), and the Send/Discard action row. The
//! focused control gets the hover fill; single-line fields draw an
//! inline caret span (the terminal cursor stays hidden, matching the
//! rest of the app).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::composer::{ComposerField, ComposerState};
use crate::app::focus::Focus;
use crate::app::route::Route;
use crate::app::state::AppState;
use crate::ui::theme::Theme;

/// Label column width (mockup `grid-template-columns: 8ch`).
const LABEL_WIDTH: usize = 8;

/// Render the composer into `area` (the body region right of the sidebar).
pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let Some(composer) = &state.composer else {
        return;
    };
    if !matches!(state.active_route(), Some(Route::Composer)) {
        return;
    }
    if area.width < 12 || area.height < 4 {
        return;
    }
    let focused = state.focus == Focus::Composer;
    let inner_w = area.width.saturating_sub(4) as usize; // 2 columns of padding
    let x = area.x + 2;
    let bottom = area.y + area.height;
    let mut y = area.y;

    // Header: title left, autosave status right (status text lands in 6.7).
    if y < bottom {
        let header = Rect {
            x,
            y,
            width: inner_w as u16,
            height: 1,
        };
        let mut spans = vec![Span::styled(
            "New message",
            Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
        )];
        if let Some(status) = draft_status(composer) {
            let w = status.width();
            if w + 14 < inner_w {
                spans.push(Span::raw(" ".repeat(inner_w - 12 - w)));
                spans.push(Span::styled(status, Style::new().fg(theme.dim)));
            }
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), header);
        y += 1;
    }

    // Field rows (value row + hairline row each), mockup `.fields`. The
    // Cc/Bcc toggles ride the To row's right edge (mockup `.field-extra`).
    for field in visible_fields(composer) {
        if y + 2 > bottom {
            break;
        }
        let reserved = if field == ComposerField::To {
            16 // room for "[Cc] [Bcc] "
        } else {
            0
        };
        let (label, mut value) = field_row(composer, field, focused, theme, inner_w, reserved);
        if field == ComposerField::To {
            let toggles = toggle_spans(composer, focused, theme);
            // Right-align the buttons (mockup `.field-extra`): pad between
            // the value and the buttons.
            let used: usize = value.iter().map(|s| s.content.width()).sum();
            let toggle_w: usize = toggles.iter().map(|s| s.content.width()).sum();
            let pad = inner_w.saturating_sub(LABEL_WIDTH + 2 + used + toggle_w);
            value.push(Span::raw(" ".repeat(pad)));
            value.extend(toggles);
        }
        render_field_row(frame, x, y, inner_w, label, value);
        y += 1;
        hairline(frame, x, y, inner_w, theme);
        y += 1;
    }

    // Body fills the remaining space above the action row (mockup
    // `.msg-body` textarea).
    if bottom > y + 1 {
        let body = Rect {
            x,
            y,
            width: inner_w as u16,
            height: bottom - y - 1,
        };
        frame.render_widget(&composer.body, body);
    }

    // Action row: Send (accent) and Discard (warning), mockup
    // `.compose-actions`.
    if bottom > y {
        let actions = Rect {
            x,
            y: bottom - 1,
            width: inner_w as u16,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(action_spans(composer, focused, theme))),
            actions,
        );
    }
}

/// The fields drawn as rows, in order (toggle controls are buttons on the
/// To row and are not drawn separately).
fn visible_fields(composer: &ComposerState) -> Vec<ComposerField> {
    let mut fields = vec![ComposerField::To];
    if composer.show_cc {
        fields.push(ComposerField::Cc);
    }
    if composer.show_bcc {
        fields.push(ComposerField::Bcc);
    }
    fields.push(ComposerField::Subject);
    fields
}

/// Label + value spans of one field row, with an inline caret on the
/// focused single-line field.
fn field_row<'a>(
    composer: &'a ComposerState,
    field: ComposerField,
    focused: bool,
    theme: &'a Theme,
    inner_w: usize,
    reserved_right: usize,
) -> (Vec<Span<'a>>, Vec<Span<'a>>) {
    let label = match field {
        ComposerField::To => "To",
        ComposerField::Cc => "Cc",
        ComposerField::Bcc => "Bcc",
        ComposerField::Subject => "Subject",
        _ => "",
    };
    let is_focused = focused && composer.field == field;
    let value_w = inner_w
        .saturating_sub(LABEL_WIDTH + 2)
        .saturating_sub(reserved_right);
    let (text, cursor) = match field {
        ComposerField::To => (&composer.to, composer.cursor),
        ComposerField::Cc => (&composer.cc, composer.cursor),
        ComposerField::Bcc => (&composer.bcc, composer.cursor),
        ComposerField::Subject => (&composer.subject, composer.cursor),
        // The body is not a single-line string; this arm is unreachable in
        // practice (visible_fields never yields it).
        _ => (&composer.to, composer.cursor),
    };
    let label_spans = vec![
        Span::styled(format!("{label:>LABEL_WIDTH$}"), Style::new().fg(theme.dim)),
        Span::styled("  ", Style::new()),
    ];
    let is_address = matches!(
        field,
        ComposerField::To | ComposerField::Cc | ComposerField::Bcc
    );
    let value_spans = value_spans(text, cursor, is_focused, is_address, value_w, theme);
    (label_spans, value_spans)
}

/// Value spans of one field. Address fields (To/Cc/Bcc) validate on the
/// fly (plan §14): invalid entries render in the warning color, valid ones
/// in the normal text color. The focused field additionally draws an
/// inline caret (reversed cell); the terminal cursor stays hidden
/// app-wide.
fn value_spans<'a>(
    text: &'a str,
    cursor: usize,
    focused: bool,
    address_field: bool,
    value_w: usize,
    theme: &'a Theme,
) -> Vec<Span<'a>> {
    let caret_style = Style::new()
        .fg(theme.background)
        .bg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let normal = Style::new().fg(theme.text);
    let invalid = Style::new().fg(theme.warning);
    let entries = if address_field {
        crate::domain::address::address_entries(text)
    } else {
        Vec::new()
    };
    let char_count = text.chars().count();
    let mut spans = Vec::new();
    let mut used = 0usize;
    for (index, (byte, ch)) in text.char_indices().enumerate() {
        let width = ch.to_string().width();
        if used + width > value_w {
            break;
        }
        let style = if entries.iter().any(|e| e.range.contains(&byte) && !e.valid) {
            invalid
        } else {
            normal
        };
        if focused && index == cursor {
            spans.push(Span::styled(ch.to_string(), caret_style));
        } else {
            spans.push(Span::styled(ch.to_string(), style));
        }
        used += width;
    }
    if focused && cursor >= char_count && used < value_w {
        // Caret past the end of the text: a reversed space.
        spans.push(Span::styled(" ", caret_style));
    }
    spans
}

/// `Cc`/`Bcc` buttons on the To row; hidden while their field is revealed.
/// The focused one gets the badge fill (mockup `.field-extra button`).
fn toggle_spans<'a>(composer: &'a ComposerState, focused: bool, theme: &'a Theme) -> Vec<Span<'a>> {
    let button = |label: &'a str, active: bool| {
        if focused && active {
            Span::styled(format!(" [{label}] "), theme.mode_badge())
        } else {
            Span::styled(format!(" [{label}] "), Style::new().fg(theme.dim))
        }
    };
    let mut spans = Vec::new();
    if !composer.show_cc {
        spans.push(button("Cc", composer.field == ComposerField::CcToggle));
    }
    if !composer.show_bcc {
        spans.push(button("Bcc", composer.field == ComposerField::BccToggle));
    }
    spans
}

fn render_field_row(
    frame: &mut Frame<'_>,
    x: u16,
    y: u16,
    inner_w: usize,
    label: Vec<Span<'_>>,
    value: Vec<Span<'_>>,
) {
    let mut spans = label;
    spans.extend(value);
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            x,
            y,
            width: inner_w as u16,
            height: 1,
        },
    );
}

fn hairline(frame: &mut Frame<'_>, x: u16, y: u16, inner_w: usize, theme: &Theme) {
    frame.render_widget(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(theme.hairline()),
        Rect {
            x,
            y,
            width: inner_w as u16,
            height: 1,
        },
    );
}

/// Send + Discard spans; the focused action gets the strong fill (mockup
/// `.btn-send` / `.compose-action.warn`).
fn action_spans<'a>(composer: &'a ComposerState, focused: bool, theme: &'a Theme) -> Vec<Span<'a>> {
    let send_focused = focused && composer.field == ComposerField::Send;
    let discard_focused = focused && composer.field == ComposerField::Discard;
    let send = if send_focused {
        Span::styled(
            " [ Send ^↵ ] ",
            theme.mode_badge().add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " [ Send ^↵ ] ",
            Style::new().fg(theme.text_soft).bg(theme.surface),
        )
    };
    let discard = if discard_focused {
        Span::styled(
            " Discard ",
            Style::new()
                .fg(theme.background)
                .bg(theme.warning)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(" Discard ", Style::new().fg(theme.muted))
    };
    vec![send, Span::raw("   "), discard]
}

/// Autosave status line (mockup `.draft-status`); the states fill in with
/// Phase 6.7.
fn draft_status(composer: &ComposerState) -> Option<String> {
    let _ = composer;
    None
}
