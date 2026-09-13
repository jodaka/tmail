//! Format-preserving writer for the wizard account (ADR 0003 §3.6).
//!
//! The wizard merges the new `[accounts.<name>]` block into the single
//! TOML file shared with himalaya with `toml_edit`, so existing
//! accounts, `[tmail]` tables, comments and ordering survive. The
//! account block is built by composing a TOML fragment string and
//! parsing it with `toml_edit` (matching himalaya's own wizard output:
//! dotted `imap.sasl.plain.*` keys), then inserted wholesale. A
//! freshly created config file is created with mode 0600; an existing
//! file keeps its mode, and a group/world-readable file about to
//! receive `password.raw` earns a warning instead of a silent chmod.

use std::io::Write;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};

use crate::config::parse_with_issues;

/// How the wizard stores the account secret (ADR 0003 §3.2 W4): the
/// same pair himalaya accepts, `password.raw` or `password.cmd`.
#[derive(Clone, PartialEq, Eq)]
pub enum SecretStorage {
    /// The secret itself, stored in the config file.
    Raw(String),
    /// A command producing the secret on stdout (never executed by
    /// Tmail; himalaya runs it during connections).
    Command(String),
}

impl std::fmt::Debug for SecretStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The raw password never renders — these values ride inside
        // Debug-printable payloads (operation kinds, wizard state) and
        // tracing must never see them (ADR 0003 §3.7). A `password.cmd`
        // command line is config data himalaya stores in plain text; it
        // is not itself a secret and renders verbatim.
        match self {
            SecretStorage::Raw(_) => f.write_str("Raw(███)"),
            SecretStorage::Command(command) => write!(f, "Command({command:?})"),
        }
    }
}

impl SecretStorage {
    /// The himalaya key carrying the secret: `password.raw` or
    /// `password.cmd`.
    fn key(&self) -> &'static str {
        match self {
            SecretStorage::Raw(_) => "raw",
            SecretStorage::Command(_) => "cmd",
        }
    }

    /// The secret value (raw password or the command line).
    fn value(&self) -> &str {
        match self {
            SecretStorage::Raw(password) => password,
            SecretStorage::Command(command) => command,
        }
    }

    /// Whether a secret string is written into the file (only the raw
    /// mode stores a secret; `password.cmd` stores a command line).
    fn stores_secret(&self) -> bool {
        matches!(self, SecretStorage::Raw(_))
    }
}

/// The account-name collision rule (ADR 0003 §3.6): `base`, then
/// `base-2`, `base-3`, … Shared by the wizard and the save-side tests.
pub fn next_free_name(existing: &[String], base: &str) -> String {
    if !existing.iter().any(|name| name == base) {
        return base.to_string();
    }
    for suffix in 2.. {
        let candidate = format!("{base}-{suffix}");
        if !existing.iter().any(|name| name == &candidate) {
            return candidate;
        }
    }
    unreachable!("suffix loop always returns")
}

/// The account draft the wizard saves, already carrying the final
/// account name (collision-resolved by the wizard).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftAccount {
    /// Final `[accounts.<name>]` id, sanitized and collision-resolved.
    pub name: String,
    /// The account email address.
    pub email: String,
    /// Optional display name (omitted from the file when absent).
    pub display_name: Option<String>,
    /// Full IMAP server URL, e.g. `imaps://imap.gmail.com:993`.
    pub imap_server: String,
    /// Whether IMAP negotiates STARTTLS (writes `imap.starttls = true`).
    pub imap_starttls: bool,
    /// Full SMTP server URL, e.g. `smtps://smtp.gmail.com:465`.
    pub smtp_server: String,
    /// Whether SMTP negotiates STARTTLS (writes `smtp.starttls = true`).
    pub smtp_starttls: bool,
    /// The SASL PLAIN username (same for IMAP and SMTP in v1).
    pub username: String,
    /// Raw password or retrieval command, shared by IMAP and SMTP.
    pub secret: SecretStorage,
    /// `mailbox.alias.<role>` entries in display order.
    pub aliases: Vec<(String, String)>,
}

/// What happened during a successful save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveReport {
    /// The file that now holds the account.
    pub path: PathBuf,
    /// Whether the file was freshly created (and got mode 0600).
    pub created: bool,
    /// Warning for the confirm/save flow: an existing group/world-readable
    /// config received `password.raw`.
    pub permissions_warning: Option<String>,
}

