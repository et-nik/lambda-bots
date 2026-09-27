# lambdabots

Bots for Half-Life 1 Deathmatch, packaged as a Metamod plugin. Behavior is written in Rust; a thin C++ adapter
talks to the engine. The algorithms are based on YaPB and its HLDM port (yapb-halflife), moved onto an
architecture with fair perception, utility-based decisions and verifiable navigation transitions.

Status: **M0 — skeleton and integration.** Bots join the server, respawn and run motor tests; there is no
behavior (combat, navigation) yet. Acceptance results and known limitations are in `docs/m0-acceptance.md`,
the ReHLDS and Windows test stands in `docs/stands/`, design notes in `docs/design/`.

## Platforms

| Platform    | Engine + Metamod                        | Module                  |
|-------------|-----------------------------------------|-------------------------|
| Linux i386  | ReHLDS + Metamod-r (production)         | `lambdabots_mm_i386.so` |
| Windows x86 | HLDS/ReHLDS + Metamod-r                 | `lambdabots_mm.dll`     |
| macOS arm64 | Xash3D FWGS + Metamod-FWGS (test stand) | `lambdabots_mm.dylib`   |

Game DLL: BugfixedHL-Rebased (production) and hlsdk-portable. Only the `valve` mod is supported.

## Installation

1. Extract the release archive into the mod directory so that you get `valve/addons/lambdabots/`.
2. Add a line to `valve/addons/metamod/plugins.ini` (the ready-made line is in the archive's `plugins.ini.txt`):

   ```
   linux addons/lambdabots/bin/lambdabots_mm_i386.so
   ```

   On Windows use `win32 addons/lambdabots/bin/lambdabots_mm.dll`, on macOS —
   `osx addons/lambdabots/bin/lambdabots_mm.dylib`.
3. Configure `addons/lambdabots/config/lambdabots.yaml` (default: fill up to 8 players with personalities of
   normal skill).

The module depends only on glibc 2.27+ (Linux) or system libraries (Windows, macOS).

## Bot personalities

Every nickname is a lasting personality: play style, skill 0–100, model, colors and character traits. Write your
own in `profiles/*.yaml`; for other nicknames the server creates a personality on first join and keeps it in
`data/profiles.yaml`. `lb_difficulty` and `lb_style` do not change personalities, they choose who joins. Details
in `docs/personas.md`.

## Perception

Bots only know what a player could know: players in view once recognized, anonymous sounds with a guessed position,
the HUD damage compass, the kill feed. They never read positions, health or ammo of other players from the server.
How vision, hearing and memory work, and the skill parameters behind them: `docs/perception.md`.

## Behavior

Bots pick a goal (fight, chase, back off, collect an item, roam) by weighted utility with commitment, fight with
yapb's movement and aim model, and choose weapons by expected damage at the distance. Details and the `lb brain`
decision trace: `docs/behavior.md`.

## Commands and cvars

All commands are `lb <subcommand>` in the server console or via rcon. Players listed in `access.admins` can
run them from their own console.

| Command                                | Action                                         |
|----------------------------------------|------------------------------------------------|
| `lb add [n\|name]`                     | add bots or one personality (raises the quota) |
| `lb kick [#userid\|name\|all]`         | kick bots (lowers the quota)                   |
| `lb kill [#userid\|all]`               | kill bots with the `kill` command              |
| `lb quota <n> [normal\|fill\|match]`   | set the quota                                  |
| `lb list`, `lb status`                 | bot list, core state                           |
| `lb roster [all]`, `lb profile <name>` | personalities and their skill parameters       |
| `lb nav`                               | navigation graph and where bots are walking    |
| `lb vision [name]`                     | what bots see, hear and remember               |
| `lb brain [name]`                      | goals, candidates, target, weapon, reactions   |
| `lb perf [reset\|bots]`                | core time per frame, bot command timing        |
| `lb compat`                            | server compatibility profile                   |
| `lb config show\|reload`               | show the config, or reload config and profiles |
| `lb test motor …`                      | motor measurements (requires `lb_dev 1`)       |

| cvar            | Default | Purpose                                             |
|-----------------|---------|-----------------------------------------------------|
| `lb_quota`      | 8       | number of bots or players (depends on the mode)     |
| `lb_quota_mode` | fill    | `normal`, `fill`, `match`                           |
| `lb_difficulty` | normal  | skill filter: any, a preset, `normal-hard`, `40-70` |
| `lb_style`      | any     | style filter: any or `rusher,sniper`                |
| `lb_cmd_rate`   | 100     | bot commands per second (0 — every frame)           |
| `lb_game_mode`  | -1      | forced mode: -1 auto, 0 FFA, 1 teamplay             |
| `lb_gungame`    | auto    | GunGame detection: auto, on, off                    |
| `lb_log_level`  | info    | level of the `logs/lambdabots.<date>.log` file      |
| `lb_dev`        | 0       | debug commands                                      |

## Building

Requires Rust 1.94 (`rust-toolchain.toml`), CMake 3.22+ and Ninja; the Rust staticlib is linked via Corrosion.

```sh
# macOS (Xash3D test stand)
cmake --preset dev-macos && cmake --build --preset dev-macos

# Linux i386 in Docker: Ubuntu 18.04, glibc 2.27, i686 cross toolchain (works on Apple Silicon too)
scripts/build-linux-i386-docker.sh

# Windows x86 from the x86 Developer PowerShell for VS 2022
scripts/build-windows.ps1

# module check: exactly 5 exports, GLIBC <= 2.27, no dynamic C++ runtime
scripts/check-binary.sh build/release-linux-i386/lambdabots_mm_i386.so
```

Rust checks: `cargo test --workspace`, `cargo clippy --workspace --all-targets`,
`cargo xtask layering` (crate layering rules), `cargo xtask abi --check` (C headers are up to date),
`cargo run -p lb-cli -- config check data/`.

## macOS test stand

```sh
scripts/stand/macos-install.sh --disable-yapb --link-config   # build and install into Xash3D
scripts/stand/macos-run.sh --bots 8 --dev --interactive       # server in the background
scripts/stand/lbcmd.sh "lb list" "lb status"                  # commands to the server console
scripts/stand/check-respawn.sh 100                            # 100 kill → respawn cycles
scripts/stand/check-changelevel.sh 20                         # 20 map changes, names preserved
scripts/stand/msec-matrix.sh                                  # msec measurement: fps × lb_cmd_rate
scripts/stand/lbcmd.sh --stop
```

## Layout

| Path                   | Contents                                                      |
|------------------------|---------------------------------------------------------------|
| `adapter/`             | C++ Metamod adapter: hooks, event arena, fake clients, ReHLDS |
| `crates/lb-ffi`        | C ABI between the adapter and the core (source of `lb_abi.h`) |
| `crates/lb-host`       | event arena parsing, command driver (msec, buttons)           |
| `crates/lb-game`       | HLDM message decoders, bot state, rules, sounds, game modes   |
| `crates/lb-bsp`        | BSP loading, exact hull traces, PVS and PAS                   |
| `crates/lb-nav`        | navigation graph, yapb import, planner, path following        |
| `crates/lb-perception` | vision, hearing, damage compass                               |
| `crates/lb-knowledge`  | beliefs: tracks of players, hypotheses from sounds and damage |
| `crates/lb-brain`      | per-bot senses, beliefs and attention                         |
| `crates/lb-runtime`    | frame pipeline, bot manager, commands, cvars, logging         |
| `crates/lb-plugin`     | exported `lb_core_*`, panic isolation                         |
| `data/`                | config, name lists; installed into `addons/lambdabots/`       |
| `docs/design/`         | design notes (platform, AI, navigation, yapb analysis)        |
| `scripts/`             | builds, binary checks, test stands                            |

## License

MIT, see `LICENSE`. Provenance of ported code and data is in `NOTICE` and `THIRD_PARTY_LICENSES.md`.
