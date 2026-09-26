#!/usr/bin/env bash
# Builds and checks lambdabots modules; optionally packs them into dist/.
#
# Usage: scripts/build.sh [macos] [linux-i386] [--debug] [--package VERSION]
#   no targets: macos on a Mac, linux-i386 elsewhere. Windows: scripts/build-windows.ps1.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGETS=()
DEBUG=0
VERSION=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        macos | linux-i386) TARGETS+=("$1"); shift ;;
        --debug) DEBUG=1; shift ;;
        --package) VERSION="$2"; shift 2 ;;
        *) echo "unknown argument $1" >&2; exit 2 ;;
    esac
done
if [[ ${#TARGETS[@]} -eq 0 ]]; then
    [[ "$(uname)" == Darwin ]] && TARGETS=(macos) || TARGETS=(linux-i386)
fi

CLION="$HOME/Applications/CLion.app/Contents/bin"
command -v cmake >/dev/null 2>&1 || export PATH="$CLION/cmake/mac/aarch64/bin:$PATH"
command -v ninja >/dev/null 2>&1 || export PATH="$CLION/ninja/mac/aarch64:$PATH"

cd "$ROOT"
package_args=()
for target in "${TARGETS[@]}"; do
    case "$target" in
        macos)
            preset=release-macos
            [[ $DEBUG -eq 1 ]] && preset=dev-macos
            cmake --preset "$preset" >/dev/null
            cmake --build --preset "$preset"
            scripts/check-binary.sh "build/$preset/lambdabots_mm.dylib"
            package_args+=(--macos "build/$preset/lambdabots_mm.dylib")
            ;;
        linux-i386)
            scripts/build-linux-i386-docker.sh
            package_args+=(--linux build/release-linux-i386/lambdabots_mm_i386.so)
            ;;
    esac
done

if [[ -n "$VERSION" ]]; then
    cargo xtask package --version "$VERSION" --out dist "${package_args[@]}"
fi
