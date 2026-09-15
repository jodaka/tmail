//! Himalaya argv construction (ADR 0001 findings table).
//!
//! Global flags come first (`-c <config>`, `-a <account>`), then the
//! subcommand. All operations pass the mailbox explicitly with `-m` (ADR
//! 0001 finding 7) and request JSON with `--json`. Pagination is 1-based on
//! the wire: `p = offset/limit + 1`, `s = limit` (ADR 0001 finding 8).

use std::path::Path;

/// Pushes every element onto `argv` as one `String`. Mixed literals and
/// computed values (`&str`, `&String`, numbers) become argv entries without
/// the ten copies of `extend([...].into_iter().map(String::from))`.
macro_rules! args {
    ($argv:expr, $($arg:expr),+ $(,)?) => {
        $($argv.push($arg.to_string());)+
    };
}

/// `mailbox list --json` with the per-mailbox counters when `counts` is
/// set (`--counts` populates total and unread; maildir does not implement
/// counts yet, in which case the fields stay `None` and the sidebar
/// degrades gracefully). The wizard credential test omits it.
pub(crate) fn mailbox_list_argv(
    config: Option<&Path>,
    account: Option<&str>,
    counts: bool,
) -> Vec<String> {
    let mut argv = global_flags(config, account);
    args!(argv, "mailbox", "list", "--json");
    if counts {
        args!(argv, "--counts");
    }
    argv
}

/// Shared shape of `envelope {list,search} -m <mailbox> -p <page>
/// -s <size> --json`: the two calls differ only in the subcommand word and
/// (for search) the trailing query.
fn envelope_page_argv(
    config: Option<&Path>,
    account: Option<&str>,
    subcommand: &str,
    mailbox_id: &str,
    page_number: usize,
    page_size: usize,
) -> Vec<String> {
    let mut argv = global_flags(config, account);
    args!(
        argv,
        "envelope",
        subcommand,
        "-m",
        mailbox_id,
        "-p",
        page_number,
        "-s",
        page_size,
        "--json"
    );
    argv
}

/// `envelope list -m <mailbox> -p <page> -s <size> --json`
pub(crate) fn envelope_list_argv(
    config: Option<&Path>,
    account: Option<&str>,
    mailbox_id: &str,
    page_number: usize,
    page_size: usize,
) -> Vec<String> {
    envelope_page_argv(config, account, "list", mailbox_id, page_number, page_size)
}

/// The predicates and connectors of the himalaya 2.1.0 search DSL,
/// verified against the binary: lowercase keywords, space-separated
/// values, `and`/`or` connectors, parentheses for grouping, and quoted
/// values with `\"` escapes. There is no all-fields `text` predicate.
const SEARCH_PREDICATES: [&str; 10] = [
    "from", "to", "subject", "body", "flag", "not", "date", "after", "and", "or",
];

/// Gmail-style search (plan §16, user feedback): a query with no DSL
/// predicate is full text — the user types `plati` and Tmail matches
/// sender, subject, or body. Because this himalaya version has no `text`
/// predicate, that becomes an `or` chain over the three fields, with the
/// whole query as one quoted value (spaces stay inside the value; embedded
/// quotes are `\"`-escaped, verified on 2.1.0). Queries that already look
/// like the DSL pass through unchanged, so hand-written filters keep
/// working; an empty query stays empty (it matches everything).
pub(crate) fn normalize_search_query(query: &str) -> String {
    let query = query.trim();
    if query.is_empty() {
        return String::new();
    }
    let tokens: Vec<&str> = query.split_whitespace().collect();
    // A keyword anywhere ahead of a value token means the user is writing
    // the DSL by hand. A keyword in final position is just a word being
    // searched (`hello from` → full text for the words, not a broken
    // filter).
    let looks_like_dsl = tokens
        .iter()
        .take(tokens.len().saturating_sub(1))
        .any(|token| SEARCH_PREDICATES.contains(token));
    if looks_like_dsl {
        return query.to_owned();
    }
    let quoted = format!("\"{}\"", query.replace('"', "\\\""));
    format!("(from {quoted}) or (subject {quoted}) or (body {quoted})")
}

/// `envelope search -m <mailbox> -p <page> -s <size> --json <query>`
/// (Phase 9). All flags precede the query: himalaya parses *every*
/// trailing positional as the shared search DSL, so the query must be the
/// single final argv entry — and it travels as one entry, never through a
/// shell, so special characters stay one argument (verified on himalaya
/// 2.1.0: `envelope search -m Inbox -p 1 -s 20 --json "from x"`; an empty
/// query is valid backend behavior and matches everything). Bare text is
/// normalized into an any-field match — see [`normalize_search_query`].
pub(crate) fn envelope_search_argv(
    config: Option<&Path>,
    account: Option<&str>,
    mailbox_id: &str,
    query: &str,
    page_number: usize,
    page_size: usize,
) -> Vec<String> {
    let mut argv = envelope_page_argv(
        config,
        account,
        "search",
        mailbox_id,
        page_number,
        page_size,
    );
    argv.push(normalize_search_query(query));
    argv
}

