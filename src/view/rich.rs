//! App-owned rich text model and the HTML→terminal rendering subsystem
//! (plan §13: "Convert HTML into an app-owned `RichText` model, then into
//! Ratatui lines/spans. Do not let a third-party renderer's types leak into
//! UI state.").
//!
//! Pipeline: `Message.html_body` → [`html_to_rich`] (html2text/html5ever,
//! lenient, never fetches remote resources) → [`RichLine`]s → the reader
//! maps styles onto theme tokens. Plain bodies go through [`plain_to_rich`]
//! so both paths share one rendering contract. Renderer errors and blank
//! output never panic; the caller falls back to plain text (plan §13.3).

use html2text::render::{RichAnnotation, TaggedLine, TaggedLineElement};

/// Style flags for one span. Flags are orthogonal; the reader maps them to
/// theme tokens (no literal colors here — plan §18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RichStyle {
    /// Headings and `<strong>` (plan §13: heading/strong → bold).
    pub bold: bool,
    /// `<em>` (plan §13: emphasis → italic when supported).
    pub italic: bool,
    /// Link text (plan §13: accent + underline; target kept on the span).
    pub link: bool,
    /// `<code>` and `<pre>` content (plan §13: preserve whitespace).
    pub code: bool,
    /// Blockquote content (plan §13: dim text with left marker).
    pub blockquote: bool,
    /// Heading text (subset of bold; lets the theme pick a stronger color).
    pub heading: bool,
}

/// One styled run of text with an optional link target (plan §13: "retain
/// target for selection/copy/open later" — Phase 8/10 consume it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RichSpan {
    pub text: String,
    pub style: RichStyle,
    pub link_target: Option<String>,
}

/// One rendered line: styled spans, already wrapped to the requested width
/// (code/pre lines keep their original whitespace and are clipped, not
/// wrapped, by the reader).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct RichLine {
    pub spans: Vec<RichSpan>,
}

impl RichLine {
    /// A single default-styled span (plain text, placeholders).
    pub(crate) fn from_plain(text: impl Into<String>) -> Self {
        RichLine {
            spans: vec![RichSpan {
                text: text.into(),
                style: RichStyle::default(),
                link_target: None,
            }],
        }
    }

