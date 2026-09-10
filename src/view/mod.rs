//! Shared view model: pure rendering-adjacent computation consumed by both
//! the app layer (reducer scroll/page bookkeeping) and the UI layer
//! (drawing). Nothing here depends on `crate::app`; `ui` may import from
//! here, and `app` does, so the two stay one-way.

pub mod dates;
pub mod layout;
pub mod overlay;
pub(crate) mod rich;
pub mod text;
pub mod theme;
