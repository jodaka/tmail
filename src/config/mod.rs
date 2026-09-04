//! Post-owned configuration (plan §17).
//!
//! One canonical TOML file is shared with Himalaya (ADR 0001 finding 13:
//! himalaya 2.1.0 tolerates the unknown `[post]` root table). Post reads its
//! own `[post]` section plus the selected account's `mailbox.alias` table,
//! which the backend adapter uses to resolve mailbox roles (ADR 0001: the
//! UI never guesses folder names).
//!
//! Loading is forgiving in shape (an invalid value falls back to its
//! default so the app can still run) but never silent: every detected
//! problem — parse failure, missing account, invalid refresh/page/autosave
//! values, unusable editor command, invalid downloads path — is collected
//! as an actionable issue. `load_with_issues` reports them all together at
//! startup (plan §17/§19 Phase 10); issues never echo file content, and any
//! detail that could carry a secret is run through the sanitizer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::app::sanitize::sanitize;
use crate::domain::draft::DEFAULT_AUTOSAVE_DELAY_MS;

/// Default page size when the config does not provide a usable one
/// (plan §16/§17: explicit pagination, default 20).
/// Default `[post.mail].page_size` for manual pagination (ticket kjfq).
pub const DEFAULT_PAGE_SIZE: usize = 50;
/// Default `[post.mail].page_size_auto` (ticket kjfq): size pages to the
/// terminal so the whole page fits the list without scrolling.
pub const DEFAULT_PAGE_SIZE_AUTO: bool = true;

/// Default periodic refresh interval in seconds (plan §11/§19 Phase 9);
/// `[post.mail].refresh_interval_seconds = 0` disables the timer.
pub const DEFAULT_REFRESH_INTERVAL_SECONDS: u64 = 60;

/// Bounds of `[post.composer].autosave_delay_ms`: below the floor every
/// keystroke would race a save; above the ceiling the debounce is not a
/// debounce any more (plan §17: invalid autosave values are reported).
pub const AUTOSAVE_DELAY_MIN_MS: u64 = 100;
pub const AUTOSAVE_DELAY_MAX_MS: u64 = 600_000;

/// The theme names Post knows (plan §17/§18): the dark reference theme and
/// a light variant.
pub const THEME_NAMES: [&str; 2] = ["default", "light"];

/// The `[post.theme]` color tokens a user may override (ticket wrs7), as
/// hex strings like `"#4e86dd"`. Kept beside the config parser because the
/// token list is part of the file's grammar; [`crate::ui::theme::Theme`]
/// applies them (a test pins the two lists together).
pub const THEME_TOKENS: [&str; 14] = [
    "background",
    "surface",
    "border",
    "text",
    "text_soft",
    "muted",
    "dim",
    "snippet",
    "accent",
    "accent_bg",
    "bulk_selected_bg",
    "warning",
    "error",
    "selection",
];

/// Defaults for `[post.cache]` (ticket haeb): 50 viewed messages, 10 MiB
/// total. The page/mailbox caches are tiny and not user-limited.
pub const DEFAULT_CACHE_MAX_MESSAGES: usize = 50;
pub const DEFAULT_CACHE_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// Message-list density (`[post].view_mode`), Gmail-style. `Compact` is
/// the reference one-line-per-message list; `Comfortable` interleaves a
/// faint horizontal separator under every row, so each message costs two
/// terminal lines: fewer messages fit on screen and the list gains
/// negative space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Compact,
    Comfortable,
}

impl ViewMode {
    /// Parse a `[post].view_mode` value; `None` when the name is unknown
    /// (the caller reports the problem and falls back to the default).
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "compact" => Some(Self::Compact),
            "comfortable" => Some(Self::Comfortable),
            _ => None,
        }
    }

    /// Terminal lines each message occupies in the list: one content line
    /// in compact, content plus a separator line in comfortable.
    pub fn row_height(self) -> usize {
        match self {
            Self::Compact => 1,
            Self::Comfortable => 2,
        }
    }
}

/// Parse a config-file color: `#rgb` or `#rrggbb` (case-insensitive hex).
/// Returns the normalized `#rrggbb` form, or `None` when the value is not
/// a color Post can use.
pub fn parse_hex_color(value: &str) -> Option<String> {
    let hex = value.strip_prefix('#')?;
    if hex.is_empty() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        3 => {
            let doubled: String = hex
                .chars()
                .flat_map(|c| {
                    let low = c.to_ascii_lowercase();
                    [low, low]
                })
                .collect();
            Some(format!("#{doubled}"))
        }
        6 => Some(format!("#{}", hex.to_ascii_lowercase())),
        _ => None,
    }
}

/// A loaded configuration plus every problem found while reading it, in
/// file order. Issues are user-facing, actionable, and secret-free.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoadIssues {
    pub items: Vec<String>,
}