/// The first label of `domain`, lowercased, reduced to `[a-z0-9-]`
/// (ADR 0003 §3.6: `mail.example.com` → `example`). Empty when the
/// domain yields nothing usable.
pub fn sanitize_account_name(domain: &str) -> String {
    let label = domain
        .trim()
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let sanitized: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    sanitized.trim_matches('-').to_string()
}

/// The account names already present in the config file; empty when
/// the file is missing or unreadable (the wizard then skips the
/// collision flow).
pub fn existing_account_names(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(doc) = text.parse::<DocumentMut>() else {
        return Vec::new();
    };
    account_names(&doc)
}

/// The name of the account marked `default = true` in the file, if any
/// (the wizard snapshots it for the `default` preview and the collision
/// flow). Missing/unreadable file → `None`.
pub fn file_default_account(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let doc = text.parse::<DocumentMut>().ok()?;
    doc.get("accounts")
        .and_then(Item::as_table)?
        .iter()
        .find(|(_, account)| {
            account
                .get("default")
                .and_then(Item::as_bool)
                .unwrap_or(false)
        })
        .map(|(name, _)| name.to_string())
}

/// Whether an existing config file is group/world-readable (the
/// confirm screen shows the chmod warning when `password.raw` is
/// about to join it). Missing file → `false`.
pub fn file_shared_readable(path: &Path) -> bool {
    shared_readable(
        std::fs::metadata(path)
            .ok()
            .map(|meta| meta.permissions())
            .as_ref(),
    )
}

/// Saves the draft account into `path` (ADR 0003 §3.6): format-
/// preserving merge, fresh-file 0600 creation, post-write validation.
/// On failure nothing partial is left behind — the caller surfaces the
/// sanitized message.
pub fn save_account(path: &Path, draft: &DraftAccount) -> Result<SaveReport, String> {
    let existing = std::fs::read_to_string(path);
    let created = existing.is_err();
    let mut doc = match existing {
        Ok(text) => text
            .parse::<DocumentMut>()
            .map_err(|err| format!("existing config is not valid TOML: {err}"))?,
        // Fresh file: start from an empty document.
        Err(_) => DocumentMut::new(),
    };

    // `default = true` only when no *other* account has it: replacing
    // the file's default account keeps it default, and the pre-existing
    // default of a different account stays authoritative.
    let set_default = !other_account_has_default(&doc, &draft.name);

    let warning = if draft.secret.stores_secret() && !created && file_shared_readable(path) {
        Some(format!(
            "existing config is readable by others; run chmod 600 {}",
            path.display()
        ))
    } else {
        None
    };

    let (account, container) = build_account_item(draft, set_default)?;
    insert_account(&mut doc, draft.name.as_str(), account, container)?;

    write_document(path, &doc, created)?;

    validate_written(path, &draft.name)?;

    Ok(SaveReport {
        path: path.to_path_buf(),
        created,
        permissions_warning: warning,
    })
}

/// Builds the `[accounts.<name>]` item from a TOML fragment string
/// (ADR 0003 §3.6: composing the fragment avoids fighting
/// `toml_edit`'s implicit-table API for dotted keys, and the output
/// reads like himalaya's own wizard output). Returns the account item
/// plus the fragment's implicit `accounts` container for the
/// fresh-file case: an implicit container renders without an empty
/// `[accounts]` header, matching the ADR §1 target shape.
fn build_account_item(draft: &DraftAccount, set_default: bool) -> Result<(Item, Item), String> {
    let fragment = account_fragment(draft, set_default);
    let doc: DocumentMut = fragment.parse().map_err(|err| {
        format!("internal error: generated account fragment does not parse: {err}")
    })?;
    Ok((
        doc["accounts"][&draft.name].clone(),
        doc["accounts"].clone(),
    ))
}

/// Inserts the account block: into the existing `[accounts]` table
/// (wholesale replacement on collision), or as the implicit container
/// when the document has no accounts table yet.
fn insert_account(
    doc: &mut DocumentMut,
    name: &str,
    account: Item,
    container: Item,
) -> Result<(), &'static str> {
    match doc.get_mut("accounts") {
        Some(Item::Table(table)) => {
            table.insert(name, account);
            Ok(())
        }
        Some(_) => Err("[accounts] exists but is not a table"),
        None => {
            doc["accounts"] = container;
            Ok(())
        }
    }
}

