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
    state.session.size = (width, height);
    // Production renders with the active palette each frame (main), so the
    // harness does too — the theme picker previews by switching it.
    let theme = state.active_theme();
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
    assert!(
        text.contains(concat!("tmail v", env!("CARGO_PKG_VERSION"))),
        "brand missing:\n{text}"
    );
    // Search field with placeholder.
    assert!(text.contains("Search mail"), "search placeholder missing");
    // Sidebar: folders with unread counts (the Compose affordance was
    // removed in the redesign; the composer opens via `c`). No labels,
    // no storage.
    assert!(text.contains("Inbox"), "inbox folder missing");
    assert!(text.contains("Archive"), "folders missing");
    // Brand: the accented bullet and the dim version ride the panel.
    assert!(text.contains("• tmail"), "brand bullet missing");
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
    assert_absent(&text, "UTF-8", size);
    assert_absent(&text, "152×40", size);
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
    // Somewhere on the selected row the marker fill must be active — the
    // gold highlight, not the blue accent.
    let has_accent_bg = (0..buffer.area.width).any(|x| {
        let style = buffer[(x, row as u16)].style();
        style.bg == Some(theme.marker)
    });
    assert!(has_accent_bg, "selected row lacks marker fill:\n{text}");
    // And the fill is one color across the whole row: every cell from the
    // bar to the date carries the marker gold — a mixed fill (bar, text,
    // preview, and padding each their own bg) reads as stripes.
    for x in tmail::ui::layout::SIDEBAR_WIDTH..buffer.area.width {
        assert_eq!(
            buffer[(x, row as u16)].bg,
            theme.marker,
            "selected row cell {x} breaks the single fill:\n{text}"
        );
    }
}

/// In the Drafts mailbox the sender column would carry the user's own
/// address on every row, so rows show the "To" recipient instead. Search
/// results over Drafts follow the same rule (per-row mailbox match).
#[test]
fn drafts_rows_show_recipients_instead_of_senders() {
    let mut state = mock_initial_state();
    let drafts = mock::mock_mailboxes()
        .into_iter()
        .find(|m| m.role == Some(tmail::domain::MailboxRole::Drafts))
        .expect("mock has a drafts mailbox");
    state.session.routes = vec![tmail::app::route::Route::Mailbox(
        tmail::app::route::MailboxRoute {
            mailbox_id: drafts.id.clone(),
        },
    )];
    state.mailbox_selection = mock::mock_mailboxes()
        .iter()
        .position(|m| m.id == drafts.id)
        .unwrap();
    state.messages = mock::mock_page(&drafts.id, 0, mock::PAGE_SIZE);
    state.selection = 0;
    state.session.size = (152, 40);
    let (buffer, _) = draw_state_hits(&state, 152, 40);
    let text = text_of(&buffer);
    assert!(
        text.contains("DRAFTS"),
        "drafts pane title missing:\n{text}"
    );
    assert!(
        text.contains("Bob Smith"),
        "drafts rows must show the recipient:\n{text}"
    );
    assert_absent(&text, "Tmail Probe", (152, 40));
    assert_absent(&text, "(no recipients)", (152, 40));
}

/// The mockup's `.sidebar` border-right used to be a hairline divider
/// column; the redesign lets the two fills do the separation: the
/// `sidebar_bg` panel runs through the sidebar's last column even below
/// (or above) the folder rows, while the message list sits on the page
/// background.
#[test]
fn sidebar_divider_separates_folders_from_messages() {
    let theme = Theme::default_dark();
    let buffer = draw(152, 40);
    let top = tmail::ui::layout::TOPBAR_HEIGHT;
    let bottom = 40 - tmail::ui::layout::STATUSBAR_HEIGHT;
    let margin_x = tmail::ui::layout::SIDEBAR_WIDTH - 1;
    // The empty region below the folder rows: the fill runs to the last
    // sidebar column and the message list starts on the page background.
    for y in top + 8..bottom {
        assert_eq!(
            buffer[(margin_x, y)].bg,
            theme.sidebar_bg,
            "sidebar fill at {margin_x},{y}"
        );
        assert_eq!(
            buffer[(margin_x + 1, y)].bg,
            theme.background,
            "a gap between the fills at {margin_x},{y}"
        );
    }
}

/// The accent bar marks focus, not selection: whichever pane holds focus
/// carries the bar on its cursor row, and the other pane carries none.
#[test]
fn focus_marker_follows_the_focused_pane() {
    // Default state: the message list holds focus. The active folder row
    // carries the bar permanently (mockup `.folder.active`), the list
    // cursor row carries it while the list holds focus.
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
        "▎",
        "list focused: selected message shows the bar"
    );
    assert_eq!(
        buffer[(0, folder_row)].symbol(),
        "▎",
        "the active folder shows the bar in both focieres"
    );

    // Sidebar focused: the cursor folder (Inbox, selection 0) shows the
    // bar and the selected message does not.
    let mut state = mock_initial_state();
    state.session.focus = tmail::app::Focus::Sidebar;
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
        "▎",
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

/// Selection must not erase the unread signal: a focused unread row keeps
/// its bold under the accent fill, while a focused read row stays regular.
#[test]
fn selected_unread_row_stays_bold_and_selected_read_row_does_not() {
    // The list pane starts after the sidebar divider; the sidebar shares
    // buffer rows with the list and renders bold unread counters, so the
    // bold scans below must look at the list columns only.
    let list_x = tmail::ui::layout::SIDEBAR_WIDTH;

    // Selection 0 (KKF Notifications) is unread and the list holds focus.
    let mut state = mock_initial_state();
    let buffer = buffer_after(&mut state, &[], 152, 40);
    let text = text_of(&buffer);
    let unread_row = text
        .lines()
        .position(|line| line.contains("KKF Notifications"))
        .expect("selected unread row") as u16;
    let unread_bold = (list_x..buffer.area.width).any(|x| {
        buffer[(x, unread_row)]
            .style()
            .add_modifier
            .contains(ratatui::style::Modifier::BOLD)
    });
    assert!(unread_bold, "selected unread row must stay bold:\n{text}");

    // Move the cursor onto a read row (PayPal, the fourth message): the
    // fill moves with it but no bold may appear anywhere on the row.
    let mut state = mock_initial_state();
    let buffer = buffer_after(
        &mut state,
        &[Action::MoveDown, Action::MoveDown, Action::MoveDown],
        152,
        40,
    );
    let text = text_of(&buffer);
    let read_row = text
        .lines()
        .position(|line| line.contains("PayPal"))
        .expect("selected read row") as u16;
    let read_bold = (list_x..buffer.area.width).any(|x| {
        buffer[(x, read_row)]
            .style()
            .add_modifier
            .contains(ratatui::style::Modifier::BOLD)
    });
    assert!(!read_bold, "selected read row must not be bold:\n{text}");
}

/// The theme picker (ticket k5ba): `t` opens a small dialog listing every
/// palette, the cursor row carries the list's accent fill, and moving the
/// cursor previews the highlighted palette behind the dialog at once.
#[test]
fn theme_picker_lists_previews_and_highlights() {
    let dark = Theme::default_dark();
    let light = Theme::default_light();

    // `t` opens the picker; every palette is named, the hint is shown.
    let mut state = mock_initial_state();
    let buffer = buffer_after(&mut state, &[Action::OpenThemePicker], 152, 40);
    let text = text_of(&buffer);
    assert!(text.contains(" Theme "), "dialog title missing:\n{text}");
    assert!(text.contains("default"), "builtin palette missing:\n{text}");
    assert!(text.contains("light"), "builtin palette missing:\n{text}");
    assert!(text.contains("↵ confirm"), "binding hint missing:\n{text}");

    // The cursor row (the active palette) carries the marker fill like a
    // focused list row.
    let cursor_row = text
        .lines()
        .position(|line| line.contains("default"))
        .expect("cursor row") as u16;
    let highlighted =
        (0..buffer.area.width).any(|x| buffer[(x, cursor_row)].style().bg == Some(dark.marker));
    assert!(highlighted, "cursor row lacks the marker fill:\n{text}");

    // Moving the cursor previews the highlighted palette behind the
    // dialog: the message-list rows repaint at once (ticket k5ba).
    let mut state = mock_initial_state();
    let buffer = buffer_after(
        &mut state,
        &[Action::OpenThemePicker, Action::MoveDown],
        152,
        40,
    );
    // A message row cell: the list sits on the palette's page background.
    let preview_bg = buffer[(130, 15)].style().bg;
    assert_eq!(
        preview_bg,
        Some(light.background),
        "the highlighted palette must preview behind the dialog"
    );
    // And the fill moved to the newly highlighted row.
    let text = text_of(&buffer);
    let cursor_row = text
        .lines()
        .position(|line| line.contains("light"))
        .expect("previewed row") as u16;
    let highlighted =
        (0..buffer.area.width).any(|x| buffer[(x, cursor_row)].style().bg == Some(light.marker));
    assert!(highlighted, "previewed row lacks the marker fill:\n{text}");
}

