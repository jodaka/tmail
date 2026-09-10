//! The keymap: configurable key bindings as data (configurable-keybind
//! research, ticket 00y7; `[tmail.keybindings]`).
//!
//! Bindings live in per-context tables (`global`, `list`, `reader` — the
//! context column of `docs/shortcusts.md`). [`DEFAULT_GLOBAL`] and friends
//! reproduce the previously hardcoded `keyboard::to_action` behavior
//! exactly; the config replaces any action's key list (an **empty array
//! unbinds** the action). The map is the single source of truth: the
//! translation layer looks keys up here, and the status bar renders its
//! hints from it, so rebinding can never make the UI lie.
//!
//! Build-time policy (decided with the user):
//! - **Conflicts are refused**: when a configured key is already bound to
//!   a different action in the same context, the earlier binding (default
//!   or alphabetically-earlier override) stays, the new key is dropped,
//!   and a warning is reported at startup. The app still runs.
//! - **Structural actions stay reachable**: `cancel`, `activate`,
//!   `focus_next`, `focus_previous`, and `quit` must end up with at least
//!   one binding; emptied (or conflict-stripped) ones restore their
//!   defaults with a warning. Everything else is freely rebindable,
//!   including arrows and `Esc` itself.
//! - Unknown action names, unknown contexts, and unparseable key specs
//!   are *fatal* config problems (they come back as errors and the app
//!   refuses to start, like unknown theme tokens).

use std::collections::HashMap;

use crossterm::event::KeyEvent;

use crate::app::action::Action;
use crate::app::focus::Focus;
use crate::config::KeybindingTable;
use crate::input::keyspec::{KeySpec, parse_spec};

/// Which binding table a focus consults (on top of the global one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    /// Message-list rows.
    List,
    /// The open-message reader.
    Reader,
    /// Every other shortcut-accepting focus (sidebar, modals, picker).
    Global,
}

/// The context tables a focus consults, most specific first.
fn contexts_for_focus(focus: Focus) -> [Option<Context>; 2] {
    match focus {
        Focus::MessageList => [Some(Context::List), Some(Context::Global)],
        Focus::Reader => [Some(Context::Reader), Some(Context::Global)],
        _ => [None, Some(Context::Global)],
    }
}

/// One default binding: action name, its key specs, and the action the
/// reducer receives. Names are the config surface (`[tmail.keybindings.
/// <context>].<name>`) and mirror the wording of `docs/shortcusts.md`.
struct DefaultBinding {
    name: &'static str,
    keys: &'static [&'static str],
    action: Action,
}

macro_rules! binding {
    ($name:literal, $action:expr, [$($key:literal),* $(,)?]) => {
        DefaultBinding { name: $name, keys: &[$($key),*], action: $action }
    };
}

/// Keys that fire in every shortcut-accepting focus. Reproduces the
/// hardcoded table exactly (including `q` mirroring `Esc` and `S`/`o`
/// being reachable outside the reader, as today).
const DEFAULT_GLOBAL: &[DefaultBinding] = &[
    binding!("move_up", Action::MoveUp, ["↑"]),
    binding!("move_down", Action::MoveDown, ["↓"]),
    binding!("previous_page", Action::PagePrevious, ["←"]),
    binding!("next_page", Action::PageNext, ["→"]),
    binding!("activate", Action::Activate, ["↵"]),
    binding!("cancel", Action::BackOrCancel, ["Esc", "q"]),
    binding!("focus_next", Action::FocusNext, ["Tab"]),
    binding!("focus_previous", Action::FocusPrevious, ["Shift+Tab"]),
    binding!("open_search", Action::OpenSearch, ["/"]),
    binding!("compose", Action::Compose, ["c"]),
    binding!("refresh", Action::Refresh, ["Ctrl+R"]),
    binding!("quit", Action::Quit, ["Ctrl+C"]),
    binding!("select_all", Action::SelectAll, ["Ctrl+A"]),
    binding!("toggle_mouse", Action::ToggleMouseCapture, ["m"]),
    binding!("theme", Action::OpenThemePicker, ["t"]),
    binding!("reply", Action::Reply, ["r"]),
    binding!("reply_all", Action::ReplyAll, ["a"]),
    binding!("forward", Action::Forward, ["f"]),
    binding!("archive", Action::Archive, ["e"]),
    binding!("star", Action::ToggleStar, ["s"]),
    binding!("mark_unread", Action::MarkUnread, ["u"]),
    binding!("mark_read", Action::MarkRead, ["i"]),
    binding!("save_attachment", Action::SaveAttachment, ["S"]),
    binding!("open_attachment", Action::OpenAttachment, ["o"]),
];