/// Renders a standalone account block (ADR 0003 §3.4): the same
/// fragment the writer merges, always marked `default = true` so a
/// temporary himalaya config holding only this block resolves it. Used
/// by the wizard's credential test to serialize the draft into a
/// temporary 0600 config file.
pub fn draft_account_fragment(draft: &DraftAccount) -> String {
    account_fragment(draft, true)
}

/// Renders the account block exactly in the ADR §1 target shape.
fn account_fragment(draft: &DraftAccount, set_default: bool) -> String {
    let mut fragment = String::new();
    fragment.push_str(&format!("[accounts.{}]\n", draft.name));
    if set_default {
        fragment.push_str("default = true\n");
    }
    fragment.push_str(&format!("email = {}\n", toml_str(&draft.email)));
    if let Some(display_name) = draft
        .display_name
        .as_deref()
        .filter(|name| !name.is_empty())
    {
        fragment.push_str(&format!("display-name = {}\n", toml_str(display_name)));
    }
    fragment.push('\n');
    fragment.push_str(&format!("imap.server = {}\n", toml_str(&draft.imap_server)));
    if draft.imap_starttls {
        fragment.push_str("imap.starttls = true\n");
    }
    fragment.push_str(&format!(
        "imap.sasl.plain.username = {}\n",
        toml_str(&draft.username)
    ));
    fragment.push_str(&format!(
        "imap.sasl.plain.password.{} = {}\n",
        draft.secret.key(),
        toml_str(draft.secret.value())
    ));
    fragment.push('\n');
    fragment.push_str(&format!("smtp.server = {}\n", toml_str(&draft.smtp_server)));
    if draft.smtp_starttls {
        fragment.push_str("smtp.starttls = true\n");
    }
    fragment.push_str(&format!(
        "smtp.sasl.plain.username = {}\n",
        toml_str(&draft.username)
    ));
    fragment.push_str(&format!(
        "smtp.sasl.plain.password.{} = {}\n",
        draft.secret.key(),
        toml_str(draft.secret.value())
    ));
    if !draft.aliases.is_empty() {
        fragment.push('\n');
        for (role, mailbox) in &draft.aliases {
            fragment.push_str(&format!("mailbox.alias.{} = {}\n", role, toml_str(mailbox)));
        }
    }
    fragment
}

/// Renders a TOML basic string with `toml_edit`'s own escaping (values
/// like passwords may carry quotes, backslashes or control bytes).
fn toml_str(value: &str) -> String {
    toml_edit::Value::from(value).to_string()
}

/// Writes the merged document: a fresh file is created with mode 0600
/// (`create_new` — never world-readable at any instant); an existing
/// file is replaced atomically (temp file, sync, rename — ticket 12h7),
/// so a crash mid-write cannot corrupt the shared config, and its mode
/// is preserved.
fn write_document(path: &Path, doc: &DocumentMut, created: bool) -> Result<(), String> {
    let text = doc.to_string();
    if created {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("could not create config directory: {err}"))?;
        }
        let mut file = open_new_private_file(path)?;
        file.write_all(text.as_bytes())
            .map_err(|err| format!("could not write config file: {err}"))
    } else {
        write_existing_atomically(path, text.as_bytes())
    }
}

/// Replace an existing file atomically (the `DraftJournal::write_atomic`
/// pattern): write a sibling temp file, sync it, then rename over the
/// target. Rename is atomic on the same filesystem, so the target is
/// either the old or the new content — never a partial write. The temp
/// file inherits restrictive defaults and is removed on any failure.
fn write_existing_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension(format!("toml.tmp-{}", std::process::id()));
    let remove_tmp = |tmp: &Path| {
        let _ = std::fs::remove_file(tmp);
    };
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)
            .map_err(|err| format!("could not create a temporary config file: {err}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Match the target's mode when readable, else owner-only; a
            // shared-readable config is deliberately not downgraded
            // silently (the wizard warns about that separately).
            let mode = std::fs::metadata(path)
                .ok()
                .map(|meta| meta.permissions().mode() & 0o777)
                .unwrap_or(0o600);
            file.set_permissions(std::fs::Permissions::from_mode(mode))
                .map_err(|err| format!("could not set config file permissions: {err}"))?;
        }
        file.write_all(bytes)
            .map_err(|err| format!("could not write config file: {err}"))?;
        file.sync_all()
            .map_err(|err| format!("could not sync the config file: {err}"))?;
        drop(file);
        std::fs::rename(&tmp, path)
            .map_err(|err| format!("could not replace the config file: {err}"))?;
        Ok(())
    })();
    if result.is_err() {
        remove_tmp(&tmp);
    }
    result
}

