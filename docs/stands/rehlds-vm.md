# ReHLDS test stand on a Linux VM

The stand mirrors production: ReHLDS + Metamod-r + BugfixedHL-Rebased + AMX Mod X + GunGame. It covers what the
Mac stand cannot:
- the 32-bit module;
- ReHLDS channels (`SV_StartSound`, `DropClient`, host time; the message manager is only reported, not hooked:
  hooking it crashed a production-like server, see `docs/m5-acceptance.md`);
- compatibility with AMXX and GunGame;
- load at 1000 fps.

You create the VM yourself; the scripts below only provision it and run the checks.

## VM requirements

| Parameter | Value                                  |
|-----------|----------------------------------------|
| OS        | Debian 12 or Ubuntu 22.04/24.04, amd64 |
| CPU       | 2+ cores (4 for load runs)             |
| RAM       | 2 GB                                   |
| Disk      | 5 GB free (HLDS ~1 GB)                 |
| Network   | internet access; UDP 27015 for clients |

## What to copy to the VM

1. The module archive: `dist/lambdabots-<version>-linux-i386.tar.gz`. It is built by the release CI or locally:

   ```sh
   scripts/build-linux-i386-docker.sh
   cargo xtask package --version dev --out dist --linux build/release-linux-i386/lambdabots_mm_i386.so
   ```
2. The repository's `scripts/` directory: VM provisioning and checks.
3. Optionally, the GunGame sources (`~/Git/half-life/hl-gungame`): the `configs`, `data`, `scripting` directories.

```sh
rsync -a scripts dist/lambdabots-dev-linux-i386.tar.gz root@<vm>:/root/lambdabots/
rsync -a ~/Git/half-life/hl-gungame/ root@<vm>:/root/hl-gungame/
```

## Provisioning

On the VM as root:

```sh
cd /root/lambdabots
scripts/rehlds-vm/provision.sh --package lambdabots-dev-linux-i386.tar.gz --gungame /root/hl-gungame
```

The script is idempotent: it can be re-run after a new module build or a version change.

Script steps:
1. Installs i386 libraries and creates the `hlds` user.
2. Downloads steamcmd and HLDS (`app_update 90`, `valve` mod) into `/opt/hlds`.
3. Installs the pinned versions on top:

   | Component          | Version        |
   |--------------------|----------------|
   | ReHLDS             | 3.15.0.896     |
   | Metamod-r          | 1.3.0.149      |
   | BugfixedHL-Rebased | 1.13.2         |
   | AMX Mod X          | 1.10.0-git5486 |

4. Compiles the GunGame plugins with `amxxpc` (by default `gungame` and `gg_respawnItems`; the list is set by
   `--gg-plugins`) and enables them in `amxmodx/configs/plugins.ini`.
5. Extracts lambdabots, adds AMXX and lambdabots to `metamod/plugins.ini`.
6. Writes a test `server.cfg` (`sys_ticrate 1000`, `mp_footsteps 1`, the rcon password is printed at the end) and
   the `lambdabots-hlds` systemd unit (24 slots, crossfire).
7. Does a trial start on port 27016 and prints the `meta list`, `amxx plugins`, `lb compat`, `lb list` output.

## What to check after provisioning

In the trial start output:

| Line                                             | Expected                                   |
|--------------------------------------------------|--------------------------------------------|
| `meta list`                                      | `LambdaBots` and `AMX Mod X` in RUN status |
| `engine:` in `lb compat`                         | `rehlds`, version 3.15                     |
| `rehlds_sv_startsound`, `rehlds_message_manager` | `true`                                     |
| `metamod_hook_tables`                            | `true`                                     |
| `amxx plugins`                                   | GunGame plugins in running status          |

## M0 checks

The server for the checks is started by the stand script, not by the unit, so stop the unit first:
`systemctl stop lambdabots-hlds`.

```sh
cd /root/lambdabots
scripts/stand/linux-run.sh --bots 8 --dev --interactive
scripts/stand/lbcmd.sh "lb list" "lb compat" "meta list"
scripts/stand/check-respawn.sh 100          # 100 kill → respawn cycles
scripts/stand/check-changelevel.sh 20       # 20 map changes, bot names preserved
scripts/stand/lbcmd.sh "lb debug stall 300" "lb perf bots"
scripts/stand/lbcmd.sh --stop
scripts/stand/msec-matrix.sh                # fps 100/500/1000 × lb_cmd_rate every frame/250/100
```

ReHLDS-specific:
- **AMXX sees the bots.** `amx_who` or `status` in the console: bots are listed; GunGame gives them level 1 weapons.
- **Kick via `DropClient`.** `lb kick #<id>` removes the bot immediately, `lb list` shows no `leaving` state.
- **GunGame levels reach the core.** The frags the plugin writes (level × 100) are read from the players' entities:
  `lb gg` shows every player's level right after a level change.

Core logs are in `/opt/hlds/valve/addons/lambdabots/logs/`, the run console in `stand-runs/<time>/console.log`.

## Regular start

`systemctl start lambdabots-hlds` — a server on 27015 for people to play with bots. Commands go through rcon with
the password from `server.cfg`.
