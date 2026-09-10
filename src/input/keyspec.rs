//! Key spec grammar (configurable keybindings, `src/input/keyboard.rs`):
//! one config string ↔ one `(KeyCode, ctrl, alt)` pair.
//!
//! A spec is `+`-joined, modifiers first, case-insensitive on modifier and
//! named-key tokens: `"Ctrl+R"`, `"esc"`, `"alt+enter"`. The key itself is
//! either a named key (`"esc"`, `"enter"`, `"tab"`, `"backtab"`,
//! `"backspace"`, `"delete"`, `"insert"`, `"home"`, `"end"`, `"pageup"`,
//! `"pagedown"`, `"up"`, `"down"`, `"left"`, `"right"`, `"space"`,
//! `"f1"`–`"f12"`), a glyph alias from the docs (`↑ ↓ ← → ⌫ ↵ ⇥`, and
//! `"shift+tab"` ≡ `"backtab"`), or any single character (`"j"`, `"?"`,
//! `"]"`, `"S"`). An uppercase letter is that character — crossterm
//! reports shifted letters as the uppercase `char`, so `"S"` means
//! exactly today's save-attachment key.
//!
//! Matching semantics are plain structural equality: the keymap stores
//! specs in a `HashMap` keyed by `(KeyCode, ctrl, alt)`, so lookup is key
//! equality against that triple. Character bindings
//! without `ctrl`/`alt` reject those modifiers (so `Ctrl+C` never fires a
//! plain `c` binding) but ignore `SHIFT` (terminals vary); `ctrl`/`alt`
//! chords require exactly those modifiers. `shift` is not representable on
//! named keys (rejected with a clear error): for characters it is implied
//! by the character itself, and `shift+tab` has its own `BackTab` code.
//!
//! Two chords are special-cased by the terminal layer: Ctrl+`\` `]` `^`
//! `_` arrive as `Char('4'..'7')` + CONTROL and are remapped to their keys
//! in `keyboard::normalize_ctrl_chords`, while Ctrl+`[` is not bindable at
//! all — its byte *is* ESC, so terminals report it as a plain Esc.

use crossterm::event::KeyCode;

/// One bindable key: a crossterm code plus the ctrl/alt chord state.
/// `SHIFT` is deliberately absent — it is either implied by the character
/// (uppercase letters, symbols) or has its own code (`BackTab`), and
/// terminals report it inconsistently for named keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeySpec {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
}

impl KeySpec {
    /// Plain (no modifiers) spec for `code`.
    pub fn plain(code: KeyCode) -> Self {
        KeySpec {
            code,
            ctrl: false,
            alt: false,
        }
    }

    /// The spec of a pressed key event: the event's code plus its
    /// ctrl/alt chord state (`SHIFT` does not enter a spec — see module
    /// docs). The shared normalization behind keymap lookup and the
    /// translation layer's own ctrl/alt checks.
    pub fn from_event(event: &crossterm::event::KeyEvent) -> Self {
        use crossterm::event::KeyModifiers;
        KeySpec {
            code: event.code,
            ctrl: event.modifiers.contains(KeyModifiers::CONTROL),
            alt: event.modifiers.contains(KeyModifiers::ALT),
        }
    }

    /// Canonical display form (hints, docs): `"Ctrl+R"`, `"Esc"`, `"→"`,
    /// `"S"`.
    pub fn display(&self) -> String {
        let mut out = String::new();
        if self.ctrl {
            out.push_str("Ctrl+");
        }
        if self.alt {
            out.push_str("Alt+");
        }
        out.push_str(&display_code(self.code));
        out
    }
}

