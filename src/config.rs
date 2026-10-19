//! User configuration, read from `config.toml` in the config folder:
//! `$LAZYPOST_CONFIG_DIR`, else `$XDG_CONFIG_HOME/lazypost`, else `~/.config/lazypost`.
//!
//! Only application-level keys are configurable; vim motions and editing keys
//! inside text fields are fixed.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{collections::HashMap, fs, path::PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Action {
    Send,
    Cancel,
    Quit,
    Help,
    Command,
    Method,
    Env,
    Zoom,
    NextPane,
    PrevPane,
    PaneLeft,
    PaneRight,
    PaneUp,
    PaneDown,
    FocusCollection,
    FocusUrl,
    FocusRequest,
    FocusResponse,
    NextTab,
    PrevTab,
    NewRequest,
    NewFolder,
    Rename,
    Delete,
    Reload,
    AddRow,
    AddRowAbove,
    DeleteRow,
    ToggleRow,
    CopyResponse,
}

/// Every action: config name, default keys, description. Order is the order of `--print-config`.
const ACTIONS: &[(Action, &str, &[&str], &str)] = &[
    (Action::Send, "send", &["ctrl-s"], "send the request (ctrl/alt/F-keys also work while typing)"),
    (Action::Cancel, "cancel", &["ctrl-c"], "cancel a running request"),
    (Action::Quit, "quit", &["q"], "quit"),
    (Action::Help, "help", &["?"], "show the help popup"),
    (Action::Command, "command", &[":"], "open the command line"),
    (Action::Method, "method", &["m"], "choose the HTTP method"),
    (Action::Env, "env", &["E"], "edit the collection's .env variables"),
    (Action::Zoom, "zoom", &["z"], "zoom the focused pane"),
    (Action::NextPane, "next_pane", &["tab"], "focus the next pane"),
    (Action::PrevPane, "prev_pane", &["shift-tab"], "focus the previous pane"),
    (Action::PaneLeft, "pane_left", &["ctrl-h"], "focus the pane to the left"),
    (Action::PaneRight, "pane_right", &["ctrl-l"], "focus the pane to the right"),
    (Action::PaneUp, "pane_up", &["ctrl-k"], "focus the pane above"),
    (Action::PaneDown, "pane_down", &["ctrl-j"], "focus the pane below"),
    (Action::FocusCollection, "focus_collection", &["1"], "focus pane 1, the collection"),
    (Action::FocusUrl, "focus_url", &["2"], "focus pane 2, the URL"),
    (Action::FocusRequest, "focus_request", &["3"], "focus pane 3, the request"),
    (Action::FocusResponse, "focus_response", &["4"], "focus pane 4, the response"),
    (Action::NextTab, "next_tab", &["]", "L"], "next tab of the focused pane"),
    (Action::PrevTab, "prev_tab", &["[", "H"], "previous tab of the focused pane"),
    (Action::NewRequest, "new_request", &["a"], "collection: new request"),
    (Action::NewFolder, "new_folder", &["A"], "collection: new folder"),
    (Action::Rename, "rename", &["r"], "collection: rename or move"),
    (Action::Delete, "delete", &["d"], "collection: delete"),
    (Action::Reload, "reload", &["R"], "collection: reload from disk"),
    (Action::AddRow, "add_row", &["o"], "params/headers: add a row below"),
    (Action::AddRowAbove, "add_row_above", &["O"], "params/headers: add a row above"),
    (Action::DeleteRow, "delete_row", &["d"], "params/headers: delete the row"),
    (Action::ToggleRow, "toggle_row", &["space", "x"], "params/headers: enable / disable the row"),
    (Action::CopyResponse, "copy_response", &["y"], "response: copy everything to the clipboard"),
];

/// One key combination. Shift is part of the character (`A`, `?`), so only
/// ctrl and alt are stored as modifiers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Key {
    code: KeyCode,
    mods: KeyModifiers,
}

