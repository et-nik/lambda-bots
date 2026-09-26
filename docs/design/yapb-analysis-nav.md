# yapb-halflife analysis: navigation

> Original design note (in English), prepared during planning on 2026-09-26.
> A condensed version with the final decisions lives in the implementation plan; in case of conflict, the plan
> and later decisions in the repository take precedence. `file:line` references reflect the sources as of the note's date.

# YaPB-HL navigation stack: specification for the Rust rewrite

## 0. Scope, files, runtime pipeline

**Files read.** All paths below are under `/Users/nikita/Git/half-life/yapb-halflife/`. Line references later in this report use bare file names such as `graph.cpp:1944`.
- `/Users/nikita/Git/half-life/yapb-halflife/inc/graph.h`, `/Users/nikita/Git/half-life/yapb-halflife/src/graph.cpp`: node and link model, editor, persistence glue.
- `/Users/nikita/Git/half-life/yapb-halflife/inc/analyze.h`, `/Users/nikita/Git/half-life/yapb-halflife/src/analyze.cpp`: automatic graph generation.
- `/Users/nikita/Git/half-life/yapb-halflife/inc/planner.h`, `/Users/nikita/Git/half-life/yapb-halflife/src/planner.cpp`: A*, Floyd–Warshall, Dijkstra.
- `/Users/nikita/Git/half-life/yapb-halflife/src/navigate.cpp`: goal selection, path following, all movement handling.
- `/Users/nikita/Git/half-life/yapb-halflife/inc/vistable.h`, `/Users/nikita/Git/half-life/yapb-halflife/src/vistable.cpp`: visibility matrix.
- `/Users/nikita/Git/half-life/yapb-halflife/inc/practice.h`, `/Users/nikita/Git/half-life/yapb-halflife/src/practice.cpp`: danger and goal-value map.
- `/Users/nikita/Git/half-life/yapb-halflife/inc/storage.h`, `/Users/nikita/Git/half-life/yapb-halflife/src/storage.cpp`: file container, load and save.
- `/Users/nikita/Git/half-life/yapb-halflife/src/control.cpp`: `graph` commands and menus.
- `/Users/nikita/Git/half-life/yapb-halflife/inc/yapb.h`: `PathWalk` and the bot's navigation state.
- `/Users/nikita/Git/half-life/yapb-halflife/inc/constant.h`
- Supporting files:
  - `/Users/nikita/Git/half-life/yapb-halflife/src/botlib.cpp`: `logic`, `runMovement`, breakables, practice updates.
  - `/Users/nikita/Git/half-life/yapb-halflife/src/tasks.cpp`: task functions that drive navigation.
  - `/Users/nikita/Git/half-life/yapb-halflife/src/vision.cpp`: look direction.
  - `/Users/nikita/Git/half-life/yapb-halflife/src/combat.cpp`: `isNotSafeToMove` use, `calcThrow`/`calcToss`.
  - `/Users/nikita/Git/half-life/yapb-halflife/src/engine.cpp`, `/Users/nikita/Git/half-life/yapb-halflife/src/linkage.cpp`, `/Users/nikita/Git/half-life/yapb-halflife/src/manager.cpp`, `/Users/nikita/Git/half-life/yapb-halflife/src/message.cpp`
  - `/Users/nikita/Git/half-life/yapb-halflife/ext/crlib/crlib/{ulz,vector,binheap,random,array,timers}.h`

**Upstream comparison.** The repo's first commit (`1ac9326`) imports the whole tree, so git history does not show what changed from CS yapb. I diffed against the upstream CS clone at `/Users/nikita/Git/half-life/yapb`. `graph.h` is identical except for one added function. `analyze.cpp`, `planner.cpp`, `vistable.cpp` and `storage.cpp` differ only in the CS-removal hunks listed in §7.

**Per server frame** (`pfnStartFrame`, `linkage.cpp:364-410`), in order:
1. `graph.frame()`, only when an editor exists and the edit flag `On` is set (`:379-381`).
2. `analyzer.update()` (`:384`).
3. `game.slowFrame()`. Once per second this runs `graph.initLightLevels()` and `graph.initNarrowPlaces()` (`engine.cpp:1076-1079`) and kicks bots with `m_kickMeFromServer` set.
4. `vistab.rebuild()`, which is incremental (`:390`).
5. Bot quota and think.

**Per bot think** (up to `think_fps`=90, `manager.cpp:1747-1760`). `Bot::update` → `logic()` (`botlib.cpp:1814-1956`):
1. `resetMovement`, then `m_moveSpeed = maxspeed`.
2. Moved-distance sample every 0.2 s (`:1839-1847`).
3. `setConditions` at 10 Hz.
4. `executeTasks()`. The task calls `updateNavigation()` and `findPath()`.
5. `setAimDirection`, then `updateLookAngles`.
6. `m_moveAngles = angles(m_destOrigin - (origin + velocity*frameInterval))`, pitch negated (`:1882-1887`).
7. `overrideConditions`.
8. `moveToGoal()` if `m_moveToGoal`.
9. If `m_checkTerrain`: `checkBreakable(nullptr)`, `doPlayerAvoidance`, `checkTerrain`.
10. `checkFall`.
11. Darkness check, grenade avoidance, and the stuck-on-pickup check (`:1933-1937`).
12. `checkParachute`, `checkLongJump`, `checkGaussJumpTravel`.

Then `runMovement()` (`botlib.cpp:2399-2428`): `translateInput()` followed by `pfnRunPlayerMove(getRpmAngles(), m_moveSpeed, m_strafeSpeed, 0, buttons, impulse, msec)`. `getRpmAngles` returns `v_angle` when the bot is stuck, is approaching a ladder, or is in the Attack task. Otherwise it returns `m_moveAngles` (`botlib.cpp:2390-2397`). This is what lets movement follow the path while the view looks elsewhere.

**Threading.** All of these run through `worker.enqueue`: `findPath`→`syncFindPath`, Floyd rebuild, practice load/update, light levels, predicted index, and graph DB collect. The default is 1 worker thread (`manager.cpp:1928-1962`). With no pool (`YAPB_SINGLE_THREADED=1`, `sys_timescale`, or 0 workers) they run inline (`manager.h:209-215`). The worker writes `m_pathWalk` and `m_chosenGoalIndex` while the main thread reads them. That is a data race; do not port it.

---

## 1. Waypoint graph data model

### 1.1 Limits
- `kMaxNodes = 4096`, `kMaxNodeLinks = 8` (`graph.h:10-11`).
- A graph needs at least 8 nodes to be saved (`storage.cpp:232`), to load (`storage.cpp:118`), and for A* to run (`planner.cpp:197`).

### 1.2 Node `Path` (`graph.h:110-116`): 220 bytes, the raw on-disk record, little-endian

I checked the sizes and offsets with a compile-time `static_assert`. `Vector` is 3 packed floats.

| off | field | type | semantics |
|---|---|---|---|
| 0 | `number` | i32 | Must equal the array index. `erase` renumbers (`graph.cpp:951-963`). `checkNodes` validates it. |
| 4 | `flags` | i32 | Node flags, see §1.4. |
| 8 | `origin` | vec3 | Player-origin height: about 36 above the floor for standing nodes, lower for crouch nodes. Analyzer crouch nodes sit at floor+28 (`analyze.cpp:352`); the editor's crouch toggle moves the node −18 (`control.cpp:1698-1705`). |
| 20 | `start` | vec3 | Camp start view angles (pitch, yaw, 0). |
| 32 | `end` | vec3 | Camp end view angles. |
| 44 | `radius` | f32 | Wayzone, see §1.7. |
| 48 | `light` | f32 | Light level at origin. Written on save (`storage.cpp:244-249`), recomputed at +16z when players are present (`graph.cpp:1529-1559`). `kInvalidLightLevel` = 9999999. Used only by the flashlight logic (`vision.cpp:62-98`). |
| 52 | `display` | f32 | Editor redraw timestamp. Forced to 0 on save. |
| 56 | `links[8]` | PathLink×8 | Outgoing links, see §1.3. |
| 216 | `vis` | {u16 stand, u16 crouch} | Count of nodes visible from this node (vistable). Also stored as a trailer in the `.vis` file. |

### 1.3 Link `PathLink` (`graph.h:102-107`): 20 bytes
- `velocity` (vec3, off 0). The recorded run-up velocity for learned jump links (`graph.cpp:776`). Zero otherwise.
- `distance` (i32, off 12). The integer absolute value of a float distance (`addPath`, `graph.cpp:433`):
  - automatic links (editor, analyzer, ladders): 2-D distance (`graph.cpp:849,856,896`);
  - manual `pathCreate` links and learned jump links: 3-D distance (`graph.cpp:770,1086`).
  - Consequence: vertical ladder links have a cost of about 0.
- `flags` (u16, off 16). Link flags.
- `index` (i16, off 18). The target node, or −1 for a free slot.
- Links are directed. "Bidirectional" means two links.

`addPath` (`graph.cpp:420-463`):
1. Refuses self-links and duplicates.
2. Takes the first free slot.
3. If all 8 slots are full, overwrites the slot with the largest distance, even when the new link is longer.

### 1.4 Node flags (`graph.h:14-29`). All bits are kept for file compatibility.

