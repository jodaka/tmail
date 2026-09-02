//! Text helpers: width-aware, panic-free truncation (plan §13: never slice
//! strings at invalid byte boundaries; Unicode-safe everywhere).

use unicode_width::UnicodeWidthStr;

/// Truncate to `max_width` display columns, appending `…` when truncated.
pub fn truncate(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text.width() <= max_width {
        return text.to_string();
    }
    let ellipsis_w = 1;
    let budget = max_width.saturating_sub(ellipsis_w);
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.to_string().width();
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// Truncate without an ellipsis (hard clip at width).
pub fn clip(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text.width() <= max_width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.to_string().width();
        if used + w > max_width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

/// Pad/truncate to an exact display width (left-aligned).
pub fn fit_left(text: &str, width: usize) -> String {
    let t = clip(text, width);
    let pad = width.saturating_sub(t.width());
    format!("{t}{}", " ".repeat(pad))
}

/// Word-wrap `text` to `width` display columns, preserving explicit
/// newlines. Tokens wider than `width` (long URLs in error details) are
/// chunked across lines rather than truncated. Never panics on Unicode
/// input; empty input yields one empty line.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    for line in text.split('\n') {
        if line.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in line.split(' ').filter(|w| !w.is_empty()) {
            if word.width() > width {
                // Flush, then hard-chunk the oversized token.
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                let mut chunk = String::new();
                let mut used = 0;
                for ch in word.chars() {
                    let w = ch.to_string().width();
                    if used + w > width {
                        out.push(std::mem::take(&mut chunk));
                        used = 0;
                    }
                    chunk.push(ch);
                    used += w;
                }
                current = chunk;
                continue;
            }
            if current.is_empty() {
                current.push_str(word);
            } else if current.width() + 1 + word.width() <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                out.push(std::mem::take(&mut current));
                current.push_str(word);
            }
        }
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_ascii() {
        assert_eq!(truncate("hello", 4), "hel…");
        assert_eq!(truncate("hi", 5), "hi");
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn handles_unicode_without_panics() {
        // Wide-ish multibyte characters must never be split mid-codepoint.
        assert_eq!(
            truncate("Shirogami #2, 3.5 mm — customs", 18),
            "Shirogami #2, 3.5…"
        );
        assert_eq!(clip("°C × “quote”", 5), "°C × ");
        assert_eq!(fit_left("Re: WIP", 10), "Re: WIP   ");
    }

    #[test]
    fn zero_width_requests() {
        assert_eq!(truncate("abc", 0), "");
        assert_eq!(clip("abc", 0), "");
        assert_eq!(fit_left("abc", 0), "");
    }

    #[test]
    fn wraps_on_word_boundaries() {
        assert_eq!(wrap("alpha beta gamma", 11), vec!["alpha beta", "gamma"]);
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        // Explicit newlines survive.
        assert_eq!(wrap("a\nb", 10), vec!["a", "b"]);
        // A token wider than the line is hard-clipped, never split badly.
        assert_eq!(
            wrap("imaps://user:pass@host.example.com/path", 12),
            vec!["imaps://user", ":pass@host.e", "xample.com/p", "ath"]
        );
        assert_eq!(wrap("", 10), vec![String::new()]);
        assert_eq!(wrap("anything", 0), vec![String::new()]);
    }
}