impl LoadIssues {
    fn push(&mut self, message: impl Into<String>) {
        self.items.push(message.into());
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, String> {
        self.items.iter()
    }
}

/// Resolved, validated configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Config file forwarded to himalaya with `-c`, when one was resolved.
    pub path: Option<PathBuf>,
    /// `[post].account`: the himalaya account Post drives. `None` lets
    /// himalaya pick its default account (and disables alias-based role
    /// resolution, which is per-account).
    pub account: Option<String>,
    /// `[post.mail].page_size`, defaulting to [`DEFAULT_PAGE_SIZE`]; a
    /// non-positive or absent value falls back to the default. Ignored
    /// while [`Config::page_size_auto`] is on.
    pub page_size: usize,
    /// `[post.mail].page_size_auto` (ticket kjfq): size each page to the
    /// number of message rows the terminal can show, so the page fits the
    /// list without scrolling. On by default; overrides `page_size`.
    pub page_size_auto: bool,
    /// `[post.mail].refresh_interval_seconds` (plan §11/§19 Phase 9):
    /// the periodic background-refresh interval; `0` disables the timer.
    /// Absent defaults to 60; a negative value disables (treated as 0).
    pub refresh_interval_seconds: u64,
    /// `[accounts.<account>.mailbox.alias]` entries: role key → mailbox
    /// name. Non-string values are ignored.
    pub aliases: HashMap<String, String>,
    /// `[accounts.<account>].email`: the configured account address, used
    /// as the `From` identity of drafts (Phase 6).
    pub account_email: Option<String>,
    /// `[accounts.<account>].display-name`, when configured.
    pub account_display_name: Option<String>,
    /// `[post.attachments].downloads_dir` (plan §17), as written — a
    /// leading `~` is expanded by the backend when the directory is used.
    /// `None` falls back to the platform default (`$HOME/Downloads`).
    pub downloads_dir: Option<PathBuf>,
    /// `[post].mouse` (plan §10): enable mouse capture and click/wheel
    /// translation. Off by default: capture changes what terminal text
    /// selection does, so it stays opt-in.
    pub mouse: bool,
    /// `[post.ui].clock` (ticket w7f5): show the top-right date/time
    /// clock. Off by default.
    pub ui_clock: bool,
    /// `[post].view_mode` list density (Gmail-style): `compact` (default)
    /// or `comfortable`. Comfortable draws a faint horizontal separator
    /// under every message row, doubling the row height.
    pub view_mode: ViewMode,
    /// `[post].status_timeout` (ticket h1d7): seconds a status message
    /// stays up before it fades out and clears. `0` (the default) keeps a
    /// message until the next one replaces it.
    pub status_timeout: u64,
    /// `[post.cache].max_messages` (ticket haeb): maximum number of cached
    /// viewed messages (LRU-evicted). `0` disables message caching.
    pub cache_max_messages: usize,
    /// `[post.cache].max_bytes` (ticket haeb): total size cap in bytes for
    /// the viewed-message cache.
    pub cache_max_bytes: u64,
    /// `[post.composer].editor` (plan §14/§17): `"builtin"`, `"$EDITOR"`,
    /// or an explicit command. The external-editor flow itself is Phase 11;
    /// v1 validates the value so a broken entry is reported up front.
    pub editor: String,
    /// `[post.composer].editor` resolved into an argv (Phase 11.4):
    /// `None` is the builtin editor; `Some` is program + arguments,
    /// spawned directly, never a shell. Resolution: `"builtin"` → `None`;
    /// `"$EDITOR"` → the environment value split on whitespace; anything
    /// else is the value itself split on whitespace.
    pub editor_command: Option<Vec<String>>,
    /// `[post.composer].autosave_delay_ms` (plan §14): the draft autosave
    /// debounce for the builtin editor.
    pub autosave_delay_ms: u64,
    /// `[post.theme].name` (plan §17/§18); see [`THEME_NAMES`].
    pub theme_name: String,
    /// `[post.theme]` color overrides (ticket wrs7): `(token, "#rrggbb")`
    /// pairs, validated at parse time and applied over the named theme in
    /// file order.
    pub theme_overrides: Vec<(String, String)>,
    /// `[post.themes.<name>]` user themes (ticket z0s4): additional named
    /// palettes for runtime switching, each a `(token, "#rrggbb")` table
    /// applied over the dark reference palette. Cycle order is the
    /// parser's table iteration order (alphabetical by name); a theme
    /// shadowing a built-in name replaces it.
    pub theme_tables: Vec<(String, Vec<(String, String)>)>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            path: None,
            account: None,
            page_size: DEFAULT_PAGE_SIZE,
            page_size_auto: DEFAULT_PAGE_SIZE_AUTO,
            refresh_interval_seconds: DEFAULT_REFRESH_INTERVAL_SECONDS,
            aliases: HashMap::new(),
            account_email: None,
            account_display_name: None,
            downloads_dir: None,
            mouse: false,
            ui_clock: false,
            view_mode: ViewMode::Compact,
            status_timeout: 0,
            cache_max_messages: DEFAULT_CACHE_MAX_MESSAGES,
            cache_max_bytes: DEFAULT_CACHE_MAX_BYTES,
            editor: String::from("builtin"),
            editor_command: None,
            autosave_delay_ms: DEFAULT_AUTOSAVE_DELAY_MS,
            theme_name: String::from("default"),
            theme_overrides: Vec::new(),
            theme_tables: Vec::new(),
        }
    }
}

impl Config {
    /// Load the configuration: the CLI path wins, then `POST_CONFIG`, then
    /// the well-known himalaya config locations. A file that exists but
    /// cannot be parsed still pins `path` (himalaya must receive `-c` and
    /// will report the real problem) while Post itself runs on defaults —
    /// and the parse failure is reported as an issue.
    pub fn load_with_issues(cli_path: Option<&Path>) -> (Self, LoadIssues) {
        let path = resolve_path(cli_path);
        let Some(path) = path else {
            return (Config::default(), LoadIssues::default());
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => parse_with_issues(&text, Some(path)),
            Err(err) => {
                let mut issues = LoadIssues::default();
                issues.push(format!(
                    "config file {} could not be read: {err}",
                    path.display()
                ));
                (
                    Config {
                        path: Some(path),
                        ..Config::default()
                    },
                    issues,
                )
            }
        }
    }

    /// Load with the startup issues discarded (tests, the probe binary).
    pub fn load(cli_path: Option<&Path>) -> Self {
        Self::load_with_issues(cli_path).0
    }
}

/// Resolve the config file path. An explicitly provided path (CLI arg or
/// `POST_CONFIG`) is used even when missing so the real error surfaces; the
/// well-known defaults are only used when they exist.
fn resolve_path(cli_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = cli_path {
        return Some(path.to_path_buf());
    }
    if let Some(path) = std::env::var_os("POST_CONFIG") {
        return Some(PathBuf::from(path));
    }
    default_candidates().into_iter().find(|p| p.exists())
}

/// Well-known himalaya config locations, in preference order. Only paths
/// that exist are candidates.
fn default_candidates() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    vec![
        home.join(".config/himalaya/config.toml"),
        home.join("Library/Application Support/himalaya/config.toml"),
    ]
}

/// Parse the shared TOML into a [`Config`], tolerating any Himalaya content.
/// Unknown tables are skipped; a malformed file yields defaults plus the
/// path (never a hard failure in Phase 2).
pub fn parse(text: &str, path: Option<PathBuf>) -> Config {
    parse_with_issues(text, path).0
}