/// Message-list-only keys (`d` first: the compact form is what the hint
/// row shows; `Space` marks the focused row — the sidebar has no selection
/// semantics, and the reader scrolls instead, ticket p0s3).
const DEFAULT_LIST: &[DefaultBinding] = &[
    binding!("trash", Action::Trash, ["d", "Delete"]),
    binding!("toggle_selected", Action::ToggleSelected, ["Space"]),
];

/// Reader-only keys: `⌫` trashes the open message (ticket zg41).
const DEFAULT_READER: &[DefaultBinding] = &[binding!("trash", Action::Trash, ["d", "Delete", "⌫"])];

/// Actions that must keep at least one binding: the escape hatches.
const STRUCTURAL_ACTIONS: &[&str] = &["cancel", "activate", "focus_next", "focus_previous", "quit"];

/// Every bindable action name → the action it dispatches. The default
/// tables are the single source of truth: each `binding!` carries the
/// config name and the action on one line, and both lookups walk the
/// tables, so a name can never drift away from the action it dispatches.
/// The defaults above and the config both address actions through these
/// names.
fn action_by_name(name: &str) -> Option<Action> {
    named_default_binding(name).map(|binding| binding.action.clone())
}

/// The config-facing name of an action (reverse of [`action_by_name`]),
/// used to describe binding owners in warnings. `None` never happens for
/// actions this module created, but keeps the helper total: data-carrying
/// actions (`SearchEdit`, `BackendCompleted`, …) bind no names.
fn action_name_of(action: &Action) -> Option<&'static str> {
    named_default_binding_by_action(action).map(|binding| binding.name)
}

/// The default binding with this config name, from the built-in tables.
fn named_default_binding(name: &str) -> Option<&'static DefaultBinding> {
    DEFAULT_GLOBAL
        .iter()
        .chain(DEFAULT_LIST.iter())
        .chain(DEFAULT_READER.iter())
        .find(|binding| binding.name == name)
}

/// The default binding that dispatches to `action`, from the built-in
/// tables.
fn named_default_binding_by_action(action: &Action) -> Option<&'static DefaultBinding> {
    DEFAULT_GLOBAL
        .iter()
        .chain(DEFAULT_LIST.iter())
        .chain(DEFAULT_READER.iter())
        .find(|binding| &binding.action == action)
}

/// One context's bindings: key → action, plus action → keys (for
/// replacement and the structural check) and action → display (hints).
#[derive(Debug, Clone, Default)]
struct ContextTable {
    by_key: HashMap<KeySpec, Action>,
    by_action: HashMap<String, Vec<KeySpec>>,
    display: HashMap<String, String>,
}

impl ContextTable {
    fn from_defaults(defaults: &[DefaultBinding], errors: &mut Vec<String>) -> Self {
        let mut table = Self::default();
        for binding in defaults {
            let specs = binding
                .keys
                .iter()
                .map(|spec| match parse_spec(spec) {
                    Ok(parsed) => Some(parsed),
                    Err(err) => {
                        // Defaults are compile-time constants; a typo here
                        // is a programming error, surfaced loudly in tests
                        // and at startup.
                        let name = binding.name;
                        errors.push(format!("built-in binding {name}: {err}"));
                        None
                    }
                })
                .collect::<Vec<_>>();
            table.set_action(
                binding.name,
                binding.action.clone(),
                specs.into_iter().flatten(),
            );
        }
        table
    }

    /// Replace one action's bindings (empty `specs` unbinds it).
    fn set_action(&mut self, name: &str, action: Action, specs: impl IntoIterator<Item = KeySpec>) {
        if let Some(old) = self.by_action.remove(name) {
            for spec in old {
                self.by_key.remove(&spec);
            }
        }
        let specs: Vec<KeySpec> = specs.into_iter().collect();
        for spec in &specs {
            self.by_key.insert(*spec, action.clone());
        }
        if let Some(first) = specs.first() {
            self.display.insert(String::from(name), first.display());
        } else {
            self.display.remove(name);
        }
        if !specs.is_empty() {
            self.by_action.insert(String::from(name), specs);
        }
    }

