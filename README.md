# lazypost

[![CI](https://github.com/afsalz/lazypost/actions/workflows/ci.yml/badge.svg)](https://github.com/afsalz/lazypost/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A keyboard-first HTTP client for the terminal. No mouse required: jump to any
pane by the number in its border, edit every text field like vim, and keep
requests as plain `.toml` files in git.

![lazypost demo: sending requests, @ variable completion, editing headers](assets/demo.gif)

## Install

**Prebuilt binary** (Linux and macOS, x86_64 and arm64):

```sh
curl -fsSL https://raw.githubusercontent.com/afsalz/lazypost/main/install.sh | sh
```

The script downloads the latest release, verifies its SHA-256 checksum and
installs `lazypost` into `~/.local/bin`. Set `LAZYPOST_VERSION=v0.1.0` to pin a
version or `LAZYPOST_INSTALL_DIR=/usr/local/bin` to install elsewhere. You can
also download an archive by hand from the
[releases page](https://github.com/afsalz/lazypost/releases).

**With cargo** (Rust 1.88 or newer):

```sh
cargo install --locked --git https://github.com/afsalz/lazypost
```

**From source:**

```sh
git clone https://github.com/afsalz/lazypost && cd lazypost
cargo install --locked --path .
```

## Usage

```sh
lazypost                 # default collection in ~/.local/share/lazypost
lazypost ./my-api        # or any directory
lazypost --help          # options
```

The first run of the default collection creates a couple of example requests.

## How input works

There are three layers, and `Esc` always moves one layer up:

1. **NAV**: move between panes and within lists and tables.
2. **NORMAL**: inside a text field, vim normal mode.
3. **INSERT**: typing.

The bottom line shows the current mode and the keys that apply right now, and
the UI's accent color follows the mode (NAV cyan, NORMAL green, INSERT yellow,
VISUAL magenta). Press `?` for the full list.

| Keys | Action |
| --- | --- |
| `1` `2` `3` `4` | focus Collection / URL / Request / Response |
| `Tab` `S-Tab`, `Ctrl-h/j/k/l` | cycle panes / move in a direction |
| `[` `]` or `H` `L` | switch tabs of the focused pane (3: Params, Headers, Body, Auth · 4: Body, Headers) |
| `Ctrl-s` | send the request (works everywhere, including while typing) |
| `Ctrl-c` | cancel a running request |
| `m` then `g p u a d h o` | pick the HTTP method |
| `z` | zoom the focused pane |
| `E` or `:env` | edit the collection's `.env` variables (`Esc` saves and closes) |
| `:w`, `:w name`, `:w! name`, `:new`, `:q`, `:wq`, `:q!` | save, save as (`!` overwrites an existing request), new request, quit |
| `i` / `Enter` | edit the focused field (insert / normal mode) |
| `=` (body, normal mode) | pretty-print the body if it is valid JSON |
| `o` `O` `d` `Space` | tables: add row below / above, delete row, toggle row |
| `l` / `Enter` | collection: open the request (focus stays in the collection) |
| `a` `A` `r` `d` `R` | collection: new request, new folder, rename/move, delete, reload |
| `y` | response: copy the whole body to the clipboard |

Name prompts take a path relative to the collection root and are prefilled
with the selected folder (e.g. `examples/`). Keep the prefix to create inside
that folder, or clear it (`Ctrl-u` in insert mode) to create at the top level.
`a/b/name` creates intermediate folders, and renaming to a different path moves the item.

Vim support in fields: counts, `h j k l w b e W B E 0 ^ $ gg G f t F T ; % { }`,
operators `d c y` with motions and text objects (`iw aw i" a" i( a{ i[` …),
`dd cc yy D C x X s S r ~ J p P`, visual and visual-line modes, `u` / `Ctrl-r`,
and `/ n N` search. Yanks go to the system clipboard.

## Configuration

Keys are configured in `config.toml` in the config folder, the first of
`$LAZYPOST_CONFIG_DIR`, `$XDG_CONFIG_HOME/lazypost` or `~/.config/lazypost`.
Start from the defaults:

```sh
mkdir -p ~/.config/lazypost
lazypost --print-config > ~/.config/lazypost/config.toml
```

Each action takes one key or a list of keys, and an empty list unbinds it:

```toml
[keys]
send = ["ctrl-s", "F5"]
quit = []
next_tab = ["]", "tab"]
next_pane = "ctrl-n"
```

Keys are written as a character (`a`, `A`, `?`, `]`) or a name (`tab`,
`enter`, `space`, `esc`, `backspace`, `up`, `F1`–`F12`, …), optionally with
`ctrl-`, `alt-` or `shift-`. The application-level actions (panes, tabs,
sending, collection and table actions) are configurable. Vim keys inside text
fields are not.
A send key with `ctrl`, `alt` or an F-key also works while typing.
The status line and `?` help show your keys. `lazypost --check-config`
validates the file; mistakes are also reported in the status line at
startup, and the defaults are kept for anything that could not be read.

## Storage

Each request is a `.toml` file and folders are directories, so a collection
diffs and merges well in git:

```toml
method = "POST"
url = "{{base_url}}/post"
body = """
{
  "name": "lazypost"
}"""

[[headers]]
key = "Accept"
value = "application/json"
enabled = true
```

Put `KEY=VALUE` lines in a `.env` file at the collection root (press `E` to edit
it in the app) and reference them as `{{KEY}}` in URLs, params, headers, auth
and bodies. Process environment variables work too. While typing, `@` followed
by part of a name (e.g. `@base_u`) pops up the matching `.env` variables:
`Tab`/`Ctrl-n` and `S-Tab`/`Ctrl-p` choose, and `Enter` inserts `{{base_url}}`.

## Development

```sh
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

CI runs these on every push and pull request. The README demo is recorded with
[vhs](https://github.com/charmbracelet/vhs): `vhs assets/demo.tape`.

### Releasing

1. Bump `version` in `Cargo.toml` and move the `Unreleased` notes in
   `CHANGELOG.md` under the new version.
2. Commit, then tag and push: `git tag v0.2.0 && git push origin main v0.2.0`.

The release workflow checks that the tag matches `Cargo.toml` and runs the tests.
It then builds static Linux (musl) and macOS binaries for x86_64 and arm64 and
publishes a GitHub release with the archives, checksums and the changelog notes.
`install.sh` downloads from that release.

## License

[MIT](LICENSE)