/// Parse the shared TOML, collecting every detected problem. An invalid
/// value still falls back to its default so the rest of the file is
/// honored; the issue list is what startup reports (plan §17: all issues
/// together, sanitized).
pub fn parse_with_issues(text: &str, path: Option<PathBuf>) -> (Config, LoadIssues) {
    let mut config = Config {
        path,
        ..Config::default()
    };
    let mut issues = LoadIssues::default();
    let doc = match toml::from_str::<toml::Value>(text) {
        Ok(doc) => doc,
        Err(err) => {
            // The error text can quote a fragment of the offending line;
            // run it through the sanitizer so a secret can never surface.
            issues.push(format!(
                "config file is not valid TOML: {}",
                sanitize(&err.to_string())
            ));
            return (config, issues);
        }
    };
    let post = doc.get("post");

    if let Some(account) = post
        .and_then(|post| post.get("account"))
        .and_then(toml::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
    {
        config.account = Some(account);
    }
    config.mouse = post
        .and_then(|post| post.get("mouse"))
        .map(|value| match value.as_bool() {
            Some(mouse) => mouse,
            None => {
                issues.push(String::from(
                    "[post].mouse must be true or false; using false",
                ));
                false
            }
        })
        .unwrap_or(false);
    parse_page_size(post, &mut config, &mut issues);
    parse_page_size_auto(post, &mut config, &mut issues);
    parse_refresh_interval(post, &mut config, &mut issues);
    parse_autosave_delay(post, &mut config, &mut issues);
    parse_editor(post, &mut config, &mut issues);
    parse_theme(post, &mut config, &mut issues);
    parse_theme_tables(post, &mut config, &mut issues);
    parse_downloads_dir(post, &mut config, &mut issues);
    parse_ui_clock(post, &mut config, &mut issues);
    parse_view_mode(post, &mut config, &mut issues);
    parse_status_timeout(post, &mut config, &mut issues);
    parse_cache_limits(post, &mut config, &mut issues);

    // Without `[post].account`, drive the account himalaya itself would
    // pick (no `-a` is forwarded): the one marked `default = true`, else
    // the sole account. The alias table is per-account, so role resolution
    // stays dark until this matches (real-config finding: a `default =
    // true` Gmail account with mailbox aliases resolved nothing).
    if config.account.is_none() {
        config.account = default_account(&doc);
    }
    if let Some(account) = config.account.as_deref() {
        let exists = doc
            .get("accounts")
            .and_then(|accounts| accounts.get(account))
            .is_some();
        if !exists {
            issues.push(format!(
                "[post].account selects {account:?} but [accounts.{account}] does not exist"
            ));
        }
        config.aliases = aliases_for(&doc, account);
        config.account_email = account_field(&doc, account, "email");
        config.account_display_name = account_field(&doc, account, "display-name");
    }
    (config, issues)
}

fn parse_page_size(post: Option<&toml::Value>, config: &mut Config, issues: &mut LoadIssues) {
    match post
        .and_then(|post| post.get("mail"))
        .and_then(|mail| mail.get("page_size"))
    {
        None => {}
        Some(value) => match value.as_integer() {
            Some(size) if size > 0 => config.page_size = size as usize,
            Some(size) => issues.push(format!(
                "[post.mail].page_size must be a positive integer, not {size}; using {}",
                DEFAULT_PAGE_SIZE
            )),
            None => issues.push(format!(
                "[post.mail].page_size must be an integer; using {DEFAULT_PAGE_SIZE}"
            )),
        },
    }
}

/// `[post.mail].page_size_auto` (ticket kjfq): a boolean; anything else
/// is reported and the default (true) applies.
fn parse_page_size_auto(post: Option<&toml::Value>, config: &mut Config, issues: &mut LoadIssues) {
    match post
        .and_then(|post| post.get("mail"))
        .and_then(|mail| mail.get("page_size_auto"))
    {
        None => {}
        Some(value) => match value.as_bool() {
            Some(auto) => config.page_size_auto = auto,
            None => issues.push(String::from(
                "[post.mail].page_size_auto must be true or false; using true",
            )),
        },
    }
}

fn parse_refresh_interval(
    post: Option<&toml::Value>,
    config: &mut Config,
    issues: &mut LoadIssues,
) {
    let Some(value) = post
        .and_then(|post| post.get("mail"))
        .and_then(|mail| mail.get("refresh_interval_seconds"))
    else {
        return;
    };
    match value.as_integer() {
        // Explicit 0 disables the timer; a negative value is nonsense and
        // must never become a busy loop (plan §17: invalid values report).
        Some(seconds) if seconds >= 0 => config.refresh_interval_seconds = seconds as u64,
        Some(seconds) => {
            issues.push(format!(
                "[post.mail].refresh_interval_seconds must be ≥ 0, not {seconds}; using 0 (disabled)"
            ));
            config.refresh_interval_seconds = 0;
        }
        None => issues.push(format!(
            "[post.mail].refresh_interval_seconds must be an integer; using {DEFAULT_REFRESH_INTERVAL_SECONDS}"
        )),
    }
}

fn parse_autosave_delay(post: Option<&toml::Value>, config: &mut Config, issues: &mut LoadIssues) {
    let Some(value) = post
        .and_then(|post| post.get("composer"))
        .and_then(|composer| composer.get("autosave_delay_ms"))
    else {
        return;
    };
    let fallback = format!("; using {DEFAULT_AUTOSAVE_DELAY_MS}");
    match value.as_integer() {
        Some(delay)
            if (AUTOSAVE_DELAY_MIN_MS as i64..=AUTOSAVE_DELAY_MAX_MS as i64).contains(&delay) =>
        {
            config.autosave_delay_ms = delay as u64;
        }
        Some(delay) => issues.push(format!(
            "[post.composer].autosave_delay_ms must be between {AUTOSAVE_DELAY_MIN_MS} and {AUTOSAVE_DELAY_MAX_MS} ms, not {delay}{fallback}"
        )),
        None => issues.push(format!(
            "[post.composer].autosave_delay_ms must be an integer{fallback}"
        )),
    }
}

/// `[post.cache]` (ticket haeb): limits for the viewed-message cache.
/// `max_messages = 0` disables message caching; the summary/page cache
/// itself is tiny and always on.
fn parse_cache_limits(post: Option<&toml::Value>, config: &mut Config, issues: &mut LoadIssues) {
    let Some(table) = post
        .and_then(|post| post.get("cache"))
        .and_then(|c| c.as_table())
    else {
        return;
    };
    for (key, value) in table {
        match key.as_str() {
            "max_messages" => match value.as_integer() {
                Some(count) if count >= 0 => config.cache_max_messages = count as usize,
                _ => issues.push(String::from(
                    "[post.cache].max_messages must be a non-negative integer",
                )),
            },
            "max_bytes" => match value.as_integer() {
                Some(bytes) if bytes >= 0 => config.cache_max_bytes = bytes as u64,
                _ => issues.push(String::from(
                    "[post.cache].max_bytes must be a non-negative integer",
                )),
            },
            other => issues.push(format!(
                "[post.cache].{other} is unknown (known: max_messages, max_bytes)"
            )),
        }
    }
}

/// `[post.ui].clock` (ticket w7f5): show the top-right date/time clock.
/// Off by default.
fn parse_ui_clock(post: Option<&toml::Value>, config: &mut Config, issues: &mut LoadIssues) {
    let Some(value) = post
        .and_then(|post| post.get("ui"))
        .and_then(|ui| ui.get("clock"))
    else {
        return;
    };
    config.ui_clock = match value.as_bool() {
        Some(clock) => clock,
        None => {
            issues.push(String::from(
                "[post.ui].clock must be true or false; using false",
            ));
            false
        }
    };
}

/// `[post].view_mode` (Gmail-style list density): `"compact"` (default)
/// or `"comfortable"`. Anything else is reported and the default applies.
fn parse_view_mode(post: Option<&toml::Value>, config: &mut Config, issues: &mut LoadIssues) {
    let Some(value) = post.and_then(|post| post.get("view_mode")) else {
        return;
    };
    config.view_mode = match value.as_str().map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => match ViewMode::parse(name) {
            Some(mode) => mode,
            None => {
                issues.push(format!(
                    "[post].view_mode {name:?} is unknown (known: compact, comfortable); using \"compact\""
                ));
                ViewMode::Compact
            }
        },
        None => {
            issues.push(String::from(
                "[post].view_mode must be \"compact\" or \"comfortable\"; using \"compact\"",
            ));
            ViewMode::Compact
        }
    };
}

