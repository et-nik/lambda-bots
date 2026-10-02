#!/usr/bin/env bash
# Builds lambdabots and installs it into a local Xash3D FWGS (macOS arm64) stand.
# Usage: scripts/stand/macos-install.sh [--stand DIR] [--preset dev-macos] [--disable-yapb] [--link-config]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STAND="${LB_STAND:-$HOME/Git/half-life/xash3d-fwgs-apple-arm64}"
PRESET=dev-macos
DISABLE_YAPB=0
LINK_CONFIG=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --stand) STAND="$2"; shift 2 ;;
        --preset) PRESET="$2"; shift 2 ;;
        --disable-yapb) DISABLE_YAPB=1; shift ;;
        --link-config) LINK_CONFIG=1; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done

CLION="$HOME/Applications/CLion.app/Contents/bin"
if ! command -v cmake >/dev/null 2>&1; then export PATH="$CLION/cmake/mac/aarch64/bin:$PATH"; fi
if ! command -v ninja >/dev/null 2>&1; then export PATH="$CLION/ninja/mac/aarch64:$PATH"; fi

GAME="$STAND/valve"
[[ -d "$GAME" ]] || { echo "no valve directory in $STAND" >&2; exit 1; }

cmake --preset "$PRESET" >/dev/null
cmake --build --preset "$PRESET"

DEST="$GAME/addons/lambdabots"
mkdir -p "$DEST/bin" "$DEST/logs"
# A new file, not one rewritten in place: macOS kills a process that maps a rewritten dylib (Code Signature Invalid).
rm -f "$DEST/bin/lambdabots_mm.dylib"
cp "$ROOT/build/$PRESET/lambdabots_mm.dylib" "$DEST/bin/"
if [[ $LINK_CONFIG -eq 1 ]]; then
    for d in config names profiles maps; do
        rm -rf "${DEST:?}/$d"
        ln -s "$ROOT/data/$d" "$DEST/$d"
    done
else
    rsync -a --ignore-existing "$ROOT/data/" "$DEST/"
fi

INI="$GAME/addons/metamod/plugins.ini"
LINE="osx addons/lambdabots/bin/lambdabots_mm.dylib"
[[ -f "$INI.bak-lambdabots" ]] || cp "$INI" "$INI.bak-lambdabots"
grep -qxF "$LINE" "$INI" || printf '\n%s\n' "$LINE" >> "$INI"
if [[ $DISABLE_YAPB -eq 1 ]]; then
    sed -i '' -e 's|^osx addons/yapb/|;osx addons/yapb/|' -e 's|^osx addons/hellorust/|;osx addons/hellorust/|' "$INI"
fi
echo "installed into $DEST"
grep -n "osx" "$INI"
