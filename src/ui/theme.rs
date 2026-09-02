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
}