/// Parse one key spec; `Err` carries a user-facing message with the spec
/// echoed back (secrets never appear in key specs).
pub fn parse_spec(spec: &str) -> Result<KeySpec, String> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err(String::from("empty key spec"));
    }
    // The one spelled-out shift chord: Shift+Tab is BackTab's own code
    // (checked before the `+` split claims `shift` as a modifier).
    if trimmed.eq_ignore_ascii_case("shift+tab") {
        return Ok(KeySpec {
            code: KeyCode::BackTab,
            ctrl: false,
            alt: false,
        });
    }
    let mut ctrl = false;
    let mut alt = false;
    let parts: Vec<&str> = trimmed.split('+').map(str::trim).collect();
    // Every part but the last must be a modifier; the last is the key.
    for part in &parts[..parts.len() - 1] {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "alt" | "option" => alt = true,
            "shift" => {
                return Err(format!(
                    "{spec:?}: 'shift' is not a standalone modifier — write the \
                     shifted character itself (\"S\", \"?\"); Shift+Tab is \"backtab\""
                ));
            }
            "" => return Err(format!("{spec:?}: empty modifier before '+'")),
            other => {
                return Err(format!(
                    "{spec:?}: unknown modifier {other:?} (known: ctrl, alt)"
                ));
            }
        }
    }
    let key = parts[parts.len() - 1];
    if key.is_empty() {
        return Err(format!("{spec:?}: missing key after '+'"));
    }
    let lower = key.to_ascii_lowercase();
    let code = if let Some(code) = named_key(&lower) {
        code
    } else {
        let mut chars = key.chars();
        let (c, rest) = (chars.next().expect("non-empty"), chars.next());
        if rest.is_some() {
            return Err(format!(
                "{spec:?}: {key:?} is not a named key or a single character \
                 (named keys: esc, enter, tab, backtab, backspace, delete, insert, \
                 home, end, pageup, pagedown, up, down, left, right, space, f1–f12)"
            ));
        }
        // Control chords report the lowercase letter on every terminal
        // layout this app targets ("Ctrl+R" arrives as `Char('r')` +
        // CONTROL), so the spec normalizes to match. Shifted characters
        // (`"S"`) keep their case: shift is not folded into ctrl chords.
        let c = if ctrl { c.to_ascii_lowercase() } else { c };
        KeyCode::Char(c)
    };
    Ok(KeySpec { code, ctrl, alt })
}

/// One named key: every accepted name (aliases included, matched
/// case-insensitively), the KeyCode it maps to, and the canonical display
/// form used in hints and docs. The single source of truth for both
/// directions of the mapping — parsing and display share one table, so a
/// code added to parsing is displayable with no second edit.
const NAMED_KEYS: &[(&str, KeyCode, &str)] = &[
    ("esc", KeyCode::Esc, "Esc"),
    ("escape", KeyCode::Esc, "Esc"),
    ("⎋", KeyCode::Esc, "Esc"),
    ("enter", KeyCode::Enter, "↵"),
    ("return", KeyCode::Enter, "↵"),
    ("⏎", KeyCode::Enter, "↵"),
    ("↵", KeyCode::Enter, "↵"),
    ("tab", KeyCode::Tab, "Tab"),
    ("⇥", KeyCode::Tab, "Tab"),
    ("backtab", KeyCode::BackTab, "Shift+Tab"),
    ("backspace", KeyCode::Backspace, "⌫"),
    ("⌫", KeyCode::Backspace, "⌫"),
    ("delete", KeyCode::Delete, "Del"),
    ("del", KeyCode::Delete, "Del"),
    ("insert", KeyCode::Insert, "Ins"),
    ("home", KeyCode::Home, "Home"),
    ("end", KeyCode::End, "End"),
    ("pageup", KeyCode::PageUp, "PageUp"),
    ("pgup", KeyCode::PageUp, "PageUp"),
    ("pagedown", KeyCode::PageDown, "PageDown"),
    ("pgdn", KeyCode::PageDown, "PageDown"),
    ("up", KeyCode::Up, "↑"),
    ("↑", KeyCode::Up, "↑"),
    ("down", KeyCode::Down, "↓"),
    ("↓", KeyCode::Down, "↓"),
    ("left", KeyCode::Left, "←"),
    ("←", KeyCode::Left, "←"),
    ("right", KeyCode::Right, "→"),
    ("→", KeyCode::Right, "→"),
    ("space", KeyCode::Char(' '), "Space"),
];

/// Named keys and glyph aliases, matched case-insensitively. The token
/// arrives lowercased; a single-char glyph stays as-is.
fn named_key(lower: &str) -> Option<KeyCode> {
    // Function keys: "f1"–"f12" (the token arrives lowercased, and
    // "f"? would otherwise read as a character key).
    if matches!(lower.len(), 2..=3) && lower.starts_with('f') {
        let n: u8 = lower[1..].parse().ok()?;
        if !(1..=12).contains(&n) {
            return None;
        }
        return Some(KeyCode::F(n));
    }
    NAMED_KEYS
        .iter()
        .find(|(name, _, _)| *name == lower)
        .map(|&(_, code, _)| code)
}

