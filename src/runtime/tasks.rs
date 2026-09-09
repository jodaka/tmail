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
use crate::app::operation::{OperationFailure, OperationKind, OperationOutcome, OperationResult};
use crate::app::sanitize::sanitize;
use crate::backend::{BackendError, MailBackend, PathOpener, RequestContext};
use crate::discovery::EmailConfigDiscoverer;

/// Spawns backend tasks for the effects the reducer emits.
pub struct OperationManager {
    backend: Arc<dyn MailBackend>,
    /// Platform open-with adapter for `OpenPath` effects (plan §15,
    /// Phase 8.5): `open`/`xdg-open`, spawned directly.
    opener: Arc<dyn PathOpener>,
    /// The email settings discoverer for the wizard's `DiscoverConfig`
    /// effects (ADR 0003 §3.7): injected like the backend and opener,
    /// so tests and smoke runs swap in a fake.
    discoverer: Arc<dyn EmailConfigDiscoverer>,
    /// The himalaya executable the wizard credential test runs
    /// (ADR 0003 §3.4): the backend's own program name is private to
    /// the adapter, so the manager carries it for the test invocation.
    /// Overridable so contract tests point it at the fake.
    himalaya_program: String,
    results: UnboundedSender<OperationResult>,
}

impl OperationManager {
    pub fn new(
        backend: Arc<dyn MailBackend>,
        opener: Arc<dyn PathOpener>,
        discoverer: Arc<dyn EmailConfigDiscoverer>,
        himalaya_program: String,
        results: UnboundedSender<OperationResult>,
    ) -> Self {
        Self {
            backend,
            opener,
            discoverer,
            himalaya_program,
            results,
        }
    }

