//! `HimalayaCliBackend`: the Phase 2 `MailBackend` implementation on top of
//! the installed Himalaya CLI (ADR 0001 decision 1).
//!
//! Phase 2 covers the mailbox and message-list operations; read, flags,
//! search, send, and draft operations extend this impl in their phases. The
//! backend is stateless per call apart from its configuration, so it can be
//! shared freely once the Phase 3 operation manager holds an `Arc` to it.

mod command;
pub(crate) mod dto;
#[cfg(feature = "test-fixtures")]
pub mod fixtures;
pub(crate) mod map;
mod process;

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address as MailAddress;

use crate::backend::journal::DraftJournal;
use crate::backend::tester::AccountTester;
use crate::backend::traits::{BackendError, BackendResult, MailBackend, RequestContext};
use crate::config::Config;
use crate::domain::{
    AttachmentRequest, DraftAttachment, DraftSnapshot, Mailbox, MailboxId, MailboxRole, Message,
    MessageId, MessageLocator, MessageSummary, OutboundMessage, Page, PageRequest, RestoredDraft,
    SearchRequest, SendOutcome,
};

/// Whether the backend's executable can be found before anything is
/// spawned (plan §17 startup validation: a missing Himalaya executable is
/// reported up front, not as the first operation's failure). A name with a
/// path separator must exist as a file; otherwise the `PATH` is searched.
pub fn executable_available(program: &str) -> bool {
    crate::domain::paths::program_on_path_exists(program)
}

/// The default executable for the Himalaya adapter (issue 5ab7): one
/// constant instead of scattered literals, so `from_config`, the startup
/// check, and the wizard's credential test all name the same program and
/// fake-program tests cannot drift from the real wiring.
pub const PROGRAM: &str = "himalaya";

/// How long the wizard's credential test may run before it fails
/// (ADR 0003 §3.4: a 30 s timeout bounds a hung endpoint).
const TEST_ACCOUNT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The wizard credential test (ADR 0003 §3.4): writes `draft` into a
/// fresh temporary 0600 config file, runs `himalaya -c <temp> mailbox
/// list -a <account> --json` through the same process plumbing as every
/// other backend call, and returns the mailbox names. The temp file is
/// deleted in all outcomes (guarded `NamedTempFile` drop); nothing
/// containing the credential is ever written to the real config.
async fn test_account_mailbox_names(
    program: &str,
    draft: &crate::config::write::DraftAccount,
    cancellation: tokio_util::sync::CancellationToken,
) -> BackendResult<Vec<String>> {
    // Guarded creation: the file exists only after the mode is 0600 (on
    // unix, tempfile already creates it 0600; re-assert defensively)
    // and before any secret is placed inside. The tempfile work hops to
    // the blocking pool — the single-threaded runtime never waits on a
    // disk (plan §3).
    let fragment = crate::config::write::draft_account_fragment(draft);
    let temp = tokio::task::spawn_blocking(move || -> BackendResult<tempfile::NamedTempFile> {
        let temp = tempfile::Builder::new()
            .prefix("tmail-wizard-")
            .suffix(".toml")
            .tempfile()
            .map_err(|err| {
                BackendError::File(format!("could not create the test config: {err}"))
            })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temp.as_file()
                .set_permissions(std::fs::Permissions::from_mode(
                    crate::domain::private_fs::OWNER_FILE_MODE,
                ))
                .map_err(|err| {
                    BackendError::File(format!("could not secure the test config: {err}"))
                })?;
        }
        temp.as_file()
            .write_all(fragment.as_bytes())
            .map_err(|err| BackendError::File(format!("could not write the test config: {err}")))?;
        temp.as_file()
            .flush()
            .map_err(|err| BackendError::File(format!("could not write the test config: {err}")))?;
        Ok(temp)
    })
    .await
    .map_err(|join| {
        BackendError::Io(std::io::Error::other(format!(
            "test config task failed: {join}"
        )))
    })??;

    let path = temp.path().to_path_buf();
    let argv = command::mailbox_list_argv(Some(&path), Some(&draft.name), false);

    // The timeout wraps the run: on `Elapsed` the run future is dropped,
    // which kills the child (`kill_on_drop`), and the temp file drops
    // right after — nothing lingers in either failure mode.
    let run = process::run(program, &argv, &cancellation);
    let output = tokio::time::timeout(TEST_ACCOUNT_TIMEOUT, run)
        .await
        .map_err(|_| BackendError::Command {
            program: program.to_owned(),
            code: None,
            detail: String::from(
                "the connection test timed out after 30s (check the server settings)",
            ),
        })??;

    let dto: dto::MailboxesDto = process::decode_on_pool(output).await?;
    // Drop the temp file (deleting it) before reporting.
    drop(temp);
    Ok(map::mailboxes(dto, &HashMap::new())
        .into_iter()
        .map(|mailbox| mailbox.name)
        .collect())
}

/// The real credential-test adapter (ADR 0003 §3.4): wraps
/// [`test_account_mailbox_names`] with the adapter's own program name, so
/// the operation manager never names a concrete backend (ticket 55t6).
#[derive(Debug, Clone)]
pub struct HimalayaAccountTester {
    /// Executable name or path, mirroring [`HimalayaCliBackend`]'s
    /// `program` field; overridable so tests can point at a fake.
    program: String,
}

impl HimalayaAccountTester {
    /// Test through the given executable name or path.
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }
}

#[async_trait]
impl AccountTester for HimalayaAccountTester {
    async fn test_account(
        &self,
        draft: &crate::config::write::DraftAccount,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> BackendResult<Vec<String>> {
        test_account_mailbox_names(&self.program, draft, cancellation).await
    }
}

/// Drives the `himalaya` executable with argv-only child processes.
#[derive(Debug, Clone)]
pub struct HimalayaCliBackend {
    /// Executable name or path; overridable so contract tests can point at
    /// a fake himalaya.
    program: String,
    /// Forwarded as `-c` when known (one shared config file, ADR 0001
    /// finding 13).
    config_path: Option<PathBuf>,
    /// Forwarded as `-a` when configured.
    account: Option<String>,
    /// `[accounts.<account>.mailbox.alias]` entries used to resolve mailbox
    /// roles inside the adapter (ADR 0001).
    aliases: HashMap<String, String>,
    /// Last successful `mailbox list` result, so semantic operations
    /// (archive) resolve their target mailbox from what the account
    /// actually exposes instead of guessing names (ADR 0001: map semantic
    /// operations inside the backend adapter). `Arc` keeps the struct
    /// cheaply cloneable.
    ///
    /// The lock is deliberately `std::sync::RwLock`: it is never held
    /// across an `.await`, its critical sections are a `Vec` clone of a
    /// handful of mailboxes, and every writer runs on the single
    /// main-runtime thread — so there is no executor stall to avoid
    /// (ticket tnc1 review) and no async lock has to infect the sync
    /// callers (`mailbox_for_role`, used from sync cleanup paths).
    mailboxes: Arc<RwLock<Option<Vec<Mailbox>>>>,
    /// Tmail-owned crash-safe draft journal (ADR 0002 §D.1): every revision
    /// is recorded here before any remote call. `None` when no data
    /// directory could be derived (`$TMAIL_DATA_DIR` unset and no `$HOME`)
    /// — draft operations then fail with an explicit request error
    /// instead of an obscure filesystem failure behind a sentinel path.
    journal: Option<DraftJournal>,
    account_email: Option<String>,
    account_display_name: Option<String>,
    /// `[tmail.attachments].downloads_dir` (plan §17), as written; a leading
    /// `~` is expanded at use time. `None` falls back to `$HOME/Downloads`.
    downloads_dir: Option<PathBuf>,
    /// At most one stray-draft sweep runs at a time (ticket 8s0g): sweeps
    /// are detached so cleanup never delays the save result, and their
    /// token belongs to the already-finished save — rapid saves would
    /// otherwise pile up unbounded detached sweeps. A skipped sweep is
    /// harmless: every copy carries the same stable `Message-ID`, so the
    /// next save's sweep removes whatever this one missed.
    sweeps: Arc<tokio::sync::Semaphore>,
}

impl HimalayaCliBackend {
    pub fn new(
        program: impl Into<String>,
        config_path: Option<PathBuf>,
        account: Option<String>,
        aliases: HashMap<String, String>,
    ) -> Self {
        // The journal is scoped per account (ticket c0n0): a draft
        // recorded under one account is never restored under another.
        let journal = DraftJournal::open_default(account.as_deref());
        Self {
            program: program.into(),
            config_path,
            account,
            aliases,
            mailboxes: Arc::new(RwLock::new(None)),
            journal,
            account_email: None,
            account_display_name: None,
            downloads_dir: None,
            sweeps: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }

    /// Override the journal location (tests, explicit data dirs).
    pub fn with_journal(mut self, journal: DraftJournal) -> Self {
        self.journal = Some(journal);
        self
    }

    /// The journal for a draft write, or the typed error explaining why
    /// there is none (no data dir derivable at startup).
    fn journal_required(&self) -> BackendResult<&DraftJournal> {
        self.journal.as_ref().ok_or_else(|| {
            BackendError::InvalidRequest(String::from(
                "no draft journal directory is available (check $TMAIL_DATA_DIR / $HOME)",
            ))
        })
    }

    /// Backend wired from the loaded Tmail configuration.
    pub fn from_config(config: &Config) -> Self {
        Self::new(
            PROGRAM,
            config.path.clone(),
            config.account.clone(),
            config.aliases.clone(),
        )
        .with_account_identity(
            config.account_email.clone(),
            config.account_display_name.clone(),
        )
        .with_downloads_dir(config.downloads_dir.clone())
    }

    /// Set the configured account identity (`From` of drafts).
    pub fn with_account_identity(
        mut self,
        email: Option<String>,
        display_name: Option<String>,
    ) -> Self {
        self.account_email = email;
        self.account_display_name = display_name;
        self
    }

    /// Set the configured downloads directory (plan §17).
    pub fn with_downloads_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.downloads_dir = dir;
        self
    }

