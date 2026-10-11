mod app;
mod http;
mod model;
mod ui;
mod vim;

use app::App;
use ratatui::{
    crossterm::{
        cursor::SetCursorStyle,
        event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind},
        execute,
    },
    DefaultTerminal,
};
use std::{io, path::PathBuf, time::Duration};

const USAGE: &str = "\
lazypost — a keyboard-first HTTP client for the terminal

Usage: lazypost [COLLECTION_DIR]

Requests are stored as .toml files in COLLECTION_DIR
(default: $XDG_DATA_HOME/lazypost or ~/.local/share/lazypost).
Press ? inside the app for keybindings.";

fn default_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
    });
    base.join("lazypost")
}

fn main() -> io::Result<()> {
    let root = match std::env::args().nth(1).as_deref() {
        Some("-h" | "--help") => {
            println!("{USAGE}");
            return Ok(());
        }
        Some("-V" | "--version") => {
            println!("lazypost {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some(dir) => {
            std::fs::create_dir_all(dir)?;
            PathBuf::from(dir)
        }
        None => {
            let dir = default_dir();
            if !dir.exists() {
                std::fs::create_dir_all(&dir)?;
                model::seed(&dir);
            }
            dir
        }
    };
    let mut app = App::new(root.canonicalize().unwrap_or(root));
    let mut terminal = ratatui::init();
    let _ = execute!(io::stdout(), EnableBracketedPaste);
    let result = run(&mut terminal, &mut app);
    let _ = execute!(io::stdout(), DisableBracketedPaste, SetCursorStyle::DefaultUserShape);
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    let mut bar_cursor = false;
    while !app.quit {
        app.tick();
        terminal.draw(|f| ui::draw(f, app))?;
        let bar = app.cmdline.is_some()
            || app.edit.is_some_and(|e| matches!(app.vim(e).mode, vim::Mode::Insert | vim::Mode::Search));
        if bar != bar_cursor {
            bar_cursor = bar;
            let style = if bar { SetCursorStyle::SteadyBar } else { SetCursorStyle::SteadyBlock };
            execute!(io::stdout(), style)?;
        }
        let timeout = Duration::from_millis(if app.sending().is_some() { 80 } else { 1000 });
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.on_key(key),
                Event::Paste(text) => app.on_paste(&text),
                _ => {}
            }
        }
    }
    Ok(())
}
