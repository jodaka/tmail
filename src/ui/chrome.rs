//! Shared chrome the screens and components draw the same way: centered
//! dialog geometry, hairline rules, scrollbar construction, modal frames,
//! and dim one-line notes. One implementation per look, so a palette or
//! anatomy change lands everywhere at once.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Frame;
use unicode_width::UnicodeWidthChar;

use crate::ui::text;
use crate::ui::theme::Theme;

/// One side of a hairline rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HairlineSide {
    /// Rule on the bottom edge of `area`.
    Bottom,
    /// Rule on the top edge of `area`.
    Top,
}

/// A horizontally centered dialog rectangle: the shared anatomy of the
/// modal `layout()` functions (`x = (size.0 - width) / 2`, and so on).
/// The rectangle never exceeds the terminal: oversized dimensions clamp to
/// the terminal first, so callers may pass raw caps.
pub use crate::view::layout::centered;

/// A hairline rule on one edge of `area` (mockup `border-bottom: 1px solid
/// var(--border)`): one-color border block, the only difference between
/// the five drawn sites being the edge and rectangle.
pub fn hairline(frame: &mut Frame<'_>, area: Rect, side: HairlineSide, theme: &Theme) {
    let borders = match side {
        HairlineSide::Bottom => Borders::BOTTOM,
        HairlineSide::Top => Borders::TOP,
    };
    frame.render_widget(
        Block::default()
            .borders(borders)
            .border_style(theme.hairline()),
        area,
    );
}

/// The dimensionless part of the shared scrollbar look (full-height
/// vertical right rail, no end symbols, `│` track); styles ride the active
/// palette. Callers keep their stateful position math.
///
/// Two plumbing helpers wrap the copy-pasted wiring the three scroll
/// surfaces (theme picker, mail list, reader body) repeat: width
/// reservation and render. One anatomy — a change lands everywhere.
pub fn scrollbar(theme: &Theme) -> Scrollbar<'_> {
    Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .track_style(Style::new().fg(theme.border).bg(theme.background))
        .thumb_style(Style::new().fg(theme.dim).bg(theme.background))
}

/// The width the content may occupy next to a scrollbar: when `total`
/// exceeds `visible`, the last column of the rail area is reserved for the
/// rail (text and scrollbar never overlap); when everything fits, the
/// full width is used at no scrollbar.
pub fn scrollbar_content_width(rail_width: u16, total: usize, visible: usize) -> u16 {
    if total > visible {
        rail_width.saturating_sub(1)
    } else {
        rail_width
    }
}

/// Render the shared scrollbar over `area` for a `total` list whose
/// scroll window starts at `position` (first visible row/line).
pub fn render_scrollbar(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: &Theme,
    total: usize,
    position: usize,
) {
    let mut scrollbar_state = ScrollbarState::new(total).position(position);
    frame.render_stateful_widget(scrollbar(theme), area, &mut scrollbar_state);
}

/// The shared modal scaffold: clear the area, draw the bordered block with
/// an inverse-filled title on `fill`, and return the inner content rect.
/// One anatomy for all the dialogs; `Some` means the area was large
/// enough to open (callers keep their own bigger minimums).
///
/// Anatomy (top to bottom): title border, one blank margin row, the
/// returned content rect, then one more row — the *label slot*, one row
/// above the bottom border, where the dialog's bottom hint renders —
/// separated from the content by the last row of the returned rect, which
/// stays blank. Dialog geometry must budget all four fixed rows (outer
/// height = content + 5).
pub fn modal_frame(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &Span<'_>,
    fill: ratatui::style::Color,
    theme: &Theme,
) -> Option<Rect> {
    if area.width < 4 || area.height < 3 {
        return None;
    }
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            title.content.as_ref(),
            Style::new()
                .fg(theme.background)
                .bg(fill)
                .add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::new().fg(fill))
        .style(theme.on_background());
    frame.render_widget(block, area);
    Some(Rect {
        x: area.x + 2,
        y: area.y + 2,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(4),
    })
}

/// A dim one-line note clipped to `area`'s width (empty states in panes).
pub fn render_note(frame: &mut Frame<'_>, area: Rect, theme: &Theme, note: &str) {
    if area.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Span::styled(
            text::clip(note, area.width as usize),
            Style::new().fg(theme.dim),
        )),
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        },
    );
}

/// The wrapped label of a bracketed action button: ` [ label ] `.
/// Render spans and click-target hit rects both derive their width from
/// this, so they can never spell the padding twice.
pub fn labeled_button_label(label: &str) -> String {
    format!(" [ {label} ] ")
}

/// One bracketed action button span: ` [ label ] ` with the focused-
/// button look (`Theme::button_style`: page-background text on `fill` and
/// bold when focused, quiet muted otherwise). The single look behind the
/// confirm modal, the composer's actions, and the wizard's selected rows.
pub fn labeled_button<'a>(label: &str, focused: bool, fill: Color, theme: &'a Theme) -> Span<'a> {
    Span::styled(
        labeled_button_label(label),
        theme.button_style(focused, fill),
    )
}

