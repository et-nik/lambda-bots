#!/usr/bin/env bash
# Chat on the stand: a line by hand, once in the chat log under the map's header; an answer of the model; a phrase for
# a game moment; a dead bot's line before its respawn.
# Needs a stand server started with scripts/stand/macos-run.sh --interactive whose config points the chat at a model;
# for a model without a key, run tools/chat/fake_llm.py --line привет (its default answers include `-`, which leaves
# the bot quiet) and set in config/lambdabots.yaml:
#   chat: { enabled: true, require_humans: false, phrases: { ai_share: 0 },
#           provider: { kind: openai, base_url: "http://127.0.0.1:8099/v1", api_key_env: "" } }
# Keep `phrases: { ai_share: 0 }` with a real model too: every game moment then gets a phrase, so `lb chat event`
# always types one. The chat log (chat.chatlog, on by default) is read from the stand ($LB_STAND, as in lbcmd.sh):
# valve/addons/lambdabots/logs/chatlog.<UTC date>.log.
# Usage: scripts/stand/check-chat.sh <bot's full in-game name> [answer timeout s=20]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
BOT="${1:?usage: check-chat.sh <bot name> [timeout]}"
TIMEOUT="${2:-20}"
LOG="$ROOT/stand-runs/current/console.log"
if [[ "$(uname)" == Darwin ]]; then
    STAND="${LB_STAND:-$HOME/Git/half-life/xash3d-fwgs-apple-arm64}"
else
    STAND="${LB_STAND:-/opt/hlds}"
fi
CHATLOGS="$STAND/valve/addons/lambdabots/logs"
# The console shows a line said in the game as "\x02<name>: <text>" (Host_Say); `lb chat status` and `lb chat log`
# print "<name>: ..." too, without the \x02.
SAID=$'\x02'"$BOT: "
HEADER='^---- .+ ----$'
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

# Today's chat log: one file per UTC day.
chatlog_today() {
    echo "$CHATLOGS/chatlog.$(date -u +%F).log"
}

size_of() {
    if [[ -f $1 ]]; then echo $(( $(wc -c <"$1") )); else echo 0; fi
}

# Goes through `file` past byte `from`, counting the bot's гг lines: as our bot's (`mine`; the first one goes to
# `first`, the map's header above it to `under`) and as a player's (`theirs`).
scan_chatlog() {
    local file="$1" from="$2" header="" l
    [[ -f $file ]] || return 0
    if (( from > 0 )); then
        header=$(head -c "$from" "$file" | grep -a -E "$HEADER" | tail -n 1 || true)
    fi
    while IFS= read -r l || [[ -n $l ]]; do
        if [[ $l =~ $HEADER ]]; then
            header="$l"
        elif [[ $l == *"» $BOT: гг" || $l == *"» $BOT: гг)" ]]; then
            mine=$((mine + 1))
            if (( mine == 1 )); then first="$l" under="$header"; fi
        elif [[ $l == *"   $BOT: гг" || $l == *"   $BOT: гг)" ]]; then
            theirs=$((theirs + 1))
        fi
    done < <(tail -c +$((from + 1)) "$file")
}

# The line typed by hand reaches the chat log once, marked `»` as our bot's, under a "---- <map> ----" header. With an
# old game DLL the bot types `гг)`: that DLL drops a line without an ASCII character.
check_chatlog() {
    local file="$1" from="$2" today why=""
    mine=0 theirs=0 first="" under=""
    # The chat log has the line before the game shows it; the wait leaves time for a doubled one to show up.
    sleep 1
    scan_chatlog "$file" "$from"
    today=$(chatlog_today)
    if [[ $today != "$file" ]]; then scan_chatlog "$today" 0; fi
    if (( mine == 0 )); then
        why="no line \"» $BOT: гг\" in $file"
    elif (( mine > 1 )); then
        why="\"» $BOT: гг\" written $mine times"
    elif (( theirs > 0 )); then
        why="the bot's line also written as a player's"
    elif [[ -z $under ]]; then
        why="no \"---- <map> ----\" line above \"$first\""
    fi
    if [[ -z $why ]]; then
        echo "  logged: $first"
        echo "  under: $under"
        echo "ok: the line in the chat log"
    else
        echo "FAIL: the line in the chat log: $why"
        failures=$((failures + 1))
    fi
}

"$HERE/lbcmd.sh" --wait 0.3 "lb chat on" >/dev/null
# The worker tries the provider at once, after an earlier refusal too (check-chat-credit.sh leaves one).
"$HERE/lbcmd.sh" --wait 1 "lb chat reload" | sed 's/^/  /'
chatlog=$(chatlog_today)
chatlog_size=$(size_of "$chatlog")
check "a line typed by hand" "lb chat say \"$BOT\" гг"
check_chatlog "$chatlog" "$chatlog_size"
check "the model's answer to a player" "lb chat test \"$BOT\" привет, как дела"
check "a phrase for a game moment" "lb chat event \"$BOT\" win"
"$HERE/lbcmd.sh" --wait 0.5 "lb kill all" >/dev/null
# While it types, the bot stays dead: `lb chat status` shows the line, `lb list` the bot dead.
check "a dead bot's line before its respawn" "lb chat say \"$BOT\" ну и ладно" "lb chat status" "lb list"
"$HERE/lbcmd.sh" --wait 0.5 "lb chat status" "lb chat log 12"
echo "chat check: $failures failures"
(( failures == 0 ))
