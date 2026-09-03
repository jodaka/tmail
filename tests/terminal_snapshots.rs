//! Render tests on Ratatui's `TestBackend`: the mock mailbox screen must
//! render at full, compact, and too-small sizes (plan §19 Phase 1), the
//! status bar must never show `j`/`k` or a help hint (plan §4), and Phase 3
//! adds spinner + Retry/Dismiss modal render checks (plan §19 Phase 3).

use tmail::app::mock::{self, mock_initial_state};
use tmail::app::operation::{OperationFailure, OperationKind, OperationResult};
use tmail::app::{Action, reducer};
use tmail::input::mouse::HitMap;
use tmail::ui::{RenderContext, Theme, dates, render};

use ratatui::backend::TestBackend;
use ratatui::{Terminal, style::Color};

fn draw(width: u16, height: u16) -> ratatui::buffer::Buffer {
    draw_with_hits(width, height).0
}

/// Draw the mock state and return the buffer together with the recorded
/// widget rectangles, so tests can verify that hit-testing agrees with
/// what was actually drawn (Phase 10.1).
fn draw_with_hits(width: u16, height: u16) -> (ratatui::buffer::Buffer, HitMap) {
    let mut state = mock_initial_state();
    state.size = (width, height);
    let theme = Theme::default_dark();
    let now = mock::now();
    let ctx = RenderContext::new(now, dates::format_clock(now));
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend");
    let mut hits = HitMap::default();
    terminal
        .draw(|frame| render(frame, &state, &theme, &ctx, &mut hits))
        .expect("draw");
    (terminal.backend().buffer().clone(), hits)
}

/// Flatten the buffer into a plain string, one line per row.
fn text_of(buffer: &ratatui::buffer::Buffer) -> String {
    let mut out = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn assert_absent(text: &str, needle: &str, size: (u16, u16)) {
    assert!(
        !text.contains(needle),
        "{needle:?} must not appear at {size:?}:\n{text}"
    );
}

#[test]
fn full_layout_renders_all_regions() {
    let size = (152, 40);
    let text = text_of(&draw(size.0, size.1));
    // Brand + version.
    assert!(text.contains("post v0.1.0"), "brand missing:\n{text}");
    // Search field with placeholder.
    assert!(text.contains("Search mail"), "search placeholder missing");
    // Sidebar: compose + folders with unread counts. No labels, no storage.
    assert!(text.contains("Compose"), "compose affordance missing");
    assert!(text.contains("Inbox"), "inbox folder missing");
    assert!(text.contains("Archive"), "folders missing");
    // List head: title, unread, range.
    assert!(text.contains("INBOX"), "pane title missing");
    assert!(text.contains("24 unread"), "unread sub missing");
    assert!(text.contains("1–20 of 25"), "range missing");
    // Rows.
    assert!(text.contains("KKF Notifications"), "row sender missing");
    assert!(text.contains("Shipment #TF-8841"), "row subject missing");
    // Status bar: key hints only — the mode badge was dropped (it named
    // the screen the user is already looking at).
    assert_absent(&text, "NORMAL", size);
    assert_absent(&text, "READER", size);
    assert_absent(&text, "COMPOSE", size);
    assert!(text.contains("UTF-8 · 152×40"), "env info missing");
    // v1 overrides: no j/k hint, no help hint anywhere.
    assert_absent(&text, "help", size);
    assert_absent(&text, "j k", size);
    assert_absent(&text, "j/k", size);
    // Out-of-scope elements must not exist even in the mock.
    assert_absent(&text, "Labels", size);
    assert_absent(&text, "Snoozed", size);
    assert_absent(&text, "GB of", size);
}

#[test]
fn compact_layout_hides_sidebar_and_snippets() {
    let size = (100, 30);
    let text = text_of(&draw(size.0, size.1));
    assert!(text.contains("INBOX"), "pane title missing");
    assert_absent(&text, "NORMAL", size);
    // Sidebar hidden in compact.
    assert_absent(&text, "Compose", size);
    assert_absent(&text, "Archive", size);
    // Still renders rows; long senders clip exactly like the mockup's
    // compact 14ch column.
    assert!(text.contains("PayPal"), "rows missing");
}

#[test]
fn too_small_layout_shows_message_instead_of_widgets() {
    let size = (60, 15);
    let text = text_of(&draw(size.0, size.1));
    assert!(
        text.contains("Terminal too small"),
        "message missing:\n{text}"
    );
    assert!(text.contains("60×15"), "size report missing");
    assert!(text.contains("90×20"), "minimum report missing");
    // No overlapping chrome.
    assert_absent(&text, "NORMAL", size);
    assert_absent(&text, "INBOX", size);
}

#[test]
fn selected_row_uses_accent_fill() {
    let theme = Theme::default_dark();
    let buffer = draw(152, 40);
    let text = text_of(&buffer);
    let row = text
        .lines()
        .position(|line| line.contains("KKF Notifications"))
        .expect("first row visible");
    // Somewhere on the selected row the accent background must be active.
    let has_accent_bg = (0..buffer.area.width).any(|x| {
        let style = buffer[(x, row as u16)].style();
        style.bg == Some(theme.accent_bg)
    });
    assert!(has_accent_bg, "selected row lacks accent fill:\n{text}");
}

/// The compose well uses rounded corners like the search field and no
/// longer carries an inline `c` hint (the status bar advertises it).
#[test]
fn compose_well_is_rounded_without_an_inline_hint() {
    let buffer = draw(152, 40);
    let text = text_of(&buffer);
    let row = text
        .lines()
        .position(|line| line.contains("Compose"))
        .expect("compose well") as u16;
    // Rounded corners above and below the content row.
    assert_eq!(buffer[(1, row - 1)].symbol(), "╭", "top-left corner");
    assert_eq!(buffer[(1, row + 1)].symbol(), "╰", "bottom-left corner");
    // No inline `c` hint inside the well.
    let line = text.lines().nth(row as usize).expect("compose row");
    assert!(!line.contains(" c"), "inline hint must be gone: {line}");
}

/// The mockup's `.sidebar` border-right: a hairline divider column between
/// the folders and the message list, spanning the full body height.
#[test]
fn sidebar_divider_separates_folders_from_messages() {
    let buffer = draw(152, 40);
    let top = tmail::ui::layout::TOPBAR_HEIGHT;
    let bottom = 40 - tmail::ui::layout::STATUSBAR_HEIGHT;
    let x = tmail::ui::layout::SIDEBAR_WIDTH - 1;
    let divider_rows = (top..bottom)
        .filter(|y| buffer[(x, *y)].symbol() == "│")
        .count();
    assert_eq!(
        divider_rows,
        (bottom - top) as usize,
        "divider must span the body height"
    );
}

/// The accent bar marks focus, not selection: whichever pane holds focus
/// carries the bar on its cursor row, and the other pane carries none.
#[test]
fn focus_marker_follows_the_focused_pane() {
    // Default state: the message list holds focus.
    let mut state = mock_initial_state();
    let buffer = buffer_after(&mut state, &[], 152, 40);
    let text = text_of(&buffer);
    let list_row = text
        .lines()
        .position(|line| line.contains("KKF Notifications"))
        .expect("selected message row") as u16;
    let folder_row = text
        .lines()
        .position(|line| line.contains("Inbox"))
        .expect("active folder row") as u16;
    assert_eq!(
        buffer[(tmail::ui::layout::SIDEBAR_WIDTH, list_row)].symbol(),
        "▏",
        "list focused: selected message shows the bar"
    );
    assert_eq!(
        buffer[(0, folder_row)].symbol(),
        " ",
        "list focused: no folder shows the bar"
    );

    // Sidebar focused: the cursor folder (Inbox, selection 0) shows the
    // bar and the selected message does not.
    let mut state = mock_initial_state();
    state.focus = tmail::app::Focus::Sidebar;
    let buffer = buffer_after(&mut state, &[], 152, 40);
    let text = text_of(&buffer);
    let folder_row = text
        .lines()
        .position(|line| line.contains("Inbox"))
        .expect("cursor folder row") as u16;
    let list_row = text
        .lines()
        .position(|line| line.contains("KKF Notifications"))
        .expect("selected message row") as u16;
    assert_eq!(
        buffer[(0, folder_row)].symbol(),
        "▏",
        "sidebar focused: cursor folder shows the bar"
    );
    assert_eq!(
        buffer[(tmail::ui::layout::SIDEBAR_WIDTH, list_row)].symbol(),
        " ",
        "sidebar focused: no message shows the bar"
    );
}

#[test]
fn unread_rows_are_brighter_than_read_rows() {
    let theme = Theme::default_dark();
    let buffer = draw(152, 40);
    let text = text_of(&buffer);
    let unread_row = text
        .lines()
        .position(|line| line.contains("Maksim Orlov"))
        .expect("unread row") as u16;
    let read_row = text
        .lines()
        .position(|line| line.contains("Ilya Semyonov"))
        .expect("read row") as u16;
    let unread_bold = (0..40).any(|x| {
        buffer[(x, unread_row)]
            .style()
            .add_modifier
            .contains(ratatui::style::Modifier::BOLD)
    });
    let read_bold = (0..40).any(|x| {
        buffer[(x, read_row)]
            .style()
            .add_modifier
            .contains(ratatui::style::Modifier::BOLD)
    });
    assert!(unread_bold, "unread row must be bold");
    assert!(!read_bold, "read row must not be bold");
    let _ = theme;
    let _ = Color::Reset;
}

#[test]
fn resize_through_actions_switches_modes() {
    // Drive the reducer the same way the runtime does, then render.
    let mut state = mock_initial_state();
    let theme = Theme::default_dark();
    let now = mock::now();
    let ctx = RenderContext::new(now, dates::format_clock(now));

    let draw_state = |state: &tmail::app::AppState, w: u16, h: u16| {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).expect("test backend");
        let mut hits = HitMap::default();
        terminal
            .draw(|frame| render(frame, state, &theme, &ctx, &mut hits))
            .expect("draw");
        text_of(terminal.backend().buffer())
    };

    reducer::reduce(
        &mut state,
        &Action::Resize {
            width: 100,
            height: 30,
        },
    );
    let compact = draw_state(&state, 100, 30);
    assert!(!compact.contains("Compose"));

    reducer::reduce(
        &mut state,
        &Action::Resize {
            width: 60,
            height: 15,
        },
    );
    let small = draw_state(&state, 60, 15);
    assert!(small.contains("Terminal too small"));

    reducer::reduce(
        &mut state,
        &Action::Resize {
            width: 152,
            height: 40,
        },
    );
    let full = draw_state(&state, 152, 40);
    assert!(full.contains("Compose"));
}

