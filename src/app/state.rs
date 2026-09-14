//! Application state (plan §9).
//!
//! Everything here is plain data mutated only by the reducer (one
//! documented exception: `CacheBundle.reader_doc`, interior-mutable so
//! the renderer's cache can update through `&AppState`). In-flight
//! backend work is tracked in `operations` (the operation registry); the
//! `overlay` stack holds one modal at a time: the error modal, the
//! discard/send confirmations, the attachment explorer, the theme
//! picker, or help.

use crate::app::composer::ComposerState;
use crate::app::focus::Focus;
use crate::app::operation::OperationRegistry;
use crate::app::overlay::Overlay;
use crate::app::reader::CachedReaderDoc;
use crate::app::route::Route;
use crate::domain::{Mailbox, MailboxId, MailboxRole, Message, MessageId, MessageSummary, Page};
use crate::view::theme::Theme;
use chrono::{DateTime, FixedOffset};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

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

/// What the reader's Tab cycle has focused inside one open message
/// (tickets 1fnh/hc9n). The cycle walks the body's links in document
/// order first, then the attachment chips; `None` on `AppState` means
/// nothing is focused yet, and `Enter`/`S`/`o` fall back to the first
/// chip as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderFocus {
    /// A link run of the scrollable body (index over link runs, not
    /// spans: a wrapped anchor is one item).
    Link(usize),
    /// An attachment chip by index.
    Attachment(usize),
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

/// New-mail notification bookkeeping (ticket b28p). The background refresh
/// marks the visible page as clean at its start and finish; the leading run
/// of ids a finished page adds is the mail that arrived since the last
/// boundary, which [`crate::config::Notifications`] decides what to do with.
#[derive(Debug, Clone, Default)]
pub struct NotificationsState {
    /// Message ids considered clean at the last background-update boundary.
    /// Ids outside this set are new arrivals.
    pub clean: HashSet<MessageId>,
}

impl NotificationsState {
    /// Mark `page` as clean (a background update's start or finish
    /// boundary): everything on screen now is known mail.
    pub fn mark_clean(&mut self, page: &Page<MessageSummary>) {
        self.clean = page.items.iter().map(|m| m.id.clone()).collect();
    }

    /// The leading run of messages in `page` whose ids are not in the clean
    /// set — the mail that arrived since the last boundary. Stops at the
    /// first known id, so a widened page (a terminal resize) never counts
    /// rows that already existed deeper in the list.
    pub fn new_messages<'a>(&self, page: &'a Page<MessageSummary>) -> Vec<&'a MessageSummary> {
        page.items
            .iter()
            .take_while(|m| !self.clean.contains(&m.id))
            .collect()
    }
}

/// Transient status area state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusState {
    /// One-line transient message; cleared on the next interaction.
    pub message: Option<String>,
    /// Injected-clock instant the message was set (ticket h1d7): arms the
    /// `[tmail].status_timeout` fade-and-clear timer. `None` while no
    /// message is up, or the clock has not ticked yet (the reducer has no
    /// time source before the first tick).
    pub shown_at: Option<DateTime<FixedOffset>>,
}

