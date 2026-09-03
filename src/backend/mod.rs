//! Backend layer (plan §6/§8): the async, mockable mail interface and its
//! Himalaya CLI implementation.
//!
//! Layering rules (plan §5, ADR 0001):
//! - UI and app modules never see Himalaya DTOs; they only ever touch
//!   [`crate::domain`] types produced through [`MailBackend`].
//! - The adapter owns semantic mapping (mailbox roles, archive/trash
//!   semantics) so the UI never guesses folder names.

pub mod himalaya;
pub mod journal;
pub mod opener;
pub mod traits;

pub use crate::domain::AttachmentRequest;
pub use opener::{PathOpener, SystemOpener};
pub use traits::{BackendError, BackendResult, MailBackend, RequestContext};