/// `[post.themes.<name>]` (ticket z0s4): additional named themes for
/// runtime switching with `t`. Each table holds the same color tokens as
/// `[post.theme]` (minus `name`, which the table's key already carries);
/// values are validated and normalized exactly like `[post.theme]`
/// colors, unknown tokens are reported, and the cycle order is the
/// parser's table iteration order (alphabetical by name).
fn parse_theme_tables(post: Option<&toml::Value>, config: &mut Config, issues: &mut LoadIssues) {
    let Some(themes) = post
        .and_then(|post| post.get("themes"))
        .and_then(|themes| themes.as_table())
    else {
        return;
    };
    for (name, value) in themes {
        let Some(table) = value.as_table() else {
            issues.push(format!("[post.themes.{name}] must be a table"));
            continue;
        };
        if name.is_empty() {
            issues.push(String::from("[post.themes] names must not be empty"));
            continue;
        }
        let mut overrides = Vec::new();
        for (key, value) in table {
            if !THEME_TOKENS.contains(&key.as_str()) {
                issues.push(format!(
                    "[post.themes.{name}].{key} is unknown (known tokens: {})",
                    THEME_TOKENS.join(", ")
                ));
                continue;
            }
            match value.as_str() {
                Some(hex) => match parse_hex_color(hex) {
                    Some(normalized) => overrides.push((key.clone(), normalized)),
                    None => issues.push(format!(
                        "[post.themes.{name}].{key} must be a hex color like \"#4e86dd\""
                    )),
                },
                None => issues.push(format!(
                    "[post.themes.{name}].{key} must be a hex color like \"#4e86dd\""
                )),
            }
        }
        config.theme_tables.push((name.clone(), overrides));
    }
}

/// `[post].status_timeout` (ticket h1d7): seconds a status message stays
/// up before it fades out and clears; `0` (the default) keeps a message
/// until the next one replaces it. A negative value is nonsense and must
/// never become a busy timer (plan §17: invalid values report).
fn parse_status_timeout(post: Option<&toml::Value>, config: &mut Config, issues: &mut LoadIssues) {
    let Some(value) = post.and_then(|post| post.get("status_timeout")) else {
        return;
    };
    match value.as_integer() {
        Some(seconds) if seconds >= 0 => config.status_timeout = seconds as u64,
        Some(seconds) => issues.push(format!(
            "[post].status_timeout must be ≥ 0, not {seconds}; using 0 (disabled)"
        )),
        None => issues.push(String::from(
            "[post].status_timeout must be an integer; using 0 (disabled)",
        )),
    }
}

/// `[post.composer].editor`: `"builtin"`, `"$EDITOR"`, or an explicit
/// command (program + arguments, resolved without a shell — plan §14).
fn parse_editor(post: Option<&toml::Value>, config: &mut Config, issues: &mut LoadIssues) {
    let Some(value) = post
        .and_then(|post| post.get("composer"))
        .and_then(|composer| composer.get("editor"))
    else {
        return;
    };
    let Some(editor) = value.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        issues.push(String::from(
            "[post.composer].editor must be a non-empty string; using \"builtin\"",
        ));
        return;
    };
    match validate_editor(editor) {
        Ok(()) => {
            config.editor = editor.to_owned();
            config.editor_command = resolve_editor_command(editor);
        }
        Err(problem) => {
            issues.push(format!(
                "[post.composer].editor: {problem}; using \"builtin\""
            ));
        }
    }
}

/// Resolve an editor value into an argv (Phase 11.4): `"builtin"` is the
/// builtin editor (`None`); `"$EDITOR"` resolves from the environment;
/// anything else is program + arguments split on whitespace, spawned
/// directly — never a shell (plan §14 step 4).
pub fn resolve_editor_command(editor: &str) -> Option<Vec<String>> {
    match editor {
        "builtin" => None,
        "$EDITOR" => std::env::var_os("EDITOR").map(|value| {
            value
                .to_string_lossy()
                .split_whitespace()
                .map(str::to_owned)
                .collect()
        }),
        _ => Some(editor.split_whitespace().map(str::to_owned).collect()),
    }
}

fn validate_editor(editor: &str) -> Result<(), String> {
    validate_editor_with(
        editor,
        std::env::var("EDITOR")
            .ok()
            .filter(|value| !value.is_empty()),
    )
}

fn validate_editor_with(editor: &str, editor_env: Option<String>) -> Result<(), String> {
    if editor == "builtin" {
        return Ok(());
    }
    if editor == "$EDITOR" {
        return editor_env
            .map(|_| ())
            .ok_or_else(|| String::from("$EDITOR is not set in the environment"));
    }
    // Explicit command: Post never spawns a shell, so metacharacters have
    // no meaning and only invite confusion (plan §14 step 4).
    if editor.contains(['|', '&', ';', '<', '>', '`', '$', '\\', '"', '\'']) {
        return Err(
            "must be \"builtin\", \"$EDITOR\", or a plain command without shell metacharacters"
                .into(),
        );
    }
    let Some(program) = editor.split_whitespace().next() else {
        return Err(String::from("is empty"));
    };
    if program_exists(program) {
        Ok(())
    } else if program.contains('/') {
        Err(format!("program {program:?} does not exist"))
    } else {
        Err(format!("program {program:?} was not found on PATH"))
    }
}

/// Whether `program` resolves as an executable: a direct path (anything
/// with a separator) must exist; otherwise each `PATH` entry is searched.
pub fn program_exists(program: &str) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(program))
                .any(|candidate| candidate.is_file())
        })
        .unwrap_or(false)
}