/// The config-derived knobs, seeded once from the shared TOML file at
/// startup (plan §17). Everything here is read by the reducer and renderer;
/// `mouse_capture` and the theme cursor are the only fields the user flips
/// at runtime.
#[derive(Clone)]
pub struct Settings {
    /// The keyboard binding table (configurable keybindings): defaults
    /// seeded, then reshaped by `[tmail.keybindings]`. Consulted by the
    /// translation layer (`input::keyboard`) and the status-bar hints, so
    /// a rebind can never leave the UI lying about its keys.
    pub keymap: crate::input::keymap::KeyMap,
    /// The configured account address (plan §17 `[accounts.<account>].email`).
    /// Reply-all seeding excludes it from recipients (Phase 7.5); the
    /// backend independently uses it as the `From` of outgoing mail.
    pub account_email: Option<String>,
    /// The himalaya account the backend drives (`[tmail].account`, or the
    /// resolved default — ticket c0n0). `None` in the
    /// multi-account-without-default case, where the account switcher is
    /// the way to pick one. Shown under the logo.
    pub account_name: Option<String>,
    /// Every account in the config file at startup (ticket c0n0), for the
    /// runtime account switcher. Session-stale by design: the switch's own
    /// reload re-reads the file, so an account added while the app runs
    /// appears after the next restart — or after any switch.
    pub accounts: Vec<crate::config::AccountEntry>,
    /// Periodic refresh interval in seconds (plan §11/§19 Phase 9,
    /// `[tmail.mail].refresh_interval_seconds`); `0` disables the timer.
    /// Set from the config at startup; the reducer never reads a clock for
    /// it — arming uses the injected `Action::Tick` wall clock.
    pub refresh_interval_seconds: u64,
    /// Ticket kjfq (`[tmail.mail].page_size_auto`): size each page to the
    /// number of message rows the terminal can show, overriding
    /// `page_size`. Set from the config at startup; the reducer recomputes
    /// the limit from `size` on every resize so the page always matches
    /// the visible rows.
    pub page_size_auto: bool,
    /// Draft autosave debounce in milliseconds (plan §14,
    /// `[tmail.composer].autosave_delay_ms`). Set from the config at
    /// startup; the reducer applies it with the injected tick clock.
    pub autosave_delay_ms: u64,
    /// `[tmail].view_mode` list density (Gmail-style): `comfortable`
    /// interleaves a faint horizontal separator under every message row,
    /// so each message takes two terminal lines and fewer fit. Set from
    /// the config at startup; the renderer and the reducer's visible-row
    /// math both read it.
    pub view_mode: crate::config::ViewMode,
    /// `[tmail].status_timeout` (ticket h1d7): seconds a status message
    /// stays up before it fades out and clears; `0` (default) keeps it
    /// until the next message replaces it. Set from the config at
    /// startup; expiry runs on the injected tick clock.
    pub status_timeout_seconds: u64,
    /// `[tmail].notifications` (ticket b28p): no notification, terminal
    /// bell, or desktop notification for new mail found by the periodic
    /// background refresh. Set from the config at startup; the reducer
    /// only reads it.
    pub notifications: crate::config::Notifications,
    /// The configured external editor as an argv (plan §14, Phase 11),
    /// resolved from the config at startup. `None` means the builtin
    /// editor: `Ctrl+E` is inert.
    pub editor_command: Option<Vec<String>>,
    /// Whether terminal mouse capture is currently active (plan §10,
    /// `[tmail].mouse`). Set from the config at startup and flipped by
    /// `Action::ToggleMouseCapture`; the runtime applies the actual
    /// capture mode and keeps the terminal in sync. With capture off, the
    /// terminal's native text selection works untouched.
    pub mouse_capture: bool,
    /// Runtime-switchable themes (ticket z0s4): `(name, palette)` in cycle
    /// order — the two built-ins plus every `[tmail.themes.<name>]` table,
    /// precomputed at startup. `t` cycles the list; the shared config
    /// file is never rewritten, so switching is session-only.
    pub themes: Vec<(String, Theme)>,
    /// Index into `themes` of the palette currently on screen.
    pub theme_index: usize,
}

