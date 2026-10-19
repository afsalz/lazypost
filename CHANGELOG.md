# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0]

First release.

- Four-pane layout (collection, URL, request, response), each pane reached by its number.
- Vim editing in every text field, with the UI accent color following the mode.
- Requests stored as `.toml` files, folders as directories; `{{var}}` substitution from `.env`.
- `@name` completion of `.env` variables while typing.
- `Ctrl-s` sends from anywhere; `H`/`L` and `[`/`]` switch the focused pane's tabs.
- Configurable keys in `~/.config/lazypost/config.toml` (`--print-config`, `--check-config`).
- Prebuilt binaries for Linux and macOS and a `curl | sh` installer.

[Unreleased]: https://github.com/afsalz/lazypost/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/afsalz/lazypost/releases/tag/v0.1.0