    /// Launch one effect. The cancellation token comes from the operation
    /// registry (`AppState.operations`), so `Esc` reaches the child process.
    pub fn launch(&self, effect: Effect, ctx: RequestContext) {
        let backend = Arc::clone(&self.backend);
        let opener = Arc::clone(&self.opener);
        let discoverer = Arc::clone(&self.discoverer);
        let himalaya_program = self.himalaya_program.clone();
        let results = self.results.clone();
        let id = effect.id;
        tokio::spawn(async move {
            tracing::debug!(id = %id, "operation launched");
            match run_effect(
                &backend,
                &opener,
                &discoverer,
                &himalaya_program,
                effect,
                ctx,
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
async fn run_effect(
    backend: &Arc<dyn MailBackend>,
    opener: &Arc<dyn PathOpener>,
    discoverer: &Arc<dyn EmailConfigDiscoverer>,
    himalaya_program: &str,
    effect: Effect,
    ctx: RequestContext,
) -> Option<Result<OperationOutcome, OperationFailure>> {
    match effect.kind.clone() {
        OperationKind::LoadMailboxes => match backend.list_mailboxes(ctx).await {
            Ok(mailboxes) => Some(Ok(OperationOutcome::Mailboxes(mailboxes))),
            Err(err) => operation_failure(&effect, err).map(Err),
        },
        OperationKind::LoadPage(request) => match backend.list_messages(ctx, request).await {
            Ok(page) => Some(Ok(OperationOutcome::Page(page))),
            Err(err) => operation_failure(&effect, err).map(Err),
        },
        OperationKind::Search(request) => match backend.search_messages(ctx, request).await {
            Ok(page) => Some(Ok(OperationOutcome::Page(page))),
            Err(err) => operation_failure(&effect, err).map(Err),
        },
        OperationKind::LoadMessage(locator) => match backend.get_message(ctx, locator).await {
            Ok(message) => Some(Ok(OperationOutcome::Message(Box::new(message)))),
            Err(err) => operation_failure(&effect, err).map(Err),
        },
        // Same backend call as the reader's load, different consumer: the
        // reducer turns the fetched copy into a composer draft (the Drafts
        // list's Enter).
        OperationKind::OpenDraft(locator) => match backend.get_message(ctx, locator).await {
            Ok(message) => Some(Ok(OperationOutcome::Message(Box::new(message)))),
            Err(err) => operation_failure(&effect, err).map(Err),
        },
        // Same backend call as the reader's load (ticket wxtx), different
        // consumer: the list's faded preview. Failures land in the reducer
        // as ordinary failures, which it logs and drops for this kind.
        OperationKind::Preview(locator) => match backend.get_message(ctx, locator).await {
            Ok(message) => Some(Ok(OperationOutcome::Message(Box::new(message)))),
            Err(err) => operation_failure(&effect, err).map(Err),
        },
        OperationKind::SetRead { locator, read } => {
            match backend.set_read(ctx, locator, read).await {
                Ok(()) => Some(Ok(OperationOutcome::Done)),
                Err(err) => operation_failure(&effect, err).map(Err),
            }
        }
        OperationKind::SetStarred { locator, starred } => {
            match backend.set_starred(ctx, locator, starred).await {
                Ok(()) => Some(Ok(OperationOutcome::Done)),
                Err(err) => operation_failure(&effect, err).map(Err),
            }
        }
        OperationKind::Archive(locator) => match backend.archive(ctx, locator).await {
            Ok(()) => Some(Ok(OperationOutcome::Done)),
            Err(err) => operation_failure(&effect, err).map(Err),
        },
        OperationKind::Trash(locator) => match backend.trash(ctx, locator).await {
            Ok(()) => Some(Ok(OperationOutcome::Done)),
            Err(err) => operation_failure(&effect, err).map(Err),
        },
        OperationKind::SaveDraft { draft } => match backend.save_draft(ctx, *draft).await {
            Ok(remote_id) => Some(Ok(OperationOutcome::DraftSaved { remote_id })),
            Err(err) => operation_failure(&effect, err).map(Err),
        },
        OperationKind::LoadDrafts => match backend.load_drafts(ctx).await {
            Ok(drafts) => Some(Ok(OperationOutcome::Drafts(drafts))),
            Err(err) => operation_failure(&effect, err).map(Err),
        },
        OperationKind::DeleteDraft { .. } => {
            match backend.delete_draft(ctx, effect_draft(&effect)).await {
                Ok(()) => Some(Ok(OperationOutcome::Done)),
                Err(err) => operation_failure(&effect, err).map(Err),
            }
        }
        OperationKind::Send { message } => match backend.send_message(ctx, *message).await {
            // Classified delivery outcomes (plan §12) — including failures
            // — travel as success payloads; only structural refusals
            // (spawn I/O, refused request) come back as errors.
            Ok(outcome) => Some(Ok(OperationOutcome::SendOutcome(outcome))),
            Err(err) => operation_failure(&effect, err).map(Err),
        },
        OperationKind::ReadAttachment { path } => match backend.read_attachment(ctx, path).await {
            Ok(attachment) => Some(Ok(OperationOutcome::Attachment(attachment))),
            Err(err) => operation_failure(&effect, err).map(Err),
        },
        OperationKind::ListAttachmentFiles { path } => {
            match build_attachment_explorer(path.as_deref()) {
                Ok(explorer) => Some(Ok(OperationOutcome::Explorer(Box::new(explorer)))),
                Err(detail) => Some(Err(OperationFailure {
                    code: None,
                    detail: sanitize(&detail),
                    retry: Some(effect.retry_spec()),
                    ambiguous: false,
                })),
            }
        }
        OperationKind::SaveAttachment { request, .. } => {
            match backend.save_attachment(ctx, request).await {
                Ok(path) => Some(Ok(OperationOutcome::SavedPath(path))),
                Err(err) => operation_failure(&effect, err).map(Err),
            }
        }
        OperationKind::OpenPath { path } => {
            tracing::debug!(path = %path.display(), "opening with platform handler");
            // Spawned directly (argv, no shell); not cancellable, so no
            // token dance here — the handler app owns its own lifetime.
            match opener.open(&path) {
                Ok(()) => Some(Ok(OperationOutcome::Done)),
                Err(err) => {
                    let failure = OperationFailure {
                        code: None,
                        detail: sanitize(&format!(
                            "`{}` could not be opened: {err}",
                            path.display()
                        )),
                        retry: Some(effect.retry_spec()),
                        ambiguous: false,
                    };
                    Some(Err(failure))
                }
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
        // the bounded blocking client on its own worker thread.
        OperationKind::DiscoverConfig { email } => {
            let services = discoverer.discover(&email).await;
            Some(Ok(OperationOutcome::Discovered(services)))
        }
        // Wizard credential test (ADR 0003 §3.4): a real `himalaya
        // mailbox list` against a temporary 0600 config; the detail of a
        // failure is sanitized below like every other backend error.
        OperationKind::TestAccount { draft } => {
            match crate::backend::himalaya::test_account_mailbox_names(
                himalaya_program,
                &draft,
                ctx.cancellation.clone(),
            )
            .await
            {
                Ok(mailboxes) => Some(Ok(OperationOutcome::TestAccountCompleted { mailboxes })),
                Err(err) => operation_failure(&effect, err).map(Err),
            }
        }
        // Wizard save (ADR 0003 §3.6): the format-preserving merge runs
        // here (file I/O), keeping the reducer I/O-free.
        OperationKind::SaveAccount { path, draft } => {
            match crate::config::write::save_account(&path, &draft) {
                Ok(report) => Some(Ok(OperationOutcome::AccountSaved {
                    path: report.path,
                    created: report.created,
                    permissions_warning: report.permissions_warning,
                })),
                Err(detail) => Some(Err(OperationFailure {
                    code: None,
                    detail: sanitize(&detail),
                    retry: Some(effect.retry_spec()),
                    ambiguous: false,
                })),
            }
        }
    }
}

/// The snapshot carried by a delete-draft effect.
fn effect_draft(effect: &Effect) -> crate::domain::DraftSnapshot {
    match &effect.kind {
        OperationKind::DeleteDraft { draft, .. } => (**draft).clone(),
        other => unreachable!("effect_draft on non-delete kind: {other:?}"),
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
        .unwrap_or_default();
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
        let (tx, rx) = unbounded_channel();
        (
            OperationManager::new(
                backend,
                opener,
                std::sync::Arc::new(crate::discovery::FakeDiscoverer),
                String::from("himalaya"),
                tx,
            ),
            rx,
        )
    }

    /// A PathOpener double that records the paths it was asked to open.
    #[derive(Default)]
    struct RecordingOpener {
        opened: std::sync::Mutex<Vec<std::path::PathBuf>>,
    }

    impl PathOpener for RecordingOpener {
        fn open(&self, path: &std::path::Path) -> std::io::Result<()> {
            self.opened
                .lock()
                .expect("opener lock")
                .push(path.to_path_buf());
            Ok(())
        }
    }

    impl RecordingOpener {
        fn opened(&self) -> Vec<std::path::PathBuf> {
            self.opened.lock().expect("opener lock").clone()
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
        impl PathOpener for RefusingOpener {
            fn open(&self, _path: &std::path::Path) -> std::io::Result<()> {
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
}
