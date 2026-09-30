#!/usr/bin/env bash
# Builds lb-editor with its page (tools/editor) built into it. Without options for this machine; with --linux a
# static Linux x86_64 binary (musl), cross-built with the toolchain's own rust-lld: no Docker or cross compiler.
# The result is build/editor/<target>/lb-editor.
#
# Usage: scripts/build-editor.sh [--linux] [--no-page]
#   --no-page  keep tools/editor/dist as it is (the page is built with npm otherwise)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LINUX=0
PAGE=1
while [[ $# -gt 0 ]]; do
    case "$1" in
        --linux) LINUX=1; shift ;;
        --no-page) PAGE=0; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done

if [[ $PAGE -eq 1 ]]; then
    (cd "$ROOT/tools/editor" && npm ci --no-audit --no-fund && npm run build)
fi
[[ -f "$ROOT/tools/editor/dist/index.html" ]] || { echo "no page in tools/editor/dist" >&2; exit 1; }

cd "$ROOT"
if [[ $LINUX -eq 1 ]]; then
    TARGET=x86_64-unknown-linux-musl
    rustup target add "$TARGET" >/dev/null
    export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
    # blake3 assembles its SIMD code with the C compiler: any clang assembles for the target.
    export CC_x86_64_unknown_linux_musl="${CC_x86_64_unknown_linux_musl:-clang}"
    export CFLAGS_x86_64_unknown_linux_musl="--target=$TARGET"
    if [[ "$(uname)" == Darwin ]]; then
        AR_x86_64_unknown_linux_musl="$(xcrun -f ar)"
        export AR_x86_64_unknown_linux_musl
    fi
    cargo build -p lb-editor --release --target "$TARGET"
    BIN="target/$TARGET/release/lb-editor"
else
    TARGET=host
    cargo build -p lb-editor --release
    BIN="target/release/lb-editor"
fi

OUT="build/editor/$TARGET"
mkdir -p "$OUT"
# A new file renamed over the old one: an editor running from it keeps its own copy.
cp "$BIN" "$OUT/lb-editor.new"
mv -f "$OUT/lb-editor.new" "$OUT/lb-editor"
echo "$ROOT/$OUT/lb-editor"
