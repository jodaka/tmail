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
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address as MailAddress;

use crate::backend::journal::DraftJournal;
use crate::backend::traits::{BackendError, BackendResult, MailBackend, RequestContext};
use crate::config::Config;
use crate::domain::{
    AttachmentRequest, DraftAttachment, DraftSnapshot, Mailbox, MailboxRole, Message, MessageId,
    MessageLocator, MessageSummary, OutboundMessage, Page, PageRequest, RestoredDraft, SendOutcome,
};

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
    mailboxes: Arc<RwLock<Option<Vec<Mailbox>>>>,
    /// Post-owned crash-safe draft journal (ADR 0002 §D.1): every revision
    /// is recorded here before any remote call.
    journal: DraftJournal,
    /// Configured account identity, used as the `From` header of drafts.
    account_email: Option<String>,
    account_display_name: Option<String>,
    /// `[post.attachments].downloads_dir` (plan §17), as written; a leading
    /// `~` is expanded at use time. `None` falls back to `$HOME/Downloads`.
    downloads_dir: Option<PathBuf>,
}

impl HimalayaCliBackend {
    pub fn new(
        program: impl Into<String>,
        config_path: Option<PathBuf>,
        account: Option<String>,
        aliases: HashMap<String, String>,
    ) -> Self {
        Self {
            program: program.into(),
            config_path,
            account,
            aliases,
            mailboxes: Arc::new(RwLock::new(None)),
            journal: DraftJournal::open_default()
                .unwrap_or_else(|| DraftJournal::open(PathBuf::from("/dev/null/post-drafts"))),
            account_email: None,
            account_display_name: None,
            downloads_dir: None,
        }
    }

    /// Override the journal location (tests, explicit data dirs).
    pub fn with_journal(mut self, journal: DraftJournal) -> Self {
        self.journal = journal;
        self
    }

