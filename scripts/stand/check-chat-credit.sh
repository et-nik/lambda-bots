#!/usr/bin/env bash
# Chat on the stand while the provider refuses for lack of money: the model gives no line, `lb chat status` and
# `lb chat log` say why, and a game moment still gets a phrase.
# Needs a stand server started with scripts/stand/macos-run.sh --interactive, its config as for check-chat.sh, and the
# fake model refusing every request:
#   tools/chat/fake_llm.py --fail 429
# Its default refusal is Moonshot's to an account out of money, with the account and key ids replaced: type
# exceeded_current_quota_error, "Your account org-test <ak-test> is suspended due to insufficient balance, please
# recharge ...". The script runs `lb chat reload` first, so the test request goes to the fake at once instead of
# waiting out an earlier refusal's 10 minutes. Afterwards, restart the fake without --fail: check-chat.sh runs
# `lb chat reload` too, and the model answers again at once.
# Usage: scripts/stand/check-chat-credit.sh <bot's full in-game name> [answer timeout s=20]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
BOT="${1:?usage: check-chat-credit.sh <bot name> [timeout]}"
TIMEOUT="${2:-20}"
LOG="$ROOT/stand-runs/current/console.log"
# The console shows a line said in the game as "\x02<name>: <text>" (Host_Say); `lb chat status` and `lb chat log`
# print "<name>: ..." too, without the \x02.
SAID=$'\x02'"$BOT: "
failures=0

# Waits until the console shows a line said by the bot after byte `from`; prints it.
said_since() {
    local from="$1" start line
    start=$(date +%s)
    while (( $(date +%s) - start < TIMEOUT )); do
        line=$(tail -c +$((from + 1)) "$LOG" | grep -a -m1 -F "$SAID" || true)
        if [[ -n $line ]]; then
            echo "  said: ${line#*"$SAID"}"
            return 0
        fi
        sleep 0.5
    done
    return 1
}

# The commands must make the bot say something.
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

# The commands must leave the bot quiet for TIMEOUT seconds.
quiet() {
    local what="$1"; shift
    local from
    from=$(wc -c <"$LOG")
    "$HERE/lbcmd.sh" --wait 0.3 "$@" | sed 's/^/  /'
    if said_since "$from"; then
        echo "FAIL: $what: the bot said a line (does fake_llm.py run with --fail 429?)"
        failures=$((failures + 1))
    else
        echo "ok: $what: nothing said in ${TIMEOUT}s"
    fi
}

# The command's output must hold `text`.
shows() {
    local what="$1" text="$2" command="$3" out
    out=$("$HERE/lbcmd.sh" --wait 0.5 "$command")
    printf '%s\n' "$out" | sed 's/^/  /'
    if grep -q -F -- "$text" <<<"$out"; then
        echo "ok: $what"
    else
        echo "FAIL: $what: no \"$text\" in \`$command\`"
        failures=$((failures + 1))
    fi
}

"$HERE/lbcmd.sh" --wait 0.3 "lb chat on" >/dev/null
"$HERE/lbcmd.sh" --wait 1 "lb chat reload" | sed 's/^/  /'
quiet "no line from the model" "lb chat test \"$BOT\" привет"
shows "the status tells the credit is out" "out of API credit" "lb chat status"
# The worker picks the phrase at once. The bot types it when it can: an alive one only once no enemy is about, and it
# lets the line go after 15 s, so a bot in a fight may never say it.
"$HERE/lbcmd.sh" --wait 1 "lb chat event \"$BOT\" win" | sed 's/^/  /'
shows "a phrase without the model" "will type a phrase" "lb chat log 1"
shows "the log tells why the test got no line" "no line (Billing)" "lb chat log 20"
echo "chat credit check: $failures failures"
(( failures == 0 ))
