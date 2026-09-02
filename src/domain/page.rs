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

    /// Whether a next page exists. Requires a known total; without one the
    /// UI must not guess (plan §16 degrade path).
    pub fn has_next(&self) -> bool {
        match self.total {
            Some(total) => self.offset + self.items.len() < total,
            None => false,
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
        let unknown = page(20, None);
        assert!(!unknown.has_next());
    }
}
