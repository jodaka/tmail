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
}
