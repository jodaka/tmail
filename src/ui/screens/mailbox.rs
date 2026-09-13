//! Mailbox screen: list header + message rows (mockup `list.html`).
//!
//! One row per message (the mockup's single-line grid). Unread rows get the
//! `unread` modifier — under the selection fill too, so a focused unread
//! row stays bold; the selected row gets the accent fill. The same list
//! renders search results (Phase 9): the head then names the query, and an
//! empty result set is a valid, explicit state. No thread count, no labels
//! column, no tags (plan §4 overrides; labels carry no backend meaning —
//! ADR 0001).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::action::ClickTarget;
use crate::app::focus::Focus;
use crate::app::route::Route;
use crate::app::state::AppState;
use crate::domain::MessageSummary;
use crate::input::mouse::HitMap;
use crate::ui::chrome::{self, HairlineSide};
use crate::ui::dates;
use crate::ui::layout::LayoutMode;
use crate::ui::text;
use crate::ui::theme::Theme;

/// Gap between the rendered date text and the row's right edge: the date
/// cell is 8 columns wide (row anatomy in `message_spans`) and the common
/// `format_relative` outputs are 5 columns (`HH:MM`, `Sep 1`), left-aligned
/// — so dates end 3 columns short of the edge. The header's range label
/// right-aligns to the same edge, lining it up with the date/time column
/// instead of leaving it stuck to the pane border.
const DATE_TEXT_RIGHT_INSET: usize = 3;

/// Render the message list into `area` (already split off from the sidebar).
pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    mode: LayoutMode,
    theme: &Theme,
    now: chrono::DateTime<chrono::FixedOffset>,
    hits: &mut HitMap,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let (head, rows) = crate::ui::layout::split_list(area);

    // Draw from the reducer-maintained scroll anchor so the selected row is
    // always on screen regardless of movement, page loads, or resize
    // (Phase 2 acceptance). When the page holds more rows than fit, a
    // vertical scrollbar takes the last column (ticket kjfq) and the rows
    // clip one column short.
    //
    // Row geometry follows the view mode (`[tmail].view_mode`): compact
    // draws one line per message; comfortable splits consecutive messages
    // with a faint horizontal separator, so each message costs two lines
    // and fewer fit — the same math as `layout::messages_visible`, which
    // the reducer uses for scroll and page sizing.
    let row_height = state.settings.view_mode.row_height();
    let visible = rows.height as usize / row_height;
    let scrolling = state.messages.items.len() > visible;
    let row_width = if scrolling {
        rows.width.saturating_sub(1)
    } else {
        rows.width
    };
    render_head(frame, head, state, theme, scrolling);
    // Rows of the Drafts mailbox show the recipient in the sender column:
    // a draft's sender is always the user's own address, which reads as
    // noise. Matched on the row's mailbox id, so search results over the
    // Drafts mailbox follow the same rule.
    let drafts_mailbox = state.drafts_mailbox().map(|m| &m.id);
    let bottom = area.y + area.height;
    let mut drew_any_row = false;
    let mut y = rows.y;
    for (i, message) in state
        .messages
        .items
        .iter()
        .enumerate()
        .skip(state.list_scroll)
    {
        // A message renders only when its whole row block fits: a clipped
        // comfortable separator would disagree with the reducer's notion
        // of how many rows are visible.
        if y + row_height as u16 > bottom {
            break;
        }
        drew_any_row = true;
        let selected = i == state.selection;
        let bulk_selected = state.selected.contains(&message.id);
        let content = Rect {
            x: rows.x,
            y,
            width: row_width,
            height: 1,
        };
        let shows_recipient = drafts_mailbox
            .as_ref()
            .is_some_and(|id| **id == message.mailbox_id);
        let spans = message_spans(
            message,
            row_width as usize,
            &RowContext {
                mode,
                theme,
                now,
                selected,
                bulk_selected,
                focused: state.session.focus == Focus::MessageList,
                shows_recipient,
            },
        );
        frame.render_widget(Paragraph::new(Line::from(spans)), content);
        // Comfortable density: a hairline under the row (fainter than any
        // body text) separates consecutive messages with negative space.
        // It stays inside the message's hit block, so clicking between
        // rows still targets the message above the line.
        if row_height > 1 {
            let separator = Rect {
                x: rows.x,
                y: y + 1,
                width: row_width,
                height: 1,
            };
            chrome::hairline(frame, separator, HairlineSide::Bottom, theme);
        }
        // Clicking a row selects it; clicking the selected row opens it
        // (arrows + Enter, plan §10). The hit block covers the separator
        // line too.
        hits.push(
            Rect {
                x: rows.x,
                y,
                width: row_width,
                height: row_height as u16,
            },
            ClickTarget::MessageRow(i),
        );
        y += row_height as u16;
    }
    if scrolling {
        chrome::render_scrollbar(
            frame,
            rows,
            theme,
            state.messages.items.len(),
            state.list_scroll,
        );
    }
    // An empty search result set is a valid state, not an error — say so
    // once the request is no longer in flight (Phase 9.3). While the list
    // is empty and a load is in flight, show the pane spinner instead
    // (ticket m3by: one loader look everywhere, centered in the panel).
    if !drew_any_row && list_load_in_flight(state) {
        crate::ui::components::spinner::render_centered(
            frame,
            rows,
            theme,
            crate::ui::components::spinner::pane_millis(state),
        );
    } else if !drew_any_row
        && matches!(state.active_route(), Some(Route::Search(_)))
        && state.session.operations.foreground().is_none()
    {
        chrome::render_note(frame, rows, theme, "(no results)");
    }
}