fn parse_theme(post: Option<&toml::Value>, config: &mut Config, issues: &mut LoadIssues) {
    let Some(theme) = post.and_then(|post| post.get("theme")) else {
        return;
    };
    let Some(table) = theme.as_table() else {
        issues.push(String::from("[post.theme] must be a table"));
        return;
    };
    // The table walk is deterministic; a token written twice applies its
    // last occurrence (map iteration order), matching how a duplicate key
    // reads in the file.
    for (key, value) in table {
        match key.as_str() {
            "name" => match value.as_str() {
                Some(name) if THEME_NAMES.contains(&name) => config.theme_name = name.to_owned(),
                Some(name) => issues.push(format!(
                    "[post.theme].name {name:?} is unknown (known: {})",
                    THEME_NAMES.join(", ")
                )),
                None => issues.push(String::from("[post.theme].name must be a string")),
            },
            token if THEME_TOKENS.contains(&token) => match value.as_str() {
                Some(hex) => match parse_hex_color(hex) {
                    Some(normalized) => config
                        .theme_overrides
                        .push((String::from(token), normalized)),
                    None => issues.push(format!(
                        "[post.theme].{token} must be a hex color like \"#4e86dd\""
                    )),
                },
                None => issues.push(format!(
                    "[post.theme].{token} must be a hex color like \"#4e86dd\""
                )),
            },
            other => issues.push(format!(
                "[post.theme].{other} is unknown (known tokens: {})",
                THEME_TOKENS.join(", ")
            )),
        }
    }
}

