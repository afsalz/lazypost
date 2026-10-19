# lazypost

A keyboard-first HTTP client for the terminal, in the spirit of
[posting](https://github.com/darrenburns/posting), built with Rust and
[ratatui](https://ratatui.rs). No mouse required: panes are reached btop-style
by the number in their border, and every text field is a vim editor.

```
cargo run --release                 # default collection in ~/.local/share/lazypost
cargo run --release -- ./my-api     # or any directory
```

## How input works

There are three layers, and `Esc` always moves one layer up:

1. **NAV** – move between panes and within lists/tables.
2. **NORMAL** – inside a text field, vim normal mode.
3. **INSERT** – typing.

The bottom line always shows the current mode and the keys that apply right now.
Press `?` for the full list.

| Keys | Action |
| --- | --- |
| `1` `2` `3` `4` | jump to Collection / URL / Request / Response |
| `Tab` `S-Tab`, `Ctrl-h/j/k/l` | cycle panes / move directionally |
| `[` `]` or `H` `L` | switch tabs (Params, Headers, Body, Auth · Body, Headers) |
| `s`, `Ctrl-s` | send (`Ctrl-s` also works while editing) |
| `m` then `g p u a d h o` | pick the HTTP method |
| `z` | zoom the focused pane |
| `E` or `:env` | edit the collection's `.env` variables in a vim overlay (`Esc` saves and closes) |
| `:w`, `:w name`, `:new`, `:q`, `:wq`, `:q!` | save, save as, new request, quit |
| `i` / `Enter` | edit the focused field (insert / normal mode) |
| `=` (body, normal mode) | pretty-print the body if it is valid JSON |
| `o` `O` `d` `Space` | tables: add row, delete row, toggle row |
| `a` `A` `r` `d` | collection: new request, new folder, rename/move, delete |

Name prompts take a path relative to the collection root and are prefilled
with the selected folder (e.g. `examples/`). Keep the prefix to create inside
that folder, or clear it (`Ctrl-u` in insert mode) to create at the top level.
`a/b/name` creates intermediate folders, and renaming to a different path moves the item.

Vim support in fields: counts, `h j k l w b e W B E 0 ^ $ gg G f t F T ; % { }`,
operators `d c y` with motions and text objects (`iw aw i" a" i( a{ i[` …),
`dd cc yy D C x X s S r ~ J p P`, visual and visual-line modes, `u` / `Ctrl-r`,
and `/ n N` search. Yanks go to the system clipboard.

## Storage

Each request is a `.toml` file; folders are directories. Put `KEY=VALUE` lines
in a `.env` file at the collection root (press `E` to edit it in the app) and reference them as `{{KEY}}` in
URLs, params, headers, auth and bodies (process environment variables work too).
