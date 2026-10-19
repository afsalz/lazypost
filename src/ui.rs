//! Rendering. Every pane shows its hotkey number in the border, btop-style.

use crate::{
    app::{App, Confirm, Edit, Pane, Popup, PromptKind, ReqTab, RespTab, TableCur},
    config::Action,
    model::{AuthKind, KV, METHODS},
    vim::{Mode, Syntax, Vim},
};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, List, ListItem, Paragraph},
    Frame,
};
use std::cell::Cell;

const DIM: Color = Color::DarkGray;

thread_local! {
    /// The theme color, set from the current mode at the start of every frame.
    static ACCENT: Cell<Color> = const { Cell::new(Color::Cyan) };
}

fn accent() -> Color {
    ACCENT.get()
}

/// The status-line label and theme color for the current input mode.
fn mode_theme(app: &App) -> (&'static str, Color) {
    match app.edit.map(|e| app.vim(e).mode) {
        None => ("NAV", Color::Cyan),
        Some(Mode::Insert) => ("INSERT", Color::Yellow),
        Some(Mode::Visual) => ("VISUAL", Color::Magenta),
        Some(Mode::VisualLine) => ("V-LINE", Color::Magenta),
        Some(_) => ("NORMAL", Color::Green),
    }
}

type Cursor = Option<(u16, u16)>;

fn dim() -> Style {
    Style::new().fg(DIM)
}

fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

fn method_color(method: &str) -> Color {
    match method {
        "GET" => Color::Green,
        "POST" => Color::Yellow,
        "PUT" => Color::Blue,
        "PATCH" => Color::Magenta,
        "DELETE" => Color::Red,
        _ => Color::Cyan,
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    ACCENT.set(mode_theme(app).1);
    let [main, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(f.area());
    let mut cursor = None;
    if app.zoom {
        match app.focus {
            Pane::Collection => draw_collection(f, app, main),
            Pane::Url => cursor = draw_url(f, app, main),
            Pane::Request => cursor = draw_request(f, app, main),
            Pane::Response => cursor = draw_response(f, app, main),
        }
    } else {
        let side = (main.width / 4).clamp(20, 36);
        let [left, right] = Layout::horizontal([Constraint::Length(side), Constraint::Min(10)]).areas(main);
        let [url, req, resp] =
            Layout::vertical([Constraint::Length(3), Constraint::Percentage(40), Constraint::Min(3)]).areas(right);
        draw_collection(f, app, left);
        let a = draw_url(f, app, url);
        let b = draw_request(f, app, req);
        let c = draw_response(f, app, resp);
        cursor = a.or(b).or(c);
    }
    if let Some((x, y)) = cursor {
        draw_completion(f, app, main, x, y);
    }
    let status_cursor = draw_status(f, app, status);
    let popup_cursor = draw_popup(f, app, main);
    if let Some((x, y)) = status_cursor.or(popup_cursor).or(cursor) {
        f.set_cursor_position((x, y));
    }
}

/// A bordered pane with its hotkey number and title in the top border.
fn pane<'a>(n: u8, focused: bool, title: Vec<Span<'a>>) -> Block<'a> {
    let (border, key) = if focused {
        (Style::new().fg(accent()), Style::new().fg(Color::Black).bg(accent()).add_modifier(Modifier::BOLD))
    } else {
        (dim(), Style::new().fg(accent()).add_modifier(Modifier::BOLD))
    };
    let mut spans = vec![Span::styled(format!(" {n} "), key), Span::raw(" ")];
    spans.extend(title);
    spans.push(Span::raw(" "));
    Block::bordered().border_type(BorderType::Rounded).border_style(border).title(Line::from(spans))
}

fn tabs<'a>(names: Vec<String>, sel: usize, focused: bool) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    for (i, name) in names.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" │ ", dim()));
        }
        let style = if i != sel {
            dim()
        } else if focused {
            Style::new().fg(accent()).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::new().fg(Color::White).add_modifier(Modifier::BOLD)
        };
        spans.push(Span::styled(name, style));
    }
    spans
}

