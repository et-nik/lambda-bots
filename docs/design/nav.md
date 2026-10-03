# Navigation: BSP, graph, generator, validator, planner, overlays, scenarios, editor, Lua

> Original design note (in English), prepared during planning on 2026-09-26.
> A condensed version with the final decisions lives in the implementation plan; in case of conflict, the plan
> and later decisions in the repository take precedence. `file:line` references reflect the sources as of the note's date.

# lambdabots — Navigation, Map Knowledge, Overlays, Scenarios, Editor, Lua-readiness

## 0. Facts that change earlier assumptions

I checked these against the BSP lumps (read-only parsing of the Steam maps), SDK, BugfixedHL-Rebased (BHL) and ReHLDS sources.

1. **Crossfire strike trigger geometry.**
   - The player touches the trigger from the south, standing inside the closed volume of the shutter `strike_ready_door`.
   - Hull-point scan of models `*65` (trigger), `*66` (fake button func_wall) and the world:
     - standing (hull 1, origin z −1852): touch window is origin y ∈ [−2242, −2240), about 2 u deep;
     - crouched (hull 3): the window is y ∈ [−2242, −2236).
   - The fake button func_wall and the trigger share their north plane, so the trigger cannot be reached from the north. Touch spots must be derived from hulls, never from the trigger centre.
   - At +2 s the shutter closes: it moves up at 50 u/s with dmg 9999. Its hull-expanded footprint is y ∈ [−2272, −2200].
   - So the presser must retreat south of y ≈ −2285 within about 1.5 s.
2. **Crossfire timeline (strike_mm).** Times are seconds after the trigger.

   | Time | Event |
   |---|---|
   | 0 | Siren on |
   | 2 | Shutter closes |
   | 20 | Main entrance door closes. It starts open, travels 118 u at 5 u/s and has crush dmg 100000. A standing player can pass until about +29.2, a crouched one until about +36.4. The side slabs (±416, −1514) close in 0.4 s. |
   | 44 | Flyby sound |
   | 45 | Siren off |
   | 47 / 54 | Tower hatches close / reopen |
   | 52.0–52.1 | strike_pain on (11 trigger_hurt, dmg 10000). It is a 0.1 s lethal pulse. |
   | 55 | strike_timer_mmb fires, which schedules strike_timer_mma at +180. So the shutter reopens at +235, not +180. |
   | 64 | Main door reopens (about 24 s; standing passage from about +78) |

   - The first arming is `trigger_auto` → `trigger_relay` (delay 180) → shutter at map time about 180 s.
   - The siren is 7 `ambient_generic` "ambience/siren.wav" with spawnflags 24 (large radius, start silent), including one inside the bunker.
   - **Adapter requirement:** it must hook `pfnEmitAmbientSound`.
3. **Crossfire "lifts"** are `func_door` lift1–4 (up 160 u, speed 200, wait 1).
   - Each is triggered by a `func_button` on the shaft wall, which can only be reached while standing on the platform at the bottom.
   - Tower tops are reached by `func_ladder` models `*8`/`*9`. The tower plates (spawnflags 257) target names that don't exist.
   - Also dead: `trigger_multiple` at (16, −2512) → `tower_buttons_ms_targ`, and multisource `tower_buttons_ms` (it has no inputs).
4. **Doublecross has the same airstrike with different numbers:**
   - it is fired by a `func_button` (+use, wait 180);
   - maindoor 35, flyby 59, pain 67–67.5, re-arm timer at 70 → 250;
   - `poison_mm` hurts the button room from +5 to +120 s.
   - So the airstrike must be a parameterised template.
5. **Most `func_illusionary` are `{rail1`, `{ladder*` or `{grate*` decor.** Railings are walk-through, so ledges often have no physical railing.
   - Secret detection must classify textures: `{`-prefixed alpha textures are decor; opaque textures on a vertical slab are a fake wall.
6. **Explosion knockback uses damage after armor.** Sources: `player.cpp 445–509` then `combat.cpp 893–896`, `DamageForce 1020–1030`.
   - Knockback is ×2 when crouched (bounding-box volume).
   - Explosive self-boosts are weak with armor and lethal without it; the resource model must include armor.
7. **Bunnyhop cap.** Vanilla SDK always applies the 1.7× cap (`pm_shared.c 2560`). BHL applies it only if `!mp_bunnyhop` (default is 1, i.e. uncapped) (`BHL pm_shared.cpp 2756`, `gamerules.cpp 38–47`).
   - `mp_useslowdown`: 1 = maxspeed/3 while +use on ground; 0 = velocity×0.3 every frame (`BHL pm_shared.cpp 3062, 3242`).
8. **PlayerUse** (`player.cpp 1573–1686`):
   - the target must be within 64 u of the player origin, measured to the target's bounding box (`FindEntityInSphere`);
   - cone test: normalised `ClampVectorToBox(VecBModelOrigin − eyes, size/2)` · forward > 0.7; the highest dot wins;
   - there is no line-of-sight check; the use is edge-triggered for IMPULSE/ONOFF objects;
   - if the eyes are inside the target box on all axes the vector is zero and the use fails;
   - shootable buttons (health > 0) are not usable (`cbase.h 723`).
9. **Multisource** is a toggle-parity AND with persistent hidden state (`buttons.cpp 182–237`). Bots cannot observe it, so it is executed with verify/retry and never assumed.
10. **Gasworks has 15 `func_door` named "dontopen" that nothing targets**, so they are permanent walls. A named door with no activator must be classified as solid.
11. **Trigger touch is an exact hull-point test**, not a bounding-box test (`ReHLDS world.cpp 365–377`).

---

## 0.1 Workspace layout and dependency rules

```
crates/
  lb-worldq/   tiny shared contracts: Tracer/WorldQuery traits, TraceQuery/Trace, HullKind, Contents,
               StableEntityRef, BrushPose. Also used by the adapter crate (no FFI here).
  lb-bsp/      BSP v30 parse, hull tracer, contents, faces/textures, PVS/PAS, entity lump, typed entities,
               spawn-pose resolver, mechanism graph, BspWorld (impl Tracer)
  lb-kin/      PhysicsProfile/DamageRules, simplified GoldSrc integrator, TraversalValidator, LiveCheckPlan
  lb-nav/      graph model + .lbnav format + importers + planner + follower + executors + steering
               graph/ format/ import/{yapb,ulz,pwf} plan/ follow/ exec/
  lb-navgen/   generator pipeline, coverage, vis table, tactical precompute, live-check job builder
  lb-mapknow/  items, tactical queries, spot catalogue, chokepoints/mines, danger map
  lb-ext/      overlay schema/loader/binder/validator, scenario engine, BehaviorModule API,
               primitive library, reference modules, editor model
  lb-lua/      (later) mlua bindings over lb-ext::api
tools/lb-navtool/   offline CLI: gen, import-yapb, entities, validate-overlay, path, coverage, tracecheck, export-observer
```

**Dependency direction:** `worldq ← bsp ← kin ← nav ← navgen`; `mapknow → nav`; `ext → nav, mapknow`.
- Only the runtime/adapter crate (other architect) touches FFI.
- `lb-navtool` never links engine code.

**Threads:**
- Generator and heavy precompute (vis, tactical, ALT landmarks) run on a worker pool: rayon, `lb_nav_threads`, default cores−2, minimum 1.
- Planner, follower and live validation run on the main thread in budgeted slices.
- Graph snapshots are published with `ArcSwap<NavGraph>` at frame phase 2 (v2 §5.1).
- Target is i686. The whole nav runtime should stay under about 100 MB (32-bit HLDS address space). Only pure-Rust dependencies: `blake3`, `lz4_flex`, `postcard`, `smallvec`, `arc-swap`, `rayon`.

**Needed from other subsystems:**
- **Adapter:**
  - live `TraceHull`/`TraceLine`/`TraceModel`/`PointContents` behind `lb-worldq::Tracer`;
  - `read_game_file()` (handles valve_downloads);
  - SelfState: onground, groundentity as StableEntityRef, waterlevel, movetype ladder, `FL_DUCKING`, `slj` physinfo, health/armor/inventory;
  - hooks for ambient and entity sounds;
  - DebugDraw: `TE_BEAMPOINTS`/`TE_TEXTMESSAGE` sent `MSG_ONE_UNRELIABLE`;
  - console commands `lb …` with an admin check;
  - physics/rules cvars.
- **AI core:** intent types and channels, `GoalCandidate`/`ActionRuntime`, the Perception observation stream for mechanism states and perceived players, and the `ActionRequest` path (e.g. destroy an entity).

---

## 1. `lb-bsp`: BSP v30 loader and hull tracer

**What is parsed and why**

| Lump | Use |
|---|---|
| ENTITIES | typed entity model and mechanism graph |
| PLANES, CLIPNODES | hulls 1–3 |
| NODES, LEAFS | hull 0 (as in `Mod_MakeHull0`, `model.cpp 1146–1176`), leaf lookup for PVS |
| MODELS | submodel headnodes[4], bounds, origin (`Mod_LoadBrushModel 1271–1363`) |
| VISIBILITY | PVS (and PAS derived from it) |
| FACES, TEXINFO, TEXTURES, VERTEXES, EDGES, SURFEDGES | walkable-face seeding, texture classes (`{` alpha, `sky`, `!`/`*` water), fake-wall and occluder detection, observer export |
| LIGHTING, MARKSURFACES | not needed |

- Version 30 is required; 29 is accepted with a warning. BSP2/BSPX are refused with a diagnostic because ReHLDS cannot load them.
- Structures follow `rehlds/public/rehlds/bspfile.h 25–165`.