/// Value spans of one single-line input field with an inline caret (the
/// composer's convention: the focused field draws the reversed-cell caret;
/// the terminal cursor stays hidden app-wide). Characters clip to
/// `value_w`; address fields (To/Cc/Bcc) color invalid entries in the
/// warning color (plan §14 validation-on-type); masked fields never
/// render the real character (ADR 0003 §3.2 W4 keystrokes render as •).
/// One implementation for the composer screen and the wizard.
pub fn field_value_spans<'a>(
    text: &'a str,
    cursor: usize,
    focused: bool,
    masked: bool,
    address_field: bool,
    value_w: usize,
    theme: &'a Theme,
) -> Vec<Span<'a>> {
    // The same accent caret block the body editor draws while focused
    // (ticket tz12); `Theme::caret` keeps the two in step.
    let caret_style = theme.caret();
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
    // Address entries are byte-ordered and non-overlapping (they are cut
    // sequentially at separators), so one cursor walks them in step with
    // the characters instead of rescanning the list per char.
    let mut entry = 0usize;
    for (index, (byte, ch)) in text.char_indices().enumerate() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + width > value_w {
            break;
        }
        while entry < entries.len() && entries[entry].range.end <= byte {
            entry += 1;
        }
        let inside_invalid =
            entry < entries.len() && entries[entry].range.contains(&byte) && !entries[entry].valid;
        let style = if focused && index == cursor {
            caret_style
        } else if inside_invalid {
            invalid
        } else {
            normal
        };
        let shown = if masked {
            "•"
        } else {
            &text[byte..byte + ch.len_utf8()]
        };
        spans.push(Span::styled(shown, style));
        used += width;
    }
    // Caret past the end of the text: a reversed space.
    if focused && cursor >= char_count && used < value_w {
        spans.push(Span::styled(" ", caret_style));
    }
    spans
}

/// The centered "Terminal too small" screen (plan §18: one look, two
/// callers). `requirement` names what minimum is missing; `quit` names
/// the key that exits.
pub fn render_too_small(
    frame: &mut Frame<'_>,
    area: Rect,
    current: (u16, u16),
    requirement: &str,
    quit: &str,
    theme: &Theme,
) {
    let message = format!(
        "Terminal too small ({}×{})\n{}\n{}",
        current.0, current.1, requirement, quit
    );
    let lines: Vec<Line<'_>> = message
        .lines()
        .map(|line| {
            Line::from(Span::styled(
                line,
                Style::new().fg(theme.text).bg(theme.background),
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).centered(), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(elems: (u16, u16, u16, u16)) -> Rect {
        Rect {
            x: elems.0,
            y: elems.1,
            width: elems.2,
            height: elems.3,
        }
    }

    #[test]
    fn centered_dialog_geometry() {
        assert_eq!(centered((152, 40), 76, 18), rect((38, 11, 76, 18)));
        // Clamped inside tiny terminals.
        assert_eq!(centered((40, 10), 96, 30), rect((0, 0, 40, 10)));
        // Degenerate terminals still keep a 1x1 rect (u16 floor).
        assert_eq!(centered((0, 0), 52, 8), rect((0, 0, 1, 1)));
    }

    /// Address-field validation styling (plan §14): invalid entries render
    /// in the warning color, valid ones (and separators) do not — across
    /// several entries, so the per-char entry walk stays in step (the old
    /// per-char rescan covered this; the cursor must too).
    #[test]
    fn address_field_styles_invalid_entries() {
        let theme = Theme::default_dark();
        let spans = field_value_spans(
            "a@b.co, broken, c@d.io",
            usize::MAX,
            false,
            false,
            true,
            80,
            &theme,
        );
        let styled: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(styled, "a@b.co, broken, c@d.io");
        // Exactly the `broken` entry is warning-styled: char positions
        // 8..=13 (each char is its own span; the field is unfocused, so
        // no caret cell).
        let warning_positions: Vec<usize> = spans
            .iter()
            .enumerate()
            .filter(|(_, span)| span.style.fg == Some(theme.warning))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(warning_positions, vec![8, 9, 10, 11, 12, 13]);
    }

    /// Unfocused fields render no caret span (the reversed-cell caret is
    /// only for the focused field), and plain (non-address) fields never
    /// style anything invalid.
    #[test]
    fn unfocused_field_renders_plain_chars() {
        let theme = Theme::default_dark();
        let spans = field_value_spans("abc, broken", 0, false, false, false, 80, &theme);
        let styled: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(styled, "abc, broken");
        assert!(spans.iter().all(|s| s.style.fg == Some(theme.text)));
    }

    /// Masked fields never reveal the real character (ADR 0003 §3.2 W4):
    /// one bullet per char, exactly `char_count` of them.
    #[test]
    fn masked_field_renders_bullets_only() {
        let theme = Theme::default_dark();
        let spans = field_value_spans("hunter2", usize::MAX, false, true, false, 80, &theme);
        assert_eq!(spans.len(), 7);
        assert!(spans.iter().all(|s| s.content == "•"));
    }

    /// The focused field's caret rides the cursor cell even inside an
    /// invalid entry, and clipping stops at `value_w` columns.
    #[test]
    fn caret_wins_over_invalid_and_clip_holds() {
        let theme = Theme::default_dark();
        let spans = field_value_spans("broken, more", 1, true, false, true, 5, &theme);
        let styled: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(styled, "broke");
        // Index 1 sits in the invalid `broken` entry but is the caret cell.
        assert_eq!(spans[1].style.fg, Some(theme.background));
        assert_eq!(spans[1].style.bg, Some(theme.accent));
        // The rest of the clipped range stays warning-colored.
        assert!(spans[3..].iter().all(|s| s.style.fg == Some(theme.warning)));
    }
}
