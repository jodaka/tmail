//! Secret redaction before details reach logs or the UI (plan §12: "Never
//! display or log credentials, OAuth tokens, authorization headers, secret
//! config values, or raw command environments").
//!
//! A small, dependency-free, deterministic filter: it redacts labeled
//! secret values (`password=…`, `"token": "…"`, `Authorization: …`),
//! bare `Bearer` credentials, and passwords embedded in URL userinfo
//! (`imaps://user:secret@host`). Everything else passes through unchanged,
//! so error details stay actionable. Failing safe (redacting too much of a
//! value-looking run) is acceptable; leaking a credential is not.

/// Secret labels whose value is redacted when followed by `=` or `:`.
/// Longest-first so e.g. `access_token` is preferred over `token`.
const SECRET_KEYS: &[&str] = &[
    "access_token",
    "refresh_token",
    "client_secret",
    "access-key",
    "access_key",
    "api-key",
    "api_key",
    "apikey",
    "authorization",
    "password",
    "passwd",
    "secret",
    "token",
    "pwd",
];

/// Redact secret-looking substrings from `input`.
pub fn sanitize(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut redacted = vec![false; chars.len()];
    redact_url_userinfo(&chars, &mut redacted);
    redact_labeled_values(&chars, &mut redacted);
    redact_bearer(&chars, &mut redacted);

    let mut out = String::with_capacity(input.len());
    for (index, ch) in chars.iter().enumerate() {
        if redacted[index] {
            out.push('█');
        } else {
            out.push(*ch);
        }
    }
    out
}

/// Mark `range` (char indices, end-exclusive) as redacted.
fn mark(redacted: &mut [bool], range: std::ops::Range<usize>) {
    for index in range {
        if index < redacted.len() {
            redacted[index] = true;
        }
    }
}

fn lowercase_at(chars: &[char], index: usize, key: &str) -> bool {
    chars[index..]
        .iter()
        .zip(key.chars())
        .all(|(ch, key_ch)| ch.to_ascii_lowercase() == key_ch)
}

/// `imaps://user:secret@host`, `--url user:pass@host`: the password part of
/// a userinfo segment (back to the previous `/` or whitespace) is redacted.
fn redact_url_userinfo(chars: &[char], redacted: &mut [bool]) {
    for (at, ch) in chars.iter().enumerate() {
        if *ch != '@' || at == 0 {
            continue;
        }
        // Backtrack to the start of the segment: previous '/' or whitespace.
        let mut start = at;
        while start > 0 {
            let prev = chars[start - 1];
            if prev == '/' || prev.is_whitespace() {
                break;
            }
            start -= 1;
        }
        // First ':' inside the segment splits user from password.
        let Some(colon) = (start..at).find(|&i| chars[i] == ':') else {
            continue;
        };
        if colon + 1 < at {
            mark(redacted, colon + 1..at);
        }
    }
}

/// `password=hunter2`, `"token": "abc"`, `secret: xyz`: redact the value
/// after the `=`/`:` separator. `authorization` redacts to end of line so
/// whole headers (`Authorization: Bearer xyz`) disappear.
fn redact_labeled_values(chars: &[char], redacted: &mut [bool]) {
    for index in 0..chars.len() {
        // Word boundary: a key never starts mid-word.
        if index > 0 && chars[index - 1].is_alphanumeric() {
            continue;
        }
        for key in SECRET_KEYS {
            if !lowercase_at(chars, index, key) {
                continue;
            }
            let mut cursor = index + key.len();
            while cursor < chars.len() && (chars[cursor] == ' ' || chars[cursor] == '\t') {
                cursor += 1;
            }
            // Optional closing quote between key and separator (JSON shape
            // `"token": …`).
            if matches!(chars.get(cursor), Some('"') | Some('\'')) {
                cursor += 1;
                while cursor < chars.len() && (chars[cursor] == ' ' || chars[cursor] == '\t') {
                    cursor += 1;
                }
            }
            if cursor >= chars.len() || (chars[cursor] != '=' && chars[cursor] != ':') {
                break;
            }
            cursor += 1;
            while cursor < chars.len() && (chars[cursor] == ' ' || chars[cursor] == '\t') {
                cursor += 1;
            }
            if matches!(chars.get(cursor), Some('"') | Some('\'')) {
                // Quoted value (`"token": "abc"`, `password='x'`): redact
                // inside the quotes, keep the quotes themselves.
                let quote = chars[cursor];
                match (cursor + 1..chars.len()).find(|&i| chars[i] == quote) {
                    Some(close) => mark(redacted, cursor + 1..close),
                    None => mark(redacted, cursor + 1..chars.len()),
                }
            } else if *key == "authorization" {
                let end = (cursor..chars.len())
                    .find(|&i| chars[i] == '\n')
                    .unwrap_or(chars.len());
                mark(redacted, cursor..end);
            } else {
                let end = (cursor..chars.len())
                    .find(|&i| {
                        chars[i].is_whitespace() || matches!(chars[i], ',' | ';' | ']' | ')')
                    })
                    .unwrap_or(chars.len());
                mark(redacted, cursor..end);
            }
            break;
        }
    }
}

