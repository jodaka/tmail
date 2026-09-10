//! Semantic theme tokens (plan §18). No literal colors outside this module.
//!
//! The dark reference palette is the "Black & Gold Elegance" scheme
//! (#0a101e #e5e5e5 #fca311 — derived from the trending Coolors palette,
//! see `config.toml`'s theme derivations); they are RGB so the look does
//! not depend on a 16-color palette and degrade gracefully in `no-color`
//! terminals.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Page background (mockup `--bg`).
    pub background: Color,
    /// Input wells, list rows, panels (`--surface`).
    pub surface: Color,
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
    /// Faded message-body preview in the list rows (ticket wxtx): dimmer
    /// than `dim`, so the preview reads as context, never as content.
    pub snippet: Color,
    /// Interactive highlight (`--accent`).
    pub accent: Color,
    /// Selected row / active folder fill (`--accent-bg`).
    pub accent_bg: Color,
    /// Bulk-selected row highlight (ticket p0s3): visibly distinct from
    /// the cursor fill, so checked rows read as "marked", not "focused".
    pub bulk_selected_bg: Color,
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
            background: Color::Rgb(0x0A, 0x10, 0x1E),
            surface: Color::Rgb(0x0E, 0x18, 0x2C),
            border: Color::Rgb(0x3A, 0x3F, 0x4A),
            text: Color::Rgb(0xE5, 0xE5, 0xE5),
            text_soft: Color::Rgb(0xBE, 0xBF, 0xC1),
            muted: Color::Rgb(0x92, 0x94, 0x99),
            dim: Color::Rgb(0x78, 0x7A, 0x82),
            snippet: Color::Rgb(0x5D, 0x61, 0x6A),
            accent: Color::Rgb(0xFC, 0xA3, 0x11),
            accent_bg: Color::Rgb(0x2E, 0x26, 0x1C),
            bulk_selected_bg: Color::Rgb(0x46, 0x35, 0x1B),
            warning: Color::Rgb(0xFC, 0xA3, 0x11),
            error: Color::Rgb(0xE5, 0x48, 0x4D),
            selection: Color::Rgb(0x53, 0x3C, 0x1A),
            unread: Modifier::BOLD,
        }
    }

    /// Theme by `[tmail.theme].name` (plan §17/§18). Unknown names are
    /// rejected by config validation; here they fall back to the default
    /// so a stale file can never blank the UI.
    pub fn from_name(name: &str) -> Self {
        match name {
            "light" => Self::default_light(),
            _ => Self::default_dark(),
        }
    }

    /// Runtime-switchable theme list (ticket z0s4): the two built-ins —
    /// with the `[tmail.theme]` color overrides applied to the startup
    /// theme — plus every `[tmail.themes.<name>]` user theme built over
    /// the dark reference palette. A user theme shadowing a built-in name
    /// replaces it in place, so `[tmail.themes.default]` redefines the
    /// default. Returns the list in cycle order together with the index
    /// of the startup theme.
    pub fn theme_list(
        startup_name: &str,
        startup_overrides: &[(String, String)],
        user_themes: &[(String, Vec<(String, String)>)],
    ) -> (Vec<(String, Theme)>, usize) {
        let apply = |theme: &mut Theme, overrides: &[(String, String)]| {
            for (token, hex) in overrides {
                // Validation guarantees known tokens and valid hex; a
                // stale parse would only skip the override, never crash
                // startup (the same contract as the [tmail.theme] path).
                if let Some(color) =
                    crate::config::parse_hex_color(hex).and_then(|hex| Theme::color_from_hex(&hex))
                {
                    theme.set_token(token, color);
                }
            }
        };
        let with = |base: Theme, overrides: &[(String, String)]| {
            let mut theme = base;
            apply(&mut theme, overrides);
            theme
        };
        let mut list = vec![
            (
                String::from("default"),
                with(
                    Theme::default_dark(),
                    if startup_name == "default" {
                        startup_overrides
                    } else {
                        &[]
                    },
                ),
            ),
            (
                String::from("light"),
                with(
                    Theme::default_light(),
                    if startup_name == "light" {
                        startup_overrides
                    } else {
                        &[]
                    },
                ),
            ),
        ];
        for (name, overrides) in user_themes {
            let theme = with(Theme::default_dark(), overrides);
            match list.iter_mut().find(|(existing, _)| existing == name) {
                Some(slot) => slot.1 = theme,
                None => list.push((name.clone(), theme)),
            }
        }
        let index = list
            .iter()
            .position(|(name, _)| name == startup_name)
            .unwrap_or(0);
        (list, index)
    }

    /// Apply one `[tmail.theme]` color override (ticket wrs7). Unknown
    /// tokens are rejected by config validation; here they are ignored so
    /// a stale file can never blank the UI. Returns whether the token was
    /// known.
    pub fn set_token(&mut self, token: &str, color: Color) -> bool {
        match token {
            "background" => self.background = color,
            "surface" => self.surface = color,
            "border" => self.border = color,
            "text" => self.text = color,
            "text_soft" => self.text_soft = color,
            "muted" => self.muted = color,
            "dim" => self.dim = color,
            "snippet" => self.snippet = color,
            "accent" => self.accent = color,
            "accent_bg" => self.accent_bg = color,
            "bulk_selected_bg" => self.bulk_selected_bg = color,
            "warning" => self.warning = color,
            "error" => self.error = color,
            "selection" => self.selection = color,
            _ => return false,
        }
        true
    }

    /// The light variant (ticket wrs7): the reference palette inverted to
    /// a paper background, with darkened accent/warning/error hues so
    /// contrast stays readable.
    pub fn default_light() -> Self {
        Self {
            background: Color::Rgb(0xF6, 0xF5, 0xF1),
            surface: Color::Rgb(0xEC, 0xEB, 0xE5),
            border: Color::Rgb(0xC9, 0xC7, 0xBE),
            text: Color::Rgb(0x24, 0x26, 0x2E),
            text_soft: Color::Rgb(0x3E, 0x41, 0x4B),
            muted: Color::Rgb(0x5C, 0x5F, 0x6A),
            dim: Color::Rgb(0x74, 0x77, 0x82),
            snippet: Color::Rgb(0x94, 0x97, 0xA1),
            accent: Color::Rgb(0x2D, 0x63, 0xB8),
            accent_bg: Color::Rgb(0xDC, 0xE6, 0xF7),
            bulk_selected_bg: Color::Rgb(0xF7, 0xE8, 0xC8),
            warning: Color::Rgb(0x9A, 0x6B, 0x1A),
            error: Color::Rgb(0xB3, 0x36, 0x2A),
            selection: Color::Rgb(0xD8, 0xDF, 0xEE),
            unread: Modifier::BOLD,
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
            border: Color::Reset,
            text: Color::Reset,
            text_soft: Color::Reset,
            muted: Color::Reset,
            dim: Color::Reset,
            snippet: Color::Reset,
            accent: Color::Reset,
            accent_bg: Color::Reset,
            bulk_selected_bg: Color::Reset,
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

    /// Convert a normalized `#rrggbb` string (as validated and normalized
    /// by [`crate::config::parse_hex_color`]) into a color (ticket wrs7).
    pub fn color_from_hex(hex: &str) -> Option<Color> {
        let hex = hex.strip_prefix('#')?;
        if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let value = u32::from_str_radix(hex, 16).ok()?;
        Some(Color::Rgb(
            ((value >> 16) & 0xFF) as u8,
            ((value >> 8) & 0xFF) as u8,
            (value & 0xFF) as u8,
        ))
    }

    /// Style for a selected list row (mockup `.mail.selected`).
    pub fn row_selected(&self) -> Style {
        Style::new().bg(self.accent_bg)
    }

    /// Style for a bulk-selected row (ticket p0s3): the amber-tinted
    /// highlight fill. In the monochrome theme the terminal default stays,
    /// so a dim modifier marks the rows instead.
    pub fn row_bulk_selected(&self) -> Style {
        if self.bulk_selected_bg == Color::Reset {
            Style::new().add_modifier(Modifier::DIM)
        } else {
            Style::new().bg(self.bulk_selected_bg)
        }
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

    /// The bulk-selection checkbox (ticket cvc4): accent-colored so it
    /// reads as "marked" at a glance.
    pub fn accent_fg(&self) -> Style {
        Style::new().fg(self.accent)
    }

    pub fn hairline(&self) -> Style {
        Style::new().fg(self.border)
    }

    /// Focused/unfocused control fill: focused gets a full-color fill with
    /// background text and bold; unfocused stays quiet (`muted`). `fill`
    /// is the focused fill color — accent for normal buttons, error for
    /// destructive ones, warning for the composer's discard.
    pub fn button_style(&self, focused: bool, fill: Color) -> Style {
        if focused {
            Style::new()
                .fg(self.background)
                .bg(fill)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(self.muted)
        }
    }

    /// Mode badge in the status bar (mockup `.mode`): accent fill, dark text.
    pub fn mode_badge(&self) -> Style {
        Style::new()
            .bg(self.accent)
            .fg(self.background)
            .add_modifier(Modifier::BOLD)
    }

    /// The caret of a focused text input — the single-line field spans
    /// (ticket tz12) and the body editor's cursor cell draw the same
    /// block: accent fill with page-background text, the terminal
    /// stand-in for the mockup's accent `caret-color`.
    pub fn caret(&self) -> Style {
        Style::new()
            .fg(self.background)
            .bg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    /// Text on top of `background`.
    pub fn on_background(&self) -> Style {
        Style::new().fg(self.text).bg(self.background)
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
        assert_ne!(t.accent, t.accent_bg);
        assert_ne!(t.text, t.muted);
        assert_ne!(t.muted, t.dim);
        assert_ne!(t.dim, t.snippet);
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

#[cfg(test)]
mod theme_list_tests {
    use super::*;

    fn user(name: &str, tokens: &[(&str, &str)]) -> (String, Vec<(String, String)>) {
        (
            String::from(name),
            tokens
                .iter()
                .map(|(t, h)| (String::from(*t), String::from(*h)))
                .collect(),
        )
    }

    #[test]
    fn builtins_come_first_and_the_startup_theme_is_selected() {
        let (list, index) = Theme::theme_list("light", &[], &[]);
        assert_eq!(
            list.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["default", "light"]
        );
        assert_eq!(index, 1);
        assert_eq!(list[0].1, Theme::default_dark());
        assert_eq!(list[1].1, Theme::default_light());
    }

    #[test]
    fn startup_overrides_land_only_on_the_named_builtin() {
        let overrides = vec![(String::from("accent"), String::from("#aabbcc"))];
        let (list, _) = Theme::theme_list("light", &overrides, &[]);
        // The light base with the override; the dark default stays pure.
        assert_ne!(list[1].1.accent, Theme::default_light().accent);
        assert_eq!(list[1].1.accent, Color::Rgb(0xaa, 0xbb, 0xcc));
        assert_eq!(list[0].1, Theme::default_dark());
    }

    #[test]
    fn user_themes_append_over_the_dark_reference() {
        let users = vec![
            user("nord", &[("background", "#2e3440"), ("accent", "#88c0d0")]),
            user("solar", &[("accent", "#b58900")]),
        ];
        let (list, index) = Theme::theme_list("default", &[], &users);
        let names: Vec<_> = list.iter().map(|(n, _)| n.as_str()).collect();
        // List order follows the config parser's table iteration order
        // (alphabetical); these names are already sorted.
        assert_eq!(names, vec!["default", "light", "nord", "solar"]);
        assert_eq!(index, 0);
        // Unspecified tokens keep the dark reference values.
        assert_eq!(list[2].1.background, Color::Rgb(0x2e, 0x34, 0x40));
        assert_eq!(list[2].1.accent, Color::Rgb(0x88, 0xc0, 0xd0));
        assert_eq!(list[2].1.text, Theme::default_dark().text);
        assert_eq!(list[3].1.accent, Color::Rgb(0xb5, 0x89, 0x00));
    }

    #[test]
    fn a_user_theme_shadowing_a_builtin_replaces_it_in_place() {
        let users = vec![user("default", &[("background", "#101014")])];
        let (list, index) = Theme::theme_list("default", &[], &users);
        let names: Vec<_> = list.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["default", "light"]);
        assert_eq!(index, 0, "the shadowed builtin stays the startup slot");
        assert_eq!(list[0].1.background, Color::Rgb(0x10, 0x10, 0x14));
        assert_eq!(list[0].1.accent, Theme::default_dark().accent);
    }

    #[test]
    fn an_unknown_startup_name_selects_the_first_entry() {
        let (list, index) = Theme::theme_list("nope", &[], &[]);
        assert_eq!(index, 0);
        assert_eq!(list[0].1, Theme::default_dark());
    }
}

#[cfg(test)]
mod default_theme_doc_tests {
    use super::*;
    use crate::config::{THEME_TOKENS, parse_with_issues};

    /// `docs/default-theme.toml` documents the built-in dark theme as a
    /// ready-to-paste `[tmail.theme]` block (ticket dn04). This pins the
    /// file to the real palette: every token listed, no parse issues, and
    /// applying the block over the named theme reproduces
    /// `default_dark()` exactly — so the documentation cannot drift from
    /// what Tmail actually draws.
    #[test]
    fn the_documented_default_theme_matches_the_builtin() {
        let text = include_str!("../../docs/default-theme.toml");
        let (config, issues) = parse_with_issues(text, None);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(config.theme.name, "default");
        assert_eq!(
            config.theme.overrides.len(),
            THEME_TOKENS.len(),
            "every token documented: {issues:?}"
        );
        let mut theme = Theme::from_name(&config.theme.name);
        for (token, hex) in &config.theme.overrides {
            let color =
                Theme::color_from_hex(hex).unwrap_or_else(|| panic!("{token}: bad hex {hex:?}"));
            assert!(theme.set_token(token, color), "unknown token {token}");
        }
        assert_eq!(theme, Theme::default_dark());
    }
}

#[cfg(test)]
mod token_tests {
    use super::*;
    use crate::config::THEME_TOKENS;

    #[test]
    fn set_token_accepts_exactly_the_config_token_list() {
        // Every config token maps (no typos drift between config grammar
        // and the theme), and every set_token arm is listed.
        let base = Theme::default_dark();
        for token in THEME_TOKENS {
            let mut theme = base;
            assert!(
                theme.set_token(token, Color::Rgb(1, 2, 3)),
                "token {token:?} rejected"
            );
        }
        let mut base = base;
        assert!(!base.set_token("font", Color::Rgb(1, 2, 3)));
    }

    #[test]
    fn every_token_lands_on_its_own_field() {
        // Assign a distinct color per token and read each field back, so
        // a copy-paste slip in the set_token arms cannot hide.
        let mut theme = Theme::default_dark();
        let colors: Vec<_> = (0..THEME_TOKENS.len())
            .map(|i| Color::Rgb(i as u8 + 1, 0x80, 0x33))
            .collect();
        for (token, color) in THEME_TOKENS.iter().zip(&colors) {
            assert!(theme.set_token(token, *color));
        }
        assert_eq!(theme.background, colors[0]);
        assert_eq!(theme.surface, colors[1]);
        assert_eq!(theme.border, colors[2]);
        assert_eq!(theme.text, colors[3]);
        assert_eq!(theme.text_soft, colors[4]);
        assert_eq!(theme.muted, colors[5]);
        assert_eq!(theme.dim, colors[6]);
        assert_eq!(theme.snippet, colors[7]);
        assert_eq!(theme.accent, colors[8]);
        assert_eq!(theme.accent_bg, colors[9]);
        assert_eq!(theme.bulk_selected_bg, colors[10]);
        assert_eq!(theme.warning, colors[11]);
        assert_eq!(theme.error, colors[12]);
        assert_eq!(theme.selection, colors[13]);
    }

    #[test]
    fn color_from_hex_round_trips() {
        assert_eq!(
            Theme::color_from_hex("#4e86dd"),
            Some(Color::Rgb(0x4e, 0x86, 0xdd))
        );
        assert_eq!(Theme::color_from_hex("4e86dd"), None);
        assert_eq!(Theme::color_from_hex("#abc"), None, "needs normalization");
    }

    #[test]
    fn light_theme_differs_from_dark_and_is_selectable() {
        let dark = Theme::default_dark();
        let light = Theme::from_name("light");
        assert_eq!(light, Theme::default_light());
        assert_ne!(light.background, dark.background);
        assert_ne!(light.accent, dark.accent);
        // Text stays readable against the light page background: dark text.
        assert_ne!(light.text, light.background);
    }
}