/// Drive the reducer like the runtime does and render; returns the text.
fn draw_after(
    state: &mut tmail::app::AppState,
    actions: &[Action],
    width: u16,
    height: u16,
) -> String {
    text_of(&buffer_after(state, actions, width, height))
}

fn buffer_after(
    state: &mut tmail::app::AppState,
    actions: &[Action],
    width: u16,
    height: u16,
) -> ratatui::buffer::Buffer {
    let theme = Theme::default_dark();
    let now = mock::now();
    let ctx = RenderContext::new(now, dates::format_clock(now));
    for action in actions {
        reducer::reduce(state, action);
    }
    state.size = (width, height);
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend");
    let mut hits = HitMap::default();
    terminal
        .draw(|frame| render(frame, state, &theme, &ctx, &mut hits))
        .expect("draw");
    terminal.backend().buffer().clone()
}

/// A failure action with a sanitized detail (what the operation manager
/// hands the reducer after Phase 3.5 redaction).
fn sanitized_failure(id: tmail::app::OperationId, kind: OperationKind, detail: &str) -> Action {
    Action::BackendCompleted(OperationResult {
        id,
        outcome: Err(OperationFailure {
            code: Some(4),
            detail: String::from(detail),
            retry: Some(kind.retry_spec()),
            ambiguous: false,
        }),
    })
}

#[test]
fn spinner_shows_foreground_work_without_blocking_the_frame() {
    let mut state = mock_initial_state();
    // A page request starts an operation; the spinner appears in the
    // status bar with the operation summary (plan §11).
    let actions = vec![Action::PageNext];
    let text = draw_after(&mut state, &actions, 152, 40);
    assert!(text.contains("⠋"), "spinner frame missing:\n{text}");
    assert!(
        text.contains("Loading messages"),
        "summary missing:\n{text}"
    );
    // The list underneath still rendered — work never blocks the frame.
    assert!(
        text.contains("INBOX"),
        "list hidden behind spinner:\n{text}"
    );
}

