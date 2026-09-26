#!/usr/bin/env bash
# M0 acceptance: kill every bot N times in a row and require all of them to respawn each time.
# Needs a stand server started with scripts/stand/macos-run.sh --interactive.
# Usage: scripts/stand/check-respawn.sh [cycles=100] [timeout_s=8]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
CYCLES="${1:-100}"
TIMEOUT="${2:-8}"
failures=0
worst=0

count_states() {
    local out
    out="$("$HERE/lbcmd.sh" --wait 0.25 "lb list")"
    total=$(grep -Eo '[0-9]+ bots \(quota' <<<"$out" | grep -Eo '^[0-9]+' || echo 0)
    alive=$(grep -c ' alive ' <<<"$out" || true)
}

for cycle in $(seq 1 "$CYCLES"); do
    "$HERE/lbcmd.sh" --wait 0.1 "lb kill all" >/dev/null
    start=$(date +%s)
    ok=0
    sleep 1
    while (( $(date +%s) - start < TIMEOUT )); do
        count_states
        if (( total > 0 && alive == total )); then ok=1; break; fi
        sleep 0.25
    done
    took=$(( $(date +%s) - start ))
    (( took > worst )) && worst=$took
    if (( ok == 0 )); then
        failures=$((failures + 1))
        echo "cycle $cycle: only $alive of $total bots alive after ${TIMEOUT}s"
    fi
    (( cycle % 10 == 0 )) && echo "cycle $cycle done (failures $failures, worst ${worst}s)"
done
echo "respawn check: $CYCLES cycles, $failures failures, worst respawn ${worst}s"
(( failures == 0 ))
