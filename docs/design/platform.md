# Platform: C++ adapter, C ABI, build, runtime, operations

> Original design note (in English), prepared during planning on 2026-09-26.
> A condensed version with the final decisions lives in the implementation plan; in case of conflict, the plan
> and later decisions in the repository take precedence. `file:line` references reflect the sources as of the note's date.

# LambdaBots: platform, engine integration, C ABI, build, runtime and ops design

## 0. Summary

The binary is split in two:

- **Thin C++17 adapter (about 3 kLOC).** It knows nothing about HLDM. It is the native half of the `EngineDriver`.
- **Rust staticlib.** It holds all logic, including HLDM message interpretation (`HldmAdapter`).

CMake with Corrosion links them into one module. That module exports exactly 5 symbols.

**Core decisions**

- **No behaviour code inside hooks.** Every engine or metamod hook only appends to a per-frame event arena. Rust runs only in these places:
  - `StartFrame` pre and post
  - map start and map end
  - the `lb` server command
  - init, shutdown and fatal

  Rust calls back into the adapter synchronously through a versioned `LbHostApi` table, on the main thread only.
- **Other plugins see our bots.** Bot `ClientConnect`, `ClientPutInServer` and `ClientCommand` go through metamod's *hooked* DLL table (`GET_HOOK_TABLES`), so AMXX and GunGame see bots. yapb's `MDLL_*` calls bypassed other plugins.
- **Why message capture has two backends.** Metamod gives every plugin the *raw* engine table (`metamod-r/metamod/src/metamod.cpp:164-170`). So messages sent by other plugins, such as AMXX GunGame's `ScoreInfo`, never reach our `MessageBegin` hooks.
  - On ReHLDS, `IMessageManager` hooks are the preferred message backend, because they see every sender.
  - Everywhere else, `pev->frags` is read from snapshots as the fallback.
- **Honest engine interaction.** Only `RunPlayerMove` and client commands change the world. The adapter also emulates two duties that real clients and the network layer perform but fake clients lack:
  - **fixangle acknowledgement.** The engine clears `fixangle` only when it writes the client datagram, and it skips fake clients (`rehlds/.../sv_main.cpp:1287-1302, 5259`; `xash .../sv_frame.c:540-554`). Without clearing it, a bot's `v_angle` stays frozen after spawn or teleport. BHL sets fixangle on every spawn (`gamerules.cpp:83`).
  - **CmdStart random seed.** ReHLDS passes seed 0 for bots (`pr_cmds.cpp:2064`).
- **Command emission is quantized** by a "virtual client rate" with exact msec accounting. We never send more time than has elapsed. This matters because the speed-hack guard `clockwindow` / `net_clockwindow` also applies to fake clients (`rehlds sv_main.cpp:8177-8206`, `xash sv_main.c:265-290`). The default rate is chosen by M0 measurement.

**Deviations from v2 (with reasons)**

1. **Supported engines.** §14 "integration/rehlds" becomes three engines. The ReHLDS API is optional and runtime-probed (user decision).
2. **Motor timing (§5.2).** Motor state is serviced every frame, but *engine commands* go out at `lb_cmd_rate`, not every frame, when the server runs at 500–1000 fps. Reasons:
   - human clients also send commands at their own rate;
   - running `PM_Move` 1000 times per second per bot is about 10× a human's cost;
   - it is clockwindow-safe.

   M0 validates this against per-frame emission.
3. **Allowed engine writes (invariant 2).** A short, documented list of "network-layer emulation writes" is allowed (§3.10). Nothing else writes engine state.
4. **Raw world frame.** Players are pushed every frame. Entities are pulled on demand under the perception budget.
5. **Glass and transparency.** LOS through transparent brushes uses side-effect-free "pierce" traces. yapb instead cleared `FL_WORLDBRUSH`, which also changes `SV_ClipToWorldbrush` semantics for everyone (`rehlds world.cpp:1206,1295`).
6. **Extra honest channel.** The bot's own client-prediction data (`GetWeaponData`, `UpdateClientData` called read-only for the bot) is exactly what a human client receives. It is added as a M2 channel for weapon readiness and gauss charge.
7. **Lifecycle.** Adds `Leaving` and `Faulted` states. The v2 C++ sketches become Rust types, owned by the AI architect.

---

## 1. Repository layout (`/Users/nikita/Git/hl-bots`)

```
Cargo.toml              workspace (edition 2024, license MIT, [workspace.dependencies], profiles: panic="unwind" everywhere,
                        release: lto="thin", codegen-units=1, debug="line-tables-only")
rust-toolchain.toml     channel="1.94.0"; targets aarch64-apple-darwin, i686-unknown-linux-gnu, i686-pc-windows-msvc
.cargo/config.toml      [target.i686-pc-windows-msvc] rustflags=["-C","target-feature=+crt-static"]; alias xtask
deny.toml               license allowlist (MIT/Apache/BSD/ISC/Zlib/Unicode), advisories, bans (serde_yaml, serde_yml)
clippy.toml             disallowed-types std HashMap/HashSet (determinism) in logic crates; large_stack_frames
CMakeLists.txt  CMakePresets.json  cmake/{LbPlatform.cmake, Corrosion.cmake, toolchains/linux-i386.cmake}
adapter/
  include/lb/lb_abi.h           GENERATED (cbindgen), committed
  include/lb/lb_abi_layout.inc  GENERATED size/offset static_asserts
  include/lb/metamod_abi.h      own MIT clean-room decl. of metamod 5:13 plugin ABI (~150 lines)
  src/  (file list in §3.1)     exports/{lambdabots.map, lambdabots.def, lambdabots_macos.txt}
crates/  lb-ffi lb-core lb-host lb-config lb-game lb-telemetry lb-ext lb-runtime lb-plugin lb-testkit lb-cli
         lb-bsp lb-nav (nav architect)   lb-sense lb-ai (AI architect)
xtask/                  abi gen/check, layering check, package, stand install, glibc/exports check
third_party/hlsdk/      hlsdk-portable header subset (Valve SDK license) + VENDORED.md (pinned commit)
third_party/rehlds/     ReHLDS public API headers (MIT since 2025, see rehlds/LICENSE-TRANSITION.md) + VENDORED.md
data/                   → packaged as addons/lambdabots/ (config/, profiles/, names/, maps/, amxx/)
tools/observer/         bridge.py index.html fake_server.py bsp2json.py soak_assert.py protocol.md schemas/ sign.py
scripts/  docker/linux-i386.Dockerfile  docs/  .github/workflows/{ci.yml,release.yml}
LICENSE (MIT)  NOTICE (YaPB MIT copyright, PODBot credit, observer tool origin)  THIRD_PARTY_LICENSES.md
```

**Crate boundaries** enforce v2 access rules through the Cargo graph. A crate cannot `use` a transitive dependency, and `xtask layering` fails CI on forbidden edges.

| Crate | Type | Responsibility | May depend on | Must NOT depend on |
|---|---|---|---|---|
| lb-ffi | lib | `#[repr(C)]` ABI types, constants, fn-pointer table types. No logic. | none | anything |
| lb-core | lib | SimTime/clock, `MapEpoch` and handles, RNG streams, bounded queues, token buckets and EDF queue, `TraceService`/`LosService` traits, `TraceSink`, atomic-write fs helper, glam re-export | none | lb-ffi, lb-host |
| lb-host | lib | Rust half of EngineDriver: `Host` trait, `FfiHost`, `HostCtx<'a>`, raw sensor types (`RawWorldFrame`), event-arena decoder, command emission (msec, button latch, seed), `RecordingHost`/`ReplayHost` | lb-ffi, lb-core | lb-ai, lb-nav |
| lb-config | lib | YAML schemas, loader, validation, provenance, versioned snapshots | lb-core | lb-host |
| lb-game | lib | HldmAdapter: decoders, SelfState, PublicRules, GameMode (FFA/TDM/GunGame), weapon/item tables, entity track rules, sound/event classification, CompatibilityProfile model | lb-core, lb-host, lb-config | lb-ai |
| lb-sense *(AI arch.)* | lib | sensors: raw data → observations | lb-core, lb-host (raw only), lb-game | — |
| lb-ai *(AI arch.)* | lib | beliefs, decision, actions, combat, motor, CommandEncoder | lb-core, lb-config, lb-game (model only), lb-sense (Observation), lb-nav | **lb-host, lb-ffi** |
| lb-bsp / lb-nav *(nav arch.)* | lib | BSP, graph, generation, paths | lb-core, lb-config | **lb-host** (traces go through lb-core traits) |
| lb-telemetry | lib | protocol v2, encoder, UDP sink, rate limits, HMAC command channel, trace rings, crash dumps | lb-core | lb-host, lb-ai |
| lb-ext | lib | event bus, extension traits (scripting host), mlua later | lb-core | lb-host |
| lb-chat | lib | chat: journal of the map from public events, director (who speaks, limits), a bot's line from request to `say`, prompts, text rules, memory of players | lb-core, lb-styles | lb-host |
| lb-llm | lib | chat models over HTTPS (Anthropic Messages, OpenAI-compatible), blocking, for a worker thread | — | lb-host |
| lb-runtime | lib | frame pipeline, scheduler, budgets, workers, BotManager and QuotaPolicy, commands/cvars, config apply, compat assembly, diagnostics | all | — |
| lb-plugin | staticlib+rlib | `extern "C" lb_core_*`, `ffi_guard` (catch_unwind), global Runtime | lb-runtime, lb-host, lb-ffi | — |
| lb-testkit | lib (dev) | MockHost, frame builders, recorded message fixtures, scenario DSL | lb-host, lb-runtime | — |
| lb-cli | bin | `config check/schema/migrate`, `replay`, `nav build`, `bsp dump`, `abi dump`, `names import` | most | — |

**Dependencies and rationale**

- **Logging:** `tracing`, `tracing-subscriber`, `tracing-appender` (non-blocking file writer).
- **Errors:** `thiserror` 2 in libraries; `anyhow` only in lb-cli and xtask.
- **YAML:** `serde-saphyr`.
  - It is pure Rust, maintained, supports both ser and de, prints error snippets with line and column, and has a configurable `Budget` against malicious input. It requires Rust ≥ 1.89.
  - `serde_yaml` is deprecated. `serde_yml` has advisory RUSTSEC-2025-0068. `serde_norway` shows no releases since Dec 2024.
  - Wrap it behind `lb_config::yaml` so it can be swapped (for example to `noyalib`).
- **JSON:** `serde_json` for telemetry. `schemars` behind a feature, for editor JSON Schemas.
- **RNG:** `rand_core` + `rand_pcg`, with our own distributions in `lb-core::rng`. The algorithm and output must stay stable across crate versions for replay.
- **Concurrency:** `crossbeam-channel` (bounded MPMC), `parking_lot`, `arc-swap` for config snapshots.
- **Collections:** `rustc-hash` (deterministic FxHashMap), `indexmap`, `smallvec`, `arrayvec`, `bitflags`.
- **Math:** `glam` (shared math type, to agree with the AI architect).
- **Hashing:** `xxhash-rust` (xxh3-128) for fingerprints.
- **Command-channel auth:** `hmac` + `sha2` + `subtle` + `getrandom`.
- **Thread priority:** `libc` / `windows-sys`.
- **Dev only:** `proptest`, `insta`, `criterion`.
- **xtask only:** `cbindgen`, `cargo_metadata`, `xshell`.
- **Later:** `mlua` (`lua54`, `vendored`).