    /// Full display text (tests and blank checks).
    pub(crate) fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

/// Largest HTML slice the reader parses through html2text (review.md
/// finding 7: the conversion is synchronous on the single-threaded UI
/// runtime, and a linear 7.8 MiB parse already costs ~0.6 s in release).
/// Real text-html bodies sit far below; beyond this the body is parsed
/// truncated, with an explicit note line. Overflow never loses mail
/// silently: the note says what happened.
const MAX_HTML_RENDER_BYTES: usize = 1024 * 1024;

/// Deepest tag nesting the reader converts through html2text (review.md
/// finding 7: html2text's cost grows quadratically with nesting depth —
/// 10k tags ≈ 0.6 s, 50k ≈ 15 s release, 100k ≈ 69 s — so a crafted deep
/// email froze the UI thread for minutes). Real email sits at tens-of-
/// tags depth, so 500 is generous; beyond it the body renders as
/// tag-stripped plain text instead.
const MAX_HTML_NESTING_DEPTH: usize = 500;

/// What one HTML body should enter the conversion with: the depth-refusal
/// fallback or the bounded slice (whole, or size-truncated with a note).
enum HtmlText<'a> {
    /// Whole body: convert directly.
    Render(&'a str),
    /// Prefix slice: convert, then append the truncation note.
    RenderTruncated(&'a str),
    /// Depth-refusal fallback: tag-stripped text plus the reason note.
    Plain(String, String),
}

/// Complexity gate for every HTML conversion (review.md finding 7): one
/// linear pre-scan decides between the direct conversion and the plain
/// fallback, so the quadratic parser never sees an unbounded input.
fn html_for_render(html: &str) -> HtmlText<'_> {
    let scan = scan_html(html);
    if scan.max_depth > MAX_HTML_NESTING_DEPTH {
        return HtmlText::Plain(
            scan.stripped_text(),
            format!(
                "(rich rendering skipped: the HTML nests more than {MAX_HTML_NESTING_DEPTH} levels)"
            ),
        );
    }
    if html.len() > MAX_HTML_RENDER_BYTES {
        // Cut at a char boundary so the slice stays valid UTF-8.
        let mut end = MAX_HTML_RENDER_BYTES;
        while !html.is_char_boundary(end) {
            end -= 1;
        }
        return HtmlText::RenderTruncated(html.split_at(end).0);
    }
    HtmlText::Render(html)
}

/// Render an HTML body into rich lines wrapped to `width` columns.
///
/// html5ever parses malformed markup the way browsers do (recovery, never a
/// panic, plan §13/§19 Phase 5 acceptance) and never performs network
/// requests for images or other remote resources (plan §13.5): `<img>`
/// renders as its alt text only.
///
/// Complexity is bounded first ([`html_for_render`], review.md finding 7):
/// inputs beyond the depth or size ceilings never reach the quadratic
/// parser — they render as tag-stripped plain text with an explicit note,
/// so a crafted email can cost at most one linear pass of the UI thread.
pub(crate) fn html_to_rich(html: &str, width: usize) -> Vec<RichLine> {
    let width = width.max(10);
    match html_for_render(html) {
        HtmlText::Plain(text, note) => {
            let mut lines = plain_to_rich(&text, width);
            lines.push(RichLine::from_plain(note));
            lines
        }
        HtmlText::Render(bounded) => convert_html(bounded, width),
        HtmlText::RenderTruncated(bounded) => {
            let mut lines = convert_html(bounded, width);
            lines.push(RichLine::from_plain(format!(
                "(long HTML body truncated at {})",
                crate::ui::text::human_size(MAX_HTML_RENDER_BYTES as u64)
            )));
            lines
        }
    }
}

/// The direct html2text conversion of an already-bounded slice.
fn convert_html(bounded: &str, width: usize) -> Vec<RichLine> {
    let Ok(lines) = html2text::from_read_rich(bounded.as_bytes(), width) else {
        // from_read_rich only fails on I/O; an in-memory reader cannot, but
        // failing safe keeps the contract total.
        return Vec::new();
    };
    lines.iter().map(rich_line_from).collect()
}

/// What the linear HTML pre-scan learned about one input: the observed
/// tag-nesting depth (the complexity verdict) and the tag-stripped text
/// (the depth-refusal fallback's rendering input). The scan allocates no
/// more than the stripped text itself.
struct HtmlScan {
    max_depth: usize,
    text: Vec<u8>,
}

impl HtmlScan {
    /// Tag-stripped text (`from_utf8_lossy` keeps the contract total on
    /// broken utf-8, like every other body decoding).
    fn stripped_text(&self) -> String {
        String::from_utf8_lossy(&self.text).into_owned()
    }
}

/// Linear HTML pre-scan. Enough lexer to know where tags start and end —
/// comments, doctypes/processing instructions, quoted attribute values,
/// and the raw-text `<script>`/`<style>` elements — without reproducing
/// html5ever: the complexity guard needs unbiased-enough nesting counts
/// and stripped text, both of which a boundary-correct lexer gives.
/// Depth is a saturating count (open non-self-closing tags increment,
/// closing tags decrement at zero), so malformed mail drifts downward
/// instead of running depth away. Strictly linear: one pass, two output
/// allocations — a crafted deep email costs one scan, never the parse.
fn scan_html(html: &str) -> HtmlScan {
    let bytes = html.as_bytes();
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum State {
        /// Rendered text: collect until `<`.
        Text,
        /// Inside `<!--` … `-->`.
        Comment,
        /// Inside `<script>`/`<style>`: opaque until the case-insensitive
        /// matching closing tag.
        RawText { closing: RawClose },
    }
    /// The lazy case-insensitive search for a raw-text element's closer.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct RawClose {
        /// `script` or `style`, lowercase.
        kind: &'static [u8],
        /// How much of `</kind` already matched (bytes consumed of it).
        matched: usize,
    }
    let mut depth = 0usize;
    let mut max_depth = 0usize;
    let mut state = State::Text;
    let mut text: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        match state {
            State::Text => {
                if bytes[i] == b'<' {
                    if bytes[i..].starts_with(b"<!--") {
                        state = State::Comment;
                        i += 4;
                        continue;
                    }
                    i += 1;
                    let tag_start = i;
                    // Skip to the unquoted `>`; `>` inside quoted attribute
                    // values cannot end the tag.
                    let mut j = i;
                    let mut quote = 0u8;
                    while j < bytes.len() {
                        let byte = bytes[j];
                        if quote != 0 {
                            if byte == quote {
                                quote = 0;
                            }
                        } else if byte == b'"' || byte == b'\'' {
                            quote = byte;
                        } else if byte == b'>' {
                            break;
                        }
                        j += 1;
                    }
                    let tag = &bytes[tag_start..j.min(bytes.len())];
                    let raw = tag_first_word(tag);
                    if raw.is_empty() {
                        // `<>` or an unterminated tag: rendered text only.
                        i = j + 1;
                        continue;
                    }
                    let lower: Vec<u8> = raw.to_ascii_lowercase();
                    if lower.first() == Some(&b'!') || lower.first() == Some(&b'?') {
                        // Declaration/processing instruction: never depth.
                    } else if raw[0] == b'/' {
                        depth = depth.saturating_sub(1);
                    } else if raw.last() != Some(&b'/') {
                        // Open tag without self-closing: one level down.
                        depth += 1;
                        max_depth = max_depth.max(depth);
                        if lower == b"script" || lower == b"style" {
                            state = State::RawText {
                                closing: RawClose {
                                    kind: if lower == b"script" {
                                        b"script"
                                    } else {
                                        b"style"
                                    },
                                    matched: 0,
                                },
                            };
                        }
                    }
                    i = j + 1;
                } else {
                    text.push(bytes[i]);
                    i += 1;
                }
            }
            State::Comment => {
                if bytes[i..].starts_with(b"-->") {
                    state = State::Text;
                    i += 3;
                } else {
                    i += 1;
                }
            }
            State::RawText { mut closing } => {
                // Byte-by-byte case-insensitive search for `</kind`: the
                // matcher resets on any mismatch, so a stray `<` or quoting
                // noise inside the element never derails it.
                let matched = closing.matched;
                let expected = if matched == 0 {
                    b'<'
                } else if matched == 1 {
                    b'/'
                } else {
                    closing.kind[matched - 2]
                };
                if bytes[i].to_ascii_lowercase() == expected {
                    closing.matched = matched + 1;
                    if closing.matched >= closing.kind.len() + 2 {
                        state = State::Text;
                    } else {
                        state = State::RawText { closing };
                    }
                } else {
                    state = State::RawText {
                        closing: RawClose {
                            kind: closing.kind,
                            matched: 0,
                        },
                    };
                }
                i += 1;
            }
        }
    }
    HtmlScan { max_depth, text }
}

