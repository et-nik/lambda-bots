# yapb-halflife analysis: engine integration and infrastructure

> Original design note (in English), prepared during planning on 2026-09-26.
> A condensed version with the final decisions lives in the implementation plan; in case of conflict, the plan
> and later decisions in the repository take precedence. `file:line` references reflect the sources as of the note's date.

# YaPB for Half-Life: engine-integration and infrastructure spec

**Root:** `/Users/nikita/Git/half-life/yapb-halflife`. Every `src/…`, `inc/…`, `cfg/…`, `ext/…` or `tools/…` reference below is under this root. Line numbers are from HEAD `3f62b3a`. Combat, tasks and navigation are covered only where they touch the engine.

Key files (absolute):
- Linkage and hooks: `/Users/nikita/Git/half-life/yapb-halflife/src/linkage.cpp`, `.../src/hooks.cpp`, `.../inc/hooks.h`, `.../src/entities.cpp` (it is `#include`d at the end of linkage.cpp, `linkage.cpp:1128`)
- Engine wrapper: `.../src/engine.cpp`, `.../inc/engine.h`
- Messages: `.../src/message.cpp`, `.../inc/message.h`
- Bot lifecycle and quota: `.../src/manager.cpp`, `.../inc/manager.h`; per-bot frame and RunPlayerMove: `.../src/botlib.cpp`
- Configs, commands, storage, support: `.../src/config.cpp`, `.../src/control.cpp`, `.../src/storage.cpp`, `.../src/support.cpp`, `.../src/sounds.cpp`, `.../src/fakeping.cpp`, `.../src/telemetry.cpp`, `.../src/module.cpp`
- Constants and metadata: `.../inc/constant.h`, `.../inc/product.h`, `.../ext/linkage/linkage/{goldsrc,metamod,physint}.h`

---

## 1. Entry points and hooks

### 1.1 Exported symbols
The linker version script `ext/ldscripts/version.lds` exports only `Meta_*`, `Server_*`, `GiveFnptrsToDll`, `GetBotAPI`, `GetEntityAPI` and `GetNewDLLFunctions`. Everything else is hidden.

| Symbol | Where | Purpose |
|---|---|---|
| `GiveFnptrsToDll(enginefuncs_t*, globalvars_t*)` | `linkage.cpp:1051-1084` | Copies the engine table into `engfuncs` and stores `globals`, then calls `game.postload()`. Under metamod it returns there. Standalone it resolves the real game's `GiveFnptrsToDll`, hooks the engine table in place with `GetEngineFunctions(table, nullptr)`, runs `entlink.initialize()` (not on Xash), and forwards. On MSVC x86 it is exported as stdcall `_GiveFnptrsToDll@8` (`:1035-1049`). |
| `GetEntityAPI(gamefuncs_t*, int)` | `:76-498` | DLL API table. Pre-hooks under metamod; a full wrapped table standalone. `GetEntityAPI2` is not exported. |
| `GetEntityAPI_Post` | `:500-581` | C linkage but not exported. It reaches metamod through the Meta_Attach table. |
| `GetEngineFunctions` / `GetEngineFunctions_Post` | `:583-865` / `:909-935` | Engine hooks (pre / post). |
| `GetNewDLLFunctions` | `:867-907` | Only `pfnOnFreeEntPrivateData`. There is a bug here, see §13. |
| `Meta_Query` | `:937-969` | Stores `gpMetaUtilFuncs` and checks interface `"5:13"` (`metamod.h:40`) using major/minor rules. |
| `Meta_Attach` | `:971-999` | Refuses if `now > PT_CHANGELEVEL`. Supplies `{GetEntityAPI, GetEntityAPI_Post, null, null, GetNewDLLFunctions, null, GetEngineFunctions, GetEngineFunctions_Post}`. |
| `Meta_Detach` | `:1001-1025` | Stops the worker, `kickEveryone(true)`, `practice.save()`, disables the query hook, `bots.destroy()`. |
| `Meta_Init` | `:1027-1032` | Sets `GameFlags::Metamod`. This is how metamod mode is detected. |
| `Server_GetBlendingInterface` | `:1086-1098` | Forwards to the game DLL (only meaningful standalone). |
| `Server_GetPhysicsInterface` | `:1100-1125` | Xash only. `SV_CreateEntity` resolves the entity class function in the game library by name; `SV_PhysicsEntity` returns false. |
| `GetBotAPI(int version)` | `module.cpp:116-123` | `IYaPBModule` v1 for an AMXX module: `isBot`, nearest node, node origin/flags, `setBotGoal`/`GoalOrigin`, current node, `hasGraph` (`inc/module.h`). |
| `LINK_ENTITY(...)` exports | `entities.cpp:14-282` | Only when built with `-DLINKENT_STATIC` (meson option `static_linkent`). About 250 Half-Life entity classes forwarded to the game DLL. |

`Plugin_info` (`linkage.cpp:23-33`): name, version, date and author come from `product.h`; logtag `"YB"`; loadable `PT_CHANGELEVEL`; unloadable `PT_ANYTIME`.

### 1.2 Load order
- **Metamod:** `Meta_Init` → `GiveFnptrsToDll` → `postload()` → `Meta_Query` → `Meta_Attach` → metamod pulls the tables.
- **`Game::postload()`** (`engine.cpp:922-1000`) does, in order:
  1. `bstor.checkInstallLocation()`.
  2. Logger to `data/logs/yapb_L<ddmmyyyy>.txt`.
  3. Creates `conf/lang`, `data/{train,graph,logs,pwf}`.
  4. `registerCvars()` and custom cvar descriptions.
  5. Registers server commands `yb` and `yapb`, and `meta` when not under metamod.
  6. HL25 detection (`sv_use_steam_networking` exists or `host_hl25_extended_structs > 0`, `:583-588`).
  7. `conf.initWeapons()`, then `m_engineLib.locate(pfnPrecacheModel)`.
  8. Android or emscripten: sets `Xash3D|Mobility`.
  9. `loadGameBinary()`. Under metamod the library is loaded and then immediately unloaded (`:994-997`).
- **`loadGameBinary()`** (`:839-920`):
  - `logger.fatal` (process abort) if the gamedir is not `valve` (`:847-849`).
  - Library names come from `constructGameBinaryName` (`:785-837`): `hl`, `hl_i386` (32-bit Linux), `hl_amd64`, `hl_arm64`, `hl_riscv64d`, `hl_ppc64le`, `hl_armv7hf`, `libhl_android_*`, `hl_psvita`. Emscripten uses env `XASH3D_GAMELIBPATH`; Android uses env `XASH3D_GAMELIBDIR`.
  - Xash detection: cvar `build` → `Xash3DLegacy`; `host_ver` → `Xash3D`.
  - Sets `HalfLife|HasStudioModels`, plus `HasFakePings` when not Xash. **These flags are set only if a game library file is found** (`:864-918`).

### 1.3 DLL API hooks (gamefuncs)
In metamod mode each pre-hook does its work, then `RETURN_META(MRES_IGNORED)` unless noted. Standalone, the same lambda calls the real `dllapi.*`.

| Hook | Pre / post | Line | Action |
|---|---|---|---|
| GameInit | pre | `linkage.cpp:102` | `conf.loadMainConfig(true)`: executes every `yapb.cfg` line as a server command, starts the HTTP connectivity probe, binds the menu key. Then `printBotVersion()`. |
| Spawn | pre | `:123` | `game.precache()` once per map: laserbeam/arrow sprites, 3 editor sounds, resets map flags, `registerCvars(true)` for GameRef cvars. Then `game.onSpawnEntity(ent)`, which counts `info_player_start` and `info_player_deathmatch` into `m_spawnCount[0]` (`engine.cpp:144-157`). |
| Spawn | post (mm) / after game (standalone) | `:513`, `:140` | Clears `FL_WORLDBRUSH` on `kRenderTransTexture` entities so glass is not treated as world. Post returns `MRES_HANDLED`. |
| Touch | pre | `:146` | If the touched entity is a bot and the other is a breakable → `bot->checkBreakable(other)` (starts `ShootBreakable`). |
| ClientConnect | pre | `:175` | Address `"loopback"` → `setLocalEntity` and, on listen servers, graph editor = host. Then `util.updateClients()`. Bots never reach this hook (see §2.1). |
| ClientDisconnect | pre | `:215` | Bot → `bots.disconnectBot()` (save name, erase). Updates clients, clears graph editor / command issuer. |
| ClientPutInServer | pre | `:254` | `fakeping.emit(ent)` if fake ping is enabled. |
| ClientUserInfoChanged | pre | `:275` | `ctrl.assignAdminRights` (setinfo password, dedicated only). `bots.checkBotModel` → `refreshCreatureStatus` (model mask). |
| ClientCommand | pre | `:290` | `yb`/`yapb` commands → **SUPERCEDE**. `menuselect` for an open bot menu → **SUPERCEDE**. Otherwise `bots.captureChat(argv0, argv1, ent)` for say/say_team. |
| ServerActivate | post (mm) `:543`; after game standalone `:338-341` | | `game.levelInitialize(edicts, count)`. The pre-hook in mm is a no-op. |
| ServerDeactivate | pre | `:344` | `game.levelShutdown()`. |
| StartFrame | pre | `:364-407` | See §1.6 for order. Post (mm, `:527`) / after game (standalone, `:412-418`): `bots.frame()` then `telemetry.frame()`. |
| UpdateClientData | post (mm, `:558`, only if `HasFakePings`); wraps game standalone (`:421`) | | For humans with `IN_SCORE` in `button|oldbuttons` → `fakeping.emit`. |
| CmdStart | standalone only | `:445-459` | For bots, `random_seed = rg(0, 0x7fffffff)` unless `yb_whose_your_daddy`. **Absent under metamod.** |
| PM_Move | pre | `:461` | `illum.setWorldModel(pm->physents[0].model)`: grabs the world `model_t*` for lightmap sampling (first call per map). |
| KeyValue | pre | `:475` | `func_breakable` with `material == 7` → `markBreakableAsInvalid` (this marking is later wiped, see §13). |

