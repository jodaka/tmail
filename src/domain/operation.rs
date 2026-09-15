//! Cross-layer operation identity.
//!
//! Every backend request and result carries an [`OperationId`] (plan §5:
//! "Every request and result carries an `OperationId`"). The type lives in
//! the domain so the backend contract can name it without depending on the
//! app layer (ticket 55t6); the operation registry — its only constructor
//! outside tests — stays in `app::operation`.

/// Opaque identifier carried by every backend request and result. The
/// value is the registry's monotonically increasing counter; no meaning
/// is parsed from it. Constructed only by the registry; test code may
/// synthesize unknown ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OperationId(pub u64);

impl std::fmt::Display for OperationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "op-{}", self.0)
    }
}
