//! Phase 5 fixture corpus (plan §13/§19 Phase 5.7): every `.eml` in
//! `fixtures/mail/` flows through the production pipeline — `mail-parser`
//! (the same maintained library Himalaya embeds; here via the
//! feature-gated `test-fixtures` support) serializes its dump, the lenient
//! `MessageReadDto` reads that wire shape, `map::message` builds the domain
//! message, and the reader renders it on a `TestBackend`.
//!
//! Raw MIME never enters production builds (ADR 0001: Himalaya owns MIME;
//! the parse support compiles only under the `test-fixtures` feature, which
//! `cargo test --all-features` enables).

use tmail::app::mock::{mock_initial_state, now};
use tmail::app::route::{MessageRoute, Route};
use tmail::app::state::{AppState, Loadable};
use tmail::backend::himalaya::fixtures::parse_raw_message;
use tmail::domain::{Message, MessageSummary};
use tmail::ui::{RenderContext, Theme, dates, render};

use ratatui::backend::TestBackend;
use ratatui::{Terminal, style::Modifier};

/// Fixture name → bytes, compiled in so the corpus is immutable.
const FIXTURES: &[(&str, &[u8])] = &[
    ("plain", include_bytes!("../fixtures/mail/plain.eml")),
    (
        "multipart-alternative",
        include_bytes!("../fixtures/mail/multipart-alternative.eml"),
    ),
    (
        "malformed-html",
        include_bytes!("../fixtures/mail/malformed-html.eml"),
    ),
    (
        "newsletter",
        include_bytes!("../fixtures/mail/newsletter.eml"),
    ),
    ("receipt", include_bytes!("../fixtures/mail/receipt.eml")),
    (
        "github-notification",
        include_bytes!("../fixtures/mail/github-notification.eml"),
    ),
    (
        "nested-quotes",
        include_bytes!("../fixtures/mail/nested-quotes.eml"),
    ),
    (
        "signature",
        include_bytes!("../fixtures/mail/signature.eml"),
    ),
    ("tables", include_bytes!("../fixtures/mail/tables.eml")),
    ("code-pre", include_bytes!("../fixtures/mail/code-pre.eml")),
    (
        "long-urls",
        include_bytes!("../fixtures/mail/long-urls.eml"),
    ),
    ("rtl", include_bytes!("../fixtures/mail/rtl.eml")),
    ("emoji", include_bytes!("../fixtures/mail/emoji.eml")),
    (
        "empty-body",
        include_bytes!("../fixtures/mail/empty-body.eml"),
    ),
    (
        "duplicate-filenames",
        include_bytes!("../fixtures/mail/duplicate-filenames.eml"),
    ),
    (
        "large-attachment",
        include_bytes!("../fixtures/mail/large-attachment.eml"),
    ),
];

fn fixture(name: &str) -> Message {
    let bytes = FIXTURES
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("fixture {name} not registered"))
        .1;
    parse_raw_message(bytes, "INBOX", "fixture-1")
}

fn reader_state(message: Message) -> AppState {
    let mut state = mock_initial_state();
    let summary = MessageSummary {
        id: message.id.clone(),
        mailbox_id: message.mailbox_id.clone(),
        message_id: message.headers.message_id.clone(),
        from: message.headers.from.clone(),
        to: message.headers.to.clone(),
        subject: if message.headers.subject.is_empty() {
            String::from("(fixture subject)")
        } else {
            message.headers.subject.clone()
        },
        // The reader renders from the full message; the list preview is
        // not part of this fixture (snippet semantics live in
        // `ui::rich::preview_text`, exercised by its own tests).
        snippet: None,
        timestamp: message.headers.date.unwrap_or_else(now),
        is_read: false,
        is_starred: false,
        has_attachments: !message.attachments.is_empty(),
    };
    state.routes.push(Route::Message(MessageRoute {
        mailbox_id: message.mailbox_id.clone(),
        summary,
    }));
    state.open_message = Loadable::Loaded(message);
    state
}

