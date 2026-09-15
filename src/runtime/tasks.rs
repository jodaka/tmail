//! The operation manager (plan §11): spawns one task per typed effect,
//! hands each request its `OperationId` and cancellation token, and feeds
//! results back into the reducer through the task result channel as
//! `Action::BackendCompleted`.
//!
//! Responsibilities:
//! - launch effects against the backend without blocking the UI loop;
//! - map typed [`BackendError`]s into modal-ready [`OperationFailure`]s,
//!   sanitizing every detail before it can reach logs or the UI (plan §12);
//! - suppress results of cancelled operations — cancellation also
//!   terminates the child process the adapter owns (Phase 3.2), and the
//!   reducer rejects results for removed operation ids.

use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;

use crate::app::effect::Effect;
use crate::app::operation::{
    NotifyRequest, OperationFailure, OperationKind, OperationOutcome, OperationResult,
};
use crate::app::sanitize::sanitize;
use crate::backend::{BackendError, MailBackend, Notifier, PathOpener, RequestContext};
use crate::discovery::EmailConfigDiscoverer;

/// Spawns backend tasks for the effects the reducer emits.
pub struct OperationManager {
    backend: Arc<dyn MailBackend>,
    /// Platform open-with adapter for `OpenPath` effects (plan §15,
    /// Phase 8.5): `open`/`xdg-open`, spawned directly.
    opener: Arc<dyn PathOpener>,
    /// New-mail notification adapter (ticket b28p): terminal bell or
    /// `notify-rust`. Injected like the opener so tests never ring or
    /// pop a real notification.
    notifier: Arc<dyn Notifier>,
    /// The email settings discoverer for the wizard's `DiscoverConfig`
    /// effects (ADR 0003 §3.7): injected like the backend and opener,
    /// so tests and smoke runs swap in a fake.
    discoverer: Arc<dyn EmailConfigDiscoverer>,
    /// The himalaya executable the wizard credential test runs
    /// (ADR 0003 §3.4): the backend's own program name is private to
    /// the adapter, so the manager carries it for the test invocation.
    /// Overridable so contract tests point it at the fake.
    himalaya_program: String,
    /// The summary/message cache (ticket haeb): cache reads and writes
    /// are file I/O, so they run here on the blocking pool — the reducer
    /// stays I/O-free. `None` disables caching (unknown data dir).
    cache: Option<crate::app::page_cache::PageCache>,
    results: UnboundedSender<OperationResult>,
}

impl OperationManager {
    pub fn new(
        backend: Arc<dyn MailBackend>,
        opener: Arc<dyn PathOpener>,
        notifier: Arc<dyn Notifier>,
        discoverer: Arc<dyn EmailConfigDiscoverer>,
        himalaya_program: String,
        cache: Option<crate::app::page_cache::PageCache>,
        results: UnboundedSender<OperationResult>,
    ) -> Self {
        Self {
            backend,
            opener,
            notifier,
            discoverer,
            himalaya_program,
            cache,
            results,
        }
    }

    /// Launch one effect. The cancellation token comes from the operation
    /// registry (`AppState.session.operations`), so `Esc` reaches the child process.
    pub fn launch(&self, effect: Effect, ctx: RequestContext) {
        let backend = Arc::clone(&self.backend);
        let opener = Arc::clone(&self.opener);
        let notifier = Arc::clone(&self.notifier);
        let discoverer = Arc::clone(&self.discoverer);
        let himalaya_program = self.himalaya_program.clone();
        let cache = self.cache.clone();
        let results = self.results.clone();
        let id = effect.id;
        tokio::spawn(async move {
            tracing::debug!(id = %id, "operation launched");
            match run_effect(
                &backend,
                &opener,
                &notifier,
                &discoverer,
                &himalaya_program,
                &cache,
                &effect,
                &ctx,
            )
            .await
            {
                Some(outcome) => {
                    // The channel lives for the whole session; a send
                    // failure means the loop is shutting down and the
                    // result can be dropped.
                    let _ = results.send(OperationResult { id, outcome });
                }
                None => {
                    // Cancelled before completion: no result, no modal
                    // (plan §11: cancelled work never mutates state).
                    tracing::debug!(id = %id, "operation cancelled; result suppressed");
                }
            }
        });
    }
}

