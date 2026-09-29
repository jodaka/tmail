//! Account operations (ADR 0003): the configuration wizard's settings
//! discovery, credential test, and config-file save. One family of the
//! operation vocabulary — see [`super::OperationKind`].

/// The wizard account operation to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountOperation {
    /// Discover IMAP/SMTP settings for an email address with the
    /// io-pim-discovery adapter (ADR 0003 §3.3). Runs on a worker thread,
    /// bounded by the adapter's deadline; POP/JMAP results never appear.
    DiscoverConfig { email: String },
    /// Validate the wizard's draft account with a real `himalaya mailbox
    /// list` against a temporary 0600 config file (ADR 0003 §3.4). The
    /// real config is untouched; the temp file is deleted in all
    /// outcomes. Boxed: the draft carries the credentials.
    TestAccount {
        draft: Box<crate::app::wizard::DraftAccountConfig>,
    },
    /// Merge the confirmed draft account into the resolved config file
    /// (ADR 0003 §3.6): format-preserving toml_edit edit, fresh files
    /// created 0600. Runs in the manager (file I/O) so the reducer stays
    /// I/O-free. `create` carries the `(role, folder)` pairs for the
    /// special mailboxes the server lacks (issue txps): the manager
    /// provisions them first (best effort) and only the ones the server
    /// confirms join the draft's alias table.
    SaveAccount {
        path: std::path::PathBuf,
        draft: Box<crate::app::wizard::DraftAccountConfig>,
        create: Vec<(String, String)>,
    },
}

impl AccountOperation {
    /// Human-readable label for the status bar and error modal.
    pub(crate) fn summary(&self) -> &'static str {
        match self {
            AccountOperation::DiscoverConfig { .. } => "Detecting settings",
            AccountOperation::TestAccount { .. } => "Testing account",
            AccountOperation::SaveAccount { .. } => "Saving account",
        }
    }

    /// Whether `newer` supersedes `older` (plan §11): wizard work
    /// supersedes its own kind — a re-run discovery (`r`) or a retried
    /// credential test replaces the still-running previous attempt
    /// (ADR 0003 §3.2).
    pub(crate) fn supersedes(newer: &AccountOperation, older: &AccountOperation) -> bool {
        match (newer, older) {
            (
                AccountOperation::DiscoverConfig { email: newer },
                AccountOperation::DiscoverConfig { email: older },
            ) => newer == older,
            (AccountOperation::TestAccount { .. }, AccountOperation::TestAccount { .. }) => true,
            _ => false,
        }
    }

    /// No account operation coalesces duplicates.
    pub(crate) fn duplicates_of(&self, _older: &AccountOperation) -> bool {
        false
    }

    /// Whether `Esc` may cancel the operation (plan §11): yes — a
    /// cancelled wizard step just keeps the wizard open on the same
    /// screen.
    pub(crate) fn is_cancellable(&self) -> bool {
        true
    }

    /// Whether the dispatch runs a mail-backend child process (issue
    /// 1v38): the credential test drives a real `himalaya mailbox list`
    /// against a temporary config, and the save provisions mailboxes the
    /// same way — as before the family split, all three count as backend
    /// work and queue behind the permit pool.
    pub(crate) fn uses_backend_process(&self) -> bool {
        true
    }
}
