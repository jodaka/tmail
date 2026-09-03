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

/// Render an HTML body into rich lines wrapped to `width` columns.
///
/// html5ever parses malformed markup the way browsers do (recovery, never a
/// panic, plan §13/§19 Phase 5 acceptance) and never performs network
/// requests for images or other remote resources (plan §13.5): `<img>`
/// renders as its alt text only.
pub(crate) fn html_to_rich(html: &str, width: usize) -> Vec<RichLine> {
    let width = width.max(10);
    let Ok(lines) = html2text::from_read_rich(html.as_bytes(), width) else {
        // from_read_rich only fails on I/O; an in-memory reader cannot, but
        // failing safe keeps the contract total.
        return Vec::new();
    };
    lines.iter().map(rich_line_from).collect()
}

/// Convert one html2text tagged line into an app-owned [`RichLine`].
///
/// Annotation mapping (plan §13 semantic table):
/// - `Strong` → bold; `Emphasis` → italic; `Link` → link (accent +
///   underline downstream) with the target retained on the span;
/// - `Code`/`Preformat` → code (whitespace preserved by html2text);
/// - `Image(src)` → the attached title text *is* the alt text; the src is
///   deliberately dropped (no alt-text leakage of remote URLs, no fetch);
/// - `Colour`/`BgColour`/`Strikeout`/`Default` → ignored (Post theme wins,
///   plan §13: "Sender color/background: Ignore; use Post theme").
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
            // the Post theme instead of failing the render.
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
    crate::ui::text::wrap(plain, width.max(1))
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
pub(crate) fn preview_text(message: &crate::domain::Message) -> Option<String> {
    const MAX_PREVIEW_WIDTH: usize = 200;
    let html = message.html_body.as_deref().map(|html| {
        html_to_rich(html, MAX_PREVIEW_WIDTH)
            .iter()
            .map(RichLine::text)
            .collect::<Vec<_>>()
            .join(" ")
    });
    let plain = message
        .plain_body
        .as_deref()
        .map(|body| body.split_whitespace().collect::<Vec<_>>().join(" "));
    for text in [html, plain].into_iter().flatten() {
        let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !collapsed.is_empty() {
            return Some(crate::ui::text::truncate(&collapsed, MAX_PREVIEW_WIDTH));
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
    fn images_render_alt_text_only_without_fetching() {
        let lines = html_to_rich(
            r#"<p>hi <img src="https://tracking.example.org/pixel.gif" alt="photo of a cat"> bye</p>"#,
            40,
        );
        let text = text_of(&lines).join("\n");
        assert!(text.contains("photo of a cat"), "{text:?}");
        assert!(!text.contains("tracking.example.org"), "{text:?}");
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