#[test]
fn no_spinner_when_idle() {
    let mut state = mock_initial_state();
    let text = draw_after(&mut state, &[], 152, 40);
    assert!(!text.contains("⠋"), "spinner leaked while idle:\n{text}");
}

#[test]
fn error_modal_renders_summary_code_buttons_and_sanitized_detail() {
    let mut state = mock_initial_state();
    let (id, req) = match reducer::reduce(&mut state, &Action::PageNext).as_slice() {
        [effect] => (
            effect.id,
            match &effect.kind {
                OperationKind::LoadPage(request) => request.clone(),
                other => panic!("unexpected kind {other:?}"),
            },
        ),
        other => panic!("expected one effect, got {other:?}"),
    };
    let detail = "himalaya exited: password = \"███████\" token=██████ connect refused";
    let actions = vec![sanitized_failure(id, OperationKind::LoadPage(req), detail)];
    let text = draw_after(&mut state, &actions, 152, 40);

    // Title from the operation summary; exit code; both buttons; detail.
    assert!(text.contains("Loading messages failed"), "title:\n{text}");
    assert!(
        text.contains("himalaya exited with code 4"),
        "code:\n{text}"
    );
    assert!(text.contains("[ Retry ]"), "retry button:\n{text}");
    assert!(text.contains("[ Dismiss ]"), "dismiss button:\n{text}");
    assert!(
        text.contains("connect refused"),
        "safe detail must show:\n{text}"
    );
    // Fixture secrets never appear (acceptance: no fixture secrets).
    assert!(!text.contains("hunter2"), "secret leaked:\n{text}");
    assert!(!text.contains("hunter"), "secret fragment leaked:\n{text}");
    assert!(text.contains("███████"), "redaction marks missing:\n{text}");
    // Modal chrome.
    assert!(text.contains("Tab switch"), "hints missing:\n{text}");
}

#[test]
fn error_modal_detail_scrolls() {
    let mut state = mock_initial_state();
    let (id, req) = match reducer::reduce(&mut state, &Action::PageNext).as_slice() {
        [effect] => (
            effect.id,
            match &effect.kind {
                OperationKind::LoadPage(request) => request.clone(),
                other => panic!("unexpected kind {other:?}"),
            },
        ),
        other => panic!("expected one effect, got {other:?}"),
    };
    let kind = OperationKind::LoadPage(req);
    let lines: Vec<String> = (0..60).map(|i| format!("detail-line-{i:02}")).collect();
    let actions = vec![
        sanitized_failure(id, kind, &lines.join("\n")),
        // Scroll to the very end (the reducer clamps).
        Action::PageNext,
        Action::PageNext,
        Action::PageNext,
        Action::PageNext,
    ];
    let text = draw_after(&mut state, &actions, 152, 40);
    assert!(
        text.contains("detail-line-59"),
        "bottom of detail missing:\n{text}"
    );
    assert!(
        !text.contains("detail-line-00"),
        "top of detail should be scrolled away:\n{text}"
    );
    // Scrolling back to the top shows the first line again.
    let mut actions = actions;
    for _ in 0..80 {
        actions.push(Action::MoveUp);
    }
    let text = draw_after(&mut state, &actions, 152, 40);
    assert!(text.contains("detail-line-00"), "top missing:\n{text}");
    assert!(
        !text.contains("detail-line-59"),
        "bottom should be scrolled away:\n{text}"
    );
}

#[test]
fn ambiguous_failure_shows_duplicate_warning() {
    let mut state = mock_initial_state();
    let (id, req) = match reducer::reduce(&mut state, &Action::PageNext).as_slice() {
        [effect] => (
            effect.id,
            match &effect.kind {
                OperationKind::LoadPage(request) => request.clone(),
                other => panic!("unexpected kind {other:?}"),
            },
        ),
        other => panic!("expected one effect, got {other:?}"),
    };
    let action = Action::BackendCompleted(OperationResult {
        id,
        outcome: Err(OperationFailure {
            code: Some(1),
            detail: String::from("SMTP DATA failed: reached unexpected EOF"),
            retry: Some(OperationKind::LoadPage(req).retry_spec()),
            ambiguous: true,
        }),
    });
    let text = draw_after(&mut state, &[action], 152, 40);
    assert!(
        text.contains("duplicate"),
        "ambiguity warning missing:\n{text}"
    );
}

// ── Reader screen (plan §19 Phase 4) ─────────────────────────────────────

use tmail::app::Focus;
use tmail::app::route::{MessageRoute, Route};
use tmail::app::state::Loadable;
use tmail::domain::{MailboxId, Message, MessageId, Page};

/// A reader-open state: the selected mock summary is open with its mock
/// message loaded.
fn reader_state(selection: usize) -> tmail::app::AppState {
    let mut state = mock_initial_state();
    let summary = state.messages.items[selection].clone();
    let message = mock::mock_message(&summary);
    state.routes.push(Route::Message(MessageRoute {
        mailbox_id: MailboxId(String::from("inbox")),
        summary,
    }));
    state.open_message = Loadable::Loaded(message);
    state.focus = Focus::Reader;
    state
}

#[test]
fn reader_renders_exactly_one_message_document() {
    let mut state = reader_state(0);
    state.size = (152, 40);
    let text = draw_after(&mut state, &[], 152, 40);
    // Header block.
    assert!(
        text.contains("Re: WIP — 240 mm stainless-clad gyuto"),
        "subject missing:\n{text}"
    );
    assert!(text.contains("From"), "meta missing:\n{text}");
    assert!(
        text.contains("KKF Notifications"),
        "sender missing:\n{text}"
    );
    // Action row and hints: exactly one message, no thread navigation
    // (plan §4 overrides); no mode badge.
    assert!(text.contains("Archive e"), "actions missing:\n{text}");
    assert_absent(&text, "3 of 3", (152, 40));
    assert_absent(&text, "thread", (152, 40));
    assert_absent(&text, "READER", (152, 40));
    assert_absent(&text, "NORMAL", (152, 40));
    // Sidebar chrome stays visible.
    assert!(text.contains("Compose"), "sidebar hidden:\n{text}");
}