```rust
pub struct Bsp { bytes: Arc<[u8]>, planes: Vec<Plane>, nodes: Vec<DNode>, clipnodes: Vec<ClipNode>,
                 leafs: Vec<Leaf>, models: Vec<Model>, vis: Vec<u8>, faces: Vec<Face>, texinfo: Vec<TexInfo>,
                 textures: Vec<TexName>, verts: Vec<Vec3>, edges: Vec<[u16;2]>, surfedges: Vec<i32>,
                 lump: EntityLump }
#[repr(u8)] pub enum HullKind { Point = 0, Stand = 1, Large = 2, Crouch = 3 }
impl HullKind {
  /// SV_HullForBsp rules (world.cpp 177–228): size.x<=8→0; size.x<=36→(size.z<=36→3 else 1); else 2
  pub fn for_size(mins: Vec3, maxs: Vec3) -> Self;
  pub fn clip(self) -> (Vec3, Vec3); // 1:(±16,±16,±36) 2:(±32)^3 3:(±16,±16,±18) (model.cpp 1104–1136)
}
impl Bsp {
  pub fn parse(bytes: Arc<[u8]>) -> Result<Self, BspError>;
  pub fn fingerprint(&self) -> BspFingerprint;                 // blake3(full file) + size (entities included)
  pub fn hull(&self, m: ModelIdx, k: HullKind) -> HullRef<'_>; // hull0 walks dnodes, leaf contents via leafs[-1-c]
  pub fn hull_contents(&self, h: HullRef, p: Vec3) -> Contents; // SV_HullPointContents (world.cpp 581–607)
  pub fn recursive_hull_check(&self, h: HullRef, p1: Vec3, p2: Vec3, tr: &mut Trace) -> bool;
      // exact port of SV_RecursiveHullCheck (world.cpp 727–866): DIST_EPSILON 0.03125, NaN guard,
      // "backup past 0" loop; f32 math, f64 only for the midpoint (as REHLDS_FIXES real3_t)
  pub fn point_contents(&self, p: Vec3) -> Contents;           // world hull0, CURRENT_* folded to Water (world.cpp 695–710)
  pub fn leaf_at(&self, p: Vec3) -> LeafIdx;
  pub fn pvs_row(&self, leaf: LeafIdx) -> BitRow;              // RLE-zero decompression, row=(visleafs+7)/8, bit j→leaf j+1
  pub fn pas(&self) -> &PasTable;                              // lazily: OR of PVS rows of visible leafs (as SV_CalcPAS)
  pub fn model_textures(&self, m: ModelIdx) -> impl Iterator<Item=&TexName>; // dmodel firstface/numfaces are fields 14/15
  pub fn walkable_faces(&self, m: ModelIdx, min_nz: f32) -> impl Iterator<Item=FacePoly>; // honour face.side
}
```

**Brush entities and poses**
- Rotated SOLID_BSP entities are traced like `SV_SingleClipMoveToEntity` (`world.cpp 1021–1141`): endpoints are rotated into the model frame and the hit normal is rotated back.
- Entities with an ORIGIN brush use the `origin` key as their transform.
- Collision boxes follow `SetObjectCollisionBox` (`cbase.cpp 628–662`): ±1 u, and a max-radius box when rotated. These matter for touch overlap and use distance.

**Spawn-pose resolver (port of spawn rules)**
- `func_door` / `func_door_rotating`:
  - `SetMovedir`: angle −1 → up, −2 → down;
  - `pos2 = pos1 + movedir·(|movedir·(size−2)| − lip)`;
  - START_OPEN (1) swaps poses; flags USE_ONLY 256, NO_AUTO_RETURN 32, ONEWAY 16, PASSABLE 8;
  - source: `doors.cpp 300–330`.
- `func_plat`: `pos1` = top, `pos2 = top − height` (height default `size.z − 8`). Untargeted plats start at the bottom; the trigger field is spawned above them (`plats.cpp 275–396`).
- `func_train`: centre placed on the first `path_corner`.
- Items/weapons: bounds (−16, −16, 0)–(16, 16, 16), then `DROP_TO_FLOOR`, i.e. a trace down 256 u with `HullKind::for_size` (`items.cpp 91–99`).

**Typed entity model**

```rust
pub struct Entity { pub idx: EntIdx /*lump order*/, pub class: SmolStr, pub targetname: Option<SmolStr>,
  pub target: Option<SmolStr>, pub killtarget: Option<SmolStr>, pub master: Option<SmolStr>, pub origin: Vec3,
  pub angles: Vec3, pub model: Option<ModelRef>, pub spawnflags: u32,
  pub kv: Vec<(SmolStr,SmolStr)> /*ordered, duplicates kept: multi_manager*/, pub kind: EntityKind,
  pub spawn_pose: BrushPose, pub bounds: Aabb }
pub enum EntityKind {
  Worldspawn, Spawn{deathmatch: bool}, Item(ItemClass), Weapon(WeaponClass), Ammo(AmmoClass), LongJump,
  Charger{kind: ChargerKind},                                   // func_healthcharger / func_recharge
  Door(DoorSpec /*linear|rotating: movedir|axis, distance, speed, wait, lip, dmg, flags, health, sounds*/),
  Button(ButtonSpec /*linear|rot|momentary, touch_only(256), dont_move(1), toggle(32), health, wait*/),
  Plat(PlatSpec), Train(TrainSpec), TrackTrain, PathCorner{target, wait, speed},
  Trigger(TriggerSpec /*Multiple|Once|Teleport{target}|Push{speed,dir,once,start_off}|Hurt{dmg_per_s,start_off,damagetype}
                        |Gravity{mult}|Other*/),
  Ladder, Water{contents: Contents}, Illusionary{contents, visual: VisualClass}, Wall{toggle: bool, start_off: bool},
  Breakable(BreakSpec /*health, material, trigger_only(1), pressure(4), crowbar(256), target*/), Pushable,
  Conveyor{speed, dir}, Rotating, Tank{..}, TankControls{target},
  MultiManager{targets: Vec<(SmolStr, f32)>, thread: bool}, Relay{target, delay, state}, AutoTrigger{target, delay, state, once},
  MultiSource, EnvGlobal, ChangeTarget{new_target}, Ambient{sample, radius: AttnClass, start_silent, looped},
  TeleportDest, InfoNode, Other }
```

Visual classes for occluders: `Opaque`, `Alpha{` (`{`-textures or rendermode 4), `Translucent` (rendermode texture/additive with renderamt < 255), `Invisible`. These are exported as `VisualOccluders` for perception, because engine traces go through `func_illusionary` but people cannot see through opaque ones.

**Mechanism graph** (replaces `lookupButton`, `navigate.cpp 3306–3339`, which could not see through multi_manager chains)
- Nodes are entities. Edges are:
  - `target` / `killtarget`;
  - multi_manager keys (the `#n` suffix is stripped, each key keeps its delay);
  - relay/auto delays;
  - multisource AND inputs;
  - master locks;
  - `trigger_changetarget` (marks the graph dynamic).
- For every effect entity it computes `Activation{activator, action: Use|Touch|Shoot|Break|Auto|Timer, delay, via, conditions}`.
- It flags dead links: targets without a matching targetname, e.g. crossfire `tower*_button_target_X`.
- It flags hidden-state gates (multisource parity, toggle doors) and never-activated named doors (gasworks "dontopen").

**Trace and world contracts (`lb-worldq`)**

```rust
pub struct TraceQuery { pub start: Vec3, pub end: Vec3, pub hull: HullKind, pub mask: SolidMask, pub ignore: Option<EntityRef> }
bitflags! { pub struct SolidMask: u16 { WORLD; STATIC_BRUSH /*func_wall, chargers, buttons*/; MOVERS /*doors,plats,trains at pose*/;
            BREAKABLES; PUSHABLES; PLAYERS /*live only*/; OPAQUE_ILLUSIONARY /*visual tests only*/ } }
pub struct Trace { pub all_solid: bool, pub start_solid: bool, pub in_open: bool, pub in_water: bool,
                   pub fraction: f32, pub end: Vec3, pub normal: Vec3, pub dist: f32, pub hit: Hit /*World|Entity(EntityRef)*/ }
pub trait Tracer {                      // BspWorld view (offline, &mut self for counters) and LiveTracer (adapter, budgeted)
  fn trace(&mut self, q: &TraceQuery) -> Trace;
  fn contents(&mut self, p: Vec3) -> Contents;                               // world + func_water/func_ladder volumes (skin)
  fn touching(&mut self, origin: Vec3, hull: HullKind, out: &mut SmallVec<[EntityRef; 4]>); // exact hull-point test
}
pub struct WorldView<'a> { world: &'a BspWorld, poses: &'a PoseOverrides, removed: &'a EntBitSet } // "door open", "breakable gone"
```

`StableEntityRef = {classname, targetname, model "*N" (unique for brush entities), origin_q (1 u), kv_hash}`.
- The adapter resolves it to an edict: brush entities by model; point entities by class + targetname + origin within 16 u, because items drop to the floor.
- An ambiguous match disables dependent features with a diagnostic (v2 §4.2).

---

## 2. Graph model, TraversalSpec, file format, cache key, yapb importer

### 2.1 Graph

```rust
pub struct NavNode { pub key: NodeKey /*quantized floor(8u)+stance+layer*/, pub floor: Vec3 /*feet*/, pub normal: [i8;3],
  pub stance: Stance /*Stand|Crouch|Swim|Ladder*/, pub arrival_r: f16, pub edge_dist: f16, pub contents: ContentsFlags
  /*water level 0..3, slime, lava, hurt, ladder, lowgrav, conveyor*/, pub flags: NodeFlags /*Narrow, DeadEnd, Edge, Hazard,
  Secret, Spawn, ItemAnchor, InteractionSpot, YieldSpot, ChokePoint*/, pub leaf: u32, pub area: AreaId, pub out: (u32,u16), pub inc: (u32,u16) }
pub struct NavLink { pub key: LinkKey /*hash(kind, q(entry), q(exit), entity refs)*/, pub from: NodeId, pub to: NodeId,
  pub kind: LinkKind, pub flags: LinkFlags /*OfflineValid, LiveConfirmed, LivePending, LiveMismatch, Secret, DynamicGeometry,
  Imported, Overlay, Editor*/, pub base_cost: f32 /*lower-bound seconds*/, pub length: f32, pub width: f16, pub spec: Option<SpecIdx> }
pub enum LinkKind { Walk, Crouch, Jump, CrouchJump, Drop, LadderBoard, LadderClimb, LadderExit, Swim, Surface, Dive, WaterExit,
  Door, Lift, Plat, Train, Teleport, Push, Breakable, SecretPass, LongJump, GaussBoost, RocketBoost, GrenadeBoost, SatchelJump, Scripted }
```

Adjacency is CSR (outgoing and incoming). Walk/Crouch links stay compact; everything else has a `TraversalSpec`, which is v2 §9.1 expressed in Rust:

