//! Application state and key handling.
//!
//! Input is layered: popups, then the `:` command line, then the active vim
//! editor, then pane navigation. `Esc` always moves one layer up.

use crate::{
    http::{self, Response},
    model::{self, Auth, AuthKind, Node, Request, KV, METHODS},
    vim::{self, Outcome, Syntax, Vim},
};
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    widgets::ListState,
};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Collection,
    Url,
    Request,
    Response,
}

const PANES: [Pane; 4] = [Pane::Collection, Pane::Url, Pane::Request, Pane::Response];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReqTab {
    Params,
    Headers,
    Body,
    Auth,
}

const REQ_TABS: [ReqTab; 4] = [ReqTab::Params, ReqTab::Headers, ReqTab::Body, ReqTab::Auth];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RespTab {
    Body,
    Headers,
}

/// Which editor currently captures the keyboard.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edit {
    Url,
    Body,
    Response,
    Cell,
    Prompt,
    /// The collection's `.env` file, shown as an overlay.
    Env,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptKind {
    NewRequest,
    NewFolder,
    Rename,
    SaveAs,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Confirm {
    Delete(PathBuf),
    Open(PathBuf),
    New,
    Quit,
}

pub enum Popup {
    Help { scroll: u16 },
    Method { sel: usize },
    Prompt(PromptKind),
    Confirm { msg: String, action: Confirm },
}

#[derive(Clone, Copy, Default)]
pub struct TableCur {
    pub row: usize,
    pub col: usize,
    pub top: usize,
}

pub struct App {
    pub root: PathBuf,
    pub tree: Vec<Node>,
    pub tree_state: ListState,
    collapsed: HashSet<PathBuf>,

    pub current: Option<PathBuf>,
    saved: Request,
    pub method: String,
    pub url: Vim,
    pub params: Vec<KV>,
    pub headers: Vec<KV>,
    pub body: Vim,
    pub auth: Auth,

    pub focus: Pane,
    last_right: Pane,
    pub zoom: bool,
    pub req_tab: ReqTab,
    pub resp_tab: RespTab,
    pub params_cur: TableCur,
    pub headers_cur: TableCur,
    pub auth_row: usize,

    pub edit: Option<Edit>,
    pub cell: Vim,
    pub env: Vim,

    pub response: Option<Result<Response, String>>,
    pub resp_body: Vim,
    pub resp_headers: Vim,
    pending: Option<(Receiver<Result<Response, String>>, Instant)>,

    pub popup: Option<Popup>,
    pub cmdline: Option<String>,
    pub message: Option<(String, bool, Instant)>,
    pub quit: bool,
}

impl App {
    pub fn new(root: PathBuf) -> Self {
        let mut body = Vim::new(false);
        body.line_numbers = true;
        let mut app = Self {
            root,
            tree: Vec::new(),
            tree_state: ListState::default(),
            collapsed: HashSet::new(),
            current: None,
            saved: Request::default(),
            method: "GET".into(),
            url: Vim::new(true),
            params: Vec::new(),
            headers: Vec::new(),
            body,
            auth: Auth::default(),
            focus: Pane::Collection,
            last_right: Pane::Url,
            zoom: false,
            req_tab: ReqTab::Params,
            resp_tab: RespTab::Body,
            params_cur: TableCur::default(),
            headers_cur: TableCur::default(),
            auth_row: 0,
            edit: None,
            cell: Vim::new(true),
            env: {
                let mut env = Vim::new(false);
                env.line_numbers = true;
                env.syntax = Syntax::Env;
                env
            },
            response: None,
            resp_body: Vim::read_only(),
            resp_headers: Vim::read_only(),
            pending: None,
            popup: None,
            cmdline: None,
            message: None,
            quit: false,
        };
        app.rescan(None);
        match app.tree.iter().position(|n| !n.is_dir) {
            Some(i) => {
                app.tree_state.select(Some(i));
                let path = app.tree[i].path.clone();
                app.load_into(&path);
            }
            None => app.focus = Pane::Url,
        }
        app
    }

    // ---- state queries -------------------------------------------------

    pub fn build_request(&self) -> Request {
        Request {
            method: self.method.clone(),
            url: self.url.text(),
            body: self.body.text(),
            auth: self.auth.clone(),
            params: self.params.clone(),
            headers: self.headers.clone(),
        }
    }

    pub fn dirty(&self) -> bool {
        self.build_request() != self.saved
    }

    pub fn sending(&self) -> Option<Duration> {
        self.pending.as_ref().map(|(_, t)| t.elapsed())
    }

    pub fn vim(&self, e: Edit) -> &Vim {
        match e {
            Edit::Url => &self.url,
            Edit::Body => &self.body,
            Edit::Response if self.resp_tab == RespTab::Body => &self.resp_body,
            Edit::Response => &self.resp_headers,
            Edit::Cell | Edit::Prompt => &self.cell,
            Edit::Env => &self.env,
        }
    }

    fn vim_mut(&mut self, e: Edit) -> &mut Vim {
        match e {
            Edit::Url => &mut self.url,
            Edit::Body => &mut self.body,
            Edit::Response if self.resp_tab == RespTab::Body => &mut self.resp_body,
            Edit::Response => &mut self.resp_headers,
            Edit::Cell | Edit::Prompt => &mut self.cell,
            Edit::Env => &mut self.env,
        }
    }

    pub fn auth_fields(&self) -> &'static [&'static str] {
        match self.auth.kind {
            AuthKind::None => &[],
            AuthKind::Basic => &["Username", "Password"],
            AuthKind::Bearer => &["Token"],
        }
    }

    fn selected_node(&self) -> Option<&Node> {
        self.tree.get(self.tree_state.selected()?)
    }

    /// Directory new items should be created in, based on the tree selection.
    fn target_dir(&self) -> PathBuf {
        match self.selected_node() {
            Some(n) if n.is_dir => n.path.clone(),
            Some(n) => n.path.parent().unwrap_or(&self.root).to_path_buf(),
            None => self.root.clone(),
        }
    }

    fn info(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), false, Instant::now()));
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), true, Instant::now()));
    }

    // ---- collection ----------------------------------------------------

    fn rescan(&mut self, select: Option<&Path>) {
        let keep = select.map(Path::to_path_buf).or_else(|| self.selected_node().map(|n| n.path.clone()));
        self.tree = model::scan(&self.root, &self.collapsed);
        let idx = keep.and_then(|p| self.tree.iter().position(|n| n.path == p));
        let old = self.tree_state.selected().unwrap_or(0);
        self.tree_state.select(if self.tree.is_empty() {
            None
        } else {
            Some(idx.unwrap_or(old.min(self.tree.len() - 1)))
        });
    }

    fn load_into(&mut self, path: &Path) {
        match model::load(path) {
            Ok(req) => {
                self.set_request(&req);
                self.current = Some(path.to_path_buf());
            }
            Err(e) => self.error(format!("Cannot open: {e}")),
        }
    }

    fn set_request(&mut self, req: &Request) {
        self.method = req.method.clone();
        self.url.set_text(&req.url);
        self.body.set_text(&req.body);
        self.params = req.params.clone();
        self.headers = req.headers.clone();
        self.auth = req.auth.clone();
        self.params_cur = TableCur::default();
        self.headers_cur = TableCur::default();
        self.auth_row = 0;
        self.saved = self.build_request();
        self.response = None;
        self.resp_body.set_text("");
        self.resp_headers.set_text("");
        self.pending = None;
    }

    /// Save to the current file, `name` in the target directory, or prompt for a name.
    fn save(&mut self, name: Option<&str>) -> bool {
        let path = if let Some(n) = name.and_then(model::rel_path) {
            self.root.join(format!("{n}.toml"))
        } else if let Some(p) = self.current.clone() {
            p
        } else {
            self.start_prompt(PromptKind::SaveAs);
            return false;
        };
        let req = self.build_request();
        match model::save(&path, &req) {
            Ok(()) => {
                self.saved = req;
                self.rescan(Some(&path));
                self.info(format!("Saved {}", self.rel(&path)));
                self.current = Some(path);
                true
            }
            Err(e) => {
                self.error(format!("Save failed: {e}"));
                false
            }
        }
    }

    fn rel(&self, path: &Path) -> String {
        path.strip_prefix(&self.root).unwrap_or(path).display().to_string()
    }

    /// Returns true if it is safe to replace the current request; otherwise asks.
    fn guard(&mut self, action: Confirm) -> bool {
        if !self.dirty() {
            return true;
        }
        self.popup = Some(Popup::Confirm { msg: "Discard unsaved changes?".into(), action });
        false
    }

    fn start_prompt(&mut self, kind: PromptKind) {
        // Names are paths relative to the collection root, prefilled with the
        // selected folder so the destination is visible and editable.
        let rel_dir = |dir: &Path| match self.rel(dir) {
            r if r.is_empty() => r,
            r => format!("{r}/"),
        };
        let prefill = match kind {
            PromptKind::Rename => self
                .selected_node()
                .map(|n| self.rel(&n.path).trim_end_matches(".toml").to_string())
                .unwrap_or_default(),
            PromptKind::SaveAs => match &self.current {
                Some(p) => rel_dir(p.parent().unwrap_or(&self.root)),
                None => rel_dir(&self.target_dir()),
            },
            _ => rel_dir(&self.target_dir()),
        };
        self.cell.set_text(&prefill);
        self.cell.insert_at_end();
        self.popup = Some(Popup::Prompt(kind));
        self.edit = Some(Edit::Prompt);
    }

    fn finish_prompt(&mut self, kind: PromptKind, text: &str) {
        let Some(name) = model::rel_path(text) else { return };
        match kind {
            PromptKind::NewRequest => self.new_request(&name),
            PromptKind::NewFolder => {
                let path = self.root.join(&name);
                match fs::create_dir_all(&path) {
                    Ok(()) => self.rescan(Some(&path)),
                    Err(e) => self.error(e.to_string()),
                }
            }
            PromptKind::Rename => {
                let Some(node) = self.selected_node().cloned() else { return };
                let file = if node.is_dir { name } else { format!("{name}.toml") };
                let to = self.root.join(file);
                if to == node.path {
                    return;
                }
                if to.exists() {
                    return self.error("A file with that name already exists");
                }
                if to.starts_with(&node.path) {
                    return self.error("Cannot move a folder into itself");
                }
                if let Some(dir) = to.parent() {
                    let _ = fs::create_dir_all(dir);
                }
                match fs::rename(&node.path, &to) {
                    Ok(()) => {
                        let rest = self.current.as_ref().and_then(|c| c.strip_prefix(&node.path).ok());
                        if let Some(rest) = rest.map(Path::to_path_buf) {
                            self.current = Some(if rest.as_os_str().is_empty() { to.clone() } else { to.join(rest) });
                        }
                        self.rescan(Some(&to));
                    }
                    Err(e) => self.error(e.to_string()),
                }
            }
            PromptKind::SaveAs => {
                self.save(Some(&name));
            }
        }
    }

    fn new_request(&mut self, name: &str) {
        let path = self.root.join(format!("{name}.toml"));
        if path.exists() {
            return self.error("A request with that name already exists");
        }
        let req = Request::default();
        match model::save(&path, &req) {
            Ok(()) => {
                self.set_request(&req);
                self.current = Some(path.clone());
                self.rescan(Some(&path));
                self.set_focus(Pane::Url);
                self.url.insert_at_end();
                self.edit = Some(Edit::Url);
            }
            Err(e) => self.error(e.to_string()),
        }
    }

    fn confirmed(&mut self, action: Confirm) {
        match action {
            Confirm::Delete(path) => {
                let res = if path.is_dir() { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
                match res {
                    Ok(()) => {
                        if self.current.as_ref().is_some_and(|c| c.starts_with(&path)) {
                            self.current = None;
                        }
                        self.rescan(None);
                    }
                    Err(e) => self.error(e.to_string()),
                }
            }
            Confirm::Open(path) => self.load_into(&path),
            Confirm::New => self.start_prompt(PromptKind::NewRequest),
            Confirm::Quit => self.quit = true,
        }
    }

    // ---- environment ---------------------------------------------------

    fn open_env(&mut self) {
        let text = fs::read_to_string(self.root.join(".env")).unwrap_or_default();
        self.env.set_text(text.strip_suffix('\n').unwrap_or(&text));
        self.edit = Some(Edit::Env);
    }

    /// Write the `.env` buffer to disk if it differs from what is there.
    fn save_env(&mut self) {
        let path = self.root.join(".env");
        let mut text = self.env.text();
        if !text.is_empty() {
            text.push('\n');
        }
        if fs::read_to_string(&path).unwrap_or_default() == text {
            return;
        }
        match fs::write(&path, text) {
            Ok(()) => self.info("Saved .env"),
            Err(e) => self.error(format!("Cannot save .env: {e}")),
        }
    }

    // ---- sending -------------------------------------------------------

    pub fn send(&mut self) {
        match self.edit {
            Some(Edit::Cell) => self.commit_cell(),
            Some(Edit::Env) => self.save_env(),
            _ => {}
        }
        let req = self.build_request();
        if req.url.trim().is_empty() {
            return self.error("URL is empty");
        }
        let vars = model::load_env(&self.root);
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(http::send(&req, &vars));
        });
        self.pending = Some((rx, Instant::now()));
    }

    /// Called every loop iteration: collect a finished request, expire messages.
    pub fn tick(&mut self) {
        if let Some((rx, _)) = &self.pending {
            match rx.try_recv() {
                Ok(res) => {
                    self.pending = None;
                    self.set_response(res);
                }
                Err(TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.set_response(Err("Request thread died".into()));
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.message.as_ref().is_some_and(|(_, _, t)| t.elapsed() > Duration::from_secs(5)) {
            self.message = None;
        }
    }

    fn set_response(&mut self, res: Result<Response, String>) {
        if self.edit == Some(Edit::Response) {
            self.edit = None;
        }
        match &res {
            Ok(r) => {
                self.resp_body.set_text(&r.body);
                self.resp_body.syntax = if r.json { Syntax::Json } else { Syntax::Plain };
                let headers: Vec<String> = r.headers.iter().map(|(k, v)| format!("{k}: {v}")).collect();
                self.resp_headers.set_text(&headers.join("\n"));
            }
            Err(e) => {
                self.resp_body.set_text(e);
                self.resp_body.syntax = Syntax::Plain;
                self.resp_headers.set_text("");
            }
        }
        self.response = Some(res);
    }

    // ---- input ---------------------------------------------------------

    pub fn on_paste(&mut self, text: &str) {
        if let Some(cmd) = self.cmdline.as_mut() {
            cmd.push_str(&text.replace(['\n', '\r'], " "));
        } else if let Some(e) = self.edit {
            self.vim_mut(e).paste(text);
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('s') {
            return self.send();
        }
        match self.popup.take() {
            Some(Popup::Prompt(kind)) => self.popup = Some(Popup::Prompt(kind)),
            Some(p) => return self.key_popup(p, key),
            None => {}
        }
        if self.cmdline.is_some() {
            self.key_cmdline(key);
        } else if let Some(e) = self.edit {
            self.key_edit(e, key);
        } else {
            self.key_nav(key);
        }
    }

    fn key_popup(&mut self, popup: Popup, key: KeyEvent) {
        match popup {
            Popup::Help { scroll } => {
                let scroll = match key.code {
                    KeyCode::Char('j') | KeyCode::Down => scroll.saturating_add(1),
                    KeyCode::Char('k') | KeyCode::Up => scroll.saturating_sub(1),
                    KeyCode::Char('d') => scroll.saturating_add(10),
                    KeyCode::Char('u') => scroll.saturating_sub(10),
                    KeyCode::Char('g') => 0,
                    _ => return,
                };
                self.popup = Some(Popup::Help { scroll });
            }
            Popup::Method { sel } => {
                let pick = match key.code {
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.popup = Some(Popup::Method { sel: (sel + 1) % METHODS.len() });
                        return;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.popup = Some(Popup::Method { sel: (sel + METHODS.len() - 1) % METHODS.len() });
                        return;
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => Some(sel),
                    KeyCode::Char(c) => METHODS.iter().position(|(_, k)| *k == c.to_ascii_lowercase()),
                    _ => None,
                };
                if let Some(i) = pick {
                    self.method = METHODS[i].0.to_string();
                }
            }
            Popup::Confirm { msg, action } => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => self.confirmed(action),
                KeyCode::Char('s') if !matches!(action, Confirm::Delete(_)) => {
                    if self.save(None) {
                        self.confirmed(action);
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('q') | KeyCode::Esc => {}
                _ => self.popup = Some(Popup::Confirm { msg, action }),
            },
            Popup::Prompt(_) => {}
        }
    }

    fn key_cmdline(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(cmd) = self.cmdline.as_mut() else { return };
        match key.code {
            KeyCode::Esc => self.cmdline = None,
            KeyCode::Char('c') if ctrl => self.cmdline = None,
            KeyCode::Char('u') if ctrl => cmd.clear(),
            KeyCode::Char('w') if ctrl => {
                let keep = cmd.trim_end().rfind(' ').map_or(0, |i| i + 1);
                cmd.truncate(keep);
            }
            KeyCode::Backspace => {
                if cmd.pop().is_none() {
                    self.cmdline = None;
                }
            }
            KeyCode::Enter => {
                let cmd = self.cmdline.take().unwrap_or_default();
                self.run_cmd(cmd.trim());
            }
            KeyCode::Char(c) if !ctrl => cmd.push(c),
            _ => {}
        }
    }

    fn run_cmd(&mut self, cmd: &str) {
        let (name, arg) = cmd.split_once(char::is_whitespace).unwrap_or((cmd, ""));
        let arg = Some(arg.trim()).filter(|a| !a.is_empty());
        match name {
            "" => {}
            "w" | "write" => {
                self.save(arg);
            }
            "q" | "quit" => self.try_quit(),
            "q!" | "quit!" => self.quit = true,
            "wq" | "x" => {
                if self.save(arg) {
                    self.quit = true;
                }
            }
            "new" | "enew" => {
                if self.guard(Confirm::New) {
                    match arg.and_then(model::rel_path) {
                        Some(n) => self.new_request(&n),
                        None => self.start_prompt(PromptKind::NewRequest),
                    }
                }
            }
            "s" | "send" => self.send(),
            "e" | "e!" | "reload" => {
                if let Some(p) = self.current.clone() {
                    self.load_into(&p);
                }
                self.rescan(None);
            }
            "env" => self.open_env(),
            "h" | "help" => self.popup = Some(Popup::Help { scroll: 0 }),
            _ => self.error(format!("Not a command: {name}")),
        }
    }

    fn try_quit(&mut self) {
        if self.guard(Confirm::Quit) {
            self.quit = true;
        }
    }

    fn key_edit(&mut self, e: Edit, key: KeyEvent) {
        let vim = self.vim_mut(e);
        if e == Edit::Body && vim.mode == vim::Mode::Normal && vim.pending().is_empty() && key.code == KeyCode::Char('=') {
            self.format_body();
            return;
        }
        let outcome = self.vim_mut(e).handle_key(key);
        match (e, outcome) {
            (_, Outcome::Consumed) => {}
            (Edit::Cell, Outcome::Next) => self.cell_step(true),
            (Edit::Cell, Outcome::Prev) => self.cell_step(false),
            (_, Outcome::Next | Outcome::Prev) => {}
            (Edit::Cell, _) => self.end_cell(),
            (Edit::Prompt, out) => {
                self.edit = None;
                if let Some(Popup::Prompt(kind)) = self.popup.take() {
                    if out == Outcome::Submit {
                        let text = self.cell.text();
                        self.finish_prompt(kind, &text);
                    }
                }
            }
            (Edit::Url, out) => {
                self.edit = None;
                if out == Outcome::Submit {
                    self.send();
                }
            }
            (Edit::Env, _) => {
                self.save_env();
                self.edit = None;
            }
            (Edit::Body | Edit::Response, _) => self.edit = None,
        }
    }

    /// Pretty-print the request body if it is valid JSON; `=` in the body's normal mode.
    fn format_body(&mut self) {
        match serde_json::from_str::<serde_json::Value>(&self.body.text()) {
            Ok(v) => {
                let pretty = serde_json::to_string_pretty(&v).unwrap_or_default();
                self.body.replace_text(&pretty);
                self.info("Formatted JSON body");
            }
            Err(e) => self.error(format!("Body is not valid JSON: {e}")),
        }
    }

    fn set_focus(&mut self, pane: Pane) {
        self.focus = pane;
        if pane != Pane::Collection {
            self.last_right = pane;
        }
    }

    fn key_nav(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let idx = PANES.iter().position(|p| *p == self.focus).unwrap_or(0);
        match key.code {
            KeyCode::Char('h') if ctrl => self.set_focus(Pane::Collection),
            KeyCode::Char('l') if ctrl => self.set_focus(self.last_right),
            KeyCode::Char('j') if ctrl => {
                if idx > 0 && idx < 3 {
                    self.set_focus(PANES[idx + 1]);
                }
            }
            KeyCode::Char('k') if ctrl => {
                if idx > 1 {
                    self.set_focus(PANES[idx - 1]);
                }
            }
            KeyCode::Char('c') if ctrl => {
                if self.pending.take().is_some() {
                    self.info("Request cancelled");
                }
            }
            KeyCode::Char(c @ '1'..='4') => self.set_focus(PANES[c as usize - '1' as usize]),
            KeyCode::Tab => self.set_focus(PANES[(idx + 1) % 4]),
            KeyCode::BackTab => self.set_focus(PANES[(idx + 3) % 4]),
            KeyCode::Char('q') if !ctrl => self.try_quit(),
            KeyCode::Char('?') => self.popup = Some(Popup::Help { scroll: 0 }),
            KeyCode::Char(':') => self.cmdline = Some(String::new()),
            KeyCode::Char('s') => self.send(),
            KeyCode::Char('m') => {
                let sel = METHODS.iter().position(|(m, _)| *m == self.method).unwrap_or(0);
                self.popup = Some(Popup::Method { sel });
            }
            KeyCode::Char('E') => self.open_env(),
            KeyCode::Char('z') => self.zoom = !self.zoom,
            KeyCode::Esc => {
                self.zoom = false;
                self.message = None;
            }
            KeyCode::Char(']') | KeyCode::Char('L') => self.cycle_tab(1),
            KeyCode::Char('[') | KeyCode::Char('H') => self.cycle_tab(-1),
            _ => match self.focus {
                Pane::Collection => self.key_collection(key),
                Pane::Url => self.key_url(key),
                Pane::Request => match self.req_tab {
                    ReqTab::Params | ReqTab::Headers => self.key_table(key),
                    ReqTab::Body => self.key_view(Edit::Body, key),
                    ReqTab::Auth => self.key_auth(key),
                },
                Pane::Response => self.key_view(Edit::Response, key),
            },
        }
    }

    fn cycle_tab(&mut self, dir: isize) {
        if self.focus == Pane::Response {
            self.resp_tab = if self.resp_tab == RespTab::Body { RespTab::Headers } else { RespTab::Body };
        } else {
            let i = REQ_TABS.iter().position(|t| *t == self.req_tab).unwrap_or(0) as isize;
            self.req_tab = REQ_TABS[(i + dir).rem_euclid(4) as usize];
            self.set_focus(Pane::Request);
        }
    }

    fn key_collection(&mut self, key: KeyEvent) {
        let n = self.tree.len();
        let sel = self.tree_state.selected().unwrap_or(0);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let mut select = |i: usize| {
            if n > 0 {
                self.tree_state.select(Some(i.min(n - 1)));
            }
        };
        match key.code {
            KeyCode::Char('d') if ctrl => select(sel + 10),
            KeyCode::Char('u') if ctrl => select(sel.saturating_sub(10)),
            KeyCode::Char('j') | KeyCode::Down => select(sel + 1),
            KeyCode::Char('k') | KeyCode::Up => select(sel.saturating_sub(1)),
            KeyCode::Char('g') => select(0),
            KeyCode::Char('G') => select(usize::MAX),
            KeyCode::Char('l') | KeyCode::Right => self.activate(false),
            KeyCode::Enter => self.activate(true),
            KeyCode::Char('h') | KeyCode::Left => {
                let Some(node) = self.tree.get(sel).cloned() else { return };
                if node.is_dir && node.expanded {
                    self.collapsed.insert(node.path);
                    self.rescan(None);
                } else if let Some(parent) = (0..sel).rev().find(|&i| self.tree[i].depth < node.depth) {
                    self.tree_state.select(Some(parent));
                }
            }
            KeyCode::Char('a') => {
                if self.guard(Confirm::New) {
                    self.start_prompt(PromptKind::NewRequest);
                }
            }
            KeyCode::Char('A') => self.start_prompt(PromptKind::NewFolder),
            KeyCode::Char('r') if n > 0 => self.start_prompt(PromptKind::Rename),
            KeyCode::Char('R') => self.rescan(None),
            KeyCode::Char('d') => {
                if let Some(node) = self.tree.get(sel) {
                    let what = if node.is_dir { "folder and everything in it" } else { "request" };
                    self.popup = Some(Popup::Confirm {
                        msg: format!("Delete {what} \"{}\"?", node.name),
                        action: Confirm::Delete(node.path.clone()),
                    });
                }
            }
            _ => {}
        }
    }

    /// Open the selected request or toggle the selected folder.
    fn activate(&mut self, jump: bool) {
        let Some(node) = self.selected_node().cloned() else { return };
        if node.is_dir {
            if !self.collapsed.remove(&node.path) {
                self.collapsed.insert(node.path);
            }
            self.rescan(None);
            return;
        }
        if self.current.as_ref() != Some(&node.path) {
            if !self.guard(Confirm::Open(node.path.clone())) {
                return;
            }
            self.load_into(&node.path);
        }
        if jump {
            self.set_focus(Pane::Url);
        }
    }

    fn key_url(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.send(),
            KeyCode::Char('i') | KeyCode::Char('a') | KeyCode::Char('A') => {
                self.url.insert_at_end();
                self.edit = Some(Edit::Url);
            }
            KeyCode::Char('I') => {
                self.url.insert_at_start();
                self.edit = Some(Edit::Url);
            }
            KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Char('S') => {
                self.url.change_all();
                self.edit = Some(Edit::Url);
            }
            KeyCode::Char('v') | KeyCode::Char('e') => self.edit = Some(Edit::Url),
            _ => {}
        }
    }

    /// Navigation keys for a multi-line buffer (request body / response) before entering it.
    fn key_view(&mut self, e: Edit, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let read_only = e == Edit::Response;
        if read_only && self.response.is_none() {
            return;
        }
        let vim = self.vim_mut(e);
        let half = vim.half_page();
        match key.code {
            KeyCode::Char('d') if ctrl => vim.scroll(half),
            KeyCode::Char('u') if ctrl => vim.scroll(-half),
            KeyCode::Char('j') | KeyCode::Down => vim.scroll(1),
            KeyCode::Char('k') | KeyCode::Up => vim.scroll(-1),
            KeyCode::Char('g') => vim.goto(false),
            KeyCode::Char('G') => vim.goto(true),
            KeyCode::Char('y') if read_only => {
                let text = vim.text();
                vim::yank(&text, false);
                self.info("Copied to clipboard");
            }
            KeyCode::Enter => self.edit = Some(e),
            KeyCode::Char('i') if read_only => self.edit = Some(e),
            KeyCode::Char('v' | 'V' | '/') | KeyCode::Char('i' | 'a' | 'I' | 'A' | 'o' | 'O') => {
                vim.handle_key(key);
                self.edit = Some(e);
            }
            _ => {}
        }
    }

    fn table(&mut self) -> Option<(&mut Vec<KV>, &mut TableCur)> {
        match self.req_tab {
            ReqTab::Params => Some((&mut self.params, &mut self.params_cur)),
            ReqTab::Headers => Some((&mut self.headers, &mut self.headers_cur)),
            _ => None,
        }
    }

    fn key_table(&mut self, key: KeyEvent) {
        let Some((rows, cur)) = self.table() else { return };
        let n = rows.len();
        cur.row = cur.row.min(n.saturating_sub(1));
        // Some(clear) => start editing the selected cell afterwards
        let mut edit = None;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => cur.row = (cur.row + 1).min(n.saturating_sub(1)),
            KeyCode::Char('k') | KeyCode::Up => cur.row = cur.row.saturating_sub(1),
            KeyCode::Char('g') => cur.row = 0,
            KeyCode::Char('G') => cur.row = n.saturating_sub(1),
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Char('0') => cur.col = 0,
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Char('$') => cur.col = 1,
            KeyCode::Char(' ') | KeyCode::Char('x') if n > 0 => rows[cur.row].enabled ^= true,
            KeyCode::Char('d') if n > 0 => {
                let kv = rows.remove(cur.row);
                vim::yank(&format!("{}: {}", kv.key, kv.value), false);
                cur.row = cur.row.min(rows.len().saturating_sub(1));
            }
            KeyCode::Char('o') | KeyCode::Char('O') => {
                let at = if n == 0 || key.code == KeyCode::Char('O') { cur.row } else { cur.row + 1 };
                rows.insert(at, KV::default());
                *cur = TableCur { row: at, col: 0, top: cur.top };
                edit = Some(false);
            }
            KeyCode::Char('i') | KeyCode::Char('a') | KeyCode::Enter => {
                if n == 0 {
                    rows.push(KV::default());
                    cur.col = 0;
                }
                edit = Some(false);
            }
            KeyCode::Char('c') if n > 0 => edit = Some(true),
            _ => {}
        }
        if let Some(clear) = edit {
            self.start_cell(clear);
        }
    }

    fn key_auth(&mut self, key: KeyEvent) {
        let rows = self.auth_fields().len() + 1;
        let cycle = |k: AuthKind, fwd: bool| match (k, fwd) {
            (AuthKind::None, true) | (AuthKind::Bearer, false) => AuthKind::Basic,
            (AuthKind::Basic, true) | (AuthKind::None, false) => AuthKind::Bearer,
            (AuthKind::Bearer, true) | (AuthKind::Basic, false) => AuthKind::None,
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.auth_row = (self.auth_row + 1).min(rows - 1),
            KeyCode::Char('k') | KeyCode::Up => self.auth_row = self.auth_row.saturating_sub(1),
            KeyCode::Char('g') => self.auth_row = 0,
            KeyCode::Char('G') => self.auth_row = rows - 1,
            KeyCode::Char('h') | KeyCode::Left if self.auth_row == 0 => self.auth.kind = cycle(self.auth.kind, false),
            KeyCode::Char('l' | ' ' | 'i') | KeyCode::Right | KeyCode::Enter if self.auth_row == 0 => {
                self.auth.kind = cycle(self.auth.kind, true)
            }
            KeyCode::Char('i') | KeyCode::Char('a') | KeyCode::Enter => self.start_cell(false),
            KeyCode::Char('c') => self.start_cell(true),
            _ => {}
        }
    }

    // ---- cell editing --------------------------------------------------

    fn cell_text_mut(&mut self) -> Option<&mut String> {
        match self.req_tab {
            ReqTab::Params | ReqTab::Headers => {
                let (rows, cur) = self.table()?;
                let kv = rows.get_mut(cur.row)?;
                Some(if cur.col == 0 { &mut kv.key } else { &mut kv.value })
            }
            ReqTab::Auth => match (self.auth.kind, self.auth_row) {
                (AuthKind::Basic, 1) => Some(&mut self.auth.username),
                (AuthKind::Basic, 2) => Some(&mut self.auth.password),
                (AuthKind::Bearer, 1) => Some(&mut self.auth.token),
                _ => None,
            },
            ReqTab::Body => None,
        }
    }

    fn start_cell(&mut self, clear: bool) {
        let Some(text) = self.cell_text_mut().map(|s| s.clone()) else { return };
        self.cell.set_text(if clear { "" } else { &text });
        self.cell.insert_at_end();
        self.edit = Some(Edit::Cell);
    }

    fn commit_cell(&mut self) {
        let text = self.cell.text();
        if let Some(s) = self.cell_text_mut() {
            *s = text;
        }
    }

    fn end_cell(&mut self) {
        self.commit_cell();
        self.edit = None;
        if let Some((rows, cur)) = self.table() {
            rows.retain(|kv| !kv.key.is_empty() || !kv.value.is_empty());
            cur.row = cur.row.min(rows.len().saturating_sub(1));
        }
    }

    /// Tab / Shift-Tab while editing: move to the neighbouring cell and keep editing.
    fn cell_step(&mut self, fwd: bool) {
        self.commit_cell();
        if self.req_tab == ReqTab::Auth {
            let rows = self.auth_fields().len();
            let next = if fwd { self.auth_row + 1 } else { self.auth_row.saturating_sub(1) };
            if next == 0 || next > rows {
                return self.end_cell();
            }
            self.auth_row = next;
            return self.start_cell(false);
        }
        let Some((rows, cur)) = self.table() else { return };
        match (fwd, cur.col) {
            (true, 0) => cur.col = 1,
            (true, _) => {
                let kv = &rows[cur.row];
                if kv.key.is_empty() && kv.value.is_empty() {
                    return self.end_cell();
                }
                cur.row += 1;
                cur.col = 0;
                if cur.row >= rows.len() {
                    rows.push(KV::default());
                }
            }
            (false, 1) => cur.col = 0,
            (false, _) => {
                if cur.row == 0 {
                    return self.end_cell();
                }
                cur.row -= 1;
                cur.col = 1;
            }
        }
        self.start_cell(false);
    }
}
