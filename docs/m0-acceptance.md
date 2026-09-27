# M0 acceptance: skeleton and integration

State as of 2026-09-27. Test stand: Xash3D FWGS 0.21 (arm64) + Metamod-FWGS 1.0.0.223 + hlsdk-portable, macOS.
Production-like server (2026-09-27): ReHLDS 3.15 + Metamod-r 1.3.0.131 + BugfixedHL + AMX Mod X 1.9 + GunGame 2.3,
Linux i386, `sys_ticrate 1000`. Windows has not been run yet (`docs/stands/windows-smoke.md`).

## Results against the plan's criteria

| Criterion                                 | Status           | How it was checked                                       |
|-------------------------------------------|------------------|----------------------------------------------------------|
| Three binaries, exactly 5 exports         | 2 of 3           | `check-binary.sh`: macOS, Linux i386; Windows — CI       |
| `.so`: GLIBC ≤ 2.27, no C++ runtime       | yes              | max GLIBC 2.25, NEEDED is glibc only                     |
| Loads under Metamod-FWGS/Xash             | yes              | test stand                                               |
| Loads under Metamod-r/ReHLDS              | yes              | ReHLDS server: `lb compat`, all ReHLDS channels on       |
| AMXX sees bots, GunGame gives weapons     | likely yes       | bots got the GunGame warmup crowbar instead of the glock |
| SelfState is correct                      | yes              | `lb list`, golden message fixtures                       |
| 100 kill → respawn cycles                 | yes              | `check-respawn.sh 100`: 0 failures, worst respawn 3 s    |
| Slot reuse rejects the old gen            | yes              | `lb debug stalecmd` → STALE; slot "ghosts"               |
| 20 map changes, names preserved           | yes              | `check-changelevel.sh 20`: 0 failures                    |
| Autovacate on 24 slots                    | yes              | quota 24 normal → 23 bots                                |
| Golden decoder fixtures                   | yes              | `lb-game/tests/decode_fixtures.rs` + snapshot            |
| msec measurement, choice of `lb_cmd_rate` | yes              | Xash: `msec-matrix.sh`; ReHLDS: motor tests at rate 100  |
| 300 ms long frame                         | yes              | `lb debug stall 300`: debt is capped, bots alive         |
| fixangle after spawn                      | yes              | mode 1 events = spawn angles, v_angle matches            |
| fixangle after teleport                   | deferred         | M2: obstacle course with a Teleport transition           |
| Panic isolation                           | yes              | `lb debug panic`: bot faulted → kick, server runs        |
| Unsigned telemetry command is rejected    | yes              | no signature/wrong key/nonce replay — rejected           |
| Performance, 12 bots, 1000 fps            | yes (Xash)       | release: core p99 61 µs, process CPU 13.7%               |
| Late load and unload                      | yes, with caveat | see "Known limitations"                                  |

## msec semantics (Xash, crossfire, 4 bots)

A 3 s run and three jumps for each combination; "drift" is the test's frame time minus the msec sent (the
remainder stays in the accumulator, it is not lost).

| fps  | `lb_cmd_rate` | Run speed | Jump apex | msec/command | Drift, ms |
|------|---------------|-----------|-----------|--------------|-----------|
| 100  | every frame   | 270.0     | 45.0      | 9.93         | 0.22      |
| 100  | 250           | 270.0     | 45.0      | 9.94         | 0.53      |
| 100  | 100           | 270.0     | 45.0      | 10.59        | 0.70      |
| 500  | every frame   | 270.0     | 45.0      | 2.00         | 0.62      |
| 500  | 250           | 270.0     | 45.0      | 4.03         | 2.00      |
| 500  | 100           | 270.0     | 45.0      | 10.01        | 6.39      |
| 1000 | every frame   | 270.0     | 45.0      | 1.00         | 0.27      |
| 1000 | 250           | 270.0     | 45.0      | 4.00         | 2.48      |
| 1000 | 100           | 270.0     | 45.0      | 10.00        | 6.82      |