/// Execute one effect and translate the backend result. `None` means the
/// operation was cancelled — there is nothing to report.
///
/// The plain `OperationKind` arms dispatch through [`run_call`]: a backend
/// call, a success payload, and the shared `operation_failure` error
/// mapping. Only the arms with a different control shape (attachment
/// explorer, platform opener, editor, discovery, credential test, account
/// save) are spelled out.
#[allow(clippy::too_many_arguments)]
async fn run_effect(
    backend: &Arc<dyn MailBackend>,
    opener: &Arc<dyn PathOpener>,
    notifier: &Arc<dyn Notifier>,
    discoverer: &Arc<dyn EmailConfigDiscoverer>,
    himalaya_program: &str,
    cache: &Option<crate::app::page_cache::PageCache>,
    effect: &Effect,
    ctx: &RequestContext,
) -> Option<Result<OperationOutcome, OperationFailure>> {
    // Ticket sakb: dispatch on a borrow — arms move only the sub-entity
    // they need, instead of cloning the whole `OperationKind` (including
    // draft/outbound bodies) on every launch.
    match &effect.kind {
        OperationKind::LoadMailboxes => {
            run_call(
                effect,
                ctx,
                |c| backend.list_mailboxes(c),
                OperationOutcome::Mailboxes,
            )
            .await
        }
        OperationKind::LoadPage(request) => {
            run_call(
                effect,
                ctx,
                move |c| backend.list_messages(c, request.clone()),
                OperationOutcome::Page,
            )
            .await
        }
        OperationKind::Search(request) => {
            run_call(
                effect,
                ctx,
                move |c| backend.search_messages(c, request.clone()),
                OperationOutcome::Page,
            )
            .await
        }
        // Reader load, draft reopen (the reducer turns the fetched copy
        // into a composer draft), list preview (ticket wxtx), and the
        // list-initiated reply/forward seed share one backend call and
        // one payload shape.
        OperationKind::LoadMessage(locator)
        | OperationKind::OpenDraft(locator)
        | OperationKind::Preview(locator)
        | OperationKind::SeedComposer { locator, .. } => {
            run_call(
                effect,
                ctx,
                move |c| backend.get_message(c, locator.clone()),
                |message| OperationOutcome::Message(Box::new(message)),
            )
            .await
        }
        OperationKind::SetRead { locator, read } => {
            run_call(
                effect,
                ctx,
                move |c| backend.set_read(c, locator.clone(), *read),
                |_| OperationOutcome::Done,
            )
            .await
        }
        OperationKind::SetStarred { locator, starred } => {
            run_call(
                effect,
                ctx,
                move |c| backend.set_starred(c, locator.clone(), *starred),
                |_| OperationOutcome::Done,
            )
            .await
        }
        OperationKind::Archive(locator) => {
            run_call(
                effect,
                ctx,
                move |c| backend.archive(c, locator.clone()),
                |_| OperationOutcome::Done,
            )
            .await
        }
        OperationKind::Trash(locator) => {
            run_call(
                effect,
                ctx,
                move |c| backend.trash(c, locator.clone()),
                |_| OperationOutcome::Done,
            )
            .await
        }
        OperationKind::SaveDraft { draft } => {
            run_call(
                effect,
                ctx,
                move |c| backend.save_draft(c, draft.as_ref().clone()),
                |remote_id| OperationOutcome::DraftSaved { remote_id },
            )
            .await
        }
        OperationKind::LoadDrafts => {
            run_call(
                effect,
                ctx,
                |c| backend.load_drafts(c),
                OperationOutcome::Drafts,
            )
            .await
        }
        OperationKind::DeleteDraft { draft, .. } => {
            run_call(
                effect,
                ctx,
                move |c| backend.delete_draft(c, draft.as_ref().clone()),
                |_| OperationOutcome::Done,
            )
            .await
        }
        OperationKind::Send { message } => {
            run_call(
                effect,
                ctx,
                move |c| backend.send_message(c, message.as_ref().clone()),
                OperationOutcome::SendOutcome,
            )
            .await
        }
        OperationKind::ReadAttachment { path } => {
            run_call(
                effect,
                ctx,
                move |c| backend.read_attachment(c, path.clone()),
                OperationOutcome::Attachment,
            )
            .await
        }
        OperationKind::ListAttachmentFiles { path } => {
            // The directory walk is file I/O: it hops to the blocking pool
            // so the single-threaded runtime never stalls (plan §3).
            let path = path.clone();
            match tokio::task::spawn_blocking(move || build_attachment_explorer(path.as_deref()))
                .await
            {
                Ok(Ok(explorer)) => Some(Ok(OperationOutcome::Explorer(Box::new(explorer)))),
                Ok(Err(detail)) => Some(Err(plain_failure(effect, &detail))),
                Err(err) => Some(Err(plain_failure(
                    effect,
                    &format!("directory listing task failed: {err}"),
                ))),
            }
        }
        OperationKind::SaveAttachment { request, .. } => {
            run_call(
                effect,
                ctx,
                move |c| backend.save_attachment(c, request.clone()),
                OperationOutcome::SavedPath,
            )
            .await
        }
        OperationKind::OpenPath { path } => {
            tracing::debug!(path = %path.display(), "opening with platform handler");
            // Spawned directly (argv, no shell); not cancellable, so no
            // token dance here — the handler app owns its own lifetime.
            match opener.open(path).await {
                Ok(()) => Some(Ok(OperationOutcome::Done)),
                Err(err) => Some(Err(plain_failure(
                    effect,
                    &format!("`{}` could not be opened: {err}", path.display()),
                ))),
            }
        }
        OperationKind::OpenUrl { url } => {
            tracing::debug!(url = %url, "opening link with platform handler");
            // Spawned directly (argv, no shell); the opener itself refuses
            // non-web schemes before dispatching (ticket hc9n).
            match opener.open_url(url).await {
                Ok(()) => Some(Ok(OperationOutcome::Done)),
                Err(err) => Some(Err(plain_failure(
                    effect,
                    &format!("`{url}` could not be opened: {err}"),
                ))),
            }
        }
        // The external editor never reaches the manager: the main loop
        // runs it synchronously on the terminal owner (plan §14, Phase
        // 11). This arm keeps the match total; reaching it would mean the
        // editor was spawned behind a suspended TUI, so nothing is
        // reported and the registry entry is finished by the real runner.
        OperationKind::EditExternally { .. } => {
            tracing::error!(id = %effect.id, "external editor effect reached the operation manager");
            None
        }
        // Wizard discovery (ADR 0003 §3.3): the injected discoverer runs
        // the bounded blocking client on its own worker thread. `Err` is
        // a discoverer-level break surfaced like every operation failure;
        // `Ok(empty)` strictly means "nothing found in time".
        OperationKind::DiscoverConfig { email } => match discoverer.discover(email).await {
            Ok(services) => Some(Ok(OperationOutcome::Discovered(services))),
            Err(reason) => Some(Err(plain_failure(effect, &reason))),
        },
        // Wizard credential test (ADR 0003 §3.4): a real `himalaya
        // mailbox list` against a temporary 0600 config; the detail of a
        // failure is sanitized below like every other backend error.
        OperationKind::TestAccount { draft } => {
            match crate::backend::himalaya::test_account_mailbox_names(
                himalaya_program,
                draft,
                ctx.cancellation.clone(),
            )
            .await
            {
                Ok(mailboxes) => Some(Ok(OperationOutcome::TestAccountCompleted { mailboxes })),
                Err(err) => operation_failure(effect, err).map(Err),
            }
        }
        // Wizard save (ADR 0003 §3.6): the format-preserving merge runs
        // here (file I/O on the blocking pool), keeping the reducer
        // I/O-free and the runtime loop unblocked.
        OperationKind::SaveAccount { path, draft } => {
            let path = path.clone();
            let draft = draft.as_ref().clone();
            match tokio::task::spawn_blocking(move || {
                crate::config::write::save_account(&path, &draft)
            })
            .await
            {
                Ok(Ok(report)) => Some(Ok(OperationOutcome::AccountSaved {
                    path: report.path,
                    created: report.created,
                    permissions_warning: report.permissions_warning,
                })),
                Ok(Err(detail)) => Some(Err(plain_failure(effect, &detail))),
                Err(err) => Some(Err(plain_failure(
                    effect,
                    &format!("account save task failed: {err}"),
                ))),
            }
        }
        // New-mail notification (ticket b28p): best-effort and off the UI
        // thread. A failure is logged, never modaled.
        OperationKind::Notify { request } => {
            match request {
                // One byte to stdout — but a write to a flow-controlled
                // or full tty buffer can block, so it hops to the blocking
                // pool like the Desktop arm. A single-byte write cannot
                // split a frame's escape sequences (each `write(2)` lands
                // whole), so offloading it is safe for the terminal.
                NotifyRequest::Bell => {
                    let notifier = Arc::clone(notifier);
                    let rung = tokio::task::spawn_blocking(move || notifier.bell()).await;
                    match rung {
                        Ok(Ok(())) => tracing::debug!("bell rung"),
                        Ok(Err(err)) => tracing::warn!(%err, "bell notification failed"),
                        Err(err) => tracing::warn!(%err, "bell task failed"),
                    }
                }
                // notify-rust blocks while the desktop service answers,
                // and the main runtime is single-threaded: the delivery
                // runs on the blocking pool so the frame loop never waits.
                NotifyRequest::Desktop { summary, body } => {
                    let notifier = Arc::clone(notifier);
                    let summary = summary.clone();
                    let body = body.clone();
                    let delivered = tokio::task::spawn_blocking(move || {
                        let result = notifier.notify(&summary, &body);
                        if result.is_ok() {
                            tracing::debug!(summary = %summary, "desktop notification sent");
                        }
                        result
                    })
                    .await;
                    match delivered {
                        Ok(Ok(())) => {}
                        Ok(Err(detail)) => tracing::warn!(detail = %detail, "notification failed"),
                        Err(err) => tracing::warn!(%err, "notification task failed"),
                    }
                }
            }
            Some(Ok(OperationOutcome::Done))
        }
        // ── Summary/message cache (ticket haeb, off-thread I/O) ─────────
        //
        // Every arm hops to the blocking pool: the main runtime is
        // single-threaded, and a cache read or write must never stall the
        // frame loop. Reads never fail — a broken cache degrades to a
        // miss; writes are best-effort (failures are logged inside the
        // cache) and always report Done.
        OperationKind::CacheListLoad {
            mailbox,
            query,
            offset,
            limit,
            ..
        } => {
            let mailbox = mailbox.clone();
            let query = query.clone();
            let offset = *offset;
            let limit = *limit;
            let page = run_cache(cache, move |cache| {
                cache.load(&mailbox, query.as_deref(), offset, limit)
            })
            .await;
            Some(Ok(match page {
                Some(page) => OperationOutcome::CachedPage(page),
                None => OperationOutcome::CacheMiss,
            }))
        }
        OperationKind::CacheListStore {
            mailbox,
            query,
            page,
        } => {
            let mailbox = mailbox.clone();
            let query = query.clone();
            let page = page.as_ref().clone();
            run_cache_store(cache, move |cache| {
                cache.store(&mailbox, query.as_deref(), &page)
            })
            .await;
            Some(Ok(OperationOutcome::Done))
        }
        OperationKind::CacheMailboxesLoad => {
            let mailboxes = run_cache(cache, move |cache| cache.load_mailboxes()).await;
            Some(Ok(match mailboxes {
                Some(mailboxes) => OperationOutcome::CachedMailboxes(mailboxes),
                None => OperationOutcome::CacheMiss,
            }))
        }
        OperationKind::CacheMailboxesStore { mailboxes } => {
            let mailboxes = mailboxes.clone();
            run_cache_store(cache, move |cache| cache.store_mailboxes(&mailboxes)).await;
            Some(Ok(OperationOutcome::Done))
        }
        OperationKind::CacheMessageLoad { locator } => {
            let mailbox = locator.mailbox.clone();
            let id = locator.id.0.clone();
            let message = run_cache(cache, move |cache| cache.load_message(&mailbox, &id)).await;
            Some(Ok(match message {
                Some(message) => OperationOutcome::CachedMessage(Box::new(message)),
                None => OperationOutcome::CacheMiss,
            }))
        }
        OperationKind::CachePreviewLoad { locator } => {
            let mailbox = locator.mailbox.clone();
            let id = locator.id.0.clone();
            let message = run_cache(cache, move |cache| cache.load_message(&mailbox, &id)).await;
            Some(Ok(match message {
                Some(message) => OperationOutcome::CachedMessage(Box::new(message)),
                None => OperationOutcome::CacheMiss,
            }))
        }
        OperationKind::CacheMessageStore {
            mailbox,
            id,
            message,
        } => {
            let mailbox = mailbox.clone();
            let id = id.clone();
            let message = message.as_ref().clone();
            run_cache_store(cache, move |cache| {
                cache.store_message(&mailbox, &id, &message)
            })
            .await;
            Some(Ok(OperationOutcome::Done))
        }
    }
}