#[test]
fn reader_loading_state_renders_placeholder() {
    let mut state = reader_state(0);
    state.open_message = Loadable::Loading;
    let text = draw_after(&mut state, &[], 152, 40);
    assert!(text.contains("loading message…"), "placeholder:\n{text}");
    assert!(
        !text.contains("body line 01"),
        "body must not exist while loading:\n{text}"
    );
}

#[test]
fn reader_scrolls_body_with_reducer_state() {
    let mut state = reader_state(0);
    state.size = (152, 40);
    let total = tmail::ui::screens::reader::content_line_count(&state, state.size.0 as usize);
    let viewport = tmail::ui::layout::reader_rows_visible(state.size);
    assert!(total > viewport, "document must overflow: {total} lines");
    // Scroll to the end the way the reducer does.
    for _ in 0..total {
        reducer::reduce(&mut state, &Action::MoveDown);
    }
    let text = draw_after(&mut state, &[], 152, 40);
    assert!(
        text.contains(&format!("body line {:02}", 40)),
        "bottom of body missing:\n{text}"
    );
    assert!(
        !text.contains("body line 01"),
        "top of body should be scrolled away:\n{text}"
    );
}

#[test]
fn reader_idle_message_never_panics() {
    let mut state = mock_initial_state();
    // A reader route whose data was cleared (defensive state): the summary
    // snapshot still carries subject/sender, the body is the idle note.
    let summary = state.messages.items[1].clone();
    state.routes.push(Route::Message(MessageRoute {
        mailbox_id: MailboxId(String::from("inbox")),
        summary,
    }));
    state.open_message = Loadable::Idle;
    state.focus = Focus::Reader;
    let text = draw_after(&mut state, &[], 152, 40);
    assert!(
        text.contains("Re: gyuto for September"),
        "summary meta:\n{text}"
    );
    assert!(text.contains("(no message loaded)"), "idle note:\n{text}");
}

/// A message with missing subject/from/body headers renders explicit
/// placeholders: acceptance "reader handles missing subject/from/body".
#[test]
fn reader_handles_missing_fields() {
    let mut state = mock_initial_state();
    let mut summary = state.messages.items[1].clone();
    summary.subject = String::new();
    summary.from = Vec::new();
    summary.to = Vec::new();
    summary.timestamp = chrono::DateTime::from_timestamp(0, 0)
        .expect("epoch")
        .with_timezone(&chrono::FixedOffset::east_opt(0).expect("utc"));
    let message = Message {
        id: MessageId(String::from("m2")),
        mailbox_id: MailboxId(String::from("inbox")),
        headers: tmail::domain::MessageHeaders::default(),
        plain_body: None,
        html_body: None,
        attachments: Vec::new(),
    };
    state.routes.push(Route::Message(MessageRoute {
        mailbox_id: MailboxId(String::from("inbox")),
        summary,
    }));
    state.open_message = Loadable::Loaded(message);
    state.focus = Focus::Reader;
    let text = draw_after(&mut state, &[], 152, 40);
    assert!(
        text.contains("(no subject)"),
        "subject placeholder:\n{text}"
    );
    assert!(
        text.contains("(no content)"),
        "empty body placeholder:\n{text}"
    );
    assert!(
        text.contains("(unknown sender)"),
        "sender placeholder:\n{text}"
    );
    assert!(
        text.contains("(no recipients)"),
        "recipient placeholder:\n{text}"
    );
    assert!(text.contains("unknown date"), "date placeholder:\n{text}");
}

// ── Composer screen (plan §19 Phase 6.1) ─────────────────────────────────

#[test]
fn composer_renders_fields_actions_and_toggles() {
    let mut state = mock_initial_state();
    let text = draw_after(&mut state, &[Action::Compose], 152, 40);
    assert!(text.contains("New message"), "header missing:\n{text}");
    assert!(text.contains("      To"), "To label missing:\n{text}");
    assert!(
        text.contains("   Subject"),
        "Subject label missing:\n{text}"
    );
    assert!(text.contains("[Cc]"), "Cc toggle missing:\n{text}");
    assert!(text.contains("[Bcc]"), "Bcc toggle missing:\n{text}");
    assert!(text.contains("Send"), "send button missing:\n{text}");
    assert!(text.contains("Discard"), "discard button missing:\n{text}");
    // The mode badge is gone; the hints row still lists composer keys.
    assert_absent(&text, "COMPOSE", (152, 40));
}

#[test]
fn composer_renders_typed_text_and_revealed_fields() {
    let mut state = mock_initial_state();
    let actions: Vec<Action> = [
        Action::Compose,
        // "max@" into To.
        Action::ComposerEdit(tmail::app::action::ComposerEdit::Char('m')),
        Action::ComposerEdit(tmail::app::action::ComposerEdit::Char('a')),
        Action::ComposerEdit(tmail::app::action::ComposerEdit::Char('x')),
        Action::ComposerEdit(tmail::app::action::ComposerEdit::Char('@')),
        // Enter on the Cc toggle reveals the Cc row.
        Action::FocusNext,
        Action::Activate,
    ]
    .into_iter()
    .collect();
    let text = draw_after(&mut state, &actions, 152, 40);
    assert!(text.contains("max@"), "typed address missing:\n{text}");
    assert!(
        !text.contains("[Cc]"),
        "Cc toggle must hide once revealed:\n{text}"
    );
    // Subject label still there; body textarea occupies the lower pane.
    assert!(text.contains("Subject"), "subject row missing:\n{text}");
}

#[test]
fn composer_focused_field_draws_a_caret_cell() {
    let mut state = mock_initial_state();
    let actions: Vec<Action> = [
        Action::Compose,
        Action::ComposerEdit(tmail::app::action::ComposerEdit::Char('x')),
    ]
    .into_iter()
    .collect();
    let text = draw_after(&mut state, &actions, 152, 40);
    // "x" then the reversed caret block drawn at the value start column.
    assert!(
        text.contains("x "),
        "typed text + caret must render:\n{text}"
    );
}

