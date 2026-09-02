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
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use crate::backend::journal::DraftJournal;
use crate::backend::traits::{BackendError, BackendResult, MailBackend, RequestContext};
use crate::config::Config;
use crate::domain::{
    DraftSnapshot, Mailbox, MailboxRole, Message, MessageId, MessageLocator, MessageSummary, Page,
    PageRequest,
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
        let target = self.mailbox_for_role(MailboxRole::Archive).ok_or_else(|| {
            BackendError::InvalidRequest(String::from(
                "no archive mailbox is known; load mailboxes first or set \
                 [accounts.<account>.mailbox.alias] archive",
            ))
        })?;
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
        let drafts = self.mailbox_for_role(MailboxRole::Drafts).ok_or_else(|| {
            BackendError::InvalidRequest(String::from(
                "no drafts mailbox is known; set \
                 [accounts.<account>.mailbox.alias] drafts",
            ))
        })?;
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
        //    the previous remote copy (ADR 0002 §D.3/§D.4 — failure here
        //    leaves a duplicate, never data loss; reconciliation is
        //    opportunistic).
        if let Some(old) = draft.remote_id.as_ref()
            && old != &new_id
        {
            let argv = command::message_delete_argv(
                self.config_path.as_deref(),
                self.account.as_deref(),
                &drafts,
                &old.0,
            );
            if let Err(err) = process::run(&self.program, &argv, &ctx.cancellation).await
                && !matches!(err, BackendError::Cancelled)
            {
                tracing::warn!(old = %old.0, %err, "old draft copy could not be deleted");
            }
        }

        // 5. Confirm the revision in the journal (newest pushed).
        self.journal
            .mark_remote(&draft.local_id.0, draft.revision)
            .map_err(BackendError::Io)?;

        Ok(new_id)
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

    /// Serialize one draft revision as a single-part `text/plain` RFC 5322
    /// message via the mail-builder library (plan §14: never hand-concatenate
    /// MIME). The stable `Message-ID` (ADR 0002 §D.6) and Post-owned
    /// `X-Post-Draft-Id` header make replacement and reconciliation
    /// possible; only valid parsed addresses are written (the composer
    /// flags invalid ones and send refuses them in Phase 7).
    fn serialize_draft(&self, draft: &DraftSnapshot) -> BackendResult<Vec<u8>> {
        use mail_builder::MessageBuilder;
        use mail_builder::headers::address::Address as MailAddress;

        /// Parse one composer address field into library addresses (valid
        /// entries only; invalid ones are flagged in the composer UI and
        /// refused at send time in Phase 7).
        fn parse(field: &str) -> Vec<MailAddress<'static>> {
            crate::domain::address::parse_address_list(field)
                .into_iter()
                .filter_map(Result::ok)
                .map(|a| MailAddress::new_address(a.name, a.email))
                .collect()
        }

        /// Header form: omitted when the field is empty.
        fn addresses(list: Vec<MailAddress<'static>>) -> Option<MailAddress<'static>> {
            (!list.is_empty()).then_some(MailAddress::List(list))
        }

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
            // The draft stores the full RFC form `<id-left@id-right>`;
            // mail-builder adds the angle brackets itself.
            let bare = message_id.trim_start_matches('<').trim_end_matches('>');
            builder = builder.message_id(bare);
        }
        if let Some(email) = &self.account_email {
            builder = builder.from(MailAddress::new_address(
                self.account_display_name.clone(),
                email.clone(),
            ));
        }
        // mail_builder takes ownership; build owned lists per field.
        builder = match addresses(parse(&draft.to)) {
            Some(addr) => builder.to(addr),
            None => builder,
        };
        builder = match addresses(parse(&draft.cc)) {
            Some(addr) => builder.cc(addr),
            None => builder,
        };
        builder = match addresses(parse(&draft.bcc)) {
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
}
