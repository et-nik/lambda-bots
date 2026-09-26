#!/usr/bin/env bash
# Prepares a lambdabots test server: HLDS (steamcmd app 90) + ReHLDS + Metamod-r + BugfixedHL-Rebased + AMX Mod X
# + GunGame + lambdabots, on Debian 12 or Ubuntu 22.04/24.04 amd64. Idempotent: rerun after changing a version, the
# package or the GunGame sources. Run as root on the VM; the server itself runs as the `hlds` user.
#
# Usage: provision.sh --package lambdabots-<ver>-linux-i386.tar.gz [--gungame DIR] [--gg-plugins "gungame gg_respawnItems"]
#                     [--dir /opt/hlds] [--rcon PASSWORD]
set -euo pipefail
shopt -s inherit_errexit

REHLDS_VERSION=3.15.0.896
METAMOD_VERSION=1.3.0.149
BHL_VERSION=1.13.2
AMXX_VERSION=1.10.0-git5486

HLDS_USER=hlds
HLDS_DIR=/opt/hlds
PACKAGE=""
GUNGAME=""
GG_PLUGINS="gungame gg_respawnItems"
RCON=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --package) PACKAGE="$2"; shift 2 ;;
        --gungame) GUNGAME="$2"; shift 2 ;;
        --gg-plugins) GG_PLUGINS="$2"; shift 2 ;;
        --dir) HLDS_DIR="$2"; shift 2 ;;
        --rcon) RCON="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[[ $EUID -eq 0 ]] || { echo "run as root" >&2; exit 1; }
[[ -f "$PACKAGE" ]] || { echo "--package lambdabots-<ver>-linux-i386.tar.gz is required" >&2; exit 1; }
PACKAGE="$(realpath "$PACKAGE")"
[[ -z "$GUNGAME" ]] || GUNGAME="$(realpath "$GUNGAME")"

GAME="$HLDS_DIR/valve"
CACHE=/var/cache/lambdabots-vm
as_hlds() { runuser -u "$HLDS_USER" -- "$@"; }
step() { printf '\n== %s\n' "$*"; }

fetch() {
    local url="$1" out
    out="$CACHE/$(basename "$1")"
    if [[ ! -s "$out" ]]; then
        curl -fsSL --retry 3 -o "$out.part" "$url"
        mv "$out.part" "$out"
    fi
    echo "$out"
}

step "packages"
dpkg --add-architecture i386
apt-get update -qq
apt-get install -y -qq ca-certificates curl unzip tar python3 lib32gcc-s1 lib32stdc++6 libc6-i386 >/dev/null
install -d "$CACHE"

step "user $HLDS_USER and steamcmd"
id "$HLDS_USER" >/dev/null 2>&1 || useradd -m -s /bin/bash "$HLDS_USER"
HOME_DIR="$(getent passwd "$HLDS_USER" | cut -d: -f6)"
install -d -o "$HLDS_USER" -g "$HLDS_USER" "$HLDS_DIR" "$HOME_DIR/steamcmd"
if [[ ! -x "$HOME_DIR/steamcmd/steamcmd.sh" ]]; then
    as_hlds bash -c "curl -fsSL https://steamcdn-a.akamaihd.net/client/installer/steamcmd_linux.tar.gz | tar -xz -C '$HOME_DIR/steamcmd'"
fi

step "HLDS (app 90, mod valve)"
# app 90 often needs more than one pass before it reports success.
for attempt in 1 2 3 4; do
    if as_hlds "$HOME_DIR/steamcmd/steamcmd.sh" +force_install_dir "$HLDS_DIR" +login anonymous \
        +app_set_config 90 mod valve +app_update 90 validate +quit; then
        [[ -f "$GAME/liblist.gam" ]] && break
    fi
    echo "steamcmd pass $attempt did not finish, retrying"
done
[[ -f "$GAME/liblist.gam" ]] || { echo "HLDS install failed" >&2; exit 1; }

step "ReHLDS $REHLDS_VERSION"
zip="$(fetch "https://github.com/rehlds/ReHLDS/releases/download/$REHLDS_VERSION/rehlds-bin-$REHLDS_VERSION.zip")"
tmp="$(mktemp -d)"
unzip -qo "$zip" 'bin/linux32/*' -d "$tmp"
cp -a "$tmp/bin/linux32/." "$HLDS_DIR/"
rm -rf "$tmp"

step "BugfixedHL-Rebased $BHL_VERSION"
zip="$(fetch "https://github.com/tmp64/BugfixedHL-Rebased/releases/download/v$BHL_VERSION/BugfixedHL-$BHL_VERSION-server.zip")"
tmp="$(mktemp -d)"
unzip -qo "$zip" -d "$tmp"
cp -a "$tmp/BugfixedHL-$BHL_VERSION-server/valve_addon/." "$GAME/"
rm -rf "$tmp"

