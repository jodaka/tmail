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
//! Matching semantics live in [`KeySpec::matches`]: character bindings
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

    /// Whether a pressed key matches this spec (see module docs for the
    /// exact modifier rules).
    pub fn matches(&self, code: KeyCode, ctrl: bool, alt: bool) -> bool {
        if code != self.code {
            return false;
        }
        self.ctrl == ctrl && self.alt == alt
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

/// Named keys and glyph aliases, matched case-insensitively.
fn named_key(lower: &str) -> Option<KeyCode> {
    let code = match lower {
        "esc" | "escape" | "⎋" => KeyCode::Esc,
        "enter" | "return" | "↵" | "⏎" => KeyCode::Enter,
        "tab" | "⇥" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "backspace" | "⌫" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        "up" | "↑" => KeyCode::Up,
        "down" | "↓" => KeyCode::Down,
        "left" | "←" => KeyCode::Left,
        "right" | "→" => KeyCode::Right,
        "space" => KeyCode::Char(' '),
        // Function keys: "f1"–"f12" (the token arrives lowercased).
        f if matches!(f.len(), 2..=3) && f.starts_with('f') => {
            let n: u8 = f[1..].parse().ok()?;
            let n = if (1..=12).contains(&n) {
                n
            } else {
                return None;
            };
            function_key(n)
        }
        _ => return None,
    };
    Some(code)
}

fn function_key(n: u8) -> KeyCode {
    match n {
        1 => KeyCode::F(1),
        2 => KeyCode::F(2),
        3 => KeyCode::F(3),
        4 => KeyCode::F(4),
        5 => KeyCode::F(5),
        6 => KeyCode::F(6),
        7 => KeyCode::F(7),
        8 => KeyCode::F(8),
        9 => KeyCode::F(9),
        10 => KeyCode::F(10),
        11 => KeyCode::F(11),
        _ => KeyCode::F(12),
    }
}

/// Canonical display for a code (inverse of the named/glyph set): arrows
/// and Enter render as the compact glyphs the docs and hints use.
fn display_code(code: KeyCode) -> String {
    match code {
        KeyCode::Esc => String::from("Esc"),
        KeyCode::Enter => String::from("↵"),
        KeyCode::Tab => String::from("Tab"),
        KeyCode::BackTab => String::from("Shift+Tab"),
        KeyCode::Backspace => String::from("⌫"),
        KeyCode::Delete => String::from("Del"),
        KeyCode::Insert => String::from("Ins"),
        KeyCode::Home => String::from("Home"),
        KeyCode::End => String::from("End"),
        KeyCode::PageUp => String::from("PageUp"),
        KeyCode::PageDown => String::from("PageDown"),
        KeyCode::Up => String::from("↑"),
        KeyCode::Down => String::from("↓"),
        KeyCode::Left => String::from("←"),
        KeyCode::Right => String::from("→"),
        KeyCode::F(n) => format!("F{n}"),
        KeyCode::Char(' ') => String::from("Space"),
        KeyCode::Char(c) => c.to_string(),
        other => format!("{other:?}"),
    }
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

    #[test]
    fn matching_honors_the_modifier_rules() {
        let plain_c = spec("c");
        // Plain char bindings ignore SHIFT but reject ctrl/alt.
        assert!(plain_c.matches(KeyCode::Char('c'), false, false));
        assert!(!plain_c.matches(KeyCode::Char('c'), true, false));
        assert!(!plain_c.matches(KeyCode::Char('c'), false, true));
        // A different character never matches.
        assert!(!plain_c.matches(KeyCode::Char('d'), false, false));
        let ctrl_r = spec("ctrl+r");
        assert!(ctrl_r.matches(KeyCode::Char('r'), true, false));
        // Chords match exactly: alt never rides along.
        assert!(!ctrl_r.matches(KeyCode::Char('r'), true, true));
        assert!(!ctrl_r.matches(KeyCode::Char('r'), false, false));
        let esc = spec("esc");
        assert!(esc.matches(KeyCode::Esc, false, false));
        assert!(!esc.matches(KeyCode::Esc, true, false));
    }
}
