#!/usr/bin/env bash
# Chat on the stand: a line by hand, an answer of the model, a dead bot's line before its respawn.
# Needs a stand server started with scripts/stand/macos-run.sh --interactive whose config points the chat at a model;
# for a model without a key, run tools/chat/fake_llm.py and set in config/lambdabots.yaml:
#   chat: { enabled: true, require_humans: false,
#           provider: { kind: openai, base_url: "http://127.0.0.1:8099/v1", api_key_env: "" } }
# Usage: scripts/stand/check-chat.sh <bot name> [answer timeout s=20]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
BOT="${1:?usage: check-chat.sh <bot name> [timeout]}"
TIMEOUT="${2:-20}"
LOG="$ROOT/stand-runs/current/console.log"
failures=0

# Waits until the console shows a line said by the bot after byte `from`; prints it.
said_since() {
    local from="$1" start
    start=$(date +%s)
    while (( $(date +%s) - start < TIMEOUT )); do
        if line=$(tail -c +$((from + 1)) "$LOG" | grep -a -m1 -F "$BOT: "); then
            echo "  said: ${line#*"$BOT": }"
            return 0
        fi
        sleep 0.5
    done
    return 1
}

check() {
    local what="$1"; shift
    local from
    from=$(wc -c <"$LOG")
    "$HERE/lbcmd.sh" --wait 0.3 "$@" | sed 's/^/  /'
    if said_since "$from"; then
        echo "ok: $what"
    else
        echo "FAIL: $what: nothing said in ${TIMEOUT}s"
        failures=$((failures + 1))
    fi
}

"$HERE/lbcmd.sh" --wait 0.3 "lb chat on" >/dev/null
check "a line typed by hand" "lb chat say \"$BOT\" гг"
check "the model's answer to a player" "lb chat test \"$BOT\" привет, как дела"
"$HERE/lbcmd.sh" --wait 0.5 "lb kill all" >/dev/null
# While it types, the bot stays dead: `lb chat status` shows the line, `lb list` the bot dead.
check "a dead bot's line before its respawn" "lb chat say \"$BOT\" ну и ладно" "lb chat status" "lb list"
"$HERE/lbcmd.sh" --wait 0.5 "lb chat status" "lb chat log 12"
echo "chat check: $failures failures"
(( failures == 0 ))