impl Key {
    fn parse(text: &str) -> Result<Key, String> {
        let mut rest = text.trim();
        let mut mods = KeyModifiers::NONE;
        let mut shift = false;
        // A lone "-" is a key, so only strip prefixes that leave something behind.
        loop {
            let lower = rest.to_ascii_lowercase();
            let (m, len) = if lower.starts_with("ctrl-") {
                (Some(KeyModifiers::CONTROL), 5)
            } else if lower.starts_with("alt-") {
                (Some(KeyModifiers::ALT), 4)
            } else if lower.starts_with("shift-") {
                (None, 6)
            } else {
                break;
            };
            if rest.len() == len {
                break;
            }
            match m {
                Some(m) => mods |= m,
                None => shift = true,
            }
            rest = &rest[len..];
        }
        let mut chars = rest.chars();
        let code = match (chars.next(), chars.next()) {
            // Terminals cannot tell ctrl-a from ctrl-shift-a.
            (Some(_), None) if shift && mods.contains(KeyModifiers::CONTROL) => {
                return Err(format!("\"{text}\": ctrl and shift cannot be combined"));
            }
            (Some(c), None) if shift => KeyCode::Char(c.to_ascii_uppercase()),
            (Some(c), None) if mods.contains(KeyModifiers::CONTROL) => KeyCode::Char(c.to_ascii_lowercase()),
            (Some(c), None) => KeyCode::Char(c),
            _ => match rest.to_ascii_lowercase().as_str() {
                "tab" if shift => KeyCode::BackTab,
                "tab" => KeyCode::Tab,
                "backtab" => KeyCode::BackTab,
                "enter" | "return" => KeyCode::Enter,
                "esc" | "escape" => KeyCode::Esc,
                "space" => KeyCode::Char(' '),
                "backspace" => KeyCode::Backspace,
                "delete" | "del" => KeyCode::Delete,
                "up" => KeyCode::Up,
                "down" => KeyCode::Down,
                "left" => KeyCode::Left,
                "right" => KeyCode::Right,
                "home" => KeyCode::Home,
                "end" => KeyCode::End,
                "pageup" => KeyCode::PageUp,
                "pagedown" => KeyCode::PageDown,
                f if f.starts_with('f') => match f[1..].parse::<u8>() {
                    Ok(n @ 1..=12) => KeyCode::F(n),
                    _ => return Err(format!("unknown key \"{text}\"")),
                },
                _ => return Err(format!("unknown key \"{text}\"")),
            },
        };
        Ok(Key { code, mods })
    }

    fn matches(&self, key: &KeyEvent) -> bool {
        let mods = key.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT);
        let code = match key.code {
            KeyCode::Char(c) if mods.contains(KeyModifiers::CONTROL) => KeyCode::Char(c.to_ascii_lowercase()),
            code => code,
        };
        code == self.code && mods == self.mods
    }

    /// Whether the key has ctrl or alt or is a function key, i.e. means nothing to a text field.
    pub fn is_chord(&self) -> bool {
        !self.mods.is_empty() || matches!(self.code, KeyCode::F(_))
    }

    /// Short label for hints, e.g. `^s`, `M-x`, `S-Tab`.
    pub fn label(&self) -> String {
        let mut s = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            s.push('^');
        }
        if self.mods.contains(KeyModifiers::ALT) {
            s.push_str("M-");
        }
        match self.code {
            KeyCode::Char(' ') => s.push('␣'),
            KeyCode::Char(c) => s.push(c),
            KeyCode::Tab => s.push_str("Tab"),
            KeyCode::BackTab => s.push_str("S-Tab"),
            KeyCode::Enter => s.push('⏎'),
            KeyCode::Esc => s.push_str("Esc"),
            KeyCode::Backspace => s.push_str("BS"),
            KeyCode::Delete => s.push_str("Del"),
            KeyCode::Up => s.push('↑'),
            KeyCode::Down => s.push('↓'),
            KeyCode::Left => s.push('←'),
            KeyCode::Right => s.push('→'),
            KeyCode::Home => s.push_str("Home"),
            KeyCode::End => s.push_str("End"),
            KeyCode::PageUp => s.push_str("PgUp"),
            KeyCode::PageDown => s.push_str("PgDn"),
            KeyCode::F(n) => s.push_str(&format!("F{n}")),
            _ => s.push('?'),
        }
        s
    }
}

pub struct Keymap {
    keys: HashMap<Action, Vec<Key>>,
}

impl Default for Keymap {
    fn default() -> Self {
        let keys = ACTIONS
            .iter()
            .map(|(a, _, defaults, _)| (*a, defaults.iter().filter_map(|k| Key::parse(k).ok()).collect()))
            .collect();
        Keymap { keys }
    }
}

impl Keymap {
    /// The first of `actions` bound to `key`.
    pub fn find(&self, key: &KeyEvent, actions: &[Action]) -> Option<Action> {
        actions.iter().copied().find(|a| self.is(*a, key))
    }

    pub fn is(&self, action: Action, key: &KeyEvent) -> bool {
        self.keys.get(&action).is_some_and(|ks| ks.iter().any(|k| k.matches(key)))
    }

    /// Like [`Keymap::is`], but only for chords, which are safe to catch while typing.
    pub fn is_chord(&self, action: Action, key: &KeyEvent) -> bool {
        self.keys.get(&action).is_some_and(|ks| ks.iter().any(|k| k.is_chord() && k.matches(key)))
    }

    /// The keys bound to `action`, e.g. `]/L`; empty if unbound.
    pub fn label(&self, action: Action) -> String {
        let keys = self.keys.get(&action).map(Vec::as_slice).unwrap_or_default();
        keys.iter().map(Key::label).collect::<Vec<_>>().join("/")
    }
}

pub struct Config {
    pub keys: Keymap,
}

pub fn dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("LAZYPOST_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("lazypost")
}

pub fn path() -> PathBuf {
    dir().join("config.toml")
}

/// Load the config file. Problems are returned as warnings and the defaults
/// are kept for anything that could not be read.
pub fn load() -> (Config, Vec<String>) {
    let path = path();
    match fs::read_to_string(&path) {
        Ok(text) => {
            let (config, warnings) = parse(&text);
            (config, warnings.into_iter().map(|w| format!("{}: {w}", path.display())).collect())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Config { keys: Keymap::default() }, Vec::new()),
        Err(e) => (Config { keys: Keymap::default() }, vec![format!("{}: {e}", path.display())]),
    }
}