    /// The 1-based page number and the page-aligned offset for a requested
    /// offset/limit (ADR 0001 finding 8). Himalaya pages are 1-based and
    /// Tmail requests page-aligned offsets; aligning here keeps an
    /// off-grid offset from silently reading a different page than
    /// requested.
    fn page_alignment(offset: usize, limit: usize) -> Result<(usize, usize), BackendError> {
        if limit == 0 {
            return Err(BackendError::InvalidRequest(String::from(
                "page limit must be non-zero",
            )));
        }
        let aligned = offset - offset % limit;
        let page_number = aligned / limit + 1;
        Ok((aligned, page_number))
    }

    /// The shared envelope page fetch: `envelope list` when `query` is
    /// `None`, `envelope search` when it carries the DSL (the output shape
    /// matches on himalaya 2.1.0, so the same envelope mapping applies; the
    /// search backend provides no total, so the page degrades to
    /// next-availability — plan §16).
    async fn fetch_page(
        &self,
        ctx: &RequestContext,
        mailbox_id: MailboxId,
        query: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> BackendResult<Page<MessageSummary>> {
        let (aligned, page_number) = Self::page_alignment(offset, limit)?;
        let argv = match query {
            Some(query) => command::envelope_search_argv(
                self.config_path.as_deref(),
                self.account.as_deref(),
                &mailbox_id.0,
                query,
                page_number,
                limit,
            ),
            None => command::envelope_list_argv(
                self.config_path.as_deref(),
                self.account.as_deref(),
                &mailbox_id.0,
                page_number,
                limit,
            ),
        };
        let output = process::run(&self.program, &argv, &ctx.cancellation).await?;
        let dto: dto::EnvelopesDto = process::decode_on_pool(output).await?;
        Ok(map::envelopes(dto, mailbox_id, aligned, limit))
    }

    /// The shared CLI identity (program, config, account) for callers that
    /// take a [`Cli`]: flags, truncation, and detached cleanup helpers.
    fn cli(&self) -> Cli {
        Cli {
            program: self.program.clone(),
            config: self.config_path.clone(),
            account: self.account.clone(),
        }
    }
}

#[async_trait]
impl MailBackend for HimalayaCliBackend {
    async fn list_mailboxes(&self, ctx: RequestContext) -> BackendResult<Vec<Mailbox>> {
        tracing::debug!(operation = %ctx.operation, "list_mailboxes");
        let argv =
            command::mailbox_list_argv(self.config_path.as_deref(), self.account.as_deref(), true);
        let output = process::run(&self.program, &argv, &ctx.cancellation).await?;
        let dto: dto::MailboxesDto = process::decode_on_pool(output).await?;
        let mailboxes = map::mailboxes(dto, &self.aliases);
        // Remember the listing so archive/trash resolution works even when
        // the semantic operation runs long after the sidebar loaded.
        if let Ok(mut cache) = self.mailboxes.write() {
            *cache = Some(mailboxes.clone());
        }
        Ok(mailboxes)
    }

    async fn list_messages(
        &self,
        ctx: RequestContext,
        page: PageRequest,
    ) -> BackendResult<Page<MessageSummary>> {
        tracing::debug!(operation = %ctx.operation, "list_messages");
        self.fetch_page(&ctx, page.mailbox_id, None, page.offset, page.limit)
            .await
    }

    async fn get_message(
        &self,
        ctx: RequestContext,
        locator: MessageLocator,
    ) -> BackendResult<Message> {
        tracing::debug!(operation = %ctx.operation, mailbox = %locator.mailbox.0, "get_message");
        let argv = command::message_read_argv(
            self.config_path.as_deref(),
            self.account.as_deref(),
            &locator.mailbox.0,
            &locator.id.0,
        );
        let output = process::run(&self.program, &argv, &ctx.cancellation).await?;
        let dto: dto::MessageReadDto = process::decode_on_pool(output).await?;
        Ok(map::message(dto, locator))
    }

    /// `envelope search` with the query passed through unchanged (Phase 9).
    /// The output shape matches `envelope list` (verified on himalaya
    /// 2.1.0), so the same envelope mapping applies; the backend provides
    /// no total, so the page degrades to next-availability (plan §16).
    async fn search_messages(
        &self,
        ctx: RequestContext,
        request: SearchRequest,
    ) -> BackendResult<Page<MessageSummary>> {
        tracing::debug!(
            operation = %ctx.operation,
            mailbox = %request.mailbox_id.0,
            "search_messages"
        );
        let SearchRequest {
            mailbox_id,
            query,
            offset,
            limit,
        } = request;
        self.fetch_page(&ctx, mailbox_id, Some(&query), offset, limit)
            .await
    }

    async fn set_read(
        &self,
        ctx: RequestContext,
        locator: MessageLocator,
        read: bool,
    ) -> BackendResult<()> {
        tracing::debug!(operation = %ctx.operation, read, "set_read");
        self.run_flag(&ctx, &locator, "seen", read).await
    }

    /// One `flag {add,remove}` process for the whole selection (ticket
    /// aavy): himalaya carries every id as one argv run, so the batch
    /// costs one IMAP session instead of one per message — N concurrent
    /// logins is what tripped the server's throttling in the reported
    /// failure. Locators are grouped by mailbox (a bulk selection is
    /// single-mailbox today; grouping keeps that assumption local).
    async fn set_read_bulk(
        &self,
        ctx: RequestContext,
        locators: Vec<MessageLocator>,
        read: bool,
    ) -> BackendResult<()> {
        tracing::debug!(operation = %ctx.operation, read, count = locators.len(), "set_read_bulk");
        let groups = grouped_mailbox_ids(&locators);
        for (mailbox, ids) in groups {
            let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            let argv = command::flag_argv(
                self.config_path.as_deref(),
                self.account.as_deref(),
                read,
                &mailbox,
                "seen",
                &id_refs,
            );
            let output = process::run(&self.program, &argv, &ctx.cancellation).await?;
            process::decode_on_pool::<serde_json::Value>(output).await?;
        }
        Ok(())
    }

    async fn set_starred(
        &self,
        ctx: RequestContext,
        locator: MessageLocator,
        starred: bool,
    ) -> BackendResult<()> {
        tracing::debug!(operation = %ctx.operation, starred, "set_starred");
        self.run_flag(&ctx, &locator, "flagged", starred).await
    }

    async fn archive(&self, ctx: RequestContext, locator: MessageLocator) -> BackendResult<()> {
        tracing::debug!(operation = %ctx.operation, mailbox = %locator.mailbox.0, "archive");
        let target = self
            .verified_role_target(MailboxRole::Archive, "archive")
            .map_err(BackendError::InvalidRequest)?;
        self.run_move(&ctx, &locator, &target).await
    }

    async fn trash(&self, ctx: RequestContext, locator: MessageLocator) -> BackendResult<()> {
        tracing::debug!(operation = %ctx.operation, mailbox = %locator.mailbox.0, "trash");
        // `message delete` is trash-first on the himalaya side (ADR 0001
        // finding 5); no Tmail-side target resolution needed.
        let argv = command::message_delete_argv(
            self.config_path.as_deref(),
            self.account.as_deref(),
            &locator.mailbox.0,
            [locator.id.0.as_str()].as_slice(),
        );
        let output = process::run(&self.program, &argv, &ctx.cancellation).await?;
        process::decode_on_pool::<serde_json::Value>(output).await?;
        Ok(())
    }

    /// Batched archive (ticket j9bq): the grouping mirrors
    /// [`Self::set_read_bulk`] (locators grouped by mailbox; a bulk
    /// selection is single-mailbox today) and one `message move`
    /// invocation per group carries every id — one IMAP session, no
    /// per-message login fanout.
    async fn archive_bulk(
        &self,
        ctx: RequestContext,
        locators: Vec<MessageLocator>,
    ) -> BackendResult<()> {
        tracing::debug!(operation = %ctx.operation, count = locators.len(), "archive_bulk");
        let target = self
            .verified_role_target(MailboxRole::Archive, "archive")
            .map_err(BackendError::InvalidRequest)?;
        for (mailbox, ids) in grouped_mailbox_ids(&locators) {
            let argv = command::message_move_argv(
                self.config_path.as_deref(),
                self.account.as_deref(),
                &mailbox,
                &target,
                &ids.iter().map(String::as_str).collect::<Vec<_>>(),
            );
            let output = process::run(&self.program, &argv, &ctx.cancellation).await?;
            process::decode_on_pool::<serde_json::Value>(output).await?;
        }
        Ok(())
    }

    /// Batched trash (ticket j9bq): one trash-first `message delete`
    /// invocation per mailbox group.
    async fn trash_bulk(
        &self,
        ctx: RequestContext,
        locators: Vec<MessageLocator>,
    ) -> BackendResult<()> {
        tracing::debug!(operation = %ctx.operation, count = locators.len(), "trash_bulk");
        for (mailbox, ids) in grouped_mailbox_ids(&locators) {
            let argv = command::message_delete_argv(
                self.config_path.as_deref(),
                self.account.as_deref(),
                &mailbox,
                &ids.iter().map(String::as_str).collect::<Vec<_>>(),
            );
            let output = process::run(&self.program, &argv, &ctx.cancellation).await?;
            process::decode_on_pool::<serde_json::Value>(output).await?;
        }
        Ok(())
    }

