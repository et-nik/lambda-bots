#!/usr/bin/env bash
# Runs the per-style scenarios on the stand server started by macos-run.sh with `--set "sv_cheats 1"`: for each
# style, the bots are replaced by personalities of that style (trappers get tripmines and satchels on every spawn);
# after SECONDS every bot's decisions and the weapon statistics are written to stand-runs/current/styles/<style>.txt,
# and each bot's goal line is printed.
#
# Usage: scripts/stand/style-scenarios.sh [--seconds 180] [--bots 8] [style ...]
#        styles: balanced rusher sniper controller trapper (default: all five)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SECONDS_PER=180
BOTS=8
STYLES=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --seconds) SECONDS_PER="$2"; shift 2 ;;
        --bots) BOTS="$2"; shift 2 ;;
        *) STYLES+=("$1"); shift ;;
    esac
done
if [[ ${#STYLES[@]} -eq 0 ]]; then
    STYLES=(balanced rusher sniper controller trapper)
fi

OUT="$ROOT/stand-runs/current/styles"
mkdir -p "$OUT"
CMD="$ROOT/scripts/stand/lbcmd.sh"
trap '"$CMD" "lb_style any" "lb weapons all" >/dev/null || true' EXIT
for style in "${STYLES[@]}"; do
    echo "== $style: $SECONDS_PER s"
    "$CMD" "lb_style $style" "lb kick all" >/dev/null
    if [[ $style == trapper ]]; then
        "$CMD" "lb weapons glock mp5 tripmine satchel give" >/dev/null
    else
        "$CMD" "lb weapons all" >/dev/null
    fi
    "$CMD" --wait 8 "lb quota $BOTS" >/dev/null
    "$CMD" "lb stats reset" >/dev/null
    sleep "$SECONDS_PER"
    {
        echo "# $style, $SECONDS_PER s"
        "$CMD" --wait 2 "lb brain"
        "$CMD" --wait 2 "lb stats"
        "$CMD" --wait 1 "lb map danger"
    } | sed -e 's/\x1b\[[0-9;]*m//g' >"$OUT/$style.txt"
    grep -E "\((balanced|rusher|sniper|controller|trapper) [0-9]+|mind:|kills by bots" "$OUT/$style.txt" \
        | sed -e 's/^\[[0-9:]*\] //' -e 's/^/   /' | cut -c1-400 || true
done
echo "reports in $OUT"
