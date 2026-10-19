//! A small modal (vim-style) text editor used for every editable field.

use ratatui::{
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::Rect,
    style::{Color, Modifier, Style},
};
use std::cell::RefCell;
use unicode_width::UnicodeWidthChar;

thread_local! {
    /// Unnamed register shared by all editors, so yanks can cross fields.
    static REGISTER: RefCell<(String, bool)> = const { RefCell::new((String::new(), false)) };
    // Tests must not touch the real system clipboard.
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> =
        RefCell::new(if cfg!(test) { None } else { arboard::Clipboard::new().ok() });
}

/// Store text in the shared register and the system clipboard.
pub fn yank(text: &str, linewise: bool) {
    if text.is_empty() {
        return;
    }
    REGISTER.with(|r| *r.borrow_mut() = (text.to_string(), linewise));
    CLIPBOARD.with(|c| {
        if let Some(c) = c.borrow_mut().as_mut() {
            let _ = c.set_text(text.to_string());
        }
    });
}

/// The register contents, preferring the system clipboard if it changed externally.
fn register() -> (String, bool) {
    let (reg, linewise) = REGISTER.with(|r| r.borrow().clone());
    let sys = CLIPBOARD.with(|c| c.borrow_mut().as_mut().and_then(|c| c.get_text().ok()));
    match sys {
        Some(s) if !s.is_empty() && s != reg => (s, false),
        _ => (reg, linewise),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
    Search,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Debug)]
pub struct Pos {
    pub y: usize,
    pub x: usize,
}