/// Whether the visible list is waiting for its first rows: a page or
/// search load in flight for the active route (ticket m3by).
fn list_load_in_flight(state: &AppState) -> bool {
    match state.active_route() {
        Some(Route::Search(route)) => state
            .session
            .operations
            .search_in_flight(&route.mailbox_id)
            .is_some(),
        Some(route) => route
            .mailbox_id()
            .is_some_and(|id| state.session.operations.page_in_flight(id).is_some()),
        None => false,
    }
}

fn render_head(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    scrolling: bool,
) {
    if area.height == 0 {
        return;
    }
    let row = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    let width = area.width as usize;

    // Search results headline the query; a mailbox names itself (Phase 9).
    let (title, unread) = match state.active_route() {
        Some(Route::Search(route)) => (format!("SEARCH — {}", route.query), String::new()),
        _ => {
            let title = state
                .active_mailbox_name()
                .unwrap_or("Mailbox")
                .to_uppercase();
            let unread = state
                .active_mailbox_unread()
                .map(|n| format!("{n} unread"))
                .unwrap_or_default();
            (title, unread)
        }
    };
    let range = state.messages.range_label();

    // Right-aligned range (mockup `.pane-range`).
    let range_w = range.width();
    let left_budget = width.saturating_sub(range_w + 2).max(10);
    // The mailbox label is a plain header (ticket fypg): no checkbox, no
    // click target, not focusable — select-all stays Ctrl+A's job. Three
    // columns of padding set the label off the pane edge.
    let title = text::clip(&format!("   {title}"), left_budget);
    let mut spans = Vec::new();
    if !title.is_empty() {
        spans.push(Span::styled(
            title,
            Style::new()
                .fg(theme.text)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if !unread.is_empty() {
        spans.push(Span::styled(
            format!("  {unread}"),
            Style::new().fg(theme.dim),
        ));
    }
    if !range.is_empty() {
        // Right-align the range with the date/time column of the rows
        // below rather than the pane border: the date cell leaves a
        // 3-column gap after its 5-character times, and while the
        // scrollbar shows it shaves one more column off every row.
        let inset = (DATE_TEXT_RIGHT_INSET + usize::from(scrolling)) as u16;
        let range_area = Rect {
            x: (area.x + area.width).saturating_sub(range_w as u16 + inset),
            y: row.y,
            width: range_w as u16,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Span::styled(range, Style::new().fg(theme.dim))),
            range_area,
        );
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), row);

    // Hairline under the header.
    let hairline = Rect {
        x: area.x,
        y: area.y.saturating_add(1),
        width: area.width,
        height: 1,
    };
    chrome::hairline(frame, hairline, HairlineSide::Bottom, theme);
}

/// Per-row presentation inputs for [`message_spans`]: the frame-wide
/// styling plus the row's flags. Bundled because the renderer computes the
/// frame-wide parts once and varies only the flags per row.
#[derive(Clone, Copy)]
struct RowContext<'a> {
    mode: LayoutMode,
    theme: &'a Theme,
    now: chrono::DateTime<chrono::FixedOffset>,
    /// The cursor row.
    selected: bool,
    // Space-marked row (ticket p0s3): the bulk highlight fill. The cursor
    // row keeps its accent fill; both may apply to one row (cursor wins).
    bulk_selected: bool,
    // The message list holds focus: the selected row shows the accent
    // marker (the same bar the sidebar's focused folder carries). Selection
    // alone keeps the fill but never the marker, so focus stays readable.
    focused: bool,
    // Drafts row: the sender column shows the recipient instead (the
    // sender is always the user's own address there).
    shows_recipient: bool,
}