/// `message read -m <mailbox> <id> --json` (ADR 0001 finding 10).
pub(crate) fn message_read_argv(
    config: Option<&Path>,
    account: Option<&str>,
    mailbox_id: &str,
    id: &str,
) -> Vec<String> {
    let mut argv = global_flags(config, account);
    args!(argv, "message", "read", "-m", mailbox_id, id, "--json");
    argv
}

/// `flag {add,remove} -m <mailbox> --flag <flag> <id>... --json`. Without
/// `--json` the flag commands print human text; with it they emit
/// `{"flags":[…]}` (ADR 0001 finding 6, corrected in the Phase 4 probe:
/// plain text on stdout otherwise). Output shape is validated, not
/// interpreted — success is the exit status. The ids travel as one argv
/// run: himalaya applies them in a single IMAP session, so a bulk flag
/// change never fans out into one login per message (ticket aavy).
pub(crate) fn flag_argv(
    config: Option<&Path>,
    account: Option<&str>,
    add: bool,
    mailbox_id: &str,
    flag: &str,
    ids: &[&str],
) -> Vec<String> {
    let mut argv = global_flags(config, account);
    args!(
        argv,
        "flag",
        if add { "add" } else { "remove" },
        "-m",
        mailbox_id,
        "--flag",
        flag
    );
    argv.extend(ids.iter().map(|id| String::from(*id)));
    argv.push(String::from("--json"));
    argv
}

/// `message move --from <source> --to <target> <id> --json`. `--to` accepts
/// the resolved target mailbox name/id; archive resolution happens in the
/// adapter (ADR 0001: semantic operations map inside the backend).
pub(crate) fn message_move_argv(
    config: Option<&Path>,
    account: Option<&str>,
    source: &str,
    target: &str,
    id: &str,
) -> Vec<String> {
    let mut argv = global_flags(config, account);
    args!(
        argv, "message", "move", "--from", source, "--to", target, id, "--json"
    );
    argv
}

/// `message delete -m <mailbox> <id> --json` (trash-first, ADR 0001
/// finding 5). Without `--json` the output is human text.
pub(crate) fn message_delete_argv(
    config: Option<&Path>,
    account: Option<&str>,
    mailbox_id: &str,
    id: &str,
) -> Vec<String> {
    let mut argv = global_flags(config, account);
    args!(argv, "message", "delete", "-m", mailbox_id, id, "--json");
    argv
}

/// `message add -m <mailbox> --flag draft --json` with the raw RFC 5322
/// message piped on stdin (ADR 0002 findings: create returns
/// `{"id":"…","sent":false}`; there is no in-place update, replacement is
/// add-then-delete).
pub(crate) fn message_add_argv(
    config: Option<&Path>,
    account: Option<&str>,
    mailbox_id: &str,
    flag: &str,
) -> Vec<String> {
    let mut argv = global_flags(config, account);
    args!(
        argv, "message", "add", "-m", mailbox_id, "--flag", flag, "--json"
    );
    argv
}

/// `message send --json` with the raw RFC 5322 message piped on stdin
/// (ADR 0001 findings table; fixtures/himalaya/send-outcomes.md: success
/// prints `{"message":"Message successfully sent"}`, failures are JSON
/// errors with exit 1 — including errors that may mean the message was
/// already delivered, which Tmail classifies in the adapter).
pub(crate) fn message_send_argv(config: Option<&Path>, account: Option<&str>) -> Vec<String> {
    let mut argv = global_flags(config, account);
    args!(argv, "message", "send", "--json");
    argv
}

/// `attachment download -m <mailbox> -d <dir> <message-id> <part-id>
/// --json` (Phase 8.4, ADR 0001). The destination directory is passed as
/// one argv entry — paths with spaces never see a shell — and Tmail points
/// it at a private tempdir so collision handling stays in Tmail's hands.
pub(crate) fn attachment_download_argv(
    config: Option<&Path>,
    account: Option<&str>,
    mailbox_id: &str,
    message_id: &str,
    part_id: usize,
    dir: &Path,
) -> Vec<String> {
    let mut argv = global_flags(config, account);
    args!(
        argv,
        "attachment",
        "download",
        "-m",
        mailbox_id,
        "-d",
        dir.display(),
        message_id,
        part_id,
        "--json"
    );
    argv
}