    async fn save_draft(
        &self,
        ctx: RequestContext,
        draft: DraftSnapshot,
    ) -> BackendResult<MessageId> {
        tracing::debug!(
            operation = %ctx.operation,
            local_id = %draft.local_id.0,
            revision = draft.revision,
            "save_draft"
        );
        // 1. Crash-safe local record BEFORE any remote call (ADR 0002
        //    §D.1): a crash after this point can only leave duplicates,
        //    never lost text. Journal I/O runs on the blocking pool — the
        //    runtime is single-threaded and must never wait on a disk.
        //    No writable journal: refuse with the clear request error
        //    before touching the server.
        let journal = self.journal_required()?.clone();
        let snapshot = draft.clone();
        blocking(move || journal.record(&snapshot).map_err(BackendError::Io)).await?;

        // 2. Serialize the draft (library-built RFC 5322, plan §14).
        let message = self.serialize_draft(&draft)?;

        // 3. Add the new revision to the Drafts mailbox with the draft
        //    flag; the confirmed id from stdout is the replacement's
        //    identity (ADR 0002 §D.3).
        let drafts = self
            .verified_role_target(MailboxRole::Drafts, "drafts")
            .map_err(BackendError::InvalidRequest)?;
        let argv = command::message_add_argv(
            self.config_path.as_deref(),
            self.account.as_deref(),
            &drafts,
            "draft",
        );
        let output = process::run_with_stdin(
            &self.program,
            &argv,
            Some(message.as_slice()),
            &ctx.cancellation,
        )
        .await?;
        let added: dto::MessageAddDto = process::decode_on_pool(output).await?;
        let new_id = MessageId(added.id);

        // 4. Only after the new copy is confirmed: best-effort deletion of
        //    every previous remote copy (ADR 0002 §D.3/§D.4 — failure here
        //    leaves a duplicate, never data loss). The Message-ID sweep
        //    covers both the known previous id and any stray copies from a
        //    crash mid-replacement, so one pass handles all cleanup.
        self.delete_stray_draft_copies(&ctx, &drafts, Some(&new_id), &draft.message_id);

        // 5. Confirm the revision in the journal (newest pushed), on the
        //    blocking pool like every journal access. Best-effort: the
        //    remote save already succeeded, so failing the operation here
        //    would push the user into a retry that creates a duplicate
        //    remote copy — the dirty-revision gap is self-healed by the
        //    restore path instead.
        let journal = self.journal.clone();
        let local_id = draft.local_id.0.clone();
        let revision = draft.revision;
        let confirmed = blocking(move || {
            journal.map_or(
                Err(BackendError::InvalidRequest(String::from(
                    "no draft journal",
                ))),
                |journal| {
                    journal
                        .mark_remote(&local_id, revision)
                        .map_err(BackendError::Io)
                },
            )
        })
        .await;
        if let Err(err) = confirmed {
            tracing::warn!(
                local_id = %draft.local_id.0,
                revision = draft.revision,
                %err,
                "remote draft saved but the journal confirmation failed; restore will re-run it dirty"
            );
        }

        Ok(new_id)
    }

    async fn load_drafts(&self, _ctx: RequestContext) -> BackendResult<Vec<RestoredDraft>> {
        tracing::debug!("load_drafts");
        // Purely local (ADR 0002 §D.1): the journal is the source of truth
        // for restore, independent of account reachability. The journal
        // walk runs on the blocking pool (single-threaded runtime). No
        // journal directory: no draft was ever journaled — nothing to
        // restore.
        let Some(journal) = self.journal.clone() else {
            return Ok(Vec::new());
        };
        let entries = blocking(move || journal.load_all().map_err(BackendError::Io)).await?;
        Ok(entries
            .into_iter()
            .map(|entry| RestoredDraft {
                draft: entry.draft,
                saved_revision: entry.saved_revision,
            })
            .collect())
    }

    async fn delete_draft(&self, ctx: RequestContext, draft: DraftSnapshot) -> BackendResult<()> {
        tracing::debug!(local_id = %draft.local_id.0, "delete_draft");
        // The user confirmed the discard: journal first (worst case after a
        // crash is a lingering remote copy, never a resurrected draft).
        // Journal I/O runs on the blocking pool (single-threaded runtime).
        // No journal: nothing was recorded, so the remote cleanup below
        // runs regardless (can only mean an earlier save proceeded
        // without a journal).
        let journal = self.journal.clone();
        let local_id = draft.local_id.0.clone();
        blocking(move || match journal {
            Some(journal) => journal.remove(&local_id).map_err(BackendError::Io),
            None => Ok(()),
        })
        .await?;
        if let Some(drafts) = self.mailbox_for_role(MailboxRole::Drafts) {
            // Unlike the save-path cleanup, the sweep runs *awaited* here:
            // the operation may only report Done once the IMAP deletions
            // landed, or the follow-up refresh races the cleanup and the
            // deleted draft stays visible.
            let cli = self.cli();
            let trash = self.mailbox_for_role(MailboxRole::Trash);
            if draft.message_id.is_none() {
                // Without the stable Message-ID the sweep cannot match
                // envelopes, so delete the known copy directly.
                if let Some(old) = draft.remote_id.as_ref() {
                    run_two_phase_delete(
                        &cli,
                        &drafts,
                        trash.as_deref(),
                        &old.0,
                        None,
                        &ctx.cancellation,
                    )
                    .await;
                }
                return Ok(());
            }
            // The Message-ID sweep (keep=None) deletes the known copy and
            // any strays alike — running an explicit two-phase delete of
            // `old` on top would just repeat the same work.
            run_stray_draft_sweep(
                cli,
                drafts,
                trash,
                None,
                draft.message_id.clone(),
                ctx.clone(),
            )
            .await;
        }
        Ok(())
    }

    async fn send_message(
        &self,
        ctx: RequestContext,
        message: OutboundMessage,
    ) -> BackendResult<SendOutcome> {
        tracing::debug!(
            operation = %ctx.operation,
            recipients = message.recipient_count(),
            "send_message"
        );
        // 1. Serialize through the library (Phase 7.1) — refusals for a
        //    missing identity or empty recipient lists happen here, before
        //    any child process exists.
        let bytes = self.serialize_outbound(&message).await?;
        // 2. Deliver through the stdin contract (ADR 0001 decision 2,
        //    plan §11: "Pipe serialized mail to stdin when required").
        let argv = command::message_send_argv(self.config_path.as_deref(), self.account.as_deref());
        let output =
            process::run_with_stdin(&self.program, &argv, Some(&bytes), &ctx.cancellation).await?;
        // 3. Classify the outcome (plan §12, ADR 0001 finding 12): the
        //    exit status alone cannot separate "failed before delivery"
        //    from "may already be delivered".
        Ok(classify_send(output))
    }

    async fn read_attachment(
        &self,
        ctx: RequestContext,
        path: PathBuf,
    ) -> BackendResult<DraftAttachment> {
        tracing::debug!(operation = %ctx.operation, path = %path.display(), "read_attachment");
        // The source check stats and probe-opens the file: blocking pool.
        blocking(move || validate_attachment_source(&path)).await
    }

    async fn save_attachment(
        &self,
        ctx: RequestContext,
        request: AttachmentRequest,
    ) -> BackendResult<PathBuf> {
        tracing::debug!(
            operation = %ctx.operation,
            part = request.part_id,
            "save_attachment"
        );
        // 1. Destination directory: explicit request, then config, then
        //    the platform default. Created when missing — on the blocking
        //    pool (single-threaded runtime).
        let dir = resolve_downloads_dir(request.dir.as_deref(), self.downloads_dir.as_deref())?;
        let to_create = dir.clone();
        blocking(move || {
            std::fs::create_dir_all(&to_create).map_err(|err| {
                BackendError::File(format!(
                    "download directory `{}` could not be created: {err}",
                    to_create.display()
                ))
            })
        })
        .await?;

        // 2. Download the part into a Tmail-owned private tempdir — never
        //    straight into the destination, so nothing there can be
        //    touched until the collision-checked write is ready. The dir
        //    travels as one argv entry: no shell, whatever the path. The
        //    tempdir creation hops to the blocking pool (plan §3).
        let temp = blocking(move || {
            tempfile::tempdir()
                .map_err(|err| BackendError::File(format!("temporary download dir: {err}")))
        })
        .await?;
        let argv = command::attachment_download_argv(
            self.config_path.as_deref(),
            self.account.as_deref(),
            &request.locator.mailbox.0,
            &request.locator.id.0,
            request.part_id,
            temp.path(),
        );
        let output = process::run(&self.program, &argv, &ctx.cancellation).await?;
        let dto: dto::AttachmentsDto = process::decode_on_pool(output).await?;
        let row = find_row(&dto, request.part_id)?;
        // The backend is untrusted (ticket m89w): the row must name a file
        // canonically inside the tempdir himalaya was pointed at — an
        // absolute path, a `..` traversal, or a symlink out of the root
        // would otherwise point Tmail at an arbitrary locally readable
        // file that the saver then copies into the user's downloads
        // directory.
        let source = row
            .path
            .as_deref()
            .map(|raw| confined_source_path(raw, temp.path()))
            .transpose()?
            .ok_or_else(|| {
                BackendError::InvalidOutput(format!(
                    "attachment download row for part {} names no output path",
                    request.part_id
                ))
            })?;
        // The download can be tens of megabytes: the read hops to the
        // blocking pool so the single-threaded runtime never stalls.
        let bytes = blocking(move || {
            std::fs::read(&source).map_err(|err| {
                BackendError::File(format!(
                    "`{}` could not be read after download: {err}",
                    source.display()
                ))
            })
        })
        .await?;

        // 3. Destination name: the caller's display filename (reduced to a
        //    single component — traversal is impossible), else the row's,
        //    else a part-id fallback.
        let name = destination_component(
            request.filename.as_deref().or(row.filename.as_deref()),
            request.part_id,
        );
        // The collision-checked write lands on the blocking pool with the
        // payload moved in (single-threaded runtime).
        let byte_count = bytes.len();
        let final_path = blocking(move || write_collision_safe(&dir, &name, &bytes)).await?;
        tracing::info!(
            part = request.part_id,
            bytes = byte_count,
            saved = %final_path.display(),
            "attachment saved"
        );
        Ok(final_path)
    }
}

impl HimalayaCliBackend {
    /// `flag add`/`flag remove` for one flag; output is a flag echo (ADR
    /// 0001 finding 6) that is validated but not interpreted.
    async fn run_flag(
        &self,
        ctx: &RequestContext,
        locator: &MessageLocator,
        flag: &str,
        add: bool,
    ) -> BackendResult<()> {
        let argv = command::flag_argv(
            self.config_path.as_deref(),
            self.account.as_deref(),
            add,
            &locator.mailbox.0,
            flag,
            &[&locator.id.0],
        );
        let output = process::run(&self.program, &argv, &ctx.cancellation).await?;
        process::decode_on_pool::<serde_json::Value>(output).await?;
        Ok(())
    }

    /// `message move --from <source> --to <resolved target> <id>`.
    async fn run_move(
        &self,
        ctx: &RequestContext,
        locator: &MessageLocator,
        target: &str,
    ) -> BackendResult<()> {
        let argv = command::message_move_argv(
            self.config_path.as_deref(),
            self.account.as_deref(),
            &locator.mailbox.0,
            target,
            [locator.id.0.as_str()].as_slice(),
        );
        let output = process::run(&self.program, &argv, &ctx.cancellation).await?;
        process::decode_on_pool::<serde_json::Value>(output).await?;
        Ok(())
    }