/// The tag's first word in raw bytes: prefix decorations (`/`, `!`) and
/// attribute noise stripped. Comparisons on it are case-sensitive here;
/// the caller lowercases where needed.
fn tag_first_word(tag: &[u8]) -> &[u8] {
    let word_end = tag
        .iter()
        .position(|byte| byte.is_ascii_whitespace())
        .unwrap_or(tag.len());
    &tag[..word_end]
}

/// Convert one html2text tagged line into an app-owned [`RichLine`].
///
/// Annotation mapping (plan §13 semantic table):
/// - `Strong` → bold; `Emphasis` → italic; `Link` → link (accent +
///   underline downstream) with the target retained on the span;
/// - `Code`/`Preformat` → code (whitespace preserved by html2text);
/// - `Image(src)` → the attached title text *is* the alt text; the src is
///   deliberately dropped (no alt-text leakage of remote URLs, no fetch);
/// - `Colour`/`BgColour`/`Strikeout`/`Default` → ignored (Tmail theme wins,
///   plan §13: "Sender color/background: Ignore; use Tmail theme").
///
/// Heading (`# `) and blockquote (`> `) markers are plain text prefixes in
/// html2text's rich output rather than annotations, so they are detected on
/// the rendered line and applied as styles to the whole line.
fn rich_line_from(line: &TaggedLine<Vec<RichAnnotation>>) -> RichLine {
    let mut spans: Vec<RichSpan> = Vec::new();
    for element in line.iter() {
        let TaggedLineElement::Str(tagged) = element else {
            // FragmentStart markers are zero-width bookkeeping.
            continue;
        };
        let style = style_from_annotations(&tagged.tag);
        let link_target = tagged.tag.iter().find_map(|a| match a {
            RichAnnotation::Link(url) => Some(url.clone()),
            _ => None,
        });
        merge_span(&mut spans, &tagged.s, style, link_target);
    }
    let mut rich = RichLine { spans };
    apply_line_markers(&mut rich);
    rich
}

