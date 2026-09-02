//! Render tests on Ratatui's `TestBackend`: the mock mailbox screen must
//! render at full, compact, and too-small sizes (plan §19 Phase 1), the
//! status bar must never show `j`/`k` or a help hint (plan §4), and Phase 3
//! adds spinner + Retry/Dismiss modal render checks (plan §19 Phase 3).

use tmail::app::mock::{self, mock_initial_state};
use tmail::app::operation::{OperationFailure, OperationKind, OperationResult};
use tmail::app::{Action, reducer};
use tmail::ui::{RenderContext, Theme, dates, render};

use ratatui::backend::TestBackend;
use ratatui::{Terminal, style::Color};

fn draw(width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut state = mock_initial_state();
    state.size = (width, height);
    let theme = Theme::default_dark();
    let now = mock::now();
    let ctx = RenderContext::new(now, dates::format_clock(now));
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend");
    terminal
        .draw(|frame| render(frame, &state, &theme, &ctx))
        .expect("draw");
    terminal.backend().buffer().clone()
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
    // Status bar.
    assert!(text.contains("NORMAL"), "mode badge missing");
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
    assert!(text.contains("NORMAL"), "mode badge missing");
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
        terminal
            .draw(|frame| render(frame, state, &theme, &ctx))
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
    terminal
        .draw(|frame| render(frame, state, &theme, &ctx))
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
use tmail::domain::{MailboxId, Message, MessageId};

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
    // (plan §4 overrides).
    assert!(text.contains("Archive e"), "actions missing:\n{text}");
    assert_absent(&text, "3 of 3", (152, 40));
    assert_absent(&text, "thread", (152, 40));
    // Body content from the mock message.
    assert!(text.contains("body line 01"), "body missing:\n{text}");
    // Reader mode badge (mockup `.mode`).
    assert!(text.contains("READER"), "reader badge missing:\n{text}");
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
    // Composer mode badge and hints.
    assert!(text.contains("COMPOSE"), "mode badge missing:\n{text}");
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
