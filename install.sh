#!/bin/sh
# Install lazypost from the GitHub releases.
#
#   curl -fsSL https://raw.githubusercontent.com/afsalz/lazypost/main/install.sh | sh
#
# Environment:
#   LAZYPOST_VERSION      release to install, e.g. v0.1.0 (default: latest)
#   LAZYPOST_INSTALL_DIR  where to put the binary (default: ~/.local/bin)

set -eu

REPO="afsalz/lazypost"
VERSION="${LAZYPOST_VERSION:-latest}"
INSTALL_DIR="${LAZYPOST_INSTALL_DIR:-$HOME/.local/bin}"

say() {
    printf 'lazypost: %s\n' "$1"
}

die() {
    printf 'lazypost: error: %s\n' "$1" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || die "'$1' is required but not installed"
}

download() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        die "curl or wget is required"
    fi
}

detect_target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$arch" in
        x86_64 | amd64) arch=x86_64 ;;
        aarch64 | arm64) arch=aarch64 ;;
        *) die "unsupported architecture: $arch" ;;
    esac
    case "$os" in
        Linux) echo "$arch-unknown-linux-musl" ;;
        Darwin)
            # An x86_64 shell under Rosetta on Apple silicon still gets the native build.
            if [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
                arch=aarch64
            fi
            echo "$arch-apple-darwin"
            ;;
        *) die "unsupported OS: $os (build from source with: cargo install lazypost)" ;;
    esac
}

verify() {
    file=$1
    sums=$2
    expected=$(cut -d ' ' -f 1 "$sums")
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$file" | cut -d ' ' -f 1)
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$file" | cut -d ' ' -f 1)
    else
        say "no sha256 tool found, skipping checksum verification"
        return 0
    fi
    [ "$expected" = "$actual" ] || die "checksum mismatch for $(basename "$file")"
}

main() {
    need uname
    need tar
    target=$(detect_target)
    archive="lazypost-$target.tar.gz"
    if [ "$VERSION" = latest ]; then
        base="https://github.com/$REPO/releases/latest/download"
    else
        case "$VERSION" in
            v*) ;;
            *) VERSION="v$VERSION" ;;
        esac
        base="https://github.com/$REPO/releases/download/$VERSION"
    fi

    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT INT TERM

    say "downloading $archive ($VERSION)"
    download "$base/$archive" "$tmp/$archive" || die "download failed: $base/$archive"
    download "$base/$archive.sha256" "$tmp/$archive.sha256" || die "download failed: $base/$archive.sha256"
    verify "$tmp/$archive" "$tmp/$archive.sha256"

    tar -xzf "$tmp/$archive" -C "$tmp"
    mkdir -p "$INSTALL_DIR"
    # Copy then move so a running lazypost is replaced atomically.
    cp "$tmp/lazypost-$target/lazypost" "$INSTALL_DIR/.lazypost.new"
    chmod 755 "$INSTALL_DIR/.lazypost.new"
    mv "$INSTALL_DIR/.lazypost.new" "$INSTALL_DIR/lazypost"

    say "installed $("$INSTALL_DIR/lazypost" --version) to $INSTALL_DIR/lazypost"
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *) say "add $INSTALL_DIR to your PATH, e.g. echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> ~/.profile" ;;
    esac
}

main "$@"