| bit | name | set by | read by (navigation) |
|---|---|---|---|
| 0 | `Button` | Nothing in this code base (only imported files) | Triggers the door/button trace (`navigate.cpp:1249`); longjump exclusion (`:871`); accidentally matched in `selectBestNextNode` (`:2366`, see §8). |
| 1 | `Lift` | Editor flag menu item 3 (`control.cpp:1683`) | Lift state machine (`navigate.cpp:1235-1245`, `1447-1802`); reach radius 50 (`:1337`); longjump exclusion. |
| 2 | `Crouch` | Editor, when the editor is ducking (`graph.cpp:789`); analyzer, when the ceiling is <36 above the candidate (`analyze.cpp:341-345`) | Radius forced to 0; duck in `moveToGoal` (`navigate.cpp:996-1017`); reach radius 6 (`:1356`); g-cost ×1.5 (`planner.cpp:29,45,64`); vistable eye heights; longjump exclusion; nav aim. |
| 3 | `Crossing` | TOnly/CTOnly/Camp adds (`graph.cpp:802-813`) | Never read. |
| 4 | `Goal` | Editor goal add; `addBasic`; `markGoals`; flag menu item 5 | `m_goalPoints` (Goal tactic); reach radius 25 (`navigate.cpp:1340`); radius 0; `checkNodes` requires at least one; `syncFindPath` fallback destination (`:3438`). **In HL a Goal node is a weapon, ammo, item or charger spot.** |
| 5 | `Ladder` | Editor with movetype FLY; `addBasic` ladders via `m_isOnLadder` (`graph.cpp:793-798`) | Ladder handling throughout (§4.8); cost ×1.5; radius 0; `clearConnections` protection. |
| 6 | `Rescue` | Not set in HL | Only fills `m_rescuePoints`, which nothing uses. |
| 7 | `Camp` | Editor camp add / flag menu item 7 | Camp tactic list; radius 0; `camp_` look angles; `checkNodes` requires `end` to be set; `hide_` stays on camp nodes. |
| 8 | `NoHostage` | – | Unused in HL (upstream used it for hostage routing). |
| 9 | `DoubleJump` | Never set | HUD "JUMPHELP"; longjump exclusion. |
| 10 | `Narrow` | `initNarrowPlaces` (computed; buggy, see §1.14) | `cantSkipNode`, `setPathOrigin`, `isInNarrowPlace()` (aim, combat, camp tripmine), vision look-ahead. |
| 28 | `Sniper` | Flag menu item 4, camp nodes only (`graph.cpp:978`) | `m_sniperPoints`, which is always empty in practice (§8). |
| 29 | `TerroristOnly` ("TEAM1") | Editor Team-1 add / flag menu item 1 | Goal lists: defensive list for `Team::First`, offensive list for everyone else. |
| 30 | `CTOnly` ("TEAM2") | Editor Team-2 add / flag menu item 2 | Mirror of the above. |