/// What the owner of the editor should do after a key press.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Consumed,
    /// Esc in normal mode: leave the editor.
    Exit,
    /// Enter in a single-line editor.
    Submit,
    /// Tab / Shift-Tab in a single-line editor.
    Next,
    Prev,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Syntax {
    Plain,
    Json,
    Env,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MKind {
    Excl,
    Incl,
    Line,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pend {
    G,
    Find(char),
    Replace,
    Obj(bool),
}

type Snap = (Vec<Vec<char>>, Pos);

pub struct Vim {
    lines: Vec<Vec<char>>,
    cur: Pos,
    want_x: usize,
    pub mode: Mode,
    anchor: Pos,
    count: Option<usize>,
    pre_count: Option<usize>,
    op: Option<char>,
    pend: Option<Pend>,
    last_find: Option<(char, char)>,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    top: usize,
    left: usize,
    height: usize,
    search: String,
    search_input: Option<String>,
    pub single_line: bool,
    pub read_only: bool,
    pub line_numbers: bool,
    pub syntax: Syntax,
}

fn class(c: char, big: bool) -> u8 {
    if c.is_whitespace() {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

fn matches_at(line: &[char], pat: &[char], x: usize, ci: bool) -> bool {
    x + pat.len() <= line.len()
        && pat.iter().enumerate().all(|(i, &p)| {
            let c = line[x + i];
            c == p || (ci && c.to_lowercase().eq(p.to_lowercase()))
        })
}

fn find_from(line: &[char], pat: &[char], from: usize, ci: bool) -> Option<usize> {
    (from..line.len()).find(|&x| matches_at(line, pat, x, ci))
}

fn bracket_pair(c: char) -> Option<(char, char)> {
    match c {
        '(' | ')' | 'b' => Some(('(', ')')),
        '{' | '}' | 'B' => Some(('{', '}')),
        '[' | ']' => Some(('[', ']')),
        '<' | '>' => Some(('<', '>')),
        _ => None,
    }
}

impl Vim {
    pub fn new(single_line: bool) -> Self {
        Self {
            lines: vec![Vec::new()],
            cur: Pos::default(),
            want_x: 0,
            mode: Mode::Normal,
            anchor: Pos::default(),
            count: None,
            pre_count: None,
            op: None,
            pend: None,
            last_find: None,
            undo: Vec::new(),
            redo: Vec::new(),
            top: 0,
            left: 0,
            height: 1,
            search: String::new(),
            search_input: None,
            single_line,
            read_only: false,
            line_numbers: false,
            syntax: Syntax::Plain,
        }
    }

    pub fn read_only() -> Self {
        let mut v = Self::new(false);
        v.read_only = true;
        v.line_numbers = true;
        v
    }

    // ---- content -------------------------------------------------------

    pub fn set_text(&mut self, text: &str) {
        self.lines = if self.single_line {
            vec![text.replace(['\n', '\r'], " ").chars().collect()]
        } else {
            text.split('\n').map(|l| l.trim_end_matches('\r').chars().collect()).collect()
        };
        self.cur = Pos::default();
        self.want_x = 0;
        self.top = 0;
        self.left = 0;
        self.mode = Mode::Normal;
        self.undo.clear();
        self.redo.clear();
        self.search_input = None;
        self.reset();
    }

    pub fn text(&self) -> String {
        self.lines.iter().map(|l| l.iter().collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    /// Replace the whole buffer as one undoable change, keeping the cursor on a valid cell.
    pub fn replace_text(&mut self, text: &str) {
        if self.read_only || self.text() == text {
            return;
        }
        self.snapshot();
        self.lines = text.split('\n').map(|l| l.chars().collect()).collect();
        self.cur.y = self.cur.y.min(self.lines.len() - 1);
        self.clamp();
        self.want_x = self.cur.x;
    }

    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    pub fn looks_like_json(&self) -> bool {
        matches!(self.lines.iter().flatten().find(|c| !c.is_whitespace()), Some('{') | Some('['))
    }

    pub fn cursor(&self) -> Pos {
        self.cur
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Keys typed so far for an unfinished command (e.g. `2d`).
    pub fn pending(&self) -> String {
        let mut s = String::new();
        if let Some(n) = self.pre_count {
            s.push_str(&n.to_string());
        }
        if let Some(op) = self.op {
            s.push(op);
        }
        if let Some(n) = self.count {
            s.push_str(&n.to_string());
        }
        match self.pend {
            Some(Pend::G) => s.push('g'),
            Some(Pend::Find(c)) => s.push(c),
            Some(Pend::Replace) => s.push('r'),
            Some(Pend::Obj(inner)) => s.push(if inner { 'i' } else { 'a' }),
            None => {}
        }
        s
    }

    pub fn search_prompt(&self) -> Option<&str> {
        self.search_input.as_deref()
    }

    /// In insert mode, the word typed after an `@` that starts a word, e.g. `base_u` in `@base_u`.
    pub fn completion_prefix(&self) -> Option<String> {
        if self.mode != Mode::Insert {
            return None;
        }
        let line = &self.lines[self.cur.y];
        let x = self.cur.x.min(line.len());
        let start = (0..x).rev().find(|&i| class(line[i], false) != 1).map_or(0, |i| i + 1);
        let at = start.checked_sub(1).filter(|&i| line[i] == '@')?;
        if at > 0 && class(line[at - 1], false) == 1 {
            return None;
        }
        Some(line[start..x].iter().collect())
    }

    /// Replace the `@prefix` before the cursor with `{{name}}`.
    pub fn complete(&mut self, name: &str) {
        let Some(prefix) = self.completion_prefix() else {
            return;
        };
        let Pos { y, x } = self.cur;
        let start = x - prefix.chars().count() - 1;
        let text: Vec<char> = format!("{{{{{name}}}}}").chars().collect();
        self.cur.x = start + text.len();
        self.lines[y].splice(start..x, text);
    }

    // ---- entry points used by the app ----------------------------------

    pub fn insert_at_end(&mut self) {
        self.cur.y = self.cur.y.min(self.lines.len() - 1);
        self.begin_insert(self.lines[self.cur.y].len());
    }

    pub fn insert_at_start(&mut self) {
        self.begin_insert(0);
    }

    /// Replace the whole content and start typing (like `cc` on everything).
    pub fn change_all(&mut self) {
        if self.read_only {
            return;
        }
        self.snapshot();
        self.lines = vec![Vec::new()];
        self.cur = Pos::default();
        self.mode = Mode::Insert;
    }

    /// Scroll the viewport without entering the editor.
    pub fn scroll(&mut self, delta: isize) {
        let h = self.height.max(1);
        let max_top = self.lines.len().saturating_sub(h) as isize;
        self.top = (self.top as isize + delta).clamp(0, max_top.max(0)) as usize;
        let last = self.lines.len() - 1;
        self.cur.y = self.cur.y.clamp(self.top, (self.top + h - 1).min(last));
        self.clamp();
    }

    pub fn half_page(&self) -> isize {
        (self.height.max(2) / 2) as isize
    }

    pub fn goto(&mut self, bottom: bool) {
        self.cur = Pos { y: if bottom { self.lines.len() - 1 } else { 0 }, x: 0 };
        self.want_x = 0;
    }

    pub fn paste(&mut self, text: &str) {
        if self.read_only || matches!(self.mode, Mode::Search) {
            if let Some(s) = self.search_input.as_mut() {
                s.push_str(text);
            }
            return;
        }
        let insert = self.mode == Mode::Insert;
        if !insert {
            self.snapshot();
        }
        let text = if self.single_line { text.replace(['\n', '\r'], " ") } else { text.to_string() };
        self.clamp();
        let end = self.insert_text(self.cur, &text);
        self.cur = end;
        if !insert {
            self.cur.x = self.cur.x.saturating_sub(1);
        }
        self.clamp();
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        match self.mode {
            Mode::Insert => self.key_insert(key),
            Mode::Search => self.key_search(key),
            _ => self.key_normal(key),
        }
    }

    // ---- helpers -------------------------------------------------------

    fn is_visual(&self) -> bool {
        matches!(self.mode, Mode::Visual | Mode::VisualLine)
    }

    fn reset(&mut self) {
        self.count = None;
        self.pre_count = None;
        self.op = None;
        self.pend = None;
    }

    fn clamp(&mut self) {
        self.cur.y = self.cur.y.min(self.lines.len() - 1);
        let len = self.lines[self.cur.y].len();
        let max = if self.mode == Mode::Insert { len } else { len.saturating_sub(1) };
        self.cur.x = self.cur.x.min(max);
    }

    fn first_nonblank(&self, y: usize) -> usize {
        let line = &self.lines[y];
        line.iter().position(|c| !c.is_whitespace()).unwrap_or(line.len().saturating_sub(1))
    }

    fn indent_of(&self, y: usize) -> Vec<char> {
        self.lines[y].iter().take_while(|c| c.is_whitespace()).copied().collect()
    }

    fn snapshot(&mut self) {
        self.undo.push((self.lines.clone(), self.cur));
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn undo(&mut self) {
        while let Some((lines, cur)) = self.undo.pop() {
            if lines != self.lines {
                let old = std::mem::replace(&mut self.lines, lines);
                self.redo.push((old, self.cur));
                self.cur = cur;
                self.clamp();
                return;
            }
        }
    }

    fn redo(&mut self) {
        if let Some((lines, cur)) = self.redo.pop() {
            let old = std::mem::replace(&mut self.lines, lines);
            self.undo.push((old, self.cur));
            self.cur = cur;
            self.clamp();
        }
    }

    fn begin_insert(&mut self, x: usize) {
        if self.read_only {
            return;
        }
        self.snapshot();
        self.mode = Mode::Insert;
        self.cur.x = x;
        self.clamp();
    }

    fn leave_insert(&mut self) {
        self.mode = Mode::Normal;
        self.cur.x = self.cur.x.saturating_sub(1);
        self.clamp();
        self.want_x = self.cur.x;
    }

    fn move_lines(&mut self, delta: isize) {
        let last = self.lines.len() as isize - 1;
        self.cur.y = (self.cur.y as isize + delta).clamp(0, last) as usize;
        self.cur.x = self.want_x;
        self.clamp();
    }

    /// Insert (possibly multi-line) text at `at`, returning the end position.
    fn insert_text(&mut self, at: Pos, text: &str) -> Pos {
        let parts: Vec<Vec<char>> = text.split('\n').map(|l| l.trim_end_matches('\r').chars().collect()).collect();
        let x = at.x.min(self.lines[at.y].len());
        if parts.len() == 1 {
            let n = parts[0].len();
            self.lines[at.y].splice(x..x, parts[0].iter().copied());
            return Pos { y: at.y, x: x + n };
        }
        let tail = self.lines[at.y].split_off(x);
        self.lines[at.y].extend(&parts[0]);
        let last = parts.len() - 1;
        let end_x = parts[last].len();
        for (i, mut part) in parts.into_iter().enumerate().skip(1) {
            if i == last {
                part.extend(&tail);
            }
            self.lines.insert(at.y + i, part);
        }
        Pos { y: at.y + last, x: end_x }
    }

    fn text_range(&self, s: Pos, e: Pos, linewise: bool) -> String {
        if linewise {
            let mut out = String::new();
            for l in &self.lines[s.y..=e.y] {
                out.extend(l.iter());
                out.push('\n');
            }
            return out;
        }
        if s.y == e.y {
            let l = &self.lines[s.y];
            return l[s.x.min(l.len())..e.x.min(l.len())].iter().collect();
        }
        let mut out: String = self.lines[s.y][s.x.min(self.lines[s.y].len())..].iter().collect();
        for l in &self.lines[s.y + 1..e.y] {
            out.push('\n');
            out.extend(l.iter());
        }
        out.push('\n');
        out.extend(self.lines[e.y][..e.x.min(self.lines[e.y].len())].iter());
        out
    }

    fn delete_range(&mut self, s: Pos, e: Pos) {
        let sx = s.x.min(self.lines[s.y].len());
        let ex = e.x.min(self.lines[e.y].len());
        if s.y == e.y {
            if sx < ex {
                self.lines[s.y].drain(sx..ex);
            }
            return;
        }
        let tail: Vec<char> = self.lines[e.y][ex..].to_vec();
        self.lines[s.y].truncate(sx);
        self.lines[s.y].extend(tail);
        self.lines.drain(s.y + 1..=e.y);
    }

    fn operate(&mut self, op: char, a: Pos, b: Pos, kind: MKind) {
        let (s, mut e) = if a <= b { (a, b) } else { (b, a) };
        let linewise = kind == MKind::Line;
        if kind == MKind::Incl {
            e.x = (e.x + 1).min(self.lines[e.y].len());
        }
        let text = self.text_range(s, e, linewise);
        match op {
            'y' => {
                yank(&text, linewise);
                self.cur = if linewise { Pos { y: s.y, x: self.cur.x } } else { s };
            }
            'd' | 'c' => {
                if self.read_only {
                    return;
                }
                self.snapshot();
                yank(&text, linewise);
                if linewise && op == 'c' {
                    let indent = self.indent_of(s.y);
                    let x = indent.len();
                    self.lines.splice(s.y..=e.y, [indent]);
                    self.cur = Pos { y: s.y, x };
                } else if linewise {
                    self.lines.drain(s.y..=e.y);
                    if self.lines.is_empty() {
                        self.lines.push(Vec::new());
                    }
                    self.cur.y = s.y.min(self.lines.len() - 1);
                    self.cur.x = self.first_nonblank(self.cur.y);
                } else {
                    self.delete_range(s, e);
                    self.cur = s;
                }
                if op == 'c' {
                    self.mode = Mode::Insert;
                }
            }
            _ => {}
        }
        self.clamp();
        self.want_x = self.cur.x;
    }

    fn put(&mut self, after: bool, n: usize) {
        let (mut text, mut linewise) = register();
        if text.is_empty() || self.read_only {
            return;
        }
        if self.single_line {
            text = text.trim_end_matches('\n').replace(['\n', '\r'], " ");
            linewise = false;
        }
        self.snapshot();
        let text = text.repeat(n.max(1));
        if linewise {
            let new: Vec<Vec<char>> = text.trim_end_matches('\n').split('\n').map(|l| l.chars().collect()).collect();
            let at = if after { self.cur.y + 1 } else { self.cur.y };
            self.lines.splice(at..at, new);
            self.cur = Pos { y: at, x: 0 };
            self.cur.x = self.first_nonblank(at);
        } else {
            let len = self.lines[self.cur.y].len();
            let x = if after { (self.cur.x + 1).min(len) } else { self.cur.x.min(len) };
            let end = self.insert_text(Pos { y: self.cur.y, x }, &text);
            self.cur = Pos { y: end.y, x: end.x.saturating_sub(1) };
        }
        self.clamp();
        self.want_x = self.cur.x;
    }

    // ---- motions -------------------------------------------------------

    fn word_fwd(&self, mut p: Pos, big: bool) -> Pos {
        let line = &self.lines[p.y];
        if p.x < line.len() {
            let c0 = class(line[p.x], big);
            while c0 != 0 && p.x < line.len() && class(line[p.x], big) == c0 {
                p.x += 1;
            }
        }
        loop {
            let line = &self.lines[p.y];
            while p.x < line.len() && class(line[p.x], big) == 0 {
                p.x += 1;
            }
            if p.x < line.len() || p.y + 1 >= self.lines.len() {
                return p;
            }
            p.y += 1;
            p.x = 0;
            if self.lines[p.y].is_empty() {
                return p;
            }
        }
    }

    fn word_back(&self, mut p: Pos, big: bool) -> Pos {
        loop {
            if p.x == 0 {
                if p.y == 0 {
                    return p;
                }
                p.y -= 1;
                p.x = self.lines[p.y].len();
                if p.x == 0 {
                    return p;
                }
            }
            let line = &self.lines[p.y];
            p.x = p.x.min(line.len());
            while p.x > 0 && class(line[p.x - 1], big) == 0 {
                p.x -= 1;
            }
            if p.x == 0 {
                continue;
            }
            let c0 = class(line[p.x - 1], big);
            while p.x > 0 && class(line[p.x - 1], big) == c0 {
                p.x -= 1;
            }
            return p;
        }
    }

    fn word_end(&self, mut p: Pos, big: bool) -> Pos {
        p.x += 1;
        loop {
            let line = &self.lines[p.y];
            while p.x < line.len() && class(line[p.x], big) == 0 {
                p.x += 1;
            }
            if p.x < line.len() {
                break;
            }
            if p.y + 1 >= self.lines.len() {
                p.x = line.len().saturating_sub(1);
                return p;
            }
            p.y += 1;
            p.x = 0;
        }
        let line = &self.lines[p.y];
        let c0 = class(line[p.x], big);
        while p.x + 1 < line.len() && class(line[p.x + 1], big) == c0 {
            p.x += 1;
        }
        p
    }

    fn find_char(&self, kind: char, ch: char, n: usize) -> Option<Pos> {
        let line = &self.lines[self.cur.y];
        let y = self.cur.y;
        if kind == 'f' || kind == 't' {
            let i = (self.cur.x + 1..line.len()).filter(|&i| line[i] == ch).nth(n - 1)?;
            Some(Pos { y, x: if kind == 't' { i - 1 } else { i } })
        } else {
            let i = (0..self.cur.x.min(line.len())).rev().filter(|&i| line[i] == ch).nth(n - 1)?;
            Some(Pos { y, x: if kind == 'T' { i + 1 } else { i } })
        }
    }

    fn find_close(&self, from: Pos, open: char, close: char) -> Option<Pos> {
        let mut depth = 0usize;
        for y in from.y..self.lines.len() {
            let start = if y == from.y { from.x } else { 0 };
            for x in start..self.lines[y].len() {
                let ch = self.lines[y][x];
                if ch == open {
                    depth += 1;
                } else if ch == close {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(Pos { y, x });
                    }
                }
            }
        }
        None
    }

    /// Nearest unmatched `open` at or before `from`.
    fn find_open(&self, from: Pos, open: char, close: char) -> Option<Pos> {
        let mut depth = 0usize;
        for y in (0..=from.y).rev() {
            let line = &self.lines[y];
            if line.is_empty() {
                continue;
            }
            let end = if y == from.y { from.x.min(line.len() - 1) } else { line.len() - 1 };
            for x in (0..=end).rev() {
                let ch = line[x];
                if ch == close && !(y == from.y && x == from.x) {
                    depth += 1;
                } else if ch == open {
                    if depth == 0 {
                        return Some(Pos { y, x });
                    }
                    depth -= 1;
                }
            }
        }
        None
    }

    fn match_bracket(&self) -> Option<Pos> {
        let line = &self.lines[self.cur.y];
        let x = (self.cur.x..line.len()).find(|&i| "(){}[]".contains(line[i]))?;
        let p = Pos { y: self.cur.y, x };
        let (open, close) = bracket_pair(line[x])?;
        if line[x] == open {
            self.find_close(p, open, close)
        } else {
            self.find_open(p, open, close)
        }
    }

    fn search_next(&self, forward: bool) -> Option<Pos> {
        let pat: Vec<char> = self.search.chars().collect();
        if pat.is_empty() {
            return None;
        }
        let ci = !pat.iter().any(|c| c.is_uppercase());
        let n = self.lines.len();
        for i in 0..=n {
            if forward {
                let y = (self.cur.y + i) % n;
                let from = if i == 0 { self.cur.x + 1 } else { 0 };
                if let Some(x) = find_from(&self.lines[y], &pat, from, ci) {
                    return Some(Pos { y, x });
                }
            } else {
                let y = (self.cur.y + n - (i % n)) % n;
                let line = &self.lines[y];
                let limit = if i == 0 { self.cur.x } else { line.len() };
                if let Some(x) = (0..limit.min(line.len())).rev().find(|&x| matches_at(line, &pat, x, ci)) {
                    return Some(Pos { y, x });
                }
            }
        }
        None
    }

    /// Returns the range covered by a text object; the end is exclusive unless linewise.
    fn text_object(&self, c: char, inner: bool) -> Option<(Pos, Pos, MKind)> {
        let y = self.cur.y;
        let line = &self.lines[y];
        let len = line.len();
        match c {
            'w' | 'W' => {
                if len == 0 {
                    return None;
                }
                let big = c == 'W';
                let x = self.cur.x.min(len - 1);
                let c0 = class(line[x], big);
                let mut s = x;
                while s > 0 && class(line[s - 1], big) == c0 {
                    s -= 1;
                }
                let mut e = x + 1;
                while e < len && class(line[e], big) == c0 {
                    e += 1;
                }
                if !inner {
                    let e0 = e;
                    while e < len && line[e].is_whitespace() {
                        e += 1;
                    }
                    if e == e0 {
                        while s > 0 && line[s - 1].is_whitespace() {
                            s -= 1;
                        }
                    }
                }
                Some((Pos { y, x: s }, Pos { y, x: e }, MKind::Excl))
            }
            '"' | '\'' | '`' => {
                let qs: Vec<usize> = (0..len).filter(|&i| line[i] == c && (i == 0 || line[i - 1] != '\\')).collect();
                let pair = qs.chunks(2).find(|p| p.len() == 2 && p[1] >= self.cur.x)?;
                let (a, b) = (pair[0], pair[1]);
                let (s, e) = if inner { (a + 1, b) } else { (a, b + 1) };
                Some((Pos { y, x: s }, Pos { y, x: e }, MKind::Excl))
            }
            _ => {
                let (open, close) = bracket_pair(c)?;
                let o = self.find_open(self.cur, open, close)?;
                let cl = self.find_close(o, open, close)?;
                if !inner {
                    return Some((o, Pos { y: cl.y, x: cl.x + 1 }, MKind::Excl));
                }
                let open_at_eol = o.x + 1 == self.lines[o.y].len();
                let close_alone = self.lines[cl.y][..cl.x].iter().all(|c| c.is_whitespace());
                if open_at_eol && close_alone && cl.y > o.y {
                    if cl.y == o.y + 1 {
                        return None;
                    }
                    return Some((Pos { y: o.y + 1, x: 0 }, Pos { y: cl.y - 1, x: 0 }, MKind::Line));
                }
                Some((Pos { y: o.y, x: o.x + 1 }, cl, MKind::Excl))
            }
        }
    }

    fn apply_motion(&mut self, target: Pos, kind: MKind, vertical: bool) {
        if let Some(op) = self.op {
            let from = self.cur;
            self.operate(op, from, target, kind);
        } else {
            self.cur = target;
            self.clamp();
            if !vertical {
                self.want_x = self.cur.x;
            }
        }
        self.reset();
    }

    /// Returns true if `c` was a motion (or the start of one).
    fn try_motion(&mut self, c: char, n: usize, has_count: bool) -> bool {
        let cur = self.cur;
        let last = self.lines.len() - 1;
        let len = self.lines[cur.y].len();
        let (target, kind) = match c {
            'g' => {
                self.pend = Some(Pend::G);
                return true;
            }
            'f' | 't' | 'F' | 'T' => {
                self.pend = Some(Pend::Find(c));
                return true;
            }
            ';' => {
                let found = self.last_find.and_then(|(k, ch)| Some((k, self.find_char(k, ch, n)?)));
                match found {
                    Some((k, p)) => (p, if k.is_lowercase() { MKind::Incl } else { MKind::Excl }),
                    None => {
                        self.reset();
                        return true;
                    }
                }
            }
            'h' => (Pos { y: cur.y, x: cur.x.saturating_sub(n) }, MKind::Excl),
            'l' | ' ' => (Pos { y: cur.y, x: (cur.x + n).min(len) }, MKind::Excl),
            'j' => (Pos { y: (cur.y + n).min(last), x: self.want_x }, MKind::Line),
            'k' => (Pos { y: cur.y.saturating_sub(n), x: self.want_x }, MKind::Line),
            'w' | 'W' => {
                let big = c == 'W';
                if self.op == Some('c') && cur.x < len && !self.lines[cur.y][cur.x].is_whitespace() {
                    // `cw` behaves like `ce`
                    let mut p = Pos { y: cur.y, x: cur.x.saturating_sub(1) };
                    if cur.x == 0 {
                        // word_end advances first; emulate starting just before the cursor
                        let c0 = class(self.lines[cur.y][0], big);
                        p = cur;
                        while p.x + 1 < len && class(self.lines[cur.y][p.x + 1], big) == c0 {
                            p.x += 1;
                        }
                        for _ in 1..n {
                            p = self.word_end(p, big);
                        }
                    } else {
                        for _ in 0..n {
                            p = self.word_end(p, big);
                        }
                    }
                    (p, MKind::Incl)
                } else {
                    let mut p = cur;
                    for _ in 0..n {
                        p = self.word_fwd(p, big);
                    }
                    if self.op.is_some() && p.y > cur.y {
                        p = Pos { y: cur.y, x: len };
                    }
                    (p, MKind::Excl)
                }
            }
            'b' | 'B' => {
                let mut p = cur;
                for _ in 0..n {
                    p = self.word_back(p, c == 'B');
                }
                (p, MKind::Excl)
            }
            'e' | 'E' => {
                let mut p = cur;
                for _ in 0..n {
                    p = self.word_end(p, c == 'E');
                }
                (p, MKind::Incl)
            }
            '0' => (Pos { y: cur.y, x: 0 }, MKind::Excl),
            '^' | '_' => (Pos { y: cur.y, x: self.first_nonblank(cur.y) }, MKind::Excl),
            '$' => {
                let y = (cur.y + n - 1).min(last);
                (Pos { y, x: self.lines[y].len().saturating_sub(1) }, MKind::Incl)
            }
            'G' => (Pos { y: if has_count { (n - 1).min(last) } else { last }, x: 0 }, MKind::Line),
            '%' => match self.match_bracket() {
                Some(p) => (p, MKind::Incl),
                None => {
                    self.reset();
                    return true;
                }
            },
            '}' => {
                let mut y = cur.y;
                for _ in 0..n {
                    y = (y + 1).min(last);
                    while y < last && !self.lines[y].is_empty() {
                        y += 1;
                    }
                }
                let x = if self.lines[y].is_empty() { 0 } else { self.lines[y].len() };
                (Pos { y, x }, MKind::Excl)
            }
            '{' => {
                let mut y = cur.y;
                for _ in 0..n {
                    y = y.saturating_sub(1);
                    while y > 0 && !self.lines[y].is_empty() {
                        y -= 1;
                    }
                }
                (Pos { y, x: 0 }, MKind::Excl)
            }
            'n' | 'N' => match self.search_next(c == 'n') {
                Some(p) => (p, MKind::Excl),
                None => {
                    self.reset();
                    return true;
                }
            },
            _ => return false,
        };
        self.apply_motion(target, kind, matches!(c, 'j' | 'k'));
        true
    }

    // ---- key handling --------------------------------------------------

    fn escape(&mut self) -> Outcome {
        if self.count.is_some() || self.pre_count.is_some() || self.op.is_some() || self.pend.is_some() {
            self.reset();
            Outcome::Consumed
        } else if self.is_visual() {
            self.mode = Mode::Normal;
            self.clamp();
            Outcome::Consumed
        } else {
            Outcome::Exit
        }
    }

    fn key_normal(&mut self, key: KeyEvent) -> Outcome {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            let half = self.half_page();
            match key.code {
                KeyCode::Char('d') => self.move_lines(half),
                KeyCode::Char('u') => self.move_lines(-half),
                KeyCode::Char('f') => self.move_lines(half * 2),
                KeyCode::Char('b') => self.move_lines(-half * 2),
                KeyCode::Char('r') => self.redo(),
                KeyCode::Char('c') | KeyCode::Char('[') => return self.escape(),
                _ => {}
            }
            return Outcome::Consumed;
        }
        let c = match key.code {
            KeyCode::Esc => return self.escape(),
            KeyCode::Enter if self.single_line => return Outcome::Submit,
            KeyCode::Tab if self.single_line => return Outcome::Next,
            KeyCode::BackTab if self.single_line => return Outcome::Prev,
            KeyCode::Enter | KeyCode::Down => 'j',
            KeyCode::Up => 'k',
            KeyCode::Left | KeyCode::Backspace => 'h',
            KeyCode::Right => 'l',
            KeyCode::Home => '0',
            KeyCode::End => '$',
            KeyCode::Char(c) => c,
            _ => return Outcome::Consumed,
        };
        let visual = self.is_visual();
        let has_count = self.count.is_some() || self.pre_count.is_some();
        let n = self.count.unwrap_or(1) * self.pre_count.unwrap_or(1);

        if let Some(p) = self.pend.take() {
            match p {
                Pend::Replace => {
                    let len = self.lines[self.cur.y].len();
                    if !self.read_only && self.cur.x + n <= len {
                        self.snapshot();
                        for i in 0..n {
                            self.lines[self.cur.y][self.cur.x + i] = c;
                        }
                        self.cur.x += n - 1;
                    }
                    self.reset();
                }
                Pend::Find(kind) => {
                    self.last_find = Some((kind, c));
                    match self.find_char(kind, c, n) {
                        Some(t) => {
                            let k = if kind.is_lowercase() { MKind::Incl } else { MKind::Excl };
                            self.apply_motion(t, k, false);
                        }
                        None => self.reset(),
                    }
                }
                Pend::Obj(inner) => {
                    if let Some((s, e, kind)) = self.text_object(c, inner) {
                        if visual {
                            self.anchor = s;
                            if kind == MKind::Line {
                                self.mode = Mode::VisualLine;
                                self.cur = e;
                            } else {
                                self.cur = Pos { y: e.y, x: e.x.saturating_sub(1) };
                            }
                        } else if let Some(op) = self.op {
                            self.operate(op, s, e, kind);
                        }
                    }
                    self.reset();
                }
                Pend::G => {
                    if c == 'g' {
                        let last = self.lines.len() - 1;
                        let y = if has_count { (n - 1).min(last) } else { 0 };
                        self.apply_motion(Pos { y, x: 0 }, MKind::Line, false);
                    } else {
                        self.reset();
                    }
                }
            }
            return Outcome::Consumed;
        }

        if c.is_ascii_digit() && (c != '0' || self.count.is_some()) {
            let d = c as usize - '0' as usize;
            self.count = Some((self.count.unwrap_or(0) * 10 + d).min(99_999));
            return Outcome::Consumed;
        }

        if let Some(op) = self.op {
            if c == op {
                let end = (self.cur.y + n - 1).min(self.lines.len() - 1);
                let from = self.cur;
                self.operate(op, from, Pos { y: end, x: 0 }, MKind::Line);
                self.reset();
                return Outcome::Consumed;
            }
        }
        if (self.op.is_some() || visual) && (c == 'i' || c == 'a') {
            self.pend = Some(Pend::Obj(c == 'i'));
            return Outcome::Consumed;
        }
        if !(visual && c == ' ') && self.try_motion(c, n, has_count) {
            return Outcome::Consumed;
        }
        if self.op.is_some() {
            self.reset();
        } else if visual {
            self.visual_cmd(c);
        } else {
            self.normal_cmd(c, n);
        }
        Outcome::Consumed
    }

    fn selection(&self) -> (Pos, Pos, MKind) {
        let (s, e) = if self.anchor <= self.cur { (self.anchor, self.cur) } else { (self.cur, self.anchor) };
        (s, e, if self.mode == Mode::VisualLine { MKind::Line } else { MKind::Incl })
    }

    fn visual_cmd(&mut self, c: char) {
        let (s, e, kind) = self.selection();
        match c {
            'v' => self.mode = if self.mode == Mode::Visual { Mode::Normal } else { Mode::Visual },
            'V' => self.mode = if self.mode == Mode::VisualLine { Mode::Normal } else { Mode::VisualLine },
            'o' => std::mem::swap(&mut self.anchor, &mut self.cur),
            'y' => {
                self.mode = Mode::Normal;
                self.operate('y', s, e, kind);
            }
            'd' | 'x' | 'c' | 's' if !self.read_only => {
                self.mode = Mode::Normal;
                self.operate(if c == 'c' || c == 's' { 'c' } else { 'd' }, s, e, kind);
            }
            'p' | 'P' if !self.read_only => {
                let (text, linewise) = register();
                self.mode = Mode::Normal;
                let lines_before = self.lines.len();
                let line_len = self.lines[s.y].len();
                self.operate('d', s, e, kind);
                let at_end = if kind == MKind::Line {
                    s.y + (lines_before - self.lines.len()) >= lines_before && lines_before > e.y - s.y + 1
                } else {
                    s.y == e.y && e.x + 1 >= line_len && s.x > 0
                };
                yank(&text, linewise);
                self.put(at_end, 1);
            }
            '/' => {
                self.mode = Mode::Search;
                self.search_input = Some(String::new());
            }
            _ => {}
        }
        self.reset();
    }

    fn normal_cmd(&mut self, c: char, n: usize) {
        let len = self.lines[self.cur.y].len();
        match c {
            'v' => {
                self.anchor = self.cur;
                self.mode = Mode::Visual;
            }
            'V' => {
                self.anchor = self.cur;
                self.mode = Mode::VisualLine;
            }
            'y' => {
                self.pre_count = self.count.take();
                self.op = Some('y');
                return;
            }
            'Y' => {
                let end = (self.cur.y + n - 1).min(self.lines.len() - 1);
                self.operate('y', self.cur, Pos { y: end, x: 0 }, MKind::Line);
            }
            '/' => {
                self.mode = Mode::Search;
                self.search_input = Some(String::new());
            }
            _ if self.read_only => {}
            'u' => (0..n).for_each(|_| self.undo()),
            'd' | 'c' => {
                self.pre_count = self.count.take();
                self.op = Some(c);
                return;
            }
            'i' => self.begin_insert(self.cur.x),
            'a' => self.begin_insert((self.cur.x + 1).min(len)),
            'I' => self.begin_insert(self.first_nonblank(self.cur.y).min(len)),
            'A' => self.begin_insert(len),
            'o' | 'O' if self.single_line => self.begin_insert(len),
            'o' | 'O' => {
                self.snapshot();
                let indent = self.indent_of(self.cur.y);
                let at = if c == 'o' { self.cur.y + 1 } else { self.cur.y };
                self.mode = Mode::Insert;
                self.cur = Pos { y: at, x: indent.len() };
                self.lines.insert(at, indent);
            }
            'x' | 's' => {
                self.op = Some(if c == 'x' { 'd' } else { 'c' });
                self.try_motion('l', n, true);
            }
            'X' => {
                self.op = Some('d');
                self.try_motion('h', n, true);
            }
            'D' | 'C' => {
                self.op = Some(if c == 'D' { 'd' } else { 'c' });
                self.try_motion('$', 1, false);
            }
            'S' => self.operate('c', self.cur, self.cur, MKind::Line),
            'p' | 'P' => self.put(c == 'p', n),
            'r' => {
                self.pend = Some(Pend::Replace);
                return;
            }
            '~' => {
                self.snapshot();
                let y = self.cur.y;
                for x in self.cur.x..(self.cur.x + n).min(len) {
                    let ch = self.lines[y][x];
                    self.lines[y][x] = if ch.is_uppercase() {
                        ch.to_lowercase().next().unwrap_or(ch)
                    } else {
                        ch.to_uppercase().next().unwrap_or(ch)
                    };
                }
                self.cur.x = (self.cur.x + n).min(len.saturating_sub(1));
            }
            'J' if self.cur.y + 1 < self.lines.len() => {
                self.snapshot();
                let next = self.lines.remove(self.cur.y + 1);
                let line = &mut self.lines[self.cur.y];
                while line.last().is_some_and(|c| c.is_whitespace()) {
                    line.pop();
                }
                self.cur.x = line.len();
                let rest: Vec<char> = next.into_iter().skip_while(|c| c.is_whitespace()).collect();
                if !line.is_empty() && !rest.is_empty() {
                    line.push(' ');
                }
                line.extend(rest);
                self.clamp();
            }
            _ => {}
        }
        self.reset();
    }

    fn key_insert(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Pos { y, x } = self.cur;
        let len = self.lines[y].len();
        match key.code {
            KeyCode::Esc => self.leave_insert(),
            KeyCode::Char('c') | KeyCode::Char('[') if ctrl => self.leave_insert(),
            KeyCode::Char('w') if ctrl => {
                if x == 0 {
                    self.backspace();
                } else {
                    let mut s = self.word_back(self.cur, false);
                    if s.y != y {
                        s = Pos { y, x: 0 };
                    }
                    self.lines[y].drain(s.x..x);
                    self.cur.x = s.x;
                }
            }
            KeyCode::Char('u') if ctrl => {
                self.lines[y].drain(..x);
                self.cur.x = 0;
            }
            KeyCode::Char('h') if ctrl => self.backspace(),
            KeyCode::Enter if self.single_line => {
                self.leave_insert();
                return Outcome::Submit;
            }
            KeyCode::Tab if self.single_line => {
                self.leave_insert();
                return Outcome::Next;
            }
            KeyCode::BackTab if self.single_line => {
                self.leave_insert();
                return Outcome::Prev;
            }
            KeyCode::Enter => self.newline(),
            KeyCode::Tab => {
                self.lines[y].splice(x..x, [' ', ' ']);
                self.cur.x += 2;
            }
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => {
                if x < len {
                    self.lines[y].remove(x);
                } else if y + 1 < self.lines.len() {
                    let next = self.lines.remove(y + 1);
                    self.lines[y].extend(next);
                }
            }
            KeyCode::Left => self.cur.x = x.saturating_sub(1),
            KeyCode::Right => self.cur.x = (x + 1).min(len),
            KeyCode::Up => {
                self.cur.y = y.saturating_sub(1);
                self.clamp();
            }
            KeyCode::Down => {
                self.cur.y = y + 1;
                self.clamp();
            }
            KeyCode::Home => self.cur.x = 0,
            KeyCode::End => self.cur.x = len,
            KeyCode::Char(c) if !ctrl => {
                self.lines[y].insert(x, c);
                self.cur.x += 1;
            }
            _ => {}
        }
        Outcome::Consumed
    }

    fn backspace(&mut self) {
        let Pos { y, x } = self.cur;
        if x > 0 {
            self.lines[y].remove(x - 1);
            self.cur.x -= 1;
        } else if y > 0 {
            let line = self.lines.remove(y);
            self.cur = Pos { y: y - 1, x: self.lines[y - 1].len() };
            self.lines[y - 1].extend(line);
        }
    }

    fn newline(&mut self) {
        let Pos { y, x } = self.cur;
        let indent = self.indent_of(y);
        let indent: Vec<char> = indent.into_iter().take(x).collect();
        let tail = self.lines[y].split_off(x);
        let opener = x > 0 && matches!(self.lines[y][x - 1], '{' | '[');
        let closer = matches!(tail.first(), Some('}') | Some(']'));
        let mut new = indent.clone();
        if opener {
            new.extend([' ', ' ']);
        }
        self.cur = Pos { y: y + 1, x: new.len() };
        if opener && closer {
            let mut closing = indent;
            closing.extend(tail);
            self.lines.insert(y + 1, closing);
        } else {
            new.extend(tail);
        }
        self.lines.insert(y + 1, new);
    }

    fn key_search(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(input) = self.search_input.as_mut() else {
            self.mode = Mode::Normal;
            return Outcome::Consumed;
        };
        let mut done = false;
        match key.code {
            KeyCode::Esc => done = true,
            KeyCode::Char('c') if ctrl => done = true,
            KeyCode::Enter => {
                self.search = input.clone();
                done = true;
                if let Some(p) = self.search_next(true) {
                    self.cur = p;
                }
            }
            KeyCode::Backspace => done = input.pop().is_none(),
            KeyCode::Char(c) if !ctrl => input.push(c),
            _ => {}
        }
        if done {
            self.search_input = None;
            self.mode = Mode::Normal;
            self.clamp();
            self.want_x = self.cur.x;
        }
        Outcome::Consumed
    }

    // ---- rendering -----------------------------------------------------

    /// Draw the buffer; returns the screen position of the cursor when active.
    pub fn render(&mut self, buf: &mut Buffer, area: Rect, active: bool) -> Option<(u16, u16)> {
        if area.width == 0 || area.height == 0 {
            return None;
        }
        self.height = area.height as usize;
        let mut gutter = 0;
        if self.line_numbers {
            gutter = self.lines.len().to_string().len().max(2) + 1;
            if gutter + 4 > area.width as usize {
                gutter = 0;
            }
        }
        let width = (area.width as usize - gutter).max(1);
        self.cur.y = self.cur.y.min(self.lines.len() - 1);
        if self.cur.y < self.top {
            self.top = self.cur.y;
        }
        if self.cur.y >= self.top + self.height {
            self.top = self.cur.y + 1 - self.height;
        }
        if self.cur.x < self.left {
            self.left = self.cur.x;
        }
        if self.cur.x >= self.left + width {
            self.left = self.cur.x + 1 - width;
        }
        let sel = (active && self.is_visual()).then(|| self.selection());
        let pat: Vec<char> = self.search.chars().collect();
        let ci = !pat.iter().any(|c| c.is_uppercase());
        let dim = Style::new().fg(Color::DarkGray);
        let x0 = area.x + gutter as u16;
        let mut cursor = None;

        for row in 0..self.height {
            let y = self.top + row;
            let sy = area.y + row as u16;
            if y >= self.lines.len() {
                if self.line_numbers {
                    buf.set_string(area.x, sy, "~", dim);
                }
                continue;
            }
            if gutter > 0 {
                let style = if active && y == self.cur.y { Style::new().fg(Color::Yellow) } else { dim };
                buf.set_string(area.x, sy, format!("{:>w$} ", y + 1, w = gutter - 1), style);
            }
            let line = &self.lines[y];
            let mut styles = match self.syntax {
                Syntax::Json => json_styles(line),
                Syntax::Env => env_styles(line),
                Syntax::Plain => vec![Style::new(); line.len()],
            };
            if !self.read_only {
                template_styles(line, &mut styles);
            }
            if !pat.is_empty() {
                let mut x = 0;
                while let Some(m) = find_from(line, &pat, x, ci) {
                    for s in &mut styles[m..m + pat.len()] {
                        *s = Style::new().fg(Color::Black).bg(Color::Yellow);
                    }
                    x = m + pat.len();
                }
            }
            let line_selected = sel.is_some_and(|(s, e, _)| y >= s.y && y <= e.y);
            if let Some((s, e, kind)) = sel {
                for (x, st) in styles.iter_mut().enumerate() {
                    let p = Pos { y, x };
                    if line_selected && (kind == MKind::Line || (p >= s && p <= e)) {
                        *st = st.add_modifier(Modifier::REVERSED);
                    }
                }
            }
            let mut col = 0usize;
            for i in self.left..line.len() {
                let ch = if line[i] == '\t' { ' ' } else { line[i] };
                let w = ch.width().unwrap_or(0);
                if y == self.cur.y && i == self.cur.x {
                    cursor = Some((x0 + col.min(width - 1) as u16, sy));
                }
                if w == 0 {
                    continue;
                }
                if col + w > width {
                    break;
                }
                buf.set_string(x0 + col as u16, sy, ch.encode_utf8(&mut [0; 4]), styles[i]);
                col += w;
            }
            if y == self.cur.y && self.cur.x >= line.len() {
                cursor = Some((x0 + col.min(width - 1) as u16, sy));
            }
            if line_selected && line.is_empty() {
                buf.set_string(x0, sy, " ", Style::new().add_modifier(Modifier::REVERSED));
            }
        }
        if active && self.mode != Mode::Search {
            cursor
        } else {
            None
        }
    }
}

fn json_styles(line: &[char]) -> Vec<Style> {
    let len = line.len();
    let mut st = vec![Style::new(); len];
    let mut i = 0;
    while i < len {
        let c = line[i];
        if c == '"' {
            let mut j = i + 1;
            while j < len && line[j] != '"' {
                j += if line[j] == '\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(len);
            let is_key = line[end..].iter().find(|c| !c.is_whitespace()) == Some(&':');
            let color = if is_key { Color::Cyan } else { Color::Green };
            st[i..end].fill(Style::new().fg(color));
            i = end;
        } else if c.is_ascii_digit() || c == '-' {
            let mut j = i + 1;
            while j < len && (line[j].is_ascii_alphanumeric() || matches!(line[j], '.' | '+' | '-')) {
                j += 1;
            }
            st[i..j].fill(Style::new().fg(Color::Magenta));
            i = j;
        } else if c.is_ascii_alphabetic() {
            let mut j = i;
            while j < len && line[j].is_ascii_alphabetic() {
                j += 1;
            }
            let word: String = line[i..j].iter().collect();
            if matches!(word.as_str(), "true" | "false" | "null") {
                st[i..j].fill(Style::new().fg(Color::Yellow));
            }
            i = j;
        } else {
            if matches!(c, '{' | '}' | '[' | ']' | ',' | ':') {
                st[i] = Style::new().fg(Color::DarkGray);
            }
            i += 1;
        }
    }
    st
}

/// `KEY=VALUE` lines with `#` comments.
fn env_styles(line: &[char]) -> Vec<Style> {
    let mut st = vec![Style::new(); line.len()];
    if line.iter().find(|c| !c.is_whitespace()) == Some(&'#') {
        st.fill(Style::new().fg(Color::DarkGray));
    } else if let Some(eq) = line.iter().position(|c| *c == '=') {
        st[..eq].fill(Style::new().fg(Color::Cyan));
        st[eq] = Style::new().fg(Color::DarkGray);
    }
    st
}

/// Highlight `{{variable}}` placeholders.
fn template_styles(line: &[char], styles: &mut [Style]) {
    let mut i = 0;
    while i + 1 < line.len() {
        if line[i] == '{' && line[i + 1] == '{' {
            if let Some(end) = (i + 2..line.len() - 1).find(|&j| line[j] == '}' && line[j + 1] == '}') {
                styles[i..end + 2].fill(Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD));
                i = end + 2;
                continue;
            }
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, keys: &str) -> Vim {
        let mut v = Vim::new(false);
        v.set_text(text);
        feed(&mut v, keys);
        v
    }

    fn feed(v: &mut Vim, keys: &str) {
        for c in keys.chars() {
            let code = match c {
                '\x1b' => KeyCode::Esc,
                '\n' => KeyCode::Enter,
                c => KeyCode::Char(c),
            };
            v.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
    }

    #[test]
    fn replace_text_is_undoable() {
        let mut v = run("old", "");
        v.replace_text("new\nlines");
        assert_eq!(v.text(), "new\nlines");
        feed(&mut v, "u");
        assert_eq!(v.text(), "old");
    }

    #[test]
    fn words_and_operators() {
        assert_eq!(run("foo bar baz", "dw").text(), "bar baz");
        assert_eq!(run("foo bar baz", "wdw").text(), "foo baz");
        assert_eq!(run("foo bar baz", "2dw").text(), "baz");
        assert_eq!(run("foo bar baz", "d2w").text(), "baz");
        assert_eq!(run("foo bar baz", "wcwxy\x1b").text(), "foo xy baz");
        assert_eq!(run("foo bar baz", "cwxy\x1b").text(), "xy bar baz");
        assert_eq!(run("foo bar baz", "$x").text(), "foo bar ba");
        assert_eq!(run("foo bar baz", "wD").text(), "foo ");
        assert_eq!(run("foo bar", "eas\x1b").text(), "foos bar");
        assert_eq!(run("foo.bar baz", "dE").text(), " baz");
        assert_eq!(run("foo bar", "$bde").text(), "foo ");
    }

    #[test]
    fn lines_undo_put() {
        assert_eq!(run("a\nb\nc", "jdd").text(), "a\nc");
        assert_eq!(run("a\nb\nc", "ddjp").text(), "b\nc\na");
        assert_eq!(run("a\nb\nc", "yyjP").text(), "a\na\nb\nc");
        assert_eq!(run("a\nb\nc", "dGu").text(), "a\nb\nc");
        assert_eq!(run("a\nb\nc", "jdgg").text(), "c");
        assert_eq!(run("ab", "xp").text(), "ba");
        assert_eq!(run("a\nb", "ox\x1bOy\x1b").text(), "a\ny\nx\nb");
        assert_eq!(run("a b\nc", "J").text(), "a b c");
        assert_eq!(run("abc", "ix\x1buu").text(), "abc");
        assert_eq!(run("a\nb\nc", "Vjd").text(), "c");
        assert_eq!(run("hello", "lvly").text(), "hello");
        assert_eq!(run("hello world", "vex").text(), " world");
    }

    #[test]
    fn text_objects() {
        assert_eq!(run("say \"hi there\" ok", "fhci\"yo\x1b").text(), "say \"yo\" ok");
        assert_eq!(run("f(a, b)", "fadi(").text(), "f()");
        assert_eq!(run("f(a, b)", "fada(").text(), "f");
        assert_eq!(run("foo bar baz", "wdiw").text(), "foo  baz");
        assert_eq!(run("foo bar baz", "wdaw").text(), "foo baz");
        assert_eq!(run("{\n  \"a\": 1,\n  \"b\": 2\n}", "jdi{").text(), "{\n}");
        assert_eq!(run("{\"a\": [1, 2]}", "f1ci[9\x1b").text(), "{\"a\": [9]}");
        assert_eq!(run("a (b) c", "%x").text(), "a (b c");
    }

    #[test]
    fn find_search_insert() {
        assert_eq!(run("a,b,c", "f,x;x").text(), "abc");
        assert_eq!(run("a,b,c", "dt,").text(), ",b,c");
        assert_eq!(run("one\ntwo\nthree", "/t\ndd").text(), "one\nthree");
        assert_eq!(run("one\ntwo\nthree", "/t\nndd").text(), "one\ntwo");
        assert_eq!(run("{}", "a\n\"k\": 1\x1b").text(), "{\n  \"k\": 1\n}");
        assert_eq!(run("abc", "rxlry").text(), "xyc");
        assert_eq!(run("abc", "~~").text(), "ABc");
    }

    #[test]
    fn single_line() {
        let mut v = Vim::new(true);
        v.set_text("http://x");
        v.insert_at_end();
        feed(&mut v, "/y");
        assert_eq!(v.text(), "http://x/y");
        let out = v.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(out, Outcome::Submit);
        assert_eq!(v.mode, Mode::Normal);
        assert_eq!(v.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)), Outcome::Exit);
    }

    #[test]
    fn env_completion() {
        let mut v = Vim::new(true);
        v.set_text("");
        v.insert_at_end();
        feed(&mut v, "@base_u");
        assert_eq!(v.completion_prefix().as_deref(), Some("base_u"));
        v.complete("base_url");
        feed(&mut v, "/get");
        assert_eq!(v.text(), "{{base_url}}/get");
        assert_eq!(v.completion_prefix(), None);
        feed(&mut v, " me@ex");
        assert_eq!(v.completion_prefix(), None);
    }
}