    /// Resolve the mailbox id for a semantic role: the cached listing's
    /// role (authoritative, alias-derived) wins, then the config alias
    /// value. No name heuristics: moving mail to a lookalike folder is
    /// worse than refusing (ADR 0001).
    fn mailbox_for_role(&self, role: MailboxRole) -> Option<String> {
        if let Ok(cache) = self.mailboxes.read()
            && let Some(mailboxes) = cache.as_ref()
            && let Some(mailbox) = mailboxes.iter().find(|m| m.role == Some(role))
        {
            return Some(mailbox.id.0.clone());
        }
        let alias_key = map::alias_key_for_role(role);
        self.aliases.get(alias_key).cloned()
    }

    /// Resolve the target mailbox for a semantic role against what the
    /// account actually exposes. Resolution order: the cached listing's
    /// alias-derived role, then the config alias value — but an alias
    /// target the listing does not show is refused here, before any child
    /// process, so a stale or wrong mapping surfaces as an actionable
    /// message instead of a raw IMAP failure (real-config finding: a Gmail
    /// account whose `[Gmail]/All Mail` is not IMAP-listed). With no cached
    /// listing yet, the alias value passes through as before.
    ///
    /// [`Self::mailbox_for_role`] remains for best-effort paths (draft
    /// cleanup) that must never hard-fail on resolution.
    fn verified_role_target(&self, role: MailboxRole, alias_key: &str) -> Result<String, String> {
        let listing = self.mailboxes.read().ok().and_then(|guard| guard.clone());
        if let Some(mailboxes) = listing {
            if let Some(mailbox) = mailboxes.iter().find(|m| m.role == Some(role)) {
                return Ok(mailbox.id.0.clone());
            }
            if let Some(alias) = self.aliases.get(alias_key) {
                if mailboxes
                    .iter()
                    .any(|m| &m.id.0 == alias || &m.name == alias)
                {
                    return Ok(alias.clone());
                }
                return Err(format!(
                    "config maps {alias_key} to `{alias}`, but the account exposes \
                     no such mailbox; set [accounts.<account>.mailbox.alias] \
                     {alias_key} to an existing mailbox, or enable the folder \
                     for IMAP access in the provider's settings (Gmail: \
                     Settings → Labels → Show in IMAP)"
                ));
            }
            return Err(format!(
                "no {alias_key} mailbox is known; set \
                 [accounts.<account>.mailbox.alias] {alias_key}"
            ));
        }
        self.aliases.get(alias_key).cloned().ok_or_else(|| {
            format!(
                "no {alias_key} mailbox is known; set \
                 [accounts.<account>.mailbox.alias] {alias_key}"
            )
        })
    }

    /// Remove every remote copy of a draft (matched by its stable
    /// `Message-ID`) from the Drafts mailbox except `keep` — the
    /// reconciliation sweep that also cleans up the known previous copy
    /// on save (ADR 0002 §D.4/§D.5). Spawned detached so cleanup never
    /// delays the save result; every failure is logged and swallowed — a
    /// leftover copy is always preferable to risking data loss.
    fn delete_stray_draft_copies(
        &self,
        ctx: &RequestContext,
        drafts_mailbox: &str,
        keep: Option<&MessageId>,
        message_id: &Option<String>,
    ) {
        let Some(message_id) = message_id else {
            return;
        };
        // One sweep at a time (ticket 8s0g): each sweep is detached and
        // effectively uncancellable, so rapid saves must not pile them
        // up unbounded. Skipping is safe — every copy carries the same
        // stable Message-ID, so the next save's sweep removes the
        // leftovers this one missed.
        let Ok(permit) = self.sweeps.clone().try_acquire_owned() else {
            tracing::debug!("a stray draft sweep is already running; skipping this one");
            return;
        };
        let cli = self.cli();
        let drafts_mailbox = drafts_mailbox.to_string();
        let trash = self.mailbox_for_role(MailboxRole::Trash);
        let keep = keep.map(|id| id.0.clone());
        let message_id = Some(message_id.clone());
        let ctx = ctx.clone();
        tokio::spawn(async move {
            run_stray_draft_sweep(cli, drafts_mailbox, trash, keep, message_id, ctx).await;
            // Hold the single-sweep permit until the sweep ends.
            drop(permit);
        });
    }

    /// Serialize one draft revision as a single-part `text/plain` RFC 5322
    /// message via the mail-builder library (plan §14: never hand-concatenate
    /// MIME). The stable `Message-ID` (ADR 0002 §D.6) and Tmail-owned
    /// `X-Tmail-Draft-Id` header make replacement and reconciliation
    /// possible; only valid parsed addresses are written (the composer
    /// flags invalid ones and send refuses them before starting, Phase 7).
    fn serialize_draft(&self, draft: &DraftSnapshot) -> BackendResult<Vec<u8>> {
        // One check in one place: a draft without its stable Message-ID
        // cannot be written (it is what replacement and reconciliation
        // key on, ADR 0002 §D.6).
        let Some(message_id) = draft.message_id.as_ref() else {
            return Err(BackendError::InvalidRequest(String::from(
                "draft is missing a stable Message-ID; it must be minted \
                 before the first save",
            )));
        };

        let bare_message_id = bare_message_id(message_id);
        let mut builder = MessageBuilder::new()
            .date(chrono::Utc::now().timestamp())
            .header(
                "X-Tmail-Draft-Id",
                mail_builder::headers::raw::Raw::from(draft.local_id.0.clone()),
            )
            .message_id(bare_message_id);
        if let Some(email) = &self.account_email {
            builder = builder.from(MailAddress::new_address(
                self.account_display_name.clone(),
                email.clone(),
            ));
        }
        // mail_builder takes ownership; build owned lists per field. Draft
        // fields may hold partially typed addresses (the composer flags
        // them live); invalid entries are dropped, sends refuse them.
        builder = match header_addresses(&draft.to) {
            Some(addr) => builder.to(addr),
            None => builder,
        };
        builder = match header_addresses(&draft.cc) {
            Some(addr) => builder.cc(addr),
            None => builder,
        };
        builder = match header_addresses(&draft.bcc) {
            Some(addr) => builder.bcc(addr),
            None => builder,
        };
        if !draft.subject.is_empty() {
            builder = builder.subject(draft.subject.as_str());
        }
        builder
            .text_body(draft.body.as_str())
            .write_to_vec()
            .map_err(|err| {
                BackendError::InvalidRequest(format!("draft serialization failed: {err}"))
            })
    }

    /// Serialize one outgoing message as an RFC 5322 message via the
    /// mail-builder library (plan §14, Phase 7.1: never hand-concatenate
    /// MIME). `From` comes from the configured account identity; reply
    /// headers ride through unchanged (plan §14); the `Message-ID` reuses
    /// the draft's stable identity when the send derives from a draft
    /// (ADR 0002 §D.6) and is minted otherwise. Attached files (plan §15,
    /// Phase 8.2) are read here and become MIME attachment parts in
    /// insertion order — a missing or unreadable file is a detailed,
    /// retryable refusal, never a partial send.
    ///
    /// Recipients are refused here as defense in depth — [`crate::domain::OutboundMessage`]
    /// is constructed only from validated fields, so this arm is
    /// unreachable through the reducer.
    ///
    /// Attachment payloads are file reads (potentially tens of megabytes):
    /// they hop to the blocking pool before the builder runs, so the
    /// single-threaded runtime never stalls on a disk.
    async fn serialize_outbound(&self, message: &OutboundMessage) -> BackendResult<Vec<u8>> {
        if message.recipient_count() == 0 {
            return Err(BackendError::InvalidRequest(String::from(
                "outbound message has no recipients",
            )));
        }
        let Some(email) = &self.account_email else {
            return Err(BackendError::InvalidRequest(String::from(
                "no account email is configured; set [accounts.<account>].email \
                 so outgoing mail has a From address",
            )));
        };
        let payloads = read_attachment_payloads(&message.attachments).await?;
        let message_id = match &message.message_id {
            Some(id) => bare_message_id(id),
            None => format!(
                "{}.send@tmail.local",
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
            ),
        };
        let mut builder = MessageBuilder::new()
            .date(chrono::Utc::now().timestamp())
            .message_id(message_id)
            .from(MailAddress::new_address(
                self.account_display_name.clone(),
                email.clone(),
            ));
        for (field, addresses) in [
            ("To", &message.to),
            ("Cc", &message.cc),
            ("Bcc", &message.bcc),
        ] {
            if addresses.is_empty() {
                continue;
            }
            let list = MailAddress::List(
                addresses
                    .iter()
                    .map(|a| MailAddress::new_address(a.name.clone(), a.email.clone()))
                    .collect(),
            );
            builder = match field {
                "To" => builder.to(list),
                "Cc" => builder.cc(list),
                _ => builder.bcc(list),
            };
        }
        if !message.content.subject.is_empty() {
            builder = builder.subject(message.content.subject.as_str());
        }
        if let Some(in_reply_to) = &message.content.in_reply_to {
            builder = builder.in_reply_to(bare_message_id(in_reply_to));
        }
        if let Some(references) = &message.content.references {
            let ids: Vec<String> = references.split_whitespace().map(bare_message_id).collect();
            builder = builder.references(mail_builder::headers::message_id::MessageId::new_list(
                ids.into_iter(),
            ));
        }
        for (attachment, bytes) in message.attachments.iter().zip(payloads) {
            builder = builder.attachment(
                crate::domain::paths::media_type_for(&attachment.name),
                attachment.name.as_str(),
                bytes,
            );
        }
        builder
            .text_body(message.content.body.as_str())
            .write_to_vec()
            .map_err(|err| {
                BackendError::InvalidRequest(format!("outbound serialization failed: {err}"))
            })
    }
}

/// Run one blocking file operation on the blocking pool: the main runtime
/// is single-threaded (plan §3), so filesystem work on it stalls the UI
/// loop — every file access below the backend boundary hops threads. A
/// panicked task surfaces as an I/O error, like any other failure.
async fn blocking<T, F>(task: F) -> BackendResult<T>
where
    F: FnOnce() -> BackendResult<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|err| BackendError::Io(std::io::Error::other(err.to_string())))?
}