/// Session-local caches: everything here exists to avoid re-fetching or
/// re-building work the backend already produced. Cleared implicitly when
/// the session ends. The on-disk page cache (ticket haeb) lives in the
/// operation manager — cache I/O must never run on the reducer thread.
#[derive(Clone)]
pub struct CacheBundle {
    /// Message ids whose list preview (ticket wxtx) was already satisfied
    /// this session — fetched, in flight, or served from the on-disk
    /// message cache. One request per message, ever: a failed preview is
    /// accepted (it is decorative context), and a message fetched in an
    /// *earlier* session counts, so startup never re-fetches known mail.
    pub preview_requested: HashSet<MessageId>,
    /// One-line body previews this session (ticket wxtx), keyed by backend
    /// message id. `MessageSummary.snippet` does not survive a page load —
    /// fresh envelope listings carry none — so `apply_page` restores
    /// previews from here: a periodic refresh or a page change renders the
    /// same rows it replaced, without re-fetching anything.
    pub previews: HashMap<MessageId, String>,
    /// Paths where attachments were saved this session, keyed by
    /// (message id, part id) (plan §15). `Open` reuses the saved file
    /// instead of writing a duplicate; cleared implicitly with the state.
    pub saved_attachments: HashMap<(MessageId, usize), PathBuf>,
    /// Cached scrollable reader document (perf, `ui::screens::reader`
    /// `CachedReaderDoc`): building it re-parses the whole message HTML,
    /// and it used to run twice per wheel event — once in the reducer's
    /// scroll clamp, once in the frame draw — so a touchpad momentum burst
    /// re-parsed the body hundreds of times and starved key handling.
    /// Interior-mutable because both the clamp path and the renderer hold
    /// only `&AppState`; the app is single-threaded and no accessor
    /// re-enters while borrowing, so `RefCell` suffices.
    pub(crate) reader_doc: RefCell<Option<CachedReaderDoc>>,
}

/// Per-session UI bookkeeping: routes, focus, modals, clocks, in-flight
/// operations, and the wizard/composer slots. Everything here is runtime
/// state — not configuration, not cached backend data.
#[derive(Clone)]
pub struct SessionState {
    /// Route stack; the last entry is the visible route.
    pub routes: Vec<Route>,
    pub focus: Focus,
    /// Modal overlay above every screen; `Some` intercepts all input.
    pub overlay: Option<Overlay>,
    pub status: StatusState,
    /// Last wall clock delivered by `Action::Tick { now }`: the
    /// reducer's only source of time (autosave debounce, saved-at stamps).
    pub clock: Option<DateTime<FixedOffset>>,
    pub quit_requested: bool,
    /// Last known terminal size; drives the responsive layout (plan §18).
    pub size: (u16, u16),
    pub search_query: String,
    /// The mailbox list context saved when a search took over the visible
    /// list (Phase 9.1); restored when the search route is left. `None`
    /// while no search is open above a mailbox.
    pub search_return: Option<ListStash>,
    /// Injected-clock timestamp of the last refresh (manual or automatic),
    /// the timer's arm point. `None` until the first tick arms it.
    pub last_refresh_at: Option<DateTime<FixedOffset>>,
    /// The sanitized detail of the last *background* refresh failure
    /// (Phase 9.6): repeated identical failures are suppressed in the
    /// status line until a success or a manual refresh clears the record.
    pub last_background_error: Option<String>,
    /// Whether the terminal window currently has focus (CSI 1004 focus
    /// events, ticket b28p). New-mail notifications fire only while the
    /// window is unfocused; terminals without focus reporting leave this
    /// set, so notifications stay conservative.
    pub terminal_focused: bool,
    /// New-mail notification bookkeeping (ticket b28p).
    pub notifications: NotificationsState,
    /// In-flight backend operations with their ids, retry intents, and
    /// cancellation tokens (plan §9/§11). Results only apply while their
    /// operation is still registered here.
    pub operations: OperationRegistry,
    /// The built-in composer (plan §19 Phase 6): fields, body editor, and
    /// focus. `Some` while a draft exists — the route may be popped, the
    /// draft data stays for reopening.
    pub composer: Option<ComposerState>,
    /// The account configuration wizard (ADR 0003): `Some` while the
    /// first-run/manual wizard route is active. The reducer intercepts
    /// every action while this is set, and the renderer swaps the whole
    /// shell for the wizard screens.
    pub wizard: Option<crate::app::wizard::WizardState>,
    /// The confirmed account switch (ticket c0n0): `Some(target)` once the
    /// user confirmed through the switcher's confirm dialog. The event
    /// loop exits on it and the runtime rebuilds the whole session against
    /// the named account — every in-flight operation was already cancelled
    /// on the way here.
    pub switch_requested: Option<String>,
}

