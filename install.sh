#!/usr/bin/env bash
#
# install.sh — install or uninstall GifThatForYou natively.
#
# Usage:
#   ./install.sh [install]      Install the app (default action).
#   ./install.sh uninstall      Remove everything this script installed.
#   ./install.sh -h | --help    Show this message.
#
# Prefix selection:
#   1. $PREFIX from the environment if set.
#   2. /usr/local if running as root.
#   3. $HOME/.local otherwise.

set -euo pipefail

cd "$(dirname "$(readlink -f "$0")")"

APP_ID="io.github.kmwallio.GifThatForYou"
BIN_MAIN="gif-that-for-you"
BIN_MCP="gif-that-for-you-mcp"

usage() {
    sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
}

resolve_prefix() {
    if [ -n "${PREFIX:-}" ]; then
        printf '%s\n' "$PREFIX"
    elif [ "$(id -u)" -eq 0 ]; then
        printf '/usr/local\n'
    else
        printf '%s/.local\n' "$HOME"
    fi
}

refresh_caches() {
    local appdir="$1"
    local icon_root="$2"

    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$appdir" >/dev/null 2>&1 || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -f -t "$icon_root" >/dev/null 2>&1 || true
    fi
}

do_install() {
    local prefix="$1"
    local bindir="$prefix/bin"
    local appdir="$prefix/share/applications"
    local icon_root="$prefix/share/icons/hicolor"
    local icondir="$icon_root/scalable/apps"
    local metainfodir="$prefix/share/metainfo"

    echo "Installing $APP_ID to: $prefix"

    if [ ! -x "target/release/$BIN_MAIN" ] || [ ! -x "target/release/$BIN_MCP" ]; then
        if ! command -v cargo >/dev/null 2>&1; then
            echo "error: cargo is not installed or not on PATH" >&2
            echo "       install Rust via https://rustup.rs and re-run this script" >&2
            exit 1
        fi
        echo "Building release binaries (cargo build --release)..."
        cargo build --release
    fi

    install -Dm755 "target/release/$BIN_MAIN"  "$bindir/$BIN_MAIN"
    echo "  installed $bindir/$BIN_MAIN"
    install -Dm755 "target/release/$BIN_MCP"   "$bindir/$BIN_MCP"
    echo "  installed $bindir/$BIN_MCP"

    install -Dm644 "$APP_ID.desktop"           "$appdir/$APP_ID.desktop"
    echo "  installed $appdir/$APP_ID.desktop"

    install -Dm644 "$APP_ID.metainfo.xml"      "$metainfodir/$APP_ID.metainfo.xml"
    echo "  installed $metainfodir/$APP_ID.metainfo.xml"

    install -Dm644 "data/icons/hicolor/scalable/apps/$APP_ID.svg" \
                                               "$icondir/$APP_ID.svg"
    echo "  installed $icondir/$APP_ID.svg"

    refresh_caches "$appdir" "$icon_root"

    case ":${PATH:-}:" in
        *":$bindir:"*) ;;
        *)
            echo
            echo "note: $bindir is not on your \$PATH."
            echo "      add this to your shell profile:"
            echo "          export PATH=\"$bindir:\$PATH\""
            ;;
    esac

    echo "Done."
}

do_uninstall() {
    local prefix="$1"
    local bindir="$prefix/bin"
    local appdir="$prefix/share/applications"
    local icon_root="$prefix/share/icons/hicolor"
    local icondir="$icon_root/scalable/apps"
    local metainfodir="$prefix/share/metainfo"

    echo "Uninstalling $APP_ID from: $prefix"

    local target
    for target in \
        "$bindir/$BIN_MAIN" \
        "$bindir/$BIN_MCP" \
        "$appdir/$APP_ID.desktop" \
        "$metainfodir/$APP_ID.metainfo.xml" \
        "$icondir/$APP_ID.svg"
    do
        if [ -e "$target" ] || [ -L "$target" ]; then
            rm -f "$target"
            echo "  removed $target"
        else
            echo "  skipped $target (not present)"
        fi
    done

    refresh_caches "$appdir" "$icon_root"

    echo "Done."
}

main() {
    local action="${1:-install}"
    case "$action" in
        install)
            do_install "$(resolve_prefix)"
            ;;
        uninstall|remove)
            do_uninstall "$(resolve_prefix)"
            ;;
        -h|--help|help)
            usage
            ;;
        *)
            echo "error: unknown action '$action'" >&2
            usage >&2
            exit 1
            ;;
    esac
}

main "$@"