Web sources used for crate status: [Rust forum: serde_yaml deprecation](https://users.rust-lang.org/t/serde-yaml-deprecation-alternatives/108868), [serde_yml on crates.io](https://crates.io/crates/serde_yml), [2026 status comparison](https://github.com/Takazudo/zudo-front-builder/issues/2755), [serde-saphyr](https://github.com/bourumir-wyngs/serde-saphyr), [Corrosion releases](https://github.com/corrosion-rs/corrosion/releases).

---

## 2. Build system

**Decision: CMake at the top level, with Corrosion linking the Rust staticlib.** Exports are defined in C++.

Why this and not cargo at the top with the `cc` crate:

1. **Entry points are ABI-sensitive and belong in C/C++.**
   - `GiveFnptrsToDll` is `__stdcall` on i686 MSVC. It needs a `.def` alias `GiveFnptrsToDll=_GiveFnptrsToDll@8`, like yapb's `linkage.cpp:1034-1049`.
   - On i386 GCC we need `__attribute__((force_align_arg_pointer))` on every function the engine or ReHLDS calls. Rust has no equivalent attribute.
2. **CLion is the IDE.** The C++ adapter gets full code insight from CMake. Cargo plus `cc` gives no `compile_commands.json`.
3. **Corrosion handles the hard linking parts.** It resolves Rust std's native libs automatically and leaves the MSVC runtime choice to CMake.
4. **Pure Rust crates stay plain cargo.** `cargo test`/`clippy` work without CMake.

Costs of this choice:

- Corrosion ≥ 0.6 needs CMake ≥ 3.22. The Docker image installs Kitware CMake. On the Mac, scripts use `$CMAKE`, then `PATH`, then `~/Applications/CLion.app/Contents/bin/cmake/mac/aarch64/bin/cmake`. Ninja comes from `.../bin/ninja/mac/aarch64/ninja`.

**Per-platform settings**

| | Linux i386 (prod) | Windows x86 | macOS arm64 (stand) |
|---|---|---|---|
| Rust target | i686-unknown-linux-gnu | i686-pc-windows-msvc, `+crt-static` | aarch64-apple-darwin |
| C++ | g++ -m32, `-fvisibility=hidden -fno-exceptions -fno-rtti`, `-static-libstdc++ -static-libgcc` (HLDS ships an old libstdc++) | MSVC `/MT` (`CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`), `/GR-` | clang, `CMAKE_OSX_DEPLOYMENT_TARGET=11.0` |
| Output | `lambdabots_mm_i386.so` | `lambdabots_mm.dll` | `lambdabots_mm.dylib` (SHARED, SUFFIX set explicitly) |
| Exports | version script `exports/lambdabots.map`: global `GiveFnptrsToDll; Meta_Init; Meta_Query; Meta_Attach; Meta_Detach; local: *;` plus `-Wl,--exclude-libs,ALL -Wl,--no-undefined -Wl,--gc-sections` | `exports/lambdabots.def` | `-Wl,-exported_symbols_list,exports/lambdabots_macos.txt` |
| Gate | `nm -D --defined-only` equals the expected 5 symbols; highest `GLIBC_x` ≤ configured max; no `GLIBCXX` | `dumpbin /exports` | `nm -gU` |

The root `CMakeLists.txt` in outline:

```cmake
cmake_minimum_required(VERSION 3.22)
project(lambdabots LANGUAGES C CXX)
include(cmake/LbPlatform.cmake)        # LB_PLATFORM_TAG, lb_configure_plugin_target()
include(cmake/Corrosion.cmake)         # FetchContent corrosion v0.6.x (URL + SHA256)
corrosion_import_crate(MANIFEST_PATH Cargo.toml CRATES lb-plugin CRATE_TYPES staticlib PROFILE ${LB_CARGO_PROFILE})
corrosion_set_env_vars(lb_plugin LB_BUILD_VERSION=${LB_VERSION})   # one version string for C++ and Rust
add_library(lambdabots_mm SHARED ${LB_ADAPTER_SOURCES})
target_include_directories(lambdabots_mm PRIVATE adapter/include third_party/hlsdk/{common,engine,pm_shared,dlls})
target_link_libraries(lambdabots_mm PRIVATE lb_plugin)
lb_configure_plugin_target(lambdabots_mm)
add_custom_target(stand-install ...)   # macOS dev preset only; LB_STAND_DIR cache var
install(TARGETS lambdabots_mm DESTINATION addons/lambdabots/bin)
install(DIRECTORY data/ DESTINATION addons/lambdabots)
```

**Presets:** `dev-macos` (Debug, CLion Ninja), `release-macos`, `release-linux-i386` (toolchain file with `-m32` and `Rust_CARGO_TARGET`), `release-windows-x86` (VS 2022, `-A Win32`).

**Scripts:**
- `scripts/build.sh <preset>`
- `scripts/build-linux-i386-docker.sh`: builds with `docker build --platform linux/amd64`, then runs the preset in the container with named volumes for the cargo registry and target dir. Output goes to `dist/`.
- `scripts/build-windows.ps1` (vcvars32 + preset)
- `scripts/check-binary.sh <file>`
- `cargo xtask package` produces `dist/lambdabots-<ver>-<plat>.{tar.xz,zip}`.

**Docker (`docker/linux-i386.Dockerfile`)**
- `ARG BASE_IMAGE=ubuntu:18.04` (glibc 2.27). This matches BugfixedHL-Rebased's build image, so the prod host is known to accept it. Can be overridden, for example to `debian:bookworm`.
- Installs `g++-multilib`, Kitware CMake 3.30, ninja, rustup with the pinned toolchain and `i686-unknown-linux-gnu`, python3.
- Build arg `GLIBC_MAX=2.27` drives the gate.

**CI matrix (GitHub Actions):**
- `lint-test` (ubuntu): fmt, clippy `-D warnings`, test, cargo-deny, `xtask abi --check`, `xtask layering`, `lb-cli config check data/`.
- `test-i686` (Docker image): `cargo test --target i686-unknown-linux-gnu` for pure crates. This catches 32-bit issues.
- `build-linux-i386` (Docker), `build-windows-x86` (windows-2022), `build-macos-arm64` (macos-14). Each runs the exports and binary checks.
- `package` (needs all three builds). Releases on tag.

**Package layout:**

```
addons/lambdabots/
  bin/        lambdabots_mm_i386.so | lambdabots_mm.dll | lambdabots_mm.dylib
  config/     lambdabots.yaml difficulty.yaml styles.yaml weapons.yaml avatars.yaml rules/{default,bhl}.yaml
  profiles/   default.yaml
  names/      en.yaml ru.yaml          (ported from yapb lang/*_names.cfg; 144 + ~1000 names)
  maps/       <map>.yaml               (per-map config; nav architect owns nav-overlay parts)
  nav/  data/learned/  logs/           (writable, created on start)
  amxx/       lambdabots_gg.sma
  LICENSE NOTICE THIRD_PARTY_LICENSES.md README.md
```

`plugins.ini` lines:
- `linux addons/lambdabots/bin/lambdabots_mm_i386.so`
- `win32 addons/lambdabots/bin/lambdabots_mm.dll`
- `osx addons/lambdabots/bin/lambdabots_mm.dylib`

---

## 3. C++ adapter

### 3.1 Headers and files

**Which headers to vendor**

- **yapb `ext/linkage`: rejected.** It is compact and 64-bit clean, but tied to crlib (`cr::bit`, custom `string_t` class).
- **HLSDK: hlsdk-portable subset.** It is 64-bit clean and ABI-identical to the stand's `hl_arm64.dylib`. For 32-bit it matches GoldSrc.
- **ReHLDS public API headers (MIT).** Compiled only inside `rehlds_bridge.cpp`, with their own include path, to avoid `string_t` and typedef clashes with the hlsdk headers. That translation unit exposes a small internal C++ interface to the rest of the adapter. Build option `LB_WITH_REHLDS` is ON for x86 and gives a stub on macOS.
- **Metamod: own MIT `metamod_abi.h`** (plugin_info_t, META_FUNCTIONS, meta_globals_t, gamedll_funcs_t, mutil_funcs_t with 18 entries, enums, RETURN_META macros). This keeps GPLv3 metamod-r/fwgs text out of the MIT binary; the hello-rust project already did the same thing in Rust. A CI-only test compiles `static_assert`s against metamod-fwgs headers fetched at CI time; they are not distributed.

**Adapter files (`adapter/src/`)**

| File | Contents |
|---|---|
| `platform.h` | `LB_ENTRY` (= `force_align_arg_pointer` on i386 GCC/Clang), export macros, module path via dladdr / GetModuleHandleEx, monotonic clock |
| `plugin_exports.cpp` | `GiveFnptrsToDll`, `Meta_Init/Query/Attach/Detach`, META_FUNCTIONS tables. `plugin_info`: ifvers "5:13", name "LambdaBots", logtag "LB", PT_ANYTIME / PT_ANYTIME (late load supported in degraded mode) |
| `engine_state.{h,cpp}` | copy of engfuncs, gpGlobals, gpMetaGlobals, gamedll funcs, hooked tables (`GET_HOOK_TABLES`), core-call depth guard, map epoch, double sim clock |
| `hooks_dll.cpp` | DLL / NEW_DLL hooks |
| `hooks_engine.cpp` | engine hooks |
| `event_arena.{h,cpp}` | double-buffered byte arena of typed records, critical reserve, overflow counters, string intern table |
| `msg_capture.{h,cpp}` | MessageBegin/Write*/End capture, interest bitmap over 256 ids, RegUserMsg / PrecacheEvent name maps |
| `entity_registry.{h,cpp}` | Rust-supplied track rules, classification at spawn/free, edict scan (late load), entity snapshots |
| `fake_client.{h,cpp}` | slot table, create/kick, fake argv, client command execution, RunPlayerMove, fixangle emulation, CmdStart seed |
| `traces.{h,cpp}` | TraceLine/Hull/Model, PointContents, EntRef↔edict with serial validation |
| `cvars_cmds.{h,cpp}` | static cvar storage, `lb` server command, client `lb` routing, info-key reads |
| `debug_draw.{h,cpp}` | TE_BEAMPOINTS / TE_TEXTMESSAGE to one client, per-frame cap |
| `files.{h,cpp}` | `LoadFileForMe`/`FreeFile`, plugin/game/install paths |
| `rehlds_bridge.{h,cpp}` | ReHLDS API probe (CreateInterface through the engine module found by the address of `pfnPrecacheModel`); version-gated hookchains: `SV_StartSound`, `SV_EmitPings`, `IMessageManager`; `DropClient`; `GetTime`/`GetHostFrameTime`/`GetWorldmapCrc` |
| `disguise.{h,cpp}` | fake pings, authid, A2S `sendto` import hook (GOT/IAT, x86 only) |
| `host_api.cpp` | fills the `LbHostApi` table (thin trampolines) |
| `compat.{h,cpp}` | engine/metamod/gamedll facts |
| `log_sink.cpp` | early logging through `LOG_CONSOLE`/`LOG_ERROR`; C++ log ring drained by Rust |

### 3.2 Hook table

A single re-entrancy rule applies to every hook: it only records into the arena. Rust is never called from a hook.

| Hook | Phase | Action / forwarded data |
|---|---|---|
| **DLL `Spawn`** | pre (worldspawn only) | precache `sprites/laserbeam.spr` (stock HL file) for debug beams |
| DLL `Spawn` | post | classify by Rust rules → `EV_ENTITY{SPAWN, ref, classname_id, kind, origin}` |
| DLL `ClientConnect` | pre + post | `EV_CLIENT{CONNECT, slot, userid, name, addr, is_ours}`; post reads the result (`META_RESULT_ORIG_RET`) → `CONNECT_REJECTED` |
| DLL `ClientPutInServer` | post | `EV_CLIENT{PUT_IN_SERVER}` |
| DLL `ClientDisconnect` | pre | `EV_CLIENT{DISCONNECT}` (guaranteed queue); slot table cleared |
| DLL `ClientUserInfoChanged` | post | `EV_CLIENT{INFO, name, model, colors}` |
| DLL `ClientCommand` | pre | our bot inside our own command → IGNORED; argv0 `lb` from a human → `EV_CLIENT_CMD` + SUPERCEDE (access check is in Rust); `say`/`say_team` → `EV_CLIENT_CMD` (the chat's journal, `docs/chat.md`) |
| DLL `ServerActivate` | post | epoch++, edict scan, resolve message ids, → `lb_core_map_start` |
| DLL `ServerDeactivate` | pre | `lb_core_map_end`, registry and slot reset |
| DLL `StartFrame` | pre | fixangle emulation, adopt clients that skipped our connect hooks (other plugins' bots put in the game with `MDLL_*`, as jk_botti does) → `EV_CLIENT{CONNECT, PUT_IN_SERVER}`, snapshots, swap arena → `lb_core_frame_pre` |
| DLL `StartFrame` | post | `lb_core_frame_post` (AI, motor, commands, telemetry) |
| DLL `CmdStart` | pre | our bot with a pending seed → call gamedll `CmdStart(player, cmd, seed)` directly, SUPERCEDE |
| DLL `UpdateClientData` | post | disguise fake-ping path only on non-ReHLDS engines |
| DLL `Sys_Error` | pre | `lb_core_fatal` (dump trace rings and recorder) |
| NEW `OnFreeEntPrivateData` | pre | `EV_ENTITY{FREE}` |
| NEW `GameShutdown` | pre | `lb_core_shutdown(PROCESS_EXIT)` |
| Eng `MessageBegin` | pre | start capture if wanted id and (broadcast/PVS/PAS or target is our bot); messages to humans are dropped |
| Eng `Write{Byte,Char,Short,Long,Angle,Coord,String,Entity}` | pre | append a typed arg; strings are copied |
| Eng `MessageEnd` | post | finalize → `EV_USER_MSG` |
| Eng `RegUserMsg` | post | id ↔ name (`EV_REG_MSG`) |
| Eng `PrecacheEvent` | post | index → name (`EV_PRECACHE_EVENT`) |
| Eng `PlaybackEvent` | pre | `EV_PLAYBACK` (all fields + invoker origin) |
| Eng `EmitSound` / `EmitAmbientSound` | pre | `EV_SOUND{source=EMIT/AMBIENT}` (suppressed when the ReHLDS `SV_StartSound` hook is active, to avoid duplicates) |
| Eng `ClientCommand` (stuffcmd) / `ClientPrintf` | pre | SUPERCEDE for our bots |
| Eng `Cmd_Args/Argv/Argc` | pre | serve the fake argv while a bot command runs |
| Eng `GetPlayerAuthId` | pre | disguise (off by default) |
| ReHLDS `SV_StartSound` | chain | `EV_SOUND{source=REHLDS}`: all sounds incl. PM footsteps, jump and land |
| ReHLDS `IMessageManager` hooks | per wanted id | full-sender message capture; replaces the metamod path for those ids |
| ReHLDS `SV_EmitPings` | chain | disguise: callNext, then append a bots-only `svc_pings` |

Not hooked: `Touch`, `KeyValue` (static keyvalues come from the BSP entity lump; nav architect), `PM_Move`, `SetClientMaxspeed`, `LightStyle` (reserved for later).

### 3.3 Fake-client lifecycle (`create_bot` host call)

Each step below fixes a known yapb problem where noted.

1. Rust re-checks free slots and the quota just before calling.
2. `pfnCreateFakeClient(name)`. On NULL → `SERVER_FULL`.
3. **Edict hygiene.** Neither engine clears the edict (`rehlds pr_cmds.cpp:1980-2033`, `xash sv_client.c:501-548`). If `pvPrivateData` is set, call `pfnFreeEntPrivateData`. Zero `entvars`, then restore `pContainingEntity`, `netname`, `flags = FL_CLIENT|FL_FAKECLIENT`.
4. Apply info keys from the request through `SetClientKeyValue`: `model` (sets the team in teamplay), optional `*bot`, `*sid`. Done before connect, as a real client's userinfo would be.
5. `MUTIL_CallGameEntity(PLID, "player", &ent->v)`.
6. `hooked_dll->pfnClientConnect(ent, name, "127.0.0.1", reject)` with `g_creating_slot` set, so our own hook tags the event `is_ours`.
   - **On rejection:** drop immediately (ReHLDS `DropClient`, else a queued `kick #userid`), mark the slot `Zombie` until disconnect, return `REJECTED` with the reason. yapb kept a half-initialized bot here.
7. `hooked_dll->pfnClientPutInServer(ent)` → AMXX `client_putinserver` fires, so GunGame initializes the bot.
8. Ensure `FL_FAKECLIENT`. Record `{bot_gen = ++counter, userid = GetPlayerUserId}`. Return `CREATED`.
9. **Kick:** `kick_bot(slot, gen, reason)` verifies gen and userid, then uses ReHLDS `DropClient` (synchronous; the disconnect event is queued) or `kick #<userid> "<reason>"` through `ServerCommand` (never `ServerExecute`). yapb kicked by name, which broke on quotes and duplicate names.

The team is actually assigned at the bot's first `PreThink` (`m_fInitHUD` → InitHUD → `g_pGameRules->InitHUD`). So Connecting bots must receive neutral commands immediately.

### 3.4 Bot client commands

Rust tokenizes the command. `bot_client_commands(batch)` stores `argv[≤8]` in the slot's fake-argv buffer, sets `g_bot_cmd_slot`, and calls `hooked_dll->pfnClientCommand(ent)`, so AMXX sees it. `Cmd_Args` returns the line after argv0, as the engine does. The flag is cleared immediately after.

Rust keeps a whitelist: `weapon_*`, `lastinv`, `kill`, `say`, `say_team`, plus anything issued by `lb exec` (admin only, logged).

### 3.5 RunPlayerMove and the msec policy

**C++ side:** `run_player_moves(cmds[], n, feedback[])` validates slot and gen, sets the pending CmdStart seed, and calls `pfnRunPlayerMove(ent, view_angles, fwd, side, up, buttons, impulse, msec)`. It then fills `LbMoveFeedback` (origin, velocity, v_angle, flags, health, deadflag, `frame_no`). Everything else lives in Rust (`lb_host::driver`):

```
per bot per frame:  acc_ms += frame_time_ms                        (double, from ReHLDS GetHostFrameTime or gpGlobals->frametime)
emit if acc_ms >= quantum_ms   (quantum = 1000/lb_cmd_rate; lb_cmd_rate 0 = every frame when acc >= 1)
  msec = min(floor(acc_ms), cap_ms)                                 (cap default 50: no engine split)
  eff  = f(msec); f(m) = m if m<=50 else 2*f(floor(m/2))            (models SV_RunCmd split rounding, sv_user.cpp:792-801)
  acc_ms -= eff
  if acc_ms > max_debt_ms: dropped_debt += acc_ms - max_debt_ms; acc_ms = max_debt_ms   (diagnosed, telemetry)
invariant: sum(eff) <= sum(frame_time)  → never trips clockwindow; one engine call per bot per frame (multiple calls re-base svtimebase)
button latch: presses requested since last emission are OR-ed in; a requested press→release edge is never lost;
              MotorFeedback says which command actually went out (actions must not advance on unsent input, v2 §8)
fixangle: adapter clears v.fixangle (mode 1: forced = v.angles; mode 2: yaw += avelocity[1], avelocity[1]=0) in StartFrame pre
          and emits EV_FIXANGLE; the LookController adopts it before the next command
dead / connecting bots still get commands (PlayerDeathThink runs in PreThink; the InitHUD team assignment needs one)
```

The default `lb_cmd_rate=100` (a typical HLDM client rate) is provisional until M0 measures it. The alternatives (per-frame and 250) are measured at server fps 100, 500 and 1000.

### 3.6 User-message capture

A record holds `{msg_id, dest, target_slot, flags(HAS_ORIGIN|TRUNCATED|FROM_MSGMGR), origin, argc, args[{tag, value|str_off,len}], strings}`.

- The interest bitmap comes from Rust (`set_capture_mask`).
- Ids are resolved by name through `GET_USER_MSG_ID` at every map start. That also fixes late load.
- Engine svc ids are included: `SVC_TEMPENTITY` 23 (explosions), `SVC_INTERMISSION` 30.
- Our own sends (debug draw, pings) set `in_own_send` and are not captured.
- A nested `MessageBegin` marks the previous record as truncated.
- **Arena:** 256 KB by default. 25% is reserved for critical records: lifecycle, DeathMsg, anything to our bots. Non-critical records are dropped first and counted.
- Lifecycle events are also mirrored into a fixed 256-entry guaranteed queue (v2 §5.3).

### 3.7 Sound and event capture

| Channel | ReHLDS | Metamod-r on plain HLDS | Xash + Metamod-FWGS |
|---|---|---|---|
| gamedll `EmitSound`/`EmitAmbientSound` | via `SV_StartSound` chain | engine hooks | engine hooks |
| PM footsteps, jump, land, ladder | `SV_StartSound` chain | fallback: `iStepLeft` toggle (only if `mp_footsteps`) | fallback: `iStepLeft` toggle |
| weapon fire (client-predicted events) | `PlaybackEvent` hook + `PrecacheEvent` names | same | same |
| plugin-sent sounds | `SV_StartSound` chain | not visible | not visible |

The `iStepLeft` fallback works because `PM_PlayStepSound` flips `iStepLeft` even when footsteps are off, so the `mp_footsteps` check is required.

Each engine's availability is reported in the `CompatibilityProfile`. The bot's own PlaybackEvent doubles as confirmation that it fired.

### 3.8 Entity registry

Rust sends `LbTrackRule[]` (pattern, EXACT/PREFIX, kind) at init, so HLDM knowledge stays in lb-game. The adapter classifies at Spawn post, rescans edicts at ServerActivate, and emits spawn/free events.

**Kinds:**
- Item (`weapon_*`, `ammo_*`, `item_*`), WeaponBox
- Projectile (`grenade`, `rpg_rocket`, `bolt`, `hornet`, `monster_snark`, `monster_satchel`)
- Mine (`monster_tripmine`, `beam`)
- Mover (`func_door*`, `func_plat*`, `func_train`, `func_tracktrain`, `func_rotating`)
- Button (`func_button`, `func_rot_button`, `momentary_rot_button`)
- Breakable (`func_breakable`, breakable `func_pushable`)
- Charger (`func_healthcharger`, `func_recharge`)
- Hazard/teleport (`trigger_teleport`, `trigger_hurt`, `trigger_push`), Ladder, SpawnPoint, Monster

**Pull API:** `snapshot_entities(kind_mask, out, cap)` / `get_entity(ref, out)` return `LbEntitySnapshot`: origin, angles, velocity, avelocity, absmin/absmax, effects (EF_NODRAW = picked up / respawning), frame (charger empty, button state), render fields, solid, movetype, spawnflags, owner (sensor-only; Perception may only derive "is mine").

Health, takedamage, toggle state, nextthink and private data are never exported. yapb's search tested `EF_NODRAW` against `flags` (a bug); we read `effects`.

### 3.9 Traces, files, cvars, commands, debug draw, logging, physinfo

- **Traces.** `trace_line`, `trace_hull`, `trace_model`, `point_contents`, `trace_batch`. `LbEntRef` is validated by serial; a stale ignore becomes none. Results carry hit render mode/amount/fx, solid and classname id, so Rust implements the transparent pierce loop (at most 3 iterations, each counted against the budget). Flags: ignore monsters, ignore glass (0x100), missile.
- **Files.**
  - Read through the engine VFS: `load_file(path) → LbOwnedBuffer` + `free_file`. Used for BSP bytes (search paths, downloads, paks), on the main thread at map start.
  - Writes go through Rust `std::fs` under `install_dir`. That is `dirname(dirname(GET_PLUGIN_PATH))` when it ends in `addons/lambdabots/bin`, else `<gamedir>/addons/lambdabots`.
- **Cvars.**
  - Static storage in module `.data`: `static cvar_t g_cvars[256]; static char g_names[256][64], g_defaults[256][256];`. Metamod's `find_memloc` and `dladdr` need module memory; heap pointers crash dyld on arm64 (see yapb `engine.cpp:761-771`).
  - After `pfnCVarRegister`, always re-fetch with `pfnCVarGetPointer`, because metamod copies the cvar (`reg_support.cpp:58-75`).
  - Game cvars are looked up lazily by handle, since `gg_enabled` is registered late by AMXX.
  - Flags: `FCVAR_EXTDLL`, plus `FCVAR_PROTECTED` for secrets. No `FCVAR_SERVER` by default (no rules exposure, no change spam).
  - One `static cvar_t` shared by every registration is a yapb bug; use one slot per cvar.
- **Commands.** `pfnAddServerCommand("lb", LB_ENTRY handler)`. The handler calls `lb_core_server_command` directly unless a core call is active; then it queues `EV_SERVER_CMD` (for example when AMXX `server_exec` fires inside our hooked client command). Client `lb` commands are routed as events. Access uses `lb_password` + `setinfo lb_password_key` (default `_lbpw`), read with `pfnInfoKeyValue`, and is checked in Rust.
- **Debug draw.** `send_debug(slot, prims, n)` → TE_BEAMPOINTS (`MSG_ONE_UNRELIABLE`, laserbeam) and TE_TEXTMESSAGE, capped at about 30 primitives per frame per client.
- **Logging.** Before Rust init: `LOG_CONSOLE`/`LOG_ERROR`. After: a C++ log ring drained by Rust, printed with `pfnServerPrint`, rate limited.
- **physinfo.** `get_physics_key(slot, "slj")` → `pfnGetPhysicsKeyValue` (own longjump; BHL sets it in `items.cpp:344`).

### 3.10 Engine writes

**Allowed (complete list):**

| Write | Why |
|---|---|
| `entvars` reset + free leftover private data on a freshly created fake client | slot hygiene before `player()` |
| info keys before connect | what a real client's userinfo does |
| `flags \|= FL_FAKECLIENT` once | marks the bot |
| clear `fixangle` / `avelocity[1]` on our bots | network-layer acknowledgement emulation |
| CmdStart seed substitution | what a real client supplies |

**Forbidden (yapb did these; not ported):**
- `pev->v_angle`, `angles`, `button`, `impulse`, `maxspeed`, `velocity` writes
- `MDLL_Use` / `MDLL_Touch` / trigger_hurt killer entity (use the `kill` client command)
- `MDLL_ClientKill` and `frags++` (tkpunish)
- `FL_DORMANT`, `FL_WORLDBRUSH` changes
- spawn-point rendering changes (`EF_NODRAW`, CS models)
- engine calls from worker threads (yapb fake-ping)

---

## 4. C ABI (`adapter/include/lb/lb_abi.h`)

Generated by cbindgen from `crates/lb-ffi` plus the `lb_core_*` definitions in `crates/lb-plugin`. Committed; `xtask abi --check` gates it. `lb_abi_layout.inc` carries generated `static_assert(sizeof/offsetof)` checks.

```c
#define LB_ABI_VERSION 1u
typedef struct LbStr   { const char *ptr; uint32_t len; } LbStr;      /* borrowed bytes, not NUL-terminated, valid for the call */
typedef struct LbVec3  { float x, y, z; } LbVec3;
typedef struct LbEntRef{ uint16_t index; uint16_t _pad; uint32_t serial; } LbEntRef;   /* index 0 = world; serial = edict->serialnumber */
typedef struct LbArgs  { uint32_t argc; const LbStr *argv; LbStr line; } LbArgs;

typedef struct LbInitInfo { uint32_t struct_size, abi_version; uint32_t sizes[16];  /* LB_SZ_* table for the handshake */
  LbStr adapter_version, plugin_path, game_dir, install_dir; uint8_t platform, pointer_size, late_load, _pad; } LbInitInfo;
typedef struct LbInitResult { uint32_t struct_size, abi_version; int32_t status; LbStr core_version; } LbInitResult;
typedef struct LbMapInfo { uint32_t struct_size, map_epoch; LbStr map_name, bsp_path; uint32_t max_clients, max_edicts;
  uint32_t worldmap_crc; uint8_t has_crc, late_load, _pad[2]; } LbMapInfo;

typedef struct LbFrameHeader { uint32_t struct_size, map_epoch; uint64_t frame_no;
  double sim_time;      /* ReHLDS sv.time, else adapter-accumulated double (gpGlobals->time is float: ~7.8 ms resolution at 86400 s) */
  double frame_time; float engine_time; uint32_t flags; /* PRE|POST|PAUSED|FIRST */ uint64_t mono_ns; uint32_t max_clients, num_edicts; } LbFrameHeader;

typedef struct LbClientSnapshot {     /* one per slot; raw, SENSOR-ONLY; no health/armor/weapons/buttons/owner of anything */
  uint8_t slot, state /*FREE|CONNECTING|CONNECTED|SPAWNED*/, is_fake, is_ours; int32_t userid;
  LbVec3 origin, velocity, angles /*model angles*/, view_ofs, mins, maxs;
  uint32_t flags /*FL_ subset*/, effects; uint8_t movetype, solid, deadflag, waterlevel;
  uint8_t rendermode, renderfx, _p[2]; float renderamt; LbVec3 rendercolor;
  int32_t sequence, gaitsequence; float frame; uint16_t weaponmodel_id, model_id; float frags; uint16_t ping_ms, _p2; uint32_t _reserved[4];
} LbClientSnapshot;

typedef struct LbSelfSnapshot {       /* our bots only */
  uint8_t slot, deadflag, movetype, waterlevel; uint32_t bot_gen;
  float health, armor; uint32_t weapons_mask; float maxspeed, fov;
  LbVec3 origin, velocity, v_angle, punchangle, view_ofs, basevelocity;
  uint32_t flags; int32_t watertype; uint8_t in_duck, has_longjump, _p[2]; uint16_t groundentity, buttons_applied;
  float duck_time, fall_velocity; uint32_t _reserved[4]; } LbSelfSnapshot;

typedef struct LbEventBatch { const uint8_t *data; uint32_t len, count, dropped, first_seq; } LbEventBatch;
/* record = LbEventHeader{uint16 kind; uint16 size(8-aligned); uint32 seq; uint32 ctx /*FRAME|BOTCMD(slot)|BOTCLCMD(slot)|GAMEDLL*/; uint32 _p; double sim_time}
   + payload: LB_EV_CLIENT, LB_EV_USER_MSG, LB_EV_SOUND, LB_EV_PLAYBACK, LB_EV_ENTITY, LB_EV_CLIENT_CMD, LB_EV_SERVER_CMD,
   LB_EV_FIXANGLE, LB_EV_STRING (intern id -> bytes), LB_EV_REG_MSG, LB_EV_PRECACHE_EVENT, LB_EV_LOG, LB_EV_OVERFLOW */

typedef struct LbFrameInput { uint32_t struct_size, _p; LbFrameHeader header;
  const LbClientSnapshot *clients; uint32_t client_count; const LbSelfSnapshot *selves; uint32_t self_count;  /* NULL in POST */
  LbEventBatch events; } LbFrameInput;

typedef struct LbBotCommand { uint8_t slot, flags /*SET_SEED*/; uint16_t buttons; uint32_t bot_gen; LbVec3 view_angles;
  float forwardmove, sidemove, upmove; uint8_t impulse, msec, _p[2]; uint32_t random_seed; } LbBotCommand;
typedef struct LbMoveFeedback { uint8_t slot, status, _p[2]; uint64_t frame_no; LbVec3 origin, velocity, v_angle;
  uint32_t flags; float health; uint8_t deadflag, waterlevel, movetype, _p2; } LbMoveFeedback;
typedef struct LbClientCommand { uint8_t slot, argc, _p[2]; uint32_t bot_gen; LbStr argv[8]; } LbClientCommand;

typedef struct LbTraceRequest { LbVec3 start, end; uint8_t kind /*LINE|HULL|MODEL*/, hull; uint16_t flags; LbEntRef ignore, model; } LbTraceRequest;
typedef struct LbTraceResult { float fraction; LbVec3 end_pos, plane_normal; float plane_dist; LbEntRef hit; int32_t hitgroup;
  uint8_t all_solid, start_solid, in_open, in_water, hit_rendermode, hit_renderfx, hit_solid, _p; float hit_renderamt;
  uint16_t hit_classname_id, _p2; } LbTraceResult;
typedef struct LbEntitySnapshot { LbEntRef ent; uint16_t classname_id, model_id; uint8_t kind, solid, movetype, rendermode;
  LbVec3 origin, angles, velocity, avelocity, absmin, absmax; uint32_t effects, spawnflags; float frame, renderamt;
  LbVec3 rendercolor; uint8_t renderfx, deadflag, _p[2]; LbEntRef owner; } LbEntitySnapshot;
typedef struct LbTrackRule { LbStr pattern; uint8_t match, kind; uint16_t _p; } LbTrackRule;
typedef struct LbKeyValue { LbStr key, value; } LbKeyValue;
typedef struct LbCreateBotRequest { LbStr name, connect_addr; const LbKeyValue *infokeys; uint32_t infokey_count, flags; } LbCreateBotRequest;
typedef struct LbCreateBotResult { int32_t status; uint8_t slot, _p[3]; int32_t userid; uint32_t bot_gen; char reject_reason[128]; } LbCreateBotResult;
typedef struct LbDisguise { uint8_t slot, flags; uint16_t ping; uint32_t bot_gen; uint8_t loss, _p[3]; float session_seconds; char authid[40]; } LbDisguise;
typedef struct LbDebugPrim { uint8_t kind, r, g, b, width, life_ds, brightness, channel; LbVec3 a, b2; LbStr text; } LbDebugPrim;
typedef struct LbCvarSpec { LbStr name, default_value; uint32_t flags; } LbCvarSpec;
typedef struct LbOwnedBuffer { const uint8_t *ptr; uint32_t len; void *token; } LbOwnedBuffer;
typedef struct LbCompatFacts { uint8_t engine_kind /*HLDS|REHLDS|XASH*/, metamod_has_hook_tables, rehlds_major, rehlds_minor;
  int32_t rehlds_build; uint32_t channels /*SV_STARTSOUND|MSGMGR|EMITPINGS|DROPCLIENT|HOSTTIME*/;
  char engine_version[64], metamod_version[32], gamedll_desc[64], gamedll_path[256]; } LbCompatFacts;

typedef struct LbHostApi {   /* C++ -> Rust callbacks; main thread only, only during an lb_core_* call */
  uint32_t struct_size, abi_version; void *ctx;
  void     (*server_print)(void*, LbStr);           void (*client_print)(void*, uint8_t slot, uint8_t where, LbStr);
  void     (*server_command)(void*, LbStr);         /* queued, never executed immediately */
  int32_t  (*cvar_register)(void*, const LbCvarSpec*, uint32_t n, uint16_t *out);   uint16_t (*cvar_find)(void*, LbStr);
  float    (*cvar_get_float)(void*, uint16_t);      uint32_t (*cvar_get_string)(void*, uint16_t, char*, uint32_t); void (*cvar_set)(void*, uint16_t, LbStr);
  void     (*trace)(void*, const LbTraceRequest*, LbTraceResult*);  uint32_t (*trace_batch)(void*, const LbTraceRequest*, LbTraceResult*, uint32_t);
  int32_t  (*point_contents)(void*, LbVec3);
  int32_t  (*registry_set_rules)(void*, const LbTrackRule*, uint32_t); uint32_t (*snapshot_entities)(void*, uint32_t kind_mask, LbEntitySnapshot*, uint32_t cap);
  int32_t  (*get_entity)(void*, LbEntRef, LbEntitySnapshot*);
  int32_t  (*set_capture_mask)(void*, const uint8_t mask[32]); int32_t (*resolve_user_msg)(void*, LbStr name, int32_t *size);
  int32_t  (*create_bot)(void*, const LbCreateBotRequest*, LbCreateBotResult*);  int32_t (*kick_bot)(void*, uint8_t, uint32_t gen, LbStr reason);
  int32_t  (*bot_client_commands)(void*, const LbClientCommand*, uint32_t);
  int32_t  (*run_player_moves)(void*, const LbBotCommand*, uint32_t, LbMoveFeedback*);
  uint32_t (*get_physics_key)(void*, uint8_t slot, LbStr key, char*, uint32_t);   uint32_t (*get_client_info_key)(void*, uint8_t, LbStr, char*, uint32_t);
  int32_t  (*get_weapon_data)(void*, uint8_t slot, void *weapon_data_out, uint32_t cap);   /* M2, optional */
  int32_t  (*set_bot_disguise)(void*, const LbDisguise*);   int32_t (*get_player_stats)(void*, uint8_t, int32_t*, int32_t*);
  int32_t  (*load_file)(void*, LbStr path, LbOwnedBuffer*);  void (*free_file)(void*, LbOwnedBuffer*);
  int32_t  (*send_debug)(void*, uint8_t slot, const LbDebugPrim*, uint32_t);   int32_t (*get_compat_facts)(void*, LbCompatFacts*);
} LbHostApi;

/* Rust -> C++ (exported by lb-plugin, statically linked) */
int32_t lb_core_init(const LbHostApi*, const LbInitInfo*, LbInitResult*);
void lb_core_shutdown(uint32_t reason);          void lb_core_map_start(const LbMapInfo*);   void lb_core_map_end(uint32_t epoch);
void lb_core_frame_pre(const LbFrameInput*);     void lb_core_frame_post(const LbFrameInput*);
void lb_core_server_command(const LbArgs*);      void lb_core_fatal(LbStr);
```

**Rules**

1. **Threads.** All calls happen on the engine main thread. Host functions are valid only inside an `lb_core_*` call. Rust enforces this with `HostCtx<'call>`, which is `!Send` and lifetime-bound, so worker threads cannot hold it.
2. **No re-entrancy.** While a core call is active, the adapter never calls the core again; it queues instead.
3. **Lifetimes.** Every pointer passed in either direction is borrowed for the call. The receiver copies what it keeps. The adapter copies strings into static per-slot buffers.
4. **Ownership.** Allocators never cross the boundary. `LbOwnedBuffer` is released with `free_file`.
5. **Versioning.** Handshake on `abi_version` plus the `sizes[]` table. `struct_size` everywhere. Only append within an ABI major.
6. **Types.** Fixed-width integers only. Enums are integer constants. Explicit padding. No `bool`.
7. **Errors and panics.** Host functions return `int32` status codes and never abort. Panics never cross the boundary.
8. **Strings are bytes** (player names may be CP1251 or UTF-8). They are decoded lossily for logs only.
9. **Synchronous traces.** `HostCtx::trace()` calls through the table directly (about ns overhead). Sensors use `trace_batch`. Budgets are enforced in Rust (`TraceService`); the adapter counts traces per frame for stats.
10. **Why a table instead of direct symbols:** `MockHost`, `RecordingHost` and `ReplayHost` implement the same `Host` trait without linking C++.

---

## 5. Rust runtime (lb-runtime, lb-plugin)

**Frame pipeline (v2 §5.1)**

| Entry | Phase | Work |
|---|---|---|
| `frame_pre` | 1 | Decode the arena. Lifecycle first (guaranteed queue), then messages (HldmAdapter), sounds, events. Ingest snapshots. |
| `frame_pre` | 2 | Handle and lifecycle checks. Apply worker results only if epoch, gen and data versions match. Apply staged config. Quota reconciler; bot creation and kicks (host calls). |
| `frame_post` | 3 | Due sensor ticks (pull entity snapshots, budgeted traces) → observations → beliefs. |
| `frame_post` | 4 | Urgent transitions; due utility evaluations. |
| `frame_post` | 5 | Advance budgeted queries (path slices, link validation traces). Actions and tactics emit intents. |
| `frame_post` | 6 | Arbitration → motor → CommandEncoder → EngineDriver: one `bot_client_commands` batch, then one `run_player_moves` batch. |
| `frame_post` | 7 | Telemetry (bounded), log flush, debug draw, deferred side effects. |

All bots finish phases 3–5 before any bot moves, so traces see a consistent world. Events captured during our own commands arrive at the next `frame_pre`, tagged with `ctx` and `frame_no`.

**Scheduler.**
- Per-bot task periods with a phase offset `hash(bot_uid) mod period` (staggered). Defaults, to be tuned: perception 15 Hz, utility 8 Hz, team coordination 2 Hz, quota 4 Hz. Motor, encoder and button latch run every frame.
- Ordering is an EDF queue on due time. Priority = base + lateness / period.
- A floor guarantee means no bot's perception falls below period ×2 (no starvation of far-away bots).

**Budgets are simulation-time token buckets, refilled by `frame_time`, with a per-frame burst cap.** Example: traces 4000/s, burst 64/frame; A* expansions 200k/s, burst 2k; cost units per second. A* stands for any budgeted work.
- These are counted in deterministic work units, so replay stays exact.
- Real time (`Instant`) is only measured. A 1 Hz overload governor adjusts bucket rates by ±10%; its decisions are written to the trace and the recorder.
- Overload order (v2): defer nav generation first, then optional evaluations, then far bots' utility. Active-action control is always kept.

**Handles.**
- `MapEpoch(u32)` is bumped at `map_start`.
- `ClientRef{epoch, slot, userid}`, `EntityRef{epoch, index, serial}`, `BotId{slot, gen}`.
- A `HandleTable::resolve()` rejects stale handles. Every deferred result carries `JobTag{epoch, bot, versions}`; stale results are dropped and counted (v2 invariant 7).

**Workers.**
- Default 1 thread (`lb_workers`, −1 = auto, capped at cores−1), at lowered priority: `setpriority` on Linux, `THREAD_PRIORITY_BELOW_NORMAL` on Windows, QoS utility on macOS.
- Bounded crossbeam queues. `trait Job: Send { type Output; fn run(self, &CancelToken) -> Self::Output }`.
- `pool.drain_completed(|tag, res| ...)` runs on the main thread.
- Cancellation per epoch. Join with a timeout at shutdown.
- Jobs: BSP parse, nav generation (nav architect), config parse/validate, file writes, telemetry encode (optional).

**Panic policy.**
- `panic = "unwind"` in every profile.
- The panic hook records location and a forced backtrace to the log file, telemetry and the trace dump.
- `ffi_guard(default, |rt| ...)` wraps every export: `RUNTIME.try_lock()` (re-entrancy → error), then `catch_unwind`.
- A panic outside per-bot scope puts the runtime in **SafeMode**: AI off, bots kicked, quota 0, plugin stays loaded, `lb status` shows the fault, and a re-init is attempted at the next map.
- **Per-bot isolation:** each bot's phase 3–6 work runs in `catch_unwind`. On panic the bot becomes `Faulted`, gets a neutral command, and is kicked at the next safe point. Fault counters are kept per profile and map. Shared services must commit only after success.
- The Windows main-thread stack may be about 1 MB, so large locals go on the heap (clippy `large_stack_frames`).

**Logging.** `tracing` layers:
- file: daily rotation, 7 files, 50 MB cap, non-blocking
- console: warn+ by default; lines queued and printed by the main thread at frame end, at most 20 per second
- telemetry: warn+

C++ logs arrive as `EV_LOG`.

**RNG.** A master seed comes from config (0 = time-based) and is logged in `hello` and in traces. Each bot gets `perception`, `decision`, `motor` and `cosmetic` Pcg64 streams, seeded by `SplitMix64(master ^ hash(bot_uid, stream))`. The CmdStart seed is drawn from the motor stream, which keeps replay deterministic.

**Config snapshots.** `ArcSwap<ConfigSnapshot{version, hash, provenance}>` is swapped only at the phase-2 boundary. Uninterruptible actions pin an `Arc`.

**Recorder / replay (M2).** `RecordingHost` logs every FFI input and host-call result into `.lbrec`: an in-memory ring of about 10 s dumped on fatal, or `lb record <sec>`. `lb-cli replay` runs lb-runtime against `ReplayHost` and reports divergence (v2 §12).

---

## 6. Bot manager

**States:** `Requested` → `Connecting` → `Spawned` → `Alive` ⇄ `Dead` → `Respawning` → `Spawned`. `Leaving` and `Faulted` can be entered from any state. Disconnect or map end → `Gone`.

| State | Entered when | Next |
|---|---|---|
| Requested | quota or command, with an identity | `not_before` elapsed → creation. Failure → back-off (exponential; give up after 5 rejections) |
| Connecting | `create_bot` OK | receives neutral commands; `ResetHUD`/`InitHUD` seen → Spawned. Timeout 5 s → kick |
| Spawned | ResetHUD, `deadflag==NO`, health > 0 | SelfState minimum known (current weapon), or 0.5 s → Alive |
| Dead | DeathMsg victim or `deadflag != NO` | release all buttons (clear stale duck). `deadflag == RESPAWNABLE` → Respawning |
| Respawning | human-like delay (config, e.g. 0.3–1.2 s) elapsed | one command with no buttons, then one with `IN_JUMP` (BHL needs a press *edge*: `m_afButtonPressed`, `player.cpp:1464`). No ResetHUD within 2 s → retry |
| Leaving | kick, rotation, SafeMode | ClientDisconnect → Gone |

**Quota.**

```rust
pub trait QuotaPolicy: Send { fn name(&self)->&str; fn desired(&self, cx:&QuotaContext)->QuotaDecision; }
pub struct QuotaContext<'a>{ max_clients:u8, humans_playing:u8, humans_spectating:u8, humans_connecting:u8,
  bots_active:u8, bots_pending:u8, rules:&'a PublicRules, map:&'a MapInfo, now:SimTime, cfg:&'a QuotaConfig, scenario:Option<&'a MapScenario> }
pub struct QuotaDecision{ target:u8, reason:&'static str, team_hint:Option<TeamHint>, max_joins_per_window:u8 }
```

- **Base policies:** `Normal(N)`, `Fill(N)` (default: N total players), `Match(ratio)`.
- **Decorators, applied in order:** `JoinAfterPlayer`, `AutoVacate(keep_slots, count_connecting)`, clamp to `max_clients − humans − reserve`, then the future `MapScenarioPolicy`.
- **Reconciler at 4 Hz, with hysteresis:**
  - add: one creation per 0.25 s, joins staggered randomly;
  - remove: cancel pending first, then dead bots, then lowest score, keeping teams balanced.

**Rotation.** Per-bot `stay_until ∈ [min, max]`. On expiry the bot leaves at a safe point (dead or idle). Its name is not saved; a new identity joins after a rejoin delay.

**Saved names.** When the engine drops fake clients at changelevel (`rehlds sv_main.cpp:7800-7808`), or when the map ends, identities go into `carry_over`: name, profile, style, model, team, and a session start time for the A2S disguise. They rejoin first on the next map, each with a "loading" delay.

**Names.** `names/{en,ru}.yaml`. Pick names unused by any player. Optional `lb_name_prefix`. Sanitize: ≤ 31 bytes, strip `" ; %` and control characters.

**Profiles** (YAML, replacing `bots.json`):

```yaml
schema: lambdabots/profiles@1
profiles:
  - { name: Gordon, difficulty: expert, style: rusher, model: gordon, avatar: "7656119...", weight: 1.0, overrides: { aggression: 0.9, fear: 0.1 } }
```

**Models and teams.**
- FFA: model from the profile, else the `models` list. `zombie` is allowed; yapb's "creature" logic is not ported.
- Teamplay: a team policy picks the smaller team (TeamInfo counts), or a configured team. Info key `model` = team model from `mp_teamlist` (and `mp_teamoverride`), set before connect.

**Kick.** By `(slot, gen, userid)` only.

**Spawn-protection self-knowledge.** The rules profile declares `spawn_protection {enabled, duration_s, ends_on_attack, signature {renderfx: glow_shell, color, amt}}`. SelfState then carries `protected_until` (an estimate), confirmed from the bot's own render fields. Other players' protection is only a render-signature observation in Perception.

---

## 7. HldmAdapter (lb-game)

**Decoders.** Typed tags are validated. A mismatch is counted and the message ignored, which tolerates BHL/AG variants.

| Message | Payload | Output | Fix vs yapb |
|---|---|---|---|
| WeaponList | str, b ammo1, b max1, b ammo2, b max2, b slot, b pos, b id, b flags | WeaponRegistry (per map) | — |
| CurWeapon | b state (≠0 active; 0x40 on-target), b id, b clip (−1 = no clip) | current weapon, clip; confirms weapon switch (pending, 1 s timeout → reason) | fire detected from own PlaybackEvent, not from clip decrease |
| AmmoX | b idx, b total | `ammo[idx]` | — |
| AmmoPickup | b idx, b amount | `AmmoPickedUp{delta}` event | yapb stored the delta as a total |
| WeapPickup / ItemPickup | b id / str | pickup events (longjump, battery, …) | new |
| Health / Battery | b / s | HUD view of health and armor (cross-checked with pev) | new |
| Damage | b armor, b dmg, l bits, 3× coord | `DamageTaken{amount, armor, bits, source_pos}` | never exposes `dmg_inflictor` |
| DeathMsg | b killer, b victim, str weapon | public `KillEvent` | — |
| ScoreInfo | b idx, s frags, s deaths, s class, s team | Scoreboard | plus `pev->frags` from snapshots (covers AMXX/GunGame writes) |
| TeamInfo / GameMode | b idx, str / b | Teams / `teamplay_reported` | per-map team cache |
| ResetHUD / InitHUD | — | lifecycle markers | — |
| SetFOV / ScreenFade | — | zoom / blinded | — |
| TextMsg / SayText / HudText | — | decoded, unused: players' chat comes from `ClientCommand` (lines AMXX sends are not seen) | — |
| SVC_TEMPENTITY (TE_EXPLOSION…), SVC_INTERMISSION | — | raw visual/audio stimuli; match end (the chat's gg) | new |

**SelfState.** Health, armor, `weapons_mask` (from pev), clip and ammo per weapon and type, current weapon (confirmed vs requested), longjump (physinfo `slj`), zoom, on-ground/duck/water/ladder, deadflag, spawn time, damage events, own score, protection estimate, and prediction data (M2). Fields start as `Known<T>::Unknown` and are never assumed zero (v2 §4.1).

**PublicRules.** Read at map start and polled at 1 Hz:
- `mp_teamplay`, `mp_teamlist`, `mp_teamoverride`, `mp_defaultteam`, `mp_friendlyfire`, `mp_weaponstay`, `mp_forcerespawn`, `mp_footsteps`, `mp_falldamage`, `mp_flashlight`, `mp_fraglimit`, `mp_timelimit`
- movevars: `sv_maxspeed`, `sv_gravity`, `sv_accelerate`, `sv_airaccelerate`, `sv_friction`, `sv_stopspeed`, `sv_stepsize`, `edgefriction`, `sv_wateraccelerate`
- BHL: `mp_bunnyhop` (1 = no cap; `gamerules.cpp:45`), `mp_selfgauss`, `mp_respawn_fix`, `mp_dmg_*` (weapon damage table)
- `mp_bhopcap` if present (the user's name for the setting; this BHL version uses `mp_bunnyhop`)
- item respawn times from YAML (HL defaults: weapons 20 s, ammo 20 s, items 30 s)

**Physics profile hash** (a nav cache key component): movevars + bhop cap + use-slowdown type + gravity + stepsize + maxspeed.

**Item respawn learner.** Uses the registry's EF_NODRAW transitions to estimate *rules* per class (median delay). Stored in `data/learned/<rules_profile>/item_respawn.yaml`. Decision code only gets the rule; per-instance timers come only from the bot's own observations.

**GameMode detection.**
- Teamplay: the cvar, or the GameMode message, or `lb_game_mode`.
- GunGame: bridge `hello` (authoritative), or `rules.gungame.detect_cvar` (default `gg_enabled`) > 0, or `lb_gungame 1`.
- `GunGameState`: levels, per-player level, leader, warmup, own weapon set.

**CompatibilityProfile.** Engine and version (ReHLDS API major.minor and build, Xash `host_ver`, `sv_version`), metamod flavor and version, gamedll description and path, BHL detection (`mp_respawn_fix` present), ABI/build, capture channels per category, resolved message ids, known event names, fps (`sys_ticrate`, measured), `clockwindow`, `host_framerate` warning, plugins detected (`amxmodx_version`, `gungame`, `gg_enabled`), BSP load, nav status. Printed at map start, available via `lb compat`, and written to `logs/compat-<map>.yaml`.

---

## 8. Configuration

**Files**
- `config/lambdabots.yaml` (server: quota, telemetry, disguise, logging, budgets, paths)
- `config/difficulty.yaml` (port of `difficulty.cfg`; field mapping by the AI architect)
- `config/styles.yaml`, `config/weapons.yaml` (weapon policies per style)
- `profiles/*.yaml`
- `config/rules/<profile>.yaml` (item respawns, spawn protection, GunGame detection, custom plugin semantics)
- `maps/<map>.yaml` (per-map quota and rules overrides; nav overlays by the nav architect)
- `names/*.yaml`, `config/avatars.yaml`

**Schema.** Every file starts with `schema: lambdabots/<kind>@<major>`. Minor additions come with serde defaults. `deny_unknown_fields` catches typos. `lb-cli config migrate` handles major bumps. JSON Schemas are exported to `docs/schemas/` for editor completion.

**Validation.** Unit newtypes (`Seconds`, `Units`, `Percent`), ranges, cross-field checks (min ≤ max), references (profile → difficulty id). Errors show a YAML snippet. The main config is loaded synchronously in `lb_core_init`; everything else is parsed and validated on a worker.

**Provenance.** Each value carries `Value<T>{v, source: Default | File{path, line} | Cvar | Command}`. `lb config why <key>` prints it.

**Precedence:** built-in < main YAML < rules profile < map YAML (schema-limited: no physics profile, no honesty switches) < runtime cvar or command.
- Cvar defaults are the YAML values at registration.
- A runtime change (value differs from the last value we applied) is kept across maps and reloads, unless `lb config reload --force`.
- This replaces yapb's `ignore_cvars_on_changelevel`.

**Hot reload.** `lb config reload [all|rules|profiles|map]`, or opt-in mtime polling at 1 Hz. Read → validate on a worker → build snapshot → swap at phase 2. On error the old version stays in effect. A physics change invalidates the affected nav links (nav architect).

---

## 9. Commands and cvars (parity)

**Commands** (`lb <sub>`; yapb aliases kept):

| Command | Notes |
|---|---|
| `add [difficulty] [style] [team] [model] [name]` | aliases: `addbot` |
| `addp <profile\|*>` | from the roster |
| `kick [#userid\|name\|team]`, `kickall [instant] [team]` | — |
| `kill [#userid\|team]` | uses the `kill` client command |
| `fill [team] [count] [difficulty] [style]` | — |
| `list`, `version`, `compat`, `status` | status: budgets, p95, queues, faults |
| `config show\|why\|reload\|save` | replaces yapb `cvars` |
| `exec #userid <cmd>` | admin only |
| `weapons melee\|standard` | — |
| `trace <bot> on\|off\|dump`, `debug <bot\|off>`, `record <sec>` | — |
| `gg ...` | GunGame bridge, server source only |
| `nav ...` | nav architect |
| `test motor <bot> <script>`, `debug panic <bot>` | require `lb_dev 1` |
| `help [cmd]` | — |

Dropped: menus (ShowMenu), `vote`/`votemap`, graph download/upload.

**Platform cvars** (yapb source → default):

| lb cvar | from yapb | default |
|---|---|---|
| `lb_quota` | quota | 8 |
| `lb_quota_mode` | quota_mode | fill |
| `lb_quota_match` | quota_match | 0 |
| `lb_autovacate`, `lb_autovacate_keep_slots` | autovacate, autovacate_keep_slots | 1, 1 |
| `lb_autovacate_count_connecting` | kick_after_player_connect | 1 |
| `lb_join_after_player` | join_after_player | 0 |
| `lb_join_delay` | join_delay | 5 |
| `lb_join_interval_min/max` | new | 0.5 / 3 |
| `lb_join_team` | join_team | any |
| `lb_models` | botskin | "" |
| `lb_name_prefix` | name_prefix | "" |
| `lb_save_names` | save_bots_names | 1 |
| `lb_rotate`, `lb_rotate_stay_min/max` | rotate_bots, rotate_stay_min/max | 0, 360 / 3600 |
| `lb_language` | language | en |
| `lb_difficulty` | difficulty | normal (0–4 aliases accepted) |
| `lb_difficulty_min/max` | difficulty_min/max | −1 |
| `lb_difficulty_auto` (+ interval) | difficulty_auto | 0 |
| `lb_style` | preferred_personality | random |
| `lb_profiles` | bots_roster | 1 |
| `lb_game_mode` | game_mode | −1 |
| `lb_gungame` | force_gungame | auto |
| `lb_cmd_rate` | replaces think_fps | 100, decided in M0 |
| `lb_workers` | threadpool_workers | −1 |
| `lb_force_respawn` | force_respawn | 1 |
| `lb_respawn_delay_min/max` | new | 0.3 / 1.2 |
| `lb_autokill_delay` | autokill_delay | 0 |
| `lb_freeze` | freeze_bots | 0 |
| `lb_debug` | debug | 0 |
| `lb_telemetry` / `_port` / `_hz` / `_dest` / `_cmd_bind` | telemetry* | 0 / 27070 / 10 / 127.0.0.1 / 127.0.0.1 |
| `lb_telemetry_secret` | new, protected | "" |
| `lb_trace` | new | 1 |
| `lb_password`, `lb_password_key` | password, password_key | "", `_lbpw` |
| `lb_version` (read-only), `lb_log_level`, `lb_bot_seed`, `lb_dev` | new | —, info, 1, 0 |
| `lb_fakeping` (+ `_min` 5 / `_max` 20 / `_interval` 1.25 / `_follow_humans` 1) | show_latency 2, ping_* | 0 |
| `lb_scoreboard_botflag` | show_latency 1 | 0 |
| `lb_avatars` | show_avatars | 0 |
| `lb_fake_steamid` | enable_fake_steamids | 0 |
| `lb_hide_bots_in_queries` | enable_query_hook | 0 |

**AI parity cvars** (AI architect owns semantics): jasonmode, avoid_grenades, use_chargers (+ health/armor thresholds), pickup_best, camping_allowed (+ min/max), use_longjump, gauss_precharge, use_gauss_jump, use_satchel_jump, spraypaints, restricted_weapons, breakable_health_limit, destroy_breakables_around, object_pickup_radius, user_follow_percent / max_followers, gungame_leader_priority, ignore_enemies, random_knife_attacks, stab_close_enemies, shoots_thru_walls (only as honest suppressive fire), grenadier_mode, ignore_enemies_after_spawn_time. Nav cvars: nav architect.

**Dropped yapb cvars:** tkpunish, chat*, whose_your_daddy, graph_url*, bind_menu_key, display_menu_text, think_fps_disable, ignore_cvars_on_changelevel, check_darkness (later), attack_monsters.

---

## 10. Telemetry and observer v2

**Transport.** UDP JSON datagrams to `lb_telemetry_dest` (default 127.0.0.1:27070). Large payloads are chunked (`part`/`parts`, reassembled by the bridge); packets stay ≤ 1200 B for non-loopback destinations. Recordings are NDJSON.

**Envelope:** `{"v":2,"t":<type>,"sid":<session>,"seq":n,"ep":<map_epoch>,"ts":<sim_time>,...}`

**Message types:**
- `hello` (every 5 s): build, ABI, compat summary, capabilities, seeds, map, and a command nonce base
- `frame` (`lb_telemetry_hz`): compact players and bot summaries (state, action, goal, hp for bots only)
- `bot` (subscribed bots, up to 20 Hz): observations with confidence and uncertainty, beliefs, utility top-k with factor breakdown, action and phase, channel owners, intents, the command sent (msec, buttons), path progress
- `trace`: decision-trace records, per-bot rate cap (default 50/s)
- `path`, `graph_begin|nodes|links|end`, `overlay` (payload defined by the nav architect)
- `event`: kill, spawn, lifecycle, pickup, config reload, errors
- `perf` (1 Hz): p50/p95/p99 per phase, traces per frame, expansions, queue ages, dropped events, msec histogram, debt dropped, stale results
- `log`: warn and above

**Rate limits.** A token bucket per type plus a global `telemetry.max_kbps` (1024). Priority order: hello, event, trace, frame, bot, graph. Drop counters go into `perf`.

**Command channel.** UDP `port+1`, bound to `lb_telemetry_cmd_bind` (loopback by default). Message:

```
{"v":2,"cmd":"lb","args":"add","sid":..,"nonce":u64,"ts_ms":..,"mac":hex}
```

- `mac = HMAC-SHA256(secret, "lbcmd1\n"+sid+"\n"+nonce+"\n"+ts+"\n"+payload)`.
- Nonces are strictly increasing per session; timestamps must be within ±30 s.
- Failures are rate-limited, with a temporary ban per source address.
- **No secret configured → the command channel is disabled.** Loopback without auth requires the explicit `allow_unauthenticated_loopback: true`.
- Whitelist: `lb *` and `sub`. Arbitrary console commands require `allow_console: true`.
- `lb` output returns as `cmd_result`.

yapb's command port had no authentication.

**Observer port** from yapb-halflife `tools/observer`, which is MIT:
- `bridge.py`: v2 envelope, reassembly, HMAC signing (secret from environment or file), subscription relay, page access token, recordings index.
- `index.html`: stays one file, no build step. New panels: bot inspector (perception, beliefs, utility table), decision-trace timeline, channel owners, nav layers and validation status, perf graphs, compat panel.
- `fake_server.py`: v2 generator.
- `bsp2json.py`: kept (later to be replaced by `lb-cli bsp export`).
- `soak_assert.py`: v2 invariants, see §12.
- `protocol.md` and `schemas/*.json`.

---

## 11. Disguise features and GunGame bridge

**Disguise.** Every feature is off by default and has its own toggle. A warning banner is printed when any is on. The data comes from Rust at 1 Hz through `set_bot_disguise`, on the main thread only (fixing yapb's worker-thread engine call).

- **Fake ping.**
  - ReHLDS: the `SV_EmitPings` chain calls `callNext`, then appends a bots-only `svc_pings` (1 / 5 / 12 / 7 bits, through `RehldsFuncs` `MSG_*Bits`).
  - Other engines: yapb's method. In `UpdateClientData` post, when a human holds `IN_SCORE`, send an unreliable `SVC_PINGS` (id 17), throttled to 0.5 s per human.
  - Values: average of human pings (`GetPlayerStats`), plus a per-bot base and jitter.
- **`*bot` / `*sid` info keys** before connect (avatar from the profile or `avatars.yaml`).
- **Fake SteamID.** `GetPlayerAuthId` pre-hook for our slots returns a stable `STEAM_0:1:<hash(name)>` from a static per-slot buffer. `is_user_bot` in AMXX still sees `FL_FAKECLIENT`.
- **A2S hide.** Rewrites A2S_PLAYER durations from the persistent session model (survives map changes via carry-over), and sets the bot count to 0 in `I` and `m` replies.
  - x86 only. Implemented as an import-table patch of the *engine module's* `sendto`: ELF GOT of `engine_i486.so`, or PE IAT of `swds.dll` matched by address, since the import may be by ordinal. This avoids inline patching of the libc function, which is what yapb's detour did and which affects every caller.
  - Restored on detach. Auto-disabled on any mismatch.
  - Steam's master-server count is out of scope (documented limitation).

**GunGame bridge** (`data/amxx/lambdabots_gg.sma`; server commands only):

```pawn
#include <amxmodx>
#include <gungame>
#define LBGG_PROTO 1
public plugin_natives() set_native_filter("nf")
public nf(const n[], i, trap) return trap ? PLUGIN_CONTINUE : PLUGIN_HANDLED
public plugin_init() { register_plugin("LambdaBots GG Bridge","1.0","lambdabots"); register_srvcmd("lb_gg_sync","full_sync"); }
public plugin_cfg() set_task(2.0, "full_sync")
public full_sync() {
  server_cmd("lb gg hello %d", LBGG_PROTO); server_cmd("lb gg state %d", get_cvar_num("gg_enabled"))
  new maxl = gg_get_max_level(), ws[weaponSetStruct], eq[equipStruct], list[256]
  server_cmd("lb gg levels %d", maxl)
  for (new l = 0; l < maxl; l++) { if (!gg_get_level_data(l, ws)) continue; list[0] = 0
    for (new i = 0, n = ArraySize(ws[WSET_EQUIP_ITEMS]); i < n; i++) { ArrayGetArray(ws[WSET_EQUIP_ITEMS], i, eq); add(list, charsmax(list), eq[EQUIP_NAME]); add(list, charsmax(list), ",") }
    server_cmd("lb gg level %d %d %d ^"%s^" ^"%s^"", l, ws[WSET_KILLS], ws[WSET_BOTCANT], ws[WSET_SHOWNAME], list) }
  new p[32], n; get_players(p, n); for (new i = 0; i < n; i++) send_player(p[i]); send_leader(); return PLUGIN_HANDLED }
send_player(id) { new pd[playersDataStruct]; new lvl = gg_get_player_level(id, pd); if (lvl >= 0)
  server_cmd("lb gg player #%d %d %d %d", get_user_userid(id), lvl, pd[PLAYER_KILLS], pd[PLAYER_NEEDKILLS]) }
send_leader() { new l = gg_get_leader_id(); server_cmd("lb gg leader #%d", l > 0 ? get_user_userid(l) : 0) }
public gg_state(a) server_cmd("lb gg state %d", a)
public gg_levelup(id, nl, ol) send_player(id)
public gg_update_players_ranks() send_leader()
public gg_warmup_start() server_cmd("lb gg warmup start %.1f", get_cvar_float("gg_warmup"))  // forward exists in gungame.sma:908
public gg_warmup_end() server_cmd("lb gg warmup end")
public gg_win(id) server_cmd("lb gg win #%d", get_user_userid(id))
public client_putinserver(id) set_task(0.5, "tsp", id)
public tsp(id) if (is_user_connected(id)) send_player(id)
```

**Protocol** (Rust parser, versioned, unknown verbs logged):

```
hello <proto>
state 0|1
levels <n>
level <idx> <kills> <botskip> "<name>" "<classnames,>"
player #<uid> <lvl> <kills> <need>
leader #<uid|0>
warmup start <s>|end
win #<uid>
```

If GunGame is detected but no `hello` arrives, Rust sends `lb_gg_sync` at map start.

**Fallback without the bridge:**
- Detection by the `gg_enabled` cvar.
- Own level from the spawn inventory.
- Other players' levels from `pev->frags/100` in snapshots. gungame writes frags = level × 100 through AMXX (`gungame.sma:2763,2777,3943`), and that `ScoreInfo` is invisible to metamod hooks.
- Leader = maximum frags.
- Optional parse of `configs/gungame/<map>.ini` for the level table (public rules).

---

## 12. Stands and verification

**macOS Xash stand** (`/Users/nikita/Git/half-life/xash3d-fwgs-apple-arm64`, which already has `crossfire.bsp` and `hl_arm64.dylib`):
- `scripts/stand/macos-install.sh [--stand DIR] [--preset dev-macos] [--link-config] [--disable-yapb]`
  - builds, copies the dylib to `valve/addons/lambdabots/bin/`, syncs `data/` without overwriting local edits;
  - idempotently adds `osx addons/lambdabots/bin/lambdabots_mm.dylib` to `valve/addons/metamod/plugins.ini`, and can comment out the yapb and hellorust lines.
- `scripts/stand/macos-run.sh --map crossfire --bots 8 --fps 1000 --duration 300 [--soak] [--changelevel crossfire,stalkyard,boot_camp]`
  - runs `./xash3d -dedicated -game valve -port 27015 +maxplayers 16 +sys_ticrate $FPS +map $MAP +lb_quota $BOTS +lb_telemetry 1` and logs to `stand-runs/<ts>/console.log`;
  - starts `soak_assert.py`;
  - sends signed `changelevel` commands;
  - collects `addons/lambdabots/logs`, recordings and the compat report.
- `soak_assert.py` v2 invariants:
  - process alive, no `Host_Error`
  - bots reach quota within 20 s; kills ≥ K
  - no alive bot without progress for T seconds (uses action progress, not only displacement)
  - stale results applied == 0; lifecycle errors == 0
  - p99 AI frame ≤ budget; debt dropped ≤ limit
  - respawn latency within the configured window
  - identities survive changelevel
- Two stand limits:
  - AMXX/GunGame cannot run on arm64 macOS, so the bridge is exercised by typing `lb gg ...` commands or with an `exec` script.
  - Unloading a Rust dylib is effectively a no-op on macOS (thread-local destructors), so development iteration means restarting the server.

**ReHLDS Linux i386 VM** (the user provisions the VM; no SSH orchestration):
- `scripts/rehlds-vm/provision.sh`: idempotent; Debian 12 or Ubuntu 22.04/24.04 amd64; all versions and checksums pinned at the top.
  1. `dpkg --add-architecture i386`, then install `lib32gcc-s1 lib32stdc++6 libc6-i386 curl unzip xz-utils python3 tmux`.
  2. Create an `hlds` user; steamcmd `app_update 90` with `mod valve` (retry loop).
  3. Overlay the ReHLDS release binaries.
  4. Metamod-r → `addons/metamod/metamod_i386.so`; update `liblist.gam`.
  5. BugfixedHL-Rebased server release.
  6. AMXX 1.10, hl-gungame, `lambdabots_gg`.
  7. lambdabots from `dist/*.tar.xz`; add the `plugins.ini` line.
  8. Test `server.cfg` (`sv_lan 1`, `maxplayers 24`, `sys_ticrate 1000`).
  9. systemd unit `hlds.service` (`hlds_run -game valve +map crossfire -pingboost 3 +sys_ticrate 1000`).
  10. Checks: `meta list`, `lb compat`, `amxx plugins`.
- Walkthrough in `docs/stands/rehlds-vm.md`.

**Windows.** CI-built DLL. `docs/stands/windows-smoke.md`: HLDS or ReHLDS with metamod-r; `dumpbin /exports`; `meta list`; add, kick, changelevel and kill checks.

**M0 acceptance**

1. **Builds.** Three binaries with exactly 5 exports. The Linux `.so` has `GLIBC ≤ 2.27` and no `GLIBCXX`.
2. **Load.** RUN under Metamod-r/ReHLDS and Metamod-FWGS/Xash. The ABI handshake passes. The CompatibilityProfile is correct.
3. **AMXX sees bots.** `lb add ×N` → a test `.sma` logs `client_connect`, `client_putinserver` with `is_user_bot=1`, and `client_disconnected`. GunGame gives bots the level-1 weapon; `gg_get_player_level(bot) == 1`.
4. **SelfState.** Health 100; weapons mask has crowbar and glock; CurWeapon confirms; AmmoX totals correct; 14 WeaponList entries.
5. **Death and respawn.** `lb kill` → DeathMsg decoded → Dead → Respawning → Alive within the configured window. 100 cycles, zero stuck-dead.
6. **Slot reuse.** `lb kick #uid` → disconnect; `lb add` reuses the slot with a new gen; a stale-gen command is rejected (`lb debug stale-cmd`).
7. **changelevel ×20.** Bots dropped and rejoin with saved names; epoch increments; zero old-epoch applications; old worker jobs discarded.
8. **Full server.** At 24 slots, a joining human gets a slot (autovacate counts connecting clients); fill holds N.
9. **Golden decoder tests.** Raw message streams recorded on each engine/gamedll pair become `insta` fixtures.
10. **msec semantics.** At `sys_ticrate` 100/500/1000 with `cmd_rate` {frame, 250, 100}, `lb test motor`:
    - run speed within ±1% of `sv_maxspeed`
    - jump apex within ±1 u; duck-jump, ladder and strafe checked
    - zero clockwindow-ignore symptoms (forwardmove > 0 with unchanged origin)
    - Σmsec vs Σsim-time drift < 1 ms/min

    This decides the `lb_cmd_rate` default and the StartFrame pre-vs-post command phase.
11. **Long frame.** Injected 300 ms stall (`lb debug stall`) → bounded debt policy; no teleport or freeze.
12. **fixangle.** After spawn and teleport, the bot adopts the forced angles and later commands turn it.
13. **Panic isolation.** `lb debug panic <bot>` → that bot faulted and kicked; the others keep playing.
14. **Telemetry.** The bridge shows bots; an unsigned command is rejected; a signed `lb add` works.
15. **Perf baseline.** 12 bots at 1000 fps; adapter and core p99 recorded. Thresholds are set later (v2 §12).
16. **Late load and unload.** `meta load` mid-map gives degraded but working behaviour. `meta unload` kicks bots and restores hooks without a crash (Linux).

---

## 13. Milestones (platform) and risks

| M | Platform deliverables | Verification |
|---|---|---|
| **M0** | repo, builds (3 platforms, Docker, CI), adapter hooks, ABI v1, lifecycle, BotManager basics (add/kick/kill/list, fill quota, respawn, saved names), SelfState decoders, cvars/commands, compat, telemetry v2 minimum (hello/frame/event/perf) + bridge port, motor test harness, stand scripts, VM provisioning | M0 acceptance list above |
| **M1** | raw sensor channels (ClientSnapshot, registry pull, budgeted traces, pierce LOS), sound/event capture incl. ReHLDS `SV_StartSound`/`IMessageManager`, scheduler with sim-time windows, RNG streams, decision-trace sink + viewer, YAML v1 (main, difficulty, styles, profiles, names) | vertical slice: a bot notices, attacks and loses contact honestly; trace visible |
| **M2** | fixangle and seed emulation hardened, prediction-data channel (`GetWeaponData`/`UpdateClientData`), trace batch, full registry kinds, recorder/replay (`.lbrec`, `lb-cli replay`), editor command routing + debug draw, physics profile hash | traversal scenarios at several fps and long frames; replay reproduces decisions |
| **M3** | worker pool + epoch validation, BSP via VFS, nav cache store, budgeted live validation traces, config hot reload with staging | changelevel during generation discards stale results; cache keyed correctly |
| **M4** | perf metrics (p50/p95/p99), telemetry subscriptions, soak v2, difficulty/weapon YAML complete, GunGame bridge + inference, item rules YAML + learner | GunGame run on the VM; soak with 8 bots, 60 min clean |
| **M5** | teamplay join and balance, full event/sound classification (all 14 weapons, tripmine beams), disguise port (fakeping, avatars, SteamID, A2S), rotation and join/quit simulation | toggles verified individually; team switches correct |
| **M6** | load tests (8 bots + 16 humans; 12 + 12) on the ReHLDS VM, profiling, Windows smoke, release packaging, docs | budgets held; release artifacts |
| **M7** | `lb-ext` scripting host (mlua, instruction-count hook, memory-limited allocator), map-scenario QuotaPolicy hooks (chat came earlier, without the bus: `docs/chat.md`) | a scenario is added through the bounded API without breaking lifecycle, budgets or honesty |

**Risks** (mitigation in parentheses):

1. msec, timebase and clockwindow semantics per engine (M0 measurement; never over-send; warn on `host_framerate`).
2. fixangle emulation correctness on both engines (M0 tests 12).
3. Metamod re-entrancy through hooked tables, plus other plugins' side effects (flags; tests with AMXX, GunGame, reunion, spawn-protection on the VM).
4. `GLIBC`/`GLIBCXX` mismatch on the prod host (old base image, static libstdc++, CI gate).
5. i386 stack alignment (`LB_ENTRY` everywhere; ReHLDS callbacks tested).
6. Panic and unwind safety (`catch_unwind` at every entry, no re-entrancy, `extern "C"` abort as the final guard).
7. YAML crate churn (facade + cargo-deny).
8. Xash vs ReHLDS callback ordering and sound coverage (order-agnostic lifecycle; compat flags; `iStepLeft` fallback).
9. BHL vs hlsdk-portable format drift (typed-tag validation, golden fixtures).
10. 1000-fps overhead of Write* hooks (cheap early-out; measured in M0).
11. Event floods from plugins (interest bitmap, critical reserve).
12. Metamod gives plugins raw engine functions, so plugin-sent messages are invisible without ReHLDS `IMessageManager` (snapshot `frags` fallback).
13. Windows: SAFESEH / stack size / `crt-static` link issues (CI; `/SAFESEH:NO` fallback if LNK2026).
14. A2S import-hook fragility (x86 only, off by default, auto-disable).
15. Float engine time on long maps (double clock).
16. Late load has no event names until the next map (flagged in compat).
17. Worker CPU contention on small VPS hosts (1 low-priority worker; offline nav build via `lb-cli`).
18. Licensing: Valve SDK terms for hlsdk; own metamod ABI header to avoid GPL; NOTICE for YaPB-derived code, names and observer.
19. Spawn-protection plugin semantics unknown (rules YAML + observation).
20. Command channel exposure when telemetry runs on a remote VM (HMAC; loopback bind by default).

---

## Interfaces for the other architects

**AI core (lb-sense, lb-ai):**
- `lb_host::raw::{RawWorldFrame, RawClient, RawEntity, RawSound, RawPlayback, RawTempEnt}`: sensors only.
- `lb_game::model::{SelfState, Known<T>, PublicRules, GameModeState, Scoreboard, WeaponRegistry}` and `lb_game::events::{KillEvent, DamageTaken, Pickup*, SpawnEvent}`: visible to decision code.
- `lb_core::services::{TraceService, LosService}` (budgeted; `TraceResult` carries hit render info).
- `lb_host::driver::{BotInput{view_angles, move_world→encoded fwd/side/up, buttons_state, edges, impulse, weapon_cmd}, MotorFeedback{sent, frame_no, self_after}}`.
- Scheduler task registration with desired rates and cost units; `lb_core::rng::BotRng`; `lb_core::trace::TraceSink`; pinned config snapshots (difficulty, style, weapon policy); chat requests and replies (`lb_chat`, replies as a recorded outside input).

**Navigation (lb-bsp, lb-nav):**
- `BspSource{bytes: Arc<[u8]>, fingerprint: u128, worldmap_crc: Option<u32>, map}` delivered at map start.
- `TraceService` (hull/line/model, point contents) on the main thread under budget.
- Registry view of movers, buttons, doors, breakables, teleports and ladders.
- `WorkerPool` + `JobTag` + `CancelToken`.
- `install_dir/nav` (cache), `install_dir/maps` (overlays), and the `lb_core::fs::atomic_write` helper.
- `PhysicsProfile` and hash from lb-game.
- Editor routing: `lb nav …` with `CommandIssuer{slot, userid, access}` + the `DebugDraw` API.
- Telemetry `graph_*` and `overlay` message slots.

### Critical Files for Implementation
- /Users/nikita/Git/hl-bots/crates/lb-ffi/src/lib.rs (ABI source of truth → `adapter/include/lb/lb_abi.h` via cbindgen)
- /Users/nikita/Git/hl-bots/adapter/src/fake_client.cpp (creation via hooked tables, kick, fake argv, RunPlayerMove, fixangle/seed emulation)
- /Users/nikita/Git/hl-bots/adapter/src/hooks_engine.cpp (message/sound/event capture into the event arena; reference: /Users/nikita/Git/half-life/yapb-halflife/src/linkage.cpp)
- /Users/nikita/Git/hl-bots/crates/lb-runtime/src/frame.rs (pre/post pipeline, scheduler, budgets, panic isolation)
- /Users/nikita/Git/hl-bots/CMakeLists.txt (Corrosion link, per-platform exports and runtime flags)
