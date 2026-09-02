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
    /// `[accounts.<account>.mailbox.alias]` entries: role key → mailbox
    /// name. Non-string values are ignored.
    pub aliases: HashMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            path: None,
            account: None,
            page_size: DEFAULT_PAGE_SIZE,
            aliases: HashMap::new(),
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
    if let Some(account) = config.account.as_deref() {
        config.aliases = aliases_for(&doc, account);
    }
    config
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
    fn alias_without_account_is_not_resolved() {
        let text = r#"
            [accounts.probe.mailbox.alias]
            inbox = "INBOX"
        "#;
        let config = parse(text, None);
        assert!(config.aliases.is_empty());
    }
}