/// Parse one composer address field into library addresses (valid entries
/// only). Draft fields may hold partially typed input — the composer flags
/// invalid entries live and send refuses them before starting (Phase 7) —
/// so drafts drop the invalid ones instead of failing the autosave.
fn header_addresses(field: &str) -> Option<MailAddress<'static>> {
    let list: Vec<MailAddress<'static>> = crate::domain::address::parse_address_list(field)
        .into_iter()
        .filter_map(Result::ok)
        .map(|a| MailAddress::new_address(a.name, a.email))
        .collect();
    (!list.is_empty()).then_some(MailAddress::List(list))
}

/// Bare id form for mail-builder, which adds the angle brackets itself
/// (shared normalizer: `domain::message::bare_message_id`).
fn bare_message_id(message_id: &str) -> String {
    crate::domain::message::bare_message_id(message_id).to_string()
}

/// Read every outbound attachment's bytes on the blocking pool, in
/// insertion order (deterministic wire order). A missing or unreadable
/// file is the same detailed, retryable refusal the inline read produced.
async fn read_attachment_payloads(
    attachments: &[crate::domain::OutboundAttachment],
) -> BackendResult<Vec<Vec<u8>>> {
    let specs: Vec<PathBuf> = attachments
        .iter()
        .map(|attachment| attachment.path.clone())
        .collect();
    blocking(move || {
        let mut payloads = Vec::with_capacity(specs.len());
        for path in &specs {
            let bytes = std::fs::read(path).map_err(|err| {
                BackendError::File(format!(
                    "`{}` could not be read for sending: {err}",
                    path.display()
                ))
            })?;
            payloads.push(bytes);
        }
        Ok(payloads)
    })
    .await
}

/// Resolve the destination directory for a save (plan §15): the request's
/// explicit directory wins, then the configured downloads dir, then the
/// platform default. `~` is expanded here — inside Tmail, never a shell.
fn resolve_downloads_dir(
    request_dir: Option<&Path>,
    configured: Option<&Path>,
) -> BackendResult<PathBuf> {
    resolve_downloads_dir_with(
        request_dir,
        configured,
        crate::domain::paths::home_dir().as_deref(),
    )
}

/// [`resolve_downloads_dir`] with an injected home directory for tests.
fn resolve_downloads_dir_with(
    request_dir: Option<&Path>,
    configured: Option<&Path>,
    home: Option<&Path>,
) -> BackendResult<PathBuf> {
    let raw = request_dir
        .or(configured)
        .map(|dir| crate::domain::paths::expand_tilde(dir, home));
    match raw {
        Some(dir) => Ok(dir),
        None => match home {
            Some(home) => Ok(home.join("Downloads")),
            None => Err(BackendError::File(String::from(
                "no downloads directory is configured and $HOME is not set",
            ))),
        },
    }
}

/// The row of the download output matching the requested part id.
fn find_row(dto: &dto::AttachmentsDto, part_id: usize) -> BackendResult<dto::AttachmentRowDto> {
    let wanted = part_id.to_string();
    dto.attachments
        .iter()
        .find(|row| row.id == wanted)
        .cloned()
        .ok_or_else(|| {
            BackendError::InvalidOutput(format!(
                "attachment download returned no row for part {part_id}"
            ))
        })
}

/// Reduce a display filename to a single path component (plan §15: prevent
/// path traversal). Only the final component survives — `../../.zshenv`
/// becomes `.zshenv` — and empty, `..`, or unusable results fall back to
/// `attachment-<part>` so the saver never fabricates a location.
fn destination_component(filename: Option<&str>, part_id: usize) -> String {
    let usable = filename
        .and_then(|name| {
            Path::new(name)
                .file_name()
                .and_then(|component| component.to_str())
        })
        .filter(|component| !component.is_empty() && *component != "." && *component != "..");
    usable
        .map(str::to_owned)
        .unwrap_or_else(|| format!("attachment-{part_id}"))
}

/// Write `bytes` to `dir/name` without ever overwriting (plan §15
/// acceptance): an atomic `create_new` write, walking `name (1).ext`,
/// `name (2).ext`, … when the name is taken. Deterministic and
/// crash-safe — a partially written file can never masquerade as the
/// previous one because a collision-rename never reuses an existing path.
fn write_collision_safe(dir: &Path, name: &str, bytes: &[u8]) -> BackendResult<PathBuf> {
    use std::io::Write;
    let write_new = |path: &Path| -> Result<PathBuf, (PathBuf, std::io::Error)> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|err| (path.to_path_buf(), err))?;
        file.write_all(bytes)
            .map_err(|err| (path.to_path_buf(), err))?;
        Ok(path.to_path_buf())
    };
    let path = dir.join(name);
    // Fast path: the plain name is free. A race (taken between the check
    // and the open) surfaces as AlreadyExists and falls through to the
    // numbered walk.
    if !path.exists()
        && let Ok(saved) = write_new(&path)
    {
        return Ok(saved);
    }
    let (stem, ext) = split_stem_ext(name);
    for index in 1..=999u32 {
        let candidate = dir.join(format!("{stem} ({index}){ext}"));
        match write_new(&candidate) {
            Ok(saved) => return Ok(saved),
            Err((_, err)) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err((failed, err)) => {
                return Err(BackendError::File(format!(
                    "`{}` could not be written: {err}",
                    failed.display()
                )));
            }
        }
    }
    Err(BackendError::File(format!(
        "`{}` is taken and no free numbered name was found (tried 999)",
        dir.join(name).display()
    )))
}

/// Split a filename into stem and dot-prefixed extension for the numbered
/// collision walk (`report.pdf` → `report`, `.pdf`).
fn split_stem_ext(name: &str) -> (String, String) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_owned(), format!(".{ext}")),
        _ => (name.to_owned(), String::new()),
    }
}

/// Constrain the backend-reported attachment path to the Tmail-owned
/// tempdir the download ran in (ticket m89w). The backend is untrusted:
/// any path — absolute, `..` traversal, or a symlink pointing out of the
/// root — would otherwise let a compromised or buggy backend point Tmail
/// at an arbitrary locally readable file that the saver then copies into
/// the user's downloads directory. Real himalaya echoes the absolute path
/// of the file it wrote *into* the tempdir, so absoluteness alone is not
/// the boundary: the row must canonically resolve — symlinks followed —
/// to a regular file inside the canonical root. The resolved path is
/// returned, so the subsequent read travels the same boundary-checked
/// route the check validated.
fn confined_source_path(raw: &str, root: &Path) -> BackendResult<PathBuf> {
    confined_source_path_inner(raw, root, |path| std::fs::canonicalize(path))
}

/// [`confined_source_path`] with an injected canonicalize, so the
/// containment logic is unit-testable (the real one is exercised end to
/// end by the same tests on Unix).
fn confined_source_path_inner(
    raw: &str,
    root: &Path,
    canonicalize: impl Fn(&Path) -> std::io::Result<PathBuf>,
) -> BackendResult<PathBuf> {
    // Relative rows resolve against the tempdir himalaya was pointed at;
    // absolute rows are taken as reported — the containment check below
    // is the security boundary, not the path's shape.
    let reported = PathBuf::from(raw);
    let joined = if reported.is_absolute() {
        reported
    } else {
        root.join(reported)
    };
    let resolved = canonicalize(&joined).map_err(|err| {
        BackendError::InvalidOutput(format!(
            "attachment download row names `{raw}`, which does not resolve inside the temporary download directory: {err}"
        ))
    })?;
    // The root itself is canonicalized before the containment compare: on
    // macOS the tempdir lives under a `/var → /private/var` symlink, so a
    // raw-vs-canonical prefix check would misfire.
    let root = canonicalize(root)
        .map_err(|err| BackendError::File(format!("temporary download dir: {err}")))?;
    if !resolved.starts_with(&root) {
        return Err(BackendError::InvalidOutput(format!(
            "attachment download row names `{raw}`, outside the temporary download directory"
        )));
    }
    if !resolved.is_file() {
        return Err(BackendError::InvalidOutput(format!(
            "attachment download row names `{raw}`, which is not a regular file"
        )));
    }
    Ok(resolved)
}

/// Validate one attachment source path (plan §15, Phase 8): expand `~` in
/// Tmail (never a shell), then require an existing, regular, readable file
/// within the acceptable size. Every refusal is detailed so the composer
/// dialog can show it next to the still-editable entry (Phase 8
/// acceptance: "missing/unreadable files produce retryable detailed
/// errors").
fn validate_attachment_source(raw: &Path) -> BackendResult<DraftAttachment> {
    validate_attachment_source_inner(
        raw,
        crate::domain::paths::home_dir().as_deref(),
        crate::domain::paths::MAX_DRAFT_ATTACHMENT_BYTES,
    )
}

/// [`validate_attachment_source`] with injected home directory and size
/// limit, so the checks are unit-testable without the environment.
fn validate_attachment_source_inner(
    raw: &Path,
    home: Option<&Path>,
    limit: u64,
) -> BackendResult<DraftAttachment> {
    let expanded = crate::domain::paths::expand_tilde(raw, home);
    let display = expanded.display().to_string();
    let meta = match std::fs::metadata(&expanded) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(BackendError::File(format!("`{display}` does not exist")));
        }
        Err(err) => {
            return Err(BackendError::File(format!(
                "`{display}` could not be read: {err}"
            )));
        }
    };
    if !meta.is_file() {
        return Err(BackendError::File(format!(
            "`{display}` is not a regular file"
        )));
    }
    let size = meta.len();
    if size > limit {
        return Err(BackendError::File(format!(
            "`{display}` is {} — the limit is {}",
            crate::ui::text::human_size(size),
            crate::ui::text::human_size(limit),
        )));
    }
    if let Err(err) = std::fs::File::open(&expanded) {
        return Err(BackendError::File(format!(
            "`{display}` is not readable: {err}"
        )));
    }
    let name = expanded
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| BackendError::File(format!("`{display}` has no usable file name")))?;
    Ok(DraftAttachment {
        path: expanded,
        name,
        size,
    })
}

