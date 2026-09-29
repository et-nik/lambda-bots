#!/usr/bin/env bash
# Runs the trick scenarios on the stand server started by macos-run.sh with `--set "sv_cheats 1"`: for each set,
# every bot is given what the set needs on spawn (the long jump module, the gauss and its uranium); after SECONDS
# the trick statistics and every bot's report are written to stand-runs/current/tricks/<set>-<difficulty>.txt. The
# bots are hard ones (`lb_difficulty hard`, or `--difficulty`): below the normal preset (skill 50) only the long
# jumps on the way are done, below hard neither bold ones nor long jumps to dodge.
#
#   longjump  the module, the map's weapons: long jump links, long jumps along the way and at enemies
#   gauss     only the gauss (and the crowbar), given: gauss boost links and gauss jumps on the way
#   both      the module and only the gauss
#   plain     the map's weapons, no module: the control for longjump
# (Giving every weapon and its ammo on each spawn litters the map with ammo the bots cannot take, until the server
# runs out of entities.)
#
# Usage: scripts/stand/tricks-scenarios.sh [--seconds 150] [--bots 8] [--difficulty hard] [set ...]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SECONDS_PER=150
BOTS=8
DIFFICULTY=hard
SETS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --seconds) SECONDS_PER="$2"; shift 2 ;;
        --bots) BOTS="$2"; shift 2 ;;
        --difficulty) DIFFICULTY="$2"; shift 2 ;;
        *) SETS+=("$1"); shift ;;
    esac
done
if [[ ${#SETS[@]} -eq 0 ]]; then
    SETS=(longjump gauss both plain)
fi

OUT="$ROOT/stand-runs/current/tricks"
mkdir -p "$OUT"
CMD="$ROOT/scripts/stand/lbcmd.sh"
trap '"$CMD" "lb weapons all" "lb items none" "lb_difficulty normal" >/dev/null || true' EXIT
"$CMD" "lb_difficulty $DIFFICULTY" "lb kick all" "lb_quota $BOTS" >/dev/null
sleep 8
for set in "${SETS[@]}"; do
    case "$set" in
        longjump) setup=("lb weapons all" "lb items longjump") ;;
        gauss) setup=("lb weapons gauss give" "lb items none") ;;
        both) setup=("lb weapons gauss give" "lb items longjump") ;;
        plain) setup=("lb weapons all" "lb items none") ;;
        *) echo "unknown set $set" >&2; exit 2 ;;
    esac
    echo "== $set: $SECONDS_PER s"
    "$CMD" "${setup[@]}" "lb kill all" >/dev/null
    sleep 3
    "$CMD" "lb stats reset" >/dev/null
    sleep "$SECONDS_PER"
    {
        echo "# $set, $DIFFICULTY, $SECONDS_PER s"
        "$CMD" --wait 2 "lb stats"
        "$CMD" --wait 2 "lb brain"
    } | sed -e 's/\x1b\[[0-9;]*m//g' >"$OUT/$set-$DIFFICULTY.txt"
    grep -E "kills by bots|damage from explosions|tricks landed" "$OUT/$set-$DIFFICULTY.txt" | sed 's/^/   /' || true
done
"$CMD" "lb weapons all" "lb items none" "lb_difficulty normal" >/dev/null
trap - EXIT
echo "reports in $OUT"
