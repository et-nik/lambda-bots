#!/usr/bin/env bash
# Runs a dedicated Xash3D FWGS server on the macOS stand with lambdabots.
#
# Usage: scripts/stand/macos-run.sh [--map crossfire] [--bots 8] [--fps 1000] [--maxplayers 16]
#                                   [--duration 120 | --interactive] [--dev] [--set "cvar value"]...
#
# The console reads commands from stand-runs/<run>/console.in; stand-runs/current points at the last run.
# With --interactive the script returns right after the start: send commands with scripts/stand/lbcmd.sh
# and stop the server with scripts/stand/lbcmd.sh --stop.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STAND="${LB_STAND:-$HOME/Git/half-life/xash3d-fwgs-apple-arm64}"
MAP=crossfire
BOTS=8
FPS=1000
DURATION=120
MAXPLAYERS=16
INTERACTIVE=0
EXTRA=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --map) MAP="$2"; shift 2 ;;
        --bots) BOTS="$2"; shift 2 ;;
        --fps) FPS="$2"; shift 2 ;;
        --duration) DURATION="$2"; shift 2 ;;
        --maxplayers) MAXPLAYERS="$2"; shift 2 ;;
        --interactive) INTERACTIVE=1; shift ;;
        --dev) EXTRA+=(+lb_dev 1); shift ;;
        --set) read -r -a kv <<<"$2"; EXTRA+=("+${kv[0]}" "${kv[@]:1}"); shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done

if pgrep -f "xash3d -dedicated" >/dev/null; then
    echo "a dedicated server is already running; stop it with scripts/stand/lbcmd.sh --stop" >&2
    exit 1
fi

RUN_DIR="$ROOT/stand-runs/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$RUN_DIR"
ln -sfn "$RUN_DIR" "$ROOT/stand-runs/current"
: >"$RUN_DIR/console.in"

# sv_hibernate_when_empty does not count bots as players and would throttle the server to ~25 fps.
cd "$STAND"
tail -n +1 -f "$RUN_DIR/console.in" | ./xash3d -dedicated -game valve -port 27015 \
    +maxplayers "$MAXPLAYERS" +sys_ticrate "$FPS" +sv_hibernate_when_empty 0 \
    +lb_quota "$BOTS" ${EXTRA[@]+"${EXTRA[@]}"} +map "$MAP" >"$RUN_DIR/console.log" 2>&1 &
echo $! >"$RUN_DIR/server.pid"
echo "server pid $(cat "$RUN_DIR/server.pid"), run dir $RUN_DIR"

if [[ $INTERACTIVE -eq 1 ]]; then
    exit 0
fi
sleep "$DURATION"
exec "$ROOT/scripts/stand/lbcmd.sh" --stop