/// Collapse adjacent spans that share style and link target.
fn merge_span(
    spans: &mut Vec<RichSpan>,
    text: &str,
    style: RichStyle,
    link_target: Option<String>,
) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = spans.last_mut()
        && last.style == style
        && last.link_target == link_target
    {
        last.text.push_str(text);
        return;
    }
    spans.push(RichSpan {
        text: text.to_string(),
        style,
        link_target,
    });
}

fn style_from_annotations(annotations: &[RichAnnotation]) -> RichStyle {
    let mut style = RichStyle::default();
    for annotation in annotations {
        match annotation {
            RichAnnotation::Strong => style.bold = true,
            RichAnnotation::Emphasis => style.italic = true,
            RichAnnotation::Link(_) => style.link = true,
            RichAnnotation::Code | RichAnnotation::Preformat(_) => style.code = true,
            RichAnnotation::Image(_)
            | RichAnnotation::Colour(_)
            | RichAnnotation::BgColour(_)
            | RichAnnotation::Strikeout
            | RichAnnotation::Default
            // The enum is `#[non_exhaustive]`: future annotations degrade to
            // the Tmail theme instead of failing the render.
            | _ => {}
        }
    }
    style
}

/// html2text renders headings as `#`-prefixed lines and blockquotes as
/// `>`-prefixed lines (its rich decorator emits them as un-annotated text
/// prefixes). Detect those markers and tag the line so the theme can dim
/// blockquotes (left marker included) and bold headings (plan §13).
fn apply_line_markers(line: &mut RichLine) {
    let text = line.text();
    let heading_dashes = text.starts_with('#') && {
        let dashes = text.chars().take_while(|ch| *ch == '#').count();
        dashes <= 6 && text[dashes..].starts_with(' ')
    };
    let quote_depth = text.chars().take_while(|ch| *ch == '>').count();
    let is_quote = quote_depth > 0
        && text[quote_depth..].starts_with(' ')
        // `pre` content keeps literal `>` characters; annotation wins.
        && !line.spans.iter().any(|span| span.style.code);
    if heading_dashes {
        for span in &mut line.spans {
            span.style.bold = true;
            span.style.heading = true;
        }
    }
    if is_quote {
        for span in &mut line.spans {
            span.style.blockquote = true;
        }
    }
}

/// Convert a plain-text body into rich lines wrapped to `width` columns
/// (word boundaries, Unicode-safe hard chunking of oversized tokens —
/// plan §13: long URLs never corrupt the layout).
pub(crate) fn plain_to_rich(plain: &str, width: usize) -> Vec<RichLine> {
    crate::view::text::wrap(plain, width.max(1))
        .into_iter()
        .map(RichLine::from_plain)
        .collect()
}