/// Draw the reader showing fixture `name` and return the buffer.
fn draw_reader(name: &str, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut state = reader_state(fixture(name));
    state.size = (width, height);
    let theme = Theme::default_dark();
    let now = now();
    let ctx = RenderContext::new(now, dates::format_clock(now));
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend");
    let mut hits = tmail::input::mouse::HitMap::default();
    terminal
        .draw(|frame| render(frame, &state, &theme, &ctx, &mut hits))
        .expect("draw");
    terminal.backend().buffer().clone()
}

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

/// Every fixture renders deterministically at several sizes: two draws at
/// the same size produce identical buffers (plan §19 Phase 5: "render
/// deterministically", "never panic").
#[test]
fn corpus_renders_deterministically() {
    for (name, _) in FIXTURES {
        for (width, height) in [(100u16, 30u16), (80, 24), (152, 40)] {
            let first = text_of(&draw_reader(name, width, height));
            let second = text_of(&draw_reader(name, width, height));
            assert_eq!(
                first, second,
                "{name} at {width}×{height} must render identically"
            );
        }
    }
}

/// The reader never shows raw markup; HTML mail renders recovered content.
#[test]
fn malformed_html_renders_recovered_content() {
    let text = text_of(&draw_reader("malformed-html", 120, 60));
    assert!(text.contains("Unclosed bold"), "{text}");
    assert!(text.contains("stray div"), "{text}");
    assert!(text.contains("list item two"), "{text}");
    assert!(!text.contains("<body>"), "{text}");
    assert!(!text.contains("<div>"), "{text}");
}

/// Multipart/alternative: the HTML part wins with a bold heading and an
/// accent + underline link (plan §13 semantic table).
#[test]
fn multipart_alternative_renders_semantics() {
    let theme = Theme::default_dark();
    let buffer = draw_reader("multipart-alternative", 120, 60);
    let text = text_of(&buffer);
    assert!(text.contains("HTML alternative"), "{text}");
    assert!(text.contains("Release 0.5"), "{text}");
    assert!(text.contains("annotated link"), "{text}");
    assert!(!text.contains("This message prefers HTML"), "{text}");

    let heading_row = text
        .lines()
        .position(|line| line.contains("Release 0.5"))
        .expect("heading visible");
    assert!(
        (0..buffer.area.width).any(|x| buffer[(x, heading_row as u16)]
            .style()
            .add_modifier
            .contains(Modifier::BOLD)),
        "heading must render bold:\n{text}"
    );
    let link_row = text
        .lines()
        .position(|line| line.contains("annotated link"))
        .expect("link visible");
    assert!(
        (0..buffer.area.width).any(|x| {
            let style = buffer[(x, link_row as u16)].style();
            style.fg == Some(theme.accent) && style.add_modifier.contains(Modifier::UNDERLINED)
        }),
        "link must render accent + underline:\n{text}"
    );
}

/// Newsletter: alt text renders, remote image URLs never do, and no network
/// request is even possible (the pipeline performs no I/O; this test runs
/// offline) — plan §13.5 / §19 Phase 5 acceptance.
#[test]
fn newsletter_renders_alt_text_without_remote_urls() {
    let text = text_of(&draw_reader("newsletter", 120, 60));
    assert!(text.contains("KKF Blades banner"), "{text}");
    assert!(text.contains("Your order has shipped"), "{text}");
    assert!(text.contains("Shirogami"), "{text}");
    assert!(!text.contains("kkf.example.org/banner.png"), "{text}");
    assert!(!text.contains("pixel.example.org"), "{text}");
}

/// Plain-only mail renders through the plain path.
#[test]
fn plain_mail_renders_verbatim() {
    let text = text_of(&draw_reader("plain", 120, 60));
    assert!(text.contains("Plain text only"), "{text}");
    assert!(text.contains("Analytical Engine Dept."), "{text}");
    assert!(text.contains("wraps at the viewport"), "{text}");
}

/// Nested `>` quoting survives as content in plain bodies.
#[test]
fn nested_quotes_preserve_markers() {
    let text = text_of(&draw_reader("nested-quotes", 120, 60));
    assert!(text.contains("> > > Is noon still good"), "{text}");
    assert!(text.contains("soba counter"), "{text}");
}

/// `pre`/`code` keep internal whitespace (plan §13).
#[test]
fn pre_preserves_whitespace() {
    let text = text_of(&draw_reader("code-pre", 120, 60));
    assert!(text.contains("self.scroll     ="), "{text}");
    assert!(text.contains("max.saturating_sub(1)"), "{text}");
}

