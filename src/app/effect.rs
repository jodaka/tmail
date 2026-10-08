//! Typed effects (plan §5: "Effects launch typed backend requests").
//!
//! The reducer is I/O-free; when state transitions need data from the
//! backend it returns effects, and the runtime's operation manager
//! ([`crate::runtime::tasks`]) spawns the typed
//! [`crate::backend::MailBackend`] requests whose results flow back as
//! `Action::BackendCompleted`. Every effect carries the [`OperationId`] the
//! reducer allocated, so every request and result is traceable and
//! cancellable (plan §11).

use crate::app::operation::{OperationId, OperationKind, OperationOrigin, RetrySpec};

/// One typed backend request, ready for the operation manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effect {
    /// Allocated by the operation registry when this effect was created.
    pub id: OperationId,
    /// The typed request to launch.
    pub kind: OperationKind,
    /// Whether the reducer started this as interactive work or as
    /// background consequence (timer refresh, ticket wxtx preview).
    /// Carried on the effect because the manager decides which permit
    /// pool the request queues on (ticket d5rf): decorative work must
    /// never take the slot interactive work is about to wait for.
    pub origin: OperationOrigin,
}

impl Effect {
    /// The typed intent to replay if this operation fails (plan §12); the
    /// retry gets a fresh id from the registry.
    pub fn retry_spec(&self) -> RetrySpec {
        self.kind.retry_spec()
    }

    /// True when this effect carries background-origin work.
    pub fn is_background(&self) -> bool {
        matches!(self.origin, OperationOrigin::Background)
    }
}
