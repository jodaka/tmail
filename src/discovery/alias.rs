//! Mailbox alias derivation for the account configuration wizard
//! (ADR 0003 §3.5).
//!
//! From the mailbox listing obtained during the credential test, this
//! derives the `mailbox.alias.*` role table so Tmail can address the
//! special folders (`Sent`, `Drafts`, …) portably. Two strategies:
//! the fixed Gmail well-known-folder preset, and generic name
//! heuristics — both seeded with the roles the server itself
//! attributed (RFC 6154 `SPECIAL-USE` attributes, which resolve
//! localized or lookalike folder names authoritatively). Roles whose
//! mailbox does not exist are simply omitted — the config parser
//! tolerates a partial alias table.

use crate::backend::himalaya::map::alias_key_for_role;
use crate::domain::TestedMailbox;

use super::Provider;

/// The special-mailbox roles Tmail aliases, in derivation priority
/// order. These are the role keys of himalaya's
/// `mailbox.alias.<role>` table.
pub const ALIAS_ROLES: [&str; 6] = ["inbox", "sent", "drafts", "trash", "junk", "archive"];

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
const GENERIC_ALIASES: [(&str, &[&str]); 6] = [
    ("inbox", &["INBOX"]),
    ("sent", &["sent", "sent items", "sent messages"]),
    ("drafts", &["drafts", "draft"]),
    (
        "trash",
        &["trash", "deleted", "deleted items", "deleted messages"],
    ),
    ("junk", &["junk", "spam", "junk e-mail", "junk mail"]),
    ("archive", &["archive", "all mail"]),
];

/// Derives `(role, mailbox name)` pairs from the credential-test
/// listing. Gmail accounts (discovery tag or `imap.gmail.com` host,
/// decided by the caller) use the fixed preset; everything else takes
/// the server's own SPECIAL-USE roles first and falls back to name
/// heuristics. Every mailbox name is claimed by at most one role and
/// every role appears at most once.
pub fn derive_aliases(
    mailboxes: &[TestedMailbox],
    provider: Option<Provider>,
) -> Vec<(String, String)> {
    if provider == Some(Provider::Gmail) {
        gmail_aliases(mailboxes)
    } else {
        generic_aliases(mailboxes)
    }
}

/// The canonical English folder name the wizard provisions when a role
/// has no mailbox on the server (issue txps: a bare Dovecot exposes
/// `Inbox` alone). `inbox` is absent on purpose: INBOX is mandated by
/// RFC 3501 and never needs creating.
pub fn canonical_name(role: &str) -> Option<&'static str> {
    match role {
        "sent" => Some("Sent"),
        "drafts" => Some("Drafts"),
        "trash" => Some("Trash"),
        "junk" => Some("Junk"),
        "archive" => Some("Archive"),
        _ => None,
    }
}

/// The roles the listing could not resolve — no server attribute and
/// no well-known name — paired with the canonical folder name the
/// wizard creates for them at confirm time (issue txps). Inbox is
/// never offered: INBOX always exists.
pub fn missing_special_roles(
    mailboxes: &[TestedMailbox],
    provider: Option<Provider>,
) -> Vec<(String, String)> {
    let derived = derive_aliases(mailboxes, provider);
    ALIAS_ROLES
        .iter()
        .filter(|role| **role != "inbox")
        .filter(|role| !derived.iter().any(|(key, _)| key == *role))
        .filter_map(|role| canonical_name(role).map(|name| ((*role).to_string(), name.to_string())))
        .collect()
}

/// The Gmail preset: `inbox = "INBOX"` unconditionally, the fixed
/// well-known folders only when they actually exist in the listing.
/// Gmail's system ids are stable and its IMAP does not advertise
/// SPECIAL-USE for them, so the preset stays name-based.
fn gmail_aliases(mailboxes: &[TestedMailbox]) -> Vec<(String, String)> {
    let mut aliases = vec![("inbox".to_string(), "INBOX".to_string())];

    for (role, name) in GMAIL_PRESET {
        if mailboxes.iter().any(|mailbox| mailbox.name == name) {
            aliases.push((role.to_string(), name.to_string()));
        }
    }

    aliases
}