/// A theme list longer than the dialog scrolls, with a scrollbar in the
/// last inner column — the message list's anatomy (ticket k5ba).
#[test]
fn theme_picker_shows_a_scrollbar_when_the_list_overflows() {
    let theme = Theme::default_dark();
    let mut state = mock_initial_state();
    // Twelve palettes against the dialog's ten visible rows: it scrolls.
    state.settings.themes = (0..12)
        .map(|i| (format!("theme-{i:02}"), Theme::default_dark()))
        .collect();

    // Dialog geometry at 152×40: width 34, height 13 → inner rows at
    // y = 14..24 (10 rows + hint), scrollbar column at x = 90.
    let buffer = buffer_after(&mut state, &[Action::OpenThemePicker], 152, 40);
    let text = text_of(&buffer);
    assert!(text.contains("theme-00"), "first theme visible:\n{text}");
    assert!(text.contains("theme-09"), "tenth theme visible:\n{text}");
    assert!(
        !text.contains("theme-10"),
        "eleventh theme stays below the fold:\n{text}"
    );
    let scroll_x = 90;
    // The thumb sits at the top of the track while the window is at the
    // top, and the track continues below it.
    assert_eq!(
        buffer[(scroll_x, 14)].symbol(),
        "█",
        "thumb at the top of the track:\n{text}"
    );
    assert_eq!(
        buffer[(scroll_x, 23)].symbol(),
        "│",
        "track below the thumb:\n{text}"
    );
    // The highlight ends before the scrollbar column, never under it.
    assert_ne!(
        buffer[(scroll_x, 14)].style().bg,
        Some(theme.marker),
        "the row fill must not run under the scrollbar"
    );

    // Scrolling to the last theme moves the window and the thumb with it.
    // The scrollbar maps the window position across the whole content (the
    // same proportional look as the message list), so at the bottom the
    // thumb has moved off the top of the track without pinning to its end.
    let downs: Vec<Action> = (0..11).map(|_| Action::MoveDown).collect();
    let buffer = buffer_after(&mut state, &downs, 152, 40);
    let text = text_of(&buffer);
    assert!(
        text.contains("theme-11"),
        "the last theme scrolled into view:\n{text}"
    );
    assert!(
        text.contains("theme-02"),
        "the window keeps the last ten rows:\n{text}"
    );
    assert_eq!(
        buffer[(scroll_x, 14)].symbol(),
        "│",
        "the thumb moved off the top of the track:\n{text}"
    );
    assert!(
        (14..24).any(|y| buffer[(scroll_x, y)].symbol() == "█"),
        "the thumb stays visible on the track:\n{text}"
    );
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
        Action::Resize {
            width: 100,
            height: 30,
        },
    );
    let compact = draw_state(&state, 100, 30);
    // The sidebar hides in compact (the Compose affordance is gone with
    // the redesign; the folder rows are the marker).
    assert!(!compact.contains("Inbox"));

    reducer::reduce(
        &mut state,
        Action::Resize {
            width: 60,
            height: 15,
        },
    );
    let small = draw_state(&state, 60, 15);
    assert!(small.contains("Terminal too small"));

    reducer::reduce(
        &mut state,
        Action::Resize {
            width: 152,
            height: 40,
        },
    );
    let full = draw_state(&state, 152, 40);
    assert!(full.contains("Inbox"), "the sidebar is back");
    assert!(!full.contains("Compose"));
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
    let now = mock::now();
    let ctx = RenderContext::new(now, dates::format_clock(now));
    for action in actions.iter().cloned() {
        reducer::reduce(state, action);
    }
    // Production renders with the active palette each frame (main), so the
    // harness reads it after the actions ran — the theme picker previews
    // by switching it.
    let theme = state.active_theme();
    state.session.size = (width, height);
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
    // An empty list with a page load in flight (startup / mailbox switch
    // look): the loader runs in the status bar's left corner (ticket m3by,
    // moved from under the brand) and the list pane shows its own centered
    // spinner. The cold-context cache read (ticket haeb) is background
    // work and shows no spinner, so the test completes it with a miss —
    // which is what starts the fresh foreground load the user waits on.
    state.messages = Page::empty(20);
    let effects = reducer::reduce(&mut state, Action::Refresh);
    let [effect] = effects.as_slice() else {
        panic!("the cold refresh reads the cache first: {effects:?}");
    };
    reducer::reduce(
        &mut state,
        Action::BackendCompleted(OperationResult {
            id: effect.id,
            outcome: Ok(OperationOutcome::CacheMiss),
        }),
    );
    let buffer = buffer_after(&mut state, &[], 152, 40);
    let text = text_of(&buffer);
    assert!(text.contains("■⬝"), "spinner frame missing:\n{text}");
    assert!(
        text.contains(concat!("tmail v", env!("CARGO_PKG_VERSION"))),
        "the brand row keeps only the brand:\n{text}"
    );
    // The scanner sits in the footer's left corner: two spaces in from the
    // border, on the status bar's content row (y=38 at 152×40).
    let status_row = 40 - tmail::ui::layout::STATUSBAR_HEIGHT + 1;
    let head = (2..10)
        .find(|&x| buffer[(x, status_row)].symbol() == "■")
        .expect("loader head in the status bar's left corner");
    assert!(head >= 2, "loader starts two spaces in: {head}");
    // The row above (the topbar's former loader slot) carries no scanner.
    let topbar_rows: String = (0..4)
        .flat_map(|y| (0..152).map(move |x| (x, y)))
        .map(|(x, y)| buffer[(x, y)].symbol())
        .collect();
    assert!(
        !topbar_rows.contains("■"),
        "no loader left under the brand:\n{topbar_rows}"
    );
    // The list head and sidebar still render — work never blocks the frame.
    assert!(
        text.contains("INBOX"),
        "list head hidden behind spinner:\n{text}"
    );
    // The list rows area spans y 6..37, x 24..152: the 8-wide scanner is
    // centered with its head first.
    assert_eq!(buffer[(84, 21)].symbol(), "■", "list pane spinner:\n{text}");
}

#[test]
fn no_spinner_when_idle() {
    let mut state = mock_initial_state();
    let text = draw_after(&mut state, &[], 152, 40);
    assert!(!text.contains("⠋"), "spinner leaked while idle:\n{text}");
    assert!(
        text.contains(concat!("tmail v", env!("CARGO_PKG_VERSION"))),
        "brand restored when idle"
    );
}

/// The status bar's hint panel starts where the sidebar ends (x=25 at
/// 152×40), lining it up with the message list pane; the left corner
/// stays free for the loader. Each hint opens with its own two-space
/// padding, so the first glyph lands two columns past the edge.
#[test]
fn status_hints_align_with_the_sidebar_edge() {
    let mut state = mock_initial_state();
    state.session.size = (152, 40);
    let buffer = buffer_after(&mut state, &[], 152, 40);
    let status_row = 40 - tmail::ui::layout::STATUSBAR_HEIGHT + 1;
    // The first non-space glyph of the hint row: the hint's two-space pad
    // sits on the sidebar's right edge, the key follows.
    let first = (0..152)
        .find(|&x| buffer[(x, status_row)].symbol() != " ")
        .expect("hints drawn");
    assert_eq!(
        first,
        tmail::ui::layout::SIDEBAR_WIDTH + 2,
        "hints aligned at the sidebar's end (after the hint pad)"
    );
    // Nothing before it: the left corner is clear.
    assert!(
        (0..tmail::ui::layout::SIDEBAR_WIDTH).all(|x| buffer[(x, status_row)].symbol() == " "),
        "the loader slot stays clear while idle"
    );
}

