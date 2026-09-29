//! Attachment-file operations (plan §15, Phase 8): source validation,
//! the directory chooser, and saving a MIME part to disk. One family of
//! the operation vocabulary — see [`super::OperationKind`].

/// The attachment operation to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOperation {
    /// Validate one composer attachment source (plan §15, Phase 8): the
    /// path as typed (`~` unexpanded); the backend expands and checks it.
    /// No bytes travel — only the resulting metadata.
    ReadAttachment { path: std::path::PathBuf },
    /// List one directory for the attachment file chooser (plan §15,
    /// ticket 95x0): builds the explorer state for the target directory.
    /// `None` opens the chooser in the user's home directory (fallback:
    /// the working directory). Filesystem work in the manager keeps the
    /// reducer I/O-free.
    ListAttachmentFiles { path: Option<std::path::PathBuf> },
    /// Save one incoming attachment to disk (plan §15, Phase 8.4). The
    /// request freezes the target message, part, name, and directory;
    /// retries replay it verbatim. `open_after` chains the platform
    /// opener on the saved path (Phase 8.5).
    SaveAttachment {
        request: crate::domain::AttachmentRequest,
        open_after: bool,
    },
}

impl FileOperation {
    /// Human-readable label for the status bar and error modal.
    pub(crate) fn summary(&self) -> &'static str {
        match self {
            FileOperation::ReadAttachment { .. } => "Checking file",
            FileOperation::ListAttachmentFiles { .. } => "Listing files",
            FileOperation::SaveAttachment { .. } => "Saving attachment",
        }
    }

    /// Whether `newer` supersedes `older` (plan §11): a repeated
    /// validation of the same entry supersedes the one in flight (only
    /// the newest submit can win), and a newer directory listing
    /// supersedes the one in flight — navigation keeps moving, only the
    /// newest target can land.
    pub(crate) fn supersedes(newer: &FileOperation, older: &FileOperation) -> bool {
        match (newer, older) {
            (
                FileOperation::ReadAttachment { path: newer },
                FileOperation::ReadAttachment { path: older },
            ) => newer == older,
            (
                FileOperation::ListAttachmentFiles { path: newer },
                FileOperation::ListAttachmentFiles { path: older },
            ) => newer == older,
            _ => false,
        }
    }

    /// No file operation coalesces duplicates.
    pub(crate) fn duplicates_of(&self, _older: &FileOperation) -> bool {
        false
    }

    /// Whether `Esc` may cancel the operation (plan §11): yes — a
    /// cancelled validation/listing/save just leaves the composer as it
    /// was.
    pub(crate) fn is_cancellable(&self) -> bool {
        true
    }

    /// Whether the dispatch runs a mail-backend child process (issue
    /// 1v38): validation and saving go through the backend; the chooser
    /// listing is a local directory walk on the blocking pool.
    pub(crate) fn uses_backend_process(&self) -> bool {
        !matches!(self, FileOperation::ListAttachmentFiles { .. })
    }
}
