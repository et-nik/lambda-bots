#!/usr/bin/env bash
# M0 measurement: how the engine executes bot commands at different server fps and command rates.
# For every sys_ticrate x lb_cmd_rate pair it runs `lb test motor all run` and `... jump` and prints
# max/steady run speed, jump apex and drift (frame time minus msec sent) per bot.
#
# Usage: scripts/stand/msec-matrix.sh [--fps "100 500 1000"] [--rates "0 250 100"] [--bots 4] [--map crossfire]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
FPS_LIST="100 500 1000"
RATES="0 250 100"
BOTS=4
MAP=crossfire
while [[ $# -gt 0 ]]; do
    case "$1" in
        --fps) FPS_LIST="$2"; shift 2 ;;
        --rates) RATES="$2"; shift 2 ;;
        --bots) BOTS="$2"; shift 2 ;;
        --map) MAP="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done

RUNNER="$HERE/macos-run.sh"
[[ "$(uname)" == Darwin ]] || RUNNER="$HERE/linux-run.sh"
OUT="$ROOT/stand-runs/msec-matrix-$(date +%Y%m%d-%H%M%S).txt"
for fps in $FPS_LIST; do
    "$RUNNER" --map "$MAP" --bots "$BOTS" --fps "$fps" --interactive >/dev/null
    sleep $((6 + BOTS * 2))
    for rate in $RATES; do
        "$HERE/lbcmd.sh" --wait 1.5 "lb_cmd_rate $rate" >/dev/null
        "$HERE/lbcmd.sh" --wait 5 "lb test motor all run 3" >/dev/null
        "$HERE/lbcmd.sh" --wait 8 "lb test motor all jump 3" >/dev/null
    done
    grep -o 'motor test .* fps=.*' "$ROOT/stand-runs/current/console.log" | sed 's/\x1b\[[0-9;]*m//g' >>"$OUT" || true
    "$HERE/lbcmd.sh" --stop >/dev/null
    sleep 1
done

python3 - "$OUT" <<'PY'
import re, sys, statistics
rows = {}
for line in open(sys.argv[1]):
    kv = dict(re.findall(r'(\w+)=([^ ]+)', line))
    if 'fps' not in kv:
        continue
    script = line.split()[2]
    key = (round(float(kv['fps']), -1), kv['cmd_rate'], script.split('{')[0].split(' ')[0])
    rows.setdefault(key, []).append(kv)
print(f"{'fps':>6} {'rate':>5} {'test':<5} {'max v':>7} {'steady v':>9} {'apex':>18} {'avg msec':>9} {'drift ms':>9}")
for (fps, rate, test), kvs in sorted(rows.items(), key=lambda x: (x[0][0], x[0][1], x[0][2])):
    maxv = statistics.median(float(k['max_speed2d']) for k in kvs)
    steady = statistics.median(float(k['steady_speed2d']) for k in kvs)
    apexes = [float(a) for k in kvs if k['apex'] != '-' for a in k['apex'].split('/')]
    apex = f"{min(apexes):.1f}..{max(apexes):.1f}" if apexes else "-"
    msec = statistics.median(float(k['avg_msec']) for k in kvs)
    drift = max(abs(float(k['drift_ms'])) for k in kvs)
    print(f"{fps:>6.0f} {rate:>5} {test:<5} {maxv:>7.1f} {steady:>9.1f} {apex:>18} {msec:>9.2f} {drift:>9.2f}")
PY
echo "raw results: $OUT"
