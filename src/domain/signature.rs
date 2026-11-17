//! Signature seeding for composer drafts (RFC 3676 §4.3).
//!
//! Tmail builds and serializes outgoing MIME itself, so himalaya's
//! compose-time `signature` handling never runs. The configured signature
//! is instead seeded into the draft: after the author's text and before
//! any quoted or forwarded source, introduced by the signature delimiter
//! line RFC 3676 §4.3 reserves for exactly this purpose.
//!
//! Himalaya documents `signature-delim` as written verbatim; the default
//! is the RFC 3676 `"-- "` line (dash dash space) followed by a newline.
//! A configured delimiter that lacks its own trailing newline gets one so
//! the signature always starts on its own line.

/// The default signature delimiter, including its trailing newline: a
/// line containing exactly `-- ` (dash dash space), per RFC 3676 §4.3.
pub const DEFAULT_SIGNATURE_DELIM: &str = "-- \n";

/// A resolved signature: the delimiter line plus the signature text.
///
/// The text is normalized to `\n` line endings with no trailing newline,
/// so callers can place the rendered [`Signature::block`] between the
/// author's text and a quoted body without producing stray blank lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    /// The delimiter line, ending in `\n` unless explicitly configured
    /// empty.
    delimiter: String,
    /// The signature text, without its trailing newline.
    text: String,
}

impl Signature {
    /// Resolve a configured signature.
    ///
    /// Returns `None` when the text is empty once CRLF is normalized and
    /// trailing newlines are dropped; a whitespace-only signature is kept
    /// (it is still an explicit user choice). The delimiter is normalized
    /// to LF and terminated by a newline so it stands on its own line;
    /// an empty delimiter is left empty.
    pub fn new(text: &str, delimiter: &str) -> Option<Self> {
        let text = normalize_newlines(text);
        let text = text.trim_end_matches('\n');
        if text.is_empty() {
            return None;
        }
        Some(Self {
            delimiter: normalize_delimiter(delimiter),
            text: text.to_owned(),
        })
    }

    /// The delimiter line, including its trailing newline.
    pub fn delimiter(&self) -> &str {
        &self.delimiter
    }

    /// The signature text, without its trailing newline.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The rendered signature block: delimiter plus text, with no trailing
    /// newline. Callers add the blank separation from surrounding text.
    pub fn block(&self) -> String {
        format!("{}{}", self.delimiter, self.text)
    }
}

/// Normalize CRLF and lone CR line endings to LF. The composer and draft
/// journal work in `\n`; a TOML basic string written on another platform
/// can otherwise carry `\r`.
fn normalize_newlines(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}

/// A delimiter is a line of its own: normalize its endings and append the
/// missing newline (Himalaya writes the value verbatim, and users commonly
/// set just `"-- "`). An explicitly empty delimiter stays empty.
fn normalize_delimiter(delimiter: &str) -> String {
    let mut delimiter = normalize_newlines(delimiter);
    if !delimiter.is_empty() && !delimiter.ends_with('\n') {
        delimiter.push('\n');
    }
    delimiter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_delimiter_is_the_rfc_3676_line() {
        assert_eq!(DEFAULT_SIGNATURE_DELIM, "-- \n");
        let signature = Signature::new("С уважением,\nАнтон Кудрис", DEFAULT_SIGNATURE_DELIM)
            .expect("non-empty signature");
        assert_eq!(signature.delimiter(), "-- \n");
        assert_eq!(signature.block(), "-- \nС уважением,\nАнтон Кудрис");
        // The delimiter line itself is exactly dash-dash-space.
        assert!(signature.block().starts_with("-- \n"));
    }

    #[test]
    fn empty_signature_is_ignored() {
        assert_eq!(Signature::new("", DEFAULT_SIGNATURE_DELIM), None);
        assert_eq!(Signature::new("\r\n\n", DEFAULT_SIGNATURE_DELIM), None);
    }

    #[test]
    fn trailing_newlines_and_crlf_are_normalized() {
        let signature = Signature::new("Best,\r\nAlice\r\n\r\n", DEFAULT_SIGNATURE_DELIM)
            .expect("non-empty signature");
        assert_eq!(signature.text(), "Best,\nAlice");
        assert_eq!(signature.block(), "-- \nBest,\nAlice");
    }

    #[test]
    fn a_custom_delimiter_gets_its_own_line() {
        // Himalaya writes `signature-delim` verbatim; a value without a
        // trailing newline would otherwise merge the text onto the
        // delimiter line.
        let signature = Signature::new("Alice", "~~~").expect("non-empty signature");
        assert_eq!(signature.block(), "~~~\nAlice");

        let signature = Signature::new("Alice", "~~~\r\n").expect("non-empty signature");
        assert_eq!(signature.block(), "~~~\nAlice");
    }

    #[test]
    fn an_explicitly_empty_delimiter_is_left_alone() {
        let signature = Signature::new("Alice", "").expect("non-empty signature");
        assert_eq!(signature.delimiter(), "");
        assert_eq!(signature.block(), "Alice");
    }
}
