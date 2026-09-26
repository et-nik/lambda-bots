#!/usr/bin/env bash
# Sends console commands to the stand server started by macos-run.sh and prints the output that follows each.
#
# Usage: scripts/stand/lbcmd.sh [--wait SECONDS] "lb list" ["lb status" ...]
#        scripts/stand/lbcmd.sh --stop
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
if [[ "$(uname)" == Darwin ]]; then
    STAND="${LB_STAND:-$HOME/Git/half-life/xash3d-fwgs-apple-arm64}"
else
    STAND="${LB_STAND:-/opt/hlds}"
fi
RUN="$ROOT/stand-runs/current"
[[ -d "$RUN" ]] || { echo "no stand run in $RUN" >&2; exit 1; }
RUN="$(cd "$RUN" && pwd -P)"
WAIT=1

stop() {
    local pid
    pid="$(cat "$RUN/server.pid" 2>/dev/null || true)"
    printf 'quit\n' >>"$RUN/console.in"
    for _ in $(seq 1 50); do
        [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null || break
        sleep 0.2
    done
    [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true
    pkill -f "tail -n \+1 -f $RUN/console.in" 2>/dev/null || true
    cp -R "$STAND/valve/addons/lambdabots/logs" "$RUN/" 2>/dev/null || true
    echo "stopped: $RUN"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --stop) stop; exit 0 ;;
        --wait) WAIT="$2"; shift 2 ;;
        *) break ;;
    esac
done

for cmd in "$@"; do
    before=$(wc -c <"$RUN/console.log")
    printf '%s\n' "$cmd" >>"$RUN/console.in"
    sleep "$WAIT"
    tail -c +$((before + 1)) "$RUN/console.log"
done