/// Classify one finished `message send` run into a [`SendOutcome`] (plan
/// §12; characterization: `fixtures/himalaya/send-outcomes.md`).
///
/// - exit 0 → [`SendOutcome::Sent`], even if the success JSON is odd: the
///   message was almost certainly delivered, and claiming failure would
///   invite a duplicate send.
/// - exit != 0 with a diagnostic matching a *pre-DATA* phase marker
///   (connection/DNS/TLS/auth failures, unresolved `--save` targets) →
///   `FailedBeforeDelivery`: nothing was transmitted, retrying is safe.
/// - everything else → conservatively `Unknown`: killed or unparseable
///   runs, and transport errors during/after DATA (verified ambiguous —
///   the sink received the payload while himalaya reported an error).
///
/// Cancellation never reaches here: the process layer reports it as
/// [`BackendError::Cancelled`] before a classification exists.
fn classify_send(output: process::ChildOutput) -> SendOutcome {
    if output.code == Some(0) {
        return SendOutcome::Sent;
    }
    let code = output.code;
    let detail = process::error_detail(&output);
    if pre_delivery_failure(&detail) {
        SendOutcome::FailedBeforeDelivery { code, detail }
    } else {
        SendOutcome::Unknown { code, detail }
    }
}

/// Whether a send diagnostic clearly marks a failure *before* the SMTP
/// DATA phase (nothing transmitted). Matched case-insensitively against
/// the Phase 0 probe vocabulary; anything not listed is conservatively
/// treated as unknown delivery state.
///
/// Ticket 9gx6: the old vocabulary matched the broad substring
/// "connect", so a DATA-phase error phrased "connection reset by peer"
/// was misclassified as pre-delivery — inviting a duplicate send, the
/// exact failure mode [`SendOutcome`] exists to prevent. The markers are
/// now anchored: explicit DATA-phase markers and known-ambiguous
/// transport phrases are checked first and always route to the
/// conservative outcome; dial errors must match the himalaya shape
/// ("connect <addr>: …") or a narrow pre-DATA vocabulary.
fn pre_delivery_failure(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    // Explicit DATA-phase markers are never pre-delivery: himalaya tags
    // transport errors during/after DATA with these prefixes, and the
    // payload may already have been transmitted (probe-verified).
    const DATA_PHASE_MARKERS: [&str; 2] = ["smtp data", "smtp write"];
    if DATA_PHASE_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return false;
    }
    // Known-ambiguous transport phrases: these read like connection
    // failures but occur mid-session (the sink may already hold the
    // payload), so they must never count as safe-to-retry.
    const AMBIGUOUS_MARKERS: [&str; 5] = [
        "connection reset",
        "connection closed",
        "broken pipe",
        "unexpected eof",
        "timed out",
    ];
    if AMBIGUOUS_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return false;
    }
    // Dial errors carry the himalaya shape "connect <addr>: <reason>"
    // (probe: "connect 127.0.0.1:3425: connection refused"). Anchoring at
    // the start keeps mid-session "connection …" phrases out.
    if lower.starts_with("connect ") {
        return true;
    }
    const PRE_DATA_MARKERS: [&str; 12] = [
        "no route",
        "network is unreachable",
        "resolve", // DNS resolution failures
        "lookup",
        "nodename",
        "name or service",
        "tls",
        "ssl",
        "certificate",
        "handshake",
        "auth",            // SMTP authentication happens before DATA
        "not a directory", // an unusable `--save` target (probe: pre-delivery)
    ];
    PRE_DATA_MARKERS.iter().any(|marker| lower.contains(marker))
}

/// The awaited stray-sweep body (see the detached
/// `delete_stray_draft_copies` wrapper): list the Drafts mailbox, then
/// trash-first delete every envelope carrying the draft's `Message-ID`
/// except `keep`. Best-effort — any failure just ends the sweep.
async fn run_stray_draft_sweep(
    cli: Cli,
    drafts_mailbox: String,
    trash: Option<String>,
    keep: Option<String>,
    message_id: Option<String>,
    ctx: RequestContext,
) {
    let Some(message_id) = message_id else {
        return;
    };
    let bare = bare_message_id(&message_id);
    let ids =
        list_envelope_ids_with_message_id(&cli, &drafts_mailbox, &bare, &ctx.cancellation).await;
    for id in ids {
        if keep.as_deref() == Some(id.as_str()) {
            continue;
        }
        tracing::info!(id = %id, "deleting stray draft copy");
        run_two_phase_delete(
            &cli,
            &drafts_mailbox,
            trash.as_deref(),
            &id,
            Some(&message_id),
            &ctx.cancellation,
        )
        .await;
    }
}