/// True when the rendered document carries no visible text (used to decide
/// the plain-text fallback, plan §13.3).
pub(crate) fn is_blank(lines: &[RichLine]) -> bool {
    lines.iter().all(|line| line.text().trim().is_empty())
}

/// One-line plain-text preview of a message body for the list rows (ticket
/// wxtx): the body converted to plain text and stringified. Body selection
/// matches the reader (`body_lines`): HTML first, plain fallback when the
/// HTML is absent or renders blank. Newlines collapse to single spaces so
/// the preview is one string; it is capped (with a trailing `…`) so a huge
/// body never bloats the summary or the cached page.
///
/// Unlike the reader's rich render, the HTML conversion is bare text only
/// (ticket h2em): no table borders, no heading/quote/bullet/link markers —
/// the list row wants the words, not the layout.
pub(crate) fn preview_text(message: &crate::domain::Message) -> Option<String> {
    const MAX_PREVIEW_WIDTH: usize = 200;
    let html = message.html_body.as_deref().map(|html| {
        // The preview builds on the same successful page draws as the list
        // rows, so the complexity gate protects it too (review.md finding
        // 7): a depth-bound body uses the tag-stripped fallback text and a
        // size-bound body the truncated prefix, never the raw quadratic
        // parser on an unbounded input.
        match html_for_render(html) {
            HtmlText::Render(bounded) | HtmlText::RenderTruncated(bounded) => {
                html2text::config::with_decorator(html2text::render::TrivialDecorator::new())
                    .no_table_borders()
                    .string_from_read(bounded.as_bytes(), MAX_PREVIEW_WIDTH)
                    .unwrap_or_default()
            }
            HtmlText::Plain(text, _note) => text,
        }
    });
    let plain = message
        .plain_body
        .as_deref()
        .map(|body| body.split_whitespace().collect::<Vec<_>>().join(" "));
    for text in [html, plain].into_iter().flatten() {
        let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !collapsed.is_empty() {
            return Some(crate::view::text::truncate(&collapsed, MAX_PREVIEW_WIDTH));
        }
    }
    None
}