/// Creates the config file exclusively with mode 0600 where the OS
/// supports file modes.
#[cfg(unix)]
fn open_new_private_file(path: &Path) -> Result<std::fs::File, String> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|err| format!("could not create config file: {err}"))
}

#[cfg(not(unix))]
fn open_new_private_file(path: &Path) -> Result<std::fs::File, String> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| format!("could not create config file: {err}"))
}

/// Re-parses the merged file with the existing `parse_with_issues` and
/// only reports success when the new account table carries the keys
/// himalaya needs (guards against a `toml_edit` shape mistake).
fn validate_written(path: &Path, name: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("saved config could not be re-read: {err}"))?;
    let (_, issues) = parse_with_issues(&text, Some(path.to_path_buf()));
    if issues.iter().any(|issue| issue.contains("not valid TOML")) {
        // Ticket 12h7: by this point the file IS written — the old message
        // ("nothing was changed") was false. Name the file so the user can
        // inspect or restore it.
        return Err(format!(
            "internal error: the merged config written to {} does not parse; \
             the file on disk carries the broken merge and may need manual repair",
            path.display()
        ));
    }
    let doc: toml::Value = toml::from_str(&text)
        .map_err(|_| String::from("internal error: the merged config does not parse"))?;
    let account = doc.get("accounts").and_then(|accounts| accounts.get(name));
    let resolved = account
        .and_then(|account| account.get("email"))
        .is_some_and(toml::Value::is_str)
        && account
            .and_then(|account| account.get("imap"))
            .and_then(|imap| imap.get("server"))
            .is_some_and(toml::Value::is_str)
        && account
            .and_then(|account| account.get("smtp"))
            .and_then(|smtp| smtp.get("server"))
            .is_some_and(toml::Value::is_str);
    if resolved {
        Ok(())
    } else {
        Err(String::from(
            "internal error: the saved account does not resolve; please report this",
        ))
    }
}

/// Whether any account other than `except` is marked `default = true`.
fn other_account_has_default(doc: &DocumentMut, except: &str) -> bool {
    let Some(accounts) = doc.get("accounts").and_then(Item::as_table) else {
        return false;
    };
    accounts
        .iter()
        .filter(|(name, _)| *name != except)
        .any(|(_, account)| {
            account
                .get("default")
                .and_then(Item::as_bool)
                .unwrap_or(false)
        })
}

/// The account names of a parsed document, in file order.
fn account_names(doc: &DocumentMut) -> Vec<String> {
    doc.get("accounts")
        .and_then(Item::as_table)
        .map(|table| table.iter().map(|(name, _)| name.to_string()).collect())
        .unwrap_or_default()
}

/// Unix: group/world permission bits set.
#[cfg(unix)]
fn shared_readable(permissions: Option<&std::fs::Permissions>) -> bool {
    use std::os::unix::fs::PermissionsExt;

    permissions
        .map(|permissions| permissions.mode() & 0o077 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn shared_readable(_permissions: Option<&std::fs::Permissions>) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gmail_raw_draft(name: &str) -> DraftAccount {
        DraftAccount {
            name: name.to_string(),
            email: "some-email@gmail.com".to_string(),
            display_name: Some("Vasya Pupkin".to_string()),
            imap_server: "imaps://imap.gmail.com:993".to_string(),
            imap_starttls: false,
            smtp_server: "smtps://smtp.gmail.com:465".to_string(),
            smtp_starttls: false,
            username: "some-email@gmail.com".to_string(),
            secret: SecretStorage::Raw("app-password".to_string()),
            aliases: vec![
                ("inbox".into(), "INBOX".into()),
                ("sent".into(), "[Gmail]/Sent Mail".into()),
                ("drafts".into(), "[Gmail]/Drafts".into()),
                ("trash".into(), "[Gmail]/Trash".into()),
                ("archive".into(), "[Gmail]/All Mail".into()),
            ],
        }
    }

    #[test]
    fn fresh_save_pins_the_target_shape_byte_for_byte() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");

        let report = save_account(&path, &gmail_raw_draft("gmail")).expect("save succeeds");

        assert!(report.created);
        assert_eq!(report.permissions_warning, None);
        let text = std::fs::read_to_string(&path).expect("written");
        let expected = "\
[accounts.gmail]
default = true
email = \"some-email@gmail.com\"
display-name = \"Vasya Pupkin\"

imap.server = \"imaps://imap.gmail.com:993\"
imap.sasl.plain.username = \"some-email@gmail.com\"
imap.sasl.plain.password.raw = \"app-password\"

smtp.server = \"smtps://smtp.gmail.com:465\"
smtp.sasl.plain.username = \"some-email@gmail.com\"
smtp.sasl.plain.password.raw = \"app-password\"

mailbox.alias.inbox = \"INBOX\"
mailbox.alias.sent = \"[Gmail]/Sent Mail\"
mailbox.alias.drafts = \"[Gmail]/Drafts\"
mailbox.alias.trash = \"[Gmail]/Trash\"
mailbox.alias.archive = \"[Gmail]/All Mail\"
";
        assert_eq!(text, expected, "fresh output must match the ADR §1 shape");
    }

    #[test]
    fn fresh_file_gets_mode_0600_on_unix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");

        save_account(&path, &gmail_raw_draft("gmail")).expect("save succeeds");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "fresh config must be 0600");
        }
    }

    #[test]
    fn merge_preserves_comments_tmail_tables_and_other_accounts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "\
# my personal tweaks
[tmail]
mouse = true