#[test]
fn error_modal_renders_summary_code_buttons_and_sanitized_detail() {
    let mut state = mock_initial_state();
    let (id, req) = match reducer::reduce(&mut state, Action::PageNext).as_slice() {
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
    let (id, req) = match reducer::reduce(&mut state, Action::PageNext).as_slice() {
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
    let (id, req) = match reducer::reduce(&mut state, Action::PageNext).as_slice() {
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
    state.session.routes.push(Route::Message(MessageRoute {
        mailbox_id: MailboxId(String::from("inbox")),
        summary,
    }));
    state.open_message = Loadable::Loaded(message);
    state.session.focus = Focus::Reader;
    state
}

#[test]
fn reader_renders_exactly_one_message_document() {
    let mut state = reader_state(0);
    state.session.size = (152, 40);
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
    // No action row (ticket 3rt5): the actions stay bound to their
    // hotkeys, the buttons are gone. No thread navigation either
    // (plan §4 overrides); no mode badge.
    assert_absent(&text, "Archive e", (152, 40));
    assert_absent(&text, "Reply r", (152, 40));
    assert_absent(&text, "3 of 3", (152, 40));
    assert_absent(&text, "thread", (152, 40));
    assert_absent(&text, "READER", (152, 40));
    assert_absent(&text, "NORMAL", (152, 40));
    // Sidebar chrome stays visible (the Compose affordance is gone).
    assert!(text.contains("Inbox"), "sidebar hidden:\n{text}");
}

/// Attachment chips must advertise their actions (ticket 61qx): the
/// status bar lists `S save` / `o open` only when the open message
/// carries attachments.
#[test]
fn reader_status_bar_advertises_attachment_actions_only_with_attachments() {
    let mut with = reader_state(0);
    with.session.size = (152, 40);
    let message = match &mut with.open_message {
        Loadable::Loaded(message) => message,
        other => panic!("reader_state leaves the message loaded: {other:?}"),
    };
    message.attachments = vec![tmail::domain::Attachment {
        name: Some(String::from("report.pdf")),
        mime_type: Some(String::from("application/pdf")),
        size: Some(14),
        part_id: 3,
    }];
    let text = draw_after(&mut with, &[], 152, 40);
    assert!(text.contains("S save"), "save hint missing:\n{text}");
    assert!(text.contains("o open"), "open hint missing:\n{text}");

    // Without attachments the actions have no target: no hints, no lie.
    let mut without = reader_state(0);
    let text = draw_after(&mut without, &[], 152, 40);
    assert_absent(&text, "S save", (152, 40));
    assert_absent(&text, "o open", (152, 40));
}

/// The Tab-focused attachment chip carries the button fill (the composer's
/// focused controls use the same badge): accent text alone is not a
/// visible cursor. Unfocused chips, even the default target, stay on the
/// page background.
#[test]
fn focused_attachment_chip_shows_the_button_fill() {
    use tmail::app::state::ReaderFocus;
    let mut state = reader_state(0);
    if let Loadable::Loaded(message) = &mut state.open_message {
        // Short body so the chips are on screen without scrolling.
        message.plain_body = Some(String::from("short body\n"));
        message.html_body = None;
        message.attachments = vec![
            tmail::domain::Attachment {
                name: Some(String::from("first.pdf")),
                mime_type: Some(String::from("application/pdf")),
                size: Some(14),
                part_id: 2,
            },
            tmail::domain::Attachment {
                name: Some(String::from("second.png")),
                mime_type: Some(String::from("image/png")),
                size: Some(2048),
                part_id: 3,
            },
        ];
    }
    let theme = state.active_theme();
    let buffer = buffer_after(&mut state, &[], 152, 40);
    let text = text_of(&buffer);
    let (row, col) = position_of(&text, "first.pdf");
    assert_ne!(
        buffer[(col as u16, row as u16)].bg,
        theme.accent,
        "the default target is not focus:\n{text}"
    );

    state.reader_focus = Some(ReaderFocus::Attachment(1));
    let buffer = buffer_after(&mut state, &[], 152, 40);
    let text = text_of(&buffer);
    let (row, col) = position_of(&text, "second.png");
    assert_eq!(
        buffer[(col as u16, row as u16)].bg,
        theme.accent,
        "focused chip carries the button fill:\n{text}"
    );
    let (row, col) = position_of(&text, "first.pdf");
    assert_ne!(
        buffer[(col as u16, row as u16)].bg,
        theme.accent,
        "focus moved off the first chip:\n{text}"
    );
}

#[test]
fn reader_loading_state_renders_placeholder() {
    let mut state = reader_state(0);
    state.open_message = Loadable::Loading;
    let buffer = buffer_after(&mut state, &[], 152, 40);
    let text = text_of(&buffer);
    assert!(
        !text.contains("loading message…"),
        "text placeholder replaced by the pane spinner (ticket m3by):\n{text}"
    );
    assert!(
        !text.contains("body line 01"),
        "body must not exist while loading:\n{text}"
    );
    // The spinner sits centered in the body area (header ends at row 9;
    // the body spans 9..37 → center row 22 column, the 8-wide scanner
    // starts at x=24+((152-24)-8)/2=84).
    assert_eq!(buffer[(84, 22)].symbol(), "■", "centered spinner:\n{text}");
}

#[test]
fn reader_scrolls_body_with_reducer_state() {
    let mut state = reader_state(0);
    state.session.size = (152, 40);
    // The body scrolls beneath the fixed header (ticket 6864): the clamp
    // tracks the body against the viewport under the header.
    let width = state.session.size.0 as usize;
    let total = tmail::ui::screens::reader::scroll_line_count(&state, width);
    let viewport = tmail::ui::layout::reader_rows_visible(state.session.size)
        .saturating_sub(tmail::ui::screens::reader::header_line_count(&state, width));
    assert!(total > viewport, "document must overflow: {total} lines");
    // Scroll to the end the way the reducer does.
    for _ in 0..total {
        reducer::reduce(&mut state, Action::MoveDown);
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

/// A body link is accent+underline; the hit map addresses its drawn
/// columns, and a click focuses it before a second click opens it (ticket
/// hc9n) — the same two-step rhythm as rows and chips.
#[test]
fn reader_link_click_focuses_then_opens() {
    use tmail::app::state::ReaderFocus;
    let mut state = reader_state(0);
    state.session.size = (152, 40);
    if let Loadable::Loaded(message) = &mut state.open_message {
        message.plain_body = None;
        message.html_body = Some(String::from(
            "<p>see <a href=\"https://example.org/x\">the docs</a></p>",
        ));
    }
    let (buffer, hits) = draw_state_hits(&state, 152, 40);
    let text = text_of(&buffer);
    let (row, col) = position_of(&text, "the docs");
    assert_eq!(
        hits.hit_test((col + 2) as u16, row as u16, false),
        Some(ClickTarget::ReaderLink(0)),
        "the hit map must address the drawn link columns:\n{text}"
    );
    // First click focuses, exactly like Tab.
    reducer::reduce(&mut state, Action::Click(ClickTarget::ReaderLink(0)));
    assert_eq!(state.reader_focus, Some(ReaderFocus::Link(0)));
    // The focused link draws the cursor style.
    let (focused, _) = draw_state_hits(&state, 152, 40);
    let style = focused[(col as u16, row as u16)].style();
    assert!(
        style
            .add_modifier
            .contains(ratatui::style::Modifier::REVERSED),
        "focused link must draw the cursor style"
    );
    // Second click opens it through the platform opener.
    let effects = reducer::reduce(&mut state, Action::Click(ClickTarget::ReaderLink(0)));
    let [effect] = &effects[..] else {
        panic!("expected one effect, got {effects:?}");
    };
    assert!(
        matches!(
            &effect.kind,
            OperationKind::OpenUrl { url } if url == "https://example.org/x"
        ),
        "kind: {:?}",
        effect.kind
    );
}

#[test]
fn reader_idle_message_never_panics() {
    let mut state = mock_initial_state();
    // A reader route whose data was cleared (defensive state): the summary
    // snapshot still carries subject/sender, the body is the idle note.
    let summary = state.messages.items[1].clone();
    state.session.routes.push(Route::Message(MessageRoute {
        mailbox_id: MailboxId(String::from("inbox")),
        summary,
    }));
    state.open_message = Loadable::Idle;
    state.session.focus = Focus::Reader;
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
    state.session.routes.push(Route::Message(MessageRoute {
        mailbox_id: MailboxId(String::from("inbox")),
        summary,
    }));
    state.open_message = Loadable::Loaded(message);
    state.session.focus = Focus::Reader;
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
    // No header title (ticket a9y3): the row above To is blank unless a
    // draft status is shown.
    assert_absent(&text, "New message", (152, 40));
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

/// While composing, the sidebar marks the Drafts folder active (the
/// composer saves drafts there, plan §14) — not the mailbox underneath —
/// and the folder list stays focusable: Tab from the composer's last
/// control lands on it with the cursor bar on the cursor folder, and the
/// composer fields lose their caret while it holds focus.
#[test]
fn composer_sidebar_marks_drafts_and_stays_focusable() {
    use tmail::app::composer::ComposerField;
    let theme = Theme::default_dark();
    let mut state = mock_initial_state();
    let buffer = buffer_after(&mut state, &[Action::Compose], 152, 40);
    let text = text_of(&buffer);
    let (drafts_y, _) = position_of(&text, "Drafts");
    assert_eq!(
        buffer[(2, drafts_y as u16)].bg,
        theme.marker,
        "Drafts row is the active one while composing:\n{text}"
    );
    let (inbox_y, _) = position_of(&text, "Inbox");
    assert_eq!(
        buffer[(2, inbox_y as u16)].bg,
        theme.sidebar_bg,
        "the underlying mailbox is not marked:\n{text}"
    );
    // The To field holds the caret while the composer has focus.
    let (to_y, _) = position_of(&text, "      To");
    assert_eq!(
        buffer[(37, to_y as u16)].bg,
        theme.accent,
        "composer caret on the To value:\n{text}"
    );

    // Tab out of the composer (from its last control): the sidebar holds
    // focus — cursor bar on the cursor folder, Drafts still marked active,
    // and the composer caret gone.
    state.session.composer.as_mut().unwrap().field = ComposerField::Discard;
    let buffer = buffer_after(&mut state, &[Action::FocusNext], 152, 40);
    let text = text_of(&buffer);
    let (inbox_y, _) = position_of(&text, "Inbox");
    assert_eq!(
        buffer[(0, inbox_y as u16)].symbol(),
        "▎",
        "sidebar cursor bar after tabbing out:\n{text}"
    );
    let (drafts_y, _) = position_of(&text, "Drafts");
    assert_eq!(
        buffer[(2, drafts_y as u16)].bg,
        theme.marker,
        "Drafts stays marked while the sidebar holds focus:\n{text}"
    );
    let (to_y, _) = position_of(&text, "      To");
    assert_eq!(
        buffer[(37, to_y as u16)].bg,
        theme.background,
        "no composer caret while the sidebar holds focus:\n{text}"
    );
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
    // The default palette doubles its amber as both `accent` and
    // `warning` (black-gold), so the scan uses a sentinel warning color:
    // distinct from every other token, the test cannot false-positive on
    // accent-drawn cells.
    let mut theme = Theme::default_dark();
    theme.warning = ratatui::style::Color::Rgb(0xAB, 0x00, 0x01);
    let mut state = mock_initial_state();
    state.settings.themes = vec![(String::from("default"), theme)];
    state.settings.theme_index = 0;
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
    state.session.size = (152, 40);
    let mut actions: Vec<Action> = vec![Action::OpenSearch];
    for c in "quote".chars() {
        actions.push(Action::SearchEdit(tmail::app::action::SearchEdit::Char(c)));
    }
    actions.push(Action::SubmitSearch);
    let mut effects = Vec::new();
    for action in actions.iter().cloned() {
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
        Action::BackendCompleted(OperationResult {
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
    for action in actions.iter().cloned() {
        for effect in reducer::reduce(&mut state, action) {
            pending = Some(effect.id);
        }
    }
    let id = pending.expect("a save was started");
    reducer::reduce(
        &mut state,
        Action::BackendCompleted(OperationResult {
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
    for action in actions.iter().cloned() {
        for effect in reducer::reduce(&mut state, action) {
            pending = Some(effect.id);
        }
    }
    let id = pending.expect("a save was started");
    reducer::reduce(
        &mut state,
        Action::BackendCompleted(OperationResult {
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
    reducer::reduce(&mut state, Action::DismissError);
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
    for action in actions.iter().cloned() {
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
    // The full floor keeps the sidebar (the Compose affordance is gone;
    // the folder rows are its marker).
    assert!(
        text.contains("Inbox (24)"),
        "full floor keeps the sidebar:\n{text}"
    );
    assert_absent(&text, "UTF-8", (120, 30));
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
    assert_absent(&text, "UTF-8", (90, 25));
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

/// Comfortable view mode (`[tmail].view_mode = "comfortable"`): every
/// message row is followed by a faint horizontal separator, so fewer
/// messages fit and the list gains negative space.
#[test]
fn comfortable_view_mode_splits_rows_with_faint_separators() {
    let mut state = mock_initial_state();
    state.settings.view_mode = tmail::config::ViewMode::Comfortable;
    let (buffer, hits) = draw_state_hits(&state, 152, 40);
    let theme = Theme::default_dark();

    let line = |y: u16| {
        let mut out = String::new();
        for x in 0..buffer.area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out
    };

    // Geometry: list rows start at y=6 (topbar 4 + list head 2). Message 0
    // draws at y=6, its separator at y=7, message 1 at y=8 — two lines per
    // message instead of one.
    assert!(line(6).contains("KKF Notifications"), "row 0:\n{}", line(6));
    assert!(line(8).contains("Maksim Orlov"), "row 1:\n{}", line(8));
    let separators = line(7).chars().filter(|c| *c == '─').count();
    assert!(separators > 100, "separator line under row 0:\n{}", line(7));

    // Separators are more faint than the message body text: the hairline
    // (border) color, never the primary text color.
    assert_eq!(buffer[(30, 7)].fg, theme.border, "separator color");
    assert_ne!(buffer[(30, 7)].fg, theme.text);

    // Fewer messages fit: 31 list lines / 2 per message = 15 rows (0..=14).
    // Message 14 still renders (its separator lands on the last list line,
    // y=35) but message 15 — visible in compact — is pushed off screen.
    assert!(
        line(34).contains("Weekly digest #15"),
        "row 14:\n{}",
        line(34)
    );
    let tail_separators = line(35).chars().filter(|c| *c == '─').count();
    assert!(
        tail_separators > 100,
        "separator under row 14:\n{}",
        line(35)
    );
    // The last list line (y=36) stays blank apart from the scrollbar
    // track in the final column: no clipped row block is drawn.
    assert!(
        line(36).chars().all(|c| c == ' ' || c == '│'),
        "no clipped row block:\n{}",
        line(36)
    );
    let text = text_of(&buffer);
    assert_absent(&text, "Stone grit chart update #16", (152, 40));

    // The separator line belongs to the message above: clicking between
    // two rows targets the earlier message.
    assert_eq!(
        hits.hit_test(30, 7, false),
        Some(ClickTarget::MessageRow(0))
    );
}

#[test]
fn hit_map_matches_the_drawn_mailbox_screen() {
    let (_, hits) = draw_with_hits(152, 40);
    let _ = hits_is_sane(&hits);
    // Chrome geometry: topbar 0..4, body 4..37, statusbar 37..40; list head
    // 4..6, rows from y=6. Sidebar x=0..24, list x=25..152. The Compose
    // affordance is gone (the composer opens via `c`).
    assert_eq!(hits.hit_test(40, 1, false), Some(ClickTarget::SearchField));
    assert_eq!(hits.hit_test(10, 4, false), Some(ClickTarget::Mailbox(0)));
    assert_eq!(hits.hit_test(10, 5, false), Some(ClickTarget::Mailbox(1)));
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
    reducer::reduce(&mut state, failure);
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
    let effects = reducer::reduce(state, Action::PageNext);
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
    reducer::reduce(&mut state, Action::Compose);
    let (_, hits) = draw_state_hits(&state, 152, 40);
    // Chrome: body area y=4..37; the action row is its last line (y=36).
    // Send sits first, Discard after the three-space gap.
    assert_eq!(
        hits.hit_test(28, 36, false),
        Some(Target::ComposerField(ComposerField::Send))
    );
    assert_eq!(
        hits.hit_test(44, 36, false),
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
    state.session.size = (152, 40);
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
    state.session.size = (152, 40);
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
    assert_absent(&text, "UTF-8", (152, 40));
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

/// Mockup `list.html`: the pane head keeps the uppercase title + unread
/// sub + right-aligned range on one row (no checkbox — ticket fypg), and
/// every message is one row of marker | star | from | subject(+snippet) |
/// date.
#[test]
fn list_matches_mockup_density_and_hierarchy() {
    let (buffer, _) = draw_with_hits(152, 40);
    let text = text_of(&buffer);
    // Head row: title, unread count, and the range share one line, with
    // the range near the right edge (mockup `.pane-range` margin-left:auto).
    let (head_y, title_x) = position_of(&text, "   INBOX");
    let (_, unread_x) = position_of(&text, "24 unread");
    let (_, range_x) = position_of(&text, "1–20 of 25");
    assert_eq!(head_y, 4, "head sits under the topbar");
    assert!(title_x < unread_x, "unread sub follows the title");
    assert!(range_x > 120, "range is right-aligned, found at {range_x}");
    // A starred row keeps the single-line grid: star | from | subject |
    // snippet | date on one row (mockup `.mail` grid).
    let (row_y, star_x) = position_of(&text, "* PayPal");
    let (_, from_x) = position_of(&text, "PayPal");
    let (_, subject_x) = position_of(&text, "Payment received");
    let (_, date_x) = position_of(&text, "Sep 1");
    assert_eq!(row_y, 9, "rows start under the head");
    assert!(star_x < from_x && from_x < subject_x && subject_x < date_x);
    // Full mode shows snippets inline after the subject (mockup `.snippet`).
    assert!(
        text.contains("order #214, 50% deposit"),
        "snippet missing:\n{text}"
    );
}

/// Mockup `viewer.html`: subject, then From/To/Cc/Date meta, then a
/// hairline, then the body — top to bottom, one message, no thread chrome.
/// Every fixed field is padded two symbols in from the panel edges and the
/// action buttons are gone (ticket 3rt5).
#[test]
fn reader_matches_mockup_hierarchy() {
    let mut state = reader_state(0);
    let text = draw_after(&mut state, &[], 152, 40);
    let (subject_y, subject_x) = position_of(&text, "Re: WIP — 240 mm stainless-clad gyuto");
    let (from_y, _) = position_of(&text, "From ");
    let (to_y, _) = position_of(&text, "To   ");
    let (date_y, _) = position_of(&text, "Date ");
    let (body_y, _) = position_of(&text, "body line 01");
    assert!(subject_y < from_y, "subject first");
    assert!(from_y < to_y && to_y < date_y, "meta block in order");
    assert!(date_y < body_y, "body follows the meta");
    // The fixed fields sit two columns in from the list edge (ticket 3rt5).
    assert_eq!(subject_x, 27, "subject padded 2 from the panel edge");
    // A hairline separates the meta block from the body (mockup
    // `.msg-meta` border-bottom); no action row precedes it (ticket 3rt5).
    assert!(
        text.lines()
            .nth(date_y + 1)
            .is_some_and(|l| l.contains('─')),
        "hairline under the meta block"
    );
}

/// Mockup `new-mail.html`: right-aligned 8ch labels, Cc/Bcc toggles on the
/// To row, the attach row above Send/Discard. The header title is gone
/// (ticket a9y3): the To row is the topmost composer content.
#[test]
fn composer_matches_mockup_hierarchy_and_density() {
    let mut state = mock_initial_state();
    reducer::reduce(&mut state, Action::Compose);
    let text = draw_after(&mut state, &[], 152, 40);
    let (to_y, _) = position_of(&text, "[Cc]");
    let (subject_y, subject_x) = position_of(&text, " Subject");
    let (attach_y, _) = position_of(&text, "[ + attach ]");
    let (send_y, send_x) = position_of(&text, "[ Send");
    let (_, discard_x) = position_of(&text, " Discard ");
    // Vertical order: To row, Subject row, attach row, actions.
    assert!(to_y < subject_y);
    assert!(subject_y < attach_y && attach_y < send_y);
    // The label column is right-aligned within 8ch starting at x=27
    // (mockup `grid-template-columns: 8ch` + the 2ch body padding): on the
    // To row "To" sits at 27+6, and the Cc/Bcc toggles ride the same row's
    // right edge (mockup `.field-extra`).
    let to_row = text.lines().nth(to_y).expect("To row");
    let to_x = to_row
        .find("To")
        .map(|byte| to_row[..byte].chars().count())
        .expect("To label on its row");
    assert_eq!(to_x, 33, "To label right-aligned at 27+6");
    assert_eq!(subject_x, 27, "Subject fills the 8ch label column");
    let cc_x = to_row
        .find("[Cc]")
        .map(|byte| to_row[..byte].chars().count())
        .expect("Cc toggle");
    assert!(cc_x >= 130, "toggles are right-aligned, at {cc_x}");
    // Action row: Send first, Discard after it (mockup `.compose-actions`).
    assert!(send_x < discard_x, "Send precedes Discard");
}

/// The body sits in a surface well inset to align its text with the
/// subject value column, with one text line of padding above and below
/// the text and no rule between the subject and the body (tjdj, tz12),
/// while a rule separates the well from the attach, send, and discard
/// rows (tjdj).
#[test]
fn composer_body_well_is_inset_and_rule_separates_the_action_rows() {
    use tmail::app::action::ComposerEdit;
    let theme = Theme::default_dark();
    let mut state = mock_initial_state();
    let actions: Vec<Action> = [
        Action::Compose,
        // Walk to the body (CcToggle, BccToggle, Subject, Body) and type
        // "Hi".
        Action::FocusNext,
        Action::FocusNext,
        Action::FocusNext,
        Action::FocusNext,
        Action::ComposerEdit(ComposerEdit::Char('H')),
        Action::ComposerEdit(ComposerEdit::Char('i')),
    ]
    .into_iter()
    .collect();
    let buffer = buffer_after(&mut state, &actions, 152, 40);
    let text = text_of(&buffer);
    let (body_y, _) = position_of(&text, "Hi");
    // The typed text starts at the subject value column: 27 + 8ch label
    // + 2-space gap.
    assert_eq!(
        find_text_col(&buffer, body_y as u16, "Hi"),
        37,
        "body content aligns with the subject value:\n{text}"
    );
    // The surface fill spans the full row — padding columns included —
    // while the field rows above stay on the page background.
    assert_eq!(buffer[(27, body_y as u16)].bg, theme.surface);
    assert_eq!(buffer[(148, body_y as u16)].bg, theme.surface);
    // One text line of padding above the body text (ticket tz12): the
    // row directly above is well surface, the one above that is the
    // EMPTY separator line under the subject (ticket gdqm), and the
    // subject row sits above it — no hairline rule between them.
    assert_eq!(buffer[(27, body_y as u16 - 1)].bg, theme.surface);
    assert_eq!(buffer[(27, body_y as u16 - 2)].bg, theme.background);
    assert_eq!(buffer[(27, body_y as u16 - 2)].symbol(), " ", "empty line");
    assert!(
        text.lines()
            .nth(body_y - 3)
            .is_some_and(|l| l.contains(" Subject")),
        "the subject row is right above the separator"
    );

    // One rule between the well and the attach/send/discard rows: the
    // body well ends at y=33 (its bottom padding line), the rule sits at
    // y=34, the attach row at 35 and the actions at 36 (bottom = 37).
    let rule_y = 34u16;
    assert_eq!(
        buffer[(27, rule_y)].symbol(),
        "─",
        "the rule above the attach row:\n{text}"
    );
    assert_eq!(buffer[(27, rule_y + 1)].bg, theme.background);
}

/// The body editor's caret matches the single-line fields' caret while
/// the body is focused — the same accent block (ticket tz12) — and the
/// unfocused body shows no caret at all, like an HTML textarea (ticket
/// tz12: only the focused field carries a caret).
#[test]
fn composer_body_caret_matches_the_field_caret_and_hides_when_unfocused() {
    use tmail::app::action::ComposerEdit;
    let theme = Theme::default_dark();

    // Unfocused body (the To field holds the caret): no caret cell in
    // the body well. The library's default cursor renders as a REVERSED
    // cell, so any caret there would be visible in the scan.
    let mut state = mock_initial_state();
    let buffer = buffer_after(&mut state, &[Action::Compose], 152, 40);
    let reversed = buffer
        .content
        .iter()
        .any(|cell| cell.modifier.contains(ratatui::style::Modifier::REVERSED));
    assert!(!reversed, "the unfocused body must not draw a caret");

    // Focused body: the caret cell next to the typed text uses the same
    // accent-on-background style the To field's caret draws.
    let mut state = mock_initial_state();
    let actions: Vec<Action> = [
        Action::Compose,
        Action::FocusNext,
        Action::FocusNext,
        Action::FocusNext,
        Action::FocusNext, // body
        Action::ComposerEdit(ComposerEdit::Char('H')),
    ]
    .into_iter()
    .collect();
    let buffer = buffer_after(&mut state, &actions, 152, 40);
    let text = text_of(&buffer);
    let (body_y, _) = position_of(&text, "H");
    let caret_x = find_text_col(&buffer, body_y as u16, "H") + 1;
    let caret = &buffer[(caret_x as u16, body_y as u16)];
    assert_eq!(caret.bg, theme.accent, "body caret uses the accent fill");
    assert_eq!(
        caret.fg, theme.background,
        "body caret matches the field caret"
    );
}

/// Ctrl+Z reverts a body typing run at once (ticket kfmt): coalesced
/// undo, with the draft left dirty so autosave re-pushes the reverted
/// revision.
#[test]
fn composer_ctrl_z_reverts_a_body_typing_run() {
    use tmail::app::action::ComposerEdit;
    let actions: Vec<Action> = [
        Action::Compose,
        Action::FocusNext,
        Action::FocusNext,
        Action::FocusNext,
        Action::FocusNext, // body
        Action::ComposerEdit(ComposerEdit::Char('Z')),
        Action::ComposerEdit(ComposerEdit::Char('Z')),
        Action::ComposerEdit(ComposerEdit::Char('Q')),
        Action::ComposerEdit(ComposerEdit::Char('Q')),
        // Ctrl+Z: one undo step reverts the whole run.
        Action::ComposerEdit(ComposerEdit::Undo),
    ]
    .into_iter()
    .collect();
    let mut state = mock_initial_state();
    let text = draw_after(&mut state, &actions, 152, 40);
    assert!(
        !text.contains("ZZQQ"),
        "the typing run must be reverted:\n{text}"
    );
    // The revert is itself an edit: autosave re-arms.
    assert!(
        text.contains("Unsaved changes"),
        "the reverted draft stays dirty:\n{text}"
    );
}

/// Long body lines soft-wrap onto the next visual row (ticket kfmt):
/// no horizontal scrolling, the text folds within the well.
#[test]
fn composer_body_soft_wraps_long_lines() {
    use tmail::app::action::ComposerEdit;
    let mut state = mock_initial_state();
    let mut actions = vec![
        Action::Compose,
        Action::FocusNext,
        Action::FocusNext,
        Action::FocusNext,
        Action::FocusNext, // body
    ];
    // One oversized word: wider than the well, so it must split.
    for _ in 0..150 {
        actions.push(Action::ComposerEdit(ComposerEdit::Char('x')));
    }
    let buffer = buffer_after(&mut state, &actions, 152, 40);
    let text = text_of(&buffer);
    let (body_y, _) = position_of(&text, "xxxxx");
    // Count only the body's columns — the sidebar folder names contain
    // `x` characters of their own.
    let row = |y: u16| -> usize {
        (30..buffer.area.width)
            .filter(|&x| buffer[(x, y)].symbol() == "x")
            .count()
    };
    let first = row(body_y as u16);
    let second = row(body_y as u16 + 1);
    assert!(first > 0 && first < 150, "the run starts on the first row");
    assert!(
        second > 0 && first + second == 150,
        "the oversized word wraps onto the next row (first={first}, second={second}):\n{text}"
    );
}

/// Shift+Left extends a selection in the body, rendered with the
/// mockup's `.msg-body::selection` fill (ticket kfmt).
#[test]
fn composer_body_selection_renders_with_the_selection_fill() {
    use tmail::app::action::ComposerEdit;
    let theme = Theme::default_dark();
    let actions: Vec<Action> = [
        Action::Compose,
        Action::FocusNext,
        Action::FocusNext,
        Action::FocusNext,
        Action::FocusNext, // body
        Action::ComposerEdit(ComposerEdit::Char('h')),
        Action::ComposerEdit(ComposerEdit::Char('e')),
        Action::ComposerEdit(ComposerEdit::Char('l')),
        Action::ComposerEdit(ComposerEdit::Char('l')),
        Action::ComposerEdit(ComposerEdit::Char('o')),
        Action::ComposerEdit(ComposerEdit::SelectLeft),
        Action::ComposerEdit(ComposerEdit::SelectLeft),
        Action::ComposerEdit(ComposerEdit::SelectLeft),
    ]
    .into_iter()
    .collect();
    let mut state = mock_initial_state();
    let buffer = buffer_after(&mut state, &actions, 152, 40);
    let text = text_of(&buffer);
    let (body_y, _) = position_of(&text, "hello");
    // "llo" is selected: the cells over cols 3..5 of the word carry the
    // selection fill. The first selected cell (col 2) doubles as the
    // caret cell — the caret style draws there instead.
    for col in [40u16, 41] {
        assert_eq!(
            buffer[(col, body_y as u16)].bg,
            theme.accent_bg,
            "selected cell at {col} uses the selection fill:\n{text}"
        );
    }
    assert_eq!(
        buffer[(39, body_y as u16)].bg,
        theme.accent,
        "the selection head is the caret cell:\n{text}"
    );
    // Outside the selection the fill is absent.
    assert_eq!(
        buffer[(37, body_y as u16)].bg,
        theme.surface,
        "the unselected 'h' keeps the well fill:\n{text}"
    );
}

// ── Attachment file chooser (ticket 95x0) ────────────────────────────────

/// The chooser is a centered modal hosting the explorer listing: chrome
/// and directory line on top, the file rows in the middle, the status
/// and key-hint rows at the bottom.
#[test]
fn attachment_chooser_renders_the_explorer_and_chrome() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("report.pdf"), b"x").expect("write");
    std::fs::create_dir(dir.path().join("docs")).expect("mkdir");

    let mut state = mock_initial_state();
    reducer::reduce(&mut state, Action::Compose);
    // Walk to the `+ attach` control and open the chooser.
    while state.session.composer.as_ref().unwrap().field
        != tmail::app::composer::ComposerField::Attach
    {
        reducer::reduce(&mut state, Action::FocusNext);
    }
    let mut effects = reducer::reduce(&mut state, Action::Activate);
    let id = effects.pop().expect("a listing effect").id;
    // While listing: the pending state is explicit.
    let text = draw_after(&mut state, &[], 152, 40);
    assert!(text.contains(" Attach file "), "title missing:\n{text}");
    assert!(text.contains("Listing…"), "in-flight listing:\n{text}");

    // Land the listing; the explorer rows and the cwd line appear.
    let explorer =
        ratatui_explorer::FileExplorerBuilder::build_with_working_dir(dir.path()).unwrap();
    reducer::reduce(
        &mut state,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Ok(OperationOutcome::Explorer(Box::new(explorer))),
        }),
    );
    let text = draw_after(&mut state, &[], 152, 40);
    assert!(
        text.contains(&dir.path().display().to_string()),
        "the cwd line shows the directory:\n{text}"
    );
    assert!(text.contains("docs"), "directory entry:\n{text}");
    assert!(text.contains("report.pdf"), "file entry:\n{text}");
    assert!(
        text.contains("↑↓ select · ← up · → open · ↵ attach · Esc cancel"),
        "key hints:\n{text}"
    );
    assert!(
        text.contains("(↵ attaches the selected file)"),
        "status hint:\n{text}"
    );

    // A failed listing keeps the chrome with the detail inline.
    let mut state2 = mock_initial_state();
    let mut effects = {
        reducer::reduce(&mut state2, Action::Compose);
        while state2.session.composer.as_ref().unwrap().field
            != tmail::app::composer::ComposerField::Attach
        {
            reducer::reduce(&mut state2, Action::FocusNext);
        }
        reducer::reduce(&mut state2, Action::Activate)
    };
    let id = effects.pop().expect("a listing effect").id;
    reducer::reduce(
        &mut state2,
        Action::BackendCompleted(OperationResult {
            id,
            outcome: Err(OperationFailure {
                code: None,
                detail: String::from("permission denied"),
                retry: None,
                ambiguous: false,
            }),
        }),
    );
    let text = draw_after(&mut state2, &[], 152, 40);
    assert!(
        text.contains("permission denied"),
        "the failure stays visible:\n{text}"
    );
}

/// The paperclip rides one space left of the date (user amend of ticket
/// r84f): a fixed slot whatever the title and body lengths — a long
/// subject/body truncates three columns earlier, a short one pads — and
/// the date column never moves. Rows without attachments are unchanged.
#[test]
fn attachment_rows_end_in_the_paperclip_emoji() {
    let mut state = mock_initial_state();
    // Two sibling rows identical except for the attachment flag.
    let row = state.messages.items[1].clone();
    for index in [1, 2] {
        let item = &mut state.messages.items[index];
        item.subject = row.subject.clone();
        item.from = row.from.clone();
        item.snippet = Some(String::from("preview text"));
        item.timestamp = row.timestamp;
        item.is_read = row.is_read;
        item.is_starred = false;
    }
    state.messages.items[1].has_attachments = true;
    state.messages.items[2].has_attachments = false;
    let buffer = buffer_after(&mut state, &[], 152, 40);
    let text = text_of(&buffer);
    let rows: Vec<&str> = text.lines().filter(|l| l.contains(&row.subject)).collect();
    assert_eq!(rows.len(), 2, "both rows render:\n{text}");
    let (attached, plain) = (rows[0], rows[1]);
    assert!(
        attached.contains('\u{1F4CE}'),
        "the attachment row shows the clip:\n{text}"
    );
    assert!(
        !plain.contains('\u{1F4CE}'),
        "rows without attachments show none:\n{text}"
    );

    // The date text is the same on both rows and sits at the same column:
    // the slot never shifts the date.
    let date_text = tmail::ui::dates::format_relative(mock::now(), row.timestamp);
    let date_col = |line: &str| {
        line.rfind(&date_text)
            .map(|byte| line[..byte].chars().count())
            .expect("date on the row")
    };
    let (attached_date_x, plain_date_x) = (date_col(attached), date_col(plain));
    assert_eq!(
        attached_date_x, plain_date_x,
        "the date column is fixed:\n{attached}\n{plain}"
    );
    // Visually the emoji ends one column left of the date: it is one char
    // of two columns, followed in the flat text by the wide-symbol skip
    // cell and the separator space (two chars) before the date char.
    let clip_x = attached
        .rfind('\u{1F4CE}')
        .map(|byte| attached[..byte].chars().count())
        .expect("clip on the row");
    assert_eq!(
        clip_x + 3,
        attached_date_x,
        "the clip sits one space left of the date:\n{attached}"
    );

    // The slot is fixed for a long body too: the cell truncates earlier
    // instead of pushing the clip around.
    let mut long_state = mock_initial_state();
    long_state.messages.items[1].subject = row.subject.clone();
    long_state.messages.items[1].snippet = Some("x".repeat(400));
    long_state.messages.items[1].has_attachments = true;
    long_state.messages.items[1].timestamp = row.timestamp;
    let text = text_of(&buffer_after(&mut long_state, &[], 152, 40));
    let long_row = text
        .lines()
        .find(|l| l.contains(&row.subject))
        .expect("long row renders");
    let long_date_x = date_col(long_row);
    let long_clip_x = long_row
        .rfind('\u{1F4CE}')
        .map(|byte| long_row[..byte].chars().count())
        .expect("clip on the long row");
    assert_eq!(long_date_x, attached_date_x, "the date column is fixed");
    assert_eq!(
        long_clip_x + 3,
        long_date_x,
        "the clip holds the slot with a long body:\n{long_row}"
    );
}

// ── Bulk selection rendering (ticket p0s3) ───────────────────────────────
#[test]
fn select_all_marks_rows_and_the_header_stays_a_plain_label() {
    let mut state = mock_initial_state();
    state.session.focus = tmail::app::Focus::MessageList;
    let before = text_of(&buffer_after(&mut state, &[], 152, 40));
    // The header is a plain label now (ticket fypg): three columns of
    // padding, no checkbox, never focus-marked.
    assert!(
        before.contains("   INBOX"),
        "header label missing:\n{}",
        before.lines().take(6).collect::<Vec<_>>().join("\n")
    );
    assert_absent(&before, "[ ]", (152, 40));

    // One draw after SelectAll: rows flip and the header does NOT change —
    // no checkbox exists to mark.
    let mut state = mock_initial_state();
    state.session.focus = tmail::app::Focus::MessageList;
    let buffer = buffer_after(&mut state, &[Action::SelectAll], 152, 40);
    let text = text_of(&buffer);
    assert_absent(&text, "[X]", (152, 40));
    assert!(text.contains("   INBOX"), "header label unchanged:\n{text}");

    // Rows are located by their rendered subject text.
    let theme = Theme::default_dark();
    let cursor_row = text
        .lines()
        .position(|line| line.contains("KKF Notifications"))
        .expect("cursor row") as u16;
    let marked_row = text
        .lines()
        .position(|line| line.contains("Maksim Orlov"))
        .expect("another marked row") as u16;
    assert_eq!(
        buffer[(tmail::ui::layout::SIDEBAR_WIDTH + 2, cursor_row)].bg,
        theme.marker,
        "cursor row keeps the marker fill"
    );
    // Marked rows share the sidebar's selected-mailbox fill
    // (theme.selection) — visibly distinct from the cursor fill.
    assert_eq!(
        buffer[(tmail::ui::layout::SIDEBAR_WIDTH + 2, marked_row)].bg,
        theme.selection,
        "marked rows carry the bulk highlight"
    );
}

#[test]
fn selection_mode_status_bar_lists_the_bulk_buttons() {
    let mut state = mock_initial_state();
    state.session.focus = tmail::app::Focus::MessageList;
    let (buffer, hits) = draw_with_hits_at(&mut state, &[Action::SelectAll], 152, 40);
    let text = text_of(&buffer);
    assert!(text.contains("selected:"), "count label:\n{text}");
    assert!(text.contains("[delete]"), "delete button");
    assert!(text.contains("[archive]"), "archive button");
    assert!(text.contains("[read]"), "read button");
    assert!(text.contains("[unread]"), "unread button");
    assert!(text.contains("Esc clear"), "clear hint");

    // The [archive] button is clickable where it is drawn.
    let (mut bx, mut by) = (0u16, 0u16);
    'outer: for y in 0..buffer.area.height {
        for x in 0..buffer.area.width.saturating_sub(9) {
            let symbol: String = (0..9)
                .map(|dx| buffer[(x + dx, y)].symbol().chars().next().unwrap_or(' '))
                .collect();
            if symbol == "[archive]" {
                bx = x;
                by = y;
                break 'outer;
            }
        }
    }
    assert_ne!(bx + by, 0, "[archive] drawn somewhere");
    assert_eq!(
        hits.hit_test(bx + 1, by, false),
        Some(tmail::app::action::ClickTarget::BulkAction(
            tmail::app::action::BulkOp::Archive
        )),
        "the drawn button answers clicks"
    );
}

/// [`draw_with_hits`] after applying actions.
fn draw_with_hits_at(
    state: &mut tmail::app::AppState,
    actions: &[Action],
    width: u16,
    height: u16,
) -> (ratatui::buffer::Buffer, HitMap) {
    let theme = Theme::default_dark();
    let now = mock::now();
    let ctx = RenderContext::new(now, dates::format_clock(now));
    for action in actions.iter().cloned() {
        reducer::reduce(state, action);
    }
    state.session.size = (width, height);
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend");
    let mut hits = HitMap::default();
    terminal
        .draw(|frame| render(frame, state, &theme, &ctx, &mut hits))
        .expect("draw");
    (terminal.backend().buffer().clone(), hits)
}

#[test]
fn selected_rows_show_the_marked_dot_and_star_gets_a_trailing_space() {
    // Bulk-marked rows render the filled dot in the icon column (ticket
    // cvc4), whatever their star state; every icon is followed by a space.
    let mut state = mock_initial_state();
    state.session.focus = tmail::app::Focus::MessageList;
    let buffer = buffer_after(&mut state, &[Action::SelectAll], 152, 40);
    let text = text_of(&buffer);
    let cursor_row = text
        .lines()
        .position(|line| line.contains("KKF Notifications"))
        .expect("cursor row");
    let line = text.lines().nth(cursor_row).unwrap();
    let cells: Vec<char> = line.chars().collect();
    // Columns past the list edge are marker (1) + icon (2): ▎/space, then
    // ● or * followed by one space.
    let list_start = tmail::ui::layout::SIDEBAR_WIDTH as usize;
    let icon: String = cells[list_start + 1..list_start + 3].iter().collect();
    assert_eq!(icon, "● ", "marked-dot icon with trailing space");
    // A starred row renders "* " in the same column when not marked.
    let mut state = mock_initial_state();
    state.messages.items[0].is_starred = true;
    let buffer = buffer_after(&mut state, &[], 152, 40);
    let text = text_of(&buffer);
    let star_line = text
        .lines()
        .find(|l| l.contains("KKF Notifications"))
        .expect("starred row");
    let cells: Vec<char> = star_line.chars().collect();
    let icon: String = cells[tmail::ui::layout::SIDEBAR_WIDTH as usize + 1
        ..tmail::ui::layout::SIDEBAR_WIDTH as usize + 3]
        .iter()
        .collect();
    assert_eq!(icon, "* ", "star icon with trailing space");
}

#[test]
fn list_rows_render_the_faded_preview() {
    // Ticket wxtx: the list shows subject + body preview, the preview in
    // the faded `snippet` color while the subject keeps its own style.
    let (buffer, _) = draw_with_hits(152, 40);
    let theme = Theme::default_dark();
    let text = text_of(&buffer);
    assert!(
        text.contains("Re: WIP — 240 mm stainless-clad gyuto — quench done at 760 °C"),
        "preview composes onto the subject:\n{text}"
    );
    // An unselected data row carries faded preview spans after its subject
    // (row 0 is the cursor row: its fill is the accent, and the preview
    // takes the row's text color there instead of the snippet token).
    let faded = (0..buffer.area.width)
        .map(|x| buffer[(x, 8)].clone())
        .filter(|cell| cell.symbol() != " ")
        .any(|cell| cell.fg == theme.snippet);
    assert!(faded, "the preview renders in the faded snippet color");
    // The date still lands in its 8-column cell on the right edge (the
    // subject cell pads to its grid).
    let date_cell: String = (144..152).map(|x| buffer[(x, 6)].symbol()).collect();
    assert!(
        date_cell.starts_with("10:42"),
        "date on the right edge: {date_cell:?}"
    );
}

#[test]
fn reader_header_stays_fixed_and_the_scrollbar_tracks_the_body() {
    // Ticket 6864: the header never scrolls; a long body gets a vertical
    // scrollbar on the right edge of the body area.
    let mut state = reader_state(0);
    state.session.size = (152, 40);
    let before = text_of(&buffer_after(&mut state, &[], 152, 40));
    // Header pinned at the top of the reader area (row 4, under the
    // topbar), body beneath it.
    assert!(before.contains("Re: WIP — 240 mm stainless-clad gyuto"));
    assert!(before.contains("body line 01"), "top of body:\n{before}");
    // The mock body overflows the viewport: a scrollbar is drawn in the
    // last column.
    let thumb = (4..37).any(|y| buffer_after(&mut state, &[], 152, 40)[(151, y)].symbol() == "█");
    assert!(thumb, "scrollbar visible for a long message");

    // Scroll to the end: the header stays put, the bottom of the body
    // becomes visible, the top is gone.
    let downs: Vec<Action> = (0..60).map(|_| Action::MoveDown).collect();
    let after = text_of(&buffer_after(&mut state, &downs, 152, 40));
    let header_row = after.lines().nth(4).expect("header row");
    assert!(
        header_row.contains("Re: WIP — 240 mm stainless-clad gyuto"),
        "header stays fixed while the body scrolls:\n{after}"
    );
    assert!(
        after.contains("body line 40"),
        "bottom of body visible:\n{after}"
    );
    assert!(!after.contains("body line 01\n"), "top scrolled away");
}

#[test]
fn reader_without_overflow_draws_no_scrollbar() {
    // A short body fits the viewport: no scrollbar column.
    let mut state = reader_state(0);
    state.session.size = (152, 40);
    if let Loadable::Loaded(message) = &mut state.open_message {
        message.plain_body = Some(String::from("one short line\n"));
    }
    let buffer = buffer_after(&mut state, &[], 152, 40);
    let none = (4..37).any(|y| buffer[(151, y)].symbol() == "█");
    assert!(
        !none,
        "no scrollbar when the message fits:\n{}",
        text_of(&buffer)
    );
}

#[test]
fn status_message_sits_top_right_of_the_brand_row() {
    // Ticket en85 (redesign): the status message lives in the TOP bar,
    // right-aligned on the brand row in the shortcut-label color — one
    // column keeps it off the right border.
    let theme = Theme::default_dark();
    let mut state = mock_initial_state();
    state.session.size = (152, 40);
    state.set_status("Mailboxes loaded");
    let buffer = buffer_after(&mut state, &[], 152, 40);
    // The status message rides the topbar's brand row (y=1).
    let row: String = (0..152).map(|x| buffer[(x, 1)].symbol()).collect();
    let trimmed = row.trim_end();
    assert!(
        trimmed.ends_with("Mailboxes loaded"),
        "status message flush right: {trimmed:?}"
    );
    assert_absent(&text_of(&buffer), "UTF-8", (152, 40));
    // One column of padding keeps it off the right border.
    assert_eq!(buffer[(151, 1)].symbol(), " ", "padding column");
    let message = "Mailboxes loaded";
    let start = 152 - message.len() as u16 - 1;
    assert_eq!(buffer[(start, 1)].symbol(), "M", "first message glyph");
    // The faded color: the `label_dim` token — the same color the status
    // bar's shortcut labels carry — on the panel fill.
    assert_eq!(buffer[(start, 1)].fg, theme.label_dim);
    assert_eq!(buffer[(start, 1)].bg, theme.sidebar_bg);
    // The bottom status bar carries no message anymore.
    let bottom: String = (0..152).map(|x| buffer[(x, 38)].symbol()).collect();
    assert!(!bottom.contains("Mailboxes loaded"), "{bottom:?}");
}

#[test]
fn search_compose_and_sidebar_cursor_sit_on_plain_backgrounds() {
    // The redesign: the search well sits on the sidebar's panel fill
    // (`sidebar_bg`; no surface fill; focus shows via the accent border),
    // and the sidebar cursor row shows the `selection` fill.
    let theme = Theme::default_dark();
    let mut state = mock_initial_state();
    state.session.size = (152, 40);
    let idle = buffer_after(&mut state, &[], 152, 40);
    // Search field interior (x=24..84, y=0..3), past the placeholder text.
    assert_eq!(idle[(70, 1)].bg, theme.sidebar_bg, "search well (idle)");
    // The plain status-bar hints sit on the same panel fill.
    assert_eq!(
        idle[(30, 38)].bg,
        theme.sidebar_bg,
        "status hints on the panel"
    );

    // The focused search field keeps the panel fill; the accent
    // border marks focus.
    state.session.focus = tmail::app::focus::Focus::SearchField;
    let focused = buffer_after(&mut state, &[], 152, 40);
    assert_eq!(
        focused[(70, 1)].bg,
        theme.sidebar_bg,
        "search well (focused)"
    );
    assert_eq!(focused[(24, 0)].fg, theme.accent, "focused border");

    // Sidebar cursor row: the selection fill under the second folder;
    // other rows stay on the panel fill (folder 0 is the active one).
    state.session.focus = tmail::app::focus::Focus::Sidebar;
    state.mailbox_selection = 1;
    let sidebar = buffer_after(&mut state, &[], 152, 40);
    assert_eq!(sidebar[(5, 5)].bg, theme.selection, "sidebar cursor row");
    assert_eq!(sidebar[(5, 6)].bg, theme.sidebar_bg, "plain folder row");
    assert_eq!(sidebar[(5, 4)].bg, theme.marker, "active folder row");
}

/// The header's pagination label right-aligns with the date/time column
/// of the message rows — a few symbols in from the pane border, not stuck
/// to it. The rows' rightmost glyph is the date (trailing `fit_left` pad
/// is spaces), so comparing last-glyph columns checks the alignment.
#[test]
fn range_label_aligns_with_the_date_column() {
    let mut state = mock_initial_state();
    state.session.size = (152, 40);

    // Compact, 20 items in a 31-row list: no scrollbar. The date text
    // ("10:42", 5 chars in an 8-wide cell) ends 3 columns in; so must the
    // label.
    let buffer = buffer_after(&mut state, &[], 152, 40);
    let header_edge = last_text_col(&buffer, 4);
    assert_eq!(
        header_edge,
        last_text_col(&buffer, 6),
        "label meets the dates"
    );
    assert_eq!(header_edge, find_text_col(&buffer, 6, "10:42") + 4);
    assert_eq!(header_edge, 148, "three columns in from the border");

    // Comfortable density: the list scrolls (20 items > 15 visible), the
    // scrollbar shaves one column off the rows, and the label follows.
    state.settings.view_mode = tmail::config::ViewMode::Comfortable;
    let buffer = buffer_after(&mut state, &[], 152, 40);
    let header_edge = last_text_col(&buffer, 4);
    assert_eq!(
        header_edge,
        find_text_col(&buffer, 6, "10:42") + 4,
        "label meets the dates under the scrollbar"
    );
    assert_eq!(header_edge, 147);
}

/// Display column of the last non-space glyph on row `y`.
fn last_text_col(buffer: &ratatui::buffer::Buffer, y: u16) -> usize {
    let mut last = 0;
    for x in 0..buffer.area.width {
        if buffer[(x, y)].symbol() != " " {
            last = x as usize;
        }
    }
    last
}

/// Display column where `needle` starts on row `y` (mock rows carry no
/// wide glyphs, so char index == display column).
fn find_text_col(buffer: &ratatui::buffer::Buffer, y: u16, needle: &str) -> usize {
    let mut line = String::new();
    for x in 0..buffer.area.width {
        line.push_str(buffer[(x, y)].symbol());
    }
    let byte_idx = line
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} missing from row {y}: {line}"));
    // Byte offset → char count → display column (mock rows carry no wide
    // glyphs, so one char == one column).
    line[..byte_idx].chars().count()
}

#[test]
fn status_message_clears_after_the_configured_timeout() {
    // Ticket h1d7: with `[tmail].status_timeout > 0` the message clears
    // when the window elapses (the reducer owns the timer; the renderer
    // shows the message in the shortcut-label color until then).
    let theme = Theme::default_dark();
    let mut state = mock_initial_state();
    state.session.size = (152, 40);
    state.settings.status_timeout_seconds = 5;
    reducer::reduce(
        &mut state,
        Action::Tick {
            now: Box::new(mock::now()),
        },
    );
    state.set_status("Mailboxes loaded");
    // A second into the window the message is still up, in the
    // shortcut-label color on the panel fill.
    let buffer = buffer_after(
        &mut state,
        &[Action::Tick {
            now: Box::new(mock::now() + chrono::Duration::seconds(1)),
        }],
        152,
        40,
    );
    let fg = buffer[(140, 1)].fg;
    assert_eq!(fg, theme.label_dim, "message rides the label color");
    assert_eq!(buffer[(140, 1)].bg, theme.sidebar_bg);

    // Five-and-a-quarter seconds in: the window elapsed, the message and
    // its column are gone from the brand row.
    let buffer = buffer_after(
        &mut state,
        &[Action::Tick {
            now: Box::new(mock::now() + chrono::Duration::milliseconds(5250)),
        }],
        152,
        40,
    );
    let fg = buffer[(140, 1)].fg;
    assert_ne!(fg, theme.label_dim, "the message cleared past the window");
}

#[test]
fn sidebar_shows_unread_counters_in_brackets() {
    // `Inbox (4)`-style counters right after the name; the drafts
    // counter is the folder content (Gmail reports drafts as unread 0),
    // and folders without unread mail show none.
    let (buffer, _) = draw_with_hits(152, 40);
    let text = text_of(&buffer);
    assert!(text.contains("Inbox (24)"), "{text}");
    assert!(text.contains("Spam (5)"), "{text}");
    assert!(
        text.contains("Drafts (3)"),
        "drafts carry their count: {text}"
    );
    assert_absent(&text, "Sent (", (152, 40));
    assert_absent(&text, "Starred (", (152, 40));
    assert_absent(&text, "Trash (", (152, 40));
}

/// Folder text carries a one-cell margin on both sides: after the marker
/// column and before the pane's right edge, even when a long name and its
/// unread counter would otherwise fill the row.
#[test]
fn folder_rows_keep_one_space_on_both_sides_of_the_name() {
    use tmail::app::state::Loadable;
    use tmail::domain::{Mailbox, MailboxId};

    let mut state = mock_initial_state();
    state.session.size = (152, 40);
    state.mailboxes = Loadable::Loaded(vec![Mailbox {
        id: MailboxId(String::from("long")),
        name: String::from("[Gmail]/Important"),
        role: None,
        unread_count: Some(1234),
        total_count: None,
    }]);
    // Folder rows start at y=4 with a single folder; collect through the
    // sidebar's margin column (x 0..24).
    let row = |buffer: &ratatui::buffer::Buffer| {
        (0..24u16)
            .map(|x| buffer[(x, 4)].symbol())
            .collect::<String>()
    };
    let (buffer, _) = draw_state_hits(&state, 152, 40);
    let text = row(&buffer);
    // The `[Gmail]/` display prefix is cosmetic-only: the sidebar shows
    // the bare folder name. The margins still hold.
    assert!(
        text.starts_with("  Important (1234)"),
        "one pad space after the marker column: {text:?}"
    );
    assert!(
        text.ends_with(" "),
        "one pad space before the margin: {text:?}"
    );

    // The cursor row swaps the blank marker for the bar, not the padding.
    state.session.focus = tmail::app::Focus::Sidebar;
    state.mailbox_selection = 0;
    let (buffer, _) = draw_state_hits(&state, 152, 40);
    let text = row(&buffer);
    assert!(
        text.starts_with("▎ Important (1234)"),
        "bar + one pad: {text:?}"
    );
    assert!(text.ends_with(" "), "right margin survives focus: {text:?}");
}

#[test]
fn sidebar_loading_shows_a_centered_spinner() {
    // Ticket m3by: the mailboxes pane spinner is the centered scanner
    // glyph (the head block of frame 0 leads from the left).
    let mut state = mock_initial_state();
    state.mailboxes = Loadable::Loading;
    let buffer = buffer_after(&mut state, &[], 152, 40);
    // The sidebar's pane spans x 0..24, y 4..37; the width-8 scanner
    // centers at x=8, its row at y=20.
    assert_eq!(buffer[(8, 20)].symbol(), "■", "sidebar spinner head");
    assert!(
        !text_of(&buffer).contains("loading mailboxes"),
        "text placeholder replaced by the spinner"
    );
}

#[test]
fn sidebar_splits_folders_and_labels_with_a_blank_row() {
    // Gmail-shaped listing (the backend hands it folders-first): system
    // folders, then one blank separator row, then the labels. Row
    // positions shift at the blank row, but click targets keep indexing
    // `state.mailboxes` (ticket wntx).
    use tmail::app::state::Loadable;
    use tmail::domain::{Mailbox, MailboxId, MailboxRole};

    let mailbox = |id: &str, name: &str, role: Option<MailboxRole>, unread: Option<u64>| Mailbox {
        id: MailboxId(String::from(id)),
        name: String::from(name),
        role,
        unread_count: unread,
        total_count: None,
    };
    let mailboxes = vec![
        mailbox("Inbox", "Inbox", Some(MailboxRole::Inbox), Some(6)),
        mailbox(
            "[Gmail]/Sent Mail",
            "[Gmail]/Sent Mail",
            Some(MailboxRole::Sent),
            None,
        ),
        mailbox(
            "[Gmail]/Drafts",
            "[Gmail]/Drafts",
            Some(MailboxRole::Drafts),
            None,
        ),
        // No role without a `junk` alias, yet a folder: Gmail reserves
        // the `[Gmail]/` prefix for its own folders.
        mailbox("[Gmail]/Spam", "[Gmail]/Spam", None, Some(80)),
        mailbox("[Gmail]/Starred", "[Gmail]/Starred", None, None),
        mailbox(
            "[Gmail]/Trash",
            "[Gmail]/Trash",
            Some(MailboxRole::Trash),
            None,
        ),
        mailbox("Notes", "Notes", None, None),
        mailbox("social", "social", None, Some(1)),
        mailbox("пароли", "пароли", None, None),
    ];
    let mut state = mock_initial_state();
    state.session.size = (152, 40);
    state.mailboxes = Loadable::Loaded(mailboxes);
    let (buffer, hits) = draw_state_hits(&state, 152, 40);
    // Folder rows start at y=4; six folders end at y=9, the blank row is
    // y=10, labels follow. Read only the sidebar's own columns.
    let row = |y: u16| {
        (0..23u16)
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>()
    };
    assert!(row(9).contains("Trash"), "last folder above the blank row");
    assert!(row(10).trim().is_empty(), "blank separator row");
    assert!(row(11).contains("Notes"), "first label below the blank row");
    assert!(row(12).contains("social"));
    assert!(row(13).contains("пароли"), "unicode label");
    assert_eq!(
        hits.hit_test(10, 9, false),
        Some(ClickTarget::Mailbox(5)),
        "folder click target above the blank row"
    );
    assert_eq!(
        hits.hit_test(10, 10, false),
        None,
        "the blank row is not clickable"
    );
    assert_eq!(
        hits.hit_test(10, 11, false),
        Some(ClickTarget::Mailbox(6)),
        "label click target keeps its state index"
    );
}

#[test]
fn message_list_shows_a_scrollbar_only_when_rows_overflow() {
    // Ticket kjfq: a page longer than the list shows a vertical scrollbar
    // in the last column; a page that fits shows none.
    // 20 mock rows on a 16-row list (compact floor): scrollbar present.
    let (buffer, _) = draw_with_hits(90, 25);
    let col: Vec<&str> = (6..22).map(|y| buffer[(89, y)].symbol()).collect();
    assert!(
        col.iter().any(|s| *s == "│" || *s == "█"),
        "scrollbar expected: {col:?}"
    );
    // 20 rows on a 31-row list: no scrollbar.
    let (buffer, _) = draw_with_hits(152, 40);
    let col: Vec<&str> = (6..37).map(|y| buffer[(151, y)].symbol()).collect();
    assert!(
        col.iter().all(|s| *s != "│" && *s != "█"),
        "no scrollbar expected: {col:?}"
    );
}

// ── Account configuration wizard (ADR 0003 W5) ───────────────────────────

use tmail::app::wizard::{ConfigSnapshot, StorageMode, WizardAction, WizardState};
use tmail::discovery::{ConfigSource, DiscoveredService, Provider, Security, ServerEndpoint};

/// A fresh wizard state over the mock shell, with a resolved save path.
fn wizard_state() -> tmail::app::AppState {
    let mut state = mock_initial_state();
    state.session.wizard = Some(WizardState::new(
        false,
        ConfigSnapshot {
            save_path: Some(std::path::PathBuf::from("/tmp/himalaya/config.toml")),
            existing_names: Vec::new(),
            existing_default_name: None,
            existing_shared_readable: false,
        },
    ));
    state.session.focus = tmail::app::Focus::Wizard;
    state
}

fn gmail_service() -> DiscoveredService {
    DiscoveredService {
        source: ConfigSource::Provider(Provider::Gmail),
        imap: ServerEndpoint {
            url: String::from("imaps://imap.gmail.com:993"),
            security: Security::Tls,
        },
        smtp: Some(ServerEndpoint {
            url: String::from("smtps://smtp.gmail.com:465"),
            security: Security::Tls,
        }),
        provider: Some(Provider::Gmail),
        username: None,
    }
}

fn discovered_result(id: tmail::app::OperationId, services: Vec<DiscoveredService>) -> Action {
    Action::BackendCompleted(OperationResult {
        id,
        outcome: Ok(tmail::app::OperationOutcome::Discovered(services)),
    })
}

#[test]
fn wizard_email_screen_renders_title_field_and_hints() {
    let mut state = wizard_state();
    let text = text_of(&buffer_after(&mut state, &[], 80, 24));

    assert!(text.contains("Account setup — email address"), "{text}");
    assert!(text.contains("Email"), "field label: {text}");
    assert!(text.contains("↵ detect settings"), "hint row: {text}");
    assert!(text.contains("Ctrl+C quit"), "quit hint: {text}");
    assert!(text.contains("Esc cancel"), "cancel hint: {text}");
    // The mailbox shell stays hidden behind the wizard.
    assert!(
        !text.contains("Loading mailboxes"),
        "no shell chrome: {text}"
    );
}

#[test]
fn wizard_email_screen_shows_validation_error_in_place() {
    let mut state = wizard_state();
    let text = text_of(&buffer_after(
        &mut state,
        &[Action::Wizard(WizardAction::SubmitEmail)],
        80,
        24,
    ));
    assert!(
        text.contains("enter an email address"),
        "inline error: {text}"
    );
}

#[test]
fn wizard_discovery_screen_lists_ranked_services_with_source_labels() {
    let mut state = wizard_state();
    if let Some(wizard) = state.session.wizard.as_mut() {
        wizard.email.address.value = String::from("u@gmail.com");
    }
    let submit = reducer::reduce(&mut state, Action::Wizard(WizardAction::SubmitEmail));
    let id = submit[0].id;
    let text = text_of(&buffer_after(
        &mut state,
        &[discovered_result(id, vec![gmail_service()])],
        100,
        24,
    ));

    assert!(
        text.contains("known provider: Gmail"),
        "source label: {text}"
    );
    assert!(
        text.contains("imaps://imap.gmail.com:993"),
        "imap url: {text}"
    );
    assert!(
        text.contains("smtps://smtp.gmail.com:465"),
        "smtp url: {text}"
    );
    assert!(text.contains("↑↓ choose"), "hint row: {text}");
}

#[test]
fn wizard_discovery_screen_shows_the_detecting_spinner() {
    let mut state = wizard_state();
    if let Some(wizard) = state.session.wizard.as_mut() {
        wizard.email.address.value = String::from("u@gmail.com");
    }
    let submit = reducer::reduce(&mut state, Action::Wizard(WizardAction::SubmitEmail));
    assert!(!submit.is_empty(), "the discovery effect starts");
    // Still discovering: the spinner message is on screen and the
    // status bar shows the step-back hint.
    let text = text_of(&buffer_after(&mut state, &[], 80, 24));
    assert!(
        text.contains("Detecting settings for u@gmail.com"),
        "{text}"
    );
    assert!(text.contains("Esc steps back"), "hint: {text}");
}

#[test]
fn wizard_empty_discovery_opens_the_manual_override_form() {
    let mut state = wizard_state();
    if let Some(wizard) = state.session.wizard.as_mut() {
        wizard.email.address.value = String::from("u@custom.example");
    }
    let submit = reducer::reduce(&mut state, Action::Wizard(WizardAction::SubmitEmail));
    let id = submit[0].id;
    let text = text_of(&buffer_after(
        &mut state,
        &[discovered_result(id, Vec::new())],
        80,
        24,
    ));

    assert!(text.contains("IMAP server"), "override field: {text}");
    assert!(
        text.contains("imaps://imap.custom.example:993"),
        "guessed imap: {text}"
    );
    assert!(
        text.contains("smtps://smtp.custom.example:465"),
        "guessed smtp: {text}"
    );
}

#[test]
fn wizard_identity_screen_recaps_the_chosen_servers() {
    let mut state = wizard_state();
    if let Some(wizard) = state.session.wizard.as_mut() {
        wizard.email.address.value = String::from("u@gmail.com");
    }
    let submit = reducer::reduce(&mut state, Action::Wizard(WizardAction::SubmitEmail));
    let id = submit[0].id;
    let actions = [
        discovered_result(id, vec![gmail_service()]),
        Action::Wizard(WizardAction::SelectService),
    ];
    let text = text_of(&buffer_after(&mut state, &actions, 80, 24));

    assert!(text.contains("Name"), "display name field: {text}");
    assert!(
        text.contains("IMAP  imaps://imap.gmail.com:993"),
        "recap: {text}"
    );
    assert!(
        text.contains("SMTP  smtps://smtp.gmail.com:465"),
        "recap: {text}"
    );
}

#[test]
fn wizard_credentials_screen_masks_the_password_and_shows_the_gmail_hint() {
    let mut state = wizard_state();
    if let Some(wizard) = state.session.wizard.as_mut() {
        wizard.email.address.value = String::from("u@gmail.com");
        wizard.credentials.password.value = String::from("app-password");
        wizard.credentials.password.cursor = 11;
        wizard.credentials.credentials_index = 2;
    }
    let submit = reducer::reduce(&mut state, Action::Wizard(WizardAction::SubmitEmail));
    let id = submit[0].id;
    let actions = [
        discovered_result(id, vec![gmail_service()]),
        Action::Wizard(WizardAction::SelectService),
        Action::Wizard(WizardAction::SubmitCredentials),
    ];
    let text = text_of(&buffer_after(&mut state, &actions, 80, 24));

    assert!(text.contains("Username"), "username field: {text}");
    assert!(
        text.contains("store password in config"),
        "storage toggle: {text}"
    );
    assert!(text.contains("fetch via command"), "storage toggle: {text}");
    assert!(text.contains("app password"), "gmail hint: {text}");
    // The password renders as bullets, never the raw secret.
    assert!(text.contains("••••"), "masked password: {text}");
    assert!(
        !text.contains("Password  app-password"),
        "raw password rendered: {text}"
    );
}

#[test]
fn wizard_credentials_command_mode_shows_the_command_field() {
    let mut state = wizard_state();
    if let Some(wizard) = state.session.wizard.as_mut() {
        wizard.email.address.value = String::from("u@gmail.com");
        wizard.credentials.storage_mode = StorageMode::Command;
        wizard.credentials.command.value = String::from("pass show mail/gmail");
    }
    let submit = reducer::reduce(&mut state, Action::Wizard(WizardAction::SubmitEmail));
    let id = submit[0].id;
    let actions = [
        discovered_result(id, vec![gmail_service()]),
        Action::Wizard(WizardAction::SelectService),
        Action::Wizard(WizardAction::SubmitCredentials),
    ];
    let text = text_of(&buffer_after(&mut state, &actions, 80, 24));

    assert!(text.contains("Command"), "command field: {text}");
    assert!(
        text.contains("pass show mail/gmail"),
        "the command renders: {text}"
    );
    assert!(
        !text.contains("•"),
        "no masked password field in command mode: {text}"
    );
}

/// Drives the wizard to the Testing step through the real reducer and
/// completes the credential test, returning the state ready for the
/// `TestAccountCompleted` result to be synthesized with the test op's
/// own id (the manager allocates a fresh id per attempt).
fn drive_to_testing(
    state: &mut tmail::app::AppState,
    mailboxes: Vec<String>,
) -> tmail::app::OperationId {
    if let Some(wizard) = state.session.wizard.as_mut() {
        wizard.email.address.value = String::from("u@gmail.com");
        wizard.credentials.password.value = String::from("p");
        wizard.credentials.credentials_index = 2;
    }
    let submit = reducer::reduce(state, Action::Wizard(WizardAction::SubmitEmail));
    let discovery_id = submit[0].id;
    reducer::reduce(
        state,
        discovered_result(discovery_id, vec![gmail_service()]),
    );
    reducer::reduce(state, Action::Wizard(WizardAction::SelectService));
    reducer::reduce(state, Action::Wizard(WizardAction::SubmitCredentials));
    let test_effects = reducer::reduce(state, Action::Wizard(WizardAction::SubmitCredentials));
    let test_id = test_effects[0].id;
    reducer::reduce(
        state,
        Action::BackendCompleted(OperationResult {
            id: test_id,
            outcome: Ok(tmail::app::OperationOutcome::TestAccountCompleted { mailboxes }),
        }),
    );
    test_id
}

#[test]
fn wizard_confirm_screen_shows_aliases_path_and_storage() {
    let mut state = wizard_state();
    drive_to_testing(
        &mut state,
        vec![
            String::from("INBOX"),
            String::from("[Gmail]/Sent Mail"),
            String::from("[Gmail]/All Mail"),
        ],
    );
    let text = text_of(&buffer_after(&mut state, &[], 100, 30));

    assert!(text.contains("[accounts.gmail]"), "account name: {text}");
    assert!(
        text.contains("stored in config (****)"),
        "masked secret: {text}"
    );
    assert!(text.contains("inbox  INBOX"), "alias: {text}");
    assert!(text.contains("sent  [Gmail]/Sent Mail"), "alias: {text}");
    assert!(text.contains("archive  [Gmail]/All Mail"), "alias: {text}");
    assert!(
        text.contains("/tmp/himalaya/config.toml"),
        "save path: {text}"
    );
    assert!(text.contains("↵ save account"), "hint: {text}");
    // The raw typed password never reaches the screen.
    assert!(
        !text.contains("Password  p"),
        "raw password rendered: {text}"
    );
}

#[test]
fn wizard_saved_screen_reports_the_path_and_permissions() {
    let mut state = wizard_state();
    if let Some(wizard) = state.session.wizard.as_mut() {
        wizard.email.address.value = String::from("u@gmail.com");
        wizard.credentials.password.value = String::from("p");
        wizard.credentials.credentials_index = 2;
    }
    let submit = reducer::reduce(&mut state, Action::Wizard(WizardAction::SubmitEmail));
    let discovery_id = submit[0].id;
    reducer::reduce(
        &mut state,
        discovered_result(discovery_id, vec![gmail_service()]),
    );
    reducer::reduce(&mut state, Action::Wizard(WizardAction::SelectService));
    reducer::reduce(&mut state, Action::Wizard(WizardAction::SubmitCredentials));
    let test_effects = reducer::reduce(&mut state, Action::Wizard(WizardAction::SubmitCredentials));
    let test_id = test_effects[0].id;
    reducer::reduce(
        &mut state,
        Action::BackendCompleted(OperationResult {
            id: test_id,
            outcome: Ok(tmail::app::OperationOutcome::TestAccountCompleted {
                mailboxes: vec![String::from("INBOX")],
            }),
        }),
    );
    let save_effects = reducer::reduce(&mut state, Action::Wizard(WizardAction::ConfirmSave));
    let save_id = save_effects[0].id;
    let actions = [Action::BackendCompleted(OperationResult {
        id: save_id,
        outcome: Ok(tmail::app::OperationOutcome::AccountSaved {
            path: std::path::PathBuf::from("/tmp/himalaya/config.toml"),
            created: true,
            permissions_warning: None,
        }),
    })];
    let text = text_of(&buffer_after(&mut state, &actions, 80, 24));

    assert!(
        text.contains("Account saved to /tmp/himalaya/config.toml"),
        "{text}"
    );
    assert!(text.contains("0600"), "created note: {text}");
    assert!(
        text.contains("open your mailbox"),
        "first-run next step: {text}"
    );
}

#[test]
fn wizard_confirm_screen_warns_when_the_config_is_shared_readable() {
    let mut state = wizard_state();
    if let Some(wizard) = state.session.wizard.as_mut() {
        wizard.config.existing_shared_readable = true;
    }
    drive_to_testing(&mut state, vec![String::from("INBOX")]);
    let text = text_of(&buffer_after(&mut state, &[], 100, 30));

    assert!(
        text.contains("readable by others"),
        "permissions warning on the confirm screen: {text}"
    );
}

/// Counts cells carrying the accent background (the inline caret of a
/// focused field, the app-wide cursor convention, is exactly such a
/// reversed cell).
fn count_accent_bg(buffer: &ratatui::buffer::Buffer, accent: ratatui::style::Color) -> usize {
    (0..buffer.area.width)
        .flat_map(|x| (0..buffer.area.height).map(move |y| (x, y)))
        .filter(|&(x, y)| buffer[(x, y)].style().bg == Some(accent))
        .count()
}

#[test]
fn wizard_focused_fields_show_the_inline_caret() {
    let theme = Theme::default_dark();

    // W1 email: the screen's only control is focused, so exactly one
    // accent cell — the reversed-space caret — is on screen.
    let mut state = wizard_state();
    let buffer = buffer_after(&mut state, &[], 80, 24);
    assert_eq!(
        count_accent_bg(&buffer, theme.accent),
        1,
        "the focused email field must draw the caret"
    );

    // With typed text the caret rides after the last character.
    let mut state = wizard_state();
    if let Some(wizard) = state.session.wizard.as_mut() {
        wizard.email.address.value = String::from("u@example.com");
        wizard.email.address.cursor = 13;
    }
    let buffer = buffer_after(&mut state, &[], 80, 24);
    assert_eq!(
        count_accent_bg(&buffer, theme.accent),
        1,
        "the caret follows the typed text"
    );

    // W3 identity: the Name box is focused.
    let mut state = wizard_state();
    if let Some(wizard) = state.session.wizard.as_mut() {
        wizard.email.address.value = String::from("u@example.com");
    }
    let submit = reducer::reduce(&mut state, Action::Wizard(WizardAction::SubmitEmail));
    let id = submit[0].id;
    let actions = [
        discovered_result(id, vec![gmail_service()]),
        Action::Wizard(WizardAction::SelectService),
    ];
    let buffer = buffer_after(&mut state, &actions, 80, 24);
    assert_eq!(
        count_accent_bg(&buffer, theme.accent),
        1,
        "the focused Name field draws the caret"
    );

    // W4 credentials: the username row is focused first.
    let mut state = wizard_state();
    if let Some(wizard) = state.session.wizard.as_mut() {
        wizard.email.address.value = String::from("u@example.com");
    }
    let submit = reducer::reduce(&mut state, Action::Wizard(WizardAction::SubmitEmail));
    let id = submit[0].id;
    let actions = [
        discovered_result(id, vec![gmail_service()]),
        Action::Wizard(WizardAction::SelectService),
        Action::Wizard(WizardAction::SubmitCredentials),
    ];
    let buffer = buffer_after(&mut state, &actions, 80, 24);
    assert_eq!(
        count_accent_bg(&buffer, theme.accent),
        1,
        "the focused Username field draws the caret"
    );

    // A mid-string cursor replaces the character cell (still exactly
    // one caret cell).
    let mut state = wizard_state();
    if let Some(wizard) = state.session.wizard.as_mut() {
        wizard.email.address.value = String::from("u@example.com");
        wizard.email.address.cursor = 1;
    }
    let buffer = buffer_after(&mut state, &[], 80, 24);
    assert_eq!(count_accent_bg(&buffer, theme.accent), 1);
}
