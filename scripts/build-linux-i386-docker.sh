#!/usr/bin/env bash
# Builds lambdabots_mm_i386.so in docker/linux-i386.Dockerfile and checks it with scripts/check-binary.sh.
# The result is build/release-linux-i386/lambdabots_mm_i386.so.
#
# Usage: scripts/build-linux-i386-docker.sh [--preset release-linux-i386] [--test]
#   --test  also runs `cargo test --workspace` for the i686 target inside the container (needs an x86 host or
#           binfmt emulation of i386)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PRESET=release-linux-i386
IMAGE=lambdabots-build-i386:1
TEST=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --preset) PRESET="$2"; shift 2 ;;
        --test) TEST=1; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done

docker build -t "$IMAGE" -f "$ROOT/docker/linux-i386.Dockerfile" "$ROOT/docker"

steps="cmake --preset $PRESET && cmake --build --preset $PRESET && scripts/check-binary.sh build/$PRESET/lambdabots_mm_i386.so"
if [[ $TEST -eq 1 ]]; then
    steps="$steps && cargo test --workspace --target i686-unknown-linux-gnu --target-dir build/$PRESET/cargo-test"
fi

# Not directly under /: Corrosion 0.6 derives an absolute --target-dir from a workspace at /<dir>.
docker run --rm \
    --user "$(id -u):$(id -g)" \
    -v "$ROOT:/work/lambdabots" \
    -v lambdabots-cargo-registry:/opt/cargo/registry \
    -v lambdabots-cargo-git:/opt/cargo/git \
    "$IMAGE" bash -c "$steps"
