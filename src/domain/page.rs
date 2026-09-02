use super::mailbox::MailboxId;

/// An explicit page request against one mailbox (plan §7). `offset` is
/// 0-based and `limit` is the fixed page size; the Himalaya adapter maps
/// this onto its 1-based `-p`/`-s` flags (ADR 0001 finding 8). Serializable
/// so typed retry intents (plan §12) can be stored.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PageRequest {
    pub mailbox_id: MailboxId,
    pub offset: usize,
    pub limit: usize,
}

/// An explicitly paginated slice of items (plan §7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub offset: usize,
    pub limit: usize,
    /// `None` when the backend cannot provide a total (maildir, ADR 0001).
    pub total: Option<usize>,
}

impl<T> Page<T> {
    pub fn empty(limit: usize) -> Self {
        Self {
            items: Vec::new(),
            offset: 0,
            limit,
            total: None,
        }
    }

    /// Whether a previous page exists.
    pub fn has_previous(&self) -> bool {
        self.offset > 0
    }

    /// Whether a next page exists. With a known total this is exact; without
    /// one (maildir, ADR 0001 finding 2) a *full* page is assumed to have a
    /// successor — asking for the next page then either yields more items or
    /// a valid empty page, and the UI degrades to next-availability instead
    /// of a total (plan §16).
    pub fn has_next(&self) -> bool {
        match self.total {
            Some(total) => self.offset + self.items.len() < total,
            None => self.items.len() == self.limit,
        }
    }

    /// Human-readable range label: `1–20 of 312`, degrading to `1–20` or
    /// an empty string when nothing is known.
    pub fn range_label(&self) -> String {
        match (self.items.len(), self.total) {
            (0, _) => String::new(),
            (n, Some(total)) => format!("{}–{} of {}", self.offset + 1, self.offset + n, total),
            (n, None) => format!("{}–{}", self.offset + 1, self.offset + n),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(items: usize, total: Option<usize>) -> Page<u8> {
        Page {
            items: vec![0u8; items],
            offset: 0,
            limit: 20,
            total,
        }
    }

    #[test]
    fn labels() {
        assert_eq!(page(20, Some(312)).range_label(), "1–20 of 312");
        assert_eq!(page(5, None).range_label(), "1–5");
        assert_eq!(page(0, Some(0)).range_label(), "");
    }

    #[test]
    fn boundaries() {
        let mut p = page(20, Some(312));
        assert!(!p.has_previous());
        assert!(p.has_next());
        p.offset = 300;
        p.items = vec![0u8; 12];
        assert!(p.has_previous());
        assert!(!p.has_next());
        // Unknown total (maildir): a full page may have a successor, a short
        // page is the last one.
        let unknown = page(20, None);
        assert!(unknown.has_next());
        let unknown_last = page(5, None);
        assert!(!unknown_last.has_next());
        let unknown_empty = page(0, None);
        assert!(!unknown_empty.has_next());
    }
}