```rust
pub struct TraversalSpec { pub id: LinkKey, pub entry: NavAnchor, pub exit: NavAnchor, pub movement: MovementProfile,
  pub required: Preconditions, pub interaction: InteractionSpec, pub deps: DependencySet, pub success: Completion,
  pub recovery: RecoveryPolicy, pub cost: CostEstimate, pub params: TraversalParams, pub provenance: Provenance,
  pub validation: ValidationRecord }
pub struct NavAnchor { pub node: NodeId, pub region: Region /*Disc|Box|Segment{a,b,w}|Poly*/, pub stance: Stance,
  pub speed: SpeedReq { min: f32, max: f32, dir: Option<(Vec2, f32 /*cos tol*/)> }, pub yaw: Option<YawRange>, pub pitch: Option<(f32,f32)> }
pub struct Preconditions { pub caps: CapSet /*LongJump, Gauss, Rpg, HandGrenade, Satchel, Crowbar, KnowsSecrets, Swim*/,
  pub min_health: f32, pub min_armor: f32, pub max_armor: Option<f32>, pub ammo: SmallVec<[(AmmoKind,u16);2]>,
  pub rules: RuleReq /*bhop uncapped, falldamage mode*/, pub window: Option<TimeWindow> }
pub enum InteractionSpec { None, Touch{ent: EntityRef, spots: SmallVec<[TouchSpot;2]>},
  Use{ent: EntityRef, spots: SmallVec<[UseSpot;2]>, retry: UseRetry /*never while moving; toggle-safe*/},
  Shoot{ent: EntityRef, aim: Vec3, health: f32, melee_ok: bool}, Wait{until: WaitCond, max: f32},
  Board{ent: EntityRef}, Ride{ent: EntityRef, until: PoseCond}, Exit{ent: EntityRef, window: f32},
  Sequence(SmallVec<[InteractionStep;4]>) /*remote button → pass door; multisource A,B → verify*/ }
pub struct Dependency { pub ent: EntityRef, pub need: MechState /*Open, AtTop, AtBottom, Active, Broken, Removed*/,
  pub provided_by: Option<InteractionRef>, pub observable: bool, pub hidden_state: bool }
pub enum Completion { InRegion{anchor: AnchorRef, onground: bool}, OnLadder(EntityRef), WaterLevel(u8),
  Teleported{dest: Region}, MechState(EntityRef, MechState), All(Vec<Completion>), Any(Vec<Completion>) }
pub struct RecoveryPolicy { pub attempts: u8, pub phase_timeouts: [f32; 4], pub on_fail: OnFail /*ReplanPenalty{ttl}|ReturnToEntry|AbortToAnchor*/, pub irreversible_after: Phase }
pub struct CostEstimate { pub time: f32, pub wait: f32, pub risk: f32, pub health: f32, pub armor: f32, pub ammo: SmallVec<[(AmmoKind,u16);2]>, pub uncertainty: f32 }
```

### 2.2 `.lbnav` file (chunked, versioned per chunk)

- **Header:** magic `LBNAV\0\r\n`, `format u16`, `flags u16`, `toc_off u64`, `toc_n u32`, `key_hash [u8;32]`, `created u64`.
- **TOC entry:** `{tag[4], ver u16, codec u8 (0 raw / 1 lz4), off u64, len u64, raw_len u64, crc32c u32}`.
- **Chunks** (postcard-encoded, unknown tags skipped):

| Tag | Content |
|---|---|
| META | header, stats, provenance |
| ENTS | StableEntityRef table |
| NODE, LINK | struct-of-arrays and CSR |
| SPEC | traversal specs |
| MECH | mechanism graph |
| FLOR | FloorField |
| WATR | WaterField |
| VIST | vis table |
| TACT | tactical precompute |
| ALTL | ALT landmarks |
| REPT | coverage report |
| LIVE | live-check results |

```rust
pub struct CacheKey { pub bsp: BspFingerprint /*blake3(file)+size*/, pub format: u16, pub generator: SemVer,
  pub physics: u64 /*hash(PhysicsProfile)*/, pub rules: u64 /*hash(DamageRules+dll id+mp_falldamage,mp_bunnyhop,...)*/,
  pub overlay_nav: u64 /*hash of canonicalised nav-affecting overlay parts only*/ }
```

- Stored at `addons/lambdabots/data/nav/<map>/<map>-<hex8>.lbnav`, with an LRU of 4 variants.
- `live.yaml` is stored separately, keyed by `(LinkKey, physics hash, engine/dll build id)`, so live confirmations survive regeneration.
- This fixes yapb's check-by-node-count cache (`storage.cpp 113`).

### 2.3 yapb `.graph` importer (`lb-nav::import::yapb`), for bootstrapping

- **24-byte little-endian header:**
  - `magic i32` = `0x59415042` ("BPAY") or `0x544F4255` ("UBOT");
  - `version i32` (1..=2 accepted, above that warn);
  - `options i32` (Graph = 8 is required; Exten = 64 means a 68-byte trailer `author[32], mapSize i32, modified[32]`);
  - `length i32` (1..=4096);
  - `compressed i32`;
  - `uncompressed i32` (must equal `length × 220`).
  - Source: `storage.cpp 12–217`, `graph.h 131–145`.
