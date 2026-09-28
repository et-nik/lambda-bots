#!/usr/bin/env bash
# Runs the per-weapon scenarios on the stand server started by macos-run.sh with `--set "sv_cheats 1"`: for each
# weapon set, every bot may use only those weapons (and the crowbar) and is given them on spawn; after SECONDS the
# weapon statistics and every bot's weapon protocols are written to stand-runs/current/weapons/<set>.txt.
#
# Usage: scripts/stand/weapon-scenarios.sh [--seconds 150] [set ...]
#        a set is one or more weapons joined with +, e.g. tripmine+glock; default: every weapon on its own
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SECONDS_PER=150
SETS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --seconds) SECONDS_PER="$2"; shift 2 ;;
        *) SETS+=("$1"); shift ;;
    esac
done
if [[ ${#SETS[@]} -eq 0 ]]; then
    SETS=(crowbar glock python mp5 shotgun crossbow rpg gauss egon hornetgun handgrenade satchel tripmine+glock snark)
fi

OUT="$ROOT/stand-runs/current/weapons"
mkdir -p "$OUT"
CMD="$ROOT/scripts/stand/lbcmd.sh"
for set in "${SETS[@]}"; do
    weapons="${set//+/ }"
    echo "== $set: $SECONDS_PER s"
    "$CMD" "lb weapons $weapons give" "lb kill all" >/dev/null
    sleep 3
    "$CMD" "lb stats reset" >/dev/null
    sleep "$SECONDS_PER"
    {
        echo "# $set, $SECONDS_PER s"
        "$CMD" --wait 2 "lb stats"
        "$CMD" --wait 2 "lb brain"
    } | sed -e 's/\x1b\[[0-9;]*m//g' >"$OUT/$set.txt"
    grep -E "kills by bots|damage from explosions|\]   weapon_|failures" "$OUT/$set.txt" | sed 's/^/   /' || true
done
"$CMD" "lb weapons all" >/dev/null
echo "reports in $OUT"
