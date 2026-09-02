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
}
