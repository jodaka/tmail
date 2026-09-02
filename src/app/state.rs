//! Application state (plan §9) — Phase 1 subset.
//!
//! `operations: OperationRegistry` and `overlay: Option<Overlay>` arrive with
//! Phase 3; `config: PostConfig` with the config work. Everything here is
//! plain data mutated only by the reducer.

use crate::app::focus::Focus;
use crate::app::route::{MailboxRoute, Route};
use crate::domain::{Mailbox, MailboxId, MessageSummary, Page};

/// Async load lifecycle for backend-fed collections (mock-fed in Phase 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loadable<T> {
    Loading,
    Loaded(T),
    Failed(String),
}

impl<T> Loadable<T> {
    pub fn as_loaded(&self) -> Option<&T> {
        match self {
            Loadable::Loaded(t) => Some(t),
            _ => None,
        }
    }
}

/// Which mode badge the status bar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusMode {
    Normal,
}

/// Transient status area state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusState {
    pub mode: StatusMode,
    /// One-line transient message; cleared on the next interaction.
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct AppState {
    /// Route stack; the last entry is the visible route.
    pub routes: Vec<Route>,
    pub mailboxes: Loadable<Vec<Mailbox>>,
    /// Index into the loaded mailbox list: the sidebar selection.
    pub mailbox_selection: usize,
    /// The visible message page for the active route's mailbox.
    pub messages: Page<MessageSummary>,
    /// Index into `messages.items`; always valid or the list is empty.
    pub selection: usize,
    pub search_query: String,
    pub focus: Focus,
    /// Last known terminal size; drives the responsive layout (plan §18).
    pub size: (u16, u16),
    pub status: StatusState,
    pub quit_requested: bool,
    /// Tick counter for animation state (spinner lands in Phase 3).
    pub ticks: u64,
}

impl AppState {
    pub fn initial(mailboxes: Vec<Mailbox>, messages: Page<MessageSummary>) -> Self {
        let mailbox_id = mailboxes
            .first()
            .map(|m| m.id.clone())
            .unwrap_or(MailboxId(String::from("inbox")));
        Self {
            routes: vec![Route::Mailbox(MailboxRoute { mailbox_id })],
            mailboxes: Loadable::Loaded(mailboxes),
            mailbox_selection: 0,
            messages,
            selection: 0,
            search_query: String::new(),
            focus: Focus::MessageList,
            size: (152, 40),
            status: StatusState {
                mode: StatusMode::Normal,
                message: None,
            },
            quit_requested: false,
            ticks: 0,
        }
    }

    pub fn active_route(&self) -> Option<&Route> {
        self.routes.last()
    }

    /// The message under the selection, if any.
    pub fn selected_message(&self) -> Option<&MessageSummary> {
        self.messages.items.get(self.selection)
    }

    /// Unread count of the mailbox currently displayed, when known.
    pub fn active_mailbox_unread(&self) -> Option<u64> {
        let id = self.active_route()?.mailbox_id()?;
        self.mailboxes
            .as_loaded()?
            .iter()
            .find(|m| &m.id == id)
            .and_then(|m| m.unread_count)
    }

    /// Name of the mailbox currently displayed, if resolvable.
    pub fn active_mailbox_name(&self) -> Option<&str> {
        let id = self.active_route()?.mailbox_id()?;
        self.mailboxes
            .as_loaded()?
            .iter()
            .find(|m| &m.id == id)
            .map(|m| m.name.as_str())
    }

    /// Selected sidebar mailbox, if the list is loaded and index valid.
    pub fn selected_mailbox(&self) -> Option<&Mailbox> {
        self.mailboxes
            .as_loaded()
            .and_then(|ms| ms.get(self.mailbox_selection))
    }

    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status.message = Some(message.into());
    }
}
