//! Link-target policy for opening URLs with the platform handler (ticket
//! hc9n). Email HTML is untrusted input: only schemes a browser owns are
//! handed to the OS opener, so `file://`, `javascript:`, and custom
//! application schemes can never launch a local handler.

/// Whether `url` is a link tmail may hand to the platform opener. Accepts
/// `http://` and `https://` (scheme case-insensitive); everything else —
/// including scheme-less and relative targets — is refused.
pub fn is_openable_url(url: &str) -> bool {
    let url = url.trim();
    url.get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || url
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_web_schemes_case_insensitively() {
        assert!(is_openable_url("http://example.org"));
        assert!(is_openable_url("https://example.org/x?y=1#z"));
        assert!(is_openable_url("HTTPS://EXAMPLE.ORG"));
        assert!(is_openable_url("  https://example.org  "));
    }

    #[test]
    fn refuses_everything_else() {
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "mailto:ada@example.org",
            "ftp://example.org",
            "example.org",
            "/relative/path",
            "https:/example.org",
            "",
            "http:",
        ] {
            assert!(!is_openable_url(url), "{url:?} must be refused");
        }
    }

    #[test]
    fn never_panics_on_multibyte_prefixes() {
        assert!(!is_openable_url("httpé://example.org"));
        assert!(!is_openable_url("日本語のリンク"));
    }
}