#[test]
fn composer_flags_invalid_addresses_in_the_warning_color() {
    use tmail::app::action::ComposerEdit;
    let theme = Theme::default_dark();
    let mut state = mock_initial_state();
    let mut actions = vec![Action::Compose];
    for c in "broken".chars() {
        actions.push(Action::ComposerEdit(ComposerEdit::Char(c)));
    }
    let buffer = buffer_after(&mut state, &actions, 152, 40);
    let warned = buffer.content.iter().any(|cell| cell.fg == theme.warning);
    assert!(warned, "invalid address must render in warning color");

    // A fully valid address shows no warning anywhere.
    let mut state = mock_initial_state();
    let mut actions = vec![Action::Compose];
    for c in "max@example.com".chars() {
        actions.push(Action::ComposerEdit(ComposerEdit::Char(c)));
    }
    let buffer = buffer_after(&mut state, &actions, 152, 40);
    let warned = buffer.content.iter().any(|cell| cell.fg == theme.warning);
    assert!(!warned, "valid address must not warn");
}

// ── Search screen (plan §19 Phase 9) ─────────────────────────────────────

/// Search results reuse the mailbox list: the head names the query, and an
/// empty result set says so explicitly once the request has landed.
#[test]
fn search_results_render_query_head_and_empty_note() {
    let mut state = mock_initial_state();
    state.size = (152, 40);
    let mut actions: Vec<Action> = vec![Action::OpenSearch];
    for c in "quote".chars() {
        actions.push(Action::SearchEdit(tmail::app::action::SearchEdit::Char(c)));
    }
    actions.push(Action::SubmitSearch);
    let mut effects = Vec::new();
    for action in &actions {
        effects.extend(reducer::reduce(&mut state, action));
    }
    let search_id = match effects.as_slice() {
        [effect] => {
            assert!(
                matches!(effect.kind, OperationKind::Search(_)),
                "expected a Search operation"
            );
            effect.id
        }
        other => panic!("expected one effect, got {other:?}"),
    };
    // In flight: the head names the query; no empty note yet.
    let text = text_of(&buffer_after(&mut state, &[], 152, 40));
    assert!(text.contains("SEARCH — quote"), "head:\n{text}");
    assert!(
        !text.contains("(no results)"),
        "an in-flight search must not claim empty:\n{text}"
    );
    // An empty result page is valid and explicit (Phase 9.3).
    reducer::reduce(
        &mut state,
        &Action::BackendCompleted(OperationResult {
            id: search_id,
            outcome: Ok(OperationOutcome::Page(Page::empty(20))),
        }),
    );
    let text = text_of(&buffer_after(&mut state, &[], 152, 40));
    assert!(text.contains("SEARCH — quote"), "head stays:\n{text}");
    assert!(text.contains("(no results)"), "empty note:\n{text}");
}

#[test]
fn discard_dialog_renders_with_keep_as_the_safe_default() {
    let mut state = mock_initial_state();
    let actions: Vec<Action> = [
        Action::Compose,
        Action::ComposerEdit(tmail::app::action::ComposerEdit::Char('x')),
        Action::DiscardDraft,
    ]
    .into_iter()
    .collect();
    let text = draw_after(&mut state, &actions, 152, 40);
    assert!(text.contains("Discard draft?"), "title missing:\n{text}");
    assert!(
        text.contains("deleted permanently"),
        "body missing:\n{text}"
    );
    assert!(
        text.contains("[ Discard ]"),
        "discard button missing:\n{text}"
    );
    assert!(
        text.contains("[ Keep editing ]"),
        "keep button missing:\n{text}"
    );
    assert!(
        text.contains("(no subject)"),
        "empty subject preview:\n{text}"
    );
}

// ── Composer autosave status line (plan §14 Phase 6.7) ───────────────────

use tmail::app::operation::OperationOutcome;

#[test]
fn composer_shows_unsaved_changes_while_debouncing() {
    let mut state = mock_initial_state();
    let actions: Vec<Action> = [
        Action::Compose,
        Action::ComposerEdit(tmail::app::action::ComposerEdit::Char('x')),
    ]
    .into_iter()
    .collect();
    let text = draw_after(&mut state, &actions, 152, 40);
    assert!(
        text.contains("Unsaved changes"),
        "debouncing draft must show Unsaved changes:\n{text}"
    );
}

#[test]
fn composer_shows_draft_saved_with_the_time_after_success() {
    let mut state = mock_initial_state();
    // Compose, edit, let the debounce elapse, and confirm the save.
    let mut actions: Vec<Action> = vec![Action::Compose];
    actions.push(Action::Tick {
        now: Box::new(mock::now()),
    });
    actions.push(Action::ComposerEdit(
        tmail::app::action::ComposerEdit::Char('x'),
    ));
    actions.push(Action::Tick {
        now: Box::new(mock::now() + chrono::Duration::seconds(2)),
    });
    // Find the save operation and confirm it.
    let mut pending = None;
    for action in &actions {
        for effect in reducer::reduce(&mut state, action) {
            pending = Some(effect.id);
        }
    }
    let id = pending.expect("a save was started");
    reducer::reduce(
        &mut state,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::DraftSaved {
                remote_id: MessageId(String::from("remote-1")),
            }),
        }),
    );
    let text = draw_after(&mut state, &[], 152, 40);
    assert!(
        text.contains("Draft saved · 10:47"),
        "mock now is 10:47 +03:\n{text}"
    );
}

#[test]
fn composer_shows_save_failed_after_a_failure() {
    let mut state = mock_initial_state();
    let mut actions: Vec<Action> = vec![Action::Compose];
    actions.push(Action::Tick {
        now: Box::new(mock::now()),
    });
    actions.push(Action::ComposerEdit(
        tmail::app::action::ComposerEdit::Char('x'),
    ));
    actions.push(Action::Tick {
        now: Box::new(mock::now() + chrono::Duration::seconds(2)),
    });
    let mut pending = None;
    for action in &actions {
        for effect in reducer::reduce(&mut state, action) {
            pending = Some(effect.id);
        }
    }
    let id = pending.expect("a save was started");
    reducer::reduce(
        &mut state,
        &Action::BackendCompleted(OperationResult {
            id,
            outcome: Err(OperationFailure {
                code: Some(1),
                detail: String::from("imap down"),
                retry: None,
                ambiguous: false,
            }),
        }),
    );
    // Dismiss the modal: the composer header must keep flagging the failure.
    reducer::reduce(&mut state, &Action::DismissError);
    let text = draw_after(&mut state, &[], 152, 40);
    assert!(
        text.contains("Save failed"),
        "failure must stay visible:\n{text}"
    );
}