/// MIME body selection (plan §13.2/3): prefer the HTML part for the richer
/// Gmail-like experience; fall back to plain text when HTML is absent or
/// renders blank (malformed markup recovers inside html5ever, but an empty
/// result still degrades gracefully). Both source bodies stay intact on
/// `Message` for retry/debugging; only rendering notes are surfaced here.
pub(crate) fn body_lines(message: &crate::domain::Message, width: usize) -> Vec<RichLine> {
    let width = width.max(1);
    if let Some(html) = &message.html_body {
        let lines = html_to_rich(html, width);
        if !is_blank(&lines) {
            return lines;
        }
    }
    if let Some(plain) = &message.plain_body {
        return plain_to_rich(plain, width);
    }
    if message.html_body.is_some() {
        // HTML present but nothing rendered from it; wrap like any text so
        // even narrow terminals stay inside their width budget.
        plain_to_rich("(HTML message could not be rendered)", width)
    } else {
        plain_to_rich("(no content)", width)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(lines: &[RichLine]) -> Vec<String> {
        lines.iter().map(RichLine::text).collect()
    }

    fn preview_message(plain: Option<String>, html: Option<String>) -> crate::domain::Message {
        crate::domain::Message {
            id: crate::domain::MessageId(String::from("m1")),
            mailbox_id: crate::domain::MailboxId(String::from("inbox")),
            headers: crate::domain::MessageHeaders::default(),
            plain_body: plain,
            html_body: html,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn preview_text_collapses_the_plain_body_to_one_line() {
        let message = preview_message(
            Some(String::from(
                "Dear team,\n\n  shipping   on Friday.\nBest,\nA",
            )),
            None,
        );
        let preview = preview_text(&message).expect("preview");
        assert_eq!(preview, "Dear team, shipping on Friday. Best, A");
        assert!(!preview.contains('\n'));
    }

    #[test]
    fn preview_text_prefers_html_and_strips_markup() {
        let message = preview_message(
            Some(String::from("plain fallback")),
            Some(String::from(
                "<html><body><p>HTML <b>wins</b></p><p>second para</p></body></html>",
            )),
        );
        let preview = preview_text(&message).expect("preview");
        assert!(preview.starts_with("HTML wins"), "{preview}");
        assert!(preview.contains("second para"), "{preview}");
        assert!(!preview.contains("plain fallback"));
        assert!(!preview.contains('<'));
    }

    /// The list preview is bare text only (ticket h2em): tables, rules,
    /// headings, quotes, bullets, and images lose all their formatting
    /// markers — unlike the reader, which renders them richly.
    #[test]
    fn preview_text_drops_html_formatting_markers() {
        let message = preview_message(
            None,
            Some(String::from(
                "<html><body>\
                 <h2>Receipt</h2>\
                 <hr>\
                 <table><tr><th>Item</th><th>Qty</th></tr>\
                 <tr><td>Gyuto</td><td>1</td></tr></table>\
                 <blockquote><p>quoted words</p></blockquote>\
                 <ul><li>bullet one</li><li>bullet two</li></ul>\
                 <p><img src=\"x.png\" alt=\"photo\"> done</p>\
                 </body></html>",
            )),
        );
        let preview = preview_text(&message).expect("preview");
        for marker in ['─', '│', '┬', '┴', '+', '-', '#', '>', '*', '['] {
            assert!(!preview.contains(marker), "{marker:?} leaked: {preview}");
        }
        for word in [
            "Receipt",
            "Item",
            "Gyuto",
            "quoted words",
            "bullet one",
            "bullet two",
            "photo",
            "done",
        ] {
            assert!(preview.contains(word), "{word} missing: {preview}");
        }
    }

    #[test]
    fn preview_text_caps_long_bodies_with_an_ellipsis() {
        let long = "word ".repeat(200);
        let message = preview_message(Some(long), None);
        let preview = preview_text(&message).expect("preview");
        assert!(
            unicode_width::UnicodeWidthStr::width(preview.as_str()) <= 200,
            "capped, got {}",
            unicode_width::UnicodeWidthStr::width(preview.as_str())
        );
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn preview_text_of_an_empty_body_is_none() {
        assert!(preview_text(&preview_message(None, None)).is_none());
        assert!(
            preview_text(&preview_message(Some(String::from("  \n\t ")), None)).is_none(),
            "whitespace-only bodies carry no preview"
        );
    }

    #[test]
    fn plain_text_wraps_at_width() {
        let lines = plain_to_rich("alpha beta gamma", 11);
        assert_eq!(text_of(&lines), vec!["alpha beta", "gamma"]);
        assert!(lines.iter().all(|l| l.spans.iter().all(|s| !s.style.bold)));
    }

    #[test]
    fn html_headings_and_strong_render_bold() {
        let lines = html_to_rich("<h1>Title</h1><p>a <b>loud</b> word</p>", 40);
        let text = text_of(&lines);
        assert!(text.iter().any(|t| t.contains("Title")), "{text:?}");
        let heading = lines.iter().find(|l| l.text().contains("Title")).unwrap();
        assert!(
            heading
                .spans
                .iter()
                .all(|s| s.style.bold && s.style.heading)
        );
        let loud = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.text.contains("loud"))
            .expect("strong span");
        assert!(loud.style.bold && !loud.style.heading);
    }

    #[test]
    fn html_emphasis_is_italic() {
        let lines = html_to_rich("<p>a <i>soft</i> word</p>", 40);
        let soft = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.text.contains("soft"))
            .expect("em span");
        assert!(soft.style.italic);
    }

    #[test]
    fn links_carry_targets_and_mark_spans() {
        let lines = html_to_rich(
            r#"<p>see <a href="https://example.org/x">the docs</a></p>"#,
            40,
        );
        let link = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.text.contains("the docs"))
            .expect("link span");
        assert!(link.style.link);
        assert_eq!(link.link_target.as_deref(), Some("https://example.org/x"));
    }

    #[test]
    fn blockquotes_get_marker_and_style() {
        let lines = html_to_rich("<blockquote><p>wisdom</p></blockquote>", 40);
        let text = text_of(&lines);
        assert!(
            text.iter()
                .any(|t| t.contains("> ") && t.contains("wisdom")),
            "{text:?}"
        );
        let quoted = lines.iter().find(|l| l.text().contains("wisdom")).unwrap();
        assert!(quoted.spans.iter().all(|s| s.style.blockquote));
    }

    #[test]
    fn pre_preserves_whitespace_and_flags_code() {
        let html = "<pre>  keep   spacing\n    and  newlines</pre>";
        let lines = html_to_rich(html, 40);
        let text = text_of(&lines);
        assert!(
            text.iter().any(|t| t.contains("  keep   spacing")),
            "{text:?}"
        );
        let pre = lines.iter().find(|l| l.text().contains("keep")).unwrap();
        assert!(pre.spans.iter().all(|s| s.style.code));
    }

    #[test]
    fn literal_gt_lines_in_pre_never_render_as_quotes() {
        // Plain-text replies pasted into <code>/<pre>: a literal `>` is
        // content, not a quote marker (the code guard in
        // `apply_line_markers`). Pinned so decorator changes cannot
        // silently break it (review s843).
        let lines = html_to_rich("<pre>> wrote:\noriginal text</pre>", 40);
        let quoted = lines.iter().find(|l| l.text().contains("wrote")).unwrap();
        assert!(quoted.spans.iter().all(|s| !s.style.blockquote));
    }

    #[test]
    fn images_render_alt_text_only_without_fetching() {
        let lines = html_to_rich(
            r#"<p>hi <img src="https://tracking.example.org/pixel.gif" alt="photo of a cat"> bye</p>"#,
            40,
        );
        let text = text_of(&lines).join("\n");
        assert!(text.contains("photo of a cat"), "{text:?}");
        assert!(!text.contains("tracking.example.org"), "{text:?}");
    }

    /// Review.md finding 7: quadratically deep nesting must never reach
    /// html2text — the depth-refusal renders tag-stripped text fast (a
    /// linear scan) with an explicit note, and the semantic markers of the
    /// rich render (bold, headings) are cleanly absent.
    #[test]
    fn deeply_nested_html_skips_the_rich_parser() {
        const DEPTH: usize = MAX_HTML_NESTING_DEPTH + 1;
        let mut html = String::new();
        for _ in 0..DEPTH {
            html.push_str("<div>");
        }
        html.push_str("the rare centered body");
        for _ in 0..DEPTH {
            html.push_str("</div>");
        }
        let t = std::time::Instant::now();
        let lines = html_to_rich(&html, 60);
        let elapsed = t.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "depth refusal must be linear, took {elapsed:?}"
        );
        let text = text_of(&lines).join("\n");
        assert!(text.contains("the rare centered body"), "{text:?}");
        assert!(text.contains("rich rendering skipped"), "{text:?}");
    }

    /// Review.md finding 7: over-sized HTML renders its bounded prefix
    /// with an explicit truncation note instead of blocking the frame on
    /// the whole byte stream.
    #[test]
    fn oversized_html_is_truncated_with_a_note() {
        let mut html = String::from("<p>start here</p>");
        html.push_str("<p>");
        html.push_str(&"filler".repeat(300_000));
        html.push_str("</p>");
        let lines = html_to_rich(&html, 40);
        let text = text_of(&lines).join("\n");
        assert!(text.contains("start here"), "{text:?}");
        assert!(text.contains("truncated at"), "{text:?}");
        // The far end of the body cannot have made it onto the screen.
        assert!(!text.contains(&"filler".repeat(5_000)), "{text:?}");
    }

    /// The complexity gate must not misfire on ordinary mail: normal depth
    /// and comments and quoted values stay on the direct render, and the
    /// stripped-text fallback for depth refusal is the honest words-only
    /// rendering (no tags, no script/style code).
    #[test]
    fn ordinary_html_still_rich_renders_through_the_gate() {
        let html = concat!(
            "<!-- a comment -->",
            r#"<div class=\"a > b\"><strong>lead</strong><script>var x = \"<div></div>\";</script><style>div {}</style><p>rest</p></div>"#
        );
        let lines = html_to_rich(html, 40);
        let text = text_of(&lines).join("\n");
        assert!(text.contains("lead"), "{text:?}");
        assert!(text.contains("rest"), "{text:?}");
        assert!(!text.contains("rich rendering skipped"), "{text:?}");
    }

    /// The fallback text for a depth-refused body hides scripts and
    /// styles and keeps the visible words.
    #[test]
    fn depth_refusal_fallback_hides_scripts_and_styles() {
        const DEPTH: usize = MAX_HTML_NESTING_DEPTH + 1;
        let mut html = String::new();
        for _ in 0..DEPTH {
            html.push_str("<div>");
        }
        html.push_str(
            "<script>alert('leak-proof? not the point — just absent')</script><style>div{}</style>visible words",
        );
        for _ in 0..DEPTH {
            html.push_str("</div>");
        }
        let lines = html_to_rich(&html, 60);
        let text = text_of(&lines).join("\n");
        assert!(text.contains("visible words"), "{text:?}");
        assert!(!text.contains("alert"), "{text:?}");
        assert!(!text.contains("div{}"), "{text:?}");
    }

    #[test]
    fn malformed_html_recovers_without_panicking() {
        let html = "<p>unclosed <b>bold <div>stray</p></div> more <<weird>> text";
        let lines = html_to_rich(html, 40);
        let text = text_of(&lines).join("\n");
        assert!(text.contains("unclosed"), "{text:?}");
        assert!(text.contains("more"), "{text:?}");
    }

    #[test]
    fn tables_layout_within_width() {
        let html =
            "<table><tr><th>Item</th><th>Qty</th></tr><tr><td>Knife</td><td>2</td></tr></table>";
        let lines = html_to_rich(html, 30);
        assert!(
            lines
                .iter()
                .all(|l| unicode_width::UnicodeWidthStr::width(l.text().as_str()) <= 30)
        );
        let text = text_of(&lines).join("\n");
        assert!(text.contains("Knife") && text.contains("2"), "{text:?}");
    }

    #[test]
    fn long_urls_wrap_without_panics() {
        let html = "<p>see https://example.org/a/very/long/path/with/many/segments/that/exceeds/any/width here</p>";
        let lines = html_to_rich(html, 20);
        assert!(
            lines
                .iter()
                .all(|l| unicode_width::UnicodeWidthStr::width(l.text().as_str()) <= 20)
        );
        assert!(!is_blank(&lines));
    }

    #[test]
    fn unicode_and_rtl_survive_rendering() {
        let html = "<p>Grüße 🎉 مرحبا بالعالم שלום</p>";
        let lines = html_to_rich(html, 20);
        let text = text_of(&lines).join("\n");
        assert!(text.contains("Grüße"), "{text:?}");
        assert!(text.contains("مرحبا"), "{text:?}");
    }

    #[test]
    fn blank_html_detects_empty_output() {
        assert!(is_blank(&html_to_rich("", 40)));
        assert!(is_blank(&html_to_rich("<div><span>   </span></div>", 40)));
        assert!(!is_blank(&html_to_rich("<p>x</p>", 40)));
    }
}