    /// The keys bound to `name`, if any.
    fn keys_of(&self, name: &str) -> Option<&Vec<KeySpec>> {
        self.by_action.get(name)
    }
}

/// The complete binding table. Built once at startup, stored on
/// [`AppState`](crate::app::state::AppState), consulted by the translation
/// layer and the status-bar hints.
#[derive(Debug, Clone)]
pub struct KeyMap {
    global: ContextTable,
    list: ContextTable,
    reader: ContextTable,
}

/// A keymap that could not fully honor the config: fatal problems (the
/// app refuses to start) and policy warnings (the app runs with the
/// reported binding dropped or restored).
pub struct KeymapBuild {
    pub keymap: KeyMap,
    /// Fatal: unknown action/context, unparseable key spec.
    pub errors: Vec<String>,
    /// Non-fatal: refused conflicts, restored structural defaults.
    pub warnings: Vec<String>,
}

impl KeyMap {
    /// The built-in defaults (no config). Used for `AppState::initial`,
    /// tests, and as the seed for [`KeyMap::build`].
    pub fn defaults() -> Self {
        // The defaults are constants; parse failures are programming
        // errors and panic here where no config is involved.
        let mut errors = Vec::new();
        let keymap = Self {
            global: ContextTable::from_defaults(DEFAULT_GLOBAL, &mut errors),
            list: ContextTable::from_defaults(DEFAULT_LIST, &mut errors),
            reader: ContextTable::from_defaults(DEFAULT_READER, &mut errors),
        };
        assert!(
            errors.is_empty(),
            "built-in key specs must parse: {errors:?}"
        );
        keymap
    }

    /// Build the keymap from the parsed `[tmail.keybindings]` tables.
    pub fn build(tables: &[KeybindingTable]) -> KeymapBuild {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut keymap = Self::defaults();
        for table in tables {
            // Resolve the context, but validate the entries regardless so
            // one broken context still reports all of its problems.
            let mut context = match table.context.as_str() {
                "global" => Some(&mut keymap.global),
                "list" => Some(&mut keymap.list),
                "reader" => Some(&mut keymap.reader),
                other => {
                    errors.push(format!(
                        "[tmail.keybindings.{other}] is unknown (known contexts: \
                         global, list, reader)"
                    ));
                    None
                }
            };
            for (action_name, key_specs) in &table.entries {
                let Some(action) = action_by_name(action_name) else {
                    errors.push(format!(
                        "[tmail.keybindings.{}].{action_name} is not a bindable action \
                         (see config.example.toml for the list)",
                        table.context
                    ));
                    continue;
                };
                let mut parsed = Vec::new();
                for spec in key_specs {
                    match parse_spec(spec) {
                        Ok(parsed_spec) => parsed.push(parsed_spec),
                        Err(err) => errors.push(format!(
                            "[tmail.keybindings.{}].{action_name}: {err}",
                            table.context
                        )),
                    }
                }
                let Some(context) = context.as_deref_mut() else {
                    continue;
                };
                // Conflict policy: the earlier binding (default or
                // alphabetically-earlier override) keeps the key; the
                // conflicting one is dropped with a warning.
                let mut accepted = Vec::new();
                for spec in parsed {
                    let owner = context.by_key.get(&spec).and_then(action_name_of);
                    match owner {
                        Some(existing) if existing != action_name.as_str() => {
                            warnings.push(format!(
                                "[tmail.keybindings.{}]: {} is already bound to \
                                 {existing:?}; the {action_name} binding was ignored",
                                table.context,
                                spec.display(),
                            ));
                            continue;
                        }
                        _ => accepted.push(spec),
                    }
                }
                context.set_action(action_name, action, accepted);
            }
        }
        keymap.enforce_structural(&mut warnings);
        KeymapBuild {
            keymap,
            errors,
            warnings,
        }
    }

