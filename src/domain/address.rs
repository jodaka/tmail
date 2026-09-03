use serde::{Deserialize, Serialize};

/// A mail address. `name` is the display name when the backend provides one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

    /// Composer-field form (`Name <email>` or bare email) used when reply/
    /// forward seeds fill the composer (Phase 7). Deliberately round-trips
    /// through [`Address::parse_entry`]: names that cannot be represented
    /// without escaping tricks (embedded quotes, angle brackets) fall back
    /// to the bare address rather than emitting something unparseable.
    pub fn to_field(&self) -> String {
        match &self.name {
            Some(name) if !name.is_empty() && !name.contains('"') => {
                if name.contains(',') {
                    format!("\"{name}\" <{}>", self.email)
                } else if name.contains('<') || name.contains('>') {
                    self.email.clone()
                } else {
                    format!("{name} <{}>", self.email)
                }
            }
            _ => self.email.clone(),
        }
    }

    /// Parse one address entry (plan §14 composer): `user@example.com`,
    /// `Name <user@example.com>`, or `"Name" <user@example.com>`. `None`
    /// when the entry holds no valid address (see [`is_valid_email`]).
    pub fn parse_entry(entry: &str) -> Option<Address> {
        let entry = entry.trim();
        if entry.is_empty() {
            return None;
        }
        // `Name <email>` / `"Name" <email>` form.
        if entry.contains('<') || entry.contains('>') {
            let open = entry.find('<')?;
            let close = entry.rfind('>')?;
            if close < open {
                return None; // Unbalanced.
            }
            let email = entry[open + 1..close].trim();
            if !is_valid_email(email) {
                return None;
            }
            let name = entry[..open].trim();
            let name = name.strip_prefix('"').unwrap_or(name);
            let name = name.strip_suffix('"').unwrap_or(name);
            let name = name.trim();
            if name.contains('<') || name.contains('>') {
                return None; // Stray brackets in the display name.
            }
            return Some(Address {
                name: (!name.is_empty()).then(|| name.to_string()),
                email: email.to_string(),
            });
        }
        // Bare address: no spaces allowed (would be a malformed entry).
        if entry.contains(char::is_whitespace) {
            return None;
        }
        is_valid_email(entry).then(|| Address {
            name: None,
            email: entry.to_string(),
        })
    }
}

/// Whether `email` is a usable RFC-style address for composing (plan §14
/// "address parsing/validation"): exactly one `@`, non-empty local part,
/// and a non-empty dotted domain without whitespace. Deliberately stricter
/// than RFC 5322 (no comments, no quoted local parts) and deliberately
/// looser where it does not matter for delivery (any non-empty labels).
pub fn is_valid_email(email: &str) -> bool {
    let email = email.trim();
    if email.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    if local.is_empty() || email.matches('@').count() != 1 {
        return false;
    }
    // Domain must be dotted and non-empty between the dots, so a bare
    // hostname like `user@localhost` is rejected (v1 composing targets
    // real recipients; internal short names are out of scope).
    domain.split('.').all(|label| !label.is_empty()) && domain.contains('.')
}

/// One comma/semicolon-separated entry of an address field with its
/// validity (plan §14). `range` is the byte range in the original input,
/// separators excluded, so the composer can style invalid entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressEntry {
    pub range: std::ops::Range<usize>,
    pub valid: bool,
}

/// Tokenize an address field into entries (empty entries between
/// separators are skipped, not errors) and validate each one.
pub fn address_entries(input: &str) -> Vec<AddressEntry> {
    let mut entries = Vec::new();
    let mut start = None;
    for (index, ch) in input.char_indices() {
        if ch == ',' || ch == ';' {
            if let Some(begin) = start.take() {
                push_entry(input, begin..index, &mut entries);
            }
        } else if start.is_none() && !ch.is_whitespace() {
            start = Some(index);
        }
    }
    if let Some(begin) = start {
        push_entry(input, begin..input.len(), &mut entries);
    }
    entries
}

fn push_entry(input: &str, range: std::ops::Range<usize>, entries: &mut Vec<AddressEntry>) {
    let text = input[range.clone()].trim();
    if text.is_empty() {
        return;
    }
    entries.push(AddressEntry {
        range,
        valid: Address::parse_entry(text).is_some(),
    });
}

/// Parse an address field into addresses; invalid entries are reported as
/// `Err` with their raw text so send (Phase 7) can refuse and the UI can
/// flag them.
pub fn parse_address_list(input: &str) -> Vec<Result<Address, String>> {
    address_entries(input)
        .iter()
        .map(|entry| {
            let text = input[entry.range.clone()].trim().to_string();
            if entry.valid {
                Address::parse_entry(&text).ok_or(text)
            } else {
                Err(text)
            }
        })
        .collect()
}

