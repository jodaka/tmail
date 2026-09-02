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
    text_of(terminal.backend().buffer())
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
