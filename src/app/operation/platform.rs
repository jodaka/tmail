//! Platform operations: everything that leaves the terminal to the
//! user's desktop — the platform file handler, the web browser, and the
//! external editor (plan §14 Phase 11, plan §15 Phase 8.5, ticket hc9n).
//! One family of the operation vocabulary — see
//! [`super::OperationKind`].

/// The platform-handoff operation to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformOperation {
    /// Open a saved file with the platform handler (`open`/`xdg-open`,
    /// plan §15, Phase 8.5): spawned directly, never through a shell.
    OpenPath { path: std::path::PathBuf },
    /// Open a link from an HTML body in the platform browser (ticket
    /// hc9n): spawned directly, never through a shell, and only for the
    /// web schemes the opener policy accepts.
    OpenUrl { url: String },
    /// Hand the draft body to the configured external editor (plan §14,
    /// Phase 11): the runtime suspends the TUI, spawns `program` (argv
    /// only, no shell) on a secure temporary file, and waits for exit.
    /// Boxed strings keep the variant small.
    EditExternally { program: Vec<String>, body: String },
}

impl PlatformOperation {
    /// Human-readable label for the status bar and error modal.
    pub(crate) fn summary(&self) -> &'static str {
        match self {
            PlatformOperation::OpenPath { .. } => "Opening attachment",
            PlatformOperation::OpenUrl { .. } => "Opening link",
            PlatformOperation::EditExternally { .. } => "Editing externally",
        }
    }

    /// Platform handoffs never supersede anything: an already-spawned
    /// handler app or browser is let alone.
    pub(crate) fn supersedes(_newer: &PlatformOperation, _older: &PlatformOperation) -> bool {
        false
    }

    /// No platform operation coalesces duplicates.
    pub(crate) fn duplicates_of(&self, _older: &PlatformOperation) -> bool {
        false
    }

    /// Whether `Esc` may cancel the operation (plan §11): never — an
    /// already-spawned handler app, or an already-launched browser, owns
    /// its own lifetime; a suppressed `Cancelled` result could claim
    /// neither failure nor success. The external editor is likewise
    /// untouchable: it owns the terminal and the body file until it
    /// exits. The user can still leave the composer; the send completes
    /// (or is classified) in the background.
    pub(crate) fn is_cancellable(&self) -> bool {
        false
    }

    /// No platform operation runs a mail-backend child (issue 1v38): the
    /// handlers spawn directly from the manager.
    pub(crate) fn uses_backend_process(&self) -> bool {
        false
    }
}