    /// The escape hatches must stay reachable: any structural action left
    /// with no binding (emptied in config, or stripped by a conflict)
    /// restores its defaults with a warning.
    fn enforce_structural(&mut self, warnings: &mut Vec<String>) {
        for name in STRUCTURAL_ACTIONS {
            if self
                .global
                .keys_of(name)
                .is_some_and(|keys| !keys.is_empty())
            {
                continue;
            }
            let defaults = DEFAULT_GLOBAL
                .iter()
                .find(|binding| binding.name == *name)
                .expect("structural action has defaults");
            let specs: Vec<KeySpec> = defaults
                .keys
                .iter()
                .map(|spec| parse_spec(spec).expect("built-in spec parses"))
                .collect();
            let shown: Vec<String> = specs.iter().map(KeySpec::display).collect();
            warnings.push(format!(
                "[tmail.keybindings.global]: action {name:?} must keep at least one \
                 binding; its defaults were restored ({})",
                shown.join(", ")
            ));
            self.global.set_action(name, defaults.action.clone(), specs);
        }
    }

    /// The action a pressed key dispatches, consulting the focus's own
    /// context table first, then the global one.
    pub fn lookup(&self, key: &KeyEvent, focus: Focus) -> Option<Action> {
        let spec = KeySpec::from_event(key);
        for context in contexts_for_focus(focus) {
            let table = match context {
                Some(Context::List) => &self.list,
                Some(Context::Reader) => &self.reader,
                Some(Context::Global) | None => &self.global,
            };
            if let Some(action) = table.by_key.get(&spec) {
                return Some(action.clone());
            }
        }
        None
    }

    /// The display form of an action's first binding, for hint rows:
    /// the screen's own context first, then the global one.
    pub fn hint(&self, context: Option<Context>, action: &str) -> Option<&str> {
        let table = match context {
            Some(Context::List) => Some(&self.list),
            Some(Context::Reader) => Some(&self.reader),
            Some(Context::Global) | None => None,
        };
        table
            .and_then(|t| t.display.get(action))
            .or_else(|| self.global.display.get(action))
            .map(String::as_str)
    }

    /// Display forms of `move_up`/`move_down` joined for the compact
    /// scroll/move hint (`↑/↓`), when both are bound.
    pub fn move_hint(&self, context: Option<Context>) -> Option<String> {
        let up = self.hint(context, "move_up")?;
        let down = self.hint(context, "move_down")?;
        Some(format!("{up}/{down}"))
    }
}