/// Run one cache read on the blocking pool. `None` when caching is
/// disabled or the entry is absent — both are ordinary misses.
async fn run_cache<T, F>(cache: &Option<crate::app::page_cache::PageCache>, task: F) -> Option<T>
where
    F: FnOnce(crate::app::page_cache::PageCache) -> Option<T> + Send + 'static,
    T: Send + 'static,
{
    let cache = cache.clone()?;
    tokio::task::spawn_blocking(move || task(cache))
        .await
        .unwrap_or_else(|err| {
            tracing::debug!(%err, "cache read task failed");
            None
        })
}

/// Run one cache write on the blocking pool. Best-effort: a failed task
/// is logged, never surfaced — the cache is an optimization.
async fn run_cache_store<F>(cache: &Option<crate::app::page_cache::PageCache>, task: F)
where
    F: FnOnce(crate::app::page_cache::PageCache) + Send + 'static,
{
    let Some(cache) = cache.clone() else {
        return;
    };
    if let Err(err) = tokio::task::spawn_blocking(move || task(cache)).await {
        tracing::debug!(%err, "cache write task failed");
    }
}

/// Run one backend call through the shared outcome/error translation:
/// success builds the typed outcome via `build`, failure maps through
/// [`operation_failure`] with `None` on cancellation. Collapses the
/// per-kind copies of the same success/error match.
async fn run_call<F, Fut, T>(
    effect: &Effect,
    ctx: &RequestContext,
    call: F,
    build: impl Fn(T) -> OperationOutcome,
) -> Option<Result<OperationOutcome, OperationFailure>>
where
    F: FnOnce(RequestContext) -> Fut,
    Fut: std::future::Future<Output = crate::backend::BackendResult<T>>,
{
    match call(ctx.clone()).await {
        Ok(value) => Some(Ok(build(value))),
        Err(err) => operation_failure(effect, err).map(Err),
    }
}