/// List a mailbox and return the ids of the envelopes that carry
/// `message_id` (ADR 0002 §D.6: only the Message-ID survives maildir
/// renames). Best-effort: a failed listing yields no matches.
async fn list_envelope_ids_with_message_id(
    cli: &Cli,
    mailbox: &str,
    message_id: &str,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Vec<String> {
    let argv = command::envelope_list_argv(
        cli.config.as_deref(),
        cli.account.as_deref(),
        mailbox,
        1,
        100,
    );
    let Ok(output) = process::run(&cli.program, &argv, cancellation).await else {
        return Vec::new();
    };
    let Ok(listed) = process::decode_on_pool::<dto::EnvelopesDto>(output).await else {
        return Vec::new();
    };
    let bare = crate::domain::message::bare_message_id(message_id);
    listed
        .envelopes
        .into_iter()
        .filter(|envelope| envelope.message_id.as_deref() == Some(bare))
        .map(|envelope| envelope.id)
        .collect()
}

/// Group a locator batch by mailbox (a bulk selection is single-mailbox
/// today; grouping keeps that assumption local): one `(mailbox, ids)`
/// entry per mailbox, message ids in arrival order. Shared by the batched
/// read-flag, archive, and trash paths (tickets aavy/j9bq).
fn grouped_mailbox_ids(locators: &[MessageLocator]) -> Vec<(String, Vec<String>)> {
    let mut groups: std::collections::HashMap<&str, Vec<String>> = std::collections::HashMap::new();
    for locator in locators {
        groups
            .entry(locator.mailbox.0.as_str())
            .or_default()
            .push(locator.id.0.clone());
    }
    groups
        .into_iter()
        .map(|(mailbox, ids)| (mailbox.to_string(), ids))
        .collect()
}

/// Grouped CLI identity for the detached cleanup helpers, so the delete
/// calls stay under clippy's argument budget and read as one unit.
struct Cli {
    program: String,
    config: Option<PathBuf>,
    account: Option<String>,
}

/// The two-phase delete for one copy: delete in `mailbox`, then locate the
/// moved copy in the trash mailbox by `Message-ID` and delete again
/// (maildir renames give it a new id; only the Message-ID survives the
/// move, ADR 0002 §D.6). Skips phase 2 when the copy already lived in
/// trash (the first delete removed it outright).
async fn run_two_phase_delete(
    cli: &Cli,
    mailbox: &str,
    trash: Option<&str>,
    id: &str,
    message_id: Option<&str>,
    cancellation: &tokio_util::sync::CancellationToken,
) {
    let argv = command::message_delete_argv(
        cli.config.as_deref(),
        cli.account.as_deref(),
        mailbox,
        [id].as_slice(),
    );
    if let Err(err) = process::run(&cli.program, &argv, cancellation).await {
        if !matches!(err, BackendError::Cancelled) {
            tracing::warn!(old = %id, mailbox, %err, "draft copy could not be deleted");
        }
        return;
    }
    let Some(trash) = trash.filter(|trash| *trash != mailbox) else {
        return;
    };
    let Some(message_id) = message_id else {
        tracing::warn!(old = %id, "no Message-ID; trashed draft copy cannot be located");
        return;
    };
    if message_id.is_empty() {
        tracing::warn!(old = %id, "no Message-ID; trashed draft copy cannot be located");
        return;
    }
    let bare = bare_message_id(message_id);
    let ids = list_envelope_ids_with_message_id(cli, trash, &bare, cancellation).await;
    for id in ids {
        let argv = command::message_delete_argv(
            cli.config.as_deref(),
            cli.account.as_deref(),
            trash,
            [id.as_str()].as_slice(),
        );
        match process::run(&cli.program, &argv, cancellation).await {
            Ok(_) => tracing::debug!(id = %id, "trashed draft copy purged"),
            Err(err) if !matches!(err, BackendError::Cancelled) => {
                tracing::warn!(id = %id, %err, "trashed draft copy could not be purged");
            }
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod role_target_tests {
    use super::*;
    use crate::domain::MailboxId;

    fn new_backend(aliases: &[(&str, &str)]) -> HimalayaCliBackend {
        HimalayaCliBackend::new(
            "himalaya",
            None,
            Some(String::from("gmail")),
            aliases
                .iter()
                .map(|(k, v)| (String::from(*k), String::from(*v)))
                .collect(),
        )
    }

    fn mailbox(id: &str, name: &str, role: Option<MailboxRole>) -> Mailbox {
        Mailbox {
            id: MailboxId(String::from(id)),
            name: String::from(name),
            role,
            unread_count: None,
            total_count: None,
        }
    }

    /// The real captured Gmail listing shape: the account exposes no
    /// All Mail folder at all, while the config still maps archive to it.
    fn gmail_like_listing() -> Vec<Mailbox> {
        vec![
            mailbox("Inbox", "Inbox", Some(MailboxRole::Inbox)),
            mailbox(
                "[Gmail]/Drafts",
                "[Gmail]/Drafts",
                Some(MailboxRole::Drafts),
            ),
            mailbox("[Gmail]/Trash", "[Gmail]/Trash", Some(MailboxRole::Trash)),
        ]
    }

    #[test]
    fn alias_target_missing_from_the_listing_is_refused_with_guidance() {
        let backend = new_backend(&[
            ("archive", "[Gmail]/All Mail"),
            ("drafts", "[Gmail]/Drafts"),
        ]);
        *backend.mailboxes.write().expect("cache") = Some(gmail_like_listing());
        let err = backend
            .verified_role_target(MailboxRole::Archive, "archive")
            .expect_err("missing target must be refused");
        assert!(err.contains("[Gmail]/All Mail"), "{err}");
        assert!(err.contains("no such mailbox"), "{err}");
        assert!(err.contains("mailbox.alias] archive"), "{err}");
        // The drafts alias exists on the account: it resolves unchanged.
        assert_eq!(
            backend
                .verified_role_target(MailboxRole::Drafts, "drafts")
                .expect("existing target"),
            "[Gmail]/Drafts"
        );
    }

    #[test]
    fn listing_role_wins_over_the_alias_value() {
        let backend = new_backend(&[("archive", "Archive")]);
        *backend.mailboxes.write().expect("cache") =
            Some(vec![mailbox("All", "All Mail", Some(MailboxRole::Archive))]);
        assert_eq!(
            backend
                .verified_role_target(MailboxRole::Archive, "archive")
                .expect("role-resolved"),
            "All"
        );
    }

    #[test]
    fn without_a_listing_the_alias_falls_through() {
        // Before the sidebar has loaded there is nothing to verify against;
        // the historical behavior (attempt the alias) is preserved.
        let backend = new_backend(&[("archive", "[Gmail]/All Mail")]);
        assert_eq!(
            backend
                .verified_role_target(MailboxRole::Archive, "archive")
                .expect("alias falls through"),
            "[Gmail]/All Mail"
        );
        // No alias either: the actionable refusal.
        let backend = new_backend(&[]);
        let err = backend
            .verified_role_target(MailboxRole::Archive, "archive")
            .expect_err("no role, no alias");
        assert!(err.contains("no archive mailbox is known"), "{err}");
    }
}

#[cfg(test)]
mod attachment_tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn temp_file(name: &str, contents: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join(name);
        std::fs::write(&path, contents).expect("write fixture");
        (dir, path)
    }

    #[test]
    fn accepts_regular_readable_files_and_reports_metadata() {
        let (_dir, path) = temp_file("report final.pdf", b"%PDF-1.4 bytes");
        let att = validate_attachment_source(&path).expect("valid source");
        assert_eq!(att.name, "report final.pdf");
        assert_eq!(att.size, 14);
        assert_eq!(att.path, path, "already absolute: unchanged");
    }

    #[test]
    fn expands_tilde_against_home() {
        let (_dir, path) = temp_file("doc.pdf", b"x");
        // Point the injected home at the fixture's parent so `~/doc.pdf`
        // resolves to it (the expansion itself is exercised in
        // domain::paths; here we prove the backend consults it).
        let home = path.parent().unwrap().to_path_buf();
        let raw = PathBuf::from("~/doc.pdf");
        let att = validate_attachment_source_inner(
            &raw,
            Some(&home),
            crate::domain::paths::MAX_DRAFT_ATTACHMENT_BYTES,
        )
        .expect("valid");
        assert_eq!(att.path, path);
    }

    #[test]
    fn missing_files_name_the_path_in_the_error() {
        let err =
            validate_attachment_source(Path::new("/nonexistent/x y.pdf")).expect_err("missing");
        match err {
            BackendError::File(detail) => {
                assert!(detail.contains("/nonexistent/x y.pdf"), "{detail}");
                assert!(detail.contains("does not exist"), "{detail}");
            }
            other => panic!("expected a File error, got {other:?}"),
        }
    }

    #[test]
    fn directories_are_rejected_as_not_regular() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let err = validate_attachment_source(dir.path()).expect_err("directory");
        assert!(matches!(err, BackendError::File(_)));
    }

    #[test]
    fn oversized_files_are_rejected_with_the_limit() {
        let (_dir, path) = temp_file("big.bin", b"x");
        let att = validate_attachment_source_inner(&path, None, 1000).expect("under limit ok");
        assert_eq!(att.size, 1);
        // A path larger than the limit is refused with both numbers.
        let err = validate_attachment_source_inner(&path, None, 0)
            .expect_err("zero limit refuses everything");
        match err {
            BackendError::File(detail) => {
                assert!(detail.contains(&path.display().to_string()), "{detail}");
                assert!(detail.contains("the limit"), "{detail}");
            }
            other => panic!("expected a File error, got {other:?}"),
        }
    }

    #[test]
    // Non-root-solvable permissions semantics are POSIX-only; on Windows
    // the file stays readable and the test premise cannot hold.
    #[cfg(unix)]
    fn unreadable_files_are_rejected() {
        let (_dir, path) = temp_file("secret.bin", b"x");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&path, perms).expect("chmod");
        // Running as root would still read it; skip in that environment.
        if std::env::var("USER").as_deref() == Ok("root") {
            return;
        }
        let err = validate_attachment_source(&path).expect_err("unreadable");
        assert!(matches!(err, BackendError::File(_)));
    }

    /// The download source must live in the tempdir (ticket m89w): a
    /// relative name inside the root is accepted and returned canonically,
    /// including nested rows. Real himalaya reports its download as an
    /// absolute path, so an absolute row *inside* the root is the
    /// legitimate contract and is accepted the same way.
    #[test]
    fn confined_source_accepts_names_inside_the_root() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        std::fs::write(dir.path().join("report.pdf"), b"%PDF").expect("write fixture");
        let source = confined_source_path("report.pdf", dir.path()).expect("relative inside");
        assert!(source.starts_with(dir.path().canonicalize().expect("canonical root")));
        assert!(source.ends_with("report.pdf"));

        std::fs::create_dir(dir.path().join("sub")).expect("mkdir");
        std::fs::write(dir.path().join("sub/a.bin"), b"x").expect("write fixture");
        let source = confined_source_path("sub/a.bin", dir.path()).expect("nested inside");
        assert!(source.ends_with("sub/a.bin"));

        let absolute = dir.path().join("report.pdf");
        let source = confined_source_path(&absolute.display().to_string(), dir.path())
            .expect("absolute inside");
        assert!(source.ends_with("report.pdf"));
    }

    /// Any path that canonically resolves outside the tempdir is refused
    /// (ticket m89w): the backend must not be able to aim Tmail at, say,
    /// `/etc/passwd` and have it copied into the downloads directory.
    #[test]
    fn confined_source_rejects_paths_outside_the_root() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let err = confined_source_path("/etc/passwd", dir.path()).expect_err("absolute outside");
        match err {
            BackendError::InvalidOutput(detail) => {
                assert!(detail.contains("/etc/passwd"), "{detail}");
                assert!(detail.contains("outside"), "{detail}");
            }
            other => panic!("expected InvalidOutput, got {other:?}"),
        }
    }

    /// A `..` traversal (legal-looking relative) resolves out of the root
    /// and is refused (ticket m89w).
    #[test]
    fn confined_source_rejects_dotdot_traversal() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let outside = tempfile::TempDir::new().expect("outside dir");
        std::fs::write(outside.path().join("escape.txt"), b"x").expect("write fixture");
        let err = confined_source_path("../escape.txt", dir.path()).expect_err("traversal");
        assert!(matches!(err, BackendError::InvalidOutput(_)));
    }

    /// A symlink inside the root that points outside resolves out of the
    /// root under canonicalize and is refused (ticket m89w).
    #[cfg(unix)]
    #[test]
    fn confined_source_rejects_symlinks_out_of_the_root() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let outside = tempfile::TempDir::new().expect("outside dir");
        std::fs::write(outside.path().join("target.txt"), b"x").expect("write fixture");
        std::os::unix::fs::symlink(
            outside.path().join("target.txt"),
            dir.path().join("link.pdf"),
        )
        .expect("symlink");
        let err = confined_source_path("link.pdf", dir.path()).expect_err("symlink escape");
        assert!(matches!(err, BackendError::InvalidOutput(_)));
    }

    /// Missing and non-file rows are refused with clear errors even when
    /// they are contained (ticket m89w).
    #[test]
    fn confined_source_refuses_missing_and_non_file_paths() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let err = confined_source_path("missing.pdf", dir.path()).expect_err("missing");
        match err {
            BackendError::InvalidOutput(detail) => {
                assert!(detail.contains("missing.pdf"), "{detail}");
                assert!(detail.contains("does not resolve"), "{detail}");
            }
            other => panic!("expected InvalidOutput, got {other:?}"),
        }
        std::fs::create_dir(dir.path().join("subdir")).expect("mkdir");
        let err = confined_source_path("subdir", dir.path()).expect_err("directory");
        match err {
            BackendError::InvalidOutput(detail) => {
                assert!(detail.contains("regular file"), "{detail}");
            }
            other => panic!("expected InvalidOutput, got {other:?}"),
        }
    }

    #[test]
    fn downloads_dir_resolution_prefers_request_then_config_then_default() {
        let home = Path::new("/home/ada");
        // Explicit request wins verbatim (absolute).
        let req = Path::new("/tmp/My Downloads");
        assert_eq!(
            resolve_downloads_dir_with(Some(req), Some(Path::new("/cfg/dl")), Some(home))
                .expect("resolves"),
            req
        );
        // Config fills in when no request; `~` expands against home.
        assert_eq!(
            resolve_downloads_dir_with(None, Some(Path::new("~/Downloads")), Some(home))
                .expect("resolves"),
            Path::new("/home/ada/Downloads")
        );
        // Default: $HOME/Downloads.
        assert_eq!(
            resolve_downloads_dir_with(None, None, Some(home)).expect("resolves"),
            Path::new("/home/ada/Downloads")
        );
        // No home and nothing configured is a detailed refusal.
        let err = resolve_downloads_dir_with(None, None, None).expect_err("no home");
        assert!(matches!(err, BackendError::File(_)));
    }

    #[test]
    fn destination_names_reduce_to_single_components() {
        // Traversal attempts collapse to the final component.
        assert_eq!(destination_component(Some("../../.zshenv"), 3), ".zshenv");
        assert_eq!(destination_component(Some("/etc/passwd"), 3), "passwd");
        // A bare `..`, empty names, and non-names fall back to the part id.
        assert_eq!(destination_component(Some(".."), 3), "attachment-3");
        assert_eq!(destination_component(Some(""), 3), "attachment-3");
        assert_eq!(destination_component(None, 7), "attachment-7");
        // Ordinary names, spaces included, pass through.
        assert_eq!(
            destination_component(Some("report final.pdf"), 3),
            "report final.pdf"
        );
    }

    #[test]
    fn collision_walk_splits_stem_and_extension() {
        assert_eq!(
            split_stem_ext("report.pdf"),
            ("report".into(), ".pdf".into())
        );
        // Multiple dots: only the last is the extension.
        assert_eq!(
            split_stem_ext("my.report.final.tar"),
            ("my.report.final".into(), ".tar".into())
        );
        // Dotfiles keep their whole name as the stem.
        assert_eq!(split_stem_ext(".zshenv"), (".zshenv".into(), "".into()));
        // Extension-less names have nothing to strip.
        assert_eq!(
            split_stem_ext("attachment-3"),
            ("attachment-3".into(), "".into())
        );
    }

    #[test]
    fn collision_safe_writes_never_overwrite() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let dir = dir.path().to_path_buf();
        std::fs::write(dir.join("file.bin"), b"v1").expect("seed");

        // First save takes the plain name when free.
        let first = write_collision_safe(&dir, "other.bin", b"a").expect("writes");
        assert_eq!(first, dir.join("other.bin"));
        // Second save walks to a numbered name; the first is untouched.
        let second = write_collision_safe(&dir, "other.bin", b"bb").expect("writes");
        assert_eq!(second, dir.join("other (1).bin"));
        assert_eq!(std::fs::read(&first).unwrap(), b"a");
        assert_eq!(std::fs::read(&second).unwrap(), b"bb");
        // Extensions survive the walk; dotfiles number as whole names.
        let third = write_collision_safe(&dir, "file.bin", b"ccc").expect("writes");
        assert_eq!(third, dir.join("file (1).bin"));
        std::fs::write(dir.join(".zshenv"), b"old").expect("seed dotfile");
        let fourth = write_collision_safe(&dir, ".zshenv", b"d").expect("writes");
        assert_eq!(fourth, dir.join(".zshenv (1)"));
        assert_eq!(std::fs::read(dir.join("file.bin")).unwrap(), b"v1");
        assert_eq!(std::fs::read(dir.join(".zshenv")).unwrap(), b"old");
    }
}

