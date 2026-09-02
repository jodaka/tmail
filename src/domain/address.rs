/// A mail address. `name` is the display name when the backend provides one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub name: Option<String>,
    pub email: String,
}

impl Address {
    pub fn display(&self) -> &str {
        match &self.name {
            Some(name) if !name.is_empty() => name,
            _ => &self.email,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_prefers_name() {
        let a = Address {
            name: Some("Maksim Orlov".into()),
            email: "maksim@example.com".into(),
        };
        assert_eq!(a.display(), "Maksim Orlov");
    }

    #[test]
    fn display_falls_back_to_email() {
        let a = Address {
            name: None,
            email: "probe@post.local".into(),
        };
        assert_eq!(a.display(), "probe@post.local");
        let b = Address {
            name: Some(String::new()),
            email: "probe@post.local".into(),
        };
        assert_eq!(b.display(), "probe@post.local");
    }
}
