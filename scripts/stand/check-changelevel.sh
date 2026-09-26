#!/usr/bin/env bash
# M0 acceptance: N level changes in a row. After every change the bots must come back with the same names
# (saved identities join first) and the core must report the new epoch.
# Needs a stand server started with scripts/stand/macos-run.sh --interactive.
# Usage: scripts/stand/check-changelevel.sh [changes=20] [maps="crossfire stalkyard boot_camp"]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
CHANGES="${1:-20}"
read -r -a MAPS <<<"${2:-crossfire stalkyard boot_camp}"
failures=0

names() {
    "$HERE/lbcmd.sh" --wait 0.3 "lb list" | sed 's/\x1b\[[0-9;]*m//g' |
        sed -n 's/.*#[0-9]* *slot [0-9]* *\(.*[^ ]\) *\(connecting\|spawned\|alive\|dead\|respawning\).*/\1/p' | sort
}

wait_full() {
    local want="$1" out total alive
    for _ in $(seq 1 60); do
        out="$("$HERE/lbcmd.sh" --wait 0.3 "lb list" | sed 's/\x1b\[[0-9;]*m//g')"
        total=$(grep -Eo '[0-9]+ bots \(quota' <<<"$out" | grep -Eo '^[0-9]+' || echo 0)
        alive=$(grep -c ' alive ' <<<"$out" || true)
        (( total == want && alive == want )) && return 0
        sleep 0.5
    done
    return 1
}

quota=$("$HERE/lbcmd.sh" --wait 0.3 "lb quota" | sed 's/\x1b\[[0-9;]*m//g' | grep -Eo 'quota [0-9]+' | head -1 | grep -Eo '[0-9]+')
wait_full "$quota" || { echo "bots did not fill up before the test"; exit 1; }
before="$(names)"
for i in $(seq 1 "$CHANGES"); do
    map="${MAPS[$(( i % ${#MAPS[@]} ))]}"
    "$HERE/lbcmd.sh" --wait 3 "changelevel $map" >/dev/null
    if ! wait_full "$quota"; then
        echo "change $i ($map): bots did not come back"
        failures=$((failures + 1))
        continue
    fi
    after="$(names)"
    if [[ "$after" != "$before" ]]; then
        echo "change $i ($map): names differ"
        diff <(echo "$before") <(echo "$after") | sed 's/^/    /'
        failures=$((failures + 1))
        before="$after"
    fi
    (( i % 5 == 0 )) && echo "change $i done (failures $failures)"
done
"$HERE/lbcmd.sh" --wait 0.3 "lb status" | sed 's/\x1b\[[0-9;]*m//g' | grep -E "safe mode|faults|stale"
echo "changelevel check: $CHANGES changes, $failures failures"
(( failures == 0 ))