### 1.4 NEW DLL API
- `pfnOnFreeEntPrivateData` pre (`:889-904`): for each bot, if the freed entity is its enemy, clear `m_enemy` and `m_lastEnemy`; if it is the bot itself, `markStale()`. Standalone it forwards to `newapi.pfnOnFreeEntPrivateData` with no null check.

### 1.5 Engine hooks

| Engine function | Pre / post | Line | Action |
|---|---|---|---|
| CreateNamedEntity | standalone only, and only when `entlink.needsBypass()` (non-Windows listen server) | `:588` | Re-enables the dlsym detour after a paused `CreateInterface` lookup. |
| LightStyle | pre | `:600` | `illum.updateLight(style, val)` (only while light animation is enabled). |
| GetPlayerAuthId | pre | `:611` | Bot → SUPERCEDE with `util.getFakeSteamId`: `"BOT"`, or `"STEAM_0:1:%d"` (`fnv1a(name) & 0xffff00`) when `yb_enable_fake_steamids` (`support.cpp:283-291`). |
| EmitSound | pre | `:628` | `sounds.acquire(entity, sample, volume)`. |
| MessageBegin | pre | `:647` | `msgs.start(ed, msgType)`. |
| WriteByte / Char / Short / Long / Angle / Coord / String / Entity | pre | `:666-744` | `msgs.collect(value)` as int32, float, or `const char*` (the pointer is stored, not copied). |
| MessageEnd | post (mm, `:912`); standalone wraps: engine first, then `msgs.stop()` (`:658`) | | Dispatches the handler after the engine has finished, so handlers can send messages. |
| RegUserMsg | post (mm, `:918`, uses `META_RESULT_ORIG_RET`); standalone wraps (`:750`) | | `msgs.add(name, id)`. |
| ClientCommand (stuffcmd) | pre | `:747`, `Hooks::handler_engClientCommand` `:37-73` | Blocks stuffcmd to bots, fake clients or `FL_DORMANT` entities (SUPERCEDE). |
| ClientPrintf | pre | `:765` | Blocks it for fake clients. |
| Cmd_Args / Cmd_Argv / Cmd_Argc | pre | `:785-849` | While a bot command is executing (`game.isBotCmd()`), returns the bot's own argv (SUPERCEDE). |
| SetClientMaxspeed | pre | `:851` | Mirrors the value into the bot's `pev->maxspeed`. |

### 1.6 Level and frame lifecycle
**`levelInitialize`** (`engine.cpp:52-142`), in order:
1. `timerStorage` → `&globals->time`; allow commands; `bots.destroy()`; clear the team-string cache; `worker.startup(yb_threadpool_workers)`; clear breakables.
2. `conf.loadConfigs()`: custom, names, bots.json, chat, weapon, lang, logos, avatars, difficulty.
3. Reset world model; `loadMainConfig()` (second-pass rules, §9); `ensureHealthyGameEnvironment()`; exec `conf/maps/<map>.cfg`; `graph.loadGraphData()` (starts the analyzer if there is no graph).
4. `gameState.roundStart()`: one per map, see §2.5.
5. `bots.initQuota()` (join delay); `fakequeries.init()`; reset print and fake-ping timers.
6. Walk all edicts:
   - `worldspawn` → `m_startEntity` (the index base for everything).
   - `info_player_start`/`deathmatch` → `rendermode TransAlpha`, `renderamt 127`, `EF_NODRAW`.
   - `func_door*` → `MapFlags::HasDoors`; `func_button*` → `HasButtons`.
   - Breakables → `m_breakables`, validity = `impulse <= 0`.

**`levelShutdown`** (`:159-204`): hold quota management 60 s; save practice; stop the worker; remove the killer entity; kick everyone on Xash3DLegacy only; unprecache; enable light animation; `setNeedForWelcome(false)`; clear the local entity; `graph.reset`; suspend the analyzer; deny commands; zero spawn counts.

**StartFrame, pre part** (`linkage.cpp:364-407`):
1. `illum.animateLight` → `util.updateClients` (also computes `simulateNoise`).
2. `graph.frame` (editor on) → `analyzer.update`.
3. `game.slowFrame` (`engine.cpp:1048-1101`):
   - Every 0.125–0.25 s: `ensureHealthyGameEnvironment`.
   - Every 0.5–1 s: admin rights, difficulty sync, auto-kill, leaders, light levels, narrow places, **`applyGameModes`**, tripmine rescan, cvar bounds, welcome, `checkNeedsToBeKicked`, `fakeping.calculate`.
4. `vistab.rebuild` → when bots are online: `updateActiveGrenade` (0.25 s) and `updateInterestingEntities` (0.5 s).
5. `bots.maintainQuota` → `balanceBotDifficulties` → `flushPrintQueue`.

**StartFrame, post part:** `bots.frame()` then `telemetry.frame()`.

### 1.7 Standalone (gamedll replacement) mode
- `liblist.gam`: `gamedll "addons/yapb/bin/yapb.dll"` (README). Library tables are copied from the real `hl` library. `gpGamedllFuncs = &dllfuncs`, so `MDLL_*` calls the real game directly.
- Entity class lookups: the engine resolves entity classes such as `weapon_crowbar` by `dlsym`/`GetProcAddress` on **our** library. `EntityLinkHook` (`hooks.cpp:105-194`) inline-detours `GetProcAddress`/`dlsym` (and `FreeLibrary`/`dlclose` on non-Windows listen servers). Missing symbols fall back to the game library, with a cache (`m_exports`). The `CreateInterface` lookup pauses the hook (`:113-120`).
  - Disabled on non-x86, under metamod, and on Xash (`linkage.cpp:1078`; Xash uses `Server_GetPhysicsInterface` instead).
  - Alternative: `LINKENT_STATIC` explicit exports.
- `player()` creation: `entlink.callPlayerFunction` resolves `"player"` in the game library (`hooks.cpp:151-170`). Under metamod: `MUTIL_CallGameEntity(PLID,"player",&ent->v)` (`manager.cpp:122-138`).
- Only in this mode: the `CmdStart` seed hook, the MessageEnd/RegUserMsg wrappers, the `CreateNamedEntity` hook, the fake `meta` command (`engine.cpp:964-968`).

---

## 2. Fake clients and lifecycle

### 2.1 Creation sequence
Bots are created from a queue: `maintainQuota` pops one request per 0.1 s (`manager.cpp:449-474`) and calls `BotManager::create` (`:148-322`).

1. Refuse if there is no graph or the graph has changed (`GraphError`).
2. Resolve a bots.json profile (§2.4). Pick difficulty: request, then profile, then `yb_difficulty`; if that is invalid, `rg(3,4)` is chosen **and written back to the cvar** (`:220-227`). Pick personality: request, then profile, then `yb_preferred_personality`, then random 50% Normal / 25% Rusher / 25% Careful.
3. Pick the name (§2.3) and apply `yb_name_prefix` (not for roster names). A prefix forces `yb_save_bots_names 0`.
4. **`game.createFakeClient(name)`** (`engine.cpp:1243-1263`):
   - `pfnCreateFakeClient`; `ent->v = {}` (full entvars reset); restore `pContainingEntity`, `flags = FL_FAKECLIENT|FL_CLIENT`, `netname`.
   - `pfnFreeEntPrivateData` if private data is left over.
   - Null result → `MaxPlayersReached`.