fn message_spans<'a>(
    message: &'a MessageSummary,
    width: usize,
    ctx: &RowContext<'a>,
) -> Vec<Span<'a>> {
    let RowContext {
        mode,
        theme,
        now,
        selected,
        bulk_selected,
        focused,
        shows_recipient,
    } = *ctx;
    let full = mode == LayoutMode::Full;
    let compact = mode == LayoutMode::Compact;

    // Column anatomy: marker 1 + icon 2 (star/checkbox + trailing space,
    // ticket cvc4) + who + gap 2 + subject(+snippet) + gap 2 + date 8,
    // summing to the full row width so the date lands on the right edge
    // (mockup grid). `who` is the sender, or the recipient on Drafts rows.
    let date = dates::format_relative(now, message.timestamp);
    let who_w = if compact { 14 } else { 18 };
    let subject_w = width.saturating_sub(3 + who_w + 2 + 2 + 8).max(10);
    // The paperclip rides one space left of the date (ticket r84f, user
    // amend): on attachment rows the title/body cell gives up one column
    // and the clip (2 cells, one char) plus the separator space replace
    // the two-cell gap, so the icon sits at a fixed slot whatever the
    // title and body lengths, and the date cell never moves. Rows without
    // attachments keep the plain gap (nothing changes, ticket r84f).
    let cell_w = if message.has_attachments {
        subject_w.saturating_sub(1)
    } else {
        subject_w
    };

    let bg = if selected {
        // The marker gold: one fill for the whole row, the same highlight
        // the sidebar's active folder carries.
        theme.marker
    } else if bulk_selected {
        // Same fill as selected mailboxes in the sidebar (theme.selection).
        theme.selection
    } else {
        theme.background
    };
    let base = if selected {
        theme.row_selected()
    } else if bulk_selected {
        theme.row_bulk_selected()
    } else {
        Style::new().bg(theme.background)
    };

    // Icon column (ticket cvc4): a bulk-selected row shows the filled dot
    // whatever its star state; otherwise the star, or a blank. The cell is
    // always two columns — symbol + trailing space. On the selected row the
    // dot rides the base style: like-colored fg/bg would vanish.
    let (symbol, style) = if bulk_selected {
        (
            "●",
            if selected {
                base
            } else {
                theme.accent_fg().bg(bg)
            },
        )
    } else if message.is_starred {
        ("*", theme.star().bg(bg))
    } else {
        (" ", base)
    };
    let icon = Span::styled(format!("{symbol} "), style);
    // Bar in the marker column (mockup `.folder.active` bar): marks the
    // focused row while the list holds focus. White over the marker fill.
    let marker = if selected && focused {
        Span::styled("▎", Style::new().fg(theme.marker_bar).bg(bg))
    } else {
        Span::styled(" ", base)
    };

    // The fill marks selection and the `unread` modifier marks state, so a
    // focused unread row keeps its bold under the marker fill — selection
    // alone must not flatten read and unread rows into one look. The
    // selected row's text is page-background (contrast on the gold fill,
    // the mode-badge convention); bulk-marked rows keep the plain text.
    let who_style = if selected {
        if message.is_read {
            base
        } else {
            base.add_modifier(theme.unread)
        }
    } else if bulk_selected {
        if message.is_read {
            base.fg(theme.text)
        } else {
            base.fg(theme.text).add_modifier(theme.unread)
        }
    } else if message.is_read {
        theme.read_text().bg(theme.background)
    } else {
        theme.unread_text().bg(theme.background)
    };
    let subject_style = who_style;
    // The faded preview (ticket wxtx): dimmer than any subject state, so
    // the Gmail-style body preview reads as context. It keeps the row's
    // fill (marker fill included); on the selected row it takes the row's
    // text color — the snippet token has no contrast on the gold fill.
    let snippet_style = if selected {
        Style::new().fg(theme.background).bg(bg)
    } else {
        Style::new().fg(theme.snippet).bg(bg)
    };

    // Subject plus (full mode only) the faded body preview, composed so
    // the combined cell fills exactly `cell_w`: the subject stays whole
    // when it fits, the preview takes what is left and ends in `…` when
    // the body text is longer (ticket wxtx).
    let mut spans = vec![marker, icon];
    spans.push(Span::styled(
        text::fit_left(
            if shows_recipient {
                message.to_display()
            } else {
                message.from_display()
            },
            who_w,
        ),
        who_style,
    ));
    spans.push(Span::styled("  ", base));
    if full {
        let sep = " — ";
        let subject_cell = text::truncate(&message.subject, cell_w);
        let subject_alone = message.subject.width() > cell_w;
        let preview_cell = if subject_alone {
            String::new()
        } else {
            message
                .snippet
                .as_ref()
                .and_then(|snippet| {
                    let budget = cell_w
                        .saturating_sub(message.subject.width())
                        .saturating_sub(sep.width());
                    (budget > 0).then(|| format!("{sep}{}", text::truncate(snippet, budget)))
                })
                .unwrap_or_default()
        };
        let used = subject_cell.width() + preview_cell.width();
        spans.push(Span::styled(subject_cell, subject_style));
        if !preview_cell.is_empty() {
            spans.push(Span::styled(preview_cell, snippet_style));
        }
        // Pad the cell to its exact column width so the clip slot and the
        // date stay put (mockup grid).
        spans.push(Span::styled(" ".repeat(cell_w.saturating_sub(used)), base));
    } else {
        spans.push(Span::styled(
            text::fit_left(&message.subject, cell_w),
            subject_style,
        ));
    }
    if message.has_attachments {
        // Clip (2 cells, one char) + one space, exactly where the 2-cell
        // gap used to end: the emoji's right edge lands one column left
        // of the date.
        spans.push(Span::styled(CLIP, base));
        spans.push(Span::styled(" ", base));
    } else {
        spans.push(Span::styled("  ", base));
    }
    let date_style = if message.is_read && !selected {
        Style::new().fg(theme.dim).bg(bg)
    } else {
        who_style.bg(bg)
    };
    spans.push(Span::styled(text::fit_left(&date, 8), date_style));
    spans
}

/// The attachment indicator of the message list (ticket r84f): rendered
/// one space left of the date on rows whose message carries attachments.
/// One char of two terminal columns.
const CLIP: &str = "📎";
