//! `HimalayaCliBackend`: the Phase 2 `MailBackend` implementation on top of
//! the installed Himalaya CLI (ADR 0001 decision 1).
//!
//! Phase 2 covers the mailbox and message-list operations; read, flags,
//! search, send, and draft operations extend this impl in their phases. The
//! backend is stateless per call apart from its configuration, so it can be
//! shared freely once the Phase 3 operation manager holds an `Arc` to it.

mod command;
mod dto;
mod map;
mod process;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use crate::backend::traits::{BackendError, BackendResult, MailBackend, RequestContext};
use crate::config::Config;
use crate::domain::{
    Mailbox, MailboxRole, Message, MessageLocator, MessageSummary, Page, PageRequest,
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
        }
    }

    /// Backend wired from the loaded Post configuration.
    pub fn from_config(config: &Config) -> Self {
        Self::new(
            "himalaya",
            config.path.clone(),
            config.account.clone(),
            config.aliases.clone(),
        )
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
}
