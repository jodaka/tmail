//! Semantic theme tokens (plan §18). No literal colors outside this module.
//!
//! Values approximate the `mockups/list.html` oklch palette for a dark
//! terminal; they are RGB so the look does not depend on a 16-color palette
//! and degrade gracefully in `no-color` terminals.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Page background (mockup `--bg`).
    pub background: Color,
    /// Input wells, list rows, panels (`--surface`).
    pub surface: Color,
    /// Hover / secondary surface (`--surface-2`).
    pub surface2: Color,
    /// Hairlines and control borders (`--border`).
    pub border: Color,
    /// Primary text (`--fg`).
    pub text: Color,
    /// Soft primary text (`--fg-soft`).
    pub text_soft: Color,
    /// Secondary text (`--muted`).
    pub muted: Color,
    /// De-emphasized text (`--dim`).
    pub dim: Color,
    /// Interactive highlight (`--accent`).
    pub accent: Color,
    /// Selected row / active folder fill (`--accent-bg`).
    pub accent_bg: Color,
    /// Stars and warnings (`--amber`).
    pub warning: Color,
    /// Errors.
    pub error: Color,
    /// Focused-control selection fill.
    pub selection: Color,
    /// Modifier applied to unread text.
    pub unread: Modifier,
}

impl Theme {
    pub const fn default_dark() -> Self {
        Self {
            background: Color::Rgb(0x0F, 0x10, 0x14),
            surface: Color::Rgb(0x1D, 0x1F, 0x26),
            surface2: Color::Rgb(0x23, 0x25, 0x2C),
            border: Color::Rgb(0x3F, 0x43, 0x4E),
            text: Color::Rgb(0xEC, 0xEA, 0xE3),
            text_soft: Color::Rgb(0xC2, 0xC4, 0xCC),
            muted: Color::Rgb(0x9A, 0x9D, 0xA8),
            dim: Color::Rgb(0x8B, 0x8E, 0x99),
            accent: Color::Rgb(0x4E, 0x86, 0xDD),
            accent_bg: Color::Rgb(0x1C, 0x25, 0x34),
            warning: Color::Rgb(0xD4, 0xA4, 0x5C),
            error: Color::Rgb(0xD9, 0x5F, 0x51),
            selection: Color::Rgb(0x24, 0x30, 0x45),
            unread: Modifier::BOLD,
        }
    }

    /// Theme by `[post.theme].name` (plan §17/§18). Unknown names are
    /// rejected by config validation; here they fall back to the default
    /// so a stale file can never blank the UI.
    pub fn from_name(name: &str) -> Self {
        match name {
            "default" => Self::default_dark(),
            _ => Self::default_dark(),
        }
    }

    /// The no-color theme (plan §18: "support no-color behavior"): every
    /// token falls back to the terminal default and emphasis relies on
    /// modifiers only. Selected by a non-empty `NO_COLOR` environment
    /// variable, the de-facto standard (https://no-color.org).
    pub fn monochrome() -> Self {
        Self {
            background: Color::Reset,
            surface: Color::Reset,
            surface2: Color::Reset,
            border: Color::Reset,
            text: Color::Reset,
            text_soft: Color::Reset,
            muted: Color::Reset,
            dim: Color::Reset,
            accent: Color::Reset,
            accent_bg: Color::Reset,
            warning: Color::Reset,
            error: Color::Reset,
            selection: Color::Reset,
            unread: Modifier::empty(),
        }
    }

    /// Whether the environment asked for no color (`NO_COLOR` set to a
    /// non-empty value).
    pub fn no_color_requested() -> bool {
        std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty())
    }

    /// Style for a selected list row (mockup `.mail.selected`).
    pub fn row_selected(&self) -> Style {
        Style::new().bg(self.accent_bg)
    }

    /// Style for the sidebar's focused row (mockup `.folder:hover`).
    pub fn hover(&self) -> Style {
        Style::new().bg(self.surface2)
    }

    /// Style for unread from/subject (mockup `.mail.unread`).
    pub fn unread_text(&self) -> Style {
        Style::new().fg(self.text).add_modifier(self.unread)
    }

    pub fn read_text(&self) -> Style {
        Style::new().fg(self.text_soft)
    }

    pub fn star(&self) -> Style {
        Style::new().fg(self.warning)
    }

    pub fn hairline(&self) -> Style {
        Style::new().fg(self.border)
    }

    /// Mode badge in the status bar (mockup `.mode`): accent fill, dark text.
    pub fn mode_badge(&self) -> Style {
        Style::new()
            .bg(self.accent)
            .fg(self.background)
            .add_modifier(Modifier::BOLD)
    }

    /// Text on top of `background`.
    pub fn on_background(&self) -> Style {
        Style::new().fg(self.text).bg(self.background)
    }

    /// HTML headings (plan §13: heading → bold; stronger color than body).
    pub fn heading(&self) -> Style {
        Style::new()
            .fg(self.text)
            .bg(self.background)
            .add_modifier(Modifier::BOLD)
    }

    /// HTML links (plan §13: accent + underline; target rides the span).
    pub fn link(&self) -> Style {
        Style::new()
            .fg(self.accent)
            .bg(self.background)
            .add_modifier(Modifier::UNDERLINED)
    }

    /// `pre`/`code` runs (whitespace preservation is a layout property;
    /// the surface fill marks the code region).
    pub fn code(&self) -> Style {
        Style::new().fg(self.text_soft).bg(self.surface)
    }

    /// Blockquoted text (plan §13: dim; the `>` left marker is content).
    pub fn blockquote(&self) -> Style {
        Style::new().fg(self.dim).bg(self.background)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_distinct() {
        let t = Theme::default_dark();
        assert_ne!(t.background, t.surface);
        assert_ne!(t.surface, t.surface2);
        assert_ne!(t.accent, t.accent_bg);
        assert_ne!(t.text, t.muted);
        assert_ne!(t.muted, t.dim);
    }

    #[test]
    fn from_name_falls_back_to_the_reference_theme() {
        assert_eq!(Theme::from_name("default"), Theme::default_dark());
        assert_eq!(Theme::from_name("nope"), Theme::default_dark());
    }

    #[test]
    fn monochrome_uses_terminal_defaults() {
        let t = Theme::monochrome();
        assert_eq!(t.background, Color::Reset);
        assert_eq!(t.accent, Color::Reset);
        assert_eq!(t.unread, Modifier::empty());
        // Derived styles stay usable: no fg/bg means "default colors".
        let row = t.row_selected();
        assert_eq!(row.bg, Some(Color::Reset));
    }
}