[accounts.old]
default = true
email = \"old@example.com\"
imap.server = \"imap://old.example.com:143\"
",
        )
        .expect("seed");

        let mut draft = gmail_raw_draft("gmail");
        draft.display_name = None;
        save_account(&path, &draft).expect("save succeeds");

        let text = std::fs::read_to_string(&path).expect("written");
        assert!(text.contains("# my personal tweaks"), "comments survive");
        assert!(text.contains("[tmail]\nmouse = true"), "[tmail] survives");
        assert!(text.contains("[accounts.old]"), "existing account survives");
        assert!(text.contains("[accounts.gmail]"), "new account merged");
        // The pre-existing default stays authoritative: no `default`
        // line in the new block.
        let gmail_block = text.split("[accounts.gmail]").nth(1).expect("block");
        assert!(
            !gmail_block.starts_with("\ndefault = true"),
            "new account must not steal the default flag"
        );
        assert!(
            !gmail_block.contains("display-name"),
            "omitted display name"
        );

        // And the merged file still parses with the new account intact.
        let (config, issues) = parse_with_issues(&text, Some(path.clone()));
        assert!(issues.is_empty(), "merged file parses cleanly: {issues:?}");
        assert_eq!(config.account.as_deref(), Some("old"));
    }

    #[test]
    fn replacing_the_default_account_keeps_it_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "\
[accounts.gmail]
default = true
email = \"stale@gmail.com\"
imap.server = \"imaps://imap.gmail.com:993\"