/// The whole application state: the visible list/reader data plus the
/// three domain groups (`settings`, `caches`, `session`).
#[derive(Clone)]
pub struct AppState {
    pub mailboxes: Loadable<Vec<Mailbox>>,
    /// Index into the loaded mailbox list: the sidebar selection.
    pub mailbox_selection: usize,
    /// The visible message page for the active route's mailbox.
    pub messages: Page<MessageSummary>,
    /// Index into `messages.items`; always valid or the list is empty.
    pub selection: usize,
    /// Bulk-selection set (ticket p0s3): backend ids of the messages the
    /// user marked with Space. Keyed by backend id, so a selection survives
    /// paging and refreshes while the rows stay; a mailbox switch or a
    /// search leave clears it. Selection mode is on exactly when this is
    /// non-empty.
    pub selected: HashSet<MessageId>,
    /// First row of `messages.items` currently drawn, kept by the reducer so
    /// the selection always stays on screen across movement and resize
    /// (Phase 2 acceptance).
    pub list_scroll: usize,
    /// The message currently open in the reader, when a `Route::Message` is
    /// active (plan §19 Phase 4). `Idle` when the reader is closed.
    pub open_message: Loadable<Message>,
    /// First content line currently visible in the reader document.
    pub reader_scroll: usize,
    /// What the reader's Tab cycle focuses (tickets 1fnh/hc9n): a link run
    /// or an attachment chip. `None` until the user tabs or clicks; `d`/
    /// `S`/`o` still act on the first chip when no attachment is focused.
    pub reader_focus: Option<ReaderFocus>,
    /// Config-derived knobs, seeded once at startup.
    pub settings: Settings,
    /// Session-local caches.
    pub caches: CacheBundle,
    /// Per-session UI bookkeeping.
    pub session: SessionState,
}

impl AppState {
    /// Fresh startup state: nothing loaded, no route yet. Mailboxes and the
    /// first page arrive via backend results (`MailboxesLoaded` selects the
    /// Inbox or first usable mailbox).
    pub fn initial(page_size: usize) -> Self {
        Self {
            mailboxes: Loadable::Loading,
            mailbox_selection: 0,
            messages: Page::empty(page_size.max(1)),
            selection: 0,
            selected: HashSet::new(),
            list_scroll: 0,
            open_message: Loadable::Idle,
            reader_scroll: 0,
            reader_focus: None,
            settings: Settings {
                keymap: crate::input::keymap::KeyMap::defaults(),
                account_email: None,
                account_name: None,
                accounts: Vec::new(),
                refresh_interval_seconds: 0,
                page_size_auto: false,
                autosave_delay_ms: crate::domain::draft::DEFAULT_AUTOSAVE_DELAY_MS,
                view_mode: crate::config::ViewMode::Compact,
                status_timeout_seconds: 0,
                notifications: crate::config::Notifications::Off,
                editor_command: None,
                mouse_capture: false,
                themes: vec![
                    (String::from("default"), Theme::default_dark()),
                    (String::from("light"), Theme::default_light()),
                ],
                theme_index: 0,
            },
            caches: CacheBundle {
                preview_requested: HashSet::new(),
                previews: HashMap::new(),
                saved_attachments: HashMap::new(),
                reader_doc: RefCell::new(None),
            },
            session: SessionState {
                routes: Vec::new(),
                focus: Focus::MessageList,
                overlay: None,
                status: StatusState {
                    message: None,
                    shown_at: None,
                },
                clock: None,
                quit_requested: false,
                size: (152, 40),
                search_query: String::new(),
                search_return: None,
                last_refresh_at: None,
                last_background_error: None,
                terminal_focused: true,
                notifications: NotificationsState::default(),
                operations: OperationRegistry::default(),
                composer: None,
                wizard: None,
                switch_requested: None,
            },
        }
    }

    pub fn active_route(&self) -> Option<&Route> {
        self.session.routes.last()
    }

