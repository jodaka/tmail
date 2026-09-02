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

use async_trait::async_trait;

use crate::backend::traits::{BackendError, BackendResult, MailBackend};
use crate::config::Config;
use crate::domain::{Mailbox, MessageSummary, Page, PageRequest};

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
    async fn list_mailboxes(&self) -> BackendResult<Vec<Mailbox>> {
        let argv = command::mailbox_list_argv(self.config_path.as_deref(), self.account.as_deref());
        let output = process::run(&self.program, &argv).await?;
        let dto: dto::MailboxesDto = process::decode(output)?;
        Ok(map::mailboxes(dto, &self.aliases))
    }

    async fn list_messages(&self, page: PageRequest) -> BackendResult<Page<MessageSummary>> {
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
        let output = process::run(&self.program, &argv).await?;
        let dto: dto::EnvelopesDto = process::decode(output)?;
        Ok(map::envelopes(
            dto,
            page.mailbox_id.clone(),
            aligned,
            page.limit,
        ))
    }
}