fn global_flags(config: Option<&Path>, account: Option<&str>) -> Vec<String> {
    let mut argv = Vec::new();
    if let Some(config) = config {
        argv.push(String::from("-c"));
        argv.push(config.display().to_string());
    }
    if let Some(account) = account {
        argv.push(String::from("-a"));
        argv.push(account.to_owned());
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn mailbox_list_with_config_and_account() {
        let argv = mailbox_list_argv(Some(Path::new("/tmp/cfg.toml")), Some("probe"), true);
        assert_eq!(
            argv,
            vec![
                "-c",
                "/tmp/cfg.toml",
                "-a",
                "probe",
                "mailbox",
                "list",
                "--json",
                "--counts"
            ]
        );
    }

    #[test]
    fn envelope_list_maps_offset_and_limit_to_1based_page() {
        let argv = envelope_list_argv(
            Some(Path::new("/tmp/cfg.toml")),
            Some("probe"),
            "INBOX",
            3,
            20,
        );
        assert_eq!(
            argv,
            vec![
                "-c",
                "/tmp/cfg.toml",
                "-a",
                "probe",
                "envelope",
                "list",
                "-m",
                "INBOX",
                "-p",
                "3",
                "-s",
                "20",
                "--json",
            ]
        );
    }

    #[test]
    fn flags_omitted_when_unset() {
        let argv = mailbox_list_argv(None, None, true);
        assert_eq!(argv, vec!["mailbox", "list", "--json", "--counts"]);
        let argv = envelope_list_argv(None, None, "My Folder", 1, 20);
        assert_eq!(argv[0], "envelope");
        assert_eq!(argv[3], "My Folder");
    }

    #[test]
    fn config_paths_with_spaces_stay_single_argv_entries() {
        let path = PathBuf::from("/tmp/some dir/my config.toml");
        let mailbox_list_argv = mailbox_list_argv(Some(&path), None, true);
        assert_eq!(mailbox_list_argv[1], "/tmp/some dir/my config.toml");
    }

    #[test]
    fn nonascii_queries_travel_unchanged_through_normalization() {
        // Ticket fjhg: cyrillic (and any non-ASCII) text must reach
        // himalaya byte-for-byte. The reported BAD "Could not parse
        // command" is an upstream himalaya limitation (pimalaya/himalaya
        // #635, fixed by pimalaya/imap-client#23: SEARCH was sent without
        // CHARSET UTF-8); Tmail neither corrupts nor rewrites the query.
        let normalized = normalize_search_query("Аэрофлот");
        assert!(normalized.contains("Аэрофлот"), "{normalized}");
        assert_eq!(normalized.matches("Аэрофлот").count(), 3);
        // A DSL-looking query with non-ASCII values passes through as is.
        assert_eq!(
            normalize_search_query("subject Аэрофлот"),
            "subject Аэрофлот"
        );
    }

    #[test]
    fn envelope_search_argv_is_exact_and_query_comes_last() {
        // All flags precede the query: himalaya parses every trailing
        // positional as the search DSL (verified on 2.1.0), so the query
        // must be the single final argv entry.
        let argv = envelope_search_argv(
            Some(Path::new("/tmp/cfg.toml")),
            Some("gmail"),
            "Inbox",
            "from example and subject quote",
            2,
            20,
        );
        assert_eq!(
            argv,
            vec![
                "-c",
                "/tmp/cfg.toml",
                "-a",
                "gmail",
                "envelope",
                "search",
                "-m",
                "Inbox",
                "-p",
                "2",
                "-s",
                "20",
                "--json",
                "from example and subject quote",
            ]
        );
        // Special characters travel as one argv entry, never a shell line.
        let argv = envelope_search_argv(None, None, "Inbox", "subject \"quoted (x)\"", 1, 20);
        assert_eq!(
            argv.last().map(String::as_str),
            Some("subject \"quoted (x)\"")
        );
        // Bare text becomes the any-field OR chain (Gmail-style search).
        let argv = envelope_search_argv(None, None, "Inbox", "plati", 1, 20);
        assert_eq!(
            argv.last().map(String::as_str),
            Some("(from \"plati\") or (subject \"plati\") or (body \"plati\")")
        );
    }

    #[test]
    fn bare_text_becomes_an_any_field_or_chain() {
        assert_eq!(
            normalize_search_query("plati"),
            "(from \"plati\") or (subject \"plati\") or (body \"plati\")"
        );
        // Multi-word input stays one quoted value (verified on 2.1.0:
        // `body "hello world"` matches the phrase).
        assert_eq!(
            normalize_search_query("hello world"),
            "(from \"hello world\") or (subject \"hello world\") or (body \"hello world\")"
        );
        // Whitespace is trimmed; an empty query stays empty (it matches
        // everything — wrapping it would match nothing).
        assert_eq!(
            normalize_search_query("  plati  "),
            normalize_search_query("plati")
        );
        assert_eq!(normalize_search_query(""), "");
        assert_eq!(normalize_search_query("   "), "");
    }

    #[test]
    fn handwritten_dsl_queries_pass_through_unchanged() {
        // Predicate keyword ahead of a value: the user is writing the DSL.
        for query in [
            "from bob",
            "body \"hello world\"",
            "not from bob",
            "from example and subject quote",
            "(from bob) or (subject bob)",
        ] {
            assert_eq!(normalize_search_query(query), query);
        }
    }

    #[test]
    fn keywords_in_value_position_are_searched_not_parsed() {
        // A keyword in final position has no value: searching for the
        // literal word must still work.
        assert_eq!(
            normalize_search_query("hello from"),
            "(from \"hello from\") or (subject \"hello from\") or (body \"hello from\")"
        );
        assert_eq!(
            normalize_search_query("from"),
            "(from \"from\") or (subject \"from\") or (body \"from\")"
        );
    }

    #[test]
    fn embedded_quotes_are_escaped_for_the_dsl() {
        assert_eq!(
            normalize_search_query("say \"hi\""),
            "(from \"say \\\"hi\\\"\") or (subject \"say \\\"hi\\\"\") or (body \"say \\\"hi\\\"\")"
        );
    }

    #[test]
    fn message_read_argv_is_exact() {
        let argv = message_read_argv(
            Some(Path::new("/tmp/cfg.toml")),
            Some("probe"),
            "INBOX",
            "1788343420.M446833P1967Q1.RFT-R993YF",
        );
        assert_eq!(
            argv,
            vec![
                "-c",
                "/tmp/cfg.toml",
                "-a",
                "probe",
                "message",
                "read",
                "-m",
                "INBOX",
                "1788343420.M446833P1967Q1.RFT-R993YF",
                "--json",
            ]
        );
    }

    #[test]
    fn flag_argv_switches_add_and_remove() {
        let add = flag_argv(None, None, true, "INBOX", "seen", &["env-1"]);
        assert_eq!(
            add,
            vec![
                "flag", "add", "-m", "INBOX", "--flag", "seen", "env-1", "--json"
            ]
        );
        let remove = flag_argv(None, None, false, "INBOX", "flagged", &["env-1"]);
        assert_eq!(
            remove,
            vec![
                "flag", "remove", "-m", "INBOX", "--flag", "flagged", "env-1", "--json"
            ]
        );
    }

    #[test]
    fn flag_argv_carries_every_id_in_one_invocation() {
        // The batch is the point (ticket aavy): one argv run, one IMAP
        // session, no per-message fanout.
        let argv = flag_argv(
            None,
            None,
            true,
            "INBOX",
            "seen",
            &["env-1", "env-2", "env-3"],
        );
        assert_eq!(
            argv,
            vec![
                "flag", "add", "-m", "INBOX", "--flag", "seen", "env-1", "env-2", "env-3", "--json"
            ]
        );
    }

    #[test]
    fn message_move_argv_uses_from_and_to() {
        let argv = message_move_argv(None, None, "INBOX", "/root/maildir/Archive", "env-1");
        assert_eq!(
            argv,
            vec![
                "message",
                "move",
                "--from",
                "INBOX",
                "--to",
                "/root/maildir/Archive",
                "env-1",
                "--json",
            ]
        );
    }

    #[test]
    fn message_delete_argv_is_exact() {
        let argv = message_delete_argv(None, None, "INBOX", "env-1");
        assert_eq!(
            argv,
            vec!["message", "delete", "-m", "INBOX", "env-1", "--json"]
        );
    }

    #[test]
    fn message_send_argv_is_exact() {
        let argv = message_send_argv(Some(Path::new("/tmp/cfg.toml")), Some("probe"));
        assert_eq!(
            argv,
            vec![
                "-c",
                "/tmp/cfg.toml",
                "-a",
                "probe",
                "message",
                "send",
                "--json",
            ]
        );
    }

    #[test]
    fn attachment_download_argv_is_exact_and_keeps_dirs_whole() {
        let argv = attachment_download_argv(
            Some(Path::new("/tmp/cfg.toml")),
            Some("probe"),
            "INBOX",
            "env-1",
            3,
            Path::new("/tmp/My Downloads/tmail-dl"),
        );
        assert_eq!(
            argv,
            vec![
                "-c",
                "/tmp/cfg.toml",
                "-a",
                "probe",
                "attachment",
                "download",
                "-m",
                "INBOX",
                "-d",
                "/tmp/My Downloads/tmail-dl",
                "env-1",
                "3",
                "--json",
            ]
        );
    }
}