5. **`Bot::Bot`** (`manager.cpp:1133-1330`):
   1. `pev = &ent->v`; `execGameEntity` → the game's `player()` allocates the `CBasePlayer`.
   2. Infobuffer keys through `pfnSetClientKeyValue(clientIndex, buf, …)`:
      - `_vgui_menus 0`, `_ah 0` (CS leftovers)
      - `*bot 1` only if `yb_show_latency == 1`
      - `*sid <steamid64>` if `yb_show_avatars` (from `avatars.cfg`, or the profile's `avatar`)
      - `model` (§2.2)
      - **No `topcolor`, `bottomcolor` or `rate`.** The engine's CreateFakeClient default userinfo (HLDS/ReHLDS behavior: `model gordon`, `topcolor 1`, `bottomcolor 1`) stays; this is engine behavior worth verifying.
   3. `MDLL_ClientConnect(ent, netname, "127.0.0.<edictIndex+100>", reject)`. If `reject` is non-empty: log, `kick "<name>"`, `FL_KILLME`, return. The Bot object is still pushed (§13).
   4. `MDLL_ClientPutInServer(ent)`; `flags |= FL_CLIENT|FL_FAKECLIENT`.
   5. `m_notStarted = true`; `m_index = edictIndex-1` (`entindex() = m_index+1`); logo decal; stay time (rotation); chat delay and probability; difficulty (min/max randomization unless the roster set it); ping base; think timers (`m_previousThinkTime = now-0.1`); personality → aggression/fear bases (profile overrides); `clearAmmoInfo`; A* planner; `newRound()`.
6. `rg.seed(index + time)`; mark the name used; bind the profile; push to `m_bots`.

`MDLL_*` are direct game-DLL calls through `gpGamedllFuncs->dllapi_table`, bypassing metamod hooks (`ext/linkage/linkage/metamod.h:182-198`). Other plugins (for example AMXX `client_putinserver`, and `ClientCommand` hooks for bot `say` or weapon switches) do not see bot connects or commands. AMXX falls back to emulating bot connects on `ClientUserInfoChanged`.

### 2.2 Team and model
- **Teamplay** (`manager.cpp:1166-1180`): model = `yb_join_team` if it is not `"any"`. Otherwise, when the requested `team >= 1`, model = `mp_teamlist.split(";")[team-1]`. A roster `model` overrides both, so it picks the team. With no model, the code relies on game auto-assignment (but see §13 about the engine's default `gordon`).
- **FFA:** model from the comma list `yb_botskin`, or the built-in list `barney, gina, gman, gordon, helmet, hgrunt, recon, robo, scientist, zombie` (`:1185`).
- HL has no team or class menus. `updateTeamJoin` (`:1828-1840`) only clears `m_notStarted` and, with 20% chance, queues a welcome chat.

### 2.3 Name generation (`create`, `:259-293`)
Priority:
1. Explicit request name.
2. Saved names restored on changelevel (`m_saveBotNames`, popped in `addbot` `:377-381`, when `yb_save_bots_names`).
3. Roster entry name.
4. `conf.pickBotName()`: random unused entry from `lang/<yb_language>_names.cfg`, falling back to `en_names.cfg`; `2*N` attempts; names truncated to 31 chars (`config.cpp:152-179, 800-814`).
5. `"yapb_<100-10000>.<100-10000>"`.

Uniqueness is checked against connected clients only for roster picks (`config.cpp:835-848`).

### 2.4 bots.json roster (commits `f0b72da`, `e3c27dd`)
- Loaded on every level init from `conf/bots.json` via `LoadFileForMe` (`config.cpp:181-355`). rapidjson with comments and trailing commas allowed. The root **must be an array** (e3c27dd changed it from `{ "bots": [...] }`).
- Entry fields:
  - `name` (required, trimmed, truncated to 31 chars, duplicates skipped)
  - `difficulty` (0–4 or `noob|easy|normal|hard|expert`)
  - `personality` (0–2 or `normal|rusher|careful`)
  - `model`, `avatar`
  - `aggression`, `fear` (numbers clamped to 0..1)
- Active when `yb_bots_roster` is on and the list is non-empty.
- Which requests get a profile (`manager.cpp:176-207`):
  - Automatic quota creations with no name → random free entry.
  - `yb addprofiled [name]` → that entry, or any free one.
  - A restored name re-binds to its entry if free.
  - Manual `add`, `fill` and menu requests never use the roster.
- Entries are freed in `markStale`. The profile's difficulty is authoritative against `difficulty_min`/`max`.

### 2.5 Life cycle (HL has no rounds)
- **"Round":** `GameState::roundStart()` runs **once per map** (`engine.cpp:1670-1700`):
  - start = now; mid = start + 90 s; end = start + `mp_timelimit*60`, or +86400 s when there is no time limit.
  - Calls `bots.initRound()`: leaders reset, filters reset, `practice.update()`, `newRound` for all bots.
- **Alive check each think:** `isAliveEntity` = `deadflag==DEAD_NO && health>0 && movetype!=NOCLIP` (`engine.cpp:1275-1280`).
- **Spawn:** the `ResetHUD` message to the bot → `bot->spawned()` = `newRound()` + `clearTasks()` (`message.cpp:218-223`, `botlib.cpp:1958-1961`). `newRound` resets per-life state and recomputes the think interval (`manager.cpp:1484-1761`).
- **Death:** `DeathMsg` → `bots.handleDeath(killer, victim)` (`manager.cpp:1427-1482`):
  - Hard/Expert teammates who can see the killer target it.
  - Victim: `spawned()` then `m_isAlive=false`. Killer bot: `setLastVictim`. A same-team human killer → `m_voteKickIndex`.
  - Ignored when killer is world (index 0) or killer == victim (`message.cpp:117`).
- **Respawn (commit a5a428a):**
  - While dead, `update()` runs `resetMovement` + `checkRespawn()` + `runMovement()` (`botlib.cpp:1751-1759`).
  - `checkRespawn` (`:1762-1777`): if `yb_force_respawn` and `pev->deadflag == DEAD_RESPAWNABLE` and `now >= m_respawnPressTime` (set to `now + rg(0.5,1.5)` in `newRound`, `manager.cpp:1687`), press `IN_JUMP` on every other think (only if it was not in `m_oldButtons`). The game's PlayerDeathThink needs a released frame and then a press.
  - `newRound` also clears `m_duckTime`, because a stale duck would hold `IN_DUCK` and stall the respawn (`:1685`).
- **Dead-time extras:** `vote %d` / `votemap %d` bot commands (CS-only, no effect in HL); `yb_tkpunish` (`botlib.cpp:1718-1738`).
- **Kill:** `Bot::kill` → `touchKillerEntity` (`manager.cpp:70-120`): one shared `trigger_hurt` with `EF_NODRAW` placed far away; classname set to the bot's weapon classname; `dmg_inflictor = bot`; `dmg = (hp+armor)*4`; KeyValue `damagetype = 16`; `MDLL_Touch`. If the entity cannot be created → `MDLL_ClientKill`.
- **Kick:** `Bot::kick` (`:1775-1789`): `markStale()` (fake ping reset, `m_isStale`, lock both thread mutexes, free name and profile, clear `FL_FAKECLIENT`, set `FL_DORMANT`), then server command `kick "<netname>"`. The engine drop → `ClientDisconnect` hook → `disconnectBot` (save name unless kicked by rotation; erase).
- **Map change:** the engine drops fake clients (HLDS behavior); names are saved and re-added after `yb_join_delay`.

### 2.6 Quota algorithm (`maintainQuota`, `manager.cpp:425-546`)
- Skipped while the hold timer runs: 60 s after level shutdown, 10 s after a failed bot. `bots.destroy()` invalidates the timer.
- No graph or graph changed → `yb_quota=0` (with a message). Analysis running → skip. The analyzer restores the quota with `cv_quota.revert()` when done (`analyze.cpp:169`).
- **Request processing:** one per 0.1 s. Manual requests add +1 to the quota (capped at maxClients). `GraphError` → clear queue, quota 0. `MaxPlayersReached` → clear queue, quota = current bots.
- **Every 0.4 s:**
  1. `desired = yb_quota`, clamped to 0..maxClients. On a listen server with no humans → return.
  2. Mode `fill`: `desired -= humansInTeams`. Mode `match`: `desired = (quota_match or quota) * humansInTeams`.
  3. `yb_join_after_player` with no humans → 0.
  4. Autovacate on: `desired = min(desired, maxClients - (humans + keep_slots))`. `humans` = all humans if `kick_after_player_connect`, otherwise humans in teams. Off: `min(desired, maxClients - humansInTeams)`.
  5. Spawn cap: `spawnCount - humansInTeams`. Unlimited if the `CustomSpawnPoint` entity exists (default `view_spawn`) or `DisableSpawnControl=yes`.
  6. Add one bot (`createRandom`), or `balancedKickRandom(false)` (does not decrement quota), or clear saved names when balanced.
- `getHumansCount(true)` counts only `team2 ∈ {First, Second}` (`:1347-1361`). See §13 for FFA.
- **kickRandom priority** (`:766-845`): a dead bot, then the lowest frags, then random. `balancedKickRandom` picks from the bigger `team2` side.
- **Other paths:** `kickEveryone(instant, zeroQuota)`; `kickBot(index)` (decrements quota); `kickFromTeam`; `serverFill` (`:654-685`, keeps autovacate slots; sets `mp_limitteams`/`mp_autoteambalance` to 0 for team 1/2).
- **Rotation:** `yb_rotate_bots` gives each bot a stay time of `rg(stay_min, stay_max)`; it kicks itself, and the quota re-adds it (`botlib.cpp:1675-1680`).
- **Auto-kill** (`:561-595`): when no alive humans (in teams) and some bots alive → after `yb_autokill_delay`, kill all bots.
- **Difficulty:** `updateBotDifficulties` applies `yb_difficulty` changes when min, max and auto are all off. `balanceBotDifficulties` (auto, by K:D, every interval) prints a debug line.

---

## 3. RunPlayerMove and timing

- `bots.frame()` → `Bot::frame()` for every bot, every server frame (`botlib.cpp:1653-1682`):
  - `update()` runs only when `m_thinkTimer.time < now`; then `time = now + interval`.
  - Every 0.5 s: `checkSpawnConditions`, `checkForChat`, `checkBreakablesAround`, longjump detection (`pfnGetPhysicsKeyValue(ent,"slj")=="1"` while alive), rotation kick.
- **Think interval** (`manager.cpp:1747-1760`): `1/clamp(yb_think_fps, 30, 90)` (the cvar allows 24..90; default 90). On Xash the minimum is 50 fps. `interval = 0` (every frame) when Xash and `yb_think_fps_disable=1`, unless a dedicated server with `sys_ticrate > 100` (which auto-sets it to 0, `:1964-1977`). **RunPlayerMove is issued only on think frames, not every server frame.**
- `update()` (`botlib.cpp:1684-1760`):
  - Refresh alive, team, health, creature flag, GunGame forced weapon; re-assert `FL_CLIENT|FL_FAKECLIENT`.
  - If `pev->maxspeed < 10` and the task is Normal, set it from `sv_maxspeed` (HL leaves 0).
  - Movement is allowed only if alive, not frozen, not graph-changed, and `maxspeed >= 10`.
  - `logic()`; heavy sensing (`setConditions`) at 10 Hz (`canRunHeavyWeight`, `:2373-2382`).
  - Always ends with `runMovement()`.
- **`runMovement`** (`:2399-2428`):
  - `m_frameInterval = now - m_previousThinkTime`
  - `msec = min(round((now - m_previousThinkTime)*1000), 255)` (`:2384-2388`); the first msec is about 100.
  - `translateInput()` (`navigate.cpp:1056-1085`):
    - `IN_DUCK` while `m_duckTime >= now`
    - an `IN_JUMP` press records `m_jumpTime`
    - airborne within 0.85 s of a jump → `IN_DUCK`
    - `IN_FORWARD`/`BACK`/`MOVELEFT`/`MOVERIGHT` derived from the signs of `m_moveSpeed` and `m_strafeSpeed` when not already set
  - `pfnRunPlayerMove(ent, getRpmAngles(), m_moveSpeed, m_strafeSpeed, 0, (uint16)pev->button, (uint8)pev->impulse, msec)`
  - `getRpmAngles` (`:2390-2397`) = `pev->v_angle` when stuck, approaching a ladder, or in the Attack task; otherwise `m_moveAngles`.
  - `m_oldButtons = pev->button` after the call.
- **View:** the bot writes `pev->v_angle` directly (`vision.cpp:137,205,290`) and `pev->angles = {-pitch/3, yaw}` (`vision.cpp:100-107`). Verify how your target engine treats `cmd.viewangles` vs `v_angle` for fake clients.
- **Buttons:** `pev->button` is zeroed each think (`resetMovement`, `navigate.cpp:1047-1053`) and rebuilt by the tasks.
- **Impulse:** only `impulse = 100` (flashlight) from `checkDarkness` (`vision.cpp:88,94`). Logo spraying uses a TE decal message, not impulse 201 (`support.cpp:79-126`).

---

## 4. User messages and the bot's own state

Registry: `m_wanted` name → enum, and `m_reverseMap` engine id → enum (`message.cpp:225-263`). Under metamod the ids are re-queried with `MUTIL_GetUserMsgID` if they were missed (plugin loaded late) (`:304-317`).

`start()` (`:265-293`):
- Unwanted message → ignored.
- Message addressed to a non-bot entity → ignored (so MSG_ONE to humans is dropped).
- `ent == null` (broadcast) → processed with `m_bot = null`.

The handler runs in `stop()` at MessageEnd.

| Message | HL wire args (as collected) | Handler | State updated |
|---|---|---|---|
| WeaponList | str name, byte ammo1Idx, byte ammo1Max, byte ammo2Idx, byte ammo2Max, byte slot, byte pos, byte id, byte flags | `:10-32` | Global `conf.getWeaponProp(id)` = classname / ammo indices and maxima / slot / pos / id / flags (32-entry table, persists across maps; filled from messages addressed to bots). |
| CurWeapon | byte state, byte id, byte clip | `:34-56` | If `id < 32`: `state != 0` → `m_currentWeapon`, `m_weaponType`; clip decreased on the current weapon → `m_timeLastFired`; `m_ammoInClip[id] = clip` (-1 for clipless weapons; id 255 from the observer state is rejected). |
| AmmoX | byte idx, byte count (total) | `:58-72` → AmmoPickup handler | `m_ammo[idx] = count` |
| AmmoPickup | byte idx, byte **added** | `:74-86` | `m_ammo[idx] = value`. Bug: HL sends a delta, not a total; the next AmmoX corrects it. |
| Damage | byte armor, byte health, long bits, coord ×3 | `:88-102` | `takeDamage(pev->dmg_inflictor, health, armor, bits)`: enemy acquisition, TK revenge, emotions, practice damage. |
| DeathMsg | byte killer, byte victim, str weapon | `:104-124` | `bots.handleDeath` + `telemetry.onKill` (skipped for world kills and suicides). |
| ScreenFade | short dur, short hold, short flags, byte r, g, b, a | `:126-140` | Full white with `a > 180` → `takeBlind(alpha)`. |
| TeamInfo | byte index, str team | `:162-176` | `client.team2 = resolveTeamIndex(team)` (empty → Unassigned; new strings get 0, 1, and all later teams get 1); `client.team = FFA ? index : team2`. |
| GameMode | byte mode | `:178-188` | `game.setTeamplayReported(mode != 0)`. |
| ScoreInfo | byte idx, short frags, short deaths, short class, short team | `:190-206` | For a bot: `m_kpdRatio = pev->frags / max(deaths,1)`, `m_deathCount`. |
| FlashBat | byte % | `:208-216` | `m_flashLevel` |
| ResetHUD | byte | `:218-223` | `bot->spawned()`; `gameState.setResetHUD(true)` (never read). |
| TextMsg, ShowMenu, SayText | — | id only (`:249-251`) | Used for **sending**: TextMsg for console/center prints (`engine.cpp:404-440`), ShowMenu for menus (`control.cpp:1966-2045`), SayText only in the unused `sendToChatLegacy`. |

Enum-only, never registered: `SendAudio`, `BotVoice`, `Fashlight` (sic), `inc/message.h:11-31`. Not hooked at all (they exist in HL): Health, Battery, WeapPickup, ItemPickup, HideWeapon, SetFOV, Flashlight, InitHUD, TeamScore, StatusText/Value, ScreenShake, Geiger, Train.

**Bot self-state model** (`inc/yapb.h:643-709`):

| Quantity | Source |
|---|---|
| Owned weapons | `pev->weapons` bitmask; bit = HL `WEAPON_*` id (`constant.h:153-172`: crowbar 1 … snark 15, suit 31). Masks: primary = MP5, shotgun, crossbow, RPG, gauss, egon; secondary = glock, python, hornet; throwables = grenade, snark, satchel, tripmine (`:334-350`). |
| Current weapon, clip | CurWeapon |
| Reserve ammo | `m_ammo[prop.ammo1]`; secondary pool `m_ammo[prop.ammo2]` (`combat.cpp:2919-2940`) |
| Health / armor | `pev->health` (clamped into `m_healthValue` each think) / `pev->armorvalue` |
| Longjump | physinfo `slj` |
| Flashlight | on = `pev->effects & EF_DIMLIGHT`; battery from FlashBat |
| Frags | `pev->frags`; deaths from ScoreInfo |
| Weapon selection | client command `issueCommand(prop.classname)`, e.g. `weapon_9mmhandgun` (`combat.cpp:2941-2949`) |

**Bot client commands:** `game.botCommand` → `prepareBotArgs` (`engine.cpp:492-549`) splits on `;`, handles a quoted second argument, fills `m_botArgs`, calls `MDLL_ClientCommand`, then clears. The `Cmd_*` hooks serve the args. For `say*` commands `Cmd_Args` drops argv0; for any other command `Cmd_Args` **includes** argv0 (`engine.h:321-326`).

---

## 5. Sound (hearing)

- **Hook:** only `pfnEmitSound` pre → `BotSounds::acquire` (`sounds.cpp:23-100`). Not hooked: EmitAmbientSound, PlaybackEvent (HL weapon fire is client-predicted events), PM step sounds.
- **Classification** by the first 11 characters of the sample (`:10-21`); every new prefix is inserted into the map via `operator[]`:

| Prefix | Class | Radius × volume | Duration |
|---|---|---|---|
| `player/pl_p`, `player/pl_f` | HitFall | 768 | 0.52 s |
| `items/gunpi`, `items/small`, `items/suitc`, `items/medsh` | Pickup | 768 | 0.45 s |
| `items/9mmcl` | Ammo | 512 | 0.25 s |
| `debris/bust` | Broke | 1024 | 2 s |
| `doors/doorm` | Door | 1024 | 3 s |

- Recorded into the **nearest alive client to the sound origin, whatever entity emitted it** (`client.noise` {pos, dist, last}).
- **Simulated noise**, per frame per alive client, from `updateClients` → `simulateNoise` (`:102-166`):
  - `IN_ATTACK` → 2048, 0.3 s (`IN_ATTACK2` ignored)
  - `IN_USE` or `IN_RELOAD` → 512, 0.5 s
  - Ladder (`MOVETYPE_FLY`, `|vz| > 50`) → 1024, 0.3 s
  - Otherwise, if `mp_footsteps` → `1280 * speed2d/260`, 0.3 s
  - A louder sound overrides an active one.
- **Consumer:** `Bot::updateHearing` (`botlib.cpp:2430-2552`): enemy clients with active noise within `noise.dist`, filtered by PAS (`pfnSetFatPAS` at eye position, shifted when ducking, `engine.cpp:392-402`; null on Xash3DLegacy) plus `checkVisibility` (edict leafnums / `pfnCheckVisibility`, `engine.cpp:362-390`).

---

## 6. Entity scanning

`Game::searchEntities` (by field/value via `pfnFindEntityByString`, or by sphere via `pfnFindEntityInSphere`, `engine.cpp:1103-1130`) **skips `FL_CLIENT` and `v.flags & EF_NODRAW`**. That second test actually checks `FL_NOTARGET`, see §13.

| What | Classnames / test | Where | Cadence |
|---|---|---|---|
| World and spawns | `worldspawn`; `info_player_start` + `info_player_deathmatch` (hidden; counted) | `engine.cpp:108-137, 144-157` | level init / Spawn hook |
| Custom spawn gate | `CustomSpawnPoint` (default `view_spawn`) via `hasEntityInGame` | `manager.cpp:868-873` | quota |
| Doors / buttons map flags | `func_door`, `func_door_rotating`; prefix `func_button` | `engine.cpp:1313-1323, 125-130` | level init |
| Breakables | `func_breakable`; `func_pushable` with `SF_PUSH_BREAKABLE`; `func_wall`. Requires `takedamage>0`, `impulse<=0`, not `FL_WORLDBRUSH`, not `SF_BREAK_TRIGGER_ONLY`, `1 <= health < yb_breakable_health_limit`, movetype PUSH/PUSHSTEP | `engine.cpp:1325-1353` | level init + Touch hook + 0.5 s scan (`botlib.cpp:151+`) |
| Active grenades | `grenade`, `monster_satchel` | `engine.cpp:1702-1722` | 0.25 s |
| "Interesting" entities | prefixes `weapon_`, `ammo_`, `weaponbox`, `grenade`; classname contains `item_`; `func_healthcharger*`, `func_recharge*`; `func_button*` if HasButtons; any `FL_MONSTER` if `yb_attack_monsters` (snarks, turrets, tripmines etc. only this generic way) | `engine.cpp:1724-1763` | 0.5 s, sphere of radius 9999999 around the worldspawn origin |
| Pickups | weapons (aliases `weapon_glock`/`python`/`mp5`, `config.cpp:925-928`); 13 ammo classes → weapon pools (`botlib.cpp:601-615`); `item_healthkit` (hp < 85), `item_battery` (armor < 90), `item_longjump`; ignored: `item_suit`, `item_antidote`, `item_security`; chargers (thresholds); other `item_*` if `pickup_custom_items` | `botlib.cpp:498-816` | per think |
| Tripmines | `monster_tripmine` origins registry | `manager.cpp:888-896` (slow frame); `botlib.cpp:418` | ~1 s |
| Own satchels | `monster_satchel` with `owner == bot` | `tasks.cpp:850-906, 1166-1170` | tasks |
| Thrown-grenade fix | `grenade` owned by the bot with a matching model → **writes `ent->v.velocity`** | `combat.cpp:2530-2549` | after throw |
| Lifts / doors / buttons | lift = `func_door*`, `func_plat`, `func_train`; buttons found by `target == targetname`; **`MDLL_Use(ent, bot)` direct use** (Xash uses `IN_USE` for doors) | `navigate.cpp:1249-1270, 1493-1500, 1612-1630, 3306-3339`; `tasks.cpp:1777` | nav |
| Graph helpers | `func_ladder`, `func_illusionary`; pickup classes marked as Goal nodes | `graph.cpp:2641-2746`, `analyze.cpp:366-389` | editor / analyzer |

Projectile model tests use `v.model.str(9)`, i.e. the model name with `"models/w_"` stripped, compared to `grenade.mdl`, `satchel.mdl`, `squeak.mdl` (`constant.h:355-358`, `botlib.cpp:89`). Not tracked at all: `rpg_rocket`, `hornet`, `crossbow_bolt`, `func_tank`, `trigger_*` (except the internal killer). Turret use exists only as a planning doc: the untracked `FEATURE-TURRETS-AND-BUTTONS.md`.

---

## 7. Game mode and GunGame

### 7.1 FFA vs teamplay
`applyGameModes` runs every slow frame, about once per second (`engine.cpp:1002-1046`):
- `yb_game_mode 0` → FFA; `1` → Teamplay; `-1` → teamplay if `mp_teamplay > 0` **or** the GameMode message reported it (this flag is never reset between maps).
- The flags are unset during the first second after load.

Team representation:
- `Client.team` (bot-facing team). In FFA it is forced to `playerIndex+1` every frame (`support.cpp:246-250`) and by TeamInfo.
- `Client.team2` (the "real" team) comes from TeamInfo team strings (model names), mapped to 0 or 1 in first-seen order. The cache is cleared per map (`engine.cpp:65`).
- `mp_teamlist` is used only when choosing a model at creation.

Effects of FFA: `numFriendsNear = 0` (`combat.cpp:27-30`); friendly-fire check off (`combat.cpp:790`); no team avoidance in navigation (`navigate.cpp:220`); no leaders (`manager.cpp:549`).

### 7.2 GunGame
- **Detection** (`engine.cpp:1035-1045`): `yb_force_gungame > 0`, or cvar named by custom `GunGameDetectCvar` (default `gg_enabled`, the serfreeman1337 AMXX plugin) exists and is `> 0`. Re-resolved every call. Sets or clears `GameFlags::GunGame`.
- **How the bot knows its GunGame weapon:** purely from `pev->weapons` each think, via `refreshForcedWeapon` (`botlib.cpp:475-496`). If exactly one owned weapon from the table is not melee and not throwable → `m_forcedWeaponId`, otherwise none. There is no plugin API or message.
- Level classifiers:
  - `isGunGameMeleeLevel`: only the crowbar (+ suit) bit (`:304-311`).
  - `isGunGameTripmineLevel`: tripmine bit and no primary weapon, glock allowed (`:313-320`).

Every place GunGame changes behavior:

| Effect | Where |
|---|---|
| Ignore `weapon_*`, `ammo_*`, `weaponbox*` pickups (items, kits, chargers, longjump still allowed) | `botlib.cpp:678-682` |
| Goal-node desire = 0 (goal nodes mark weapon spawns) | `navigate.cpp:35-36` |
| `selectBestWeapon`: tripmine level → hold the tripmine; otherwise force `m_forcedWeaponId` | `combat.cpp:2152-2170` |
| Weapon candidate filtering to the forced weapon | `combat.cpp:1606-1660` |
| "Grenade war" when there are no guns | `combat.cpp:2144-2146` |
| Leader bias: `getGunGameLeader()` = alive client with the highest `v.frags > 0`, cached for 1 s (`manager.cpp:1045-1071`); its distance² × `yb_gungame_leader_priority` in target selection | `combat.cpp:445-476` |
| Tripmine level: no attack or chase desire; no firing; proactive / rush planting; glock detonation of planted mines; faster corner-plant cadence | `botlib.cpp:1211, 1267-1275, 322-436`; `combat.cpp:1567-1571`; `navigate.cpp:2384-2385, 2458`; `tasks.cpp:1260` |
| Melee level: always hunt | `botlib.cpp:1327-1336` |
| Per-life rush-miner roll | `manager.cpp:1655-1665` |

---

## 8. Console variables and commands

### 8.1 Cvars
- Every bot cvar is prefixed `yb_` (`engine.cpp:646-652`).
- Registration flags: `FCVAR_EXTDLL` only (`Var::NoServer` is the default). `ReadOnly` adds `SERVER|SPONLY|PRINTABLEONLY`; `Password` adds `PROTECTED` (`:590-630`).
- **The description constructor defaults to `bounded=true`, `min 0`, `max 1`** (`engine.h:580`). Out-of-range values, or values starting with a letter, revert to the default within about 1 s. `*_max`/`*_min` pairs are swapped if inverted (`engine.cpp:654-702`).
- "Xash" = registered only on Xash.
- **Effective defaults differ** where `cfg/.../yapb.cfg` overrides: quota 0, attack_monsters 1, pickup_ammo_and_kits 1.

| Cvar | Default (bounds) | Meaning | Def. |
|---|---|---|---|
| yb_version | product version, read-only | build version (hidden from `cvars`) | linkage.cpp:10 |
| yb_game_mode | -1 (-1..1) | -1 auto, 0 FFA, 1 teamplay | engine.cpp:10 |
| yb_threadpool_workers | -1 (-1..hw) | worker threads (actually -1 → 1 thread) | :11 |
| yb_grenadier_mode | 0 | skip throw conditions | :12 |
| yb_ignore_enemies_after_spawn_time | 0 (unbounded) | ignore enemies N s after spawn | :13 |
| yb_breakable_health_limit | 500 (1..3000) | max breakable health to shoot | :14 |
| yb_force_gungame | 0 | force GunGame flag | :15 |
| yb_autovacate | 1 | keep slots for humans | manager.cpp:10 |
| yb_autovacate_keep_slots | 1 (1..8) | slots kept | :11 |
| yb_kick_after_player_connect | 1 | count spectators too when vacating | :12 |
| yb_quota | 9 (0..32) | bot count | :14 |
| yb_quota_mode | normal | normal / fill / match | :15 |
| yb_quota_match | 0 (0..32) | match ratio | :16 |
| yb_think_fps | 90 (24..90) | think rate (code clamps 30..90) | :17 |
| yb_think_fps_disable | 1, Xash | think every frame | :18 |
| yb_autokill_delay | 0 (0..90) | kill bots when no humans are alive | :20 |
| yb_join_after_player | 0 | only add bots when humans are present | :22 |
| yb_join_team | any | teamplay model / team | :23 |
| yb_botskin | "" | FFA model list | :24 |
| yb_join_delay | 5 (0..30) | delay after map start | :25 |
| yb_name_prefix | "" | name prefix | :26 |
| yb_difficulty | 3 (0..4) | difficulty | :28 |
| yb_difficulty_min / _max | -1 (-1..4) | random range at creation | :30-31 |
| yb_difficulty_auto | 0 | K:D balancing | :32 |
| yb_difficulty_auto_balance_interval | 30 (30..240) | balancing interval | :33 |
| yb_show_avatars | 0 | `*sid` infokey | :35 |
| yb_show_latency | 0 (0..2) | 1 = `*bot`, 2 = fake ping | :36 |
| yb_save_bots_names | 1 | keep names across changelevel | :38 |
| yb_preferred_personality | none | none / normal / careful / rusher | :40 |
| yb_language | en | config language | :42 |
| yb_rotate_bots | 0 | join/quit simulation | :44 |
| yb_rotate_stay_min / _max | 360 (120..7200) / 3600 (1800..14400) | stay time | :45-46 |
| yb_restricted_weapons | "" | **unused** | :48 |
| yb_debug | 0 (0..4) | debug output | botlib.cpp:10 |
| yb_debug_goal | -1 (-1..4096) | force goal node | :11 |
| yb_user_follow_percent / yb_user_max_followers | 20 (0..100) / 1 (0..8) | follow users | :12-13 |
| yb_jasonmode | 0 | crowbar only | :15 |
| yb_check_darkness | 1 | flashlight logic | :17 |
| yb_avoid_grenades | 1 | dodge grenades | :18 |
| yb_tkpunish | 1 (0..1) | revenge; value 2 = slay, unreachable because of bounds | :20 |
| yb_freeze_bots | 0 | no movement | :21 |
| yb_force_respawn | 1 | press to respawn | :22 |
| yb_spraypaints | 1 | logo decals | :23 |
| yb_destroy_breakables_around | 1 | shoot nearby breakables | :24 |
| yb_object_pickup_radius / yb_object_destroy_radius | 450 / 400 (64..1024) | search radii | :26-27 |
| yb_attack_monsters | 0 | FL_MONSTER targets | :29 |
| yb_pickup_custom_items / _ammo_and_kits / _best | 0 / 0 / 1 | pickup policy | :31-33 |
| yb_use_chargers, yb_charger_health_threshold, yb_charger_armor_threshold | 1, 60, 40 (0..100) | wall chargers | :35-37 |
| yb_chat / yb_chat_percent | 1 / 30 (0..100) | chat | chatlib.cpp:10-11 |
| yb_shoots_thru_walls | 2 (0..3) | wall shooting | combat.cpp:10 |
| yb_ignore_enemies, yb_check_enemy_rendering, yb_check_enemy_invincibility, yb_stab_close_enemies, yb_use_engine_pvs_check, yb_use_hitbox_enemy_targeting, yb_aim_trace_consider_glass | 0, 0, 0, 1, 0, 0, 0 | combat toggles | :11-17 |
| yb_gungame_leader_priority | 0.25 (0.1..1) | leader bias | :18 |
| yb_gauss_precharge, yb_use_gauss_jump, yb_use_satchel_jump | 1, 1, 1 | HL weapon tricks | :20-22 |
| yb_bind_menu_key | = | listen-server bind | config.cpp:18 |
| yb_ignore_cvars_on_changelevel | yb_quota,yb_autovacate | preserved across map change | :19 |
| yb_bots_roster | 1 | bots.json | :20 |
| yb_display_menu_text | 1, Xash | menu text on mobile | control.cpp:10 |
| yb_password / yb_password_key | "" / `_ybpw` | setinfo admin | :11-12 |
| yb_ping_base_min / _max | 5 / 20 (0..100) | fake ping base | fakeping.cpp:10-11 |
| yb_ping_count_real_players | 1 | average human ping | :12 |
| yb_ping_updater_interval | 1.25 (0.1..10) | recompute interval | :13 |
| yb_graph_fixcamp | 0 | PWF camp fix | graph.cpp:10 |
| yb_graph_url / yb_graph_url_upload | "" / "" | graph DB (disabled) | :11-12 |
| yb_graph_auto_save_count | 15 (0..4096) | editor autosave | :13 |
| yb_graph_draw_distance | 400 (64..3072) | editor draw distance | :14 |
| yb_graph_auto_collect_db | 1 | upload graphs (effectively dead) | :15 |
| yb_graph_analyze_auto_start, _auto_save | 1, 1 | analyzer | analyze.cpp:10-11 |
| yb_graph_analyze_distance | 64 (42..128) | node spacing | :12 |
| yb_graph_analyze_max_jump_height | 44 (44..64) | jump height | :13 |
| yb_graph_analyze_fps | 30 (25..99) | analyzer rate | :14 |
| yb_graph_analyze_clean_paths_on_finish, _optimize_nodes_on_finish, _mark_goals_on_finish | 1, 1, 1 | post-processing | :15-17 |
| yb_has_team_semiclip | 0 | no teammate avoidance | navigate.cpp:10 |
| yb_graph_slope_height | 24 (12..48) | jump-link threshold | :11 |
| yb_use_longjump | 1 | longjump usage | :12 |
| yb_path_heuristic_mode | 0 (0..4) | A* heuristic | planner.cpp:10 |
| yb_path_floyd_memory_limit | 6 (0..32) MB | Floyd matrix limit | :11 |
| yb_path_dijkstra_simple_distance, yb_path_astar_post_smooth, yb_path_randomize_on_round_start | 1, 0, 1 | planner | :12-14 |
| yb_display_welcome_text | 1 | welcome (dead, see §13) | support.cpp:10 |
| yb_enable_query_hook | 0 | A2S rewrite | :11 |
| yb_enable_fake_steamids | 0 | fake STEAM ids | :12 |
| yb_camping_allowed | 1 | camping | tasks.cpp:10 |
| yb_camping_time_min / _max | 5 (5..90) / 15 (15..120) | camp duration | :12-13 |
| yb_random_knife_attacks | 1 | random melee | :15 |
| yb_telemetry, yb_telemetry_port, yb_telemetry_hz | 0, 27070 (1024..65534), 20 (1..60) | observer export | telemetry.cpp:27-29 |
| yb_max_nodes_for_predict | 22 (15..256) | enemy path prediction | vision.cpp:10 |
| yb_whose_your_daddy | 0 | extra-hard mode (also disables the CmdStart seed) | :11 |

**Game cvars referenced (GameRef):**
- `mp_footsteps` (botlib.cpp:40), `mp_friendlyfire`, `sv_gravity` (combat.cpp:24-25), `sv_skycolor_r/g/b` (engine.cpp:17-19), `mp_timelimit`, `mp_flashlight` (vision.cpp:14).
- CS cvars **registered with "0" if missing:** `mp_limitteams`, `mp_autoteambalance`, `mp_roundtime`, `mp_freezetime` (manager.cpp:50-54).
- Read ad hoc through `ConVarRef`: `mp_teamplay`, `mp_teamlist`, `sv_maxspeed`, `mp_weaponstay`, `sv_use_steam_networking`, `host_hl25_extended_structs`, `developer`, `sys_timescale`, `sys_ticrate`, `sv_forcesimulating` (forced to 1 on Xash3DLegacy), the GunGame cvar, the parachute cvar (`sv_parachute`), `build`, `host_ver`.

### 8.2 Commands
Server commands `yb` and `yapb` go to `handleEngineCommands` (`control.cpp:2292`). They are ignored while `m_denyCommands` is set: before the first ServerActivate and after shutdown. Client `yb`/`yapb` commands are allowed for the listen host, or on dedicated servers for clients whose setinfo `<yb_password_key>` equals `yb_password` (`:2093-2142`). `yb help [cmd]` and a bare `yb` list the commands (`:1862-1913`). Command table: `control.cpp:2172-2286`.

| Command (aliases) | Format | Purpose |
|---|---|---|
| add / addbot / addhs | `[difficulty] [personality] [team] [model] [name]` | Queue a bot (`*` = any). `addhs` = difficulty 4 + rusher. The `model` argument is ignored. |
| addprofiled / addp | `[name]` | Roster bot. |
| kick / kickone / kickbot | `[team]` | Kick a random or team bot (quota −1). |
| removebots / kickbots / kickall | `[instant] [team]` | Kick all (quota 0). |
| kill / killbots / killall | `[team] [silent]` | Kill bots. |
| fill / fillserver | `team [count] [difficulty] [personality]` | Fill the server. |
| vote / votemap | `map_id` | Bots issue `votemap` (CS-only). |
| weapons / weaponmode | knife / pistol / … / standard | Only knife → `yb_jasonmode 1`; anything else → 0. |
| menu / botmenu | `[cmd]` | Main or command menu. |
| version / ver / about | | Build info. |
| graphmenu / wpmenu / wptmenu | | Graph editor menu. |
| list / listbots | | Bot list. |
| graph / g / w / wp / wpt / waypoint | `<sub>` | See below. |
| cvars | `[save \| save_map \| pattern \| defaults]` | Dump, write `conf/yapb.cfg` or `conf/maps/<map>.cfg`, or reset. |
| show_custom (hidden) | | Print custom.cfg values. |
| exec | `<bot index> <command>` | Run a client command as the bot (0-based bot index, not a userid). |

Graph subcommands (`control.cpp:437-476`):
- `on`/`off [display|auto|noclip|models]` (on also zeroes `mp_roundtime`/`freezetime`/`timelimit` and restores them on off)
- `menu`, `add`, `addbasic`, `save [nocheck|old]`, `load`, `erase iamsure`, `erase_training`
- `delete`, `check`, `cache`, `clean [all|nearest|idx]`, `setradius r [idx]`, `flags`, `teleport idx`
- `upload`, `stats`, `fileinfo`, `adjust_height off`, `refresh iamsure`
- `path_create[_in|_out|_both|_jump]`, `path_delete`, `path_set_autopath`, `path_clean [idx]`, `iterate_camp begin|end|next`
- `acquire_editor` / `release_editor` (dedicated only)
- On HLDS without an editor only `acquire_editor, upload, save, load, help, erase, erase_training, fileinfo, check` work (`:393-419`).

Other client-side hooks: `menuselect N` for bot menus (`:1946-1964`; menus defined at `:2351-2580`: Main, Features, Control, WeaponMode, Personality, Difficulty, TeamSelect, Commands, Graph pages 1/2, Radius, Type, Debug, Flag, CampDirections, AutoPath, Path, Kick 1-4). `say`/`say_team` are captured for replies (`manager.cpp:1842-1872`). Listen servers get `bind "=" "yb menu"` at every map load (`config.cpp:135-141`). Standalone mode also registers the server command `meta`.

---

## 9. Config and storage files

**Install root:** if the library path ends with `addons/yapb/bin`, the root is that directory's parent (absolute path). Otherwise, and always on Android or emscripten, it is `<gamedir>/addons/yapb` relative to the working directory (`storage.cpp:444-514`). Configs are read through the engine's `LoadFileForMe` (VFS path such as `addons/yapb/conf/...`, `config.cpp:783-798`). A comment line starts with `#`, `/` or `;` (`config.h:153-158`).

| File | Format / keys |
|---|---|
| `conf/yapb.cfg` | Console lines. First load (GameInit) = server commands. On each level init: `key value` split on spaces; cvars in `yb_ignore_cvars_on_changelevel` keep their live value (quota ≤ 0 is overwritten); `var.init` is updated for `revert()`; non-cvars go through as server commands. Missing file → auto `yb cvars save` (`config.cpp:42-150`). |
| `conf/custom.cfg` | `Key = Value`. Defaults (`config.cpp:698-707`): `AMXParachuteCvar=sv_parachute`, `CustomSpawnPoint=view_spawn`, `GunGameDetectCvar=gg_enabled`, `EnableFakeBotFeatures=no` (magic `i'm confident for what i'm doing` enables fake ping, avatars and the query hook on dedicated servers, `engine.cpp:1191-1241`), `DisableLogFile=no`, `CheckConnectivityHost=yapb.jeefo.net`, `DisableSpawnControl=no`. Re-read per map, but several consumers cache it in statics. |
| `conf/bots.json` | See §2.4. |
| `conf/lang/<lang>_names.cfg` | One name per line (31-char cap), shuffled. Shipped: en, ru. |
| `conf/lang/<lang>_chat.cfg` | Sections `[KILLED] [DEADCHAT] [WELCOME] [TEAMATTACK] [TEAMKILL] [UNKNOWN] [REPLIES]`. In REPLIES: `@KEY "K1","K2"` lines (upper-cased; spaces inside quotes kept for whole-word matching) followed by reply lines. Placeholders `%f` top fragger, `%m` map, `%r` "round" time left, `%s` chat sender, `%v` victim, `%d` "HLDM"/"Half-Life", `%t` teammate, `%e` enemy, `%g` graph author (`chatlib.cpp:274-328`). Missing file → `yb_chat 0`. |
| `conf/lang/<lang>_lang.cfg` | `[ORIGINAL]` block, then `[TRANSLATED]` block. Keyed by a hash of the alphanumerics only. Not loaded for `en`. Shipped: ru, de, chs, cht. |
| `conf/weapon.cfg` | `PersonalityNormal/Rusher/Careful = 14 row indices` (ascending desirability; rows as in `initWeapons`, `config.cpp:900-929`). |
| `conf/difficulty.cfg` | `Noob/Easy/Normal/Hard/Expert = minReact, maxReact, headshot%, seenThru%, heardThru%, maxRecoil, aimErrX, Y, Z` (9 values; code defaults at `config.cpp:594-612`; shipped values differ). |
| `conf/logos.cfg` | Decal names (`pfnDecalIndex`) from decals.wad. |
| `conf/avatars.cfg` | SteamID64 per line (skipped on Xash). |
| `conf/maps/<map>.cfg` | Exec'd via `exec` each level init. |

**Binary data** (`storage.cpp`, `inc/storage.h`, `inc/graph.h:102-145`). All little-endian raw structs. Header `StorageHeader` = 6 × int32 `{magic 0x59415042 (or 0x544f4255), version, options, length=nodeCount, compressed, uncompressed}`, then ULZ-compressed payload (`ext/crlib/crlib/ulz.h`: LZ77, 17-bit window, token = 3-bit literal run + 1 distance bit + 4-bit length, base-128 varints).

| File | Path | Version | Option bit | Payload |
|---|---|---|---|---|
| Graph | `data/graph/<map_lower>.graph` | 2 (newer only warns) | Graph 8 (+ Official 16, Recovered 32, Exten 64, Analyzed 128, Converted 256) | `Path[N]`, 220 B each: int32 number, flags; 3 × vec3 origin/start/end; float radius, light, display; 8 × `PathLink{vec3 velocity; int32 distance; uint16 flags; int16 index}`; `PathVis{uint16 stand, crouch}`. Then raw `ExtenHeader{char author[32]; int32 bspSize; char modified[32]}` (68 B). |
| Practice | `data/train/<map>.prc` | 2 | 1 | `{uint16 start, goal, team; int16 damage, value, index}` (12 B each). |
| Vistable | `data/train/<map>.vis` | 4 | 4 | N² bytes (2-bit stand/crouch fields at shift `(dest%4)*2`), then raw `PathVis[N]`. |
| Matrix | `data/train/<map>.pmx` | 2 | 2 | N² `{int16 index, dist}`. |
| PWF (legacy) | `data/pwf/<map>.pwf` | 7 | — | `PODGraphHeader{char "PODWAY!\0"; int32 ver; int32 count; char map[32]; char author[32]}` + `PODPath[N]` (204 B), converted on load (`graph.cpp:1688-1765`). |
| Log | `data/logs/yapb_L<ddmmyyyy>.txt` | | | text |

Load rules (`storage.cpp:12-217`): node count must be in 8..4096; non-graph files must match graph length and version exactly; a failed graph load is unlinked and retried (download if `yb_graph_url`, else PWF conversion), at most 2 retries, then the analyzer runs. Saving refuses graphs with fewer than 8 nodes and stamps node light levels.

---

## 10. Build, dependencies, platforms

- **Meson** is primary (`meson.build`):
  - C++17, no exceptions, no RTTI, LTO, `-Werror`, hidden visibility, 32-bit (`-m32`) by default on x86.
  - Options: `64bit`, `native`, `winxp`, `nosimd`, `static_linkent` (`meson_options.txt`).
  - Output `yapb.{so,dll,dylib}` with no `lib` prefix; suffix `_arm64` (aarch64), `_riscv64d`, `_amd64` (64bit).
  - Release Linux builds use the version script and `--gc-sections`. macOS min 10.9. Windows: delay-load user32/ws2_32, `vc/yapb.rc`, `i386pe.lds` for old mingw.
  - Git metadata goes into `version.build.h` from `inc/version.h.in` (fallback `inc/version.h`, version 4.5). `ninja package` runs `package.py` (zip, tar.xz, extras for many architectures; layout `addons/yapb/{bin,conf,data/{pwf,train,graph,logs}}`).
- **CMake** (`CMakeLists.txt`, Xash/Velaron style): Android, Vita, Emscripten (`.wasm` SIDE_MODULE); produces `libyapb.*` (the `lib` prefix is kept, e.g. `cmake-build-debug/libyapb.dylib`).
- `vc/` MSVC project (does not include telemetry.cpp).
- CI `.github/workflows/build.yml`: Linux x86 and amd64 on every push; Windows x86 (MSVC) and macOS arm64 on main and PRs.
- **Vendored** (`ext/VENDORED.md`), fork base upstream yapb `4967a22`:
  - `crlib` @`7efccdd` (containers, strings, files, http, threads, detour, ULZ, platform)
  - `linkage` @`a21540a` (stripped HLSDK, metamod and Xash `physint` headers; GPL / HLSDK licenses)
  - `rapidjson` @`24b5e7a`, headers only (bots.json and telemetry)
- **Targets:** HLDS/ReHLDS Linux/Windows x86 (metamod or standalone, detours x86 only); HL25 (light structs); Xash3D FWGS and legacy Xash (desktop amd64/arm64/riscv/ppc, Android, emscripten, Vita). **Only the `valve` gamedir.**
- **Loading:** metamod `plugins.ini` → `addons/yapb/bin/yapb.so|dll`; standalone `liblist.gam` `gamedll`.

---

## 11. Fake ping, scoreboard, query tricks, telemetry

- **Fake ping** (`fakeping.cpp`, `fakeping.h`):
  - Requires `HasFakePings` (not on Xash; game library found) and `yb_show_latency >= 2`. Dedicated servers force it back to 0 unless the magic custom key is set; listen servers force 0 → 2.
  - Each bot gets `m_pingBase = rand(min,max)`. Every `yb_ping_updater_interval` the value is recomputed **on the worker thread**: average human ping (`pfnGetPlayerStats`, only 0 < ping < 200 counted) ±20%, + base + `rand(diff+3, diff+6)`, clamped, then scaled ×0.25 (even entindex) or ×0.5 (odd).
  - Sent as a bitpacked `SVC_PINGS` (17) message: `[1 bit flag][5 bit player index][12 bit ping][7 bit loss]`… then a terminating 0 bit, MSG_ONE_UNRELIABLE.
  - Sent to a human on PutInServer and while that human holds `IN_SCORE` (UpdateClientData). Zeroed for all humans when a bot goes stale.
- **Scoreboard infokeys:** `*bot 1` (show_latency 1), `*sid` avatar (show_avatars).
- **Fake SteamID:** `GetPlayerAuthId` hook (§1.5).
- **Query hook** (`hooks.cpp:10-103`), dedicated x86 with `yb_enable_query_hook`: detours `sendto` (ws2_32, or the engine library's imported and libc `sendto` on Linux).
  - A2S_PLAYER `'D'`: rewrites the connection-time float for names that start with a bot's name, using a jk_botti-style random play time (`manager.cpp:978-987, 1337-1345`).
  - A2S_INFO `'I'` and legacy `'m'`: zeroes the bot-count byte.
- **Telemetry** (`telemetry.cpp`), inert unless `yb_telemetry 1`; non-blocking UDP to `127.0.0.1:<port>`:
  - `frame` snapshots at `hz`: players with e, n, bot, al, o, ya, v, hp, ap, tm; bots add w, diff, pers, ping, task, tstk, cn, goal, path (≤48, taken only if the path lock is free), en, le, leo, see, stuck, mtg, fear, agr, ms, ss.
  - Events: kill, task.
  - `graph_begin`/`nodes`(120)/`links`(400)/`end`, resent on map change, node-count change, or request.
  - Commands on port+1: `cmd <server command>` (**unauthenticated**), `graph`, `ping`.
- **`tools/observer`:** Python stdlib debugging UI. `bridge.py` (UDP → WebSocket, HTTP :8090, NDJSON recorder, BSP cache); `index.html` (2D viewer); `fake_server.py` (synthetic source); `bsp2json.py` (BSP v30 walls); `examples/soak_assert.py` (CI watchdog). Protocol documented in `tools/observer/README.md`.

---

## 12. Half-Life-specific changes relative to upstream CS yapb (in these files)

- Game flags reduced to HL, Xash, Mobility, Metamod, Teamplay, FreeForAll, GunGame, HasFakePings, HL25, Xash3DLegacy, HasStudioModels (`engine.h:36-48`). `valve`-only gate and HL library naming (`engine.cpp:785-920`).
- **No rounds:** a one-shot `roundStart` per map derived from `mp_timelimit`, mid = +90 s (`engine.cpp:1670-1700`). Respawn-driven `spawned()` from ResetHUD and DeathMsg, plus `yb_force_respawn` (`a5a428a`).
- No team or class menus. Team = player model (`mp_teamlist`, `yb_join_team`); FFA skins (`yb_botskin`). TeamInfo string cache; GameMode message; FFA `team = index+1`.
- `maxspeed` seeded from `sv_maxspeed` (`botlib.cpp:1703-1712`). Longjump via physinfo `slj`. HL spawn counting (start + deathmatch in one pool).
- HL weapon table, ids, masks and aliases (`config.cpp:900-951`, `constant.h:140-358`). HL ammo, item and charger pickups; HL noise samples; satchel and tripmine tracking.
- GunGame integration (§7.2); `bots.json` roster; telemetry and observer; a metamod cvar-registration static for macOS arm64 dladdr (`3f62b3a`, `engine.cpp:761-774`).
- Chat game name "HLDM"/"Half-Life"; `resetPathSearchType` always Fast; per-life HL tactic rolls in `newRound`.

CS leftovers still present:
- `setPlayerStartDrawModels` uses the CS models urban, terror and vip (a crash risk, §13).
- `mp_limitteams`, `mp_autoteambalance`, `mp_roundtime`, `mp_freezetime`.
- Infokeys `_vgui_menus` and `_ah`.
- `vote`/`votemap` bot commands.
- Menu enums TerroristSelect / CT / CZ.
- NodeFlag Rescue / NoHostage / TerroristOnly / CTOnly (kept for graph compatibility).
- The zombie/chicken "creature" mode.
- Chat content that mentions CS.

---

## 13. Bugs and quirks (do not port blindly)

**Crash-level / correctness**
1. `setPlayerStartDrawModels` calls `SetModel` with the unprecached CS models `models/player/urban/urban.mdl`, `terror`, `vip` (`engine.cpp:347-360`). It is reached by `yb graph on` and the editor menu (`control.cpp:509-515, 2329-2349`). On HLDS/ReHLDS this is likely a Host_Error "no precache".
2. `GetNewDLLFunctions` in standalone mode does `memcpy` and then `bzero` of the table (`linkage.cpp:884-886`). The game's new-DLL functions (GameShutdown etc.) are lost, and the hook calls `newapi.pfnOnFreeEntPrivateData` with no null check (`:903`).
3. `isCreature()` treats models starting with `zo` or `ch` as zombie-mod creatures (`botlib.cpp:2615-2621`). **`zombie` is a stock HL skin in the built-in FFA list** (`manager.cpp:1185`). Those bots get knife mode, no pickups, no chat and so on (the uses of `m_isCreature` are listed via grep).
4. `searchEntities` tests `ent->v.flags & EF_NODRAW`, which is really `FL_NOTARGET` (`engine.cpp:1107,1122`). Note that `enableDrawModels` depends on hidden spawn points *not* being filtered.
5. Live HL grenades use `models/grenade.mdl`, so `v.model.str(9)` gives `"enade.mdl"` and never matches `kExplosiveModelName` (`botlib.cpp:89`, `combat.cpp:2534`, `constant.h:355-358`). Grenade dodging and the velocity fix likely never trigger for grenades (satchels are fine). `chars(9)` also reads out of bounds on short brush-model names.
6. `resolveTeamIndex` puts every team after the second into index 1 (`message.cpp:152`): with 3+ teams, those teams count as allies. Team numbering is inconsistent: `add` uses the `mp_teamlist` order, while kick, kill and list use first-seen TeamInfo order.
7. FFA human counting depends on `team2`, which HL FFA never sets. It stays 0, or the previous occupant's value, since it is not reset on disconnect. An empty TeamInfo string makes it Unassigned and silently excludes humans from fill/match/join_after_player/autokill (`manager.cpp:1347-1361`, `support.cpp:235-269`). `team` and `team2` semantics are mixed in `handleDeath` and chat.
8. ClientConnect rejection still pushes a half-initialized Bot (`manager.cpp:1209-1215` → `:317`). The `execGameEntity` failure kick cannot find the bot, which is not registered yet (`:130-137`).
9. `storage.load` never writes `*outOptions` (`storage.cpp:165-167`), so the graph `Official` bit is never honored (`graph.cpp:1793`).
10. AmmoPickup is stored as a total, but HL sends a delta (`message.cpp:74-86`).
11. The `KeyValue` `material 7` marking is wiped by `m_checkedBreakables.zap()` at level init (`linkage.cpp:484-490` vs `engine.cpp:72,134`). `m_startEntity` is also stale during Spawn and KeyValue before ServerActivate.
12. Thread safety: fake-ping calculation calls `pfnGetPlayerStats` and iterates `m_bots` on a worker thread (`fakeping.cpp:42-100`); light levels read engine `model_t` from a worker (`graph.cpp:1556`).
13. `yb_tkpunish` is bounded 0..1, but 2 means slay, so that path is dead code (`botlib.cpp:20,1725`).
14. The ConVar constructor with a description is **bounded 0..1 by default**, so any new numeric cvar silently reverts.

**Semantic / infrastructure quirks**

15. `logger.fatal` **aborts the whole server** when the gamedir is not `valve`, even under metamod, and also when the library fails to load (`engine.cpp:847-859`).
16. `HasFakePings`, `HalfLife` and `HasStudioModels` are set only when the game library is found by the naming convention (`engine.cpp:864-918`).
17. `CmdStart` seed randomization exists only standalone. Under metamod, bots keep seed 0, i.e. deterministic spread (`linkage.cpp:445-459`).
18. `MDLL_*` bypasses metamod, so plugins do not see bot connects, put-in-server or commands.
19. Bots are kicked with `kick "<name>"` (fragile with duplicate or odd names); prefer `kick #userid`.
20. `m_needToSendWelcome` is never set true, so the welcome message is dead (`engine.cpp:186`, `support.cpp:128-201`). `gameState.setRoundOver` and `isResetHUD` are never used.
21. `yb_threadpool_workers -1` means 1 thread, not half the cores (`manager.cpp:1952-1955`).
22. Debug prints left in: `"offset = %d"` (`manager.cpp:1093`) and `"EMPTY!!!!!"` (`config.cpp:774`).
23. `custom.cfg` `CheckConnectivityHost =` with an empty value is ignored, so the default `yapb.jeefo.net` is probed at startup (`config.cpp:733-737`, `http.h:342-370`). Graph auto-collection targets `http:///collect/...`. On Windows a late metamod load (no GameInit) can abort if a download is attempted.
24. Config parsing: `yapb.cfg` second-pass `split(" ")` breaks on double spaces or quoted multi-word values (`config.cpp:83-92`). In the chat loader, the last `@KEY` block before `[UNKNOWN]` is never committed (`config.cpp:473-493`).
25. Docs vs code: yapb.cfg says `yb_difficulty -1 = random per bot`, but bounds revert it, and creation writes `rg(3,4)` back into the cvar. README says the quota default is 0 (the code default is 9). The difficulty.cfg header shows 5 values but 9 are required.
26. `yb add [model]` is ignored (`skin` is dropped, `manager.cpp:299`). `yb_restricted_weapons` is unused. `yb exec` takes a bot index, not a userid. `serverFill` uses `<=` with a `count-1` compensation.
27. Leaders are chosen once per map, and `m_isLeader` resets on every respawn (`manager.cpp:548-559, 1520`). `practice.update()` runs only at map start, so danger indices are never refreshed within a map (`:1914`).
28. The "half second" slow frame actually runs every 0.125–0.25 s (`engine.cpp:1049-1059`).
29. Sound noise is credited to the nearest alive player, not the emitter; `IN_ATTACK2` makes no noise (`sounds.cpp:43-74,115`).
30. `Bot::update` re-sets `FL_FAKECLIENT` after `markStale` cleared it (`botlib.cpp:1698`). The GameMode teamplay flag is sticky across maps.
31. The static `cvar_t reg_` trick (`engine.cpp:739-748,766-771`) only works because metamod copies cvars. Standalone registration needs stable addresses for `cvar_t`.
32. `executeCommands` forms `getClient(indexOfPlayer(nullptr))` for server-console calls (`control.cpp:1838`), which is undefined behavior. `MessageDispatcher::id()` returns 0 for an unknown message (map insert).
33. Direct engine-state writes a rewrite must reproduce or replace:
    - bot `pev->velocity` for jumps (`navigate.cpp:1112-1137`)
    - thrown grenade velocity (`combat.cpp:2539`)
    - `MDLL_Use` on doors, buttons and lifts
    - `pev->maxspeed`
    - `killer->v.frags++` + `ClientKill` (tkpunish)
    - `SetOrigin` for editor teleports
    - spawn-point render fields
34. Engine internals depended on:
    - `edict_t` fields `free`, `headnode`, `num_leafs`, `leafnums`, `pvPrivateData`
    - world `model_t` node, surface and lightmap layouts (software / hardware / HL25 variants, `engine.cpp:1418-1538`)
    - `studiohdr_t` hitboxes plus `pfnGetBonePosition` (a single failure disables `HasStudioModels`, `engine.cpp:1577-1662`)
    - `globals->pStringBase` string_t arithmetic (falls back to `pfnAllocString` on x64)
    - inline detours of `dlsym`, `GetProcAddress` and `sendto` (x86 only)