fn parse_downloads_dir(post: Option<&toml::Value>, config: &mut Config, issues: &mut LoadIssues) {
    let Some(value) = post
        .and_then(|post| post.get("attachments"))
        .and_then(|attachments| attachments.get("downloads_dir"))
        .and_then(toml::Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let dir = PathBuf::from(value);
    match validate_downloads_dir(&dir) {
        Ok(()) => config.downloads_dir = Some(dir),
        Err(problem) => issues.push(format!("[post.attachments].downloads_dir: {problem}")),
    }
}

/// `~`-aware sanity checks on the downloads directory (plan §17): it must
/// be absolute (or `~/…`) and, when it already exists, a directory. A
/// missing directory is fine — the save path creates it on demand.
fn validate_downloads_dir(dir: &Path) -> Result<(), String> {
    let text = dir.to_string_lossy();
    if !text.starts_with('/') && !text.starts_with('~') {
        return Err(format!(
            "{value:?} must be an absolute path or start with ~/ (expanded in Post, never via a shell)",
            value = text
        ));
    }
    let Some(expanded) = expand_home(dir) else {
        return Err(String::from("uses ~ but $HOME is not set"));
    };
    if expanded.is_file() {
        return Err(format!(
            "{} exists and is a file, not a directory",
            expanded.display()
        ));
    }
    Ok(())
}

/// Expand a leading `~` with `$HOME` (plan §15: expansion in Post, never
/// through a shell). `None` when `~` is used but `$HOME` is missing.
fn expand_home(path: &Path) -> Option<PathBuf> {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix("~/") {
        return std::env::var_os("HOME").map(|home| PathBuf::from(home).join(rest));
    }
    if text == "~" {
        return std::env::var_os("HOME").map(PathBuf::from);
    }
    Some(path.to_path_buf())
}

/// The account himalaya would pick without an explicit selection: the
/// `[accounts]` entry marked `default = true`, else the sole account when
/// the table holds exactly one. `None` (several accounts, no default)
/// leaves role resolution off rather than guessing.
fn default_account(doc: &toml::Value) -> Option<String> {
    let accounts = doc.get("accounts")?.as_table()?;
    if let Some((name, _)) = accounts
        .iter()
        .find(|(_, account)| account.get("default").and_then(toml::Value::as_bool) == Some(true))
    {
        return Some(name.clone());
    }
    if accounts.len() == 1 {
        return accounts.keys().next().cloned();
    }
    None
}

/// One string field of `[accounts.<account>]`.
fn account_field(doc: &toml::Value, account: &str, field: &str) -> Option<String> {
    doc.get("accounts")
        .and_then(|accounts| accounts.get(account))
        .and_then(|account| account.get(field))
        .and_then(toml::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// `[accounts.<account>.mailbox.alias]` entries, keeping only string values.
fn aliases_for(doc: &toml::Value, account: &str) -> HashMap<String, String> {
    doc.get("accounts")
        .and_then(|accounts| accounts.get(account))
        .and_then(|account| account.get("mailbox"))
        .and_then(|mailbox| mailbox.get("alias"))
        .and_then(toml::Value::as_table)
        .map(|table| {
            table
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_post_section_and_aliases() {
        let text = r#"
            [accounts.probe]
            email = "probe@post.local"

            [accounts.probe.maildir]
            root = "~/tmp/post-probe-maildir"

            [accounts.probe.mailbox.alias]
            inbox = "INBOX"
            trash = "Archive"

            [post]
            account = "probe"

            [post.mail]
            page_size = 7
        "#;
        let config = parse(text, None);
        assert_eq!(config.account.as_deref(), Some("probe"));
        assert_eq!(config.page_size, 7);
        assert_eq!(
            config.aliases.get("inbox").map(String::as_str),
            Some("INBOX")
        );
        assert_eq!(
            config.aliases.get("trash").map(String::as_str),
            Some("Archive")
        );
        assert_eq!(config.aliases.len(), 2);
    }

    #[test]
    fn minimal_file_yields_defaults() {
        let config = parse("", None);
        assert_eq!(config, Config::default());
        assert_eq!(config.page_size, DEFAULT_PAGE_SIZE);
    }

    #[test]
    fn malformed_file_yields_defaults() {
        let config = parse("not [ valid toml", None);
        assert_eq!(config.account, None);
        assert_eq!(config.page_size, DEFAULT_PAGE_SIZE);
        assert!(config.aliases.is_empty());
    }

    #[test]
    fn nonpositive_page_size_falls_back_to_default() {
        let text = "[post.mail]\npage_size = 0\n";
        assert_eq!(parse(text, None).page_size, DEFAULT_PAGE_SIZE);
    }

    #[test]
    fn page_size_auto_defaults_on_and_parses_both_ways() {
        // Ticket kjfq: auto-sized pages are the default; `page_size` is
        // only honored when the auto mode is switched off.
        let text = "[post.mail]\npage_size = 7\n";
        let config = parse(text, None);
        assert!(config.page_size_auto);
        assert_eq!(config.page_size, 7, "parsed even while ignored");

        let config = parse("[post.mail]\npage_size_auto = false\n", None);
        assert!(!config.page_size_auto);

        let config = parse("[post.mail]\npage_size_auto = true\n", None);
        assert!(config.page_size_auto);
    }

    #[test]
    fn nonboolean_page_size_auto_falls_back_to_default() {
        let text = "[post.mail]\npage_size_auto = \"yes\"\n";
        let (config, issues) = parse_with_issues(text, None);
        assert!(config.page_size_auto, "default applies");
        assert!(!issues.is_empty(), "the problem is reported");
    }

    #[test]
    fn refresh_interval_defaults_to_sixty_and_zero_disables() {
        // Absent: the plan default (Phase 9.4).
        assert_eq!(parse("", None).refresh_interval_seconds, 60);
        // Explicit value wins; 0 disables the timer.
        let text = "[post.mail]\nrefresh_interval_seconds = 120\n";
        assert_eq!(parse(text, None).refresh_interval_seconds, 120);
        let text = "[post.mail]\nrefresh_interval_seconds = 0\n";
        assert_eq!(parse(text, None).refresh_interval_seconds, 0);
        // A negative value is nonsense: treated as disabled, never a loop.
        let text = "[post.mail]\nrefresh_interval_seconds = -5\n";
        assert_eq!(parse(text, None).refresh_interval_seconds, 0);
        // Non-numeric falls back to the default.
        let text = "[post.mail]\nrefresh_interval_seconds = \"soon\"\n";
        assert_eq!(parse(text, None).refresh_interval_seconds, 60);
    }

    #[test]
    fn downloads_dir_is_parsed_as_written() {
        // `~` is NOT expanded here; the backend expands it so the whole
        // path pipeline stays shell-free and testable (plan §15).
        let text = "[post.attachments]\ndownloads_dir = \"~/My Downloads\"\n";
        assert_eq!(
            parse(text, None).downloads_dir,
            Some(PathBuf::from("~/My Downloads"))
        );
        // Empty or non-string values fall back to None.
        assert_eq!(
            parse("[post.attachments]\ndownloads_dir = \"\"\n", None).downloads_dir,
            None
        );
        assert_eq!(
            parse("[post.attachments]\ndownloads_dir = 3\n", None).downloads_dir,
            None
        );
        assert_eq!(parse("", None).downloads_dir, None);
    }

    #[test]
    fn non_string_alias_values_are_ignored() {
        let text = r#"
            [post]
            account = "probe"

            [accounts.probe.mailbox.alias]
            inbox = 3
            trash = "Archive"
        "#;
        let config = parse(text, None);
        assert_eq!(config.aliases.len(), 1);
        assert_eq!(
            config.aliases.get("trash").map(String::as_str),
            Some("Archive")
        );
    }

    #[test]
    fn default_flag_account_resolves_aliases_without_post_section() {
        // The user-config shape that regressed: himalaya-style account with
        // `default = true` and mailbox aliases, no `[post]` section.
        let text = r#"
            [accounts.gmail]
            default = true
            email = "probe@example.org"
            display-name = "Post Probe"

            [accounts.gmail.imap]
            server = "imaps://imap.example.org:993"

            [accounts.gmail.mailbox.alias]
            inbox = "INBOX"
            sent = "[Gmail]/Sent Mail"
            drafts = "[Gmail]/Drafts"
            trash = "[Gmail]/Trash"
            archive = "[Gmail]/All Mail"
        "#;
        let config = parse(text, None);
        assert_eq!(config.account.as_deref(), Some("gmail"));
        assert_eq!(
            config.aliases.get("drafts").map(String::as_str),
            Some("[Gmail]/Drafts")
        );
        assert_eq!(
            config.aliases.get("archive").map(String::as_str),
            Some("[Gmail]/All Mail")
        );
        assert_eq!(config.account_email.as_deref(), Some("probe@example.org"));
        assert_eq!(config.account_display_name.as_deref(), Some("Post Probe"));
    }

    #[test]
    fn sole_account_resolves_without_default_flag() {
        let text = r#"
            [accounts.probe]
            email = "probe@post.local"

            [accounts.probe.mailbox.alias]
            drafts = "Drafts"
        "#;
        let config = parse(text, None);
        assert_eq!(config.account.as_deref(), Some("probe"));
        assert_eq!(
            config.aliases.get("drafts").map(String::as_str),
            Some("Drafts")
        );
    }

    #[test]
    fn several_accounts_without_default_stay_unresolved() {
        let text = r#"
            [accounts.one]
            email = "one@post.local"

            [accounts.two]
            default = false
            email = "two@post.local"
        "#;
        let config = parse(text, None);
        assert_eq!(config.account, None);
        assert!(config.aliases.is_empty());
    }

    #[test]
    fn explicit_post_account_wins_over_default_flag() {
        let text = r#"
            [post]
            account = "two"

            [accounts.one]
            default = true
            email = "one@post.local"

            [accounts.one.mailbox.alias]
            inbox = "ONE Inbox"

            [accounts.two.mailbox.alias]
            inbox = "TWO Inbox"
        "#;
        let config = parse(text, None);
        assert_eq!(config.account.as_deref(), Some("two"));
        assert_eq!(
            config.aliases.get("inbox").map(String::as_str),
            Some("TWO Inbox")
        );
    }

    #[test]
    fn account_identity_is_parsed_for_draft_from_headers() {
        let text = r#"
            [accounts.probe]
            email = "probe@post.local"
            display-name = "Post Probe"

            [post]
            account = "probe"
        "#;
        let config = parse(text, None);
        assert_eq!(config.account_email.as_deref(), Some("probe@post.local"));
        assert_eq!(config.account_display_name.as_deref(), Some("Post Probe"));
        // The sole account resolves even without `[post].account`, so the
        // draft `From` identity comes along.
        let config = parse("[accounts.probe]\nemail = \"probe@post.local\"\n", None);
        assert_eq!(config.account_email.as_deref(), Some("probe@post.local"));
    }

    // ── Phase 10.4: one-file completion and startup validation ──────────

    #[test]
    fn parses_the_reference_shape() {
        let text = r#"
            [post]
            account = "probe"
            mouse = true

            [post.mail]
            page_size = 50
            refresh_interval_seconds = 0

            [post.composer]
            editor = "builtin"
            autosave_delay_ms = 4000

            [post.attachments]
            downloads_dir = "~/Downloads"

            [post.theme]
            name = "default"

            [accounts.probe]
            email = "probe@post.local"
        "#;
        let (config, issues) = parse_with_issues(text, None);
        assert!(issues.is_empty(), "{issues:?}");
        assert!(config.mouse);
        assert_eq!(config.page_size, 50);
        assert_eq!(config.refresh_interval_seconds, 0);
        assert_eq!(config.editor, "builtin");
        assert_eq!(config.autosave_delay_ms, 4000);
        assert_eq!(config.theme_name, "default");
    }

    #[test]
    fn non_bool_mouse_reports_and_disables() {
        let (config, issues) = parse_with_issues("[post]\nmouse = \"yes\"\n", None);
        assert!(!config.mouse);
        assert_eq!(issues.items.len(), 1);
        assert!(issues.items[0].contains("mouse"));
    }

    #[test]
    fn out_of_bounds_autosave_delay_reports_and_falls_back() {
        for delay in [50_i64, 1_000_000] {
            let text = format!("[post.composer]\nautosave_delay_ms = {delay}\n");
            let (config, issues) = parse_with_issues(&text, None);
            assert_eq!(config.autosave_delay_ms, DEFAULT_AUTOSAVE_DELAY_MS);
            assert_eq!(issues.items.len(), 1, "{delay}");
            assert!(issues.items[0].contains("autosave_delay_ms"));
        }
        let (config, issues) =
            parse_with_issues("[post.composer]\nautosave_delay_ms = 500\n", None);
        assert!(issues.is_empty());
        assert_eq!(config.autosave_delay_ms, 500);
    }

    #[test]
    fn unknown_theme_name_reports() {
        let (config, issues) = parse_with_issues("[post.theme]\nname = \"solarized\"\n", None);
        assert_eq!(config.theme_name, "default");
        assert_eq!(issues.items.len(), 1);
        assert!(issues.items[0].contains("solarized"));
    }

    #[test]
    fn editor_validation_covers_the_documented_shapes() {
        assert!(validate_editor_with("builtin", None).is_ok());
        assert!(validate_editor_with("$EDITOR", Some(String::from("nvim"))).is_ok());
        assert!(
            validate_editor_with("$EDITOR", None)
                .unwrap_err()
                .contains("$EDITOR is not set")
        );
        // Shell metacharacters are rejected: Post never spawns a shell.
        assert!(
            validate_editor_with("nvim -c 'set nu'", None)
                .unwrap_err()
                .contains("metacharacters")
        );
        // A plain command must resolve.
        assert!(
            validate_editor_with("definitely-not-a-real-program-xyz", None)
                .unwrap_err()
                .contains("PATH")
        );
        // An absolute path must exist.
        assert!(
            validate_editor_with("/definitely/missing/editor", None)
                .unwrap_err()
                .contains("does not exist")
        );
    }

    #[test]
    fn unknown_editor_program_reports_and_falls_back() {
        let (config, issues) = parse_with_issues(
            "[post.composer]\neditor = \"definitely-not-a-real-program-xyz\"\n",
            None,
        );
        assert_eq!(config.editor, "builtin");
        assert_eq!(issues.items.len(), 1);
        assert!(issues.items[0].contains("editor"));
    }

    #[test]
    fn missing_selected_account_reports() {
        let (config, issues) = parse_with_issues("[post]\naccount = \"ghost\"\n", None);
        assert_eq!(config.account.as_deref(), Some("ghost"));
        assert_eq!(issues.items.len(), 1);
        assert!(issues.items[0].contains("[accounts.ghost]"));
    }

    #[test]
    fn relative_downloads_dir_reports() {
        let (config, issues) = parse_with_issues(
            "[post.attachments]\ndownloads_dir = \"Downloads/out\"\n",
            None,
        );
        assert_eq!(config.downloads_dir, None);
        assert_eq!(issues.items.len(), 1);
        assert!(issues.items[0].contains("absolute path"));
    }

    #[test]
    fn malformed_toml_reports_sanitized() {
        let (config, issues) = parse_with_issues("not [ valid toml", None);
        assert_eq!(config, Config::default());
        assert_eq!(issues.items.len(), 1);
        assert!(issues.items[0].contains("not valid TOML"));
    }

    #[test]
    fn issues_carry_no_secret_values() {
        // A malformed line quoting a secret-shaped value must not surface
        // the value in the reported detail.
        let (config, issues) = parse_with_issues(
            "[post]\naccount = \"probe\"\npassword = \"hunter2 )\"\n",
            None,
        );
        let _ = config;
        let joined = issues.items.join("\n");
        assert!(!joined.contains("hunter2"), "leaked: {joined}");
    }

    #[test]
    fn program_exists_rejects_unknown_names() {
        assert!(!program_exists("definitely-not-a-real-program-xyz"));
    }
}

#[cfg(test)]
mod theme_override_tests {
    use super::*;

    #[test]
    fn parse_hex_color_normalizes_and_accepts_both_lengths() {
        assert_eq!(parse_hex_color("#4e86dd").as_deref(), Some("#4e86dd"));
        assert_eq!(parse_hex_color("#4E86DD").as_deref(), Some("#4e86dd"));
        assert_eq!(parse_hex_color("#abc").as_deref(), Some("#aabbcc"));
        assert_eq!(parse_hex_color("4e86dd"), None, "missing #");
        assert_eq!(parse_hex_color("#4e86"), None, "wrong length");
        assert_eq!(parse_hex_color("#4e86dd0"), None, "too long");
        assert_eq!(parse_hex_color("#xyzxyz"), None, "not hex");
        assert_eq!(parse_hex_color("#"), None, "empty");
    }

    #[test]
    fn theme_overrides_parse_validate_and_normalize() {
        let (config, issues) = parse_with_issues(
            "[post.theme]\nname = \"light\"\naccent = \"#ABC\"\nbackground = \"#101014\"\n",
            None,
        );
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(config.theme_name, "light");
        assert_eq!(
            config.theme_overrides,
            vec![
                (String::from("accent"), String::from("#aabbcc")),
                (String::from("background"), String::from("#101014")),
            ]
        );
    }

    #[test]
    fn bad_hex_reports_the_token() {
        let (_, issues) = parse_with_issues("[post.theme]\naccent = \"blue\"\n", None);
        assert_eq!(issues.items.len(), 1);
        assert!(issues.items[0].contains("accent"), "{issues:?}");
        assert!(issues.items[0].contains("hex"), "{issues:?}");

        let (_, issues) = parse_with_issues("[post.theme]\naccent = 7\n", None);
        assert_eq!(issues.items.len(), 1);
        assert!(issues.items[0].contains("accent"), "{issues:?}");
    }

    #[test]
    fn unknown_theme_token_reports_the_known_ones() {
        let (_, issues) = parse_with_issues("[post.theme]\nfont = \"x\"\n", None);
        assert_eq!(issues.items.len(), 1);
        assert!(issues.items[0].contains("font"), "{issues:?}");
        assert!(issues.items[0].contains("background"), "{issues:?}");
    }

    #[test]
    fn light_theme_name_is_known() {
        let (config, issues) = parse_with_issues("[post.theme]\nname = \"light\"\n", None);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(config.theme_name, "light");
    }

    #[test]
    fn issues_carry_no_file_contents_for_theme_errors() {
        // The error names the token and the expected shape, never the
        // offending value's raw content beyond the token context.
        let (_, issues) = parse_with_issues("[post.theme]\naccent = \"#zz\"\n", None);
        assert!(issues.items.iter().all(|i| !i.contains("zz")));
    }
}

#[cfg(test)]
mod ui_clock_tests {
    use super::*;

    #[test]
    fn clock_is_off_by_default_and_configurable() {
        let (config, issues) = parse_with_issues("", None);
        assert!(issues.is_empty());
        assert!(!config.ui_clock, "clock off by default");

        let (config, issues) = parse_with_issues("[post.ui]\nclock = true\n", None);
        assert!(issues.is_empty(), "{issues:?}");
        assert!(config.ui_clock);
    }

    #[test]
    fn non_bool_clock_reports() {
        let (_, issues) = parse_with_issues("[post.ui]\nclock = \"yes\"\n", None);
        assert_eq!(issues.items.len(), 1);
        assert!(issues.items[0].contains("clock"), "{issues:?}");
    }
}

#[cfg(test)]
mod view_mode_tests {
    use super::*;

    #[test]
    fn view_mode_defaults_to_compact_and_parses_both_values() {
        let (config, issues) = parse_with_issues("", None);
        assert!(issues.is_empty());
        assert_eq!(config.view_mode, ViewMode::Compact, "compact by default");

        let (config, issues) = parse_with_issues("[post]\nview_mode = \"compact\"\n", None);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(config.view_mode, ViewMode::Compact);

        let (config, issues) = parse_with_issues("[post]\nview_mode = \"comfortable\"\n", None);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(config.view_mode, ViewMode::Comfortable);
    }

    #[test]
    fn unknown_view_mode_reports_and_falls_back() {
        let (config, issues) = parse_with_issues("[post]\nview_mode = \"spacious\"\n", None);
        assert_eq!(config.view_mode, ViewMode::Compact, "default applies");
        assert_eq!(issues.items.len(), 1, "{issues:?}");
        assert!(issues.items[0].contains("view_mode"), "{issues:?}");
        assert!(issues.items[0].contains("comfortable"), "{issues:?}");

        // A non-string value is reported the same way.
        let (config, issues) = parse_with_issues("[post]\nview_mode = 3\n", None);
        assert_eq!(config.view_mode, ViewMode::Compact);
        assert_eq!(issues.items.len(), 1, "{issues:?}");
    }

    #[test]
    fn comfortable_rows_cost_double() {
        assert_eq!(ViewMode::Compact.row_height(), 1);
        assert_eq!(ViewMode::Comfortable.row_height(), 2);
    }
}

#[cfg(test)]
mod status_timeout_tests {
    use super::*;

    #[test]
    fn status_timeout_defaults_to_zero_and_parses_seconds() {
        let (config, issues) = parse_with_issues("", None);
        assert!(issues.is_empty());
        assert_eq!(config.status_timeout, 0, "disabled by default");

        let (config, issues) = parse_with_issues("[post]\nstatus_timeout = 5\n", None);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(config.status_timeout, 5);

        // Explicit 0 stays disabled without complaint.
        let (config, issues) = parse_with_issues("[post]\nstatus_timeout = 0\n", None);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(config.status_timeout, 0);
    }

    #[test]
    fn invalid_status_timeout_reports_and_falls_back() {
        let (config, issues) = parse_with_issues("[post]\nstatus_timeout = -3\n", None);
        assert_eq!(config.status_timeout, 0);
        assert_eq!(issues.items.len(), 1, "{issues:?}");
        assert!(issues.items[0].contains("status_timeout"), "{issues:?}");

        let (config, issues) = parse_with_issues("[post]\nstatus_timeout = \"soon\"\n", None);
        assert_eq!(config.status_timeout, 0);
        assert_eq!(issues.items.len(), 1, "{issues:?}");
    }
}

#[cfg(test)]
mod theme_table_tests {
    use super::*;

    #[test]
    fn user_theme_tables_parse_validate_and_normalize() {
        let text = r##"
            [post.themes.nord]
            background = "#2E3440"
            accent = "#88c0d0"

            [post.themes.solar]
            accent = "#b58900"
        "##;
        let (config, issues) = parse_with_issues(text, None);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(config.theme_tables.len(), 2);
        assert_eq!(config.theme_tables[0].0, "nord");
        // Token application order follows file order within a table, but
        // the TOML map does not promise iteration order; compare as sets.
        let nord: std::collections::BTreeMap<&str, &str> = config.theme_tables[0]
            .1
            .iter()
            .map(|(t, h)| (t.as_str(), h.as_str()))
            .collect();
        assert_eq!(
            nord,
            std::collections::BTreeMap::from([("background", "#2e3440"), ("accent", "#88c0d0"),])
        );
        assert_eq!(config.theme_tables[1].0, "solar");
    }

    #[test]
    fn user_theme_table_problems_report_with_the_table_path() {
        let text = r##"
            [post.themes.broken]
            accent = "blue"
            font = "#101014"
        "##;
        let (config, issues) = parse_with_issues(text, None);
        // The bad hex is dropped, the unknown token is reported, the
        // (empty) theme entry still exists.
        assert_eq!(config.theme_tables.len(), 1);
        assert!(config.theme_tables[0].1.is_empty());
        assert_eq!(issues.items.len(), 2, "{issues:?}");
        assert!(
            issues
                .items
                .iter()
                .all(|i| i.contains("[post.themes.broken]"))
        );
    }

    #[test]
    fn non_table_user_theme_reports() {
        let (config, issues) = parse_with_issues("[post.themes]\nflat = 3\n", None);
        assert!(config.theme_tables.is_empty());
        assert_eq!(issues.items.len(), 1);
        assert!(issues.items[0].contains("[post.themes.flat]"));
    }

    #[test]
    fn themes_section_defaults_to_empty() {
        let (config, issues) = parse_with_issues("", None);
        assert!(issues.is_empty());
        assert!(config.theme_tables.is_empty());
    }
}

#[cfg(test)]
mod cache_limit_tests {
    use super::*;

    #[test]
    fn cache_limits_have_sane_defaults_and_are_configurable() {
        let (config, issues) = parse_with_issues("", None);
        assert!(issues.is_empty());
        assert_eq!(config.cache_max_messages, DEFAULT_CACHE_MAX_MESSAGES);
        assert_eq!(config.cache_max_bytes, DEFAULT_CACHE_MAX_BYTES);
        assert!(
            config.cache_max_messages > 0 && config.cache_max_bytes > 0,
            "defaults are sane, not disabled"
        );

        let (config, issues) = parse_with_issues(
            "[post.cache]\nmax_messages = 10\nmax_bytes = 1048576\n",
            None,
        );
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(config.cache_max_messages, 10);
        assert_eq!(config.cache_max_bytes, 1_048_576);
    }

    #[test]
    fn invalid_cache_limits_report() {
        let (_, issues) = parse_with_issues(
            "[post.cache]\nmax_messages = -1\nmax_bytes = \"big\"\n",
            None,
        );
        assert_eq!(issues.items.len(), 2, "{issues:?}");
        assert!(
            issues.items.iter().any(|i| i.contains("max_messages")),
            "{issues:?}"
        );
        assert!(
            issues.items.iter().any(|i| i.contains("max_bytes")),
            "{issues:?}"
        );

        let (_, issues) = parse_with_issues("[post.cache]\nflavor = \"vanilla\"\n", None);
        assert_eq!(issues.items.len(), 1);
        assert!(issues.items[0].contains("flavor"), "{issues:?}");
    }
}