### 1.5 Link flags (`graph.h:32-34`)
- The only link flag is `PathFlag::Jump` (bit 0). There is no longjump flag and no other special link type.
- Jump links come from:
  - learned jumps (JumpStart/JumpEnd, which carry a velocity);
  - `path_create_jump` (velocity 0; also sets the source node's radius to 0, `graph.cpp:1099-1109`);
  - analyzed graphs only: an implicit runtime jump when the next node is more than `graph_slope_height` (24) higher. The flag is not stored (`navigate.cpp:2547-2562`).
- Longjump is decided purely at runtime (§4.10).

### 1.6 Other enums (`graph.h`)
- `FindPath` {Fast, Optimal, Safe}
- `PathConnection` {Outgoing, Incoming, Bidirectional, Jumping}
- `GraphEdit` {On=2, Noclip=4, Auto=8}
- `LiftState` {None, LookingButtonOutside, WaitingFor, EnteringIn, WaitingForTeammates, LookingButtonInside, TravelingBy, Leaving}
- `NodeAddFlag` {Normal 0, TOnly 1, CTOnly 2, NoHostage 3 (ignored in HL), Rescue 4 (ignored), Camp 5, CampEnd 6, JumpStart 9, JumpEnd 10, Goal 100}
- `NotifySound` {Done "common/wpn_hudon.wav", Change "weapons/mine_activate.wav", Added "weapons/xbow_hit1.wav"} (`graph.cpp:1291-1302`)

### 1.7 Radius semantics

**Automatic radius**, `calculatePathRadius` (`graph.cpp:1432-1527`):
1. Radius is 0 if the node has Ladder, Goal, Camp or Crouch, if a jump is being learned, or if any linked node is a Ladder.
2. Otherwise, for `scan` = 32, 48, … 112, set `radius = scan`. Sample 18 directions starting at yaw 0. Each step adds `circleRadius` cumulatively (0, 20, 40, …), so the sampled yaws are uneven (0, 20, 60, 120, 200, 300, 60, …). For each direction:
   - (a) a zero-length head-hull trace at `origin+fwd*scan`. It never reports a hit, so the door branch (radius 0) is dead.
   - (b) a head-hull drop trace of `scan+60` from `origin+fwd*scan`. No ground → blocked.
   - (c) the same from `origin−fwd*scan`.
   - (d) a head-hull trace from `origin+fwd*scan` to +34z. A hit → blocked.
   - When blocked: `radius −= 16` and stop scanning.
3. At the end: `radius −= 16`, clamped to ≥ 0.
4. Possible results: 0 … 96.

**Manual radius:** menu values 0/8/16/32/48/64/80/96/128 (`control.cpp:1562`), or any value via `setradius`.

**Runtime uses:**
- Reach threshold = `max(radius², 48²)` for plain nodes (`navigate.cpp:1363`), so a radius under 48 does not change reach distance.
- Target randomization (§4.5): if radius >16 and the node is not narrow, take the nearest of 5 random points in a ±radius square. If radius >0, add a random offset instead.
- Occupancy test: `clamp(radius²·2, 98², 120²)` (`navigate.cpp:3293`).
- `cantSkipNode`: a zero radius means the node cannot be skipped.
- Vision look-ahead requires radius ≥16 (`vision.cpp:549`).

### 1.8 Camp angles
- A new camp node sets `start = end = v_angle` (`graph.cpp:811-819`).
- Re-adding Camp on an existing camp node sets `start = v_angle.get2d()` (`:633-645`).
- `CampEnd` sets `end` (`:647-660`).
- `camp_` alternates between start and end every 1–4 s. Pitch is zeroed and the look point is `pathOrigin + forward*500`. If a trace reaches less than 50% of that distance, it falls back to the random camp direction or the prediction (`tasks.cpp:482-518`).

### 1.9 In-memory indexes
- `m_paths` (`SmallArray<Path>`).
- `m_hashTable` buckets (`graph.h:189`, `graph.cpp:2797-2804`). They are filled **only at load** (`graph.cpp:1785-1790`); edits never update them.
  - Hash: `((int(axis)+8192) & 0x7F80) >> shift`, with shift 15 for x and 7 for y.
  - The x term is always 0 because the mask is narrower than 15 bits. In practice the key is `((y+8192)>>7)&0xFF`, which gives 128-unit strips along Y.
- `populateNodes` (`graph.cpp:1655-1686`) builds `m_terrorPoints`, `m_ctPoints`, `m_goalPoints`, `m_campPoints`, `m_sniperPoints` and `m_rescuePoints` with an **else-if chain**, so each node goes into one list only. It also builds `m_nodeNumbers` (all nodes).

### 1.10 Nearest-node queries
- `getNearestNoBuckets(origin, range, flags=-1)`: linear scan with an optional flag filter (`graph.cpp:499-518`).
- `getNearest` (`:527-567`) uses the bucket unless: there are fewer than 164 nodes, or `range > 256` and not infinite, or the bucket has fewer than 8 entries, or nothing was found.
- `getNearestInRadius` (`:569-600`).
- `getForAnalyzer` is the nearest node within `maxRange` (`:482-497`).
- `getFarest` returns the farthest node overall, but only if it is beyond `maxRange` (`:465-480`).
- `getEditorNearest` is within 50 of the editor (`:520-525`).
- `getFacingIndex` (`:1016-1059`) finds the node under the editor's crosshair.

### 1.11 Link creation and the reachability test

**`add(type, pos)`** (`graph.cpp:610-919`):
1. Kicks all bots and sets `m_hasChanged`.
2. Refuses the new node if another node is within 24 (analyzer) or 10 (editor). Also refuses at 4096 nodes.
3. Sets flags: Crouch if the analyzer says so or the editor is ducking; Ladder if the editor has movetype FLY or `m_isOnLadder`; type flags for TOnly/CTOnly/Camp/Goal.
4. **Ladder node:**
   - Links both ways to every other ladder node when the line trace is clear and |dx|<64, |dy|<64, |dz|<autopath (2-D distance).
   - Adds an outgoing link to every reachable non-ladder node.
   - Handles the nearest non-ladder node: both ways when analyzing; otherwise one-way links per a reachability check.
5. **Other node:** for every node, add new→n if reachable and n→new if reachable (each direction tested separately). In the editor, then run `clearConnections(new)`.
6. Always `calculatePathRadius`. During analysis, `markOptimized`.
7. Editor adds `m_lastNode`. Every `graph_auto_save_count` (15) additions trigger an autosave (`:750-763`).

**`isNodeReacheableEx(src, dst, maxHeight)`** (`graph.cpp:1944-2032`):
1. If `dst.z − src.z ≥ 45` → false. This 45 is fixed.
2. If distance > `m_autoPathDistance` (250 by default; the editor menu offers 0/100/…/250) → false.
3. Head-hull trace. If it hit a `func_illusionary` → false. Hull hits on anything else are ignored.
4. Line trace. It passes if clear or if it hit a door and the trace from the hit point to `dst` (ignoring the door) is clear.
5. If both endpoints are `CONTENTS_WATER` → true.
6. Walk from src toward dst in 10-unit steps, tracing 1000 down at each step. Fail if `height < lastHeight − maxHeight`, i.e. a step **up** larger than `maxHeight`. Drops are never rejected.
7. `isNodeReacheable` uses maxHeight 45. `…WithJump` uses `graph_analyze_max_jump_height` (default 44, range 44–64).

### 1.12 `clearConnections(index)`: redundant-link pruning (`graph.cpp:40-406`)
Used by editor add, by `graph clean`, and by the analyzer's finish step. It kicks bots.
1. Sort links by distance.
2. Compute each link's yaw relative to the closest link (0–360). Sort by that yaw.
3. **Pass 0:** for consecutive triples within 80°, remove the middle link if `(cur+prev2)·1.1/2 < prev`.
4. **Pass 1:** the same test across the wrap-around.
5. **Pass 2:** for consecutive pairs within 40°, always remove the roughly longer link (1.1× tolerance).
6. **Pass 3:** the pair test across the wrap-around.
7. Removal is applied in both directions. It skips jump links and ladder↔ladder links.

### 1.13 Erase and reset
- `erase(target)` (`graph.cpp:921-966`): drops every link to the node, decrements every higher `number` and link index, and removes the element.
- `resetPath` clears all incoming and outgoing links (`:1171-1208`).
- `erasePath` removes nearest→facing, or the reverse if that link is absent (`:1119-1169`).
- `unassignPath` also sets the edit flag `On` and `m_hasChanged` (`:2806-2816`).

### 1.14 Narrow places (`graph.cpp:1561-1653`)
- Skipped if the graph file version is ≥2 and the editor is not active, because the flags are persisted.
- Nodes with Camp or Goal are skipped, as are nodes with more than 4 links.
- For each link: `ang = angles((node−neighbor).normalize()*178)`. Trace from `origin+16z` to `-forward*178`, `right*178`, `-right*178` and `upward*178`, and count hits on worldspawn.
- **Bug:** those targets are absolute world positions near (0,0,0), not offsets from the node.
- Narrow if the hit count is >1.
- `saveGraphData` always recomputes it (`graph.cpp:1885-1889`).

### 1.15 `checkNodes(teleport, onlyPaths)` (`graph.cpp:2438-2617`)
Checks, in order:
- `number == index`. On mismatch it `break`s instead of returning false.
- A link index `> length` fails. It should be `≥`.
- Every node needs at least one outgoing link or one incoming link.
- Camp nodes need a non-empty `end`.
- No self-links or out-of-range links.
- If `!onlyPaths`, at least one Goal is required.
- Forward BFS from node 0 must reach every node, and reverse BFS must too, i.e. the graph must be **strongly connected**.

`planner.init` runs it with `(false, true)` to set `pathsCheckFailed`.

### 1.16 Persistence

**Container** (all yapb data files; `storage.cpp:12-304`). Structs are 24 bytes (header) and 68 bytes (exten), checked.
```
StorageHeader { i32 magic=0x59415042 ("BPAY"; also accepts 0x544F4255 "UBOT"), i32 version,
                i32 options, i32 length(=graph node count), i32 compressed, i32 uncompressed }
ULZ stream (compressed bytes)
[.vis only]   PathVis[length]              (uncompressed trailer)
[.graph + Exten option] ExtenHeader { char author[32]; i32 mapSize(bsp file size); char modified[32] }
```

**Payloads:**

| file | path | version | payload |
|---|---|---|---|
| `.graph` | `addons/yapb/data/graph/<map lowercase>.graph` | 2 | `Path[length]` (220 B each) |
| `.vis` | `.../data/train/<map>.vis` | 4 | `u8[n*n]`, row-major `[src*n+dst]`, 2 bits at `(dst%4)*2`: bit0 = stand blocked, bit1 = crouch blocked |
| `.pmx` | `.../data/train/<map>.pmx` | 2 | `{i16 nextHop, i16 dist}[n*n]` row-major |
| `.prc` | `.../data/train/<map>.prc` | 2 | `{u16 start, u16 goal, u16 team, i16 damage, i16 value, i16 index}[]` (12 B each) |
| `.pwf` | `.../data/pwf/<map>.pwf` | PODBot 7 | 80-byte header `{char[8] "PODWAY!\0", i32 ver, i32 count, char map[32], char author[32]}` + `PODPath[count]` (204 B: number, flags, origin, radius, csx, csy, cex, cey, i16 index[8], u16 conflags[8], vec3 velocity[8], i32 distance[8], PathVis) |

- Paths are built in `storage.cpp:341-388`. The root comes from the plugin library location with `bin/` stripped. Loads go through the engine VFS relative to the game dir (`getRunningPathVFS`, `:479-497`).
- **Options** (`storage.h:17-27`): Practice 1, Matrix 2, Vistable 4, Graph 8, Official 16, Recovered 32, Exten 64, Analyzed 128, Converted 256.
- **Graph versions:** v1 has no persisted Narrow flags. v2 adds them (`graph.cpp:1570-1576`). A version newer than 2 only prints a message. Non-graph files need an exact version match.

**ULZ** (`ulz.h`): LZ77, 128 KiB window.
- Each token byte: `run = token>>5`; 7 means varint extension. `len = (token&15)+4`; 19 means varint extension.
- The distance is u16, plus bit 16 taken from `token&16`.
- Varint: 7-bit groups, LSB first. Continuation byte = `128+(v&127)`, and continuation groups subtract 128 before shifting.
- The decoder writes 8-byte chunks, so buffers need 16 bytes of slack. Copies are forward and can overlap.

**Load flow** (`graph.cpp:1767-1836`, `storage.cpp:12-217`):
1. Open the `.graph`. If it is missing or bad: try a download (disabled in HL), then PWF conversion (`convertOldFormat`, `graph.cpp:1688-1765`, which saves immediately with option Converted). On a magic mismatch or bad count, the graph file is unlinked first. Retries are capped at 2.
2. Check: magic → count (8…4096; for non-graph files it must equal the node count) → version → option bit → ULZ decompress → exten (author/modified) → `.vis` trailer.
3. Then: buckets → `vistab.load()` (rebuild if needed) → `planner.init()` → `practice.load()` (async) → `populateNodes()` → warn on BSP size mismatch → warn on failed sanity check.
4. If the graph fails to load, run `analyzer.start()`.

**Save:** `saveGraphData` (`graph.cpp:1842-1892`) computes the options (adds Analyzed if the analyzer just finished; Official if the author starts with "YaPB"; Recovered in the odd no-editor listen-server case) and the author/modified strings, then recomputes narrow flags and calls `bstor.save`.

**Graph DB** (`graph.cpp:1304-1430`, `storage.cpp:42-63`):
- Download from `http://<graph_url>/graph/<map>.graph`.
- Upload is a POST to `http://<graph_url_upload>`. Collect goes to `…/collect/<hash>`. HTTPS is not supported.
- **In HL both URLs are empty** (`product.h:44-45`, `yapb.cfg:77-78`), so the database is effectively off. `graph_auto_collect_db` still fires but fails harmlessly.

---

## 2. Auto-analysis (`analyze.cpp`, a cs-ebot port)

**Start** (`:19-49`), run when the graph fails to load and `graph_analyze_auto_start`=1:
- Begin 3 s later.
- Silence graph messages and clear the `expanded[4096]` and `optimized[4096]` arrays.
- Bots cannot be added while it runs (`manager.cpp:441-444`).

**Seeding** (`addBasic`, `graph.cpp:2641-2712`):

1. **`func_ladder`** entities:
   - The node column sits at the ladder brush centre. The intended ±15-unit front/back offset is always zero (bug `^ nullptr`, `:2653`).
   - Trace down 1000 from the brush top. Place nodes from the floor +39 upward every 160 units while below `top−40`, then one node at `top+38`.
   - Every node gets the Ladder flag and is skipped if a node already exists within 50.
2. **`info_player_deathmatch`** and **`info_player_start`**: line trace 999 down, then place at floor +36 unless a node is within 50.
3. **Goal nodes**, placed the same way (`applyGoalPickupClasses`, `:2714-2746`):
   - every non-melee `weapon_*` classname plus its alias (`weapon_glock`, `weapon_python`, `weapon_mp5`);
   - 13 ammo classes;
   - `item_healthkit`, `item_battery`, `item_longjump`, `func_healthcharger`, `func_recharge`.

**Expansion** (`update`, `:51-130`):
- For each unexpanded node, mark it expanded and set the throttle.
- Call `flood(pos, target, r=graph_analyze_distance (64, range 42–128))` for dir=1…7: +x, −x, +y, −y, (+x,+128z), (−x,+128z), (+y,+128z). Case 8 (−y,+128z) is unreachable because the loop stops at `dir < 8`.

**`flood`** (`:307-355`):
1. `range *= 0.75` (48).
2. Head-hull trace from `pos` to `target+19z`. Abort on a hit unless it hit a breakable.
3. Abort if any node is within 48 of the endpoint.
4. Hull trace 999 down. Abort if there is no ground.
5. `nextPos = ground + 19z`.
6. Abort if a node is within 48, or if no node is within 250 (that node is the `target`).
7. Crouch test: line trace from `nextPos` up 36. Blocked → crouch, and the test position is `nextPos−18`.
8. Add if `(reachable(target→test) && reachable(test→target)) || (reachableWithJump both ways)`.
9. The position used is `nextPos`, or `nextPos−9` for crouch nodes.

Reachability is tested against the **nearest node within 250**, not the node that was expanded.

**Links during analysis:** `add()` creates all mutual reachability links against every node, which is O(n) traces per new node. No pruning happens until the end. The analyzer creates no stored jump links (§1.5).

**Special nodes:** the analyzer detects only Crouch, Ladder (seeding) and Goal (seeding plus `markGoals`). It never creates Camp, Sniper, Lift, Button, jump or team flags. Water is handled only through the water↔water rule in reachability. Doors are traversable in reachability if nothing is behind them. `func_illusionary` blocks, breakables pass.

**Time budget** (`setUpdateInterval`, `:357-363`):
- If `analyze_fps(30) + frametime ≤ 1/frametime` (the server runs faster than about 30 fps), then `nextUpdate = now + frametime·0.06`, which means one node expansion (7 floods) per frame.
- Otherwise there is **no throttle**, and the loop keeps expanding, including newly appended nodes, all within one frame (§8).
- The analyzer finishes once no expansion has happened for 2 s (`:126-129`).

**Finish** (`:139-171`):
1. `optimize()` (`:173-232`), if `optimize_nodes_on_finish`=1:
   - `cleanup()` (`:234-274`, buggy, §8).
   - Merge: for any node whose 8 links all go to not-yet-optimized nodes that `!cantSkipNode(i, j, skipVis=true)`, erase those 8 and add one node at their XY average with the first node's z.
   - If `clean_paths_on_finish`=1, run `clearConnections` on every node.
2. `markGoals()` (`:365-390`): OR the Goal flag onto every node whose `origin+(1,1,1)` lies inside a goal entity's **XY** bounds. Z is ignored.
3. Set analyzed, un-silence, `saveGraphData()` (Analyzed option), `loadGraphData()`, `vistab.startRebuild()`, hide spawn models, `cv_quota.revert()` (bots come back).

**Known limitations:**
- One-way drops are never linked (the dz≥45 rule, and the same test is required in both directions).
- No jump links, apart from the runtime slope rule.
- Lifts, trains, doors and buttons are not flagged.
- Ladders only get a centre column.
- It is O(n²) traces overall.
- Stale `m_isCrouch` (§8).
- The `cleanup` and merge bugs (§8).
- Goal marking is XY-only.

---

## 3. Path planning

**Structures** (`planner.h`):
- A per-bot `AStarAlgo`, sized to the node count (`manager.cpp:1313-1317`).
- A global `FloydWarshallAlgo` holding an n² `{i16 next, i16 dist}` matrix.
- A global `DijkstraAlgo` with a mutex.
- `PathWalk` capacity is `n/2 + 16` (`getMaxLength`, `planner.h:145`).

**`PathPlanner::init`** (`planner.cpp:467-491`):
- `pathsCheckFailed = !checkNodes(false, true)`.
- `memoryUse = 4·n²/1MiB` in integer MB. If it exceeds `path_floyd_memory_limit` (6), set `memoryLimitHit`, which uses Dijkstra. This happens at roughly 1355+ nodes. A failed sanity check forces Floyd even so.
- Floyd is loaded from `.pmx` or rebuilt asynchronously (`:294-338`): init −1/32767, fill links (raw `link.distance`, no penalties), diagonal 0, triple loop, save.

**Bot request:** `findPath` enqueues `syncFindPath` (`navigate.cpp:3416-3508`):
- `tryLock` per bot. A request made while one is already running is **silently dropped**.
- Bad source: use the nearest node within 256 via `changeNodeIndex`. Bad or identical destination: nearest Goal node, then a random node.
- If `pathsCheckFailed`, use `findShortestPath` (Floyd, or Dijkstra when the memory limit is hit).
- Otherwise pick the g/h functions by path type, clear the path, start `m_repathTimer`(0.5 s), and set `m_chosenGoalIndex = src` (it means "route start", §8).
- A* outcomes:
  - Success: reverse the list, giving `[src … dst]`.
  - InternalError: `m_kickMeFromServer`; the bot is kicked on the next slow frame (`manager.cpp:907-919`).
  - Failed: fall back to `findShortestPath`.

**A\*** (`planner.cpp:196-292`):
- Reset all routes (`clearRoute`, which resizes then clears).
- `src.g = gcalc(team, src, −1)`, `f = g + h`.
- The heap is a min-heap **keyed by g** (`:210,287`), not f.
- Loop:
  - Pop.
  - Guard: if the open list is ≥ `n/2+15` entries → InternalError and `setPathsCheckFailed(true)` (`:230-237`).
  - If the popped node is the destination, walk parents to build the path; optionally post-smooth.
  - Skip if the node is not Open; otherwise mark it Closed.
  - For each link: `g = cur.g + gcalc(team, child, cur)·rsRandomizer`; `f = g+h`, or `ceil(g+h+0.5)` in non-SIMD builds.
  - If the child is New or `child.f > f` (which can re-open Closed nodes): set parent, Open, push with key g.
- Because the key is g, **h never changes the result**. The algorithm is effectively Dijkstra with early exit.
- `rsRandomizer = rg(0.5, team·2)` during the first 2 s after "round start". In HL that happens once, at map start. It is a single scalar applied to every edge, so it is effectively a no-op.

**Heuristics** (`hfunctionPathDist`, `:73-111`; `path_heuristic_mode`):
- 0 = Chebyshev (the default)
- 1 = Manhattan
- 2 = none
- 3 = 3-D diagonal
- 4 = 10·Euclidean
- `hfunctionNone` = the above ÷ 1280 (`:113-115`).

**G functions:**
- `gfunctionPathDist` (Fast): link distance, ×1.5 if the child is Crouch or Ladder. 65535 if there is no link. 0 at the source.
- `gfunctionKillsDist` (Optimal): `damage(child)+teamHighest + Σ neighbour damage`, ×1.5 for crouch. **It has no distance term.**
- `gfunctionKills` (Safe): the same without teamHighest. It is also applied at the source.
- **HL always uses Fast.** `resetPathSearchType` sets Fast unconditionally (`manager.cpp:1763-1766`), where upstream chose by personality and morale. The danger-aware g functions are therefore dead in HL.

**Post-smoothing** (`path_astar_post_smooth`, default 0; `:176-194`) uses `cantSkipNode(a,b)` (`:130-174`). A node cannot be skipped if any of these hold:
- either radius is 0;
- the pair is not visible both ways (unless skipped);
- |dz| > 17;
- either node is Narrow;
- the distance is over 400;
- the distance is "too close" (a sqrt bug, §8);
- either node has any jump link.

**Floyd `find`** (`:364-384`): emit `src`, then follow next-hops. The callback returning false stops the walk early (this is how "first visible node" searches work). A next-hop of −1 means failure.

**Dijkstra** (`:398-453`): lazy-deletion heap. Rebuilds the path from the parents. On failure the path is `[dst]` alone and the function returns false.

**`planner.dist`** (`:508-525`):
- Returns ∞ if either index is invalid, and 1 if src==dst.
- When the memory limit is hit: 2-D distance if `path_dijkstra_simple_distance`=1, otherwise the full Dijkstra distance.
- Otherwise the Floyd distance.
- `preciseDistance` (`:527-533`) always runs the full search and has **no bounds check**.

**No path at all:** `findShortestPath` clears `m_prevGoalIndex` and the task data. The next frame's `normal_` picks a new goal and re-plans. `findValidNode`'s timeout eventually adds danger and picks a new goal.

---

## 4. Movement execution

### 4.1 Goal choice (navigation-relevant)
`findBestGoal` (`navigate.cpp:14-65`) picks a tactic from these desires:

| desire | value |
|---|---|
| goal | rand100 + aggression·100 (0 under GunGame) |
| forward | rand100 + aggression·100 |
| back-off | rand100 + fear·100 |
| camp | (rand100 + fear·100)·0.3, only with camp guns (MP5, crossbow, gauss, egon) |

`findGoalPost` (`:67-124`) then chooses the list:
- Defensive → own team's points; Offensive → the other team's points; Camp → sniper points (if the bot uses a sniper weapon) or camp points; Goal → goal points.
- `postProcessGoals` (`:126-181`) draws 4 random candidates, rejecting the previous goal, `prevNodes[0]`, goal history, duplicates and occupied nodes.
- Rushers pick one of the 4 at random. Everyone else sorts by `practice.getValue(team, current, goal)`.
- If nothing is found, a random node.

**In HL FFA:** the team value is the entity index, so every bot takes the `default` branch. HL graphs usually have no team points, so Defensive and Offensive tactics end up at `graph.random()`. The Goal tactic goes to item spots.

`normal_` (`tasks.cpp:17-118`):
- Reached the goal → complete the task.
- No active goal → destination is the task data or `findBestGoal()`; if that does not exist, `getFarest(origin, 1024)`; `ensureCurrentNodeIndex`; `findPath(cur, dest, m_pathType)`.
- `debug_goal` forces every bot to one node.

### 4.2 Path list
- `PathWalk` (`yapb.h:137-208`) is a cursor over an int array.
- `first()` is the node currently being walked to, which equals `m_currentNodeIndex` after an advance. `next()` and `nextX2()` look ahead. `last()` is the goal.
- `hasNext()` was fixed in HL to `length()>1` (commit `e6111c1`); upstream subtracted the cursor twice.
- `hasActiveGoal` (`navigate.cpp:183-197`): task data == current node pushes onto the goal history; otherwise the goal must equal `last()`.

### 4.3 `updateNavigation` (`navigate.cpp:1090-1445`)
1. If there is no current node: `findValidNode()` and `setPathOrigin()`.
2. `m_destOrigin = m_pathOrigin`.
3. Jump travel (§4.9), ladder (§4.8), lift (§4.14) and door (§4.12) handling.
4. **Reach threshold** (squared), first match wins:

   | condition | threshold |
   |---|---|
   | Lift node | 50² |
   | ducking, or Goal node | 25² |
   | on a ladder | 15² |
   | Ladder node | 6² |
   | Jump travel | 0, or 8² if `vel.z>16` |
   | Crouch node | 6² |
   | the `debug_goal` node | 0 |
   | otherwise | `max(radius², 48²)` |

   Overrides applied afterwards:
   - Any flagged outgoing link on the current node → 0 (`pathHasFlags`).
   - `m_lostReachableNodeTimer` running → 0.
   - `m_repathTimer` running and no flagged links → 48².
   - Precise placement: if the threshold is under 16² and the bot is within 30, compute `predict = dist(pathOrigin, origin+vel·dt)`. If `predict ≥ current distance` (moving away) or `predict ≤ threshold`, mark the node reached.
5. **Occupied final node:** if `next()==last()` and it is occupied, clear the task data, current node and chosen goal, and return true (forces a re-plan).
6. **Longjump flight:** while `m_longJumpFlightTime` is active and the bot is airborne, use 2-D distance with the threshold at least 50².
7. **Reached** (3-D distance):
   - If this is the goal node: `practice.setValue(team, m_chosenGoalIndex, cur, clamp(v + health/2 + goalValue/2, ±2040))`, `ignoreCollision`, return true.
   - If the path is empty: return false.
   - Otherwise `advanceMovement()`.

### 4.4 `advanceMovement` (`:2464-2629`)
1. `findValidNode()`. Shift the path. Clear the travel flags.
2. If this is a mid-route node:
   - `selectBestNextNode()`: swap `first` for an unoccupied alternative among the previous node's links that connect to both `next` and `prev` (conditions at `:2330-2377`).
   - Set `m_minSpeed=maxspeed`.
   - **Danger camp:** only in the Normal task, only after map start+95 s (`roundMid+5`), at least 60 s since the last camp, not a Rusher.
     - `kills = damage(team, next, next) / teamHighest`, divided by 3 (Normal personality) or 2 (others).
     - If `baseAggression < kills`, the bot has a primary weapon, and a 15% roll passes: Camp task for `rand(camp_time_min, camp_time_max)+4` seconds plus MoveToPosition to `findDefendNode(next)`.
3. Load the travel flags and `m_desiredVelocity` from the link cur→dest.
4. Analyzed-graph slope jump (§1.5).
5. Look ahead to `dest→next`:
   - If it is a jump longer than 96, set `m_jumpSequence`.
   - If it is a jump longer than 145 (or rises more than 32 over longer than 125) and the bot is not holding a pistol or reloading and sees no enemy: switch to the **crowbar**. This is a CS knife-speed habit and pointless in HL.
6. If dest is a Ladder node and another bot is on it: Pause 3 s and return without advancing.
7. `checkCornerTripminePlant(dest)` (§4.22), `changeNodeIndex(dest)`, then `setPathOrigin()`, `m_navTimeset=now`.

`changeNodeIndex` (`:2007-2026`) shifts the history and sets `m_path`, `m_pathFlags`, `m_pathOrigin=origin`, `m_navTimeset` and `m_collideTime`. `m_previousNodes[1]` is never written (§8).

### 4.5 Target randomization, `setPathOrigin` (`:2631-2681`)
- Radius >16 and not narrow: if the path has a next node, generate 5 points uniform in `pathOrigin ± radius` (XY) and take the one **nearest the bot**. Otherwise use a random offset.
- Radius >0: offset along `forward(bodyPitch, bodyYaw ± 90° random) * rand(0, radius)`.
- On a ladder: if the line from the bot's feet to the target is blocked, move the target to halfway plus 32z.

### 4.6 Steering and look direction
- **Movement** always heads toward `m_destOrigin` through `m_moveAngles`, compensated by velocity (`botlib.cpp:1882-1887`).
- **`translateInput`** (`navigate.cpp:1056-1088`):
  - duck timer;
  - after any jump, hold duck while airborne for 0.85 s (duck-jump);
  - map the signs of move and strafe speed to `IN_FORWARD`/`IN_BACK`/`IN_MOVELEFT`/`IN_MOVERIGHT`.
- **Nav aim** (`vision.cpp:512-629`). Defaults to `m_destOrigin + view_ofs`, then:
  - **Danger look:** enemies alive, no sighting for 4 s, not predicting. Look at the practice danger node `getIndex(team, cur, cur)` if it is visible both ways, is not a crouch node, and is more than 240 away. Sets the Danger aim flag.
  - **Look-ahead:** allowed when not moving vertically, on the floor, standing, within 384 of the target, radius ≥16, and both the current and next nodes have **no flags at all** with |Δz| < 8. Look at `nextX2`, or `next`, if visible both ways and not narrow.
  - **Ladders:** look at the next ladder node if it is at least 26 higher and within 96.
  - **"Looking at a wall":** trace 80 units beyond the target along prev→dest. If that hits worldspawn, look back at the previous node.
  - Keep eye level when looking at the target.
  - Briefly look at the last victim.
- **Turning** (`vision.cpp:113-209`): spring-damper (stiffness 200, damping 25, acceleration cap 3000). While navigating it avoids reverse-facing yaw flips. Noob bots use a separate model.

### 4.7 Crouch nodes
`moveToGoal` (`:996-1017`): press duck if a head-hull trace from `node+5` to `node+72` is blocked. The reach threshold is 6², or 25² while ducking.

### 4.8 Ladders
- Entering `updateNavigation` (`:1160-1233`): find a `func_ladder` within 96. The target is nudged ±1 along the plane normal of a trace that ignores the ladder itself, which has no practical effect.
- Approaching the first ladder node (previous node not a ladder, within 64): speed 0.4·maxspeed, release duck unless on the ladder, and `m_approachingLadderTimer(2·dt)` so the run command uses **view** angles.
- On the ground and not ducking, speed = `clamp(distance, 160, maxspeed)`.
- A team-mate on a ladder above or below within 192 triggers a re-plan to `prevNodes[2]`. That index is always −1, so this is dead (§8).
- Advancing onto an occupied ladder pauses 3 s (§4.4).
- Leaving (`moveToGoal`, `:1019-1026`): the node is not a ladder, the previous one was, the bot is on the ladder and the node is below → `IN_JUMP`.
- Climbing direction comes from the 3-D `m_moveAngles` fed to the run command.

### 4.9 Jumps (`updateNavigation`, `:1102-1158`)
When the travel flags include Jump, the jump is not yet done, and the bot is on the floor or ladder:
- If the link has a stored velocity: `pev->velocity = velocity` (full 3-D; this sets velocity directly, i.e. a cheat).
- Otherwise, if the graph is analyzed:
  - `feet = origin + mins`; the target is the node's floor (z −36, or −18 for crouch nodes).
  - `v = calcThrow(feet, target)`, or `calcToss` if |v|² < 100 (`combat.cpp:2425-2528`; gravity is `sv_gravity·0.55`).
  - `v *= 1.45`; apply horizontal velocity only, with `z = 0`.
- Always press `IN_JUMP`, then mark done, skip the terrain check, clear the Jump flag.
- If `m_jumpSequence` is set, Pause 0.75–1.25 s.
- After landing with the crowbar out, re-select the best weapon.
- Manual `path_create_jump` links on non-analyzed graphs only press jump, using the bot's current momentum.

### 4.10 Longjump (HL-only; commits b3f4f69, e6111c1, 6810857, f459c14, 88caaa6)

**Engine mechanics (HL `PM_Jump`):** duck and jump must both be newly pressed in the same command. That gives about 560 u/s horizontal along the command's forward vector and `sqrt(2·800·56)` vertical, roughly a 420–445 unit flight.
- The module flag comes from physinfo `slj=="1"`, polled every 0.5 s (`botlib.cpp:1668-1672`).
- It is reset on death (`manager.cpp:1680`) and set when the bot touches `item_longjump` (`tasks.cpp:1730-1732`).
- The pickup no longer depends on `pickup_ammo_and_kits`.

**`checkLongJump`** (`navigate.cpp:810-990`). Every gate must pass:
1. `use_longjump` is on, the bot has the module, it is not a creature.
2. The cooldown has passed and the bot is not stuck.
3. No gauss release in progress; not approaching a ladder.
4. Neither jump nor duck was held last command or is held this command.
5. The duck timer is inactive and the last jump was more than 1 s ago.
6. On the floor, not on a ladder, not in water, not ducking.
7. Not avoiding a grenade and not using a grenade.
8. Speed2D ≥ 150.
9. **Attack branch** (`checkAttackLongJump`, `:765-808`), tried next:
   - Task Attack, enemy visible, 400–750 units away in 2-D, Δz between −64 and +40.
   - Not backpedalling; `health·aggression ≥ 30`.
   - View forward within 0.95 cos of the enemy direction.
   - Launch along the view direction.
10. **Path branch:**
    - No enemy within 400; `m_moveToGoal` set; task is Normal, Hunt, MoveToPosition or SeekCover (so run angles equal move angles).
    - `moveSpeed ≥ maxspeed−10` and no strafing.
    - No Jump travel flag, and the current node has none of Crouch/Ladder/Lift/Button/DoubleJump.
    - The path is non-empty.
    - `destOrigin == pathOrigin`, i.e. not the frame of a node advance.
    - The target is at least 50 away in 2-D.
    - `dir = normalize2d(dest − origin)`; the velocity direction is within 0.93 of `dir`.
    - The target's Δz is within +40 / −64.
    - **Runway:** the points are the bot's origin, then the current node's raw origin (only if it is more than 24 away and its direction dot `dir` > 0.5), then up to 10 further path nodes.
      - The chain stops at a bad-flag node, a Δz outside +40/−64, or a link with any flag.
      - Needs runway ≥ 400 and at least 3 points.
      - Corridor direction = `normalize(last − first)`; `dir · corridor ≥ 0.92`; every node-to-node segment `· corridor ≥ 0.94`.
      - `maxDrop` = the largest drop along the chain.
11. **`tryLongJumpAlong(dir, maxDrop)`** (`:727-763`):
    - Throttle 0.5 s.
    - Head-hull trace from `origin+17z` to `origin + dir·250 + 50z` must be clear.
    - `!isDeadlyMove(origin + dir·445)`; if `maxDrop > 16`, also `!isDeadlyMove(origin + dir·520)`.
    - Then press `IN_DUCK|IN_JUMP`, cooldown `rand(0.9, 1.4)` s, flight window 1 s.

**During flight:** reach checks are relaxed (§4.3 step 6). The parachute check is skipped (`botlib.cpp:1632-1635`).

### 4.11 Water
- `moveToGoal` (`:1029-1045`): `IN_FORWARD`, or `IN_BACK` if the target is more than 90° off the view. `IN_DUCK` if move pitch > 60, `IN_JUMP` if < −60. The 3-D move angles steer.
- `isReachableNode` returns true when `waterlevel` is 2 or 3.
- Stuck probing in water allows Strafe and Jump only.

### 4.12 Doors (`:1248-1331`)
Active when the map has doors or the node has the Button flag.
1. Line trace from the bot to its target hits `func_door` or `func_door_rotating` and there is no lift state.
2. Door within 56: `ignoreCollision`. With a 50% roll every 1.5 s, call `MDLL_Use(door, bot)` (Xash3D presses `IN_USE` instead).
3. Face the door (clear the LastEnemy and PredictPath aim flags).
4. When the push timer has elapsed, `lookupButton(door.targetname, blind)` sets it as a Button pickup.
5. Speed below 10 and the retry timer elapsed:
   - optional Pause 0.5 s;
   - `++tryOpenDoor`;
   - tries 2–3: look for a penetrable enemy within 256 and set it as the enemy;
   - after more than 4 tries: go back to the previous node, lock out for 3 s.
6. `isBlockedForward` ignores doors (`:2692-2702`).

### 4.13 Buttons
- `lookupButton(target, blind)` (`:3306-3339`): the nearest entity whose `target` equals the given name. The non-blind version requires a clear line.
- `pickupItem_` Button case (`tasks.cpp:1749-1786`): within 90 (24 on lifts), stop, and when facing within 10° call `MDLL_Use(button)`, push time +3 s.
- Buttons are only pressed through door and lift logic, never as roaming pickups.

### 4.14 Lifts: `func_door*`, `func_plat`, `func_train` (`:1447-1802`)
The state machine runs on Lift nodes:
- **Closed door ahead:** set LookingButtonOutside (7 s).
- **Platform under the node** (trace node → 50 below). If it is a func type with zero z-velocity and the bot is within 70 vertically, set EnteringIn (5 s) and `liftTravelPos = pathOrigin`. If already TravelingBy, set Leaving.
- **No lift at the node** and the next node is a Lift: remember the entity, set LookingButtonOutside (15 s).
- **EnteringIn:** walk to `liftTravelPos`. Within 22: wait. If following bots exist: WaitingForTeammates (8 s). Otherwise: LookingButtonInside (10 s).
- **LookingButtonInside:** once standing on the stopped lift, use the button. Visible → Button pickup; not visible → `MDLL_Use`, which works remotely.
- **Moving lift under the bot:** TravelingBy (14 s); the destination is `liftTravelPos` at the bot's own z.
- **LookingButtonOutside:**
  - within 8 s of a button press: wait at the previous node;
  - lift in use by a team-mate: wait;
  - otherwise make the button the pickup and set WaitingFor (20 s).
- **WaitingFor:** wait at the previous node (the `prevNodes[1]` fallback is dead).
- **Fell into the shaft** (more than 50 below both the node and the previous node): reset, `findNextBestNode`, re-plan to `prevNodes[2]` (dead).
- `updateLiftStates` handles Leaving → None and the overall timeout (return to the previous node or `findNextBestNode`).
- The `wait()` helper zeroes movement, keeps the nav timer fresh, and ignores collision.

### 4.15 Breakables
- `checkBreakable` (`botlib.cpp:119-149`) runs from touch (`linkage.cpp:158-167`) and every frame with null. `lookupBreakable` (`:227-285`) traces 72–256 units (32 with the crowbar) toward `m_destOrigin` from the feet and from the eyes.
- Qualifying entities (`engine.cpp:1325-1353`): `func_breakable`, breakable `func_pushable`, or `func_wall` with takedamage, health between 1 and `breakable_health_limit`, and movetype PUSH or PUSHSTEP.
- `ShootBreakable` task priority is 100 (`tasks.cpp:1579-1638`).
- `checkBreakablesAround` (`botlib.cpp:151-225`) shoots visible breakables within `object_destroy_radius` (400) but more than 100 away. Each is given up after 1.5 s.
- The analyzer's flood traces pass through breakables.

### 4.16 Teleports
There is no handling. A teleport is only usable through a manual link across the `trigger_teleport`. Recovery afterwards falls to the lost-node and timeout logic.

### 4.17 Node validity and recovery
**`findValidNode`** (`:1955-2005`):
- No current node → `trySelectNewGoal`.
- Timed out (`navTimeset + getEstimatedNodeReachTime < now`, `:1923-1953`):
  - The estimate is 3.5 s base (8.5 s for crouch/ladder/ducking), or `5·dist²/(speed+1)²` clamped to 3–3.5 (3–8), +1 s if recently damaged.
  - Add +100 danger to the current node and its neighbours (written to `m_team` only, §8).
  - Then `trySelectNewGoal`.
- `trySelectNewGoal`: after more than one consecutive failure, pick a new goal and `findPath`. Otherwise `clearSearchNodes` + `findNextBestNode` and increment the failure count.

**`findNextBestNode`** (`:1809-1921`):
- Candidates come from the bucket of the predicted origin (the "handle fails" path when the graph has fewer than 1200 nodes), otherwise from all nodes.
- Skip the current node and, when `rg(0,2)` allows (graph of 512+ nodes, not stuck), the previous ones.
- Require a link from the current node and `isReachableNode`.
- Keep the 3 nearest (buggy overwrite) and pick one at random. If none, use `findNearestNode`.
- Start `lostReachableNodeTimer(dist²/maxspeed²·4)`, which forces exact reach.

**`findNearestNode`** (`:2028-2091`): nearest reachable node in the bucket within 1024 → nearest with a clear line from the eyes → any node at all.

**`isReachableNode`** (`:3341-3384`): within 600; in water → true; unless on a ladder column (2-D < 16), reject targets more than 62 above or 100 below; not occupied (with zero velocity); clear line.

### 4.18 Stuck detection and collision probing (`checkTerrain`, `:343-654`)
Runs only when move or strafe speed ≥10 (7 while ducking; negative speeds are ignored), the collision lockout is over, and the task is not Attack or Camp.

**Stuck when either:**
- moved less than 2 units in the 0.2 s window while the previous speed was above 20; or
- `isBlockedForward` stays true for 0.2 s. Not checked on a ladder path. The traces are listed at `:2683-2789`.

**Not stuck:** after `probeTime + rand(0.5, 0.75)`, `resetCollision`; until then keep applying the current probe action.

**Stuck and undecided:** probe bits are Strafe on a ladder, Strafe+Jump in water, all three otherwise. Only on the floor, ladder or in water are the moves weighted:

| move | weight contributions |
|---|---|
| Jump | +10 `canJumpUp` (**always false**); +5 target ≥18 higher; +5 target visible from both shoulders; +10 foot-level obstacle within 30 |
| Strafe left/right | ±5 by the side the target is on; −5 if that side is blocked (32×32 head-hull probe) |
| Duck | +10 `canDuckUnder` (**always false**); +5 target at least 36 below and visible |

Sort descending by weight and probe each move for `rand(0.5, 0.75)` s:
- Jump (only if on the floor or in water and not on a ladder);
- Duck;
- Strafe, via `setStrafeSpeed` (buggy, §8).

Other ways to suppress the terrain check: `ignoreCollision()` (0.65 s lockout), and many tasks that clear `m_checkTerrain`.

### 4.19 Player avoidance (`doPlayerAvoidance`, `:219-341`)
- **Skipped in FFA**, on ladders, when not solid, or with `has_team_semiclip`.
- Teamplay: choose the nearest same-team player within maxspeed distance with higher priority (`manager.cpp:1378-1404`).
- If stuck: share a single goal with the other bot and re-plan both.
- Predict positions for `dt·6` (moving) or `dt·2`. If within 64, or within 96 and closing: strafe away, and back up if very close.

### 4.20 Fall recovery
**`checkFall`** (`:656-725`):
- Only runs where |previous node z − target z| ≥ 44; flat segments skip it.
- Records the take-off point and target (enemy origin or path target).
- After landing, re-find nodes when the fall carried the bot far off or more than 138 (72) below; 1 s lockout.

**Non-player damage** (drowning, `trigger_hurt`): if the node nearest the target is unreachable, `clearSearchNodes` + `findNextBestNode` (`botlib.cpp:2181-2188`).

### 4.21 Edge safety
- **`isDeadlyMove(to)`** (`:3114-3157`): in practice it only tests whether the ground under `to` is more than 160 below, or there is no ground within 1000. The loop is dead code.
  - Used by: longjump, gauss jump, grenade avoidance (`botlib.cpp:1926`), pickup rejection (`botlib.cpp:806`).
- **`isNotSafeToMove(to)`** (`:3159-3169`): start-solid, or ground more than 160 below.
  - Used in `attackMovement` (`combat.cpp:1986-2001`): `spot = origin + fwd·move·0.2 + right·strafe·0.2 + vel·dt`. If unsafe, reverse both speeds and cancel the jump.
  - **HL fix** (4edb7f8): the check now runs when either speed is non-zero, including negative. Upstream only checked `> 0`, so backpedalling bots walked off ledges.

### 4.22 `checkCornerTripminePlant` (`:2379-2462`), called from `advanceMovement` before `changeNodeIndex(dest)`
Returns without planting unless all hold:
- not a GunGame rush-miner; cooldown passed;
- task Normal, no visible enemy, has tripmine ammo, on the floor;
- previous, current and destination nodes all valid;
- turn angle of at least 60°;
- the corner occludes: `!vistab.visible(prev, dest)`, or a trace check if the vistable is not ready;
- inner-side wall found 45 past the corner within 80, with |normal.z| ≤ 0.3, no known mine nearby, and an opposite wall within 250.

On success: cooldown 20–30 s (8–12 s in GunGame), `m_position` = wall point, `PlaceTripmine` task (6 s).

### 4.23 Other path consumers
- `findDefendNode` (`:2093-2205`) and `findCoverNode` (`:2207-2328`): vistable filtering, planner distances, practice-damage ranking.
- `getRandomCampDir` (`:3171-3228`).
- `findAimingNode` (`:3230-3255`) and the predicted enemy node (`botlib.cpp:1039-1100`): the first node along the enemy→bot planner path that is visible from the bot's node.
- `camp_`/`hide_` (`tasks.cpp:400-584`); `followUser_` (`:633-713`).
- `doublejump_` (`:1371-1441`): the bot paths to a human who asked for help and acts as a step. The node flag DoubleJump is unused.
- **Item pickups steer straight at the item**, setting `m_destOrigin` without a graph path (`tasks.cpp:1703-1738`).
- Enemy reachability uses `planner.preciseDistance` (`botlib.cpp:1582-1605`).
- Gauss-jump direction uses `pathWalk[2]` or `m_chosenGoalIndex` (`combat.cpp:1242-1269`).

---

## 5. Vistable and practice

**Vistable** (`vistable.cpp`):
- **Rebuild** (`:10-148`): per frame, one source node × 250–400 destination nodes, up to 4 line traces per pair.
  - Source eye heights: stand = origin+28 (crouch node: +46); crouch = origin−6 (crouch node: +12).
  - First trace both sources to `dst.origin`. If either is blocked, trace both again to `dst+28` (+46 for a crouch destination). The final bits are "blocked to the raised point".
  - Stored per byte `[src*n+dst]` at `(dst%4)*2`. Updates `vis.stand/crouch` = count of nodes visible from the source.
- Saves only when complete and the graph is unchanged. Stops if the graph changes.
- **`visible(a, b, Any)`** = not both bits set (`:155-160`). It is **directional**. Unbuilt entries are 0, meaning visible. `visibleBothSides` ANDs both directions.
- **Load** (`:162-183`): if the count or version mismatches, rebuild from zeros.
- While rebuilding, `practice.getDamage` returns 0 (`practice.cpp:52`) and `practice.update` is skipped.

**Practice** (`practice.cpp`, `practice.h`):
- A hash map keyed by `(start, goal, team)`, storing `{damage, value, index}` as i16. Team is clamped to 0–1. In FFA the team is the entity index, so **everyone shares slot 1**. In teamplay, teams beyond the second also share slot 1 (`message.cpp:141-176`).
- **Writes:**
  - `updatePracticeDamage` (`botlib.cpp:2257-2302`), for enemy damage ≥20: `(victimNode, attackerNode) += damage/(bot 10 : human 7)`, clamped to 2040; updates the team highest. The self entry (v, v) is re-stored unchanged, a no-op.
  - `updatePracticeValue` on death (`:2238-2255`): `(chosenGoal, prevGoal) −= health/20`.
  - Goal reached (§4.3): `(chosenGoal, cur) += health/2 + goalValue/2`.
  - Node timeout: `(n, n) += 100` (§4.17).
- **`update`** (`:91-153`), async: for each team and node i, `index(i,i)` = the visible node j with the highest `damage(i, j)`. If any exceeds 2040, subtract 1020 from everything. The team highest always decays by 1020 (minimum 1).
  - **HL runs this once per map**, from `roundStart` → `initRound` (`engine.cpp:1670-1700`, `manager.cpp:1893-1915`), so the danger index does not refresh during play. It can also race `practice.load`.
- **Readers:**
  - danger look (Nav aim);
  - `getCampDirection` fallback (`botlib.cpp:890-893`);
  - goal ranking (`getValue`);
  - danger-camp trigger and defend/cover ranking (`getDamage(n, n)`, which only accumulates timeout +100s);
  - editor HUD and arrows.
  - The planner g functions read it too, but those are unused in HL.
- **Save** on level and server shutdown when the graph is unchanged (`engine.cpp:166`, `linkage.cpp:1016`). Load skips empty entries (`:169-186`).

---

## 6. Graph editor and debug visualization

**Editor identity:**
- Listen server: the host becomes the editor on connect (`linkage.cpp:197-204`).
- Dedicated server: `graph acquire_editor` / `release_editor`.
- Command aliases: `graph|g|w|wp|wpt|waypoint`, and `graphmenu|wpmenu|wptmenu` (`control.cpp:2244-2263`).
- On a dedicated server without an editor, only acquire_editor, upload, save, load, help, erase, erase_training, fileinfo and check are allowed (`:393-419`).

**Commands** (`control.cpp:437-475`, handlers `:509-1110`):

| command | effect |
|---|---|
| `on [display\|auto\|noclip\|models]` | Enable display / auto-placement / noclip; also zeroes and saves mp_roundtime, mp_freezetime, mp_timelimit |
| `off [...]` | Disable the above; restores the saved cvars |
| `menu` | Open the editor menu |
| `add` | Node-type menu |
| `addbasic` | Run the §2 seeding |
| `save` | Checked save; `nocheck` skips the check; `old`/`oldformat` writes a PWF (fewer than 1024 nodes) |
| `load` | Reload the graph |
| `erase iamsure` | Delete graph and training files |
| `erase_training` | Delete `.vis`/`.prc`/`.pmx`, then reload |
| `refresh iamsure` | Re-download from the database |
| `delete [nearest\|idx]` | Remove a node |
| `check` | `checkNodes(teleport=true)` |
| `cache [nearest\|idx]` | Remember a node |
| `clean [all\|nearest\|idx]` | `clearConnections` |
| `setradius r [idx]` | Set the radius |
| `flags` | Flag menu |
| `teleport idx` | Move the editor to a node |
| `upload` | Refused for analyzed or invalid graphs |
| `stats`, `fileinfo` | Information |
| `adjust_height dz` | Shift every node's z |
| `path_create`, `path_create_{in,out,both,jump}` | Link nearest↔facing (falls back to the cached node) |
| `path_delete` | Remove a link |
| `path_set_autopath` | Autopath distance menu |
| `path_clean [idx]` | `resetPath` |
| `iterate_camp begin\|next\|end` | Step through camp nodes |

**Menus** (`control.cpp:2455-2573`):
- Page 1: show/hide, cache, create path, delete path, add node, delete node, autopath, radius.
- Page 2: debug goal, auto placement, flags, save (checked), save (unchecked), load, check, noclip.
- Type menu: Normal, Team 1, Team 2, Camp, Camp end, Map goal, Jump (learn).
- Flag menu: Team 1, Team 2, Lift ("Use elevator"), Sniper, Goal, Crouch (±18z), Camp (opens the directions menu).

**Frame** (`graph.cpp:2043-2421`):
- Keep noclip on.
- **Jump learning:** once armed, pressing jump creates JumpStart at the pre-jump position with the pre-jump velocity. Landing (on the floor or ladder, at least 0.1 s later) creates JumpEnd and a Jump link carrying that velocity.
- **Auto mode:** on the ground, more than 128 from the last node and with no reachable node within 128, add a Normal node.
- **Node beams:** refreshed each second within `graph_draw_distance` (400) when visible or within 64. 72 tall (36 for crouch), width 14 (28 for the node being faced).

  | node type | colour (RGB) |
  |---|---|
  | goal | 128,0,255 |
  | ladder | 128,64,0 |
  | camp | 0,255,255; tinted by team |
  | team 1 | 255,0,0 |
  | team 2 | 0,0,255 |
  | normal | 0,255,0 |

  The top 25% of the beam is coloured for Sniper (130,87,0) and Lift (255,0,255).
- **Arrows:** search target pink, cached node yellow, faced node white.
- **For the nearest node (within 56)**, drawn every 0.96 s:
  - camp lines (red, 500 units);
  - links: jump magenta, two-way yellow, one-way white, incoming-only 0,192,96;
  - radius octagon, or an X for radius 0;
  - danger arrows (team 1 red, team 2 blue);
  - HUD text blocks: current, cached and faced node info, practice data, map and time.
- Beams are `TE_BEAMPOINTS`, written end→start, using `laserbeam.spr` or `arrow1.spr` (`engine.cpp:206-234`).

**Bot overlay** (`debug`≥1 and the editor spectating the bot, `botlib.cpp:1963-2115`): HUD state; arrows for destination (green), ideal angles (blue) and view (red); the remaining path in orange.

**Observer/telemetry:** streams the graph nodes, links and bot paths (`telemetry.cpp:209-318,441-460`; `tools/observer`).

---

## 7. HL changes vs upstream CS yapb

**Removed:**
- Bomb, hostage, VIP and escape goal logic: `findBestGoalWhenBombAction`, `findBombNode`, `getNearestToPlantedBomb`, the rescue tactic, C4/VIP forcing, CS map-type desire biases.
- Hostage-aware planner functions (`*WithHostage`) and the NoHostage filter.
- Rescue and NoHostage node creation in `add`, `addBasic` and `markGoals`.
- Seeding for `info_vip_start`, `armoury_entity`, bomb/rescue/VIP/escape zones and hostages.
- The 96-unit hostage-goal reach radius.
- The bomb "sector clear" visit logic (`setVisited` is now never called).
- Camping on arrival at a camp node in `normal_`.
- Walking with shift near enemies.
- Freezetime and buy logic; radio and chatter.
- The T/CT-points and rescue requirements in `checkNodes`.
- CS `MapFlags` (only HasDoors and HasButtons remain).

**Kept in the data format:** every `NodeFlag` bit (T/CT relabelled "Team 1/2"), the `NodeAddFlag` values, and the container, PWF and podbot conversion.

**Repurposed:**
- Goal = weapon, ammo, item and charger spots.
- `Team::First/Second` for teamplay team indices in first-seen order; the entity index in FFA.

**Added or changed:**
- Longjump (§4.10).
- The corner-tripmine hook (§4.22).
- Gauss-jump travel.
- GunGame: goal desire 0.
- Camp desire ×0.3 and no sniper boost.
- Danger camp: 15% chance, `rand(5,15)+4` s, 60 s lockout (upstream: 5 s and round-midpoint based).
- Hide duration 2–5 s, standing (seekCover).
- Path type always Fast.
- Practice team clamping.
- Graph database URLs emptied.
- "Round" timers seeded once per map; midpoint = start+90 s (`engine.cpp:1670-1700`).
- Every respawn runs `newRound` (`botlib.cpp:1958-1961`).
- `maxspeed` seeded from `sv_maxspeed` (`botlib.cpp:1703-1712`).
- Fixes: `PathWalk::hasNext`, the `isBlockedForward` door/hostage logic (upstream never reported blocking on door-less maps), and the negative-speed ledge check.
- The crowbar replaces the knife for long jumps. This is a CS-ism that is pointless in HL.

---

## 8. Bugs and quirks not to port blindly

**Graph and editor**
1. `m_previousNodes[1]` is never written (`navigate.cpp:2011-2014`), so indices 1–4 are always −1. Dead features as a result: ladder-tower avoidance (`:1226`), the lift fallbacks (`:1735,1757`), and extra-previous skipping (`:1857`).
2. Bucket hash ignores X (`graph.cpp:2797-2804`). Buckets are never updated after load (`eraseFromBucket` and `addToBucket` are unused by edits).
3. Narrow-place traces go to absolute positions (`graph.cpp:1630-1643`). The resulting bogus Narrow flags are **persisted in v2 files**.
4. Ladder offset is `(a−b) ^ nullptr` = 0 (`graph.cpp:2653`).
5. `calculatePathRadius` does a zero-length hull trace (dead door branch) and uses cumulative yaw steps (`:1464-1467,1515`).
6. `populateNodes` else-if chain makes `m_sniperPoints` effectively empty (`:1665-1685`).
7. `addPath` replaces the longest link even with a longer one (`:446-462`). Automatic links use 2-D distance, so ladders cost about 0.
8. `checkNodes`: `break` on number mismatch (`:2452-2457`), `>` instead of `>=` (`:2461`), goals not counted for Camp+Goal nodes, and zero-padded incoming lists (`:2577`).
9. `getNearestInRadius` compares the radius with `256²` (`:577`); editor nearest-node display clamps radius against squared constants (`:2225`).
10. JumpEnd nodes get no automatic outgoing links and radius 0 (`:766-783`).
11. The editor crouch toggle uses `flags != Crouch` (`control.cpp:1698`), and `graph[-1]` is possible.

**Analyzer**

12. `cleanup` treats `link.index < -kInvalidNodeIndex` (<1) as invalid, which **erases every node that links to node 0** (`analyze.cpp:261`). It also erases several nodes per iteration and reads past the end after erasing.
13. The merge step erases by stale indices (`:219-222`).
14. Direction 8 is unreachable (`:88`).
15. The throttle is inverted: at low server fps there is no budget (`:360-362`).
16. `m_isCrouch` stays set after analysis, so later editor or merged nodes can wrongly get Crouch (`graph.cpp:789`).
17. `WithJump` (44) is stricter than normal reachability (45) by default. Reachability only rejects steps up, never drops.
18. Goal marking is XY-only.

**Planner**

19. A* is keyed by g (`planner.cpp:210,287`), so heuristics are irrelevant. The round-start randomizer is a no-op, and `rg(0.5, 0)` inverts its range for team 0.
20. The open-list guard kicks the bot and permanently disables A* for the map (`:230-237`, `navigate.cpp:3492-3497`).
21. `cantSkipNode` "too close" uses `sqrt(40)` against a squared distance and checks jump flags on either node's links by slot (`planner.cpp:160,168`).
22. `gfunctionKillsDist` has no distance term.
23. Dijkstra failure yields the path `[dst]`. `preciseDistance` has no bounds check (`botlib.cpp:1595`).
24. `PathWalk` capacity is `n/2+16` with no bounds check on add (`yapb.h:194-207`), so long paths can overflow.
25. Floyd uses i16 distances (anything past 32767 counts as unreachable) and n² memory.
26. `m_chosenGoalIndex = src` in `syncFindPath` (`navigate.cpp:3478`, `3402`). Practice treats it consistently as the route start, but the gauss-jump code treats it as the goal.
27. Async path finding races with the main thread; duplicate requests are dropped by `tryLock`.

**Movement**

28. `canJumpUp`, `doneCanJumpUp` and `canDuckUnder` always return false (`fraction > 1`, `navigate.cpp:2924,2995,3044`).
29. The collision probe reads `m_collideMoves[4]`, one past the end (`:614,623`).
30. `isBlockedForward`'s "diagonal" traces all run along the right side and duplicate each other (`:2718-2731,2766-2779`).
31. `setStrafeSpeed` treats a direction vector as a position (`:3258`), so strafe direction depends on the world origin.
32. The `isDeadlyMove` loop never runs (`:3128`).
33. `selectBestNextNode` tests `PathFlag::Jump` against node flags, i.e. the Button bit (`:2366`), and checks visibility prev→next rather than to the alternative node.
34. `findNextBestNodeEx` overwrites all three nearest slots (`:1882-1887`); `findDefendNode` and `findCoverNode` have the same pattern.
35. `getEstimatedNodeReachTime` uses a squared ratio (`:1944`).
36. The node-timeout danger update writes to `m_team` for both loop iterations (`:1998,2001`).
37. `isOccupiedNode` accepts `Used|Alive` as either flag and returns on the first nearby bot (`:3279,3301`). It is effectively always false in FFA, so occupancy logic, `selectBestNextNode`, ladder and lift sharing do nothing in FFA.
38. Ladder normal nudge ±1 with a self-ignoring trace; the destination lags one frame (`:1175-1185`).
39. `postProcessGoals` can leave slot 0 empty, which leads to a random goal, and shares a `static` array (`:151-180`). The goal history grows without bound during a life.
40. Cheats you may keep or drop deliberately: setting jump velocity directly, and remote `MDLL_Use` on doors and buttons.

**Storage, practice and vistable**

41. `outOptions = &hdr.options` (`storage.cpp:166`) never propagates, so the Official check in load is dead.
42. `uncompressed` is not validated against `length×220`.
43. The `.vis`, `.prc` and `.pmx` caches are validated only by node count, so **link-only edits reuse stale caches**. Add a graph hash.
44. `updatePracticeDamage`'s self entry is a no-op (`botlib.cpp:2291`).
45. FFA collapses all practice data into one slot, and the danger index is computed once per map.
46. The vistable uses 1 byte per pair of which 2 bits are meaningful, is directional, and reads unbuilt entries as visible.