#[cfg(all(test, feature = "test-fixtures"))]
mod attachment_mime_tests {
    //! Round-trip proof for outgoing attachments (plan §15, Phase 8
    //! acceptance): the serializer's bytes are parsed with `mail-parser` —
    //! the same library Himalaya embeds — and the filename, media type,
    //! bytes, and size must all survive.

    use super::*;
    use crate::domain::{OutboundAttachment, OutgoingContent};
    use mail_parser::MimeHeaders;

    fn backend() -> HimalayaCliBackend {
        HimalayaCliBackend::new("himalaya", None, None, HashMap::new())
            .with_account_identity(Some(String::from("probe@tmail.local")), None)
    }

    fn outbound(attachments: Vec<OutboundAttachment>) -> OutboundMessage {
        OutboundMessage::with_attachments(
            "ada@example.org",
            "",
            "",
            OutgoingContent {
                subject: String::from("With files"),
                body: String::from("see attached"),
                in_reply_to: None,
                references: None,
            },
            None,
            attachments,
        )
        .expect("valid recipients")
    }

    #[tokio::test]
    async fn attachments_round_trip_filename_media_type_bytes_and_size() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        // A name with a space and a binary payload with non-UTF-8 bytes.
        let pdf_path = dir.path().join("report final.pdf");
        let pdf_bytes: &[u8] = b"%PDF-1.4\n\x01\x02\xff\xfe payload";
        std::fs::write(&pdf_path, pdf_bytes).expect("write pdf");
        let txt_path = dir.path().join("notes.txt");
        std::fs::write(&txt_path, b"line one\nline two\n").expect("write txt");

        let message = outbound(vec![
            OutboundAttachment {
                name: String::from("report final.pdf"),
                path: pdf_path,
            },
            OutboundAttachment {
                name: String::from("notes.txt"),
                path: txt_path,
            },
        ]);
        let wire = backend()
            .serialize_outbound(&message)
            .await
            .expect("serializes");

        let parsed = mail_parser::MessageParser::default()
            .parse(&wire)
            .expect("wire bytes are parseable MIME");
        let parts: Vec<_> = parsed.attachments().collect();
        assert_eq!(parts.len(), 2, "both attachments ride the wire");

        // Wire order preserved (insertion order, deterministic duplicates).
        let first = parts[0];
        assert_eq!(first.attachment_name(), Some("report final.pdf"));
        let ct = first.content_type().expect("content type");
        assert_eq!(ct.c_type.as_ref(), "application");
        assert_eq!(ct.c_subtype.as_ref().map(|s| s.as_ref()), Some("pdf"));
        assert_eq!(first.contents(), pdf_bytes, "bytes byte-for-byte");
        assert_eq!(first.contents().len() as u64, pdf_bytes.len() as u64);

        let second = parts[1];
        assert_eq!(second.attachment_name(), Some("notes.txt"));
        assert!(second.is_content_type("text", "plain"));
        assert_eq!(second.contents(), b"line one\nline two\n");

        // The body text remains the plain part alongside the attachments.
        assert!(wire.windows(9).any(|w| w == b"multipart"), "mixed body");
    }

    #[tokio::test]
    async fn missing_attachment_files_are_detailed_retryable_refusals() {
        let message = outbound(vec![OutboundAttachment {
            name: String::from("gone.pdf"),
            path: std::path::PathBuf::from("/nonexistent/gone.pdf"),
        }]);
        let err = backend()
            .serialize_outbound(&message)
            .await
            .expect_err("file missing");
        match err {
            BackendError::File(detail) => {
                assert!(detail.contains("/nonexistent/gone.pdf"), "{detail}");
                assert!(detail.contains("could not be read"), "{detail}");
            }
            other => panic!("expected a File error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_send_without_attachments_stays_single_part() {
        let wire = backend()
            .serialize_outbound(&outbound(Vec::new()))
            .await
            .expect("ok");
        let text = String::from_utf8_lossy(&wire);
        assert!(!text.contains("multipart"), "no attachment scaffolding");
        assert!(text.contains("see attached"));
    }
}

#[cfg(test)]
mod send_tests {
    use super::*;

    fn output(code: Option<i32>, stdout: &str, stderr: &str) -> process::ChildOutput {
        process::ChildOutput {
            program: String::from("himalaya"),
            code,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn exit_zero_is_sent_even_with_odd_output() {
        // Claiming failure on exit 0 would invite a duplicate send.
        assert_eq!(
            classify_send(output(Some(0), "junk", "")),
            SendOutcome::Sent
        );
        assert_eq!(
            classify_send(output(
                Some(0),
                r#"{"message":"Message successfully sent"}"#,
                ""
            )),
            SendOutcome::Sent
        );
    }

    #[test]
    fn connection_failures_are_failed_before_delivery() {
        let outcome = classify_send(output(
            Some(1),
            r#"{"error":"connect 127.0.0.1:3425: connection refused","sources":["smtp"]}"#,
            "",
        ));
        assert_eq!(
            outcome,
            SendOutcome::FailedBeforeDelivery {
                code: Some(1),
                detail: String::from("connect 127.0.0.1:3425: connection refused (smtp)"),
            }
        );
        assert!(!outcome.is_ambiguous());
    }

    #[test]
    fn data_phase_errors_are_unknown() {
        // Probe-verified AMBIGUOUS: the sink received the payload.
        let outcome = classify_send(output(
            Some(1),
            r#"{"error":"SMTP DATA failed: Reached unexpected EOF"}"#,
            "",
        ));
        assert_eq!(
            outcome,
            SendOutcome::Unknown {
                code: Some(1),
                detail: String::from("SMTP DATA failed: Reached unexpected EOF"),
            }
        );
        assert!(outcome.is_ambiguous());
    }

    #[test]
    fn unclassifiable_errors_are_conservatively_unknown() {
        let outcome = classify_send(output(Some(1), r#"{"error":"something odd"}"#, ""));
        assert!(matches!(outcome, SendOutcome::Unknown { .. }));
        assert!(outcome.is_ambiguous());
        // No stdout JSON at all falls back to stderr, then the generic note.
        let outcome = classify_send(output(Some(2), "", "killed by signal?"));
        assert!(matches!(outcome, SendOutcome::Unknown { .. }));
        assert_eq!(outcome.detail(), "killed by signal?");
    }

    #[test]
    fn pre_delivery_marker_vocabulary() {
        assert!(pre_delivery_failure("connect 127.0.0.1:1: refused"));
        assert!(pre_delivery_failure("TLS handshake failed"));
        assert!(pre_delivery_failure("failed to resolve host"));
        assert!(pre_delivery_failure("path /x/NoBox is not a directory"));
        assert!(pre_delivery_failure("authentication failed"));
        // Explicit DATA-phase markers are never pre-delivery…
        assert!(!pre_delivery_failure("SMTP DATA failed: timeout"));
        // Ticket 9gx6: mid-session transport phrases read like dial
        // failures but the payload may already be transmitted.
        assert!(!pre_delivery_failure("write tcp: connection reset by peer"));
        assert!(!pre_delivery_failure("SMTP connection closed by peer"));
        // …and everything unlisted is conservative.
        assert!(!pre_delivery_failure("mailbox disappeared"));
    }
}

#[cfg(test)]
mod stray_sweep_tests {
    //! The detached stray-draft sweep is bounded to one at a time
    //! (ticket 8s0g): a sweep in flight makes further saves skip their
    //! sweep instead of piling up unbounded detached work.

    use super::*;
    use crate::domain::operation::OperationId;
    use tokio_util::sync::CancellationToken;

    fn ctx() -> RequestContext {
        RequestContext {
            operation: OperationId(1),
            cancellation: CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn a_second_sweep_is_skipped_while_one_is_in_flight() {
        let backend = HimalayaCliBackend::new("himalaya", None, None, HashMap::new());
        // Hold the single permit the way an in-flight sweep does.
        let permit = backend
            .sweeps
            .clone()
            .try_acquire_owned()
            .expect("idle permit");
        backend.delete_stray_draft_copies(&ctx(), "Drafts", None, &Some(String::from("<a@b>")));
        // The skipped call neither took nor released the permit.
        assert!(
            backend.sweeps.try_acquire().is_err(),
            "permit must still be held by the in-flight sweep"
        );
        drop(permit);
    }

    #[tokio::test]
    async fn an_idle_sweep_slot_is_taken_and_released_by_a_spawned_sweep() {
        let backend = HimalayaCliBackend::new("no-such-himalaya", None, None, HashMap::new());
        backend.delete_stray_draft_copies(&ctx(), "Drafts", None, &Some(String::from("<a@b>")));
        // The sweep took the permit; its processes fail fast (no such
        // program), and when it ends the permit is released again.
        assert!(
            backend.sweeps.try_acquire().is_err(),
            "permit held while the sweep runs"
        );
        // Wait for the spawned sweep to finish its failed runs, then the
        // slot must be free again.
        for _ in 0..200 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            if backend.sweeps.try_acquire().is_ok() {
                return;
            }
        }
        panic!("sweep permit was never released");
    }
}
