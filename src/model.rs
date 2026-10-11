//! Requests and the on-disk collection (a directory tree of `.toml` files).

use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

pub const METHODS: [(&str, char); 7] = [
    ("GET", 'g'),
    ("POST", 'p'),
    ("PUT", 'u'),
    ("PATCH", 'a'),
    ("DELETE", 'd'),
    ("HEAD", 'h'),
    ("OPTIONS", 'o'),
];

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct KV {
    pub key: String,
    pub value: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

impl Default for KV {
    fn default() -> Self {
        Self { key: String::new(), value: String::new(), enabled: true }
    }
}

impl KV {
    pub fn new(key: &str, value: &str) -> Self {
        Self { key: key.into(), value: value.into(), enabled: true }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
#[serde(rename_all = "lowercase")]
pub enum AuthKind {
    #[default]
    None,
    Basic,
    Bearer,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Default, Debug)]
#[serde(default)]
pub struct Auth {
    pub kind: AuthKind,
    pub username: String,
    pub password: String,
    pub token: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub body: String,
    #[serde(skip_serializing_if = "is_default_auth")]
    pub auth: Auth,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<KV>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<KV>,
}

fn is_default_auth(a: &Auth) -> bool {
    *a == Auth::default()
}

impl Default for Request {
    fn default() -> Self {
        Self {
            method: "GET".into(),
            url: String::new(),
            body: String::new(),
            auth: Auth::default(),
            params: Vec::new(),
            headers: Vec::new(),
        }
    }
}

pub fn load(path: &Path) -> Result<Request, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    toml::from_str(&text).map_err(|e| e.message().to_string())
}

pub fn save(path: &Path, req: &Request) -> Result<(), String> {
    let text = toml::to_string_pretty(req).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(path, text).map_err(|e| e.to_string())
}

#[derive(Clone, Debug)]
pub struct Node {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    pub method: Option<String>,
}

/// Flatten the collection directory into the visible tree rows.
pub fn scan(root: &Path, collapsed: &HashSet<PathBuf>) -> Vec<Node> {
    let mut out = Vec::new();
    walk(root, 0, collapsed, &mut out);
    out
}

fn walk(dir: &Path, depth: usize, collapsed: &HashSet<PathBuf>, out: &mut Vec<Node>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<(bool, String, PathBuf)> = rd
        .flatten()
        .map(|e| e.path())
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy().to_string();
            let is_dir = p.is_dir();
            let keep = !name.starts_with('.') && (is_dir || name.ends_with(".toml"));
            keep.then_some((!is_dir, name, p))
        })
        .collect();
    entries.sort_by_key(|(file, name, _)| (*file, name.to_lowercase()));
    for (file, name, path) in entries {
        if file {
            let method = load(&path).ok().map(|r| r.method);
            let name = name.trim_end_matches(".toml").to_string();
            out.push(Node { path, name, depth, is_dir: false, expanded: false, method });
        } else {
            let expanded = !collapsed.contains(&path);
            out.push(Node { path: path.clone(), name, depth, is_dir: true, expanded, method: None });
            if expanded {
                walk(&path, depth + 1, collapsed, out);
            }
        }
    }
}

/// Make a user-supplied name safe to use as a single path component.
pub fn sanitize(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if matches!(c, '/' | '\\' | ':') { '-' } else { c }).collect();
    s.trim_start_matches('.').trim_end_matches(".toml").to_string()
}

/// Clean a user-typed path relative to the collection root (`a/b/name`).
/// Returns `None` when no final name was given.
pub fn rel_path(text: &str) -> Option<String> {
    if text.trim_end().ends_with('/') {
        return None;
    }
    let parts: Vec<String> = text.split('/').map(sanitize).filter(|p| !p.is_empty()).collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Variables from the collection's `.env` file (`KEY=VALUE` lines).
pub fn load_env(root: &Path) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    if let Ok(text) = fs::read_to_string(root.join(".env")) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim();
                let v = v
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                    .unwrap_or(v);
                vars.insert(k.trim().trim_start_matches("export ").trim().to_string(), v.to_string());
            }
        }
    }
    vars
}

/// Replace `{{name}}` with a value from `.env`, falling back to the process environment.
pub fn substitute(s: &str, vars: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("{{") {
        let Some(end) = rest[start..].find("}}") else { break };
        let name = rest[start + 2..start + end].trim();
        out.push_str(&rest[..start]);
        match vars.get(name).cloned().or_else(|| std::env::var(name).ok()) {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[start..start + end + 2]),
        }
        rest = &rest[start + end + 2..];
    }
    out.push_str(rest);
    out
}

/// Populate a brand new collection with a couple of example requests.
pub fn seed(root: &Path) {
    let get = Request {
        url: "https://httpbin.org/get".into(),
        params: vec![KV::new("hello", "world")],
        ..Default::default()
    };
    let post = Request {
        method: "POST".into(),
        url: "{{base_url}}/post".into(),
        body: "{\n  \"name\": \"lazypost\",\n  \"vim\": true\n}".into(),
        headers: vec![KV::new("Accept", "application/json")],
        ..Default::default()
    };
    let _ = save(&root.join("examples/get.toml"), &get);
    let _ = save(&root.join("examples/post-json.toml"), &post);
    let _ = fs::write(root.join(".env"), "# Variables usable as {{name}} in URLs, headers, params and bodies\nbase_url=https://httpbin.org\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let req = Request {
            method: "POST".into(),
            url: "http://x".into(),
            body: "{\n  \"a\": 1\n}".into(),
            auth: Auth { kind: AuthKind::Basic, username: "u".into(), password: "p".into(), token: String::new() },
            params: vec![KV::new("a", "b")],
            headers: vec![KV { key: "X".into(), value: "y".into(), enabled: false }],
        };
        let text = toml::to_string_pretty(&req).unwrap();
        assert_eq!(toml::from_str::<Request>(&text).unwrap(), req);
        let text = toml::to_string_pretty(&Request::default()).unwrap();
        assert_eq!(toml::from_str::<Request>(&text).unwrap(), Request::default());
    }

    #[test]
    fn rel_paths() {
        assert_eq!(rel_path("users"), Some("users".into()));
        assert_eq!(rel_path(" /a//b / c.toml"), Some("a/b/c".into()));
        assert_eq!(rel_path("../../etc/x"), Some("etc/x".into()));
        assert_eq!(rel_path("examples/"), None);
        assert_eq!(rel_path(""), None);
    }

    #[test]
    fn substitution() {
        let vars = HashMap::from([("host".to_string(), "example.com".to_string())]);
        assert_eq!(substitute("https://{{ host }}/a/{{nope_zz}}", &vars), "https://example.com/a/{{nope_zz}}");
        assert_eq!(substitute("no {{ end", &vars), "no {{ end");
    }
}
