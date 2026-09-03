//! Post-owned configuration (plan §17) — Phase 2 subset.
//!
//! One canonical TOML file is shared with Himalaya (ADR 0001 finding 13:
//! himalaya 2.1.0 tolerates the unknown `[post]` root table). Post reads its
//! own `[post]` section plus the selected account's `mailbox.alias` table,
//! which the backend adapter uses to resolve mailbox roles (ADR 0001: the
//! UI never guesses folder names).
//!
//! Phase 2 keeps loading forgiving: an unreadable or malformed file falls
//! back to defaults (while still forwarding the path to himalaya with `-c`)
//! and logs a warning. Startup validation with actionable errors arrives
//! with the config-completion work (plan §17).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Default page size when the config does not provide a usable one
/// (plan §16/§17: explicit pagination, default 20).
pub const DEFAULT_PAGE_SIZE: usize = 20;

/// Default periodic refresh interval in seconds (plan §11/§19 Phase 9);
/// `[post.mail].refresh_interval_seconds = 0` disables the timer.
pub const DEFAULT_REFRESH_INTERVAL_SECONDS: u64 = 60;

/// Resolved, validated Phase 2 configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Config file forwarded to himalaya with `-c`, when one was resolved.
    pub path: Option<PathBuf>,
    /// `[post].account`: the himalaya account Post drives. `None` lets
    /// himalaya pick its default account (and disables alias-based role
    /// resolution, which is per-account).
    pub account: Option<String>,
    /// `[post.mail].page_size`, defaulting to [`DEFAULT_PAGE_SIZE`]; a
    /// non-positive or absent value falls back to the default.
    pub page_size: usize,
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            path: None,
            account: None,
            page_size: DEFAULT_PAGE_SIZE,
            refresh_interval_seconds: DEFAULT_REFRESH_INTERVAL_SECONDS,
            aliases: HashMap::new(),
            account_email: None,
            account_display_name: None,
            downloads_dir: None,
        }
    }
}

impl Config {
    /// Load the configuration: the CLI path wins, then `POST_CONFIG`, then
    /// the well-known himalaya config locations. A file that exists but
    /// cannot be parsed still pins `path` (himalaya must receive `-c` and
    /// will report the real problem) while Post itself runs on defaults.
    pub fn load(cli_path: Option<&Path>) -> Self {
        let path = resolve_path(cli_path);
        let Some(path) = path else {
            return Config::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => parse(&text, Some(path)),
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "config unreadable; using defaults");
                Config {
                    path: Some(path),
                    ..Config::default()
                }
            }
        }
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
    let mut config = Config {
        path,
        ..Config::default()
    };
    let Ok(doc) = toml::from_str::<toml::Value>(text) else {
        tracing::warn!("config is not valid TOML; using default [post] settings");
        return config;
    };

    if let Some(account) = doc
        .get("post")
        .and_then(|post| post.get("account"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
    {
        config.account = Some(account);
    }
    if let Some(page_size) = doc
        .get("post")
        .and_then(|post| post.get("mail"))
        .and_then(|mail| mail.get("page_size"))
        .and_then(toml::Value::as_integer)
        .filter(|size| *size > 0)
        .map(|size| size as usize)
    {
        config.page_size = page_size;
    }
    // The refresh interval: explicit 0 disables the timer; a negative
    // value is treated as disabled too (a nonsense interval must never
    // become a busy loop); anything absent or non-numeric defaults to 60.
    config.refresh_interval_seconds = doc
        .get("post")
        .and_then(|post| post.get("mail"))
        .and_then(|mail| mail.get("refresh_interval_seconds"))
        .and_then(toml::Value::as_integer)
        .map(|seconds| seconds.max(0) as u64)
        .unwrap_or(DEFAULT_REFRESH_INTERVAL_SECONDS);
    if let Some(downloads_dir) = doc
        .get("post")
        .and_then(|post| post.get("attachments"))
        .and_then(|attachments| attachments.get("downloads_dir"))
        .and_then(toml::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        config.downloads_dir = Some(downloads_dir);
    }
    // Without `[post].account`, drive the account himalaya itself would
    // pick (no `-a` is forwarded): the one marked `default = true`, else
    // the sole account. The alias table is per-account, so role resolution
    // stays dark until this matches (real-config finding: a `default =
    // true` Gmail account with mailbox aliases resolved nothing).
    if config.account.is_none() {
        config.account = default_account(&doc);
    }
    if let Some(account) = config.account.as_deref() {
        config.aliases = aliases_for(&doc, account);
        config.account_email = account_field(&doc, account, "email");
        config.account_display_name = account_field(&doc, account, "display-name");
    }
    config
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
}