    /// The message under the selection, if any.
    pub fn selected_message(&self) -> Option<&MessageSummary> {
        self.messages.items.get(self.selection)
    }

    /// Whether bulk-selection mode is on (ticket p0s3): at least one
    /// visible-list message carries the Space mark.
    pub fn selection_active(&self) -> bool {
        !self.selected.is_empty()
    }

    /// How many of the currently visible rows are bulk-selected.
    pub fn visible_selected_count(&self) -> usize {
        self.messages
            .items
            .iter()
            .filter(|m| self.selected.contains(&m.id))
            .count()
    }

    /// Whether every visible row is bulk-selected. An empty list is never
    /// "all selected".
    pub fn all_visible_selected(&self) -> bool {
        !self.messages.items.is_empty()
            && self
                .messages
                .items
                .iter()
                .all(|m| self.selected.contains(&m.id))
    }

    /// Locators of the bulk-selected messages, in visible-list order. The
    /// set may hold ids from other pages; only rows actually present are
    /// actionable.
    pub fn selected_locators(&self) -> Vec<crate::domain::MessageLocator> {
        self.messages
            .items
            .iter()
            .filter(|m| self.selected.contains(&m.id))
            .map(MessageSummary::into_locator)
            .collect()
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
        let summary = match self.session.focus {
            Focus::Reader => self.open_summary(),
            _ => self.selected_message(),
        }?;
        Some(summary.into_locator())
    }

    /// Unread count of the mailbox currently displayed, when known.
    pub fn active_mailbox_unread(&self) -> Option<u64> {
        self.active_mailbox().and_then(|m| m.unread_count)
    }

    /// Role of the mailbox currently displayed, when known (ADR 0001: the
    /// UI never guesses folder names — the backend adapter resolves them).
    /// The Drafts role changes what Enter on a list row does (a draft
    /// reopens in the composer, plan §14).
    pub fn active_mailbox_role(&self) -> Option<MailboxRole> {
        self.active_mailbox().and_then(|m| m.role)
    }

    /// Name of the mailbox currently displayed, if resolvable.
    pub fn active_mailbox_name(&self) -> Option<&str> {
        self.active_mailbox().map(|m| m.name.as_str())
    }

    /// The mailbox displayed by the active route (sidebar/search/list),
    /// when the listing is loaded. The shared lookup behind the
    /// unread/role/name accessors.
    fn active_mailbox(&self) -> Option<&Mailbox> {
        let id = self.active_route()?.mailbox_id()?;
        self.mailboxes.as_loaded()?.iter().find(|m| &m.id == id)
    }

    /// Restore the list invariant after the row set changed (crash
    /// tickets in review s843): the cursor row and the scroll window stay
    /// inside the loaded page. The accessor the many transition tails
    /// used to spell out as `min(len.saturating_sub(1))` twice.
    pub fn clamp_list_positions(&mut self) {
        let last = self.messages.items.len().saturating_sub(1);
        self.selection = self.selection.min(last);
        self.list_scroll = self.list_scroll.min(last);
    }

    /// Same invariant for the sidebar cursor against the loaded mailbox
    /// listing (empty list → 0, the conventional empty-view cursor).
    pub fn clamp_mailbox_selection(&mut self) {
        let last = self
            .mailboxes
            .as_loaded()
            .map_or(0, |list| list.len().saturating_sub(1));
        self.mailbox_selection = self.mailbox_selection.min(last);
    }

    /// The mailbox the sidebar marks active. Normally the displayed
    /// mailbox; while the composer is open, the backend's Drafts folder —
    /// drafts are saved there (plan §14), so the folder the composer
    /// writes to reads as the active one (the composer route itself
    /// carries no mailbox). `None` while composing without a resolved
    /// Drafts folder, and whenever no route is displayed.
    pub fn sidebar_active_mailbox_id(&self) -> Option<&MailboxId> {
        if matches!(self.active_route(), Some(Route::Composer)) {
            return self.drafts_mailbox().map(|m| &m.id);
        }
        self.active_route().and_then(|r| r.mailbox_id())
    }

