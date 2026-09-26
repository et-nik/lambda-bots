#!/usr/bin/env bash
# Runs the ReHLDS test server from scripts/rehlds-vm/provision.sh with lambdabots; same interface as macos-run.sh.
# Run as the hlds user (or root, which switches to it). Stop the systemd service first: systemctl stop lambdabots-hlds.
#
# Usage: scripts/stand/linux-run.sh [--map crossfire] [--bots 8] [--fps 1000] [--maxplayers 24]
#                                   [--duration 120 | --interactive] [--dev] [--set "cvar value"]...
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STAND="${LB_STAND:-/opt/hlds}"
MAP=crossfire
BOTS=8
FPS=1000
DURATION=120
MAXPLAYERS=24
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

if pgrep -f "hlds_linux -game valve" >/dev/null; then
    echo "a server is already running (systemctl stop lambdabots-hlds, or scripts/stand/lbcmd.sh --stop)" >&2
    exit 1
fi

RUN_DIR="$ROOT/stand-runs/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$RUN_DIR"
ln -sfn "$RUN_DIR" "$ROOT/stand-runs/current"
: >"$RUN_DIR/console.in"
chmod a+rw "$RUN_DIR" "$RUN_DIR/console.in"

launch=(./hlds_linux -game valve -port 27015 -pingboost 1 +maxplayers "$MAXPLAYERS" +sys_ticrate "$FPS"
    +lb_quota "$BOTS" ${EXTRA[@]+"${EXTRA[@]}"} +map "$MAP")
cmd="cd '$STAND' && tail -n +1 -f '$RUN_DIR/console.in' | LD_LIBRARY_PATH='$STAND' ${launch[*]} >'$RUN_DIR/console.log' 2>&1 & echo \$! >'$RUN_DIR/server.pid'"
if [[ $EUID -eq 0 ]]; then
    runuser -u hlds -- bash -c "$cmd"
else
    bash -c "$cmd"
fi
echo "server pid $(cat "$RUN_DIR/server.pid"), run dir $RUN_DIR"

if [[ $INTERACTIVE -eq 1 ]]; then
    exit 0
fi
sleep "$DURATION"
exec "$ROOT/scripts/stand/lbcmd.sh" --stop