#[test]
fn composer_shows_saving_while_the_save_is_in_flight() {
    let mut state = mock_initial_state();
    let mut actions: Vec<Action> = vec![Action::Compose];
    actions.push(Action::Tick {
        now: Box::new(mock::now()),
    });
    actions.push(Action::ComposerEdit(
        tmail::app::action::ComposerEdit::Char('x'),
    ));
    actions.push(Action::Tick {
        now: Box::new(mock::now() + chrono::Duration::seconds(2)),
    });
    for action in &actions {
        reducer::reduce(&mut state, action);
    }
    let text = draw_after(&mut state, &[], 152, 40);
    assert!(
        text.contains("Saving…"),
        "in-flight save must show Saving…:\n{text}"
    );
}

// ── Phase 10: responsive snapshot sizes + mouse hit maps ─────────────────

use tmail::app::action::ClickTarget;

/// Draw an arbitrary reducer-driven state and return buffer + hit map.
fn draw_state_hits(
    state: &tmail::app::AppState,
    width: u16,
    height: u16,
) -> (ratatui::buffer::Buffer, HitMap) {
    let theme = Theme::default_dark();
    let now = mock::now();
    let ctx = RenderContext::new(now, dates::format_clock(now));
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend");
    let mut hits = HitMap::default();
    terminal
        .draw(|frame| render(frame, state, &theme, &ctx, &mut hits))
        .expect("draw");
    (terminal.backend().buffer().clone(), hits)
}

/// Plan §19 Phase 10 acceptance: snapshots cover 152×40, 120×30, 90×25,
/// and too-small. 120×30 is the full-layout floor: the sidebar is present.
#[test]
fn snapshot_at_full_floor_120x30() {
    let (buffer, _) = draw_with_hits(120, 30);
    let text = text_of(&buffer);
    assert!(text.contains("INBOX"), "pane title missing:\n{text}");
    assert!(text.contains("Compose"), "full floor keeps the sidebar");
    assert!(text.contains("UTF-8 · 120×30"), "env info missing");
}

/// 90×25 is the compact floor: the sidebar hides but rows still render.
#[test]
fn snapshot_at_compact_floor_90x25() {
    let (buffer, _) = draw_with_hits(90, 25);
    let text = text_of(&buffer);
    assert!(text.contains("INBOX"), "pane title missing:\n{text}");
    // The compact sender column clips to 14ch; the subject stays whole.
    assert!(text.contains("Re: WIP — 240 mm"), "rows missing:\n{text}");
    assert_absent(&text, "Compose", (90, 25));
    assert!(text.contains("UTF-8 · 90×25"), "env info missing");
}

/// 89 wide is already too small (compact floor is 90): the message shows
/// instead of overlapping widgets.
#[test]
fn snapshot_too_small_just_below_the_compact_floor() {
    let (buffer, _) = draw_with_hits(89, 25);
    let text = text_of(&buffer);
    assert!(
        text.contains("Terminal too small"),
        "message missing:\n{text}"
    );
    assert!(text.contains("89×25"), "size report missing");
    assert_absent(&text, "INBOX", (89, 25));
}

#[test]
fn hit_map_matches_the_drawn_mailbox_screen() {
    let (_, hits) = draw_with_hits(152, 40);
    let _ = hits_is_sane(&hits);
    // Chrome geometry: topbar 0..4, body 4..37, statusbar 37..40; list head
    // 4..6, rows from y=6. Sidebar x=0..23, list x=24..152.
    assert_eq!(hits.hit_test(40, 1, false), Some(ClickTarget::SearchField));
    assert_eq!(
        hits.hit_test(10, 6, false),
        Some(ClickTarget::ComposeButton)
    );
    assert_eq!(hits.hit_test(10, 9, false), Some(ClickTarget::Mailbox(0)));
    assert_eq!(hits.hit_test(10, 10, false), Some(ClickTarget::Mailbox(1)));
    assert_eq!(
        hits.hit_test(30, 6, false),
        Some(ClickTarget::MessageRow(0))
    );
    assert_eq!(
        hits.hit_test(30, 7, false),
        Some(ClickTarget::MessageRow(1))
    );
    // Rows only exist where the page has items: 20 mock rows cover
    // y=6..26; the empty tail records nothing.
    assert_eq!(
        hits.hit_test(30, 25, false),
        Some(ClickTarget::MessageRow(19))
    );
    assert_eq!(hits.hit_test(30, 30, false), None);
    assert_eq!(hits.hit_test(30, 35, false), None);
    assert_eq!(hits.hit_test(200, 5, false), None);
}

fn hits_is_sane(hits: &HitMap) -> bool {
    !hits.is_empty()
}

#[test]
fn hit_map_records_modal_buttons_and_blocks_click_through() {
    use tmail::app::overlay::ModalButton;
    let mut state = mock_initial_state();
    let failure = Action::BackendCompleted(OperationResult {
        id: reducer_start_page(&mut state),
        outcome: Err(OperationFailure {
            code: Some(1),
            detail: String::from("short detail"),
            retry: Some(OperationKind::LoadMailboxes.retry_spec()),
            ambiguous: false,
        }),
    });
    reducer::reduce(&mut state, &failure);
    let (_, hits) = draw_state_hits(&state, 152, 40);
    // The modal geometry comes from the same layout the renderer uses.
    let layout = tmail::ui::components::error_modal::layout((152, 40), Some(1), false);
    let button_y = layout.area.y + layout.area.height - 3;
    let retry_x = layout.area.x + 2;
    assert_eq!(
        hits.hit_test(retry_x + 2, button_y, true),
        Some(ClickTarget::ErrorButton(ModalButton::Retry))
    );
    assert_eq!(
        hits.hit_test(retry_x + 14, button_y, true),
        Some(ClickTarget::ErrorButton(ModalButton::Dismiss))
    );
    // Clicking "through" the modal onto the list behind does nothing.
    assert_eq!(hits.hit_test(30, 6, true), None);
}

/// Start a page request like the runtime would and return its id.
fn reducer_start_page(state: &mut tmail::app::AppState) -> tmail::app::OperationId {
    let effects = reducer::reduce(state, &Action::PageNext);
    match &effects[..] {
        [effect] => effect.id,
        other => panic!("expected one effect, got {other:?}"),
    }
}