- **ULZ decoder** is an exact, bounds-checked port of `crlib/ulz.h 188–250`:
  - token ≥ 32 means a literal run `token>>5` (7 → +varint);
  - match length `(token&15)+4` (19 → +varint);
  - distance `((token&16)<<12) + u16le`;
  - varint = `Σ b_i<<7i` for i = 0..21, with the continuation bit left inside the value (matches the encoder's −128 bias);
  - dist < 8 copies byte-wise; otherwise non-overlapping chunks; no 8-byte overcopy; `ip == end` is required at the end.
- **Path record, 220 bytes:**

  | Field | Type |
  |---|---|
  | `number` | i32 |
  | `flags` | i32 |
  | `origin`, `start`, `end` | f32×3 each |
  | `radius`, `light`, `display` | f32 |
  | `links[8]` | `{velocity f32×3, distance i32, flags u16, index i16}` = 20 bytes each |
  | `vis` | `{stand u16, crouch u16}` |

- **Flag mapping:**
  - Crouch(2) → stance hint;
  - Ladder(5) → ladder hint;
  - Lift(1), Button(0) → mechanism hint (NeedsAnnotation);
  - Goal(4) → item tag;
  - Camp(7) with start/end yaw → place with a view sector;
  - Sniper(28) → place tag `sniper`;
  - Narrow(10) → ignored (recomputed);
  - team, rescue, hostage, doublejump → dropped;
  - `PathFlag::Jump` → Jump candidate with a velocity hint.
- **Revalidation:**
  - each node is projected to the floor (hull 1 down 96 u; if start-solid, hull 3 from origin−18), because yapb heights are inconsistent;
  - every link is reclassified with the TraversalValidator;
  - failures stay `Imported|Unvalidated` and are excluded from planning unless `lb_nav_trust_imported 1`.
- The `mapSize` versus BSP size mismatch is only a warning.
- **Fixture:** `xash3d-fwgs-apple-arm64/valve/addons/yapb/data/graph/crossfire.graph` has 1598 nodes, options `0xD8`, 58735 → 351560 bytes, trailer mapSize 1241704, which equals crossfire.bsp.
- PWF import (optional): 80-byte "PODWAY!" header plus 204-byte records (`graph.cpp 1688–1765`, `2818–2840`).

---

## 3. Generator (worker thread; the same code runs in `lb-navtool` and in tests)

**Pipeline** (inputs: `GenJob{world: Arc<BspWorld>, phys, rules, overlay_nav, opts, cancel, progress}`; each stage is deterministic and seeded by the BSP hash)

- **A. Index** (about 0.1 s):
  - typed entities, spawn poses, mechanism graph;
  - walkable faces (normal.z ≥ 0.7);
  - seed set: spawns, items after drop-to-floor, ladder tops/bottoms, teleport destinations (floor at `dest.z + 1 − mins.z`), trigger touch spots, door sides, `info_node` entries (datacore, lambda_bunker and subtransit have 70–179 of them), and face samples on a 64 u grid.
- **B. FloorField flood** (2.5-D multi-layer grid, cell 16 u):
  - one BFS over (cell, layer) from all seeds using a PM-like step move: hull sweep; if blocked, step up ≤ `stepsize` (18), move, step down; ground normal.z ≥ 0.7 as in `PM_WalkMove` (`pm_shared.c 1034–1193`);
  - two passes: hull 1 (Stand), then hull 3 (Crouch-only spans);
  - spans in the same column are separate layers if dz > 18 and there is ≥ 36 u clearance between them;
  - edges where the neighbour is lower by more than 18, or missing, are marked as Drop candidates.
  - The BFS frontier is expanded in rayon batches with an ordered merge.
  - Movers are removed and their swept volumes marked as `Gate(e)`; breakables are marked `Gate(e)` (with a removal pose); illusionary volumes crossed are marked `Illusionary(e)`.
  - This replaces the yapb analyzer (`analyze.cpp`), fixing: the unreachable direction 8, the inverted throttle (357–363), the stale crouch flag, cleanup erasing every node that links to node 0 (261), and merge by stale indices.

  ```rust
  pub struct FloorSpan { z: f32, ceil_q: u8 /*2u*/, normal: [i8;3], flags: SpanFlags, edge_q: u8 /*2u*/,
                         nbr: u8 /*8-dir walk bits*/, drop: u8 /*8-dir drop bits*/, node: NodeId, node2: NodeId, walk_q: u16 }
  ```

- **C. WaterField:**
  - 32 u voxels where waterlevel ≥ 2 (world contents plus `func_water` skin);
  - surface cells, where `PM_CheckWaterJump` can succeed (wall within 24 u at +8, clear at the top; `pm_shared.c 2617–2679`), become WaterExit candidates.
- **D. Nodes:**
  - mandatory nodes: spawns, item anchors, interaction spots, ladder mounts, special take-off/landing points, teleport destinations, lift boarding points;
  - then ridge nodes of the edge-distance transform (corridor centre lines), spacing 64;
  - then Poisson fill with `r = clamp(1.2·edge_dist, 48, 128)`;
  - each span is assigned `node`/`node2` by multi-source walk-Dijkstra over spans. This is what powers O(1) `ProjectReachableAnchor`.
- **E. Walk/Crouch links:**
  - candidates within 384 u (grid query);
  - accepted if `FloorField::line_walk(a,b,stance)` passes (a DDA over neighbour bits, per-cell dz ≤ 18, edge_dist ≥ 16+2) and `TraversalValidator::walk` confirms with hull sweeps every 16 u;
  - links are directed, since step-up/step-down are asymmetric.
  - **Pruning is a sound greedy t-spanner:** sort candidates by (length, key); keep a→b only if the current graph's a→b cost exceeds `t·cost` (t = 1.15, bounded Dijkstra). Connectivity is kept and stretch is ≤ 1.15. Out-degree is capped at 12. Special links are never pruned.
  - This replaces `clearConnections` (`graph.cpp 40–406`), which removes links by angle heuristics without checking that an alternative route exists (and has the pass-3 `360−a−b` bug).
- **F. Special candidates**, all later validated in G:
  - **Drops:** for each edge span, walk off at {80, 160, 320} u/s at 0° and ±30°. The landing span comes from the FloorField plus a sweep. Damage is from `DamageRules`; landing in water is free; landing in a hazard is rejected; links are one-way.
  - **Jump / CrouchJump:** only between spans with horizontal gap ≤ 256 and dz ≤ +63, where walk distance > 2× straight distance or the target is unreachable. The take-off is chosen along the runway for maximum robustness.
  - **LongJump:** 250–600 u, only where a normal jump fails and time saved is ≥ 30%. The generator also precomputes longjump launch windows on straight Walk chains (see §6).
  - **Ladders:**
    - per `func_ladder`: find the accessible face (thin axis, the side where hull 1 fits);
    - bottom mount within 24 u; top exit found by simulated climb until leaving the ladder volume, then forward with step-up; no ledge means NeedsAnnotation; intermediate floors become mount/dismount points;
    - `LadderBoard`, `LadderClimb` (both directions) and `LadderExit` links.
  - **Water:** Swim, Surface, Dive links (with an air budget: 12 s air minus a margin) and WaterExit.
  - **Doors:** a second flood with movers in their open pose; each gate crossing becomes a Door link.
    - Interaction:
      - no targetname and not USE_ONLY → Touch;
      - USE_ONLY → Use (use spots computed with the exact PlayerUse model at radius 56, cone 0.8, and the "highest dot wins against other usable objects nearby" rule);
      - targetname → activators from the mechanism graph. A remote button or plate becomes a `Sequence` whose detour cost and open window (open travel + wait) are checked for feasibility.
      - Never activated → permanently solid.
    - Rotating doors: the swing volume becomes a "wait outside" region.
  - **Lifts/plats:**
    - movers with vertical `movedir` and a walkable top of at least 48×48 (func_door lifts, func_plat);
    - boarding anchor at the bottom pose, exit at the top pose;
    - activation: button use-spot reachable from the platform, the plat trigger field, or self-touch;
    - the exit window is the mover's wait; `func_plat` stays up while it is occupied, so it is Up-only and the way down is a Drop into the shaft when the shaft is open.
  - **Teleports:** entry is the touch spot inside the trigger hull; exit is the floor at the destination (first entity whose targetname matches the trigger's target, `triggers.cpp 1881–1935`); master-gated teleports get a dependency.
  - **Push:** `trigger_push` with speed ≥ 300 → approximate ballistic arc from basevelocity. Always `LivePending`, often `NeedsAnnotation` (e.g. the bounce pods at 2600–3800 u/s).
  - **Breakables:** a flood with the breakable removed; any new connection becomes a Breakable link. Cost is time-to-break from health and weapon DPS; flag 256 means crowbar-instant; material 7 (unbreakable glass) is treated as solid; `trigger_only` is excluded.
  - **Secrets:**
    - links crossing an opaque vertical illusionary volume become SecretPass;
    - a door is a secret candidate if (a) its textures are camouflaged (they intersect world textures within 64 u and are not in a door-texture list), or (b) it is a thin use-only panel (min dimension ≤ 16), or (c) it is opened by a remote trigger or button, or (d) its name matches `secret|trick|hidden|panel`;
    - small (≤ 8×8×8) or shootable buttons that gate something are flagged as hidden buttons.
  - **Boosts** (optional, only if `lb_nav_tricks` allows): for pairs of nodes more than 200 u apart that are unconnected, or whose path is more than 3× longer, sample Gauss/Rocket/Grenade/Satchel arcs from §4.
  - **Hazards and gravity:**
    - always-on `trigger_hurt` (dmg/s) and lava/slime contents become risk; lethal volumes are forbidden; toggled hurt volumes become conditional hazards (for scenarios);
    - `trigger_gravity` zones (rapidcore, xen_dm) change the arc model and the link becomes `LivePending`;
    - `func_conveyor` adds basevelocity to Walk cost; pushables are `DynamicGeometry`.
- **G. Validation** (parallel): see §4. Each verdict is stored with a robustness value; links with robustness < 0.8 are dropped.
- **H. Overlay nav patches:** apply, then validate with the same validator. A manual edit never bypasses validation (v2 §9.2).
- **I. Reachability and coverage:**
  - Tarjan SCC; forward reachability from spawns and backward reachability to spawns (directed);
  - components are not deleted even if isolated (v2 §9.3).
  - **Report** (YAML + JSON): walkable face area vs covered area, unreachable clusters (centroid, area), items unreachable, spawn "traps" (no way back), mechanisms (supported / annotated / unsupported with reason, e.g. `func_tracktrain`), special links (valid / live-pending / live-failed), secret candidates, dead links, overlay patch results, per-stage timings.
- **J. Precompute:**
  - arrival radius `clamp(edge_dist−16, 8, 64)`; link width `2·min edge_dist`;
  - **vis table:**
    - eyes: Stand = floor+64, Crouch = floor+30 (as in yapb `vistable.cpp 33–43`, reproduced);
    - targets: body (floor+36/18) and head;
    - PVS pre-cull, then `trace_line` with `WORLD|STATIC_BRUSH|MOVERS@spawn|OPAQUE_ILLUSIONARY`;
    - 2 bits per pair, dense for N ≤ 4096, sparse otherwise;
  - tactical precompute (§7); chokepoints; mine spots; ALT landmarks (§5).
- **K. Serialise and publish, in three steps:**
  - v0 after E+I (Walk-only) — bots can move within a couple of seconds;
  - v1 after G/H (specials);
  - v2 after J (tactical);
  - then incremental publishes as live validation completes.
  - Until v0 exists, bots hold position or fight locally and don't roam (v2 §9.3).

**Live validation queue**
- For each special link, `TraversalValidator::live_plan` produces a list of independent hull sweeps and point-contents checks with expected results (fractions, landing normal, contents).
- The main thread runs them within `lb_nav_live_traces_per_frame` (64) and `lb_nav_live_us_per_frame` (150).
- Outcomes: all match → `LiveConfirmed`; a mismatch → `LiveMismatch`, link disabled, report entry. Links with `DynamicGeometry` are checked at use time instead.

**Determinism and timing targets**
- Same inputs give a byte-identical `.lbnav`: sorted iteration, no hash-map iteration order dependence, ordered rayon reductions, RNG seeded from blake3.
- Crossfire-size maps: v0 ≤ 3 s, full ≤ 15 s on 4 threads.
- Big custom maps (rustmill, disposal, GunGame maps): ≤ 90 s, peak memory ≤ 300 MB.
- Cached load ≤ 50 ms.

---

## 4. Kinematic validator (`lb-kin`), simplified GoldSrc

**Parameters (defaults = vanilla HLDM; read from cvars at map load and hashed into the cache key)**

| Parameter | Value | Source |
|---|---|---|
| gravity | 800 | `sv_phys.cpp 49`; `pmove->gravity` multiplier from `trigger_gravity` |
| maxspeed | 320 | `sv_user.cpp 51` (plus client maxspeed) |
| accelerate / airaccelerate | 10 / 10 | `sv_user.cpp 52`, `sv_main.cpp 127` |
| friction / stopspeed / edgefriction | 4 / 100 / 2 | `sv_phys.cpp 52–53`, `sv_user.cpp 50`; edge friction applies when there is no ground 16 u ahead within 34 u down (`pm_shared.c 1202–1277`) |
| stepsize | 18 | `sv_phys.cpp 51` |
| ground slope | normal.z ≥ 0.7 | `pm_shared.c 1168` |
| jump vz | √(2·800·45) = 268.33 | `pm_shared.c 2596–2601` |
| fresh jump press required | `oldbuttons & IN_JUMP` blocks | `pm_shared.c 2554` |
| duck-in-air | hull swaps instantly, origin kept, so feet rise +18 (effective ledge ≈ 63) | `pm_shared.c 2019–2040` |
| duck speed multiplier / time | 0.333 / 0.4 s | `pm_shared.c 123, 77` |
| longjump | needs `slj`=1, bInDuck or FL_DUCKING, IN_DUCK held, flDuckTime > 0, speed > 50; horizontal velocity := forward_xy·560 (includes a cos(pitch) factor); vz = √(2·800·56) = 299.33; flDuckTime = 1000 ms on a fresh duck press | `pm_shared.c 2576–2593, 2007–2011` |
| air control | wish speed capped at 30 for the add | `pm_shared.c 1279–1313` |
| bhop cap | vanilla always 1.7·maxspeed, crop ×0.65; BHL: only if `mp_bunnyhop 0` | `pm_shared.c 2436–2467`; BHL 2756 |
| ladder | 200 u/s (×0.333 ducked); jump pushes off along the face normal ×270; FORWARD toward the face = up, BACK = down | `pm_shared.c 2059–2188` |
| swim | wish speed ×0.8; jump vz 100/80/50 (water/slime/lava); water jump vz 225 | `pm_shared.c 1321–1408, 2508–2541, 2670–2673` |
| fall damage | above 580 u/s: `mp_falldamage 0` = fixed 10; `1` = (v−580)·100/444; no damage when landing in water. Safe drop from rest 210.25 u; fatal (progressive) 655 u. | `player.cpp 2728–2757`, `multiplay_gamerules.cpp 463–478`, `player.h 22–26` |
| armor | ratio 0.2, bonus 0.5, bonus ×2 for blast in MP; armor does not reduce fall damage | `player.cpp 442–509` |
| knockback | `min(1000, dmg_after_armor·5·(32·32·72/size_volume))`, i.e. ×2 when crouched | `combat.cpp 893–896, 1020–1030` |
| explosions | radius = 2.5·dmg; adjusted = dmg·(1−d/r); RPG 100, hand grenade 100, satchel 150 (skill.cfg) | `combat.cpp 1038+`, `gamerules.cpp 251–264` |
| gauss | full charge 1.5 s in MP; 200 dmg; recoil −forward·dmg·5 (≤ 1000), MP keeps z; about 16 ammo (1 + 1 per 0.1 s); 10 s overcharge → 50 dmg | `gauss.cpp 46–58, 233–298, 314–369` |
| +use | new mode maxspeed/3; old mode velocity×0.3 per frame | BHL 3062/3242 |

**Integrator:**
- default `sim_dt` = 1/100 s (can be set to the server frame time);
- ground: friction → accelerate → walk move with step-up/down;
- air: half-gravity steps (`PM_AddCorrectGravity`/`PM_FixupGravityVelocity`), air-accelerate, fly move with `ClipVelocity` (overbounce 1);
- landing on a plane with normal.z ≥ 0.7 while vz ≤ 0; records impact speed;
- basevelocity from push fields and conveyors.
- Every step is a hull sweep through `Tracer`, so the same code runs on the BSP and on the live engine.

```rust
pub struct TraversalValidator<'a, T: Tracer> { t: &'a mut T, phys: &'a PhysicsProfile, dmg: &'a DamageRules, m: Margins }
impl<T: Tracer> TraversalValidator<'_, T> {
  pub fn walk(&mut self, a: Vec3, b: Vec3, stance: Stance) -> WalkVerdict;              // support every 16u, step, slope, hull fit
  pub fn drop(&mut self, edge: Vec3, dir: Vec2, speeds: &[f32]) -> DropVerdict;
  pub fn jump(&mut self, q: &JumpQuery /*takeoff runway, dir, run speed, crouch-in-air t, longjump*/) -> ArcVerdict;
  pub fn ladder(&mut self, g: &LadderGeom) -> LadderVerdict;
  pub fn swim(&mut self, a: Vec3, b: Vec3) -> SwimVerdict;
  pub fn boost(&mut self, q: &BoostQuery /*kind, aim α/yaw, stance, armor, health*/) -> ArcVerdict;
  pub fn live_plan(&self, v: &ArcVerdict) -> LiveCheckPlan;                              // sweeps + expectations
}
pub struct ArcVerdict { pub ok: bool, pub reject: Option<Reject>, pub landing: Vec3, pub t_flight: f32, pub apex_clear: f32,
  pub impact_speed: f32, pub damage: DamageEstimate /*per falldamage mode, with armor*/, pub robustness: f32, pub digest: ArcDigest }
```

**Margins and robustness**
- Margins: lateral 4 u (offset sweeps), head 4 u, landing inset 8 u.
- Perturbation set: run speed {−10%, 0}, yaw ±3°, take-off ±8 u along the runway, pitch ±2° for boosts.
- `robustness` = share of perturbations that succeed. It is stored and used as the uncertainty cost.

**Reuse:**
- the generator (G);
- anchor projection (walk from a free position to a node);
- editor link creation and "test link" (§10);
- runtime executor Prepare guards (re-check health/armor and the landing region before irreversible phases);
- live plans (§3).

A full `pm_shared` port can replace the integrator behind the same trait later.

---

## 5. Planner (`lb-nav::plan`)

```rust
pub struct PathRequest { pub bot: BotHandle /*(map_epoch,slot,gen)*/, pub start: StartSpec /*Anchor|Pos{p,v}*/,
  pub goal: GoalSpec /*Node|Nodes{set, per-goal extra}|Place|ItemAnchor*/, pub profile: CostProfile, pub caps: CapabilitySnapshot,
  pub resources: ResourceBudget { health, armor, ammo, reserve_health }, pub known: Arc<KnownChanges> /*versioned*/,
  pub versions: DepVersions { graph_build, danger, overlay_nav }, pub budget: SearchBudget { per_slice: u32 /*256*/,
  total: u32 /*20_000*/, deadline: SimTime }, pub anytime: bool, pub priority: u8 }
pub enum PathStatus { Pending{expanded: u32}, Complete(Arc<PlannedPath>), Partial(Arc<PlannedPath>, PartialWhy),
  NoPath(NoPathWhy /*Disconnected|ResourceInfeasible|CapabilityMissing|AllBlockedByKnown*/), Cancelled,
  Stale(StaleWhy), Failed(FailWhy /*Timeout|BudgetExhausted|Internal*/) }
impl PlannerService {
  pub fn request(&mut self, r: PathRequest) -> ReqId;           // coalesces equivalent requests
  pub fn cost_to_many(&mut self, q: CostQuery) -> ReqId;        // multi-target Dijkstra for utility shortlists
  pub fn poll(&self, id: ReqId) -> &PathStatus; pub fn cancel(&mut self, id: ReqId);
  pub fn tick(&mut self, budget: &mut FrameBudget);             // phase 5, main thread
}
```

- **Scheduling:**
  - a CPU budget per second (default 20 ms/s), sliced per frame (at 1000 fps that is ≤ 100 µs per frame);
  - priority, then age ("last served"), so distant bots don't starve;
  - there is no async worker racing the main thread (fixes `navigate.cpp 3510–3519`).
- **Search:**
  - A* ordered by f = g + h (yapb orders by g, `planner.cpp 210, 287`);
  - closed-set reopening is handled correctly;
  - `h` = ALT: 8–16 landmarks (farthest-point selection), forward and backward distances over base costs, computed on a worker at graph publish.
  - ALT is admissible even with teleports, because every dynamic cost ≥ base cost (known changes and risk only add).
  - Fallbacks: `h = 0` (Dijkstra) until landmarks are ready; Euclid/maxspeed only if the graph has no Teleport/Push/Boost links.
- **Cost** (seconds; all terms ≥ 0, v2 §9.4):
  `c = base_time/speed_factor(profile) + wait + w_risk·risk + w_danger·danger(node) + w_exposure·exposure(link) + w_hazard·hazard + w_unc·(1−robustness+failure_rate) + resource_penalty + known.modifier(link)`
  - Style "preferences" are penalties on everything else, never discounts.
  - Profiles: `default`, `sniper` (`w_exposure` 1.5, avoids open areas), `flee` (distance-from-threat term), `stealth` (avoids noisy links: ladders, water, lifts).
- **Resources:**
  - links whose single-step cost exceeds `health − reserve` are pruned during expansion;
  - the whole path is summed afterwards; if infeasible, re-plan (up to 2 times) with the worst resource links excluded; if still infeasible, `NoPath(ResourceInfeasible)`;
  - executors re-check before irreversible actions (v2 §9.4).
- **Known changes (per bot):**
  - `KnownChanges{version, by_link: Block{until, reason}|Penalty{add, until}, by_entity: MechanismBelief{state, observed_at, revert_at}, secrets_known}`;
  - only observations and the bot's own failures write to it, so remote hidden changes never propagate (v2 §9.5);
  - secret links are enabled only for `knows_secrets` bots or after the bot has observed that secret.
- **Coalescing and caching:** identical (start anchor, goal, profile class, capability class, known version, graph build) share a result; LRU of 256 `PlannedPath` with a version check.
- **Partial:** only for `anytime` requests (flee/cover) — the best explored prefix, labelled. Otherwise the request stays `Pending` until the deadline and then fails with `Timeout`.
- **PlannedPath:** segments `Walk{smoothable polyline}` or `Action{link, spec}`; versions; resource use.
  - `revalidate(&current)` runs before each action segment; a missing or changed `LinkKey` makes the path `Stale` (it is remapped by `LinkKey` when possible, to avoid churn).
- **Smoothing:** funnel algorithm inside consecutive Walk links using link widths and `FloorField::line_walk` (no traces). It never crosses Action preparation points or the boundaries of zones the planner chose to avoid (v2 §9.4).
- **`ProjectReachableAnchor`** (replaces nearest-node lookup, `navigate.cpp 1809–2091`, which used an X-ignoring bucket hash, top-3 slots all overwritten with the same value, and random picks):
  - FloorField column lookup → layer under `pos` → `node`, `node2` with precomputed walk distance → `AnchorResult::Found(≤3 candidates, proof = FloorFieldPath)`;
  - special cases: `Airborne` (predict landing), `OnLadder`, `InWater` (WaterField), `OffMesh` (nearest span within 64 u plus a live hull trace to recover);
  - item pickup anchors are precomputed: a validated walk into the pickup overlap region (item bounds ±1 u).

---

## 6. Path following and traversal executors (cheap per frame)

```rust
pub struct PathFollower { path: Arc<PlannedPath>, cur: CorridorCursor { seg: u16, pt: u16, s: f32 }, exec: ExecSlot /*enum, no alloc*/,
  progress: ProgressMonitor, steer: LocalSteering, fails: FailureLog }
impl PathFollower { pub fn tick(&mut self, s: &SelfState, local: &LocalPerception, nav: &NavView, now: SimTime) -> NavOutput; }
pub struct NavOutput { pub intents: NavIntents /*MoveIntent{world_vel, max_speed, precision}, LookIntent{target, strength:
  Weak|Required, tol}, StanceIntent, Buttons{jump,duck,use: Press|Hold|Release}, WeaponIntent (boosts only), channels,
  interruptible, deadline*/, pub status: FollowStatus, pub requests: SmallVec<[ActionRequest;1]> /*DestroyEntity, …*/ }
```

**Budget:** the steady-state tick has no engine traces (FloorField only), about 5 µs per bot. Avoidance runs at 30–60 Hz; stuck checks at 10–20 Hz; a local trace budget of ≤ 2 traces per 50 ms per bot.

**Reach tests** (yapb `navigate.cpp 1333–1419`, fixed)

| Case | yapb | lambdabots |
|---|---|---|
| Default | 48 or radius | look-ahead switching (L = clamp(0.35 s·speed, 48, 160)) plus a "passed plane" test; tolerance max(arrival_r, 24) |
| Crouch | 6 | 12 + passed plane |
| Ladder node / on ladder | 6 / 15 | board 12; on ladder: z ±12 and face distance ≤ 20 |
| Jump link | 0 / 8 | take-off window [s0, s1] + yaw/speed gates; landing = region + onground |
| Goal | 25 | pickup-overlap containment |
| Precise pass | predicted next-frame distance (1388–1394) | kept |
| Longjump flight | 2-D 50 (1414–1417) | kept |

**Look:** Walk proposes a `Weak` look toward the path at eye height. Ladder, Use, LongJump and Boost propose `Required`, and the arbiter grants these atomically.

**Local steering and avoidance:**
- sample 12 velocities (±15/30/60°, speeds 100/60/0%) inside the corridor width;
- cost = deviation + time-to-collision penalty (τ = 0.8 s) for perceived players (visible or heard) plus physical contacts;
- applies to all players in both TDM and FFA (yapb skipped FFA, `navigate.cpp 220`).
- **Yielding:**
  - links with width < 48 and ladders take `PassageReservation{bot, dir, t_in, t_out, prio}`;
  - a conflicting reservation from a higher-priority, opposite-direction bot means wait at the `YieldSpot` for up to 3 s, then re-plan with a `TemporarilyOccupied` penalty;
  - priority: urgent action (e.g. running to shelter) > already inside > stable slot tiebreak;
  - bots yield to approaching human teammates when TTC < 1.5 s;
  - same-direction ladder use keeps ≥ 80 u spacing (replaces the yapb 3 s pause and re-route, 1210–1232 and 2605–2615);
  - reservations expire and never replace real collision checks.

**Stuck and recovery:**
- `ProgressMonitor` per phase: expected rate = 0.3·speed; window 0.75 s; below 25% means suspected stuck.
- Cause classification: physical contact or a perceived player ahead → `TemporarilyOccupied`; one hull trace in the move direction → `GeometryInvalid` candidate; waiting on a mechanism → `WaitingForInteraction`; look/aim not achieved → `ControllerFailure`; `MissingCapability`.
- Response ladder: sidestep inside the corridor 0.3 s → back off 32 u and retry → jump only if the FloorField shows a floor 18–45 u higher ahead and it is not a ledge or ladder → crouch if the ceiling is < 72 → re-plan with a TTL penalty → abort the goal.
- Never random jumps on ladders or at edges. This replaces `checkTerrain 343–654`, whose bugs were: `canJumpUp/canDuckUnder` always false (2924, 2995, 3044), probe index off-by-one (`kMaxCollideMoves` = 4, index ≤ 4), duplicate `isBlockedForward` traces from sign errors (2718/2731), and `setStrafeSpeed` treating a direction as a position (3258).

**Failure TTLs** (KnownChanges):

| Reason | TTL |
|---|---|
| TemporarilyOccupied | 4 s, +2 s per repeat, cap 20 s |
| WaitingForInteraction timeout | 15 s |
| MissingCapability | until the capability changes |
| ControllerFailure | 10 s·2ⁿ, cap 120 s |
| GeometryInvalid | 120 s, plus a global suspect count; ≥ 3 bots or 5 events in 10 min → global disable plus live re-check |

Topology changes only after a structural failure is confirmed (v2 §9.5).

**Fall recovery:**
- replaces `checkFall 656–725`;
- unplanned airborne with dz > 18 → on landing, `ProjectReachableAnchor` → continue if the next node is reachable, otherwise re-plan.

**Executors** — each implements `Approach → Prepare → Execute → Verify → Recover/Failed` with per-phase progress and deadlines. There are no velocity writes and no `MDLL_Use` (removed yapb cheats `navigate.cpp 1112, 1133–1137, 1267, 1626`).

- **Jump / CrouchJump:**
  - Approach: runway window with speed in [vmin, vmax] and yaw error ≤ 10°.
  - Prepare: jump released for at least 1 command.
  - Execute: press jump at s ≥ s0; CrouchJump holds duck from the spec time (vz < 150) until landing + 0.1 s; small air-steer.
  - Verify: onground and in the exit region within t_flight + 0.5 s.
- **Drop:**
  - Prepare guard: `health − dmg ≥ reserve`.
  - Controlled edge speed from the spec; verify landing.
- **Ladder:**
  - Board: face yaw = −face normal, move forward until movetype FLY.
  - Climb: FORWARD with pitch 0 to −30° for up; BACK for down (valid with the view level, per `PM_LadderMove`).
  - Exit at top: keep FORWARD until off the ladder, never jump (jump pushes off the ladder).
  - Exit at bottom: once onground, move away.
  - `HoldOnLadder` is the interruptible state that lets combat take Look.
  - Replaces `navigate.cpp 1160–1233`, where the ladder offset was always 0 because the trace ignored the ladder.
- **Swim / Surface / Dive / WaterExit:** 3-D steering with pitch, jump to rise; WaterExit needs waterlevel 2 facing the wall; the air budget aborts to the surface.
- **Door:**
  - Touch → walk in and wait.
  - Use → reach the use spot outside the swing volume, stop (< 20 u/s), aim until the cone test passes with margin, one-frame edge `IN_USE`.
  - Verify door motion (observed) within 1 s; never re-press while it is moving or for toggle doors; locked → `WaitingForInteraction`.
  - Remote → `Sequence` (go to activator → Use/Touch/Shoot request → pass within the window); skipped if the door is observed open.
- **Lift/Plat:** yapb lift states (1447–1802) mapped to v2 phases:
  - `ReachWaitingArea` (off the shaft) → `Call` → `WaitArrival` (observed stopped at our level) → `Board` → `VerifyBoarded` (self groundentity = lift) → `Activate` → `Ride` (centre, hold still) → `ExitWhenAligned` (within the wait window) → `VerifyArrival`;
  - timeouts are travel/speed + wait + 2 s; waiting is not "stuck";
  - fixes: missing `m_previousNodes[1]` (2011–2014), groove-fall detection → re-plan.
- **Teleport:** walk into the touch spot; verify a position discontinuity > 64 u near the destination within 1 s; re-anchor.
- **Breakable:**
  - `ActionRequest::DestroyEntity{ent, aim, health_est, melee_ok}` to the AI action runtime; the follower suspends at the attack spot;
  - verify the entity is gone (observed); timeout → `MissingCapability` or `ControllerFailure`.
- **LongJump:**
  - Guards: `slj` (self), onground, not FL_DUCKING, not in water or on a ladder, speed ≥ 100, |pitch| ≤ 5°, yaw error ≤ 2° (at 420 u of flight that is 15 u lateral), fresh duck and fresh jump in the same command, cooldown 0.9–1.4 s.
  - In flight: keep duck and forward, lenient 2-D reach.
  - Shortcut mode ports the yapb runway algorithm (`navigate.cpp 810–990`: runway ≥ 400, ≥ 3 points, corridor dot ≥ 0.92, legs ≥ 0.94, rise ≤ 40 / drop ≤ 64, speed2d ≥ 150).
  - The 250/+50 u headroom and 445/520 u deadly-drop traces are replaced by generator-precomputed launch windows, so there are no runtime traces.
- **Bhop** (as built, `lb_nav::hop`; see `docs/navigation.md`):
  - a locomotion mode of the follower on Walk links up to the first special link, sharp turn (> 60°), slope (> 8°),
    unfit node or the end of the way; no graph flag: every hop's flight is followed through live traces before the
    jump;
  - jump only when on the ground (fresh press, the first command back), optimal air-strafe in the air;
  - on every server (the user's call): where the rules say the server crops (or a crop was seen), 0.97 × 1.7 ×
    maxspeed at most; skill limits from the difficulty table (`bhop_speed`, `bhop_speed_uncapped`).
- **GaussBoost** (port of `tasks.cpp 1443–1570` and `combat.cpp 1283–1339, 1369–1399`):
  - Prepare: select gauss, stand still, hold ATTACK2 ≥ 1.5 s (ammo ≥ 16 + reserve).
  - Aim: look at `eyes + back·96cosα − up·96sinα`; α and yaw come from the validated arc, with jitter ±2° only within robustness margin (yapb used random 25–38°).
  - Execute: jump edge; release ATTACK2 on leaving the ground or after 0.25 s.
  - Hold aim for 0.2 s, air-steer, verify landing.
  - Cancel before release = dump shot (safe direction). Never hold longer than 8 s (overcharge at 10 s).
- **Rocket / Grenade / Satchel boosts:**
  - aim down per spec, crouch-jump, fire or detonate at the validated timing;
  - disabled by default (`lb_tricks`) and gated by the armor/health window from the damage model.
  - The yapb "satchel jump" (`tasks.cpp 969–1127`) is a mid-air throw attack, not a self-boost; it belongs to the AI core, not nav.
- **Scripted:** runs overlay scenario steps as the traversal (e.g. the doublecross panel).

---

## 7. Map knowledge services (`lb-mapknow`)

- **ItemRegistry:**
  - static spawns after drop-to-floor, pickup anchors, rules (HLDM: items 30 s, weapons 20 s, ammo 20 s, health charger 60 s, HEV charger 30 s — `multiplay_gamerules.cpp 45–47, 1034–1043`; `mp_weaponstay`), plus overlay overrides;
  - presence and timers are per-bot beliefs owned by the AI core.
- **TacticalPositioning:**
  - `cover(CoverQuery{me, threats: [ThreatEstimate{pos, uncertainty, stance}], max_cost, prefer})`:
    - threats are projected to 1–3 anchors each;
    - a candidate must not be visible from the threat anchors or their neighbours (stance-aware vis);
    - path cost from me must be < path cost from the threat;
    - score = cost + path exposure + (exits ≥ 2) + danger;
    - top 3 get a live line-of-sight confirmation;
    - this ports the sound core of `findCoverNode 2207–2328` and fixes the top-k fill, the practice normalisation and the stance-blind vis.
  - `hide_spots`: low visibility richness, ≥ 2 exits, off high-flow links.
  - `retreat_route`: a `flee` profile request.
  - `defend(target_area)`: ports the `findDefendNode 2093–2205` intent with correct top-k.
- **Spot catalogue** (precomputed per node and stance):
  - `flow(m)` = sampled Brandes betweenness over spawn/item pairs;
  - `sightline = Σ flow(m)·area(m)` over m visible from n with 700 ≤ d ≤ 3000;
  - `peek` = share of the sector visible standing but hidden crouched;
  - `exposure = Σ flow(m)` over m with d < 700 that can see n;
  - `exits` = outgoing links to nodes not visible from the main sector;
  - `height` = mean dz to the sector;
  - `score = 1.0·sight + 0.5·peek + 0.3·exits + 0.2·height/100 − 1.0·exposure − 0.5·danger`;
  - sectors come from yaw histograms (10° bins) of visible far nodes → top 2 `{yaw, pitch, range}` (replaces `getRandomCampDir 3171–3228`).
- **Chokepoints:** high flow plus width < 96.
- **Mine spots:** the geometry of `checkCornerTripminePlant 2379–2462`, precomputed:
  - turn ≥ 60° (dot ≤ 0.5);
  - `vis(prev, next)` false;
  - inner side from the sign of the cross-product z;
  - probe from `corner + out·45` along the inner normal for 80 u;
  - wall |normal.z| ≤ 0.3;
  - opposite wall within 250 u → `MineSpot{pos, normal, beam, flow}`;
  - at runtime only "is a mine already observed there".
- **Danger map:**
  - per-node EWMA of damage per visit, top-4 attacker areas per node, deaths per link;
  - half-life 15 min game time;
  - consolidated on a worker every 30 s (fixes "computed once per map", `practice.cpp 91–153`, and the wrong-team write at `navigate.cpp 1998`);
  - stored by `NodeKey` so it survives regeneration;
  - honest input only: the bot's own damage events, with the attacker area from the observed attacker or the damage direction plus the vis table; TDM team sharing only through the delayed team-message model.

---

## 8. Overlays (YAML) and scenario engine (`lb-ext`)

**Files:**
- `addons/lambdabots/data/maps/<map>/overlay.yaml` (hand-written, higher precedence);
- `editor.yaml` (machine-managed by the in-game editor);
- `templates/*.yaml`.
- Parsed with a span-preserving YAML parser (shared loader owned by the config architect). Unknown keys are errors with file:line; every resolved value keeps provenance.

```yaml
schema: lambdabots.overlay/1
map: crossfire
bsp: { size: 1241704, blake3: "<lb-navtool fills>" }   # mismatch → warn; bindings that fail are disabled
version: 7
imports: [templates/airstrike.yaml, templates/crossbow_ambush.yaml]
entities:                                   # stable bindings, must resolve to exactly one entity
  strike_trigger: { match: { classname: trigger_multiple, target: strike_mm, model: "*65" } }
  strike_shutter: { match: { classname: func_door, targetname: strike_ready_door } }
  bunker_entrance: { match: { classname: func_door, targetname: bunker_maindoor, model: "*5" } }
  secret_panel:   { match: { classname: func_door, targetname: secret_door } }
  secret_plate:   { match: { classname: trigger_multiple, target: secret_door } }
places:
  bunker_shelter:                          # generator clips to walkable spans ∖ hazard(strike_pain) ∖ door sweeps
    shape: { box: { min: [-330, -2600, -1900], max: [330, -1660, -1690] } }
    tags: [shelter, bunker]
  strike_approach: { shape: { point: [0, -2300, -1888], radius: 24 }, stance: stand }
  strike_retreat:  { shape: { box: { min: [-48, -2330, -1890], max: [48, -2290, -1880] } } }
  tower_west_top:
    shape: { point: [-368, -1600, -1312], radius: 24 }
    tags: [sniper, crossbow, lethal_during_airstrike]
    stance: crouch
    view_sectors: [ { yaw: [55, 125], pitch: [-2, 25], range: [600, 3200], label: courtyard, weight: 1.0 } ]
  tower_east_top: { shape: { point: [368, -1600, -1312], radius: 24 }, tags: [sniper, crossbow, lethal_during_airstrike],
                    stance: crouch, view_sectors: [ { yaw: [55, 125], pitch: [-2, 25], range: [600, 3200] } ] }
nav:
  patches:
    - op: annotate_mechanism           # usually auto-derived; overlay confirms and tags it
      entity: secret_panel
      kind: door
      activation: [ { touch: secret_plate } ]
      window: { opens_in: 0.47, stays_open: 2.0 }      # speed 200, travel 94, wait 2
      secret: { knowledge: knows_secrets }
    - op: forbid
      shape: { box: { min: [-40, -2272, -1890], max: [40, -2200, -1800] } }
      when: { observed_entity_state: { entity: strike_shutter, state: closing } }
      reason: "shutter crush volume (dmg 9999)"
    - op: add_link                     # example manual link; validated like generated ones
      from: { at: [120, 64, -1696] }
      to:   { at: [210, 64, -1650] }
      traversal: { type: crouch_jump }
rules:
  item_respawn: { item_longjump: 30 }
  features: { gauss_boost: true, rocket_boost: false, satchel_jump: false, bhop: auto }
  knowledge: { secrets_default: profile }       # profile | all | none
scenarios:
  airstrike:
    template: airstrike@1
    params:
      trigger: { entity: strike_trigger, action: touch }
      shutter: strike_shutter
      entrances: [bunker_entrance]
      hazard_targetname: strike_pain
      siren: "ambience/siren.wav"
      first_arm: 180                          # trigger_auto→relay(180)→strike_timer_mma
      timeline: { shutter_close: 2, entrance_close: 20, entrance_stand_until: 29.2, entrance_crouch_until: 36.4,
                  siren_off: 45, strike: 52.0, strike_end: 52.1, entrance_reopen: 64, rearm: 235 }
      shelter: bunker_shelter
      approach: strike_approach
      retreat: strike_retreat
      retreat_deadline: 1.5
      hold_until: 60
    utility: { base: 0.25, chance_per_decision: 0.25, style: { rusher: 1.2, camper: 0.8, sniper: 0.6 } }
    cooldown: { per_bot: 300, global: 235 }
    team_policy: { tdm: announce_then_trigger, announce: "Airstrike in 50s - bunker!", wait_after_announce: 8,
                   skip_if_known_teammates_outside: 1 }
  ambush:
    template: crossbow_ambush@1
    params: { spots: { tags: [crossbow], include_auto: true, min_score: 0.6 },
              avoid_tags_when: { lethal_during_airstrike: { recent_event: airstrike_siren, within: 60 } } }
```

**Template `templates/airstrike.yaml`** (a data template with `${…}` parameter substitution only, no code)

```yaml
template: airstrike@1
goal:                                     # proposes a GoalCandidate to AI-core utility
  requires: { all: [
    { time_since_map_start: { gte: "${first_arm + 3}" } },
    { not: { recent_event: { id: airstrike_fired, within: "${timeline.rearm + 5}" } } },
    { any: [ { observed_entity_state: { entity: "${shutter}", state: open, max_age: 20 } },
             { belief: { key: "airstrike.armed_estimate", gte: 0.7 } } ] },
    { health: { gte: 40 } }, { enemies_visible: { eq: 0 } } ] }
  steps:
    - move_to: { place: "${approach}", stance: stand, deadline: 60 }
    - wait_until: { cond: { observed_entity_state: { entity: "${shutter}", state: open } }, timeout: 3 }
    - touch_zone: { entity: "${trigger.entity}", spot: auto,
                    verify: { heard_sound: { sample: "${siren}", within: 2 } }, timeout: 4 }
    - move_to: { place: "${retreat}", deadline: "${retreat_deadline}", style: sprint }
    - emit_event: { id: airstrike_fired, scope: [self, team_if_tdm] }
    - hold: { place: "${shelter}", until: { time_since_event: { id: airstrike_fired, gte: "${hold_until}" } },
              look: { sector: toward_entrances }, on_enemy: engage }
reaction:                                 # any bot hearing the siren
  on: { heard_sound: { sample: "${siren}" } }
  when: { not: { in_place: "${shelter}" } }
  decide: { eta_to_place: { place: "${shelter}", via_entrances_by: { stand: "${timeline.entrance_stand_until}",
                            crouch: "${timeline.entrance_crouch_until}" }, margin: 3 } }
  if_feasible: { priority: urgent, steps: [ { move_to: { place: "${shelter}", style: sprint } },
                 { hold: { place: "${shelter}", until: { time_since_event: { sound: "${siren}", gte: "${hold_until}" } } } } ] }
  else: { policy: continue }              # nothing else is safe
```

**Condition vocabulary:**
`all/any/not`, `time_since_map_start`, `periodic{period, window, anchor}`, `observed_entity_state{entity, state, max_age}`, `heard_sound{sample, within}`, `inventory{has, ammo}`, `health`, `armor`, `enemies_seen{within, count}`, `enemies_visible`, `in_place`, `game_mode`, `gg_level`, `style`, `knows_secrets`, `chance` (per decision epoch, deterministic RNG stream), `cooldown`, `recent_event`, `belief{key}`, `eta_to_place`.

**Step vocabulary:**
`move_to{place|entity|pos, stance, style, deadline}`, `wait_until`, `use_entity`, `touch_zone`, `shoot_entity`, `hold{duration|until, look sector/place/entity, stance, on_enemy: engage|ignore|break, scan}`, `crouch`, `stand`, `jump`, `look_at`, `select_weapon`, `zoom`, `emit_event`, `set_memory{key, ttl}`, `say{team}` (later).

**Engine:**
- `ScenarioModule` (a `BehaviorModule`) evaluates `requires` at utility ticks against `DecisionView` (beliefs only; static map facts allowed) and emits `GoalProposal{id, utility, commitment, steps}`.
- The AI core converts it into its `GoalCandidate`; once selected, `ScenarioAction` runs the steps through the primitive library.
- Reactions are event-driven.
- Per-bot scenario blackboard (TTL) and cooldowns.

**Validation** (`lb-navtool validate-overlay` and at load):
- schema;
- bindings unique;
- places on walkable spans (warning otherwise);
- nav patches pass the validator;
- step places reachable from spawn anchors;
- deadline feasibility — e.g. a "shelter reachability map": the share of nodes whose reverse-Dijkstra ETA to the shelter is ≤ 29.2 s standing.

**Hot reload:**
- `lb overlay reload` or `lb_overlay_autoreload` (mtime poll every 2 s);
- parse and validate on a worker, apply at a frame boundary; on error the old version stays;
- nav-hash changes trigger incremental regeneration or revalidation; behaviour-only changes need no regeneration;
- running actions keep their snapshot until a safe point (v2 §11).

---

## 9. Reference extensions

**9.1 Airstrike (crossfire; the same template for doublecross)**
- Facts are in §0 items 1–2.
- A Rust helper precomputes:
  - lethal node set = nodes inside any hull-expanded `strike_pain` volume;
  - shelter set = `place` ∩ walkable ∖ lethal ∖ door-sweep volumes;
  - `eta_to_shelter[node]` via reverse Dijkstra from the shelter, so reaction decisions are O(1);
  - an entrance-gated ETA: the entrance link's allowed stance changes at 29.2 s and 36.4 s, and it is forbidden after 36.4 s (crush dmg 100000).
- **Honesty:**
  - static knowledge covers the timings, the 180 s first arming and the 235 s rearm period;
  - the shutter's open/closed state comes only from observation; otherwise a belief (`armed_estimate`) derived from the observed siren/strike time and those static timings;
  - other bots learn of a strike from the siren (`EmitAmbientSound` hook) or a team message.
- **TDM policy:**
  - `announce_then_trigger`: say_team, wait 8 s, then trigger only if the bot's known teammates outside ≤ 1;
  - alternatives `never` and `enemy_majority`;
  - FFA has no restriction.
- **Execution details:**
  - touch spot is found automatically from the trigger hull (stand y∈[−2242, −2241), crouch y∈[−2242, −2237));
  - retreat must happen within 1.5 s;
  - hold until +60 s;
  - leave after +78 s, when the entrance is standing-passable again.
- **Doublecross instance:**
  - trigger is Use on `fire_button_texture` (0, 3176, −2006);
  - entrance stand-until 42.6, crouch-until 49.8 (travel 110 at 5 u/s from +35);
  - strike at 67; rearm at 250;
  - add `leave_place: button_room, deadline: 4` because of the poison from +5 s.

**9.2 Secrets (generic)**
- Mechanism:
  - generator candidates (§3.F);
  - overlay confirmation or tags;
  - `SecretPass` / `Door{secret}` / `Scripted` links with `Secret` set;
  - knowledge gate: the profile's `knows_secrets`, or learned in-session when the bot observes a secret opened or used.
  - `SecretsModule` also proposes goals when a secret area holds items.
- Samples from the entity lumps:

| Map | Secret | Mechanism | Notes |
|---|---|---|---|
| crossfire | `secret_door` func_door `*63` at (648, 1480, −1808), 16×80×96, texture `c2a1_w1` (camouflaged wall) | `trigger_multiple` `*64` at (848, 1304, −1816), wait 10 | Door moves down 94 u at 200 u/s, open 2 s. Trigger→door is 266 u (0.83 s), so the ~2.5 s window is feasible → `Sequence{touch, pass}`. |
| stalkyard | "trick" crate door at (−480, 1228, 32), 64×8×64 | touch plate right in front (−480, 1234, 26); wait 10 | Slides 48 u at 50 u/s. |
| stalkyard | use-only crate panels (380, 1416, 64) (sf 288, toggle) and (1010, −304, 32) (sf 256) | +use | — |
| stalkyard | crowbar vent grate (−208, −228, 184), 64×8×48, sf 256, health 1 | Breakable | Crouch passage. |
| stalkyard | horizontal vent grates (0, 32, 156) and (168, −368, 132), health 1 | Breakable | Drop-through floors. |
| snark_pit | `secret_gate` (400, −852, −224), wait 5 | activators: button `secret_gate_sw` (400, −1324, −224), or button (−368, −36, 56) → `weapon_door_mm` → uses `secret_gate_sw` | Button-used-by-multi_manager chain. |
| doublecross | `panel` (−436, −1024, −1664), wait 2 | multisource `panel_ms` = use button (−476, −830, −1572, 8×4×8) AND shootable button (−252, −1024, −1632, health 1) | Toggle-parity hidden state → `Scripted`: use A, shoot B, verify observed motion within 1 s; if not, repeat A and B once; give up after 2 cycles; the cost includes uncertainty. |

**9.3 Crossbow ambush**
- **Candidates:** auto spots (§7) with crossbow range 500–2500 (zoomed MP crossbow is hitscan) plus `crossbow`/`sniper` tagged places.
- **Conditions:** has crossbow and ≥ 5 bolts (or sniper style); health ≥ 50; no visible enemies; not in a hazard window.
- **Steps:**
  1. `move_to` using the `sniper` cost profile;
  2. `hold{scan: sector yaw ±(4–8)° with human-like dwell 1.5–3 s, stance: place stance, zoom: on_contact_or_idle}`;
  3. engage through AI combat.
- **Relocate** after 1–2 shots, or when detected (took damage, recognised enemy aiming our way, shots heard within 600 u). The new spot must not be visible from the current spot's main sector.
- **Rotation:** per-spot cooldown 120 s per bot; global penalty for spots used by any bot in the last 60 s; hold duration drawn from 20–45 s per bot RNG stream.

---

## 10. In-game editor

**Acquisition:** the listen-server host automatically; on a dedicated server, `lb edit acquire` with an admin allowlist (config). One editor at a time.

**Commands:**

| Command | Effect |
|---|---|
| `lb edit on\|off` | enable/disable editor mode |
| `lb edit show nodes\|links\|places\|sectors\|mechanisms\|danger\|coverage\|live [on\|off]` | toggle visualisation layers |
| `lb edit place add <id> [radius] [tags…]` | capture a place from the editor's floor position and stance |
| `lb edit place tag <id> +sniper -hide` | edit tags |
| `lb edit place sector add <id> [fov=60] [range=3000]` | capture a sector from the current yaw/pitch |
| `lb edit place poly begin\|point\|end` | polygon capture |
| `lb edit zone box <id> corner1\|corner2` | box capture |
| `lb edit bind <alias>` | bind the aimed entity (entity trace → `match` generated from class, name, model) |
| `lb edit link add <walk\|crouch\|jump\|crouch_jump\|drop\|ladder\|longjump\|…> [params]` | from the nearest node to the aimed node or point; the validator runs immediately and the result (verdict, reason, arc) is drawn |
| `lb edit link remove\|test\|info` | remove / validate / describe |
| `lb edit forbid <radius> [reason]` | forbid an area |
| `lb edit record jump start\|stop` | records the editor's own jump (take-off, velocity, landing), then validates it into a spec; replaces yapb jump-learning (`graph.cpp 2058–2078`) |
| `lb edit path <from> <to>` / `lb edit path here there` | plan with a chosen profile and draw segments and actions |
| `lb edit undo\|redo` | edit history |
| `lb edit save` | writes `editor.yaml` (canonical order) |
| `lb overlay reload` | reload overlays |
| `lb nav regen [full\|incremental]` | regenerate |
| `lb nav status\|report` | status and coverage |

**Visualisation:**
- `TE_BEAMPOINTS` sent `MSG_ONE_UNRELIABLE` to the editor only;
- budget ≤ 64 beams per 0.3 s refresh; cull by 1024 u and a 100° view cone; priority: selected > aimed > nearby; coordinates within ±4096 (WRITE_COORD);
- colours: Walk green, Crouch dark green, Jump orange, Drop red with arrow, Ladder brown, Door blue, Lift purple, Teleport cyan, Secret magenta, Boost yellow; unvalidated grey noisy beams; LiveMismatch white blinking;
- nodes are vertical bars (72/36 tall); zones are outlines; sectors are fans;
- HUD through `TE_TEXTMESSAGE` on 4 channels: status, aimed object, last result, path test. No ShowMenu.

---

## 11. Lua readiness

```rust
pub trait BehaviorModule: Send {
  fn id(&self) -> &str; fn api(&self) -> ApiVersion;                           // semver, api 1.x
  fn on_map_load(&mut self, map: &MapView, reg: &mut Registrar) -> Result<()>; // static places, subscriptions
  fn propose(&mut self, ctx: &DecisionView, out: &mut GoalSink);               // utility tick, budgeted
  fn on_event(&mut self, ev: &BotEvent, ctx: &DecisionView, out: &mut GoalSink);
}
```

- **Views (read-only, stable, versioned):**
  - `self` (health, armor, inventory, stance, place);
  - `beliefs` (enemies, items, mechanism beliefs);
  - `map` (places, entities by alias, spots, `eta(place)`);
  - `rules`; `time`.
  - Outputs are only `GoalProposal`s built from the same primitive step library that YAML uses. There is no raw entity or input access (v2 §11).
- The built-in airstrike, secrets and crossbow ambush modules are written against exactly this API, so they double as the Lua conformance suite.
- **Lua module shape:** a file returns `{api = "1.0", on_map_load = fn, propose = fn, on_event = fn}`; steps are available as `lb.step.move_to{place = "x"}`.
- **Sandbox:**
  - no `io`/`os`/`debug`/`require` except whitelisted libraries;
  - `set_hook(every_nth_instruction(1000))` to enforce ≤ 50k instructions and ≤ 0.2 ms per call;
  - `set_memory_limit(8 MB)` per module;
  - errors are caught (pcall); 3 errors in 60 s disables the module for the map, with a report;
  - native bindings are budgeted themselves, e.g. `eta()` goes through the planner request API.

---

## 12. Tests

- **Offline on real BSPs** (skipped if `LB_MAPS_DIR` is absent):
  - parse all Steam DM maps;
  - tracer parity against golden engine dumps: `lb dbg tracedump` on the stand records (start, end, hull, result) for positions sampled from the FloorField; offline comparison requires fraction within 1e-3 and equal flags/normals;
  - entity typing and mechanism graph: crossfire dead links, gasworks "dontopen", doublecross multisource;
  - crossfire touch-spot derivation matches §0 item 1;
  - `.lbnav` determinism (byte-identical);
  - timing and memory limits.
- **Synthetic worlds:** an AABB box world implementing `Tracer` for unit tests of stairs, slopes, gaps, ledges, ladders, water, doors, lifts and teleports.
- **Validator:**
  - analytic checks: jump apex 45, crouch-jump ledge ≈ 63 minus margin, standing gap ≈ 215 u + hull, longjump ≈ 419 u flat / apex 56, safe drop 210.25 u, fixed-10 vs progressive damage, knockback with armor;
  - comparison against stand recordings (`lb dbg jumprec`).
- **Planner:**
  - property tests (proptest) — A* with ALT equals reference Dijkstra on random directed graphs with teleports, one-way links and dynamic penalties;
  - resource infeasibility detection;
  - stale/cancel/version semantics;
  - no starvation across 12 bots.
- **Importer:** `crossfire.graph` → 1598 nodes, ULZ round-trip, fuzzed decoder; revalidation statistics.
- **Overlays:** schema error paths, binding ambiguity, template expansion, doublecross vs crossfire instances, hot-reload rollback.
- **Stand (live) per acceptance map:**
  - scripted spawn→item routes with success ≥ 95% and time ≤ 1.3× planned;
  - stuck/min ≤ 0.2 per bot;
  - v2 scenarios: lift wait is not stuck, a closed mechanism produces a diagnosable failure, smoothing doesn't cut pits or buttons, long frames and 500/1000 fps parity;
  - coverage thresholds: ≥ 95% of walkable face area reachable from spawns on standard maps, all items reachable or explained, no unvalidated link used by any plan;
  - a GunGame batch coverage table.

---

## 13. Milestones and risks

| Milestone | Scope | Verification |
|---|---|---|
| **M1 slice** | lb-bsp minimal (hulls, trace, entities); yapb import with revalidation plus manual links; Walk/Crouch/Jump/Ladder executors; main-thread Dijkstra; follower with reach tests; basic stuck handling | tracer parity on crossfire/stalkyard; bot runs imported crossfire routes including ladders; no cheats (grep for velocity writes/MDLL_Use in CI) |
| **M2 traversal contracts** | TraversalSpec, all executor phases; validator walk/jump/drop/ladder/swim; doors (touch/use/remote); failure reasons and TTLs; live-check queue | M2 obstacle map set: each traversal has tested success and failure; long-frame test leaves no stuck buttons |
| **M3 auto-graph + overlays + budgeted search + cache** | FloorField generator, specials (drops, ladders, water, doors, lifts, teleports, breakables), mechanism graph, vis, coverage report, ALT, KnownChanges, YAML places/patches, basic editor, `lb-navtool` | crossfire/stalkyard/boot_camp routes pass; coverage report lists unsupported mechanisms; cache key invalidates correctly |
| **M4** | map knowledge: cover/hide/sniper/chokepoints/danger; crossbow ambush | cover correctness against vis ground truth; ambush relocation and rotation metrics |
| **M5** | scenario engine + airstrike (crossfire/doublecross) + secrets; longjump/bhop/gauss; other boosts behind flags | airstrike stand scenario: presser survives, other bots reach the shelter when ETA is feasible; secrets gated by profile |
| **M6** | full editor, observer/web export, GunGame map batch | editor workflow creates a validated overlay end-to-end |
| **M7** | Lua via mlua over the same API | built-in modules re-implemented in Lua pass the same tests |

**Risks:**
- Offline tracer vs engine differences (rotated/origin-brush entities, clip brushes, precision). Mitigation: golden trace parity plus live confirmation of special links.
- Kinematic model vs real physics at 500–1000 fps (msec quantisation, `RunPlayerMove` semantics). Mitigation: margins, robustness value, stand recordings, optional full pm port.
- Generator time/memory on huge custom maps under the 32-bit address space. Mitigation: 32 u cell fallback, sparse vis, progressive publish.
- Mechanism diversity (tracktrain, push pads, gravity zones, hidden-state multisource). Mitigation: NeedsAnnotation plus overlay; never guess.
- Honesty leaks through local traces or mechanism reads. Mitigation: planners only read KnownChanges; direct entity reads are limited to local execution and diagnostics.
- Overlay drift across map versions. Mitigation: fingerprint plus binding diagnostics.
- Beam/HUD message budget on dedicated servers. Mitigation: unreliable channel, budgeted redraw.
- Ambient sound capture depends on the adapter hooking `EmitAmbientSound`.

---

### Critical Files for Implementation
- /Users/nikita/Git/half-life/yapb-halflife/src/navigate.cpp
- /Users/nikita/Git/half-life/yapb-halflife/src/graph.cpp
- /Users/nikita/Git/half-life/halflife/pm_shared/pm_shared.c
- /Users/nikita/Git/half-life/rehlds/rehlds/engine/world.cpp
- /Users/nikita/Git/hl-bots-plan/hl1_bot_architecture_v2.md