    /// The loaded mailbox with the `Drafts` role, if any (ADR 0001: the UI
    /// never guesses folder names; the backend adapter resolves roles).
    pub fn drafts_mailbox(&self) -> Option<&Mailbox> {
        self.mailboxes
            .as_loaded()?
            .iter()
            .find(|m| m.role == Some(MailboxRole::Drafts))
    }

    /// Selected sidebar mailbox, if the list is loaded and index valid.
    pub fn selected_mailbox(&self) -> Option<&Mailbox> {
        self.mailboxes
            .as_loaded()
            .and_then(|ms| ms.get(self.mailbox_selection))
    }

    pub fn set_status(&mut self, message: impl Into<String>) {
        self.session.status.message = Some(message.into());
        // The timeout timer (ticket h1d7) arms from the last injected
        // tick. A message set before the first tick has no timer yet;
        // the first tick arms it (see the reducer's status clearing).
        self.session.status.shown_at = self.session.clock;
    }

    /// The palette currently on screen (ticket z0s4). The list is never
    /// empty by construction; the fallbacks keep a mangled index (or an
    /// empty list) harmless instead of panicking mid-frame.
    pub fn active_theme(&self) -> Theme {
        self.settings
            .themes
            .get(self.settings.theme_index)
            .or_else(|| self.settings.themes.first())
            .map(|(_, theme)| *theme)
            .unwrap_or_else(Theme::default_dark)
    }

    /// The identity shown under the logo (ticket c0n0): the current
    /// account's email, else its display name, else its raw name (the
    /// email first — display names are commonly shared between accounts,
    /// user request). `None` when no account is resolved
    /// (multi-account configs without a default — the account switcher
    /// is the way to pick one there).
    pub fn current_account_label(&self) -> Option<String> {
        let name = self.settings.account_name.as_deref()?;
        let entry = self
            .settings
            .accounts
            .iter()
            .find(|account| account.name == name);
        Some(
            entry
                .map(crate::config::AccountEntry::label)
                .unwrap_or(name)
                .to_owned(),
        )
    }

    /// The terminal window title for the current mode: the wizard over
    /// everything ("tmail setup"), the composer ("Mail to …", the first
    /// recipient, raw text since it may still be mid-typed), else the
    /// displayed mailbox with its unread count (search included: the
    /// title names the folder being searched). With several accounts the
    /// mailbox title is prefixed with the driving account's email
    /// (`me@example.org: INBOX (4 unread)`, user request) — with one
    /// account the plain mailbox title stands.
    pub fn terminal_title(&self) -> String {
        if self.session.wizard.is_some() {
            return String::from("tmail setup");
        }
        // The composer title only applies while a Composer route is
        // actually on screen (ticket bfd8): an Esc-parked draft keeps
        // `session.composer` set, but the user is back in the mailbox.
        if matches!(self.active_route(), Some(Route::Composer))
            && let Some(composer) = &self.session.composer
        {
            let to = composer.draft.to.as_str();
            let recipient = to
                .split([',', ';'])
                .map(str::trim)
                .find(|entry| !entry.is_empty())
                .unwrap_or("");
            if recipient.is_empty() {
                return String::from("Mail to —");
            }
            return format!("Mail to {recipient}");
        }
        let name = self.active_mailbox_name().unwrap_or("Mailbox");
        let mailbox = match self.active_mailbox_unread() {
            Some(unread) => format!("{name} ({unread} unread)"),
            None => String::from(name),
        };
        match self.multi_account_identity() {
            Some(identity) => format!("{identity}: {mailbox}"),
            None => mailbox,
        }
    }

    /// The identity prefix the terminal title carries when the config
    /// holds several accounts (user request): the driving account's
    /// email, else its display name, else its raw name. `None` with a
    /// single (or unresolved) account — the plain mailbox title says all
    /// there is.
    fn multi_account_identity(&self) -> Option<String> {
        if self.settings.accounts.len() < 2 {
            return None;
        }
        self.settings
            .account_email
            .clone()
            .or_else(|| self.current_account_label())
    }
}