#[test]
fn hit_map_records_composer_controls() {
    use tmail::app::action::ClickTarget as Target;
    use tmail::app::composer::ComposerField;
    let mut state = mock_initial_state();
    reducer::reduce(&mut state, &Action::Compose);
    let (_, hits) = draw_state_hits(&state, 152, 40);
    // Chrome: body area y=4..37; the action row is its last line (y=36).
    // Send sits first, Discard after the three-space gap.
    assert_eq!(
        hits.hit_test(28, 36, false),
        Some(Target::ComposerField(ComposerField::Send))
    );
    assert_eq!(
        hits.hit_test(42, 36, false),
        Some(Target::ComposerField(ComposerField::Discard))
    );
    // The attach control is on the row above (y=35), after the chip area.
    assert_eq!(
        hits.hit_test(28, 35, false),
        Some(Target::ComposerField(ComposerField::Attach))
    );
    // The To field row: the first field row under the header (y=5).
    assert_eq!(
        hits.hit_test(30, 5, false),
        Some(Target::ComposerField(ComposerField::To))
    );
    // The Cc toggle rides the To row's right edge (inner width 124:
    // value 0 + pad, then the two toggles at the far right).
    assert_eq!(
        hits.hit_test(139, 5, false),
        Some(Target::ComposerField(ComposerField::CcToggle))
    );
    assert_eq!(
        hits.hit_test(146, 5, false),
        Some(Target::ComposerField(ComposerField::BccToggle))
    );
    // The body area focuses the body.
    assert_eq!(
        hits.hit_test(60, 20, false),
        Some(Target::ComposerField(ComposerField::Body))
    );
}

#[test]
fn mouse_translation_end_to_end_uses_the_rendered_map() {
    use tmail::input::mouse;
    let mut state = mock_initial_state();
    state.size = (152, 40);
    let (_, hits) = draw_state_hits(&state, 152, 40);
    // A wheel event over the list scrolls the focused list (plan §10).
    let wheel = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::ScrollDown,
        column: 30,
        row: 8,
        modifiers: crossterm::event::KeyModifiers::empty(),
    };
    assert_eq!(
        mouse::to_action(wheel, &hits, &state),
        Some(Action::MoveDown)
    );
    // A click on the second row selects it through the same action
    // vocabulary.
    let click = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
        column: 30,
        row: 7,
        modifiers: crossterm::event::KeyModifiers::empty(),
    };
    assert_eq!(
        mouse::to_action(click, &hits, &state),
        Some(Action::Click(ClickTarget::MessageRow(1)))
    );
    // Motion events bind to nothing.
    let motion = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Moved,
        column: 30,
        row: 7,
        modifiers: crossterm::event::KeyModifiers::empty(),
    };
    assert_eq!(mouse::to_action(motion, &hits, &state), None);
}

/// Plan §18: the app must render in no-color terminals. `Theme::monochrome`
/// (selected by `NO_COLOR`) draws the same content with default colors and
/// no emphasis modifiers, and the frame never panics.
#[test]
fn monochrome_theme_renders_the_same_content() {
    let mut state = mock_initial_state();
    state.size = (152, 40);
    let theme = Theme::monochrome();
    let now = mock::now();
    let ctx = RenderContext::new(now, dates::format_clock(now));
    let backend = TestBackend::new(152, 40);
    let mut terminal = Terminal::new(backend).expect("test backend");
    let mut hits = HitMap::default();
    terminal
        .draw(|frame| render(frame, &state, &theme, &ctx, &mut hits))
        .expect("monochrome draw");
    let text = text_of(terminal.backend().buffer());
    assert!(text.contains("INBOX"), "content missing:\n{text}");
    assert!(text.contains("KKF Notificati"), "rows missing:\n{text}");
    assert!(text.contains("UTF-8 · 152×40"), "status bar missing");
    // Hit maps still record in monochrome.
    assert_eq!(
        hits.hit_test(30, 6, false),
        Some(ClickTarget::MessageRow(0))
    );
}

/// Plan §18: no Nerd Font dependency — every rendered symbol is ordinary
/// Unicode (box drawing, arrows, punctuation) or ASCII. This pins the
/// symbol inventory so a Nerd-Font codepoint cannot sneak in unnoticed.
#[test]
fn rendered_symbols_stay_outside_the_nerd_font_plane() {
    for (buffer, _) in [
        draw_with_hits(152, 40),
        draw_with_hits(90, 25),
        draw_with_hits(60, 15),
    ] {
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                for ch in buffer[(x, y)].symbol().chars() {
                    let code = ch as u32;
                    // Nerd Fonts use the Private Use Areas; ordinary
                    // terminal symbols never live there.
                    assert!(
                        !(0xE000..=0xF8FF).contains(&code)
                            && !(0xF0000..=0xFFFFD).contains(&code)
                            && !(0x100000..=0x10FFFD).contains(&code),
                        "private-use (Nerd Font) codepoint {ch:?} rendered"
                    );
                }
            }
        }
    }
}

// ── Phase 10.5: mockup density and hierarchy (list/viewer/new-mail) ──────

/// (line index, first display column) of the first occurrence of
/// `needle`. Columns are char counts, not byte offsets, so lines holding
/// multibyte box-drawing characters still compare by what the eye sees.
fn position_of(text: &str, needle: &str) -> (usize, usize) {
    for (line_index, line) in text.lines().enumerate() {
        if let Some(col) = line.find(needle) {
            return (line_index, line[..col].chars().count());
        }
    }
    panic!("{needle:?} not found in:\n{text}");
}