Speed and apex are exact in all combinations (target: ±1% and ±1u). Drift does not accumulate: it never exceeds
one command quantum, and `lb perf bots` shows a remainder below one quantum even after minutes of running. The
default `lb_cmd_rate 100` is kept: the physics is the same, and `PM_Move` costs 10 times less than sending a
command every frame at 1000 fps.

On ReHLDS (7 bots, `lb_cmd_rate 100`, ~710 fps) the motor tests gave a steady run speed of 300.0 (the server's
maxspeed), jump apexes of 45.0 in all 21 jumps, 10.3 ms per command and a drift within ±5.3 ms. Two bots that ran
into other players stopped early; the test picks the most open direction by walls only.

## Performance (release, Xash arm64, 12 bots, ~1000 fps, 60 s)

| Metric                      | Value                                 |
|-----------------------------|---------------------------------------|
| Core per frame, p50/p95/p99 | 7.9 / 27.8 / 61.2 µs                  |
| Core per frame, mean        | 11.5 µs                               |
| Maximum over 60 s           | 5.0 ms (frame with a console command) |
| Server process CPU          | 13.7%                                 |
| Process RSS                 | 64 MB                                 |

This is the M0 pipeline without AI: a baseline for comparison in later milestones.

## Findings on the test stand

- **Metamod-FWGS does not call ClientDisconnect.** In `metamod/src/dllapi.cpp` (commit 5de9af2 "Completely
  reworked game library APIs functions hooking") `mm_ClientDisconnect` only clears cvar queries and does not call
  `META_DLLAPI_HANDLE_void`: neither plugins nor the game DLL itself receive the disconnect. The core works
  without it (a new client in the slot removes the "ghost", a kicked bot is forgotten after 6 s, and on map
  change the adapter frees bot slots itself). The patch for Metamod-FWGS is two lines:

  ```cpp
  static void MM_PRE_HOOK EXT_FUNC mm_ClientDisconnect(edict_t *pEntity)
  {
      g_players.clear_player_cvar_query(pEntity);
      META_DLLAPI_HANDLE_void(FN_CLIENTDISCONNECT, pfnClientDisconnect, (pEntity));
      RETURN_API_void();
  }
  ```
- **`sv_hibernate_when_empty 1`** in Xash does not count bots as players: the server sleeps in 50 ms chunks and
  runs at ~25 fps. The stand is started with `+sv_hibernate_when_empty 0`.
- **`kill` is an engine command**, it never reaches the game DLL's `ClientCommand`. The adapter does what the
  engine does: calls `ClientKill` on a live bot through the hooked table (other plugins see it).
- **Simulation time restarts on every map.** The bot join timer was counted from the previous map's time, and
  after a long map bots did not join; now the delay is counted from the map's first frame.
- **Real messages**: on spawn HLDM sends `Damage` with zero damage (HUD reset), after death —
  `CurWeapon(0, 255, 255)`. Damage perception in M1 must ignore zero `Damage`.

## Known limitations

- **The `lb` command after `meta unload` + `meta load` without ReHLDS.** Metamod (both r and FWGS) on engines
  without ReHLDS only disables the unloaded plugin's commands and does not re-enable them on reload; until the
  server restarts, `lb` answers "command 'lb' unavailable". On ReHLDS, Metamod-r removes the command via
  `Cmd_RemoveCmd`, and it works after loading. Bots and the core work after a late load on all engines.
- **The module is not unloaded from memory** (Rust registers thread-local destructors, `dlclose` keeps the
  image), so the adapter resets its own state on detach, and a reload picks up the same image.
- **M0 bots stand still**: when the quota exceeds the number of spawn points (23 bots on crossfire's 16 points),
  simultaneous respawns turn into a chain of telefrags. With navigation (M1) bots move away from the spawn.

## What is left of M0

1. On ReHLDS: the full msec matrix (`msec-matrix.sh` on the VM), an explicit AMXX check (`amx_who`, GunGame level
   weapons after warmup) and a performance measurement.
2. First CI run: Windows build and export check via `dumpbin`.
3. Windows smoke test following `docs/stands/windows-smoke.md` (optional).