/// Canonical display for a code (inverse of [`NAMED_KEYS`]): arrows and
/// Enter render as the compact glyphs the docs and hints use; an
/// unlemmatized char is itself.
fn display_code(code: KeyCode) -> String {
    if let KeyCode::F(n) = code {
        return format!("F{n}");
    }
    NAMED_KEYS
        .iter()
        .find(|&(_, entry_code, _)| *entry_code == code)
        .map(|&(_, _, display)| String::from(display))
        .unwrap_or_else(|| match code {
            KeyCode::Char(c) => c.to_string(),
            other => format!("{other:?}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(spec: &str) -> KeySpec {
        parse_spec(spec).expect("valid spec")
    }

    fn err(spec: &str) -> String {
        parse_spec(spec).expect_err("invalid spec")
    }

    #[test]
    fn characters_parse_as_themselves() {
        assert_eq!(spec("j").code, KeyCode::Char('j'));
        assert_eq!(spec("?").code, KeyCode::Char('?'));
        assert_eq!(spec("]").code, KeyCode::Char(']'));
        assert_eq!(spec("S").code, KeyCode::Char('S'));
        assert_eq!(spec("→").code, KeyCode::Right);
        assert!(!spec("S").ctrl && !spec("S").alt);
    }

    #[test]
    fn named_keys_and_aliases_parse() {
        assert_eq!(spec("esc").code, KeyCode::Esc);
        assert_eq!(spec("Escape").code, KeyCode::Esc);
        assert_eq!(spec("⌫").code, KeyCode::Backspace);
        assert_eq!(spec("↵").code, KeyCode::Enter);
        assert_eq!(spec("space").code, KeyCode::Char(' '));
        assert_eq!(spec("backtab").code, KeyCode::BackTab);
        assert_eq!(spec("Shift+Tab").code, KeyCode::BackTab);
        assert_eq!(spec("f5").code, KeyCode::F(5));
        assert_eq!(spec("F12").code, KeyCode::F(12));
    }

    #[test]
    fn modifiers_parse_case_insensitively() {
        let parsed = spec("Ctrl+R");
        // Control chords normalize to the lowercase letter: that is what
        // the terminal actually delivers.
        assert_eq!(parsed.code, KeyCode::Char('r'));
        assert!(parsed.ctrl);
        let parsed = spec("control+r");
        assert!(parsed.ctrl);
        let parsed = spec("alt+enter");
        assert_eq!(parsed.code, KeyCode::Enter);
        assert!(parsed.alt);
        let parsed = spec("Ctrl+Alt+Delete");
        assert!(parsed.ctrl && parsed.alt);
        // Chord or not, a shifted character keeps its case: `Ctrl+S` and
        // the plain `S` (save attachment) are different worlds.
        assert_eq!(spec("S").code, KeyCode::Char('S'));
        assert!(!spec("S").ctrl);
    }

    #[test]
    fn invalid_specs_report_actionable_errors() {
        assert!(err("").contains("empty"));
        assert!(err("ctrl+").contains("missing key"));
        assert!(err("ctrl").contains("not a named key"));
        assert!(err("hyper+j").contains("unknown modifier"));
        assert!(err("shift+s").contains("shifted character"));
        assert!(err("shift+enter").contains("shift"));
        assert!(err("f13").contains("not a named key"));
        assert!(err("abc").contains("not a named key"));
    }

    #[test]
    fn display_round_trips() {
        for s in [
            "j",
            "S",
            "?",
            "→",
            "esc",
            "⌫",
            "Ctrl+r",
            "alt+enter",
            "space",
            "f5",
        ] {
            let parsed = spec(s);
            let shown = parsed.display();
            assert_eq!(spec(&shown), parsed, "{s:?} → {shown:?} must round-trip");
        }
        // Control chords display the (normalized) lowercase letter.
        assert_eq!(spec("ctrl+r").display(), "Ctrl+r");
        assert_eq!(spec("Ctrl+R").display(), "Ctrl+r");
        assert_eq!(spec("enter").display(), "↵");
        assert_eq!(spec("backtab").display(), "Shift+Tab");
    }
}