/// A structural failure that is not a [`BackendError`] (file I/O in the
/// manager: the attachment explorer, the platform opener, the account
/// save, the credential-test timeout): same shape and sanitize path as
/// [`operation_failure`], minus the exit code.
fn plain_failure(effect: &Effect, detail: &str) -> OperationFailure {
    tracing::debug!(id = %effect.id, "operation failed");
    OperationFailure {
        code: None,
        // Sanitized before entering state/UI or logs (plan §12).
        detail: sanitize(detail),
        retry: Some(effect.retry_spec()),
        // Structural failures are never ambiguous (plan §12).
        ambiguous: false,
    }
}

/// Build the attachment chooser's explorer state for `path` (ticket
/// 95x0). `None` means the user's home directory — where attachments
/// usually live — with the working directory as the fallback; an
/// unreachable home falls back to the working directory too.
fn build_attachment_explorer(
    path: Option<&std::path::Path>,
) -> Result<ratatui_explorer::FileExplorer, String> {
    let target = path
        .map(std::path::Path::to_path_buf)
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
        .filter(|dir| dir.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .filter(|dir| dir.is_dir())
        .ok_or_else(|| {
            String::from("no home or working directory available for the attachment chooser")
        })?;
    tracing::debug!(path = %target.display(), "listing attachment chooser directory");
    let listed = target.display().to_string();
    ratatui_explorer::FileExplorerBuilder::build_with_working_dir(target)
        .map_err(|err| format!("could not list `{listed}`: {err}"))
}

/// Map a backend error into a modal-ready failure (plan §12): exit status,
/// sanitized detail, and the typed retry intent. Cancelled operations
/// produce no failure at all. These failures are always *structural*
/// (the request could not even run: missing identity, refused request,
/// spawn I/O) — a send that ran and failed is classified inside
/// [`SendOutcome`](crate::domain::SendOutcome) instead, so nothing here
/// is ever ambiguous (plan §12).
fn operation_failure(effect: &Effect, err: BackendError) -> Option<OperationFailure> {
    if matches!(err, BackendError::Cancelled) {
        return None;
    }
    let (code, detail) = match &err {
        BackendError::Command { code, detail } => (*code, detail.clone()),
        BackendError::InvalidOutput(detail) | BackendError::InvalidRequest(detail) => {
            (None, detail.clone())
        }
        BackendError::File(detail) => (None, detail.clone()),
        BackendError::Io(err) => (None, err.to_string()),
        BackendError::Cancelled => unreachable!("matched above"),
    };
    tracing::debug!(id = %effect.id, ?code, "operation failed");
    Some(OperationFailure {
        code,
        // Sanitized before entering state/UI or logs (plan §12).
        detail: sanitize(&detail),
        retry: Some(effect.retry_spec()),
        // Structural failures are never ambiguous: a send that ran is
        // classified into SendOutcome by the backend (plan §12), and every
        // other operation is safely retryable.
        ambiguous: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::operation::OperationId;
    use crate::backend::BackendResult;
    use crate::domain::{
        Mailbox, MailboxId, Message, MessageId, MessageLocator, MessageSummary, Page, PageRequest,
        SearchRequest,
    };
    use std::time::Duration;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
    use tokio_util::sync::CancellationToken;

    fn ctx(id: u64, token: &CancellationToken) -> RequestContext {
        RequestContext {
            operation: OperationId(id),
            cancellation: token.clone(),
        }
    }

    fn page_request(mailbox: &str) -> PageRequest {
        PageRequest {
            mailbox_id: MailboxId(String::from(mailbox)),
            offset: 0,
            limit: 20,
        }
    }

    /// Fake backend: per-operation latency and optional failure, honoring
    /// cancellation the way the real adapter does.
    struct FakeBackend {
        mailboxes_delay: Duration,
        messages_delay: Duration,
        /// `(exit code, detail)` of a failed command.
        error: Option<(Option<i32>, String)>,
        /// Journal-restored drafts for `load_drafts`.
        drafts: Vec<crate::domain::RestoredDraft>,
    }

    impl FakeBackend {
        fn ok() -> Self {
            Self {
                mailboxes_delay: Duration::ZERO,
                messages_delay: Duration::ZERO,
                error: None,
                drafts: Vec::new(),
            }
        }

        fn failing(code: Option<i32>, detail: &str) -> Self {
            Self {
                mailboxes_delay: Duration::ZERO,
                messages_delay: Duration::ZERO,
                error: Some((code, String::from(detail))),
                drafts: Vec::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl MailBackend for FakeBackend {
        async fn list_mailboxes(&self, req: RequestContext) -> BackendResult<Vec<Mailbox>> {
            tokio::select! {
                _ = tokio::time::sleep(self.mailboxes_delay) => match &self.error {
                    Some((code, detail)) => Err(BackendError::Command {
                        code: *code,
                        detail: detail.clone(),
                    }),
                    None => Ok(Vec::new()),
                },
                _ = req.cancellation.cancelled() => Err(BackendError::Cancelled),
            }
        }

        async fn list_messages(
            &self,
            req: RequestContext,
            _page: PageRequest,
        ) -> BackendResult<Page<MessageSummary>> {
            tokio::select! {
                _ = tokio::time::sleep(self.messages_delay) => match &self.error {
                    Some((code, detail)) => Err(BackendError::Command {
                        code: *code,
                        detail: detail.clone(),
                    }),
                    None => Ok(Page::empty(20)),
                },
                _ = req.cancellation.cancelled() => Err(BackendError::Cancelled),
            }
        }

        async fn search_messages(
            &self,
            req: RequestContext,
            _request: SearchRequest,
        ) -> BackendResult<Page<MessageSummary>> {
            tokio::select! {
                _ = tokio::time::sleep(self.messages_delay) => match &self.error {
                    Some((code, detail)) => Err(BackendError::Command {
                        code: *code,
                        detail: detail.clone(),
                    }),
                    None => Ok(Page::empty(20)),
                },
                _ = req.cancellation.cancelled() => Err(BackendError::Cancelled),
            }
        }

        async fn get_message(
            &self,
            _req: RequestContext,
            _locator: MessageLocator,
        ) -> BackendResult<Message> {
            Ok(Message {
                id: MessageId(String::from("fake")),
                mailbox_id: MailboxId(String::from("fake")),
                headers: Default::default(),
                plain_body: None,
                html_body: None,
                attachments: Vec::new(),
            })
        }

        async fn set_read(
            &self,
            _req: RequestContext,
            _locator: MessageLocator,
            _read: bool,
        ) -> BackendResult<()> {
            Ok(())
        }

        async fn set_starred(
            &self,
            _req: RequestContext,
            _locator: MessageLocator,
            _starred: bool,
        ) -> BackendResult<()> {
            Ok(())
        }

        async fn archive(
            &self,
            _req: RequestContext,
            _locator: MessageLocator,
        ) -> BackendResult<()> {
            Ok(())
        }

        async fn trash(&self, _req: RequestContext, _locator: MessageLocator) -> BackendResult<()> {
            Ok(())
        }

        async fn save_draft(
            &self,
            _req: RequestContext,
            draft: crate::domain::DraftSnapshot,
        ) -> BackendResult<MessageId> {
            // Fake confirmation; failures come from the error injection.
            Ok(MessageId(format!("remote-{}", draft.revision)))
        }

        async fn load_drafts(
            &self,
            _req: RequestContext,
        ) -> BackendResult<Vec<crate::domain::RestoredDraft>> {
            Ok(self.drafts.clone())
        }

        async fn delete_draft(
            &self,
            _req: RequestContext,
            _draft: crate::domain::DraftSnapshot,
        ) -> BackendResult<()> {
            Ok(())
        }

        async fn send_message(
            &self,
            _req: RequestContext,
            _message: crate::domain::OutboundMessage,
        ) -> BackendResult<crate::domain::SendOutcome> {
            Ok(crate::domain::SendOutcome::Sent)
        }

        async fn read_attachment(
            &self,
            _req: RequestContext,
            _path: std::path::PathBuf,
        ) -> BackendResult<crate::domain::DraftAttachment> {
            Ok(crate::domain::DraftAttachment {
                path: std::path::PathBuf::from("/tmp/validated.pdf"),
                name: String::from("validated.pdf"),
                size: 10,
            })
        }

        async fn save_attachment(
            &self,
            _req: RequestContext,
            request: crate::domain::AttachmentRequest,
        ) -> BackendResult<std::path::PathBuf> {
            let dir = request
                .dir
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
            Ok(dir.join(
                request
                    .filename
                    .unwrap_or_else(|| String::from("attachment")),
            ))
        }
    }

    fn manager(
        backend: Arc<dyn MailBackend>,
    ) -> (OperationManager, UnboundedReceiver<OperationResult>) {
        manager_with_opener(backend, Arc::new(RecordingOpener::default()))
    }

    fn manager_with_opener(
        backend: Arc<dyn MailBackend>,
        opener: Arc<dyn PathOpener>,
    ) -> (OperationManager, UnboundedReceiver<OperationResult>) {
        manager_with_notifier(backend, opener, Arc::new(RecordingNotifier::default()))
    }

    fn manager_with_notifier(
        backend: Arc<dyn MailBackend>,
        opener: Arc<dyn PathOpener>,
        notifier: Arc<dyn Notifier>,
    ) -> (OperationManager, UnboundedReceiver<OperationResult>) {
        let (tx, rx) = unbounded_channel();
        (
            OperationManager::new(
                backend,
                opener,
                notifier,
                std::sync::Arc::new(crate::discovery::FakeDiscoverer),
                String::from("himalaya"),
                None,
                tx,
            ),
            rx,
        )
    }

    /// A PathOpener double that records the paths and URLs it was asked to
    /// open.
    #[derive(Default)]
    struct RecordingOpener {
        opened: std::sync::Mutex<Vec<std::path::PathBuf>>,
        urls: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl PathOpener for RecordingOpener {
        async fn open(&self, path: &std::path::Path) -> std::io::Result<()> {
            self.opened
                .lock()
                .expect("opener lock")
                .push(path.to_path_buf());
            Ok(())
        }

        async fn open_url(&self, url: &str) -> std::io::Result<()> {
            self.urls.lock().expect("opener lock").push(url.to_string());
            Ok(())
        }
    }

    impl RecordingOpener {
        fn opened(&self) -> Vec<std::path::PathBuf> {
            self.opened.lock().expect("opener lock").clone()
        }

        fn urls(&self) -> Vec<String> {
            self.urls.lock().expect("opener lock").clone()
        }
    }

    /// A `Notifier` double: records bells and desktop notifications, so
    /// tests never ring a terminal or pop a real notification.
    #[derive(Default)]
    struct RecordingNotifier {
        bells: std::sync::atomic::AtomicUsize,
        desktop: std::sync::Mutex<Vec<(String, String)>>,
    }

    impl Notifier for RecordingNotifier {
        fn bell(&self) -> std::io::Result<()> {
            self.bells.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        fn notify(
            &self,
            summary: &str,
            body: &str,
        ) -> Result<(), crate::backend::notifier::NotifyError> {
            self.desktop
                .lock()
                .expect("notifier lock")
                .push((summary.to_string(), body.to_string()));
            Ok(())
        }
    }

    impl RecordingNotifier {
        fn bell_count(&self) -> usize {
            self.bells.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn desktop(&self) -> Vec<(String, String)> {
            self.desktop.lock().expect("notifier lock").clone()
        }
    }

    fn effect(kind: OperationKind) -> (Effect, CancellationToken) {
        (
            Effect {
                id: OperationId(7),
                kind,
            },
            CancellationToken::new(),
        )
    }

    #[tokio::test]
    async fn launches_effects_and_delivers_results() {
        let (manager, mut rx) = manager(Arc::new(FakeBackend::ok()));
        let (effect, token) = effect(OperationKind::LoadMailboxes);
        manager.launch(effect.clone(), ctx(7, &token));
        let result = rx.recv().await.expect("result");
        assert_eq!(result.id, effect.id);
        assert!(result.outcome.is_ok());
    }

    #[tokio::test]
    async fn slow_operations_do_not_block_following_work() {
        // One 400 ms operation plus one instant one: the fast result must
        // arrive well before the slow one (plan §3: the UI never blocks).
        // Mailbox listing is slow; page loads are instant.
        let backend = Arc::new(FakeBackend {
            mailboxes_delay: Duration::from_millis(400),
            messages_delay: Duration::ZERO,
            error: None,
            drafts: Vec::new(),
        });
        let (manager, mut rx) = manager(backend);
        let (slow, slow_token) = effect(OperationKind::LoadMailboxes);
        let slow_id = slow.id;
        manager.launch(slow, ctx(1, &slow_token));
        let (fast, fast_token) = effect(OperationKind::LoadPage(page_request("inbox")));
        let fast_id = fast.id;
        manager.launch(fast, ctx(2, &fast_token));

        let started = std::time::Instant::now();
        let first = rx.recv().await.expect("fast result");
        assert_eq!(first.id, fast_id, "fast operation finishes first");
        assert!(started.elapsed() < Duration::from_millis(300));
        let second = rx.recv().await.expect("slow result");
        assert_eq!(second.id, slow_id);
    }

    #[tokio::test]
    async fn cancelled_operations_produce_no_result() {
        let backend = Arc::new(FakeBackend {
            mailboxes_delay: Duration::from_secs(30),
            messages_delay: Duration::ZERO,
            error: None,
            drafts: Vec::new(),
        });
        let (manager, mut rx) = manager(backend);
        let (effect, token) = effect(OperationKind::LoadMailboxes);
        manager.launch(effect, ctx(7, &token));
        // Give the task time to start sleeping, then cancel.
        tokio::time::sleep(Duration::from_millis(50)).await;
        token.cancel();
        let result = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
        assert!(result.is_err(), "cancelled operations must be silent");
    }

    #[tokio::test]
    async fn failures_carry_code_retry_and_sanitized_detail() {
        let detail = "password=hunter2 token=abc connect refused";
        let backend = Arc::new(FakeBackend::failing(Some(3), detail));
        let (manager, mut rx) = manager(backend);
        let (effect, token) = effect(OperationKind::LoadMailboxes);
        let retry_spec = effect.retry_spec();
        manager.launch(effect, ctx(7, &token));

        let result = rx.recv().await.expect("result");
        let Err(failure) = result.outcome else {
            panic!("expected failure");
        };
        assert_eq!(failure.code, Some(3));
        assert_eq!(failure.retry, Some(retry_spec));
        assert!(!failure.ambiguous);
        assert!(
            !failure.detail.contains("hunter2"),
            "detail: {}",
            failure.detail
        );
        assert!(
            !failure.detail.contains("abc"),
            "detail: {}",
            failure.detail
        );
        assert!(failure.detail.contains("connect refused"));
    }

    #[tokio::test]
    async fn open_path_effects_spawn_the_platform_opener_directly() {
        let backend = Arc::new(FakeBackend::ok());
        let opener = Arc::new(RecordingOpener::default());
        let (manager, mut rx) = manager_with_opener(backend, Arc::clone(&opener) as _);
        let path = std::path::PathBuf::from("/tmp/report final (1).pdf");
        let (effect, token) = effect(OperationKind::OpenPath { path: path.clone() });
        assert!(!effect.kind.is_cancellable(), "opens are not cancellable");
        manager.launch(effect, ctx(9, &token));
        let result = rx.recv().await.expect("result");
        assert_eq!(result.outcome, Ok(OperationOutcome::Done));
        // The path traveled whole — spaces and parens intact, no shell.
        assert_eq!(opener.opened(), vec![path]);
    }

    #[tokio::test]
    async fn open_failures_name_the_path_and_stay_retryable() {
        struct RefusingOpener;
        #[async_trait::async_trait]
        impl PathOpener for RefusingOpener {
            async fn open(&self, _path: &std::path::Path) -> std::io::Result<()> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "no opener",
                ))
            }

            async fn open_url(&self, _url: &str) -> std::io::Result<()> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "no opener",
                ))
            }
        }
        let backend = Arc::new(FakeBackend::ok());
        let (manager, mut rx) = manager_with_opener(backend, Arc::new(RefusingOpener));
        let (effect, token) = effect(OperationKind::OpenPath {
            path: std::path::PathBuf::from("/tmp/x.pdf"),
        });
        let retry = effect.retry_spec();
        manager.launch(effect, ctx(9, &token));
        let result = rx.recv().await.expect("result");
        let Err(failure) = result.outcome else {
            panic!("expected failure");
        };
        assert!(failure.detail.contains("/tmp/x.pdf"));
        assert!(failure.detail.contains("no opener"));
        assert_eq!(failure.retry, Some(retry));
    }

    #[tokio::test]
    async fn open_url_effects_spawn_the_platform_opener_directly() {
        let backend = Arc::new(FakeBackend::ok());
        let opener = Arc::new(RecordingOpener::default());
        let (manager, mut rx) = manager_with_opener(backend, Arc::clone(&opener) as _);
        let url = String::from("https://example.org/a?b=1&c=2#frag");
        let (effect, token) = effect(OperationKind::OpenUrl { url: url.clone() });
        assert!(!effect.kind.is_cancellable(), "opens are not cancellable");
        manager.launch(effect, ctx(11, &token));
        let result = rx.recv().await.expect("result");
        assert_eq!(result.outcome, Ok(OperationOutcome::Done));
        // The whole URL traveled — query and fragment intact, no shell.
        assert_eq!(opener.urls(), vec![url]);
    }

    #[tokio::test]
    async fn bell_notifications_ring_via_the_injected_notifier() {
        let backend = Arc::new(FakeBackend::ok());
        let notifier = Arc::new(RecordingNotifier::default());
        let (manager, mut rx) = manager_with_notifier(
            backend,
            Arc::new(RecordingOpener::default()),
            Arc::clone(&notifier) as _,
        );
        let (effect, token) = effect(OperationKind::Notify {
            request: NotifyRequest::Bell,
        });
        manager.launch(effect, ctx(21, &token));
        let result = rx.recv().await.expect("result");
        assert_eq!(result.outcome, Ok(OperationOutcome::Done));
        assert_eq!(notifier.bell_count(), 1, "the bell rings once");
    }

    #[tokio::test]
    async fn desktop_notifications_carry_the_reducer_text() {
        let backend = Arc::new(FakeBackend::ok());
        let notifier = Arc::new(RecordingNotifier::default());
        let (manager, mut rx) = manager_with_notifier(
            backend,
            Arc::new(RecordingOpener::default()),
            Arc::clone(&notifier) as _,
        );
        let (effect, token) = effect(OperationKind::Notify {
            request: NotifyRequest::Desktop {
                summary: String::from("Ada Example"),
                body: String::from("Lunch?"),
            },
        });
        manager.launch(effect, ctx(22, &token));
        let result = rx.recv().await.expect("result");
        assert_eq!(result.outcome, Ok(OperationOutcome::Done));
        assert_eq!(
            notifier.desktop(),
            vec![(String::from("Ada Example"), String::from("Lunch?"))]
        );
    }

    #[tokio::test]
    async fn a_failing_notifier_still_completes_the_operation() {
        struct RefusingNotifier;
        impl Notifier for RefusingNotifier {
            fn bell(&self) -> std::io::Result<()> {
                Err(std::io::Error::other("no bell"))
            }

            fn notify(
                &self,
                _summary: &str,
                _body: &str,
            ) -> Result<(), crate::backend::notifier::NotifyError> {
                Err(crate::backend::notifier::NotifyError(String::from(
                    "no notification service",
                )))
            }
        }
        let backend = Arc::new(FakeBackend::ok());
        let (manager, mut rx) = manager_with_notifier(
            backend,
            Arc::new(RecordingOpener::default()),
            Arc::new(RefusingNotifier),
        );
        let (effect, token) = effect(OperationKind::Notify {
            request: NotifyRequest::Desktop {
                summary: String::from("x"),
                body: String::from("y"),
            },
        });
        manager.launch(effect, ctx(23, &token));
        // Best-effort: a notification failure is logged, never surfaced.
        let result = rx.recv().await.expect("result");
        assert_eq!(result.outcome, Ok(OperationOutcome::Done));
    }

    #[tokio::test]
    async fn open_url_failures_name_the_link_and_stay_retryable() {
        struct RefusingUrlOpener;
        #[async_trait::async_trait]
        impl PathOpener for RefusingUrlOpener {
            async fn open(&self, _path: &std::path::Path) -> std::io::Result<()> {
                Ok(())
            }

            async fn open_url(&self, _url: &str) -> std::io::Result<()> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "no browser",
                ))
            }
        }
        let backend = Arc::new(FakeBackend::ok());
        let (manager, mut rx) = manager_with_opener(backend, Arc::new(RefusingUrlOpener));
        let (effect, token) = effect(OperationKind::OpenUrl {
            url: String::from("https://example.org/x"),
        });
        let retry = effect.retry_spec();
        manager.launch(effect, ctx(12, &token));
        let result = rx.recv().await.expect("result");
        let Err(failure) = result.outcome else {
            panic!("expected failure");
        };
        assert!(failure.detail.contains("https://example.org/x"));
        assert!(failure.detail.contains("no browser"));
        assert_eq!(failure.retry, Some(retry));
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::app::effect::Effect;
    use crate::app::operation::{OperationId, OperationOutcome};
    use crate::app::page_cache::{CacheLimits, PageCache};
    use crate::backend::BackendResult;
    use crate::domain::{
        Mailbox, MailboxId, Message, MessageId, MessageLocator, MessageSummary, Page, PageRequest,
    };
    use std::time::Duration;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
    use tokio_util::sync::CancellationToken;

    fn manager_with_cache(
        cache: Option<PageCache>,
    ) -> (OperationManager, UnboundedReceiver<OperationResult>) {
        let (tx, rx) = unbounded_channel();
        (
            OperationManager::new(
                Arc::new(StubBackend),
                Arc::new(InertOpener),
                Arc::new(InertNotifier),
                Arc::new(crate::discovery::FakeDiscoverer),
                String::from("himalaya"),
                cache,
                tx,
            ),
            rx,
        )
    }

    fn ctx(id: u64, token: &CancellationToken) -> RequestContext {
        RequestContext {
            operation: OperationId(id),
            cancellation: token.clone(),
        }
    }

    struct InertOpener;

    #[async_trait::async_trait]
    impl PathOpener for InertOpener {
        async fn open(&self, _path: &std::path::Path) -> std::io::Result<()> {
            Err(std::io::Error::other("unused"))
        }

        async fn open_url(&self, _url: &str) -> std::io::Result<()> {
            Err(std::io::Error::other("unused"))
        }
    }

    struct InertNotifier;

    impl Notifier for InertNotifier {
        fn bell(&self) -> std::io::Result<()> {
            Err(std::io::Error::other("unused"))
        }

        fn notify(
            &self,
            _summary: &str,
            _body: &str,
        ) -> Result<(), crate::backend::notifier::NotifyError> {
            Err(crate::backend::notifier::NotifyError(String::from(
                "unused",
            )))
        }
    }

    /// A backend the cache arms never touch: cache work is file I/O only.
    struct StubBackend;

    #[async_trait::async_trait]
    impl MailBackend for StubBackend {
        async fn list_mailboxes(&self, _req: RequestContext) -> BackendResult<Vec<Mailbox>> {
            Err(BackendError::InvalidRequest(String::from("unused")))
        }

        async fn list_messages(
            &self,
            _req: RequestContext,
            _page: PageRequest,
        ) -> BackendResult<Page<MessageSummary>> {
            Err(BackendError::InvalidRequest(String::from("unused")))
        }

        async fn search_messages(
            &self,
            _req: RequestContext,
            _request: crate::domain::SearchRequest,
        ) -> BackendResult<Page<MessageSummary>> {
            Err(BackendError::InvalidRequest(String::from("unused")))
        }

        async fn get_message(
            &self,
            _req: RequestContext,
            _locator: MessageLocator,
        ) -> BackendResult<Message> {
            Err(BackendError::InvalidRequest(String::from("unused")))
        }

        async fn set_read(
            &self,
            _req: RequestContext,
            _locator: MessageLocator,
            _read: bool,
        ) -> BackendResult<()> {
            Err(BackendError::InvalidRequest(String::from("unused")))
        }

        async fn set_starred(
            &self,
            _req: RequestContext,
            _locator: MessageLocator,
            _starred: bool,
        ) -> BackendResult<()> {
            Err(BackendError::InvalidRequest(String::from("unused")))
        }

        async fn archive(
            &self,
            _req: RequestContext,
            _locator: MessageLocator,
        ) -> BackendResult<()> {
            Err(BackendError::InvalidRequest(String::from("unused")))
        }

        async fn trash(&self, _req: RequestContext, _locator: MessageLocator) -> BackendResult<()> {
            Err(BackendError::InvalidRequest(String::from("unused")))
        }

        async fn save_draft(
            &self,
            _req: RequestContext,
            _draft: crate::domain::DraftSnapshot,
        ) -> BackendResult<MessageId> {
            Err(BackendError::InvalidRequest(String::from("unused")))
        }

        async fn load_drafts(
            &self,
            _req: RequestContext,
        ) -> BackendResult<Vec<crate::domain::RestoredDraft>> {
            Err(BackendError::InvalidRequest(String::from("unused")))
        }

        async fn delete_draft(
            &self,
            _req: RequestContext,
            _draft: crate::domain::DraftSnapshot,
        ) -> BackendResult<()> {
            Err(BackendError::InvalidRequest(String::from("unused")))
        }

        async fn send_message(
            &self,
            _req: RequestContext,
            _message: crate::domain::OutboundMessage,
        ) -> BackendResult<crate::domain::SendOutcome> {
            Err(BackendError::InvalidRequest(String::from("unused")))
        }

        async fn read_attachment(
            &self,
            _req: RequestContext,
            _path: std::path::PathBuf,
        ) -> BackendResult<crate::domain::DraftAttachment> {
            Err(BackendError::InvalidRequest(String::from("unused")))
        }

        async fn save_attachment(
            &self,
            _req: RequestContext,
            _request: crate::domain::AttachmentRequest,
        ) -> BackendResult<std::path::PathBuf> {
            Err(BackendError::InvalidRequest(String::from("unused")))
        }
    }

    fn effect(kind: OperationKind) -> (Effect, CancellationToken) {
        (
            Effect {
                id: OperationId(7),
                kind,
            },
            CancellationToken::new(),
        )
    }

    fn summary(id: &str) -> MessageSummary {
        MessageSummary {
            id: MessageId(String::from(id)),
            mailbox_id: MailboxId(String::from("INBOX")),
            message_id: None,
            from: Vec::new(),
            to: Vec::new(),
            subject: String::from(id),
            snippet: None,
            timestamp: chrono::DateTime::parse_from_rfc3339("2026-09-02T10:00:00+00:00")
                .expect("fixed ts"),
            is_read: false,
            is_starred: false,
            has_attachments: false,
        }
    }

    #[tokio::test]
    async fn a_cached_page_read_serves_the_page_off_thread() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = PageCache::open(dir.path().to_path_buf(), CacheLimits::default());
        cache.store(
            &MailboxId(String::from("INBOX")),
            None,
            &crate::domain::Page {
                items: vec![summary("m1")],
                offset: 0,
                limit: 20,
                total: None,
            },
        );
        let (manager, mut rx) = manager_with_cache(Some(cache));
        let (effect, token) = effect(OperationKind::CacheListLoad {
            mailbox: MailboxId(String::from("INBOX")),
            query: None,
            offset: 0,
            limit: 20,
            fresh_background_on_hit: true,
        });
        manager.launch(effect, ctx(7, &token));
        let result = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("result")
            .expect("result");
        assert!(matches!(
            result.outcome,
            Ok(OperationOutcome::CachedPage(page)) if page.items.len() == 1
        ));
    }

    #[tokio::test]
    async fn a_missing_cache_entry_reads_as_a_miss_never_a_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = PageCache::open(dir.path().to_path_buf(), CacheLimits::default());
        let (manager, mut rx) = manager_with_cache(Some(cache));
        let (effect, token) = effect(OperationKind::CacheListLoad {
            mailbox: MailboxId(String::from("INBOX")),
            query: None,
            offset: 0,
            limit: 20,
            fresh_background_on_hit: true,
        });
        manager.launch(effect, ctx(7, &token));
        let result = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("result")
            .expect("result");
        assert_eq!(result.outcome, Ok(OperationOutcome::CacheMiss));
    }

    #[tokio::test]
    async fn cache_stores_write_off_thread_and_report_done() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = PageCache::open(dir.path().to_path_buf(), CacheLimits::default());
        let (manager, mut rx) = manager_with_cache(Some(cache.clone()));
        let (effect, token) = effect(OperationKind::CacheMessageStore {
            mailbox: MailboxId(String::from("INBOX")),
            id: String::from("m1"),
            message: Box::new(Message {
                id: MessageId(String::from("m1")),
                mailbox_id: MailboxId(String::from("INBOX")),
                headers: Default::default(),
                plain_body: Some(String::from("body")),
                html_body: None,
                attachments: Vec::new(),
            }),
        });
        manager.launch(effect, ctx(7, &token));
        let result = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("result")
            .expect("result");
        assert_eq!(result.outcome, Ok(OperationOutcome::Done));
        // The write actually landed (on the blocking pool).
        assert!(
            cache
                .load_message(&MailboxId(String::from("INBOX")), "m1")
                .is_some()
        );
    }

    #[tokio::test]
    async fn cache_work_without_a_cache_is_a_miss_or_a_noop() {
        let (manager, mut rx) = manager_with_cache(None);
        let (load, token) = effect(OperationKind::CacheMailboxesLoad);
        manager.launch(load, ctx(1, &token));
        let result = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("result")
            .expect("result");
        assert_eq!(result.outcome, Ok(OperationOutcome::CacheMiss));

        let (store, token) = effect(OperationKind::CacheMailboxesStore {
            mailboxes: Vec::new(),
        });
        manager.launch(store, ctx(2, &token));
        let result = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("result")
            .expect("result");
        assert_eq!(result.outcome, Ok(OperationOutcome::Done));
    }
}
