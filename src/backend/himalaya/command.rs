//! Himalaya argv construction (ADR 0001 findings table).
//!
//! Global flags come first (`-c <config>`, `-a <account>`), then the
//! subcommand. All operations pass the mailbox explicitly with `-m` (ADR
//! 0001 finding 7) and request JSON with `--json`. Pagination is 1-based on
//! the wire: `p = offset/limit + 1`, `s = limit` (ADR 0001 finding 8).

use std::path::Path;

/// `mailbox list --json`
pub(crate) fn mailbox_list_argv(config: Option<&Path>, account: Option<&str>) -> Vec<String> {
    let mut argv = global_flags(config, account);
    argv.extend(["mailbox", "list", "--json"].into_iter().map(String::from));
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
    let mut argv = global_flags(config, account);
    argv.extend(
        [
            "envelope",
            "list",
            "-m",
            mailbox_id,
            "-p",
            &page_number.to_string(),
            "-s",
            &page_size.to_string(),
            "--json",
        ]
        .into_iter()
        .map(String::from),
    );
    argv
}

/// `envelope search -m <mailbox> -p <page> -s <size> --json <query>`
/// (Phase 9). All flags precede the query: himalaya parses *every*
/// trailing positional as the shared search DSL, so the query must be the
/// single final argv entry — and it travels as one entry, never through a
/// shell, so special characters stay one argument (verified on himalaya
/// 2.1.0: `envelope search -m Inbox -p 1 -s 20 --json "from x"`; an empty
/// query is valid backend behavior and matches everything).
pub(crate) fn envelope_search_argv(
    config: Option<&Path>,
    account: Option<&str>,
    mailbox_id: &str,
    query: &str,
    page_number: usize,
    page_size: usize,
) -> Vec<String> {
    let mut argv = global_flags(config, account);
    argv.extend(
        [
            "envelope",
            "search",
            "-m",
            mailbox_id,
            "-p",
            &page_number.to_string(),
            "-s",
            &page_size.to_string(),
            "--json",
        ]
        .into_iter()
        .map(String::from),
    );
    argv.push(String::from(query));
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
    argv.extend(
        ["message", "read", "-m", mailbox_id, id, "--json"]
            .into_iter()
            .map(String::from),
    );
    argv
}

/// `flag {add,remove} -m <mailbox> --flag <flag> <id> --json`. Without
/// `--json` the flag commands print human text; with it they emit
/// `{"flags":[…]}` (ADR 0001 finding 6, corrected in the Phase 4 probe:
/// plain text on stdout otherwise). Output shape is validated, not
/// interpreted — success is the exit status.
pub(crate) fn flag_argv(
    config: Option<&Path>,
    account: Option<&str>,
    add: bool,
    mailbox_id: &str,
    flag: &str,
    id: &str,
) -> Vec<String> {
    let mut argv = global_flags(config, account);
    argv.extend(
        [
            "flag",
            if add { "add" } else { "remove" },
            "-m",
            mailbox_id,
            "--flag",
            flag,
            id,
            "--json",
        ]
        .into_iter()
        .map(String::from),
    );
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
    argv.extend(
        [
            "message", "move", "--from", source, "--to", target, id, "--json",
        ]
        .into_iter()
        .map(String::from),
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
    argv.extend(
        ["message", "delete", "-m", mailbox_id, id, "--json"]
            .into_iter()
            .map(String::from),
    );
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
    argv.extend(
        ["message", "add", "-m", mailbox_id, "--flag", flag, "--json"]
            .into_iter()
            .map(String::from),
    );
    argv
}

/// `message send --json` with the raw RFC 5322 message piped on stdin
/// (ADR 0001 findings table; fixtures/himalaya/send-outcomes.md: success
/// prints `{"message":"Message successfully sent"}`, failures are JSON
/// errors with exit 1 — including errors that may mean the message was
/// already delivered, which Post classifies in the adapter).
pub(crate) fn message_send_argv(config: Option<&Path>, account: Option<&str>) -> Vec<String> {
    let mut argv = global_flags(config, account);
    argv.extend(["message", "send", "--json"].into_iter().map(String::from));
    argv
}

/// `attachment download -m <mailbox> -d <dir> <message-id> <part-id>
/// --json` (Phase 8.4, ADR 0001). The destination directory is passed as
/// one argv entry — paths with spaces never see a shell — and Post points
/// it at a private tempdir so collision handling stays in Post's hands.
pub(crate) fn attachment_download_argv(
    config: Option<&Path>,
    account: Option<&str>,
    mailbox_id: &str,
    message_id: &str,
    part_id: usize,
    dir: &Path,
) -> Vec<String> {
    let mut argv = global_flags(config, account);
    argv.extend(
        [
            "attachment",
            "download",
            "-m",
            mailbox_id,
            "-d",
            &dir.display().to_string(),
            message_id,
            &part_id.to_string(),
            "--json",
        ]
        .into_iter()
        .map(String::from),
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
        let argv = mailbox_list_argv(Some(Path::new("/tmp/cfg.toml")), Some("probe"));
        assert_eq!(
            argv,
            vec![
                "-c",
                "/tmp/cfg.toml",
                "-a",
                "probe",
                "mailbox",
                "list",
                "--json"
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
        let argv = mailbox_list_argv(None, None);
        assert_eq!(argv, vec!["mailbox", "list", "--json"]);
        let argv = envelope_list_argv(None, None, "My Folder", 1, 20);
        assert_eq!(argv[0], "envelope");
        assert_eq!(argv[3], "My Folder");
    }

    #[test]
    fn config_paths_with_spaces_stay_single_argv_entries() {
        let path = PathBuf::from("/tmp/some dir/my config.toml");
        let argv = mailbox_list_argv(Some(&path), None);
        assert_eq!(argv[1], "/tmp/some dir/my config.toml");
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
        let add = flag_argv(None, None, true, "INBOX", "seen", "env-1");
        assert_eq!(
            add,
            vec![
                "flag", "add", "-m", "INBOX", "--flag", "seen", "env-1", "--json"
            ]
        );
        let remove = flag_argv(None, None, false, "INBOX", "flagged", "env-1");
        assert_eq!(
            remove,
            vec![
                "flag", "remove", "-m", "INBOX", "--flag", "flagged", "env-1", "--json"
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
            Path::new("/tmp/My Downloads/post-dl"),
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
                "/tmp/My Downloads/post-dl",
                "env-1",
                "3",
                "--json",
            ]
        );
    }
}