/// A bare `Bearer <credential>` (OAuth access tokens) is redacted.
fn redact_bearer(chars: &[char], redacted: &mut [bool]) {
    for index in 0..chars.len() {
        if index > 0 && chars[index - 1].is_alphanumeric() {
            continue;
        }
        if !lowercase_at(chars, index, "bearer") {
            continue;
        }
        let mut cursor = index + "bearer".len();
        while cursor < chars.len() && chars[cursor].is_whitespace() {
            cursor += 1;
        }
        let end = (cursor..chars.len())
            .find(|&i| chars[i].is_whitespace() || chars[i] == ',')
            .unwrap_or(chars.len());
        if end > cursor {
            mark(redacted, cursor..end);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture secret used across the Phase 3 tests; must never appear in
    /// sanitized output.
    const FIXTURE_SECRET: &str = "hunter2";

    #[test]
    fn plain_errors_pass_through_unchanged() {
        let text = "mailbox not found (maildir)";
        assert_eq!(sanitize(text), text);
        let text = "connect error: connection refused 127.0.0.1:3425";
        assert_eq!(sanitize(text), text);
    }

    #[test]
    fn labeled_values_are_redacted() {
        assert_eq!(
            sanitize(&format!("password = \"{FIXTURE_SECRET}\"")),
            "password = \"███████\""
        );
        assert_eq!(sanitize("token: abc123"), "token: ██████");
        assert_eq!(sanitize("PWD=shh"), "PWD=███");
        assert_eq!(sanitize("api_key = 9f2c; next"), "api_key = ████; next");
    }

    #[test]
    fn json_shaped_secrets_are_redacted() {
        assert_eq!(
            sanitize("{\"client_secret\": \"abc\"}"),
            "{\"client_secret\": \"███\"}"
        );
    }

    #[test]
    fn authorization_headers_disappear_wholesale() {
        assert_eq!(
            sanitize("Authorization: Bearer ya29.abc def"),
            "Authorization: ███████████████████"
        );
    }

    #[test]
    fn bare_bearer_credentials_are_redacted() {
        assert_eq!(
            sanitize("auth failed for Bearer ya29.zzz token"),
            "auth failed for Bearer ████████ token"
        );
    }

    #[test]
    fn url_userinfo_passwords_are_redacted() {
        assert_eq!(
            sanitize("login to imaps://ada:hunter2@imap.example.org failed"),
            "login to imaps://ada:███████@imap.example.org failed"
        );
        // Username without a password stays intact.
        assert_eq!(
            sanitize("imaps://ada@imap.example.org"),
            "imaps://ada@imap.example.org"
        );
        // Host ports are not userinfo.
        assert_eq!(sanitize("connect 127.0.0.1:3425"), "connect 127.0.0.1:3425");
    }

    #[test]
    fn non_secret_colons_are_untouched() {
        // "token" not followed by a separator keeps its surroundings.
        assert_eq!(sanitize("token file missing"), "token file missing");
        assert_eq!(sanitize("retry at 12:30"), "retry at 12:30");
    }

    #[test]
    fn multiple_secrets_in_one_detail() {
        let out = sanitize("password=hunter2 token=abc imaps://u:pw@h");
        assert!(!out.contains(FIXTURE_SECRET));
        assert!(!out.contains("abc"));
        assert!(!out.contains("pw@"));
    }

    #[test]
    fn unicode_is_preserved_outside_secrets() {
        assert_eq!(
            sanitize("Пароль password=abc неверен"),
            "Пароль password=███ неверен"
        );
    }
}
