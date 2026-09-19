//! Semantic theme tokens (plan §18). No literal colors outside this module.
//!
//! The dark reference palette is the "Black & Gold Elegance" scheme
//! (#0d1017 #e5e5e5 #fca311 — derived from the trending Coolors palette,
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
    /// Sidebar/topbar/statusbar panel fill: a touch lighter than the page
    /// background so the chrome reads as one raised strip.
    pub sidebar_bg: Color,
    /// Hint labels and version text: dimmer than `dim`, barely-there copy
    /// that must stay legible on `sidebar_bg`.
    pub label_dim: Color,
    /// Secondary accent (the brand bullet): a supporting hue that must
    /// never compete with `accent` for attention.
    pub accent2: Color,
    /// Tertiary accent (the loader scanner): a second interactive hue for
    /// in-flight work.
    pub accent3: Color,
    /// Interactive highlight (`--accent`).
    pub accent: Color,
    /// Selected/active row fill (mockup `.mail.selected` background): the
    /// gold highlight behind the cursor row, the active folder, and the
    /// picker's cursor row.
    pub marker: Color,
    /// Left edge bar of the selected/active rows (mockup `.folder.active`):
    /// the `▎` marker column, drawn over the `marker` fill.
    pub marker_bar: Color,
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
            background: Color::Rgb(0x0D, 0x10, 0x17),
            surface: Color::Rgb(0x0E, 0x18, 0x2C),
            border: Color::Rgb(0x3A, 0x3F, 0x4A),
            text: Color::Rgb(0xE5, 0xE5, 0xE5),
            text_soft: Color::Rgb(0xBF, 0xBD, 0xB7),
            muted: Color::Rgb(0x92, 0x94, 0x99),
            dim: Color::Rgb(0x78, 0x7A, 0x82),
            snippet: Color::Rgb(0x5D, 0x61, 0x6A),
            sidebar_bg: Color::Rgb(0x15, 0x18, 0x20),
            label_dim: Color::Rgb(0x57, 0x5E, 0x71),
            accent2: Color::Rgb(0x83, 0xBD, 0x63),
            accent3: Color::Rgb(0x75, 0xC0, 0xF9),
            accent: Color::Rgb(0x75, 0xC0, 0xF9),
            marker: Color::Rgb(0xF4, 0xB7, 0x65),
            marker_bar: Color::Rgb(0xE5, 0xE4, 0xE4),
            accent_bg: Color::Rgb(0x14, 0x18, 0x21),
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
            "nord" => Self::default_nord(),
            _ => Self::default_dark(),
        }
    }

    /// Runtime-switchable theme list (ticket z0s4): the three built-ins —
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
            (
                String::from("nord"),
                with(
                    Theme::default_nord(),
                    if startup_name == "nord" {
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
            "sidebar_bg" => self.sidebar_bg = color,
            "label_dim" => self.label_dim = color,
            "accent2" => self.accent2 = color,
            "accent3" => self.accent3 = color,
            "accent" => self.accent = color,
            "marker" => self.marker = color,
            "marker_bar" => self.marker_bar = color,
            "accent_bg" => self.accent_bg = color,
            "bulk_selected_bg" => self.bulk_selected_bg = color,
            "warning" => self.warning = color,
            "error" => self.error = color,
            "selection" => self.selection = color,
            _ => return false,
        }
        true
    }

    /// The light variant (ticket wrs7): an authentic paper-editor palette
    /// — the editor background/foreground, line-number grays, and the
    /// selection blue carried straight from the source palette, with
    /// warning/error/status hues darkened just enough to stay readable on
    /// paper.
    pub fn default_light() -> Self {
        Self {
            background: Color::Rgb(0xF8, 0xF8, 0xF8),
            surface: Color::Rgb(0xEB, 0xED, 0xEF),
            border: Color::Rgb(0xC3, 0xC7, 0xCD),
            text: Color::Rgb(0x35, 0x35, 0x35),
            text_soft: Color::Rgb(0x53, 0x53, 0x53),
            muted: Color::Rgb(0x6B, 0x6E, 0x74),
            dim: Color::Rgb(0x8A, 0x8E, 0x96),
            snippet: Color::Rgb(0xBB, 0xBB, 0xBB),
            // The paper chrome: a slightly darker panel against the page.
            sidebar_bg: Color::Rgb(0xEC, 0xEE, 0xF0),
            label_dim: Color::Rgb(0xA9, 0xAE, 0xB6),
            accent2: Color::Rgb(0x10, 0xA5, 0x67),
            accent3: Color::Rgb(0x2F, 0x7F, 0xD0),
            accent: Color::Rgb(0x38, 0x6A, 0xC3),
            marker: Color::Rgb(0x38, 0x6A, 0xC3),
            marker_bar: Color::Rgb(0xFF, 0xFF, 0xFF),
            accent_bg: Color::Rgb(0xCA, 0xE9, 0xF9),
            bulk_selected_bg: Color::Rgb(0xF4, 0xDB, 0xBA),
            warning: Color::Rgb(0xA5, 0x82, 0x00),
            error: Color::Rgb(0xD0, 0x20, 0x00),
            selection: Color::Rgb(0xAB, 0xDF, 0xFA),
            unread: Modifier::BOLD,
        }
    }

    /// The Nord variant: the authentic Nord palette (nord0–nord13
    /// references) — the editor shade ladder carries the official colors
    /// unchanged, the aurora hues supply accents, and the nord13 yellow
    /// doubles as the selected-row fill with the dark nord0 over it as
    /// the marker bar.
    pub fn default_nord() -> Self {
        Self {
            background: Color::Rgb(0x2E, 0x34, 0x40),
            surface: Color::Rgb(0x3B, 0x42, 0x52),
            border: Color::Rgb(0x43, 0x4C, 0x5E),
            text: Color::Rgb(0xD8, 0xDE, 0xE9),
            text_soft: Color::Rgb(0x9C, 0xA6, 0xB8),
            muted: Color::Rgb(0x61, 0x6E, 0x88),
            dim: Color::Rgb(0x56, 0x62, 0x79),
            snippet: Color::Rgb(0x4C, 0x56, 0x6A),
            // The panel strip: between the page and the wells.
            sidebar_bg: Color::Rgb(0x33, 0x3B, 0x4A),
            label_dim: Color::Rgb(0x4B, 0x56, 0x6A),
            accent2: Color::Rgb(0xA3, 0xBE, 0x8C),
            accent3: Color::Rgb(0x81, 0xA1, 0xC1),
            accent: Color::Rgb(0x88, 0xC0, 0xD0),
            marker: Color::Rgb(0xEB, 0xCB, 0x8B),
            marker_bar: Color::Rgb(0x2E, 0x34, 0x40),
            accent_bg: Color::Rgb(0x43, 0x4C, 0x5E),
            bulk_selected_bg: Color::Rgb(0x5D, 0x5A, 0x53),
            warning: Color::Rgb(0xEB, 0xCB, 0x8B),
            error: Color::Rgb(0xBF, 0x61, 0x6A),
            selection: Color::Rgb(0x4C, 0x56, 0x6A),
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
            sidebar_bg: Color::Reset,
            label_dim: Color::Reset,
            accent2: Color::Reset,
            accent3: Color::Reset,
            accent: Color::Reset,
            marker: Color::Reset,
            marker_bar: Color::Reset,
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

    /// The attachment explorer's widget theme, built from the palette
    /// tokens. No block: the dialog draws its own chrome around the list.
    pub fn explorer_theme(&self) -> ratatui_explorer::Theme {
        ratatui_explorer::Theme::new()
            .with_style(Style::new().fg(self.text))
            .with_item_style(Style::new().fg(self.text))
            .with_dir_style(Style::new().fg(self.text_soft))
            .with_highlight_item_style(Style::new().fg(self.text).bg(self.accent_bg))
            .with_highlight_dir_style(Style::new().fg(self.text).bg(self.accent_bg))
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

    /// Style for a selected list row (mockup `.mail.selected`): the fill
    /// is the `marker` gold with page-background text for contrast (the
    /// mode-badge convention).
    pub fn row_selected(&self) -> Style {
        Style::new().fg(self.background).bg(self.marker)
    }

    /// Style for a bulk-selected row (ticket p0s3): visibly distinct from
    /// the cursor fill, so checked rows read as "marked", not "focused".
    /// The dedicated `bulk_selected_bg` token — overridable like every
    /// other token — instead of the sidebar `selection` fill.
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

    /// Mode badge in the status bar (mockup `.mode`): accent fill, dark
    /// text. Visually the focused-button look over the accent fill — it
    /// delegates, so the two can never drift apart.
    pub fn mode_badge(&self) -> Style {
        self.button_style(true, self.accent)
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

    /// The focused HTML link (ticket hc9n): the link style plus a reversed
    /// fill so the Tab cursor is unmistakable on every palette, including
    /// the monochrome theme where accent degrades to the terminal default.
    pub fn link_focused(&self) -> Style {
        self.link()
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
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
        let (list, index) = Theme::theme_list("nord", &[], &[]);
        assert_eq!(
            list.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["default", "light", "nord"]
        );
        assert_eq!(index, 2);
        assert_eq!(list[0].1, Theme::default_dark());
        assert_eq!(list[1].1, Theme::default_light());
        assert_eq!(list[2].1, Theme::default_nord());
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
            user("solar", &[("accent", "#b58900")]),
            user("warm", &[("background", "#262220")]),
        ];
        let (list, index) = Theme::theme_list("default", &[], &users);
        let names: Vec<_> = list.iter().map(|(n, _)| n.as_str()).collect();
        // List order follows the config parser's table iteration order
        // (alphabetical); these names are already sorted.
        assert_eq!(names, vec!["default", "light", "nord", "solar", "warm"]);
        assert_eq!(index, 0);
        // Unspecified tokens keep the dark reference values.
        assert_eq!(list[3].1.accent, Color::Rgb(0xb5, 0x89, 0x00));
        assert_eq!(list[4].1.background, Color::Rgb(0x26, 0x22, 0x20));
        // The built-in Nord stays untouched by them.
        assert_eq!(list[2].1, Theme::default_nord());
    }

    #[test]
    fn a_user_theme_shadowing_a_builtin_replaces_it_in_place() {
        // The nord user table replaces the built-in in its slot, and the
        // unspecified tokens still fall back to the dark reference.
        let users = vec![
            user("default", &[("background", "#101014")]),
            user("nord", &[("accent", "#aabbcc")]),
        ];
        let (list, index) = Theme::theme_list("default", &[], &users);
        let names: Vec<_> = list.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["default", "light", "nord"]);
        assert_eq!(index, 0, "the shadowed builtin stays the startup slot");
        assert_eq!(list[0].1.background, Color::Rgb(0x10, 0x10, 0x14));
        assert_eq!(list[0].1.accent, Theme::default_dark().accent);
        // Slot order is preserved: nord keeps its position in the list.
        assert_eq!(list[2].1.accent, Color::Rgb(0xaa, 0xbb, 0xcc));
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
        let by_index = |name: &str| colors[THEME_TOKENS.iter().position(|t| *t == name).unwrap()];
        assert_eq!(theme.background, by_index("background"));
        assert_eq!(theme.surface, by_index("surface"));
        assert_eq!(theme.border, by_index("border"));
        assert_eq!(theme.text, by_index("text"));
        assert_eq!(theme.text_soft, by_index("text_soft"));
        assert_eq!(theme.muted, by_index("muted"));
        assert_eq!(theme.dim, by_index("dim"));
        assert_eq!(theme.snippet, by_index("snippet"));
        assert_eq!(theme.sidebar_bg, by_index("sidebar_bg"));
        assert_eq!(theme.label_dim, by_index("label_dim"));
        assert_eq!(theme.accent2, by_index("accent2"));
        assert_eq!(theme.accent3, by_index("accent3"));
        assert_eq!(theme.accent, by_index("accent"));
        assert_eq!(theme.marker, by_index("marker"));
        assert_eq!(theme.marker_bar, by_index("marker_bar"));
        assert_eq!(theme.accent_bg, by_index("accent_bg"));
        assert_eq!(theme.bulk_selected_bg, by_index("bulk_selected_bg"));
        assert_eq!(theme.warning, by_index("warning"));
        assert_eq!(theme.error, by_index("error"));
        assert_eq!(theme.selection, by_index("selection"));
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