[accounts.work]
email = \"work@example.com\"
imap.server = \"imaps://imap.example.com:993\"
",
        )
        .expect("seed");

        save_account(&path, &gmail_raw_draft("gmail")).expect("save succeeds");

        let text = std::fs::read_to_string(&path).expect("written");
        let gmail_block = text.split("[accounts.gmail]").nth(1).expect("block");
        assert!(
            gmail_block.contains("default = true"),
            "replacing the default account must keep the default flag"
        );
        assert!(text.contains("[accounts.work]"), "other account survives");
        assert!(
            !text.contains("stale@gmail.com"),
            "the old block is replaced wholesale"
        );
    }

    #[test]
    fn collision_suffix_names_are_suggested() {
        // The shared collision rule lives in this module (ADR 0003 §3.6).
        use super::next_free_name;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[accounts.gmail]\nemail = \"a@gmail.com\"\n").expect("seed");

        let names = existing_account_names(&path);
        assert_eq!(names, vec!["gmail"]);

        // The wizard's suffix rule: gmail → gmail-2.
        let next = next_free_name(&names, "gmail");
        assert_eq!(next, "gmail-2");

        save_account(&path, &gmail_raw_draft("gmail-2")).expect("save succeeds");
        let text = std::fs::read_to_string(&path).expect("written");
        assert!(text.contains("[accounts.gmail-2]"));
        // No pre-existing default anywhere: the new account takes it.
        assert_eq!(file_default_account(&path), Some(String::from("gmail-2")));
    }

    #[test]
    fn raw_secret_into_shared_readable_file_warns() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[accounts.old]\nemail = \"old@example.com\"\n").expect("seed");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        }

        let report = save_account(&path, &gmail_raw_draft("gmail")).expect("save succeeds");

        #[cfg(unix)]
        assert!(
            report
                .permissions_warning
                .as_deref()
                .unwrap_or_default()
                .contains("chmod 600"),
            "raw secret into a shared-readable file must warn"
        );
        // The existing file's mode is untouched (no silent chmod).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path)
                    .expect("metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o644,
                "tmail must not chmod a user-owned file"
            );
        }

        // With a password.cmd no secret is stored and no warning fires.
        let mut cmd_draft = gmail_raw_draft("gmail");
        cmd_draft.secret = SecretStorage::Command("pass show mail/gmail".to_string());
        let cmd_path = dir.path().join("cmd.toml");
        let report = save_account(&cmd_path, &cmd_draft).expect("save succeeds");
        assert_eq!(report.permissions_warning, None);
        let cmd_text = std::fs::read_to_string(&cmd_path).expect("written");
        assert!(
            cmd_text.contains("imap.sasl.plain.password.cmd = \"pass show mail/gmail\""),
            "the command is stored verbatim"
        );
        assert!(!cmd_text.contains("password.raw"), "no raw key is written");
    }

    #[test]
    fn starttls_endpoints_write_the_flag() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let mut draft = gmail_raw_draft("dovecot");
        draft.imap_server = "imap://imap.example.com:143".to_string();
        draft.imap_starttls = true;
        draft.smtp_server = "smtp://smtp.example.com:587".to_string();
        draft.smtp_starttls = true;

        save_account(&path, &draft).expect("save succeeds");

        let text = std::fs::read_to_string(&path).expect("written");
        assert!(text.contains("imap.starttls = true"));
        assert!(text.contains("smtp.starttls = true"));
    }

    #[test]
    fn special_characters_in_secrets_and_names_are_escaped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let mut draft = gmail_raw_draft("tricky");
        draft.secret = SecretStorage::Raw("pass\"word\\with'specials\u{7}".to_string());

        save_account(&path, &draft).expect("save succeeds");

        // The file parses and the round-trip preserves the value.
        let text = std::fs::read_to_string(&path).expect("written");
        let doc: toml::Value = toml::from_str(&text).expect("merged file parses");
        let stored = doc["accounts"]["tricky"]["imap"]["sasl"]["plain"]["password"]["raw"]
            .as_str()
            .expect("raw password stored as a string");
        assert_eq!(stored, "pass\"word\\with'specials\u{7}");
    }

    #[test]
    fn account_name_sanitization_takes_the_first_label() {
        assert_eq!(sanitize_account_name("mail.example.com"), "mail");
        assert_eq!(sanitize_account_name("GMAIL.com"), "gmail");
        assert_eq!(
            sanitize_account_name("under_score.example.com"),
            "under-score"
        );
        assert_eq!(sanitize_account_name(""), "");
    }

    #[test]
    fn broken_existing_config_refuses_the_merge() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "not [valid toml").expect("seed");

        let err = save_account(&path, &gmail_raw_draft("gmail"))
            .expect_err("a broken file must refuse the merge");
        assert!(err.contains("not valid TOML"), "clear refusal: {err}");
        // The broken file is untouched.
        assert_eq!(
            std::fs::read_to_string(&path).expect("unchanged"),
            "not [valid toml"
        );
    }

    #[test]
    fn overwrite_is_atomic_and_preserves_mode_and_content() {
        // Ticket 12h7: the existing-file path must not overwrite in place
        // (a crash mid-write would corrupt the shared config), and the
        // target's mode must survive the replacement.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[accounts.old]\nemail = \"old@example.com\"\n").expect("seed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).expect("chmod");
        }

        save_account(&path, &gmail_raw_draft("gmail")).expect("save succeeds");

        // The merge landed…
        let text = std::fs::read_to_string(&path).expect("written");
        assert!(text.contains("[accounts.gmail]"));
        assert!(text.contains("[accounts.old]"));
        // …no temp sibling is left behind…
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .expect("dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.contains(".tmp-"))
            })
            .collect();
        assert!(leftovers.is_empty(), "temp files leaked: {leftovers:?}");
        // …and the mode survives the rename (ticket 12h7).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path)
                    .expect("metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o640,
                "the replacement must keep the original mode"
            );
        }
    }
}