/// Mockup `list.html`: the pane head keeps select-all + uppercase title +
/// unread sub + right-aligned range on one row, and every message is one
/// row of marker | star | from | subject(+snippet) | date.
#[test]
fn list_matches_mockup_density_and_hierarchy() {
    let (buffer, _) = draw_with_hits(152, 40);
    let text = text_of(&buffer);
    // Head row: title, unread count, and the range share one line, with
    // the range near the right edge (mockup `.pane-range` margin-left:auto).
    let (head_y, title_x) = position_of(&text, "[ ]  INBOX");
    let (_, unread_x) = position_of(&text, "24 unread");
    let (_, range_x) = position_of(&text, "1–20 of 25");
    assert_eq!(head_y, 4, "head sits under the topbar");
    assert!(title_x < unread_x, "unread sub follows the title");
    assert!(range_x > 120, "range is right-aligned, found at {range_x}");
    // A starred row keeps the single-line grid: star | from | subject |
    // snippet | date on one row (mockup `.mail` grid).
    let (row_y, star_x) = position_of(&text, "*PayPal");
    let (_, from_x) = position_of(&text, "PayPal");
    let (_, subject_x) = position_of(&text, "Payment received");
    let (_, date_x) = position_of(&text, "Yest");
    assert_eq!(row_y, 9, "rows start under the head");
    assert!(star_x < from_x && from_x < subject_x && subject_x < date_x);
    // Full mode shows snippets inline after the subject (mockup `.snippet`).
    assert!(
        text.contains("order #214, 50% deposit"),
        "snippet missing:\n{text}"
    );
}

/// Mockup `viewer.html`: subject, then From/To/Cc/Date meta, then the
/// action row, then a hairline, then the body — top to bottom, one
/// message, no thread chrome.
#[test]
fn reader_matches_mockup_hierarchy() {
    let mut state = reader_state(0);
    let text = draw_after(&mut state, &[], 152, 40);
    let (subject_y, _) = position_of(&text, "Re: WIP — 240 mm stainless-clad gyuto");
    let (from_y, _) = position_of(&text, "From ");
    let (to_y, _) = position_of(&text, "To   ");
    let (date_y, _) = position_of(&text, "Date ");
    let (actions_y, _) = position_of(&text, "Archive e");
    let (body_y, _) = position_of(&text, "body line 01");
    assert!(subject_y < from_y, "subject first");
    assert!(from_y < to_y && to_y < date_y, "meta block in order");
    assert!(date_y < actions_y, "actions follow the meta");
    assert!(actions_y < body_y, "body follows the actions");
    // A hairline separates actions from the body (mockup `.thread-actions`
    // border-bottom).
    assert!(
        text.lines()
            .nth(actions_y + 1)
            .is_some_and(|l| l.contains('─')),
        "hairline under the actions"
    );
}

/// Mockup `new-mail.html`: right-aligned 8ch labels, Cc/Bcc toggles on the
/// To row, the attach row above Send/Discard, and the header on top.
#[test]
fn composer_matches_mockup_hierarchy_and_density() {
    let mut state = mock_initial_state();
    reducer::reduce(&mut state, &Action::Compose);
    let text = draw_after(&mut state, &[], 152, 40);
    let (header_y, _) = position_of(&text, "New message");
    let (to_y, _) = position_of(&text, "[Cc]");
    let (subject_y, subject_x) = position_of(&text, " Subject");
    let (attach_y, _) = position_of(&text, "[ + attach ]");
    let (send_y, send_x) = position_of(&text, "[ Send");
    let (_, discard_x) = position_of(&text, " Discard ");
    // Vertical order: header, To row, Subject row, attach row, actions.
    assert!(header_y < to_y && to_y < subject_y);
    assert!(subject_y < attach_y && attach_y < send_y);
    // The label column is right-aligned within 8ch starting at x=26
    // (mockup `grid-template-columns: 8ch` + the 2ch body padding): on the
    // To row "To" sits at 26+6, and the Cc/Bcc toggles ride the same row's
    // right edge (mockup `.field-extra`).
    let to_row = text.lines().nth(to_y).expect("To row");
    let to_x = to_row
        .find("To")
        .map(|byte| to_row[..byte].chars().count())
        .expect("To label on its row");
    assert_eq!(to_x, 32, "To label right-aligned at 26+6");
    assert_eq!(subject_x, 26, "Subject fills the 8ch label column");
    let cc_x = to_row
        .find("[Cc]")
        .map(|byte| to_row[..byte].chars().count())
        .expect("Cc toggle");
    assert!(cc_x >= 130, "toggles are right-aligned, at {cc_x}");
    // Action row: Send first, Discard after it (mockup `.compose-actions`).
    assert!(send_x < discard_x, "Send precedes Discard");
}

/// The reader action row is clickable (plan §10 "action button"): each
/// segment is its own region at the exact columns where its label draws,
/// and every click maps to the action the key would run (yemf).
#[test]
fn hit_map_records_reader_action_row_segments() {
    use tmail::app::action::{ClickTarget as Target, ReaderAction};
    let mut state = mock_initial_state();
    let summary = state.messages.items[0].clone();
    let message = mock::mock_message(&summary);
    state.routes.push(Route::Message(MessageRoute {
        mailbox_id: MailboxId(String::from("inbox")),
        summary,
    }));
    state.open_message = Loadable::Loaded(message);
    state.focus = Focus::Reader;
    let (_, hits) = draw_state_hits(&state, 152, 40);
    // Document geometry at 152×40: subject y=4, From/To/Date y=5..7, so
    // the action row draws at y=8 starting at the list edge x=24.
    assert_eq!(
        hits.hit_test(25, 8, false),
        Some(Target::ReaderAction(ReaderAction::Reply))
    );
    assert_eq!(
        hits.hit_test(36, 8, false),
        Some(Target::ReaderAction(ReaderAction::Forward))
    );
    assert_eq!(
        hits.hit_test(47, 8, false),
        Some(Target::ReaderAction(ReaderAction::Archive))
    );
    assert_eq!(
        hits.hit_test(60, 8, false),
        Some(Target::ReaderAction(ReaderAction::Star))
    );
    // The separator between segments stays inside a segment's region only
    // by belonging to the preceding label's end — a click on the gap right
    // before "Archive" hits Archive only inside its label; the gap itself
    // is the tail of "Forward f"'s trailing separator, owned by Archive's
    // start: assert the boundary behaves (gap after "Forward f" hits
    // nothing until Archive's own first column).
    let archive_x = 24 + "Reply r".width() + 3 + "Forward f".width() + 3;
    assert_eq!(
        hits.hit_test(archive_x as u16, 8, false),
        Some(Target::ReaderAction(ReaderAction::Archive))
    );
    assert_eq!(
        hits.hit_test(archive_x as u16 - 1, 8, false),
        None,
        "the separator column binds to no action"
    );
}

use unicode_width::UnicodeWidthStr as _;