fn draw_collection(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Pane::Collection;
    let block = pane(1, focused, vec![Span::raw("Collection")]);
    if app.tree.is_empty() {
        let text = vec![
            Line::styled("No requests yet", dim()),
            Line::raw(""),
            Line::from(vec![Span::styled(" a ", bold()), Span::styled("new request", dim())]),
            Line::from(vec![Span::styled(" A ", bold()), Span::styled("new folder", dim())]),
        ];
        f.render_widget(Paragraph::new(text).block(block), area);
        return;
    }
    let dirty = app.dirty();
    let items: Vec<ListItem> = app
        .tree
        .iter()
        .map(|n| {
            let indent = "  ".repeat(n.depth);
            if n.is_dir {
                let arrow = if n.expanded { "▾" } else { "▸" };
                return ListItem::new(Line::styled(
                    format!("{indent}{arrow} {}/", n.name),
                    Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
                ));
            }
            let method = n.method.as_deref().unwrap_or("?");
            let tag = match method {
                "DELETE" => "DEL",
                "PATCH" => "PAT",
                "OPTIONS" => "OPT",
                m => m,
            };
            let current = app.current.as_ref() == Some(&n.path);
            let mut spans = vec![
                Span::raw(indent),
                Span::styled(format!("{tag:<4} "), Style::new().fg(method_color(method))),
                Span::styled(n.name.clone(), if current { bold() } else { Style::new() }),
            ];
            if current && dirty {
                spans.push(Span::styled(" [+]", Style::new().fg(Color::Yellow)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let highlight = if focused { Style::new().add_modifier(Modifier::REVERSED) } else { Style::new().bg(DIM) };
    let list = List::new(items).block(block).highlight_style(highlight);
    f.render_stateful_widget(list, area, &mut app.tree_state);
}

fn draw_url(f: &mut Frame, app: &mut App, area: Rect) -> Cursor {
    let focused = app.focus == Pane::Url;
    let name = match &app.current {
        Some(p) => p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        None => "[unsaved]".to_string(),
    };
    let mut title = vec![Span::raw("Request "), Span::styled(name, dim())];
    if app.dirty() {
        title.push(Span::styled(" [+]", Style::new().fg(Color::Yellow)));
    }
    let block = pane(2, focused, title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width < 12 || inner.height == 0 {
        return None;
    }
    let chip = format!(" {} ", app.method);
    let chip_style = Style::new().fg(Color::Black).bg(method_color(&app.method)).add_modifier(Modifier::BOLD);
    let buf = f.buffer_mut();
    buf.set_string(inner.x, inner.y, &chip, chip_style);
    let used = chip.len() as u16 + 1;
    let url_area = Rect::new(inner.x + used, inner.y, inner.width.saturating_sub(used), 1);
    let editing = app.edit == Some(Edit::Url);
    if app.url.is_empty() && !editing {
        buf.set_stringn(url_area.x, url_area.y, "Enter a URL…  (i to edit)", url_area.width as usize, dim());
        return None;
    }
    app.url.render(buf, url_area, editing)
}

fn draw_request(f: &mut Frame, app: &mut App, area: Rect) -> Cursor {
    let focused = app.focus == Pane::Request;
    let count = |rows: &[KV]| match rows.iter().filter(|r| r.enabled).count() {
        0 => String::new(),
        n => format!(" ({n})"),
    };
    let names = vec![
        format!("Params{}", count(&app.params)),
        format!("Headers{}", count(&app.headers)),
        format!("Body{}", if app.body.is_empty() { "" } else { " •" }),
        format!("Auth{}", if app.auth.kind == AuthKind::None { "" } else { " •" }),
    ];
    let sel = [ReqTab::Params, ReqTab::Headers, ReqTab::Body, ReqTab::Auth]
        .iter()
        .position(|t| *t == app.req_tab)
        .unwrap_or(0);
    let block = pane(3, focused, tabs(names, sel, focused));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let buf = f.buffer_mut();
    let editing = app.edit == Some(Edit::Cell);
    match app.req_tab {
        ReqTab::Params => draw_table(
            buf,
            inner,
            &app.params,
            &mut app.params_cur,
            focused,
            editing,
            &mut app.cell,
            "No query parameters — press o to add one",
        ),
        ReqTab::Headers => draw_table(
            buf,
            inner,
            &app.headers,
            &mut app.headers_cur,
            focused,
            editing,
            &mut app.cell,
            "No headers — press o to add one",
        ),
        ReqTab::Body => {
            let editing = app.edit == Some(Edit::Body);
            if app.body.is_empty() && !editing {
                buf.set_stringn(inner.x + 1, inner.y, "Empty body — press i to write one", inner.width as usize, dim());
                return None;
            }
            app.body.syntax = if app.body.looks_like_json() { Syntax::Json } else { Syntax::Plain };
            app.body.render(buf, inner, editing)
        }
        ReqTab::Auth => draw_auth(buf, inner, app, focused, editing),
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_table(
    buf: &mut Buffer,
    area: Rect,
    rows: &[KV],
    cur: &mut TableCur,
    focused: bool,
    editing: bool,
    cell: &mut Vim,
    hint: &str,
) -> Cursor {
    if area.height == 0 || area.width < 16 {
        return None;
    }
    if rows.is_empty() {
        buf.set_stringn(area.x + 1, area.y, hint, area.width as usize - 1, dim());
        return None;
    }
    let key_w = ((area.width - 5) * 2 / 5).max(4);
    let key_x = area.x + 4;
    let val_x = key_x + key_w + 1;
    let val_w = area.right().saturating_sub(val_x);
    buf.set_stringn(key_x, area.y, "Key", key_w as usize, dim());
    buf.set_stringn(val_x, area.y, "Value", val_w as usize, dim());
    let h = (area.height - 1) as usize;
    if h == 0 {
        return None;
    }
    cur.row = cur.row.min(rows.len() - 1);
    if cur.row < cur.top {
        cur.top = cur.row;
    }
    if cur.row >= cur.top + h {
        cur.top = cur.row + 1 - h;
    }
    let mut cursor = None;
    for (i, kv) in rows.iter().enumerate().skip(cur.top).take(h) {
        let y = area.y + 1 + (i - cur.top) as u16;
        let (check, key_style, val_style) = if kv.enabled {
            ("[x]", Style::new().fg(Color::Cyan), Style::new())
        } else {
            ("[ ]", dim().add_modifier(Modifier::CROSSED_OUT), dim().add_modifier(Modifier::CROSSED_OUT))
        };
        buf.set_string(area.x, y, check, if kv.enabled { Style::new().fg(Color::Green) } else { dim() });
        for (col, x, w, text, style) in [(0, key_x, key_w, &kv.key, key_style), (1, val_x, val_w, &kv.value, val_style)]
        {
            let rect = Rect::new(x, y, w, 1);
            let selected = i == cur.row && col == cur.col;
            if selected && editing {
                cursor = cell.render(buf, rect, true);
                continue;
            }
            buf.set_stringn(x, y, text, w as usize, style);
            if selected {
                let hl = if focused { Style::new().add_modifier(Modifier::REVERSED) } else { Style::new().bg(DIM) };
                buf.set_style(rect, hl);
            }
        }
    }
    cursor
}

fn draw_auth(buf: &mut Buffer, area: Rect, app: &mut App, focused: bool, editing: bool) -> Cursor {
    if area.height == 0 || area.width < 20 {
        return None;
    }
    let label_w = 10u16;
    let val_x = area.x + label_w + 1;
    let val_w = area.right().saturating_sub(val_x);
    let kind = match app.auth.kind {
        AuthKind::None => "None",
        AuthKind::Basic => "Basic",
        AuthKind::Bearer => "Bearer",
    };
    let mut rows = vec![("Type", format!("‹ {kind} ›"))];
    for name in app.auth_fields() {
        let value = match *name {
            "Username" => app.auth.username.clone(),
            "Password" => "•".repeat(app.auth.password.chars().count()),
            _ => app.auth.token.clone(),
        };
        rows.push((name, value));
    }
    app.auth_row = app.auth_row.min(rows.len() - 1);
    let mut cursor = None;
    for (i, (label, value)) in rows.iter().enumerate().take(area.height as usize) {
        let y = area.y + i as u16;
        buf.set_stringn(area.x + 1, y, label, label_w as usize - 1, Style::new().fg(Color::Cyan));
        let rect = Rect::new(val_x, y, val_w, 1);
        let selected = i == app.auth_row;
        if selected && editing {
            cursor = app.cell.render(buf, rect, true);
            continue;
        }
        buf.set_stringn(val_x, y, value, val_w as usize, Style::new());
        if selected {
            let hl = if focused { Style::new().add_modifier(Modifier::REVERSED) } else { Style::new().bg(DIM) };
            buf.set_style(Rect::new(val_x, y, val_w.min(value.chars().count().max(8) as u16), 1), hl);
        }
    }
    if app.auth.kind == AuthKind::None && area.height > 2 {
        buf.set_stringn(area.x + 1, area.y + 2, "h/l to choose an auth type", area.width as usize - 1, dim());
    }
    cursor
}

fn human_size(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

fn draw_response(f: &mut Frame, app: &mut App, area: Rect) -> Cursor {
    let focused = app.focus == Pane::Response;
    let header_count = match &app.response {
        Some(Ok(r)) => format!(" ({})", r.headers.len()),
        _ => String::new(),
    };
    let names = vec!["Body".to_string(), format!("Headers{header_count}")];
    let sel = if app.resp_tab == RespTab::Body { 0 } else { 1 };
    let mut title = vec![Span::raw("Response  ")];
    title.extend(tabs(names, sel, focused));
    let mut block = pane(4, focused, title);
    match &app.response {
        Some(Ok(r)) => {
            let color = match r.status {
                200..=299 => Color::Green,
                300..=399 => Color::Yellow,
                _ => Color::Red,
            };
            let info = Line::from(vec![
                Span::styled(
                    format!(" {} {} ", r.status, r.status_text),
                    Style::new().fg(Color::Black).bg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" {} ms  {} ", r.elapsed.as_millis(), human_size(r.size)), dim()),
                Span::styled(if r.truncated { "(truncated) " } else { "" }, Style::new().fg(Color::Yellow)),
            ]);
            block = block.title(info.right_aligned());
        }
        Some(Err(_)) => {
            let style = Style::new().fg(Color::Black).bg(Color::Red).add_modifier(Modifier::BOLD);
            block = block.title(Line::styled(" ERROR ", style).right_aligned());
        }
        None => {}
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width < 4 || inner.height == 0 {
        return None;
    }
    let buf = f.buffer_mut();
    if let Some(elapsed) = app.sending() {
        const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let frame = FRAMES[(elapsed.as_millis() / 80) as usize % FRAMES.len()];
        let cancel = match app.keys.label(Action::Cancel) {
            k if k.is_empty() => String::new(),
            k => format!("   ({k} to cancel)"),
        };
        let text = format!("{frame} Sending… {:.1}s{cancel}", elapsed.as_secs_f32());
        buf.set_stringn(inner.x + 1, inner.y, text, inner.width as usize - 1, Style::new().fg(Color::Yellow));
        return None;
    }
    if app.response.is_none() {
        let text = format!("No response yet — press {} to send", app.keys.label(Action::Send));
        buf.set_stringn(inner.x + 1, inner.y, text, inner.width as usize - 1, dim());
        return None;
    }
    let editing = app.edit == Some(Edit::Response);
    match app.resp_tab {
        RespTab::Body => app.resp_body.render(buf, inner, editing),
        RespTab::Headers => app.resp_headers.render(buf, inner, editing),
    }
}

/// Join `(keys, description)` hints, leaving out actions that have no key.
fn join_hints(parts: &[(String, &str)]) -> String {
    parts.iter().filter(|(k, _)| !k.is_empty()).map(|(k, d)| format!("{k} {d}")).collect::<Vec<_>>().join("  ")
}

fn hints(app: &App) -> String {
    let k = |a| app.keys.label(a);
    let fixed = |s: &str| s.to_string();
    if app.completion.is_some() {
        return "Tab/^n next  S-Tab/^p prev  ⏎ insert variable  (keep typing to filter)".into();
    }
    let send = k(Action::Send);
    if let Some(e) = app.edit {
        let single = matches!(e, Edit::Url | Edit::Cell | Edit::Prompt);
        let parts = match (app.vim(e).mode, single, e) {
            (Mode::Insert, true, Edit::Cell) => {
                vec![(fixed("Esc"), "normal"), (fixed("⏎"), "accept"), (fixed("Tab"), "next cell")]
            }
            (Mode::Insert, true, Edit::Url) => vec![(fixed("Esc"), "normal"), (fixed("⏎"), "accept"), (send, "send")],
            (Mode::Insert, true, _) => vec![(fixed("Esc"), "normal"), (fixed("⏎"), "accept")],
            (Mode::Insert, false, _) => vec![(fixed("Esc"), "normal"), (send, "send")],
            (Mode::Search, ..) => vec![(fixed("⏎"), "search"), (fixed("Esc"), "cancel")],
            (Mode::Visual | Mode::VisualLine, ..) => {
                vec![(fixed("y"), "yank"), (fixed("d"), "delete"), (fixed("c"), "change"), (fixed("Esc"), "cancel")]
            }
            (_, _, Edit::Response) => vec![
                (fixed("Esc"), "back"),
                (fixed("v"), "visual"),
                (fixed("y"), "yank"),
                (fixed("/"), "search"),
                (fixed("n/N"), "next/prev"),
            ],
            (_, _, Edit::Prompt) => vec![(fixed("Esc"), "cancel"), (fixed("⏎"), "accept"), (fixed("i"), "insert")],
            (_, _, Edit::Env) => vec![
                (fixed("Esc"), "save+close"),
                (fixed("i"), "insert"),
                (fixed("o"), "new line"),
                (fixed("dd"), "delete line"),
                (fixed("u"), "undo"),
                (send, "send"),
            ],
            _ => vec![
                (fixed("Esc"), "back"),
                (fixed("i"), "insert"),
                (fixed("v"), "visual"),
                (fixed("u"), "undo"),
                (fixed("/"), "search"),
                (send, "send"),
            ],
        };
        return join_hints(&parts);
    }
    let tabs = || {
        let (prev, next) = (k(Action::PrevTab), k(Action::NextTab));
        if prev.is_empty() && next.is_empty() {
            String::new()
        } else {
            format!("{prev} {next}")
        }
    };
    let help = (k(Action::Help), "help");
    let parts = match (app.focus, app.req_tab) {
        (Pane::Collection, _) => vec![
            (fixed("j/k"), "move"),
            (fixed("l/⏎"), "open"),
            (k(Action::NewRequest), "new"),
            (k(Action::NewFolder), "folder"),
            (k(Action::Rename), "rename"),
            (k(Action::Delete), "delete"),
            help,
        ],
        (Pane::Url, _) => vec![
            (fixed("i"), "edit"),
            (fixed("c"), "change"),
            (send, "send"),
            (k(Action::Method), "method"),
            (fixed(":w"), "save"),
            help,
        ],
        (Pane::Request, ReqTab::Params | ReqTab::Headers) => vec![
            (fixed("hjkl"), "move"),
            (fixed("i"), "edit"),
            (k(Action::AddRow), "add"),
            (k(Action::DeleteRow), "delete"),
            (k(Action::ToggleRow), "toggle"),
            (tabs(), "tabs"),
            (send, "send"),
            help,
        ],
        (Pane::Request, ReqTab::Body) => vec![
            (fixed("⏎"), "edit"),
            (fixed("i"), "insert"),
            (fixed("j/k"), "scroll"),
            (tabs(), "tabs"),
            (send, "send"),
            help,
        ],
        (Pane::Request, ReqTab::Auth) => vec![
            (fixed("j/k"), "move"),
            (fixed("h/l"), "type"),
            (fixed("i"), "edit"),
            (tabs(), "tabs"),
            (send, "send"),
            help,
        ],
        (Pane::Response, _) => vec![
            (fixed("j/k"), "scroll"),
            (fixed("⏎"), "cursor"),
            (fixed("v"), "select"),
            (fixed("/"), "search"),
            (k(Action::CopyResponse), "copy all"),
            (tabs(), "tabs"),
            help,
        ],
    };
    join_hints(&parts)
}

/// The pane keys, `1-4` with the defaults.
fn fixed_panes(app: &App) -> String {
    let keys: Vec<String> = [Action::FocusCollection, Action::FocusUrl, Action::FocusRequest, Action::FocusResponse]
        .into_iter()
        .map(|a| app.keys.label(a))
        .collect();
    if keys == ["1", "2", "3", "4"] {
        "1-4".into()
    } else {
        keys.join(" ")
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) -> Cursor {
    // On a one-row terminal the status line falls outside the buffer.
    if area.height == 0 || area.bottom() > f.area().bottom() {
        return None;
    }
    let buf = f.buffer_mut();
    if let Some(cmd) = &app.cmdline {
        let text = format!(":{cmd}");
        buf.set_stringn(area.x, area.y, &text, area.width as usize, Style::new());
        let x = area.x + (text.chars().count() as u16).min(area.width.saturating_sub(1));
        return Some((x, area.y));
    }
    let vim = app.edit.map(|e| app.vim(e));
    if let Some(pat) = vim.and_then(|v| v.search_prompt()) {
        let text = format!("/{pat}");
        buf.set_stringn(area.x, area.y, &text, area.width as usize, Style::new());
        let x = area.x + (text.chars().count() as u16).min(area.width.saturating_sub(1));
        return Some((x, area.y));
    }
    let (label, color) = mode_theme(app);
    let chip = format!(" {label} ");
    let chip_style = Style::new().fg(Color::Black).bg(color).add_modifier(Modifier::BOLD);
    buf.set_stringn(area.x, area.y, &chip, area.width as usize, chip_style);
    let x = area.x + chip.len() as u16 + 1;
    let right = match (&app.message, vim) {
        (Some((msg, error, _)), _) => {
            Span::styled(format!(" {msg} "), Style::new().fg(if *error { Color::Red } else { Color::Green }))
        }
        (None, Some(v)) if !v.single_line => {
            let p = v.cursor();
            Span::styled(format!(" {}  {}:{}/{} ", v.pending(), p.y + 1, p.x + 1, v.line_count()), dim())
        }
        (None, Some(v)) => Span::styled(format!(" {} ", v.pending()), dim()),
        (None, None) => {
            let parts = [
                (fixed_panes(app), "panes"),
                (app.keys.label(Action::Env), "env"),
                (app.keys.label(Action::Command), "cmd"),
            ];
            Span::styled(format!(" {} ", join_hints(&parts)), dim())
        }
    };
    let right_w = right.content.chars().count() as u16;
    let hint_w = area.right().saturating_sub(x).saturating_sub(right_w);
    buf.set_stringn(x, area.y, hints(app), hint_w as usize, dim());
    if right_w < area.width {
        buf.set_span(area.right() - right_w, area.y, &right, right_w);
    }
    None
}

/// The `@` variable completion list, just below (or above) the cursor.
fn draw_completion(f: &mut Frame, app: &App, area: Rect, x: u16, y: u16) {
    let Some((names, sel)) = &app.completion else {
        return;
    };
    let shown = names.len().min(8);
    // Fit the longest name plus a space each side and the borders, up to a cap.
    let longest = names.iter().map(|n| n.chars().count()).max().unwrap_or(0) as u16;
    let w = (longest + 4).clamp(9, 60).min(area.width);
    let h = shown as u16 + 2;
    let below = y + 1 + h <= area.bottom();
    let top = if below { y + 1 } else { y.saturating_sub(h).max(area.y) };
    let left = x.min(area.right().saturating_sub(w));
    let rect = Rect::new(left, top, w, h.min(area.height));
    let skip = (sel + 1).saturating_sub(shown);
    let lines: Vec<Line> = names
        .iter()
        .enumerate()
        .skip(skip)
        .take(shown)
        .map(|(i, n)| {
            let line = Line::raw(format!(" {n:<width$} ", width = w.saturating_sub(4) as usize));
            if i == *sel {
                line.style(Style::new().fg(Color::Black).bg(accent()))
            } else {
                line
            }
        })
        .collect();
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(popup_block("env")), rect);
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

fn popup_block<'a>(title: &'a str) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent()))
        .title(Line::styled(format!(" {title} "), bold()))
}

fn draw_env(f: &mut Frame, app: &mut App, area: Rect) -> Cursor {
    let rect = centered(area, (area.width * 4 / 5).max(40), (area.height * 4 / 5).max(8));
    let hint = " KEY=VALUE per line · use as {{KEY}} · Esc saves and closes ";
    let block = popup_block("Environment · .env").title_bottom(Line::styled(hint, dim()));
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);
    app.env.render(f.buffer_mut(), inner, true)
}

fn draw_popup(f: &mut Frame, app: &mut App, area: Rect) -> Cursor {
    if app.edit == Some(Edit::Env) {
        return draw_env(f, app, area);
    }
    match &app.popup {
        None => None,
        Some(Popup::Help { scroll }) => {
            let lines = help_lines(app);
            let rect = centered(area, 72, lines.len() as u16 + 2);
            let max = (lines.len() as u16 + 2).saturating_sub(rect.height);
            let block = popup_block("Help").title_bottom(Line::styled(" j/k scroll · any other key closes ", dim()));
            f.render_widget(Clear, rect);
            f.render_widget(Paragraph::new(lines).block(block).scroll(((*scroll).min(max), 0)), rect);
            None
        }
        Some(Popup::Method { sel }) => {
            let rect = centered(area, 22, METHODS.len() as u16 + 2);
            let lines: Vec<Line> = METHODS
                .iter()
                .enumerate()
                .map(|(i, (name, key))| {
                    let line = Line::from(vec![
                        Span::styled(format!(" {key} "), Style::new().fg(accent()).add_modifier(Modifier::BOLD)),
                        Span::styled(format!(" {name:<8}"), Style::new().fg(method_color(name))),
                    ]);
                    if i == *sel {
                        line.style(Style::new().add_modifier(Modifier::REVERSED))
                    } else {
                        line
                    }
                })
                .collect();
            f.render_widget(Clear, rect);
            f.render_widget(Paragraph::new(lines).block(popup_block("Method")), rect);
            None
        }
        Some(Popup::Confirm { msg, action }) => {
            let key = |k: &'static str| Span::styled(k, Style::new().fg(accent()).add_modifier(Modifier::BOLD));
            let mut opts = vec![key(" y"), Span::raw(" yes   "), key("n"), Span::raw(" no")];
            if !matches!(action, Confirm::Delete(_)) {
                opts.extend([Span::raw("   "), key("s"), Span::raw(" save first")]);
            }
            let rect = centered(area, (msg.chars().count() as u16 + 6).max(40), 5);
            let text = vec![Line::raw(format!(" {msg}")), Line::raw(""), Line::from(opts)];
            f.render_widget(Clear, rect);
            f.render_widget(Paragraph::new(text).block(popup_block("Confirm")), rect);
            None
        }
        Some(Popup::Prompt(kind)) => {
            let title = match kind {
                PromptKind::NewRequest => "New request · path from collection root",
                PromptKind::NewFolder => "New folder · path from collection root",
                PromptKind::Rename => "Rename / move · path from collection root",
                PromptKind::SaveAs => "Save as · path from collection root",
            };
            let rect = centered(area, 50, 3);
            let block = popup_block(title);
            let inner = block.inner(rect);
            f.render_widget(Clear, rect);
            f.render_widget(block, rect);
            let inner = Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), inner.height);
            app.cell.render(f.buffer_mut(), inner, true)
        }
    }
}

fn help_lines(app: &App) -> Vec<Line<'static>> {
    let k = |a| app.keys.label(a);
    let both = |a, b| format!("{}  {}", k(a), k(b));
    let help: Vec<(String, &str)> = vec![
        ("Everywhere".into(), ""),
        (
            [Action::FocusCollection, Action::FocusUrl, Action::FocusRequest, Action::FocusResponse].map(k).join(" "),
            "jump to pane (number shown in each border)",
        ),
        (both(Action::NextPane, Action::PrevPane), "next / previous pane"),
        (
            [Action::PaneLeft, Action::PaneDown, Action::PaneUp, Action::PaneRight].map(k).join(" "),
            "move to the pane left / below / above / right",
        ),
        (both(Action::PrevTab, Action::NextTab), "previous / next tab of the focused pane (3 or 4)"),
        (k(Action::Send), "send request (ctrl/alt keys also work while editing)"),
        (k(Action::Method), "choose HTTP method (then g p u a d h o)"),
        (k(Action::Zoom), "zoom the focused pane"),
        (format!("{}  or  :env", k(Action::Env)), "edit the collection's .env variables (Esc saves)"),
        (":w  :w name".into(), "save / save as"),
        (":new  :q  :wq  :q!".into(), "new request / quit"),
        (":w! name".into(), "save as, overwriting an existing request"),
        (k(Action::Cancel), "cancel a running request"),
        (k(Action::Quit), "quit"),
        (k(Action::Help), "this help"),
        (String::new(), ""),
        ("Collection".into(), ""),
        ("j k g G".into(), "move"),
        ("l / Enter".into(), "open request / toggle folder (focus stays here)"),
        ("h".into(), "collapse folder / go to parent"),
        (both(Action::NewRequest, Action::NewFolder), "new request / new folder"),
        (
            [Action::Rename, Action::Delete, Action::Reload].map(k).join("  "),
            "rename or move / delete / reload from disk",
        ),
        ("(name prompts)".into(), "path from the root; Ctrl-u clears the folder prefix"),
        (String::new(), ""),
        ("Params, Headers, Auth".into(), ""),
        ("h j k l".into(), "move between cells"),
        ("i / Enter".into(), "edit cell      c  replace cell"),
        (both(Action::AddRow, Action::AddRowAbove), "add row below / above"),
        (k(Action::DeleteRow), "delete row"),
        (k(Action::ToggleRow), "enable / disable row"),
        ("Tab / S-Tab".into(), "while editing: next / previous cell"),
        (String::new(), ""),
        ("Body and Response".into(), ""),
        ("j k g G ^d ^u".into(), "scroll"),
        ("Enter".into(), "enter the editor in normal mode (i a o… also work)"),
        ("=".into(), "body: pretty-print the JSON (normal mode)"),
        (k(Action::CopyResponse), "response: copy everything to the clipboard"),
        (String::new(), ""),
        ("Vim editing (URL, cells, body, response)".into(), ""),
        ("Esc".into(), "insert → normal → back to pane navigation"),
        ("h j k l w b e 0 ^ $".into(), "motions, with counts; also gg G f t F T ; % { }"),
        ("i a I A o O".into(), "insert"),
        ("d c y + motion".into(), "operators; dd cc yy D C x s r ~ J p P"),
        ("iw aw i\" a\" i( a{ …".into(), "text objects"),
        ("v  V".into(), "visual / visual line"),
        ("u  Ctrl-r".into(), "undo / redo"),
        ("/  n  N".into(), "search"),
        (String::new(), ""),
        ("Variables".into(), ""),
        ("{{name}}".into(), "replaced from the collection's .env file or the environment"),
        ("@name".into(), "while typing: pick a .env variable (Tab/^n ^p, Enter inserts {{name}})"),
        (String::new(), ""),
        ("Keys".into(), ""),
        ("config.toml".into(), "rebind the keys above: lazypost --print-config"),
    ];
    help.into_iter()
        .map(|(keys, desc)| {
            if desc.is_empty() {
                Line::styled(format!(" {keys}"), Style::new().fg(accent()).add_modifier(Modifier::BOLD))
            } else {
                Line::from(vec![Span::styled(format!("   {:<22}", keys.trim()), bold()), Span::raw(desc)])
            }
        })
        .collect()
}