/// Parse a config file, keeping the defaults for anything that is wrong.
fn parse(text: &str) -> (Config, Vec<String>) {
    let mut keys = Keymap::default();
    let mut warnings = Vec::new();
    let table = match text.parse::<toml::Table>() {
        Ok(t) => t,
        Err(e) => return (Config { keys }, vec![e.message().to_string()]),
    };
    for (section, value) in table {
        if section != "keys" {
            warnings.push(format!("unknown section [{section}]"));
            continue;
        }
        let Some(bindings) = value.as_table() else {
            warnings.push("[keys] must be a table".into());
            continue;
        };
        for (name, value) in bindings {
            let Some((action, ..)) = ACTIONS.iter().find(|(_, n, ..)| n == name) else {
                warnings.push(format!("unknown action \"{name}\""));
                continue;
            };
            let list: Vec<&toml::Value> = match value {
                toml::Value::Array(items) => items.iter().collect(),
                v => vec![v],
            };
            let mut parsed = Vec::new();
            for v in list {
                match v.as_str().map(Key::parse) {
                    Some(Ok(key)) => parsed.push(key),
                    Some(Err(e)) => warnings.push(format!("{name}: {e}")),
                    None => warnings.push(format!("{name}: keys must be strings, got {v}")),
                }
            }
            keys.keys.insert(*action, parsed);
        }
    }
    (Config { keys }, warnings)
}

/// A config file with every action at its default, for `--print-config`.
pub fn default_file() -> String {
    let mut s = String::from(
        "# lazypost configuration — save as config.toml in the config folder\n\
         # (see `lazypost --help`). Each action takes one key or a list of keys,\n\
         # e.g. \"ctrl-s\", \"alt-x\", \"shift-tab\", \"space\", \"enter\", \"F5\", \"?\".\n\
         # An empty list unbinds an action. Vim keys inside text fields are not configurable.\n\n\
         [keys]\n",
    );
    let width = ACTIONS.iter().map(|(_, n, ..)| n.len()).max().unwrap_or(0);
    for (_, name, defaults, desc) in ACTIONS {
        let keys = if defaults.len() == 1 {
            format!("{:?}", defaults[0])
        } else {
            format!("[{}]", defaults.iter().map(|k| format!("{k:?}")).collect::<Vec<_>>().join(", "))
        };
        s.push_str(&format!("{name:<width$} = {keys:<16} # {desc}\n"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn parse_keys() {
        let k = |s| Key::parse(s).unwrap();
        assert!(k("ctrl-s").matches(&ev(KeyCode::Char('s'), KeyModifiers::CONTROL)));
        assert!(k("Ctrl-S").matches(&ev(KeyCode::Char('s'), KeyModifiers::CONTROL)));
        assert!(k("A").matches(&ev(KeyCode::Char('A'), KeyModifiers::SHIFT)));
        assert!(k("shift-a").matches(&ev(KeyCode::Char('A'), KeyModifiers::SHIFT)));
        assert!(k("shift-tab").matches(&ev(KeyCode::BackTab, KeyModifiers::SHIFT)));
        assert!(k("space").matches(&ev(KeyCode::Char(' '), KeyModifiers::NONE)));
        assert!(k("-").matches(&ev(KeyCode::Char('-'), KeyModifiers::NONE)));
        assert!(k("alt--").matches(&ev(KeyCode::Char('-'), KeyModifiers::ALT)));
        assert!(k("F5").matches(&ev(KeyCode::F(5), KeyModifiers::NONE)));
        assert!(!k("s").matches(&ev(KeyCode::Char('s'), KeyModifiers::CONTROL)));
        assert!(Key::parse("ctrl-nope").is_err());
        assert!(Key::parse("ctrl-shift-a").is_err());
        assert_eq!(k("ctrl-s").label(), "^s");
    }

    #[test]
    fn config_overrides() {
        let text =
            "[keys]\nsend = [\"F5\", \"ctrl-r\"]\nquit = []\nbogus = \"x\"\nzoom = \"ctrl-?x\"\nmethod = 1\n[oops]\n";
        let (cfg, warnings) = parse(text);
        assert_eq!(warnings.len(), 4, "{warnings:?}");
        assert!(cfg.keys.is(Action::Send, &ev(KeyCode::F(5), KeyModifiers::NONE)));
        assert!(!cfg.keys.is(Action::Send, &ev(KeyCode::Char('s'), KeyModifiers::CONTROL)));
        assert!(!cfg.keys.is(Action::Quit, &ev(KeyCode::Char('q'), KeyModifiers::NONE)));
        assert!(cfg.keys.is(Action::Help, &ev(KeyCode::Char('?'), KeyModifiers::NONE)));
    }

    #[test]
    fn default_file_round_trips() {
        let (_, warnings) = parse(&default_file());
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}