/// A default table must never bind one key to two actions, and every
/// default must parse; this test walks the constants so drift fails here
/// instead of at a user's startup.
#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn default_tables_are_conflict_free_and_parse() {
        for (context, defaults) in [
            ("global", DEFAULT_GLOBAL),
            ("list", DEFAULT_LIST),
            ("reader", DEFAULT_READER),
        ] {
            let mut seen: HashMap<String, &str> = HashMap::new();
            let mut errors = Vec::new();
            let table = ContextTable::from_defaults(defaults, &mut errors);
            assert!(errors.is_empty(), "{context}: {errors:?}");
            for (spec, action) in &table.by_key {
                let owner = action_name_of(action).expect("default actions are named");
                if let Some(previous) = seen.get(&spec.display()) {
                    panic!("{context}: {spec:?} bound to both {previous} and {owner}");
                }
                seen.insert(spec.display(), owner);
            }
        }
    }

    #[test]
    fn every_default_action_is_reachable_by_name() {
        for binding in DEFAULT_GLOBAL
            .iter()
            .chain(DEFAULT_LIST.iter())
            .chain(DEFAULT_READER.iter())
        {
            assert_eq!(
                action_by_name(binding.name),
                Some(binding.action.clone()),
                "{} must map to its own action",
                binding.name
            );
        }
    }

    #[test]
    fn defaults_reproduce_the_documented_keys() {
        let keymap = KeyMap::defaults();
        let key = |code, ctrl: bool, alt: bool| {
            let mut modifiers = KeyModifiers::empty();
            if ctrl {
                modifiers.insert(KeyModifiers::CONTROL);
            }
            if alt {
                modifiers.insert(KeyModifiers::ALT);
            }
            KeyEvent::new(code, modifiers)
        };
        use KeyCode::*;
        let list = Focus::MessageList;
        assert_eq!(
            keymap.lookup(&key(Char('c'), true, false), list),
            Some(Action::Quit)
        );
        assert_eq!(
            keymap.lookup(&key(Char('r'), true, false), list),
            Some(Action::Refresh)
        );
        assert_eq!(
            keymap.lookup(&key(Char('c'), false, false), list),
            Some(Action::Compose)
        );
        assert_eq!(
            keymap.lookup(&key(Char('q'), false, false), list),
            Some(Action::BackOrCancel)
        );
        assert_eq!(
            keymap.lookup(&key(Esc, false, false), list),
            Some(Action::BackOrCancel)
        );
        assert_eq!(
            keymap.lookup(&key(Char('d'), false, false), list),
            Some(Action::Trash)
        );
        assert_eq!(
            keymap.lookup(&key(Char(' '), false, false), list),
            Some(Action::ToggleSelected)
        );
        // The reader trashes with backspace too (ticket zg41).
        assert_eq!(
            keymap.lookup(&key(Backspace, false, false), Focus::Reader),
            Some(Action::Trash)
        );
        // ...but the list does not.
        assert_ne!(
            keymap.lookup(&key(Backspace, false, false), list),
            Some(Action::Trash)
        );
        // Uppercase S stays its own binding, distinct from lowercase s.
        assert_eq!(
            keymap.lookup(&key(Char('S'), false, false), Focus::Reader),
            Some(Action::SaveAttachment)
        );
        assert_eq!(
            keymap.lookup(&key(Char('s'), false, false), Focus::Reader),
            Some(Action::ToggleStar)
        );
        // Ctrl/alt never fire plain char bindings.
        assert_eq!(
            keymap.lookup(&key(Char('c'), true, false), list),
            Some(Action::Quit)
        );
        assert_ne!(
            keymap.lookup(&key(Char('d'), true, false), list),
            Some(Action::Trash)
        );
    }

    #[test]
    fn overrides_replace_and_empty_arrays_unbind() {
        let tables = vec![KeybindingTable {
            context: String::from("global"),
            entries: vec![
                (
                    String::from("next_page"),
                    vec![String::from("→"), String::from("Ctrl+]")],
                ),
                (String::from("toggle_mouse"), vec![]),
            ],
        }];
        let built = KeyMap::build(&tables);
        assert!(built.errors.is_empty(), "{:?}", built.errors);
        let keymap = built.keymap;
        let plain = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let ctrl = |code| KeyEvent::new(code, KeyModifiers::CONTROL);
        // The user example: → and Ctrl+] both page next.
        assert_eq!(
            keymap.lookup(&plain(KeyCode::Right), Focus::MessageList),
            Some(Action::PageNext)
        );
        assert_eq!(
            keymap.lookup(&ctrl(KeyCode::Char(']')), Focus::MessageList),
            Some(Action::PageNext)
        );
        // Empty array: the action is unreachable…
        assert_eq!(
            keymap.lookup(&plain(KeyCode::Char('m')), Focus::MessageList),
            None
        );
        // …and the hint disappears with it.
        assert_eq!(keymap.hint(Some(Context::Global), "toggle_mouse"), None);
    }

    #[test]
    fn a_conflicting_binding_is_refused_with_a_warning() {
        let tables = vec![KeybindingTable {
            context: String::from("global"),
            entries: vec![
                // `r` already belongs to reply: compose keeps its own keys
                // and the `r` claim is dropped.
                (
                    String::from("compose"),
                    vec![String::from("x"), String::from("r")],
                ),
            ],
        }];
        let built = KeyMap::build(&tables);
        assert!(built.errors.is_empty());
        assert_eq!(built.warnings.len(), 1, "{:?}", built.warnings);
        assert!(built.warnings[0].contains("already bound"));
        let keymap = built.keymap;
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        assert_eq!(
            keymap.lookup(&key('r'), Focus::MessageList),
            Some(Action::Reply)
        );
        assert_eq!(
            keymap.lookup(&key('x'), Focus::MessageList),
            Some(Action::Compose)
        );
        assert_eq!(keymap.lookup(&key('c'), Focus::MessageList), None);
    }

    #[test]
    fn structural_actions_restore_defaults_when_emptied() {
        let tables = vec![KeybindingTable {
            context: String::from("global"),
            entries: vec![(String::from("cancel"), vec![])],
        }];
        let built = KeyMap::build(&tables);
        assert!(built.errors.is_empty());
        assert!(
            built
                .warnings
                .iter()
                .any(|w| w.contains("cancel") && w.contains("restored")),
            "{:?}",
            built.warnings
        );
        // Esc and q still cancel.
        assert_eq!(
            built.keymap.lookup(
                &KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                Focus::MessageList
            ),
            Some(Action::BackOrCancel)
        );
        assert_eq!(
            built.keymap.lookup(
                &KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
                Focus::MessageList
            ),
            Some(Action::BackOrCancel)
        );
    }

    #[test]
    fn structural_actions_survive_rebinding_to_new_keys() {
        let tables = vec![KeybindingTable {
            context: String::from("global"),
            entries: vec![(String::from("cancel"), vec![String::from("Ctrl+Q")])],
        }];
        let built = KeyMap::build(&tables);
        assert!(built.errors.is_empty() && built.warnings.is_empty());
        let keymap = built.keymap;
        assert_eq!(
            keymap.lookup(
                &KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
                Focus::MessageList
            ),
            Some(Action::BackOrCancel)
        );
        // The old keys are gone — replacement semantics.
        assert_eq!(
            keymap.lookup(
                &KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                Focus::MessageList
            ),
            None
        );
    }

    #[test]
    fn unknown_actions_contexts_and_specs_are_fatal() {
        let tables = vec![KeybindingTable {
            context: String::from("sidebar"),
            entries: vec![
                (String::from("teleport"), vec![String::from("x")]),
                (String::from("compose"), vec![String::from("not+a+key")]),
            ],
        }];
        let built = KeyMap::build(&tables);
        assert_eq!(built.errors.len(), 3, "{:?}", built.errors);
        assert!(built.errors.iter().any(|e| e.contains("unknown")));
        assert!(built.errors.iter().any(|e| e.contains("teleport")));
        assert!(built.errors.iter().any(|e| e.contains("not+a+key")));
    }

    #[test]
    fn a_context_binding_shadows_the_global_one() {
        let tables = vec![KeybindingTable {
            context: String::from("reader"),
            entries: vec![(String::from("star"), vec![String::from("x")])],
        }];
        let built = KeyMap::build(&tables);
        assert!(built.errors.is_empty(), "{:?}", built.errors);
        let keymap = built.keymap;
        let s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        // Same key, different meanings per context — and no conflict,
        // because the tables are independent.
        assert_eq!(
            keymap.lookup(
                &KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
                Focus::Reader
            ),
            Some(Action::ToggleStar)
        );
        assert_eq!(keymap.lookup(&s, Focus::Reader), Some(Action::ToggleStar));
        assert_eq!(
            keymap.lookup(&s, Focus::MessageList),
            Some(Action::ToggleStar)
        );
    }

    #[test]
    fn a_parsed_config_reaches_the_keymap() {
        // End-to-end: config text → parser → keymap → translation.
        let (config, issues) = crate::config::parse_with_issues(
            "[tmail.keybindings.global]\nnext_page = [\"→\", \"Ctrl+]\"]\n",
            None,
        );
        assert!(issues.is_empty(), "{issues:?}");
        let built = KeyMap::build(&config.keybindings);
        assert!(built.errors.is_empty(), "{:?}", built.errors);
        let ctrl = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL);
        assert_eq!(
            built
                .keymap
                .lookup(&ctrl, crate::app::focus::Focus::MessageList),
            Some(Action::PageNext)
        );
    }

    #[test]
    fn hints_follow_the_bindings() {
        let tables = vec![KeybindingTable {
            context: String::from("reader"),
            entries: vec![(String::from("trash"), vec![String::from("Ctrl+D")])],
        }];
        let built = KeyMap::build(&tables);
        let keymap = built.keymap;
        assert_eq!(keymap.hint(Some(Context::Reader), "trash"), Some("Ctrl+d"));
        // The list keeps its default `d`.
        assert_eq!(keymap.hint(Some(Context::List), "trash"), Some("d"));
        // Global fallback for actions the context does not override.
        assert_eq!(keymap.hint(Some(Context::Reader), "reply"), Some("r"));
        assert_eq!(
            keymap.move_hint(Some(Context::List)),
            Some(String::from("↑/↓"))
        );
    }
}
