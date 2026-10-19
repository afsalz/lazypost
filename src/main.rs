mod app;
mod config;
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

Usage: lazypost [OPTIONS] [COLLECTION_DIR]

Requests are stored as .toml files in COLLECTION_DIR
(default: $XDG_DATA_HOME/lazypost or ~/.local/share/lazypost).
Press ? inside the app for keybindings.

Options:
  --print-config   print a config.toml with every key at its default
  --check-config   validate the config file and exit
  -h, --help       print this help
  -V, --version    print the version

Keys are configured in config.toml in the config folder:
$LAZYPOST_CONFIG_DIR, else $XDG_CONFIG_HOME/lazypost, else ~/.config/lazypost.";

fn default_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
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
        Some("--print-config") => {
            print!("{}", config::default_file());
            return Ok(());
        }
        Some("--check-config") => {
            let path = config::path();
            let (_, warnings) = config::load();
            if warnings.is_empty() {
                println!("{}: ok", path.display());
                return Ok(());
            }
            for w in &warnings {
                eprintln!("{w}");
            }
            std::process::exit(1);
        }
        Some(flag) if flag.starts_with('-') => {
            eprintln!("lazypost: unknown option {flag}\n\n{USAGE}");
            std::process::exit(2);
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
    let (config, warnings) = config::load();
    let mut app = App::new(root.canonicalize().unwrap_or(root), config.keys);
    if !warnings.is_empty() {
        app.error(format!("Config: {}", warnings.join("; ")));
    }
    let mut terminal = match ratatui::try_init() {
        Ok(t) => t,
        Err(e) => {
            ratatui::restore();
            eprintln!("lazypost: cannot start the terminal UI: {e}");
            std::process::exit(1);
        }
    };
    // ratatui's panic hook restores raw mode and the screen; also undo our extras.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableBracketedPaste, SetCursorStyle::DefaultUserShape);
        hook(info);
    }));
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