/// Generic derivation, roles processed in [`ALIAS_ROLES`] priority
/// order: the first mailbox the server attributed to the role claims
/// it (authoritative even when the name is a lookalike), else the
/// first unclaimed well-known name does.
fn generic_aliases(mailboxes: &[TestedMailbox]) -> Vec<(String, String)> {
    let mut aliases = Vec::new();
    let mut claimed: Vec<String> = Vec::new();

    for role in ALIAS_ROLES {
        // The server's own SPECIAL-USE attribute is authoritative: a
        // localized "Odstraněné" carrying `\Trash` must win over any
        // name-shaped guess.
        let attributed = mailboxes.iter().find(|mailbox| {
            !claimed.contains(&mailbox.name)
                && mailbox
                    .role
                    .is_some_and(|role_of| alias_key_for_role(role_of) == role)
        });
        if let Some(mailbox) = attributed {
            claimed.push(mailbox.name.clone());
            aliases.push((role.to_string(), mailbox.name.clone()));
            continue;
        }

        // Name heuristics, case-insensitive, over the still-unclaimed
        // mailboxes. Names already claimed by an earlier role never
        // serve a second role.
        let found = mailboxes.iter().find(|mailbox| {
            !claimed.contains(&mailbox.name)
                && GENERIC_ALIASES
                    .iter()
                    .find(|(candidate_role, candidates)| {
                        *candidate_role == role
                            && candidates
                                .iter()
                                .any(|candidate| candidate.eq_ignore_ascii_case(&mailbox.name))
                    })
                    .is_some()
        });
        if let Some(mailbox) = found {
            claimed.push(mailbox.name.clone());
            aliases.push((role.to_string(), mailbox.name.clone()));
        }
    }

    aliases
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::MailboxRole;

    /// A listing of plain, server-unattributed mailboxes (the shape the
    /// name heuristics alone consume).
    fn names(values: &[&str]) -> Vec<TestedMailbox> {
        values
            .iter()
            .map(|name| TestedMailbox {
                name: name.to_string(),
                role: None,
            })
            .collect()
    }

    fn named(name: &str, role: MailboxRole) -> TestedMailbox {
        TestedMailbox {
            name: name.to_string(),
            role: Some(role),
        }
    }

    fn plain(name: &str) -> TestedMailbox {
        TestedMailbox {
            name: name.to_string(),
            role: None,
        }
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

    #[test]
    fn special_use_attributes_map_localized_names() {
        // The reported failure class (issue m0wh): a server with
        // localized special folders no candidate list can name.
        let listing = vec![
            named("Vlastní složka", MailboxRole::Inbox),
            named("Odeslané", MailboxRole::Sent),
            named("Koncepty", MailboxRole::Drafts),
            named("Odstraněné", MailboxRole::Trash),
            plain("Projects"),
        ];

        let aliases = derive_aliases(&listing, None);

        assert_eq!(
            aliases,
            vec![
                ("inbox".into(), "Vlastní složka".into()),
                ("sent".into(), "Odeslané".into()),
                ("drafts".into(), "Koncepty".into()),
                ("trash".into(), "Odstraněné".into()),
            ]
        );
    }

    #[test]
    fn special_use_wins_over_lookalike_names() {
        // The server says the trash is "Odstraněné"; the lookalike
        // "Deleted" name never gets claimed by the heuristics, and no
        // other mailbox inherits the role.
        let listing = vec![named("Odstraněné", MailboxRole::Trash), plain("Deleted")];

        let aliases = derive_aliases(&listing, None);

        assert_eq!(aliases, vec![("trash".into(), "Odstraněné".into())]);
    }

    #[test]
    fn special_use_junk_claims_the_junk_role() {
        let listing = vec![named("Nevyžádaná", MailboxRole::Spam), plain("INBOX")];

        let aliases = derive_aliases(&listing, None);

        assert_eq!(
            aliases,
            vec![
                ("inbox".into(), "INBOX".into()),
                ("junk".into(), "Nevyžádaná".into()),
            ]
        );
    }

    #[test]
    fn first_attributed_row_wins_a_doubly_claimed_role() {
        // A server that echoes `\Trash` on two folders: the first row
        // claims the role, the second stays unclaimed (one role per
        // mailbox, one mailbox per role).
        let listing = vec![
            named("Trash A", MailboxRole::Trash),
            named("Trash B", MailboxRole::Trash),
        ];

        let aliases = derive_aliases(&listing, None);

        assert_eq!(aliases, vec![("trash".into(), "Trash A".into())]);
    }

    #[test]
    fn attributed_mailboxes_are_never_re_claimed_by_name() {
        // The server attributes \Archive to "Vše"; the lookalike "All
        // Mail" name must not steal the archive role from it.
        let listing = vec![named("Vše", MailboxRole::Archive), plain("All Mail")];

        let aliases = derive_aliases(&listing, None);

        assert_eq!(aliases, vec![("archive".into(), "Vše".into())]);
    }

    #[test]
    fn bare_dovecot_listing_offers_every_role_for_creation() {
        // The klinoteka class (issue txps): the server lists Inbox
        // alone, so every non-inbox role is missing and gets its
        // canonical English folder name.
        let missing = missing_special_roles(&names(&["Inbox"]), None);

        assert_eq!(
            missing,
            vec![
                ("sent".into(), "Sent".into()),
                ("drafts".into(), "Drafts".into()),
                ("trash".into(), "Trash".into()),
                ("junk".into(), "Junk".into()),
                ("archive".into(), "Archive".into()),
            ]
        );
    }

    #[test]
    fn fully_resolved_listing_offers_no_creation() {
        // Localized server (issue m0wh): all roles attribute; nothing
        // to create, inbox never offered.
        let listing = vec![
            named("Vlastní složka", MailboxRole::Inbox),
            named("Odeslané", MailboxRole::Sent),
            named("Koncepty", MailboxRole::Drafts),
            named("Odstraněné", MailboxRole::Trash),
            named("Nevyžádaná", MailboxRole::Spam),
            named("Vše", MailboxRole::Archive),
        ];

        assert!(missing_special_roles(&listing, None).is_empty());
    }

    #[test]
    fn partially_resolved_listing_offers_the_gaps_in_priority_order() {
        let missing = missing_special_roles(&names(&["Inbox", "Sent"]), None);

        assert_eq!(
            missing,
            vec![
                ("drafts".into(), "Drafts".into()),
                ("trash".into(), "Trash".into()),
                ("junk".into(), "Junk".into()),
                ("archive".into(), "Archive".into()),
            ]
        );
    }
}