/// Comma-separated composer-field text for a list of addresses (Phase 7
/// reply/forward seeding); an empty list yields an empty field.
pub fn to_field_list(addresses: &[Address]) -> String {
    addresses
        .iter()
        .map(Address::to_field)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(name: &str, email: &str) -> Address {
        Address {
            name: Some(name.into()),
            email: email.into(),
        }
    }

    #[test]
    fn display_prefers_name() {
        let a = addr("Maksim Orlov", "maksim@example.com");
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

    #[test]
    fn parses_bare_addresses() {
        assert_eq!(
            Address::parse_entry("user@example.com"),
            Some(Address {
                name: None,
                email: String::from("user@example.com")
            })
        );
        assert_eq!(
            Address::parse_entry("  spaced@example.com  "),
            Address::parse_entry("spaced@example.com")
        );
    }

    #[test]
    fn parses_name_with_angle_brackets() {
        assert_eq!(
            Address::parse_entry("Maksim Orlov <m.orlov@mailbox.org>"),
            Some(addr("Maksim Orlov", "m.orlov@mailbox.org"))
        );
        assert_eq!(
            Address::parse_entry("\"Orlov, Maksim\" <m.orlov@mailbox.org>"),
            Some(addr("Orlov, Maksim", "m.orlov@mailbox.org")),
            "quoted names may contain separators"
        );
        assert_eq!(
            Address::parse_entry("<only@example.com>"),
            Some(Address {
                name: None,
                email: String::from("only@example.com")
            })
        );
    }

    #[test]
    fn rejects_invalid_entries() {
        assert_eq!(Address::parse_entry(""), None);
        assert_eq!(Address::parse_entry("   "), None);
        assert_eq!(Address::parse_entry("no-at-sign.com"), None);
        assert_eq!(Address::parse_entry("two@@example.com"), None);
        assert_eq!(Address::parse_entry("@no-local.example.com"), None);
        assert_eq!(Address::parse_entry("user@"), None);
        assert_eq!(
            Address::parse_entry("user@localhost"),
            None,
            "domain must be dotted"
        );
        assert_eq!(
            Address::parse_entry("user @example.com"),
            None,
            "bare addresses may not contain spaces"
        );
        assert_eq!(
            Address::parse_entry("Name <unbalanced@example.com"),
            None,
            "unbalanced brackets"
        );
        assert_eq!(
            Address::parse_entry("Name ><broken@example.com>"),
            None,
            "swapped brackets"
        );
    }

    #[test]
    fn entries_split_on_commas_and_semicolons() {
        let input = "a@example.com, ; b@x.org ;Maksim <m@y.io>,,";
        let entries = address_entries(input);
        assert_eq!(entries.len(), 3, "empty entries are skipped: {entries:?}");
        assert!(entries.iter().all(|e| e.valid));
        // Ranges cover the raw entry text for styling.
        assert_eq!(&input[entries[0].range.clone()], "a@example.com");
        assert_eq!(&input[entries[2].range.clone()], "Maksim <m@y.io>");
    }

    #[test]
    fn entries_flag_invalid_ones() {
        let entries = address_entries("ok@example.com, broken, also@fine.io");
        assert_eq!(entries.len(), 3);
        assert!(entries[0].valid);
        assert!(!entries[1].valid);
        assert!(entries[2].valid);
    }

    #[test]
    fn parse_list_reports_invalid_entries_as_err() {
        let parsed = parse_address_list("ok@example.com, broken");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].as_ref().unwrap().email, "ok@example.com");
        assert_eq!(parsed[1].as_ref().unwrap_err(), "broken");
    }

    #[test]
    fn parse_list_of_everything_invalid_is_all_err() {
        let parsed = parse_address_list("a b, @x.io");
        assert_eq!(parsed.len(), 2);
        assert!(parsed.iter().all(|r| r.is_err()));
    }

    #[test]
    fn to_field_round_trips_through_parse_entry() {
        let plain = addr("Ada Lovelace", "ada@example.org");
        assert_eq!(plain.to_field(), "Ada Lovelace <ada@example.org>");
        assert_eq!(Address::parse_entry(&plain.to_field()), Some(plain));
        // Commas force the quoted form, which parse_entry understands.
        let comma = addr("Orlov, Maksim", "m@example.org");
        assert_eq!(comma.to_field(), "\"Orlov, Maksim\" <m@example.org>");
        assert_eq!(Address::parse_entry(&comma.to_field()), Some(comma));
        // Unrepresentable names degrade to the bare address.
        let quoted = addr("Weird \"Name\"", "w@example.org");
        assert_eq!(quoted.to_field(), "w@example.org");
        let bracketed = addr("<Webmaster>", "web@example.org");
        assert_eq!(bracketed.to_field(), "web@example.org");
        assert_eq!(
            Address::parse_entry(&bracketed.to_field()).unwrap().email,
            "web@example.org"
        );
    }

    #[test]
    fn field_list_joins_with_commas() {
        let list = vec![addr("Ada", "a@x.io"), addr("Bob", "b@x.io")];
        assert_eq!(to_field_list(&list), "Ada <a@x.io>, Bob <b@x.io>");
        assert_eq!(to_field_list(&[]), "");
    }

    #[test]
    fn empty_field_parses_to_nothing() {
        assert!(address_entries("").is_empty());
        assert!(address_entries(" , ; ").is_empty());
        assert!(parse_address_list("").is_empty());
    }
}
