//! Application state (plan §9).
//!
//! Everything here is plain data mutated only by the reducer. In-flight
//! backend work is tracked in `operations` (the operation registry); the
//! `overlay` stack currently holds only the error modal.

use crate::app::composer::ComposerState;
use crate::app::focus::Focus;
use crate::app::operation::OperationRegistry;
use crate::app::overlay::Overlay;
use crate::app::route::Route;
use crate::domain::{Mailbox, Message, MessageSummary, Page};
use chrono::{DateTime, FixedOffset};

/// Async load lifecycle for backend-fed collections (mock-fed in Phase 1).
/// `Idle` marks a slot that is not currently in use (no message open).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loadable<T> {
    Idle,
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

/// The mailbox list context stashed while search results own the visible
/// list (Phase 9.1). Restored when the search route is left, so returning
/// is exact — page, selection, and scroll — with no reload (plan §9 route
/// contract).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListStash {
    pub page: Page<MessageSummary>,
    pub selection: usize,
    pub scroll: usize,
}

/// Transient status area state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusState {
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
    /// First row of `messages.items` currently drawn, kept by the reducer so
    /// the selection always stays on screen across movement and resize
    /// (Phase 2 acceptance).
    pub list_scroll: usize,
    /// The message currently open in the reader, when a `Route::Message` is
    /// active (plan §19 Phase 4). `Idle` when the reader is closed.
    pub open_message: Loadable<Message>,
    /// First content line currently visible in the reader document.
    pub reader_scroll: usize,
    /// Cursor within the open message's attachment chips (plan §15): the
    /// chip the save/open keys act on. `None` addresses the first chip;
    /// the reducer clamps with the loaded message's chip count.
    pub reader_attachment: Option<usize>,
    /// Paths where attachments were saved this session, keyed by
    /// (message id, part id) (plan §15). `Open` reuses the saved file
    /// instead of writing a duplicate; cleared implicitly with the state.
    pub saved_attachments:
        std::collections::HashMap<(crate::domain::MessageId, usize), std::path::PathBuf>,
    /// In-flight backend operations with their ids, retry intents, and
    /// cancellation tokens (plan §9/§11). Results only apply while their
    /// operation is still registered here.
    pub operations: OperationRegistry,
    /// Modal overlay above every screen; `Some` intercepts all input.
    pub overlay: Option<Overlay>,
    /// The built-in composer (plan §19 Phase 6): fields, body editor, and
    /// focus. `Some` while a draft exists — the route may be popped, the
    /// draft data stays for reopening.
    pub composer: Option<ComposerState>,
    pub search_query: String,
    /// The mailbox list context saved when a search took over the visible
    /// list (Phase 9.1); restored when the search route is left. `None`
    /// while no search is open above a mailbox.
    pub search_return: Option<ListStash>,
    /// Periodic refresh interval in seconds (plan §11/§19 Phase 9,
    /// `[post.mail].refresh_interval_seconds`); `0` disables the timer.
    /// Set from the config at startup; the reducer never reads a clock for
    /// it — arming uses the injected `Action::Tick` wall clock.
    pub refresh_interval_seconds: u64,
    /// Draft autosave debounce in milliseconds (plan §14,
    /// `[post.composer].autosave_delay_ms`). Set from the config at
    /// startup; the reducer applies it with the injected tick clock.
    pub autosave_delay_ms: u64,
    /// Injected-clock timestamp of the last refresh (manual or automatic),
    /// the timer's arm point. `None` until the first tick arms it.
    pub last_refresh_at: Option<DateTime<FixedOffset>>,
    /// The sanitized detail of the last *background* refresh failure
    /// (Phase 9.6): repeated identical failures are suppressed in the
    /// status line until a success or a manual refresh clears the record.
    pub last_background_error: Option<String>,
    pub focus: Focus,
    /// Whether terminal mouse capture is currently active (plan §10,
    /// `[post].mouse`). Set from the config at startup and flipped by
    /// `Action::ToggleMouseCapture`; the runtime applies the actual
    /// capture mode and keeps the terminal in sync. With capture off, the
    /// terminal's native text selection works untouched.
    pub mouse_capture: bool,
    /// Last known terminal size; drives the responsive layout (plan §18).
    pub size: (u16, u16),
    /// The configured account address (plan §17 `[accounts.<account>].email`).
    /// Reply-all seeding excludes it from recipients (Phase 7.5); the
    /// backend independently uses it as the `From` of outgoing mail.
    pub account_email: Option<String>,
    /// Last wall clock delivered by `Action::Tick { now }`: the
    /// reducer's only source of time (autosave debounce, saved-at stamps).
    pub clock: Option<DateTime<FixedOffset>>,
    pub status: StatusState,
    pub quit_requested: bool,
    /// Tick counter for animation state (spinner lands in Phase 3).
    pub ticks: u64,
}

impl AppState {
    /// Fresh startup state: nothing loaded, no route yet. Mailboxes and the
    /// first page arrive via backend results (`MailboxesLoaded` selects the
    /// Inbox or first usable mailbox).
    pub fn initial(page_size: usize) -> Self {
        Self {
            routes: Vec::new(),
            mailboxes: Loadable::Loading,
            mailbox_selection: 0,
            messages: Page::empty(page_size.max(1)),
            selection: 0,
            list_scroll: 0,
            open_message: Loadable::Idle,
            reader_scroll: 0,
            reader_attachment: None,
            saved_attachments: std::collections::HashMap::new(),
            operations: OperationRegistry::default(),
            overlay: None,
            composer: None,
            search_query: String::new(),
            search_return: None,
            refresh_interval_seconds: 0,
            autosave_delay_ms: crate::domain::draft::DEFAULT_AUTOSAVE_DELAY_MS,
            last_refresh_at: None,
            last_background_error: None,
            focus: Focus::MessageList,
            mouse_capture: false,
            size: (152, 40),
            account_email: None,
            clock: None,
            status: StatusState { message: None },
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

    /// The summary of the message open in the reader, if one is.
    pub fn open_summary(&self) -> Option<&MessageSummary> {
        match self.active_route() {
            Some(Route::Message(route)) => Some(&route.summary),
            _ => None,
        }
    }

    /// The `MessageLocator` of the message a reader/list action should
    /// target: the open message in the reader, else the selected row.
    pub fn action_target(&self) -> Option<crate::domain::MessageLocator> {
        let summary = match self.focus {
            Focus::Reader => self.open_summary(),
            _ => self.selected_message(),
        }?;
        Some(crate::domain::MessageLocator {
            mailbox: summary.mailbox_id.clone(),
            id: summary.id.clone(),
            message_id: summary.message_id.clone(),
        })
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