step "Metamod-r $METAMOD_VERSION"
zip="$(fetch "https://github.com/rehlds/Metamod-R/releases/download/$METAMOD_VERSION/metamod-bin-$METAMOD_VERSION.zip")"
unzip -qo "$zip" 'addons/metamod/metamod_i386.so' -d "$GAME"
touch "$GAME/addons/metamod/plugins.ini"
sed -i -E 's|^gamedll_linux .*|gamedll_linux "addons/metamod/metamod_i386.so"|' "$GAME/liblist.gam"

step "AMX Mod X $AMXX_VERSION"
tgz="$(fetch "https://www.amxmodx.org/amxxdrop/1.10/amxmodx-$AMXX_VERSION-base-linux.tar.gz")"
tar -xzf "$tgz" -C "$GAME"

step "GunGame"
AMXX="$GAME/addons/amxmodx"
if [[ -n "$GUNGAME" ]]; then
    cp -a "$GUNGAME/configs/." "$AMXX/configs/"
    cp -a "$GUNGAME/data/." "$AMXX/data/"
    cp -a "$GUNGAME/scripting/." "$AMXX/scripting/"
    for p in $GG_PLUGINS; do
        (cd "$AMXX/scripting" && ./amxxpc "$p.sma" "-o../plugins/$p.amxx" >/dev/null) ||
            { echo "failed to compile $p.sma" >&2; exit 1; }
        grep -qxF "$p.amxx" "$AMXX/configs/plugins.ini" || echo "$p.amxx" >>"$AMXX/configs/plugins.ini"
    done
else
    echo "no --gungame directory: GunGame is not installed"
fi

step "lambdabots"
tar -xzf "$PACKAGE" -C "$GAME"
[[ -f "$GAME/addons/lambdabots/bin/lambdabots_mm_i386.so" ]] || { echo "the package has no Linux module" >&2; exit 1; }

step "plugins.ini"
INI="$GAME/addons/metamod/plugins.ini"
for line in "linux addons/amxmodx/dlls/amxmodx_mm_i386.so" "linux addons/lambdabots/bin/lambdabots_mm_i386.so"; do
    grep -qxF "$line" "$INI" || echo "$line" >>"$INI"
done

step "server.cfg"
[[ -n "$RCON" ]] || RCON="$(head -c 12 /dev/urandom | base64 | tr -dc 'a-zA-Z0-9')"
cat >"$GAME/server.cfg" <<CFG
// lambdabots test server (generated by provision.sh)
hostname "lambdabots test"
rcon_password "$RCON"
sv_lan 0
mp_timelimit 30
mp_fraglimit 0
mp_footsteps 1
mp_falldamage 1
mp_weaponstay 0
sys_ticrate 1000
CFG
echo "rcon password: $RCON"

step "systemd unit"
cat >/etc/systemd/system/lambdabots-hlds.service <<UNIT
[Unit]
Description=HLDS for lambdabots tests
After=network-online.target

[Service]
User=$HLDS_USER
WorkingDirectory=$HLDS_DIR
Environment=LD_LIBRARY_PATH=$HLDS_DIR
ExecStart=$HLDS_DIR/hlds_linux -game valve -port 27015 +maxplayers 24 +sys_ticrate 1000 +map crossfire -pingboost 1
Restart=on-failure

[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload

chown -R "$HLDS_USER:$HLDS_USER" "$HLDS_DIR"

step "smoke test: start, check meta list / amxx plugins / lb compat, stop"
RUN="$(mktemp -d)"
chown "$HLDS_USER" "$RUN"
: >"$RUN/console.in"
chown "$HLDS_USER" "$RUN/console.in"
as_hlds bash -c "cd '$HLDS_DIR' && tail -n +1 -f '$RUN/console.in' | LD_LIBRARY_PATH='$HLDS_DIR' ./hlds_linux -game valve \
    -port 27016 +maxplayers 24 +map crossfire >'$RUN/console.log' 2>&1 &"
sleep 15
printf 'meta list\namxx plugins\nlb compat\nlb list\n' >>"$RUN/console.in"
sleep 5
printf 'quit\n' >>"$RUN/console.in"
sleep 3
pkill -f "tail -n \+1 -f $RUN/console.in" || true
grep -E "LambdaBots|AMX Mod X|RUN|FAIL|badf|engine: |rehlds|metamod_hook|bots \(quota" "$RUN/console.log" || true
echo
echo "full console: $RUN/console.log"
echo "done. Start the server with: systemctl start lambdabots-hlds"