    /// Backend wired from the loaded Post configuration.
    pub fn from_config(config: &Config) -> Self {
        Self::new(
            "himalaya",
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
}

#[async_trait]
impl MailBackend for HimalayaCliBackend {
    async fn list_mailboxes(&self, ctx: RequestContext) -> BackendResult<Vec<Mailbox>> {
        tracing::debug!(operation = %ctx.operation, "list_mailboxes");
        let argv = command::mailbox_list_argv(self.config_path.as_deref(), self.account.as_deref());
        let output = process::run(&self.program, &argv, &ctx.cancellation).await?;
        let dto: dto::MailboxesDto = process::decode(output)?;
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
        if page.limit == 0 {
            return Err(BackendError::InvalidRequest(String::from(
                "page limit must be non-zero",
            )));
        }
        // Himalaya pages are 1-based and Post requests page-aligned offsets;
        // aligning here keeps an off-grid offset from silently reading a
        // different page than requested (ADR 0001 finding 8).
        let aligned = page.offset - page.offset % page.limit;
        let page_number = aligned / page.limit + 1;
        let argv = command::envelope_list_argv(
            self.config_path.as_deref(),
            self.account.as_deref(),
            &page.mailbox_id.0,
            page_number,
            page.limit,
        );
        let output = process::run(&self.program, &argv, &ctx.cancellation).await?;
        let dto: dto::EnvelopesDto = process::decode(output)?;
        Ok(map::envelopes(
            dto,
            page.mailbox_id.clone(),
            aligned,
            page.limit,
        ))
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
        let dto: dto::MessageReadDto = process::decode(output)?;
        Ok(map::message(dto, locator))
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
        // finding 5); no Post-side target resolution needed.
        let argv = command::message_delete_argv(
            self.config_path.as_deref(),
            self.account.as_deref(),
            &locator.mailbox.0,
            &locator.id.0,
        );
        let output = process::run(&self.program, &argv, &ctx.cancellation).await?;
        process::decode::<serde_json::Value>(output)?;
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
        //    never lost text.
        self.journal.record(&draft)?;

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
        let added: dto::MessageAddDto = process::decode(output)?;
        let new_id = MessageId(added.id);

        // 4. Only after the new copy is confirmed: best-effort deletion of
        //    every previous remote copy (ADR 0002 §D.3/§D.4 — failure here
        //    leaves a duplicate, never data loss). The known previous id
        //    is deleted explicitly; a sweep by the stable Message-ID then
        //    removes any stray copies (e.g. from a crash mid-replacement).
        if let Some(old) = draft.remote_id.as_ref()
            && old != &new_id
        {
            self.delete_draft_copy(&ctx, &drafts, &old.0, &draft.message_id);
        }
        self.delete_stray_draft_copies(&ctx, &drafts, Some(&new_id), &draft.message_id);

        // 5. Confirm the revision in the journal (newest pushed).
        self.journal
            .mark_remote(&draft.local_id.0, draft.revision)
            .map_err(BackendError::Io)?;

        Ok(new_id)
    }

    async fn load_drafts(&self, _ctx: RequestContext) -> BackendResult<Vec<RestoredDraft>> {
        tracing::debug!("load_drafts");
        // Purely local (ADR 0002 §D.1): the journal is the source of truth
        // for restore, independent of account reachability.
        Ok(self
            .journal
            .load_all()?
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
        self.journal.remove(&draft.local_id.0)?;
        if let Some(drafts) = self.mailbox_for_role(MailboxRole::Drafts) {
            if let Some(old) = draft.remote_id.as_ref() {
                self.delete_draft_copy(&ctx, &drafts, &old.0, &draft.message_id);
            }
            self.delete_stray_draft_copies(&ctx, &drafts, None, &draft.message_id);
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
        let bytes = self.serialize_outbound(&message)?;
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
        Ok(validate_attachment_source(&path)?)
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
        //    the platform default. Created when missing.
        let dir = resolve_downloads_dir(request.dir.as_deref(), self.downloads_dir.as_deref())?;
        std::fs::create_dir_all(&dir).map_err(|err| {
            BackendError::File(format!(
                "download directory `{}` could not be created: {err}",
                dir.display()
            ))
        })?;

        // 2. Download the part into a Post-owned private tempdir — never
        //    straight into the destination, so nothing there can be
        //    touched until the collision-checked write is ready. The dir
        //    travels as one argv entry: no shell, whatever the path.
        let temp = tempfile::tempdir()
            .map_err(|err| BackendError::File(format!("temporary download dir: {err}")))?;
        let argv = command::attachment_download_argv(
            self.config_path.as_deref(),
            self.account.as_deref(),
            &request.locator.mailbox.0,
            &request.locator.id.0,
            request.part_id,
            temp.path(),
        );
        let output = process::run(&self.program, &argv, &ctx.cancellation).await?;
        let dto: dto::AttachmentsDto = process::decode(output)?;
        let row = find_row(&dto, request.part_id)?;
        let source = row
            .path
            .as_deref()
            .map(PathBuf::from)
            .map(|path| {
                // Relative paths resolve against the tempdir himalaya was
                // pointed at; absolute ones pass through.
                if path.is_absolute() {
                    path
                } else {
                    temp.path().join(path)
                }
            })
            .ok_or_else(|| {
                BackendError::InvalidOutput(format!(
                    "attachment download row for part {} names no output path",
                    request.part_id
                ))
            })?;
        let bytes = std::fs::read(&source).map_err(|err| {
            BackendError::File(format!(
                "`{}` could not be read after download: {err}",
                source.display()
            ))
        })?;

        // 3. Destination name: the caller's display filename (reduced to a
        //    single component — traversal is impossible), else the row's,
        //    else a part-id fallback.
        let name = destination_component(
            request.filename.as_deref().or(row.filename.as_deref()),
            request.part_id,
        );
        let final_path = write_collision_safe(&dir, &name, &bytes)?;
        tracing::info!(
            part = request.part_id,
            bytes = bytes.len(),
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
            &locator.id.0,
        );
        let output = process::run(&self.program, &argv, &ctx.cancellation).await?;
        process::decode::<serde_json::Value>(output)?;
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
            &locator.id.0,
        );
        let output = process::run(&self.program, &argv, &ctx.cancellation).await?;
        process::decode::<serde_json::Value>(output)?;
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
        let alias_key = match role {
            MailboxRole::Archive => "archive",
            MailboxRole::Trash => "trash",
            MailboxRole::Inbox => "inbox",
            MailboxRole::Sent => "sent",
            MailboxRole::Drafts => "drafts",
            MailboxRole::Spam => "junk",
        };
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
                     {alias_key} to an existing mailbox"
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

    /// Best-effort deletion of one remote copy from `mailbox` (ADR 0002
    /// §D.4): `message delete` is trash-first, so a copy deleted elsewhere
    /// lands in the trash mailbox under a NEW backend id; locate it there
    /// by the stable `Message-ID` and delete again for permanent removal.
    /// Spawned detached so cleanup never delays the save result; every
    /// failure is logged and swallowed — a leftover copy is always
    /// preferable to risking data loss.
    fn delete_draft_copy(
        &self,
        ctx: &RequestContext,
        mailbox: &str,
        id: &str,
        message_id: &Option<String>,
    ) {
        let trash = self.mailbox_for_role(MailboxRole::Trash);
        spawn_draft_cleanup(
            Cli {
                program: self.program.clone(),
                config: self.config_path.clone(),
                account: self.account.clone(),
            },
            CleanupTarget {
                mailbox: mailbox.to_string(),
                id: id.to_string(),
                message_id: message_id.clone(),
            },
            trash,
            ctx.clone(),
        );
    }

    /// Remove every remote copy of a draft (matched by its stable
    /// `Message-ID`) from the Drafts mailbox except `keep` — the
    /// reconciliation sweep for copies orphaned by a crash mid-replacement
    /// (ADR 0002 §D.5). Best-effort; `envelope list` is requested with a
    /// generous page (v1 drafts are few).
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
        let cli = Cli {
            program: self.program.clone(),
            config: self.config_path.clone(),
            account: self.account.clone(),
        };
        let trash = self.mailbox_for_role(MailboxRole::Trash);
        let drafts_mailbox = drafts_mailbox.to_string();
        let keep = keep.map(|id| id.0.clone());
        let message_id = message_id.clone();
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let argv = command::envelope_list_argv(
                cli.config.as_deref(),
                cli.account.as_deref(),
                &drafts_mailbox,
                1,
                100,
            );
            let Ok(output) = process::run(&cli.program, &argv, &ctx.cancellation).await else {
                return;
            };
            let Ok(listed) = process::decode::<dto::EnvelopesDto>(output) else {
                return;
            };
            let bare = message_id.trim_start_matches('<').trim_end_matches('>');
            for envelope in listed.envelopes {
                if envelope.message_id.as_deref() != Some(bare)
                    || keep.as_deref() == Some(envelope.id.as_str())
                {
                    continue;
                }
                tracing::info!(id = %envelope.id, "deleting stray draft copy");
                run_two_phase_delete(
                    &cli,
                    &drafts_mailbox,
                    trash.as_deref(),
                    &envelope.id,
                    &message_id,
                    &ctx.cancellation,
                )
                .await;
            }
        });
    }

    /// Serialize one draft revision as a single-part `text/plain` RFC 5322
    /// message via the mail-builder library (plan §14: never hand-concatenate
    /// MIME). The stable `Message-ID` (ADR 0002 §D.6) and Post-owned
    /// `X-Post-Draft-Id` header make replacement and reconciliation
    /// possible; only valid parsed addresses are written (the composer
    /// flags invalid ones and send refuses them before starting, Phase 7).
    fn serialize_draft(&self, draft: &DraftSnapshot) -> BackendResult<Vec<u8>> {
        if draft.message_id.is_none() {
            return Err(BackendError::InvalidRequest(String::from(
                "draft is missing a stable Message-ID; it must be minted \
                 before the first save",
            )));
        }

        let mut builder = MessageBuilder::new()
            .date(chrono::Utc::now().timestamp())
            .header(
                "X-Post-Draft-Id",
                mail_builder::headers::raw::Raw::from(draft.local_id.0.clone()),
            );
        if let Some(message_id) = &draft.message_id {
            builder = builder.message_id(bare_message_id(message_id));
        }
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
    fn serialize_outbound(&self, message: &OutboundMessage) -> BackendResult<Vec<u8>> {
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
        let message_id = match &message.message_id {
            Some(id) => bare_message_id(id),
            None => format!(
                "{}.send@post.local",
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
        for attachment in &message.attachments {
            let bytes = std::fs::read(&attachment.path).map_err(|err| {
                BackendError::File(format!(
                    "`{}` could not be read for sending: {err}",
                    attachment.path.display()
                ))
            })?;
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

/// Bare id form for mail-builder, which adds the angle brackets itself.
fn bare_message_id(message_id: &str) -> String {
    message_id
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_string()
}

/// Resolve the destination directory for a save (plan §15): the request's
/// explicit directory wins, then the configured downloads dir, then the
/// platform default. `~` is expanded here — inside Post, never a shell.
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

/// Validate one attachment source path (plan §15, Phase 8): expand `~` in
/// Post (never a shell), then require an existing, regular, readable file
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
/// treated as unknown delivery state — except explicit DATA-phase markers,
/// which are always post-connection and therefore never pre-delivery.
fn pre_delivery_failure(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("smtp data") {
        return false;
    }
    const PRE_DATA_MARKERS: [&str; 14] = [
        "connect",            // "connect 127.0.0.1:3425: connection refused"
        "connection refused", // redundant with the above, kept for clarity
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

/// One remote draft copy to clean up.
struct CleanupTarget {
    mailbox: String,
    id: String,
    message_id: Option<String>,
}

/// Spawn a detached best-effort cleanup: delete one copy from its mailbox,
/// then purge the copy the trash-first delete parked in the trash mailbox
/// (ADR 0002 §D.4). Failures are logged, never propagated.
fn spawn_draft_cleanup(
    cli: Cli,
    target: CleanupTarget,
    trash: Option<String>,
    ctx: RequestContext,
) {
    tokio::spawn(async move {
        run_two_phase_delete(
            &cli,
            &target.mailbox,
            trash.as_deref(),
            &target.id,
            target.message_id.as_deref().unwrap_or_default(),
            &ctx.cancellation,
        )
        .await;
    });
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
    message_id: &str,
    cancellation: &tokio_util::sync::CancellationToken,
) {
    let argv =
        command::message_delete_argv(cli.config.as_deref(), cli.account.as_deref(), mailbox, id);
    if let Err(err) = process::run(&cli.program, &argv, cancellation).await {
        if !matches!(err, BackendError::Cancelled) {
            tracing::warn!(old = %id, mailbox, %err, "draft copy could not be deleted");
        }
        return;
    }
    let Some(trash) = trash.filter(|trash| *trash != mailbox) else {
        return;
    };
    if message_id.is_empty() {
        tracing::warn!(old = %id, "no Message-ID; trashed draft copy cannot be located");
        return;
    }
    let argv =
        command::envelope_list_argv(cli.config.as_deref(), cli.account.as_deref(), trash, 1, 100);
    let Ok(output) = process::run(&cli.program, &argv, cancellation).await else {
        return;
    };
    let Ok(listed) = process::decode::<dto::EnvelopesDto>(output) else {
        return;
    };
    let bare = message_id.trim_start_matches('<').trim_end_matches('>');
    for envelope in listed.envelopes {
        if envelope.message_id.as_deref() != Some(bare) {
            continue;
        }
        let argv = command::message_delete_argv(
            cli.config.as_deref(),
            cli.account.as_deref(),
            trash,
            &envelope.id,
        );
        match process::run(&cli.program, &argv, cancellation).await {
            Ok(_) => tracing::debug!(id = %envelope.id, "trashed draft copy purged"),
            Err(err) if !matches!(err, BackendError::Cancelled) => {
                tracing::warn!(id = %envelope.id, %err, "trashed draft copy could not be purged");
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
            .with_account_identity(Some(String::from("probe@post.local")), None)
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

    #[test]
    fn attachments_round_trip_filename_media_type_bytes_and_size() {
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
        let wire = backend().serialize_outbound(&message).expect("serializes");

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

    #[test]
    fn missing_attachment_files_are_detailed_retryable_refusals() {
        let message = outbound(vec![OutboundAttachment {
            name: String::from("gone.pdf"),
            path: std::path::PathBuf::from("/nonexistent/gone.pdf"),
        }]);
        let err = backend()
            .serialize_outbound(&message)
            .expect_err("file missing");
        match err {
            BackendError::File(detail) => {
                assert!(detail.contains("/nonexistent/gone.pdf"), "{detail}");
                assert!(detail.contains("could not be read"), "{detail}");
            }
            other => panic!("expected a File error, got {other:?}"),
        }
    }

    #[test]
    fn a_send_without_attachments_stays_single_part() {
        let wire = backend()
            .serialize_outbound(&outbound(Vec::new()))
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
        // …and everything unlisted is conservative.
        assert!(!pre_delivery_failure("mailbox disappeared"));
    }
}