#[cfg(test)]
mod terminal_title_tests {
    use super::*;
    use crate::app::composer::ComposerState;
    use crate::app::route::{MailboxRoute, Route};
    use crate::app::state::Loadable;
    use crate::app::wizard::{ConfigSnapshot, WizardState};
    use crate::domain::{Mailbox, MailboxId};

    fn mailbox_state() -> AppState {
        let mut state = AppState::initial(50);
        state.mailboxes = Loadable::Loaded(vec![Mailbox {
            id: MailboxId(String::from("INBOX")),
            name: String::from("INBOX"),
            role: Some(MailboxRole::Inbox),
            unread_count: Some(4),
            total_count: Some(20),
        }]);
        state.session.routes.push(Route::Mailbox(MailboxRoute {
            mailbox_id: MailboxId(String::from("INBOX")),
        }));
        state
    }

    #[test]
    fn normal_mode_names_the_mailbox_and_unread() {
        let state = mailbox_state();
        assert_eq!(state.terminal_title(), "INBOX (4 unread)");
    }

    #[test]
    fn unknown_unread_count_falls_back_to_the_name() {
        let mut state = mailbox_state();
        state.mailboxes = Loadable::Failed(String::from("broken"));
        assert_eq!(state.terminal_title(), "Mailbox");
    }

    #[test]
    fn open_composer_names_the_recipient() {
        let mut state = mailbox_state();
        let mut composer = ComposerState::new();
        composer.draft.to = String::from("Ada Example <ada@example.io>, ");
        state.session.composer = Some(composer);
        state.session.routes.push(Route::Composer);
        assert_eq!(
            state.terminal_title(),
            "Mail to Ada Example <ada@example.io>"
        );
    }

    #[test]
    fn composer_without_a_recipient_dashes() {
        let mut state = mailbox_state();
        state.session.composer = Some(ComposerState::new());
        state.session.routes.push(Route::Composer);
        assert_eq!(state.terminal_title(), "Mail to —");
    }

    #[test]
    fn parked_draft_does_not_own_the_title() {
        // Ticket bfd8: an Esc-parked draft keeps session.composer set, but
        // with the mailbox route back on top the title must name the
        // mailbox, not "Mail to …".
        let mut state = mailbox_state();
        state.session.composer = Some(ComposerState::new());
        assert_eq!(state.terminal_title(), "INBOX (4 unread)");
    }

    #[test]
    fn the_wizard_owns_the_title() {
        let mut state = mailbox_state();
        state.session.composer = Some(ComposerState::new());
        state.session.wizard = Some(WizardState::new(
            true,
            ConfigSnapshot {
                save_path: None,
                existing_names: Vec::new(),
                existing_default_name: None,
                existing_shared_readable: false,
            },
        ));
        assert_eq!(state.terminal_title(), "tmail setup");
    }

    #[test]
    fn multi_account_titles_carry_the_account_email() {
        // User request: with several accounts the mailbox title is
        // prefixed with the driving account's email.
        let mut state = mailbox_state();
        state.settings.account_name = Some(String::from("work"));
        state.settings.account_email = Some(String::from("me@example.org"));
        state.settings.accounts = vec![
            crate::config::AccountEntry {
                name: String::from("work"),
                email: Some(String::from("me@example.org")),
                display_name: None,
                is_default: true,
            },
            crate::config::AccountEntry {
                name: String::from("personal"),
                email: Some(String::from("other@example.net")),
                display_name: None,
                is_default: false,
            },
        ];
        assert_eq!(state.terminal_title(), "me@example.org: INBOX (4 unread)");

        // A single account keeps the plain mailbox title.
        state.settings.accounts.truncate(1);
        assert_eq!(state.terminal_title(), "INBOX (4 unread)");
    }
}
