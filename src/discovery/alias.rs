//! Mailbox alias derivation for the account configuration wizard
//! (ADR 0003 §3.5).
//!
//! From the mailbox listing obtained during the credential test, this
//! derives the `mailbox.alias.*` role table so Tmail can address the
//! special folders (`Sent`, `Drafts`, …) portably. Two strategies:
//! the fixed Gmail well-known-folder preset, and generic name
//! heuristics. Roles whose mailbox does not exist are simply omitted —
//! the config parser tolerates a partial alias table.

use super::Provider;

/// The special-mailbox roles Tmail aliases, in derivation priority
/// order. These are the role keys of himalaya's
/// `mailbox.alias.<role>` table.
pub const ALIAS_ROLES: [&str; 5] = ["inbox", "sent", "drafts", "trash", "archive"];

/// Gmail's fixed well-known folders: `(role, mailbox name)`. `inbox`
/// is always emitted (`INBOX` is mandated by IMAP), the rest only
/// when the listing contains the folder.
const GMAIL_PRESET: [(&str, &str); 4] = [
    ("sent", "[Gmail]/Sent Mail"),
    ("drafts", "[Gmail]/Drafts"),
    ("trash", "[Gmail]/Trash"),
    ("archive", "[Gmail]/All Mail"),
];

/// Generic role aliases, tried case-insensitively against the listing
/// in declaration order; the first matching name per role wins.
const GENERIC_ALIASES: [(&str, &[&str]); 5] = [
    ("inbox", &["INBOX"]),
    ("sent", &["sent", "sent items", "sent messages"]),
    ("drafts", &["drafts", "draft"]),
    (
        "trash",
        &["trash", "deleted", "deleted items", "deleted messages"],
    ),
    ("archive", &["archive", "all mail"]),
];

/// Derives `(role, mailbox name)` pairs from a mailbox listing
/// (names only). Gmail accounts (discovery tag or `imap.gmail.com`
/// host, decided by the caller) use the fixed preset; everything else
/// uses name heuristics. Every mailbox name is claimed by at most one
/// role and every role appears at most once.
pub fn derive_aliases(
    mailbox_names: &[String],
    provider: Option<Provider>,
) -> Vec<(String, String)> {
    if provider == Some(Provider::Gmail) {
        gmail_aliases(mailbox_names)
    } else {
        generic_aliases(mailbox_names)
    }
}

/// The Gmail preset: `inbox = "INBOX"` unconditionally, the fixed
/// well-known folders only when they actually exist in the listing.
fn gmail_aliases(mailbox_names: &[String]) -> Vec<(String, String)> {
    let mut aliases = vec![("inbox".to_string(), "INBOX".to_string())];

    for (role, name) in GMAIL_PRESET {
        if mailbox_names.iter().any(|candidate| candidate == name) {
            aliases.push((role.to_string(), name.to_string()));
        }
    }

    aliases
}

/// Generic heuristics: case-insensitive exact-name matching, roles
/// processed in [`ALIAS_ROLES`] priority order, first unclaimed match
/// per role wins.
fn generic_aliases(mailbox_names: &[String]) -> Vec<(String, String)> {
    let mut aliases = Vec::new();
    let mut claimed: Vec<&str> = Vec::new();

    for (role, candidates) in GENERIC_ALIASES {
        let found = mailbox_names.iter().find(|name| {
            !claimed.contains(&name.as_str())
                && candidates
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(name))
        });
        if let Some(name) = found {
            claimed.push(name);
            aliases.push((role.to_string(), name.clone()));
        }
    }

    aliases
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn gmail_preset_maps_the_well_known_folders() {
        let listing = names(&[
            "INBOX",
            "[Gmail]/Sent Mail",
            "[Gmail]/Drafts",
            "[Gmail]/Trash",
            "[Gmail]/All Mail",
        ]);

        let aliases = derive_aliases(&listing, Some(Provider::Gmail));

        assert_eq!(
            aliases,
            vec![
                ("inbox".into(), "INBOX".into()),
                ("sent".into(), "[Gmail]/Sent Mail".into()),
                ("drafts".into(), "[Gmail]/Drafts".into()),
                ("trash".into(), "[Gmail]/Trash".into()),
                ("archive".into(), "[Gmail]/All Mail".into()),
            ]
        );
    }

    #[test]
    fn gmail_preset_keeps_only_existing_folders_but_always_inbox() {
        let listing = names(&["INBOX", "[Gmail]/Starred"]);

        let aliases = derive_aliases(&listing, Some(Provider::Gmail));

        assert_eq!(aliases, vec![("inbox".into(), "INBOX".into())]);
    }

    #[test]
    fn fastmail_style_listing_maps_by_exact_names() {
        let listing = names(&["Inbox", "Sent", "Drafts", "Trash", "Archive"]);

        let aliases = derive_aliases(&listing, None);

        assert_eq!(
            aliases,
            vec![
                ("inbox".into(), "Inbox".into()),
                ("sent".into(), "Sent".into()),
                ("drafts".into(), "Drafts".into()),
                ("trash".into(), "Trash".into()),
                ("archive".into(), "Archive".into()),
            ]
        );
    }

    #[test]
    fn bare_dovecot_listing_maps_case_insensitively() {
        let listing = names(&["INBOX", "Sent Messages", "Deleted Messages"]);

        let aliases = derive_aliases(&listing, None);

        assert_eq!(
            aliases,
            vec![
                ("inbox".into(), "INBOX".into()),
                ("sent".into(), "Sent Messages".into()),
                ("trash".into(), "Deleted Messages".into()),
            ]
        );
    }

    #[test]
    fn first_match_wins_and_claimed_mailboxes_are_skipped() {
        // Both "Trash" and "Deleted Items" match the trash role: the
        // first listed claims the role; "Deleted Items" is never
        // re-claimed by a later role.
        let listing = names(&["INBOX", "Deleted Items", "Trash"]);

        let aliases = derive_aliases(&listing, None);

        assert_eq!(
            aliases,
            vec![
                ("inbox".into(), "INBOX".into()),
                ("trash".into(), "Deleted Items".into()),
            ]
        );
    }

    #[test]
    fn unmatched_roles_are_omitted() {
        let listing = names(&["Everything", "Elsewhere"]);

        let aliases = derive_aliases(&listing, None);

        assert!(aliases.is_empty());
    }

    #[test]
    fn empty_listing_yields_no_aliases() {
        assert!(derive_aliases(&[], None).is_empty());
        // Gmail always pins inbox even on an empty listing.
        assert_eq!(
            derive_aliases(&[], Some(Provider::Gmail)),
            vec![("inbox".into(), "INBOX".into())]
        );
    }
}