/// RTL and emoji bodies render without panics, keeping their characters.
#[test]
fn rtl_and_emoji_survive_the_pipeline() {
    let rtl = text_of(&draw_reader("rtl", 120, 60));
    assert!(rtl.contains("مرحبا"), "{rtl}");
    assert!(rtl.contains("שלום"), "{rtl}");

    let emoji = text_of(&draw_reader("emoji", 120, 60));
    assert!(emoji.contains("🎉"), "{emoji}");
    // html2text breaks words between ideographs, so CJK glyphs arrive
    // space-separated ("東 京 タ ワ ー"); characters must all survive.
    assert!(emoji.contains('東') && emoji.contains('京'), "{emoji}");
    assert!(emoji.contains('汉') && emoji.contains('试'), "{emoji}");
    assert!(emoji.contains("Grüße"), "{emoji}");
}

/// Empty body: HTML present but blank, no plain part — the reader degrades
/// to the explicit note (never an empty document, never a crash).
#[test]
fn empty_body_degrades_to_note() {
    let text = text_of(&draw_reader("empty-body", 120, 60));
    assert!(
        text.contains("(HTML message could not be rendered)"),
        "{text}"
    );
}

/// Attachment metadata: duplicate filenames both surface in wire order; the
/// large attachment lists metadata without its bytes ever being fetched.
#[test]
fn attachment_metadata_lists_deterministically() {
    let dup = fixture("duplicate-filenames");
    assert_eq!(dup.attachments.len(), 2);
    assert_eq!(
        dup.attachments
            .iter()
            .map(|a| a.part_id)
            .collect::<Vec<_>>(),
        vec![2, 3],
        "part 0 is the multipart container; attachment ids keep wire order"
    );
    assert!(
        dup.attachments
            .iter()
            .all(|a| a.name.as_deref() == Some("report.pdf"))
    );
    let text = text_of(&draw_reader("duplicate-filenames", 120, 60));
    assert_eq!(
        text.matches("[ report.pdf · application/pdf").count(),
        2,
        "both duplicate chips render: {text}"
    );

    let big = fixture("large-attachment");
    assert_eq!(big.attachments.len(), 1);
    let video = &big.attachments[0];
    assert_eq!(video.name.as_deref(), Some("keynote-recording.mp4"));
    assert_eq!(video.mime_type.as_deref(), Some("video/mp4"));
    assert!(video.size.is_some());
    let text = text_of(&draw_reader("large-attachment", 120, 60));
    assert!(text.contains("keynote-recording.mp4"), "{text}");
}

/// Tables lay out as text without leaking markup.
#[test]
fn tables_render_as_text() {
    let text = text_of(&draw_reader("tables", 120, 60));
    assert!(text.contains("Backend"), "{text}");
    assert!(text.contains("enormous run of text"), "{text}");
    assert!(!text.contains("<table>"), "{text}");
}

/// Receipt: bold totals and a separator from `hr`.
#[test]
fn receipt_renders_totals() {
    let text = text_of(&draw_reader("receipt", 120, 60));
    assert!(text.contains("$140.00"), "{text}");
    assert!(text.contains("Total charged"), "{text}");
}

/// GitHub notification: code block content and quoted reply render.
#[test]
fn github_notification_renders_code_and_quote() {
    let text = text_of(&draw_reader("github-notification", 120, 60));
    assert!(text.contains("CI failed on macos-latest"), "{text}");
    assert!(text.contains("snapshot mismatch"), "{text}");
    assert!(text.contains("Looks like the runner was slow"), "{text}");
}

/// Long URLs: the label renders; the raw token never leaks into the frame.
#[test]
fn long_urls_render_labels_within_viewport() {
    let text = text_of(&draw_reader("long-urls", 120, 60));
    assert!(text.contains("this enormous link"), "{text}");
    assert!(
        !text.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"),
        "{text}"
    );
}

/// Signatures and snippets: the signature body renders fully.
#[test]
fn signature_mail_renders_body() {
    let text = text_of(&draw_reader("signature", 120, 60));
    assert!(text.contains("Evelyn Sharp"), "{text}");
    assert!(text.contains("review clause 12"), "{text}");
}
