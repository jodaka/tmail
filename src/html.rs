//! Bounded HTML inspection and plain-text extraction shared by the reader
//! renderer and composer seeding.
//!
//! Both paths convert the same untrusted `text/html` part synchronously on
//! the single-threaded UI runtime, so the complexity gate lives here once
//! ([`html_for_render`]): it keeps html2text's quadratic nesting cost away
//! from any unbounded input, and both the rich renderer and the
//! quote/forward converter use the same verdict.

/// Largest HTML slice handed to html2text (review.md finding 7: the
/// conversion is synchronous on the single-threaded UI runtime, and a
/// linear 7.8 MiB parse already costs ~0.6 s in release). Real text-html
/// bodies sit far below; beyond this the body is parsed truncated, with an
/// explicit note line. Overflow never loses mail silently: the note says
/// what happened.
pub(crate) const MAX_HTML_RENDER_BYTES: usize = 1024 * 1024;

/// Deepest tag nesting handed to html2text (review.md finding 7:
/// html2text's cost grows quadratically with nesting depth — 10k tags
/// ≈ 0.6 s, 50k ≈ 15 s release, 100k ≈ 69 s — so a crafted deep email
/// froze the UI thread for minutes). Real email sits at tens-of-tags
/// depth, so 500 is generous; beyond it the body falls back to
/// tag-stripped plain text instead.
pub(crate) const MAX_HTML_NESTING_DEPTH: usize = 500;

/// What one HTML body should enter the conversion with: the depth-refusal
/// fallback or the bounded slice (whole, or size-truncated with a note).
pub(crate) enum HtmlText<'a> {
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
pub(crate) fn html_for_render(html: &str) -> HtmlText<'_> {
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

/// Flatten an HTML body into plain text for quoting or forwarding.
///
/// The conversion runs behind the same complexity gate as the reader, so
/// a crafted body costs at most one linear scan on the UI runtime.
/// Renderer notes (size/depth) are deliberately omitted: quoted mail
/// carries the sender's words, not Tmail's diagnostics. Lines wrap to
/// `width`; callers add a quote prefix, so pick a width that leaves room
/// for it.
pub(crate) fn plain_text(html: &str, width: usize) -> String {
    let width = width.max(10);
    match html_for_render(html) {
        HtmlText::Plain(text, _note) => text,
        HtmlText::Render(bounded) | HtmlText::RenderTruncated(bounded) => {
            // PlainDecorator renders structure as text rather than dropping
            // it: bullets, headings and nested quote markers survive, which
            // is what a quoted or forwarded body wants (the list preview
            // deliberately uses TrivialDecorator instead).
            html2text::config::with_decorator(html2text::render::PlainDecorator::new())
                .no_table_borders()
                .string_from_read(bounded.as_bytes(), width)
                .unwrap_or_default()
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_keeps_the_words_and_structure() {
        let text = plain_text(
            "<h1>Title</h1><p>a <b>loud</b> word</p><ul><li>one</li><li>two</li></ul>",
            80,
        );
        assert!(text.contains("Title"), "{text:?}");
        assert!(text.contains("a loud word"), "{text:?}");
        assert!(text.contains("* one"), "{text:?}");
        assert!(!text.contains('<'), "{text:?}");
    }

    #[test]
    fn plain_text_of_blank_markup_is_blank() {
        assert!(plain_text("<div>   </div>", 80).trim().is_empty());
        assert!(plain_text("", 80).trim().is_empty());
    }

    #[test]
    fn plain_text_refuses_deep_nesting_without_parsing() {
        let depth = MAX_HTML_NESTING_DEPTH + 1;
        let html = format!(
            "{}deep text{}",
            "<div>".repeat(depth),
            "</div>".repeat(depth)
        );
        // No renderer note in a quote: just the stripped words.
        assert_eq!(plain_text(&html, 80).trim(), "deep text");
    }
}
