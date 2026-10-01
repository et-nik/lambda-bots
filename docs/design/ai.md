# AI core: perception, beliefs, decisions, actions, combat, motor control, styles, modes

> Original design note (in English), prepared during planning on 2026-09-26.
> A condensed version with the final decisions lives in the implementation plan; in case of conflict, the plan
> and later decisions in the repository take precedence. `file:line` references reflect the sources as of the note's date.

# lambdabots: AI core design (Rust)

**Path prefixes:**
- `YAPB=/Users/nikita/Git/half-life/yapb-halflife`
- `BHL=/Users/nikita/Git/half-life/BugfixedHL-Rebased/src`
- `SDK=/Users/nikita/Git/half-life/halflife`
- `GG=/Users/nikita/Git/half-life/hl-gungame`
- `JK=/Users/nikita/Git/half-life/jk_botti`
- Baseline: `/Users/nikita/Git/hl-bots-plan/hl1_bot_architecture_v2.md`

## 0. Findings from the source that change the port

These come from reading the code, not from memory:

1. **Satchel buttons depend on the DLL.**
   - BHL: primary always throws. Secondary throws when idle and detonates when charges are out (`BHL/game/server/satchel.cpp:346-395`).
   - HL25 SDK: primary throws, secondary only detonates (`SDK/dlls/satchel.cpp:351-388`).
   - yapb assumed the old 2.3 SDK scheme ("secondary throws, primary detonates", `YAPB/src/tasks.cpp:960-964,1121-1124,1197-1200`). On BHL, yapb's "detonate" press throws another satchel.
   - The detonator only reaches charges within 4096u of the player.
   - Consequence: a per-DLL `DllProfile` is required.
2. **Hand grenade (HL25/BHL).**
   - Throw direction a' = −10 + pitch·(80/90 when looking up, 100/90 when looking down).
   - Speed = (90 − a')·6.5, capped at 1000, plus the player's velocity. Gravity = 0.5·sv_gravity.
   - The fuse is 3 s from the pin pull. The throw happens in WeaponIdle, which needs ≥0.5 s after the pull and the attack button released (`BHL/game/server/handgrenade.cpp:106-178`).
   - yapb's `calcThrow` (gravity·0.55, velocity·0.7793, `YAPB/src/combat.cpp:2425-2528`) is a hack. Its post-throw velocity overwrite is a cheat (`combat.cpp:2530-2551`).
3. **No server-side weapon recoil under CLIENT_WEAPONS.**
   - BHL compiles `wpn_shared/hl_wpn_glock.cpp` (`BHL/game/server/CMakeLists.txt:148`). Server `punchangle` only changes on landing or longjump (`BHL/pm_shared/pm_shared.cpp:2779,2938`).
   - So yapb's `needToPauseFiring`/`isRecoilHigh` (`combat.cpp:954-990`, `inc/yapb.h:888`) never triggers.
   - HL spread cones are constant: movement and bursts do not change accuracy.
   - Replacement: a spread-aware weapon and fire policy, plus a cosmetic "trigger discipline" knob.
4. **Gauss.**
   - Full charge takes 1.5 s in MP. Ammo cost is 1 at spin-up plus 1 per 0.1 s (about 16 total).
   - Damage = 200·t/1.5. Recoil in MP = −forward·dmg·5, including the vertical component. Overcharge zaps the holder for 50 at 10 s.
   - The shot fires in WeaponIdle: both attack buttons must be up, and not before 0.5 s after spin-up.
   - Pressing IN_ATTACK during a charge fires a primary shot and loses the charge. IN_ATTACK2 is checked first in `ItemPostFrame` (`BHL/game/server/weapons.cpp:646-699`).
   - At a shallow incidence (n = −N·dir < 0.5) the beam reflects and does radius damage of dmg·n with radius 2.5×. A gauss jump aimed less than 30° below horizontal can hurt the shooter (`gauss.cpp:169-300,430-575`).
5. **Damage message.** It carries the inflictor's *Center* (for bullets, the attacker's exact position). A human only sees the 4-way HUD compass (`BHL/game/client/hud/health.cpp:245-300`; `BHL/game/server/player.cpp:4655-4685`).
6. **Sounds.**
   - Weapon fire sounds are client-side events at ATTN_NORM (`BHL/game/client/ev_hldm.cpp`), so PlaybackEvent must be hooked.
   - Explosions arrive as TE_EXPLOSION to the PAS (`ggrenade.cpp:66-84`).
   - MP footsteps only play if `mp_footsteps` is on and (on a ladder or horizontal speed > 220). Cadence is 300 ms running, 350 ms on ladders. The jump sound follows the same speed rule (`pm_shared.cpp:430-460,715-830,2765`).
7. **Items.**
   - Respawn plays `items/suitchargeok1.wav` and sets EF_MUZZLEFLASH (`items.cpp:158-166`, `weapons.cpp:504-512`), so respawns are audible and visible.
   - Respawn times: items 30 s, weapons 20 s, ammo 20 s. Chargers recharge after 60 s (health) and 30 s (HEV) (`multiplay_gamerules.cpp:45-47,1034-1043`).
8. **Armor.** It absorbs 80%; each armor point is worth 2 health for bullets and 1 for blasts in MP. So EHP = h + 2a for bullets and h + a for blasts (`player.cpp:490-545`). Knockback Δv = 5·raw damage, capped at 1000 (`combat.cpp:871-874,997-1007`).
9. **Longjump.** Needs `slj`, duck held with flDuckTime > 0 (duck pressed ≤1 s ago), a fresh jump press, and |v| > 50. Result: v_xy = forward_xy·560 (less when looking up or down), v_z = 299 (`pm_shared.cpp:2750-2796`).
10. **BHL bunnyhop.** `mp_bunnyhop 1` (the default) means **no cap**. With the cap on, speed above 1.7·maxspeed is scaled to ·0.65 (`gamerules.cpp:45`, `pm_shared.cpp:2633-2662`).
11. **Ladders.** Only the direction buttons and view pitch matter. IN_JUMP detaches the player (`pm_shared.cpp:2288-2350`).
12. **Weapons.**
    - All weapons fire while the button is held.
    - When no fire button is held, the game auto-reloads and auto-switches.
    - Deploy takes 0.5 s (`weapons.cpp:975-990`).
    - The 357 has an MP zoom at fov 40 (`python.cpp:128-152`).
    - Crossbow zoom is fov 20. Zoomed MP shots are hitscan (`mp_dmg_xbow_scope` 120). Unzoomed bolts fly at 2000 u/s with no gravity and explode for 40 dmg at r=128 (`crossbow.cpp`).
13. **GunGame plugin.** The current `GG/scripting/gungame.sma` descores **only on self-kill** (2337-2352), sets frags = level·100 on level change (2759, 2786), blocks damage from non-inflictors (2486-2520), and blocks weapon/ammo/weaponbox pickups but **not** health, battery or longjump (`gungame.ini <blockspawn>`). Warmup kills don't count (2334). Crowbar-steal descore therefore has to be a configurable rule.
14. **yapb think rate.** yapb runs everything at ≤90 Hz (`YAPB/src/manager.cpp:17,1747`). Its newbie aim constants assume 90 Hz steps.

---

## 1. Crate layout and schedule

### 1.1 The crate graph enforces honesty at compile time
```
lb-types      math(Vec3,Angles), SimTime, ids(BotId,PlayerKey,TrackId,NodeId,ItemSpotId,MapEntityRef), WeaponId, Buttons, ChannelSet
lb-raw        RawWorldFrame/RawStimulus (integration-owned)                        -> only lb-perception, lb-brain may depend
lb-rules      RulesModel, DllProfile, WeaponMechanics, pickup rules, EHP, GgRules, SpawnProtRules
lb-nav-api    trait NavQuery + types (nav-owned; AI needs listed in 4.9)
lb-perception vision/ hearing/ damage.rs public.rs batch.rs            (deps: raw, types, rules, nav-api static vis)
lb-knowledge  context.rs tracks/{filter,node_belief,association}.rs hypotheses.rs items.rs projectiles.rs mines.rs
              gungame.rs protection.rs experience.rs teammates.rs provenance.rs   (NO lb-raw)
lb-styles     difficulty.rs style.rs profile.rs resolve.rs emotions.rs rng.rs schema.rs
lb-motor      intents.rs arbiter.rs look/{spring,newbie,fixed_step}.rs locomotion.rs stance.rs weapon_ctl.rs use_ctl.rs bhop.rs encoder.rs feedback.rs
lb-combat     targeting.rs threat.rs aim/{body_part,latency,error_ou,lead}.rs fire_control.rs ballistics/{hitscan,straight,gravity,grenade,satchel,rocket}.rs
              fight_tactics.rs positioning.rs friendly_fire.rs throw_planner.rs weapon_policy.rs
lb-actions    runtime.rs action.rs layers.rs primitives/{follow_path,hold,look,use_entity,touch,wait,shoot_entity,seq}.rs
              catalogue/{fight,aim_fire,throw_grenade,throw_snark,satchel,tripmine,gauss,rocket_jump,grenade_jump,use_charger,camp,hide,
                         shoot_breakable,dodge,respawn,vigilance,reload,investigate}.rs
lb-decision   engine.rs considerations.rs commitment.rs goals/*.rs team_board.rs behavior_module.rs travel_boost.rs
lb-modes      mode.rs ffa.rs tdm.rs gungame/{infer,bridge,classify,tactics}.rs
lb-brain      brain.rs schedule.rs trace.rs events.rs (bus incl. reserved chat hook)
lb-sim-tests  fixtures, replay harness, honesty property tests
config/ai/    difficulty.yaml styles/*.yaml profiles/*.yaml rules/{hldm,bhl,gungame,spawnprot}.yaml weapons/policy.yaml
```
A CI test inspects `cargo metadata`. Only `lb-perception` and `lb-brain` may have `lb-raw` in their dependency closure. Every belief mutation carries a `Provenance{sensor, t}`. Test builds panic on a missing or `Oracle` provenance.

```rust
pub struct BotBrain { /* per-bot: perception, beliefs, emotions, decision, runtime, combat, motor, rng, trace */ }
impl BotBrain {
    pub fn on_lifecycle(&mut self, ev: LifecycleEvent);   // spawn/death/disconnect/map change (guaranteed path)
    pub fn on_self_msg(&mut self, ev: &SelfMsg);          // CurWeapon, AmmoX, Damage, WeaponList (own)
    pub fn on_public(&mut self, ev: &PublicEvent);        // DeathMsg, ScoreInfo, TeamInfo, GgBridge
    pub fn frame(&mut self, io: FrameIo<'_>) -> CmdOut;   // EVERY server frame
}
pub struct FrameIo<'a> { pub now: SimTime, pub dt: f32, pub self_state: &'a SelfState,
    pub raw: Option<&'a RawWorldFrame>,   // Some only on this bot's perception tick; consumed by Perception only
    pub stimuli: &'a [RawStimulus], pub nav: &'a dyn NavQuery, pub rules: &'a RulesModel,
    pub mode: &'a dyn GameMode, pub budget: &'a mut FrameBudget }
pub struct CmdOut { pub angles: Angles, pub forward: f32, pub side: f32, pub up: f32,
    pub buttons: Buttons, pub impulse: u8, pub client_cmds: SmallVec<[ClientCmd; 2]> } // msec is set by integration
```

### 1.2 Rates (each bot staggered by `phase = slot/N·period`)
| Rate                          | Work                                                                                                                                                                                                                                                                                                   | Budget                          |
|-------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|---------------------------------|
| **Every frame** (500–1000 Hz) | Ingest SelfState. Pre-gate stimuli (PAS bit + attenuation). Action fast lane: jump/longjump press edges, gauss release window, grenade release time, satchel/tripmine/use presses. Arbiter resolve. Look integrator (sub-steps, §7.3). Locomotion projection. Stance FSM. Weapon controller. Encoder. | O(1), no traces, no allocations |
| 20 Hz                         | Vision scan (≤12 traces/bot, +3 for a first look at a newcomer in view), evidence and recognition, hearing integration, damage stimuli, track filters, projectile/mine/item-spot observation (≤4 traces)                                                                                               | global traces/frame             |
| 10 Hz                         | Combat tick (also on the frame an enemy is recognized, the bot is hurt or its target dies): target selection, weapon policy, fire plan, fight style, throw/dodge options. Action slow lane. Watchers at 2–3 Hz.                                                                                       |                                 |
| 5 Hz, plus urgent triggers    | Utility: generate, prescore, shortlist, nav queries, commit                                                                                                                                                                                                                                            | nav expansions                  |
| 2 Hz                          | Node-belief diffusion, negative observations, reappearance prediction                                                                                                                                                                                                                                  | ≤256 nodes/track, ≤4 tracks     |
| 1–2 Hz                        | Emotions (0.5 s step), item windows, GG refresh, team-board expiry                                                                                                                                                                                                                                     |                                 |
| event or 0.2 Hz               | Experience flush, telemetry summaries                                                                                                                                                                                                                                                                  |                                 |

Urgent triggers run on the next frame: a newly recognized enemy, damage taken, a weapon lost or forced (GG level-up), a path result, action completion, death/spawn.

---

## 2. Honest perception (`lb-perception`)

### 2.1 Vision
- **Candidates.** Players; projectiles (`grenade`, `rpg_rocket`, `crossbow_bolt`, `hornet`, `monster_snark`, `monster_satchel`, `monster_tripmine` plus beam, `laser_spot`); item spots from the static registry. Each must pass the adapter's PVS bit and be within `R_view` (4096 or the map's fog).
- **FOV.** A real frustum built from the actual view (cmd angles).
  - Horizontal FOV = `self.fov` (0 means the default: 90 at 4:3, 106 effective at 16:9, configurable). When zoomed: 20 for the crossbow, 40 for the 357.
  - Vertical half-angle = atan(tan(h/2)/aspect).
  - A target is inside if any of its box corners or center projects into the frustum.
  - This replaces yapb's 90° cone (`YAPB/inc/support.h:80-87`) and its fixed 75° 16:9 frustum (`inc/vision.h:33-36`).
- **LOS points.** Ported from `checkBodyPartsWithOffsets` (`combat.cpp:191-268`).
  - Standing: origin z +22 (head, = absmin + 0.81·size), +8 chest, −10 pelvis, −28 knees, ±13u edges at chest height. Crouched: +10 / 0 / −12.
  - Weights: head .15, chest .35, pelvis .20, knees .10, each edge .10.
  - Trace: `ignore_glass=true`, `ignore_monsters=false`. Success means fraction==1 or the hit entity is the target. Hitting another player counts as occlusion.
  - If the line crosses a water boundary, visibility ×0.5.
  - Order: chest, head, pelvis, edges. For already-tracked targets, stop after 2 hits.
  - Results are stored per target. This fixes yapb's shared `m_enemyParts` bug.
- **Evidence.** For contact c on each tick (Δt = 0.05 s):
  `r = v · a_fov · m_motion · m_range · m_cue`
  - `a_fov`: 1.0 when within 0.35·half-FOV, 0.7 in the middle band, periph_gain (0.35–0.55 by difficulty) beyond 0.8·half-FOV.
  - `m_motion`: 1.0 above 150 u/s, 0.8 at 30–150, 0.5 still and standing, 0.3 still and crouched. This ports `isEnemyNoticeable`'s tables (`combat.cpp:2830-2917`) as rates instead of per-tick coin flips.
  - `m_range`: 1.0 under 300u, lerps to 0.6 at 1000u, 0.35 beyond 2500u.
  - `m_cue`: ×2 if a muzzle flash is visible or the target is shooting at us; ×1.5 if primed by a sound or damage bearing within ±30° in the last 2 s; ×1.3 when alert (in combat within 3 s).
  - `E += r·Δt / D`. With no evidence, `E -= Δt/(2D)`.
  - **D is sampled once per contact**: `U(delay_min, delay_max)` × situation factor (camp/ambush ×0.5, ported from `tasks.cpp:412-414`).
  - The contact is recognized when E ≥ 1.
  - A pending contact keeps E and D for `pending_grace` = 0.75 s, so brief LOS loss does not resample.
- **Re-acquisition.** A lost recognized track seen again within `reacquire_grace` (1.0–3.0 s) and inside the gate (3σ + 64u) is recognized after `reacquire_delay` (0.04–0.25 s), its evidence building at 0.7 of the full rate at least wherever it is in the view. There is no full reset.
- **Anonymous cue.** Once E ≥ 0.4, emit `AnonymousCue{bearing, range_bin}` with no identity. The Vigilance layer may glance at it.
- **Observable once recognized.** A human-visible player name (crosshair ID) makes identity legitimate on recognition. Observable fields:
  - position (σ = 0.002·d)
  - velocity from an alpha-beta filter over sightings (α .5, β .2; valid ≤0.3 s after the last sighting)
  - stance, on-ground/airborne, ladder, water
  - facing yaw (±10° noise)
  - held weapon from `weaponmodel` (p_crowbar … p_satchel_radio)
  - firing (EF_MUZZLEFLASH or a seen event)
  - render fields (spawn-protection cue, EF_NODRAW)
  - `has_longjump_seen` (a longjump-shaped jump was observed)
  - **Not observable**: HP, armor, ammo.
- **Hit feedback.** Own-shot hits come as `HitConfirm` (blood is visible) and feed a flagged `EnemyDamageEstimate`.
- **Items.** A spot is observed when in FOV with 1 LOS trace to its center +8z, and d ≤ `item_view_range` (900–2000). Drawn means Present; EF_NODRAW means Absent.
- **Projectiles** use FOV plus LOS with class ranges: grenade 1200, satchel 800, snark 900, rocket 3000 (EF_LIGHT plus trail), bolt 1500, hornet 800, tripmine body 1000 or any of 3 beam sample points 1500, laser spot 2500.

### 2.2 Hearing
**Sources, supplied by integration as `RawStimulus`:**

| Source                 | Hook                                                                               | Examples                                                                                                                                                                                                                                                                                    | Info carried                      |
|------------------------|------------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-----------------------------------|
| Weapon events          | PlaybackEvent, name from the precache index                                        | glock1/2, mp5, mp52 (M203), shotgun1/2, python, crossbow1/2, rpg, gauss (param = dmg, so charge level), gaussspin (charging hum), egon_fire/stop, firehornet, crowbar, tripfire, snarkfire                                                                                                  | weapon id, "charging"             |
| Game-DLL sounds        | pfnEmitSound                                                                       | pl_pain*, fall pain, pickups (gunpickup2, 9mmclip1, smallmedkit1), `items/suitchargeok1` (item respawn), chargers (medshot4, suitcharge1), doors/buttons/plats, grenade/satchel bounces, `weapons/rocket1.wav` (attn 0.5), mine_deploy (1.0) and mine_charge (0.2), squeek/*, reload sounds | category plus location            |
| Player-movement sounds | ReHLDS SV_StartSound hook (preferred)                                              | pl_step*, ladder, wade, slosh, jumpland                                                                                                                                                                                                                                                     | movement                          |
| Explosions             | MessageBegin SVC_TEMPENTITY TE_EXPLOSION                                           | grenade, rocket, satchel, tripmine, M203, bolt                                                                                                                                                                                                                                              | explosion (also a visual flash)   |
| Near miss              | derived from a hitscan event whose origin + direction passes within 64u of the bot |                                                                                                                                                                                                                                                                                             | suppression, shooter bearing ±30° |

**Footstep fallback** when the SV_StartSound hook is unavailable: synthesize steps for **every** player with the exact pm_shared rule.
- Only if multiplayer and `mp_footsteps`, and (on a ladder, or on ground with horizontal speed > 220).
- Period: 300 ms running, 350 ms ladder, 600 ms wade.
- Volume by texture: concrete 0.5, metal 0.5, dirt 0.55, vent 0.7, grate 0.5, tile 0.5, ladder 0.35, wade 0.65.
- Jump = 1.0 under the same 220 rule. Landing = 0.5/0.85/1.0 at fall speed ≥350.
- The synthesizer is disabled whenever the hook works, so nothing is doubled.

**Audibility (per listener):**
1. PAS gate: the listener's PAS contains the origin, unless the event is FEV_GLOBAL. This mirrors engine delivery; verify for ReHLDS SV_StartSound and EV_Playback.
2. Gain `g = vol · max(0, 1 − d·attn/1000)`. ATTN_NORM 0.8 means audible to 1250u.
3. Heard if `g ≥ θ_diff · mask`. θ runs 0.07 (Noob) to 0.02 (Expert). `mask` = ×3 if the bot itself fired within 0.3 s, ×1.2 while it is running.

**Localization:**
- bearing σ = lerp(σ_min_diff, 35°, 1−g); front/back flip probability 0.1·(1−g)
- range = (1 − g/vol)·1000/attn · exp(N(0, 0.3))
- elevation σ = 30°
- The result is projected onto nav nodes as a `Hypothesis`.

**Identity:** sounds are anonymous. A stimulus is associated with a track only if exactly one track gates it (position within 2σ and a compatible weapon hint). Weapon class (for example, gauss spinning) is known from the sound.

A heard stimulus **never** creates a target. It primes vision (m_cue) and raises Investigate/Hunt hypotheses.

### 2.3 Damage
- From the `Damage` message: dmg_take, dmg_save, bits, origin.
- Recompute the four HUD intensities exactly as `CalcDamageDirection`: front/rear = forward·dir, right/left = right·dir, each counted only if > 0.3; within 50u all four light up.
- `bearing = atan2(I_r − I_l, I_f − I_r) + N(0, 15°)`, with no range or elevation.
- Type bits are limited to DMG_SHOWNHUD, plus the weapon inferred from a concurrent sound.
- Output: `DamageStimulus{bearing, amount, likely_weapon, t}`. It drives a snap-look request at priority P2, a threat cone hypothesis, emotions, and experience.

### 2.4 Public information
- DeathMsg (killer, victim, weapon): closes the victim's track (state KnownDead, then a spawn-point prior over `info_player_deathmatch`). Records "killer used weapon W". Emotions. Grudge.
- ScoreInfo (frags, deaths): scoreboard, GG level = ⌊frags/100⌋, leader.
- TeamInfo and GameMode messages.
- Server cvars via RulesModel: sk_*, mp_dmg_*, mp_weaponstay, mp_falldamage, mp_bunnyhop, mp_footsteps, mp_friendlyfire, mp_teamplay, mp_selfgauss, sv_gravity, sv_maxspeed, sv_aim, gg_*.
- Static map knowledge, including learned experience.
- Chat: none in v1. `lb-brain::events` has a reserved `NarrativeEvent` bus: bounded, one-way, no feedback into decisions.

### 2.5 Honest replacements for the yapb cheats
| #  | yapb location                                                                                                         | Replacement                                                                                                                                            |
|----|-----------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------|
| 1  | `sounds.cpp:104-166` noise from buttons/velocity; `botlib.cpp:2430-2552` true origin, and LOS acquire ignoring facing | Real sound and event stimuli, PAS + attenuation gates, localization error, anonymous. Only vision (FOV) can recognize. The bot turns toward the sound. |
| 2  | `combat.cpp:2553-2817` reads lastEnemy's true origin/velocity/flags/buttons                                           | Track mean, σ and node belief. Velocity only if seen ≤0.3 s ago. "Attacking me" = observed muzzle flash or events aimed at me.                         |
| 3  | `botlib.cpp:2117-2189`, `message.cpp:95-110` use `dmg_inflictor`                                                      | HUD 4-way bearing only (2.3).                                                                                                                          |
| 4  | `manager.cpp:1427-1466` Hard+ teammates get the killer                                                                | Removed. If the bot perceived the victim's death, create a killer-region hypothesis: nodes with LOS to the victim within the kill-feed weapon's range. |
| 5  | `combat.cpp:530-547` teammate alarm with exact position                                                               | Removed (v2 §6.2).                                                                                                                                     |
| 6  | `botlib.cpp:1168-1169` global alive counts                                                                            | Count of recognized/fresh tracks. The scoreboard's player count is public.                                                                             |
| 7  | `numEnemiesNear` (`combat.cpp:47-62`) in predict aim, satchel trigger, tripmine detonation, fear halving              | `beliefs.enemies_near(p, r, min_conf)`: visible, or track age ≤0.5 s with σ < 60. Friends = visible teammates.                                         |
| 8  | `manager.cpp:875-896` global mine registry                                                                            | Per-bot MineBelief: own placements, mines seen (body or beam), heard deploys (hypothesis), explosions.                                                 |
| 10 | `combat.cpp:2530-2551` grenade velocity overwrite                                                                     | Removed. The pitch is solved with real mechanics (6.4).                                                                                                |
| 11 | `navigate.cpp:1112,1133,1137` writes `pev->velocity`                                                                  | The AI emits commands only. Nav executes jumps through the motor.                                                                                      |
| 12 | ignore-everything traces (`combat.cpp:199`)                                                                           | Vision: `ignore_glass`, players occlude. Firing checks use `dont_ignore_glass` and monsters.                                                           |
| 13 | `botlib.cpp:62-117` 360° grenade awareness                                                                            | FOV + LOS or bounce sounds. Dodge perpendicular *away* (fixes the sign bug).                                                                           |
| 15 | `whose_your_daddy` (`combat.cpp:343-345,465-467`, `vision.cpp:132-142`)                                               | Deleted.                                                                                                                                               |
| 16 | practice data                                                                                                         | Kept as ExperienceMap with honest inputs (3.6).                                                                                                        |
| –  | `MDLL_Use` (`tasks.cpp:1777`, `navigate.cpp:1267,1626`)                                                               | UseEntity primitive: 64u sphere, look dot > 0.7, edge-pressed IN_USE, verified effect.                                                                 |
| –  | 360° LOS item updates (`botlib.cpp:661-792`)                                                                          | FOV-limited observation plus respawn prediction.                                                                                                       |
| –  | `game.isAliveEntity(m_lastEnemy)` (`botlib.cpp:1054,1093`; `combat.cpp:384,2582`)                                     | Track state (KnownDead only via DeathMsg).                                                                                                             |
| –  | enemy `v_angle` in strafe/fire (`combat.cpp:1728,1893`)                                                               | Observed facing yaw ±10°.                                                                                                                              |
| –  | `getGunGameLeader` alive filter (`manager.cpp:1045-1071`)                                                             | ScoreInfo leader only.                                                                                                                                 |

---

## 3. Beliefs (`lb-knowledge`)
```rust
pub struct EnemyTrack { id: TrackId, who: Option<PlayerKey>, rel: Relation, state: TrackState, // Visible|RecentlyLost|Predicted|Stale|KnownDead
  pos: Vec3, sigma: f32, vel: Vec3, vel_valid_until: SimTime, last_seen: SimTime, last_heard: SimTime,
  last_seen_node: NodeId, nodes: NodeBelief /*sparse node->p*/, traits: ObservedTraits /*weapon,stance,facing,firing,
  protection,lj_seen*/, dmg_est: DamageEstimate, head_roll: BodyPart /*ONE roll per contact*/, contact: ContactStats, prov: Provenance }
pub struct Hypothesis { kind: HypKind /*Sound|DamageBearing|KillerRegion|AnonCue*/, nodes: NodeBelief, t: SimTime, strength: f32, weapon_hint: Option<WeaponId> }
pub struct ItemSpotBelief { spot: ItemSpotId, class: ItemClass, state: ItemState /*Present{t}|Absent{t}|Unknown*/,
  window: Option<(SimTime, SimTime)>, learned: RespawnStats, charger: Option<ChargerState>, reserved: Option<Reservation> }
pub struct ProjectileBelief { kind: ProjKind, pos: Vec3, vel: Vec3, owner: Owner /*Own|Unknown*/, t: SimTime,
  impact: Option<(Vec3, SimTime)>, fuse_est: Option<SimTime>, radius: f32 }
pub struct MineBelief { pos: Vec3, normal: Vec3, beam_end: Option<Vec3>, owner: Owner, armed_at: SimTime, verified: SimTime, conf: f32 }
```

**3.1 Tracks.**
- σ(t) = σ₀ + 0.6·maxspeed·(t − t_seen), capped at 1500.
- At loss, the node belief starts at `last_seen_node`, with neighbors weighted toward the travel direction (+50% within ±45° of v_est).
- At 2 Hz, diffuse along graph edges within reach (maxspeed·Δt along edge travel time).
- **Negative observation:** nodes visible from the bot's node (vistable), inside the FOV and within 1500u, get p ×(1 − 0.9·a_fov), then renormalize.
- Forget horizon (Stale), by difficulty: 4/6/8/10/12 s, × style chase_persistence. Dropped after 30 s.
- Reappearance points: visible nodes adjacent to occluded high-mass nodes. These port yapb's PredictPath (`botlib.cpp:1039-1126`, `vision.cpp:441-508`) without the omniscient `numEnemiesNear`.

**3.2 Items.**
- The registry comes from the BSP entity lump.
- At map start everything is Present.
- An Absent observation at t with a previous Present at t₀ gives the window [t₀ + T, t + T], with T from RulesModel. Weapon stay: T = 0 unless LIMITINWORLD.
- A heard pickup or respawn sound localized to the spot pins the time (±0.2 s).
- Present-but-unseen availability decays at hazard λ = 0.005/s per active opponent.
- Learning: an observed Absent→Present (the materialize cue) updates an EMA (α = 0.3, clipped ±45 s) once there are ≥2 samples. This honestly handles non-standard respawn plugins.
- Chargers: a use that stops raising HP or armor, or the empty texture seen, marks the charger drained until t + 60/30 s.

**3.3 Projectiles and mines.**
- Own satchels: position from observation, else integrated from the throw.
- Mines are removed when an explosion is observed at the spot, or the spot is seen without the body or beam.

**3.4 GunGame belief.** See §9.

**3.5 Spawn protection.** Per track: `{cue_since, active, est_end = cue_since + duration}`. The duration is learned from observed cue drops. Own protection comes from own render state.

**3.6 ExperienceMap.** Ports `practice.cpp` and `botlib.cpp:2238-2302`.
- Key: `ExpPartition::{Ffa, Team(u8)}`. This fixes the clamp that collapsed FFA into one team slot.
- `damage[(victim_node, attacker_node)]` is an EMA. The attacker node comes from a recognized track, or else the damage bearing ∩ vistable within weapon range, spread evenly.
- Update per hit ≥ 20 dmg: `dmg/7`.
- `danger_index[node]` = argmax over visible attacker nodes (port of `practice.cpp:109-153`).
- `goal_value[(start, goal)]`: −HP/20 on dying en route.
- Updates are continuous, with a 20-hour-of-play half-life. Saved every 300 s and at map change, versioned by map fingerprint.

---

## 4. Decision (`lb-decision`)

### 4.1 Engine: dual utility (rank + weight)
```rust
pub struct GoalScore { rank: u8, weight: f32, factors: SmallVec<[Factor; 8]>, gate: Option<GateReason> }
pub trait GoalKindImpl: Send + Sync {
  fn generate(&self, ctx: &DecisionContext, out: &mut Vec<GoalCandidate>);        // cheap
  fn prescore(&self, ctx: &DecisionContext, c: &GoalCandidate) -> Option<GoalScore>; // no path queries
  fn needs(&self, c: &GoalCandidate) -> QueryNeeds;                                // travel/path for shortlist
  fn score(&self, ctx: &DecisionContext, c: &GoalCandidate, q: &QueryResults) -> GoalScore;
  fn build(&self, ctx: &DecisionContext, c: &GoalCandidate, lib: &PrimitiveLibrary) -> Box<dyn Action>;
  fn hold(&self) -> HoldPolicy; }
```
- Base weight: `w = A_style · B · C · exp(−T/τ) · (1 − R)^ρ`, where:
  - B = benefit
  - C = knowledge confidence
  - T = travel estimate from nav (Pending uses a Euclidean/speed ·1.4 bound, flagged)
  - R = route and destination risk (experience + known threats) / EHP
  - ρ = style risk aversion × (0.5 + fear)
- Selection:
  1. Drop candidates below their threshold.
  2. Take the top rank present.
  3. Among candidates within 90% of the best weight, pick at random using the Decision RNG, once per re-decision.
  4. Apply commitment.
- Shortlist: prescore everything, then send ≤6 candidates to nav. Pending is not NoPath: keep the current goal.

### 4.2 Goal catalogue
Weights are in [0,1]. yapb desire/100 gives the prior.

| Goal (rank)                   | Weight                                                                                                                                                                                                                                                                                                                                                                                                                                                    | yapb origin                                                                    |
|-------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------|
| Engage(track) (2)             | 0.9 · reach · adv^0.5 · (0.6+0.4·aggr) · A. `reach` = 1 if detour ≤ 112 + 0.1d or the weapon is effective at the current distance, else 0.4. `adv` = myEff/(myEff + enemyEff). Gate: recognized.                                                                                                                                                                                                                                                          | Attack 90 when SeeingEnemy && reactOnEnemy (`botlib.cpp:1273-1278,1531-1612`)  |
| Retreat(threat) (2)           | `(100−HP_eff)·fear'/100 · recency · mult`, threshold 0.40. HP_eff = EHP/3. recency = clamp((10 − age)/10). mult: ×3 reloading with no alternative or sniper low ammo; ×2 all guns dry; ×1.5 outgunned; ×style; /3 stuck or melee; DM base 0.5 (yapb used 0). fear' = fear·0.5 if visible friends > perceived enemies near (TDM), ·1.5 with a sniper weapon.                                                                                               | SeekCover (`botlib.cpp:1284-1325,1245-1248`); Hide phase (`tasks.cpp:528-584`) |
| Hunt(track) (1)               | `(4096 − (1−aggr)·d)/4096 − retreat`, cap 0.89, × track confidence, threshold 0.60. GG melee level: max(·, 0.70). Sniper style A = 0.3.                                                                                                                                                                                                                                                                                                                   | `botlib.cpp:1331-1352,1425`                                                    |
| Investigate(hyp) (1)          | 0.45 · strength · (1 − age/τ) · A, threshold 0.2. Destination: vantage node maximizing Σp(n)·vis(v, n).                                                                                                                                                                                                                                                                                                                                                   | new (honest replacement for hearing acquire)                                   |
| CollectItem(spot) (1)         | min(1, B_item·W_style) · P_avail(arrival) · e^(−T/10) · (1−R)^ρ. Floor when visible and d < 450: 0.5 + 0.45·(1 − d/450). Rank 2 if HP_eff < 30, health/armor item, no track within 800. B_item = ΔEHP/300 (health: min(cap, 100−h); battery: 2·min(cap, 100−a)); weapon: affinity(new) − affinity(best owned same slot); ammo: need·affinity (9mm feeds glock+MP5, uranium feeds gauss+egon); LJ 0.8. Knees as in yapb: HK h<85, battery a<90, ammo <70%. | Pickup max(50, 500−0.2d) (`botlib.cpp:1252-1261`, `498-816`)                   |
| UseCharger(c) (1)             | Like CollectItem plus charge-time cost. Knees h<60, a<40.                                                                                                                                                                                                                                                                                                                                                                                                 | `botlib.cpp:760-773`, `tasks.cpp:1331-1369`                                    |
| ControlItem(spot, window) (1) | value · (own_gain + 0.5·denial·pickable) · P_window · timing_fit · A (Controller 2.0). Arrive at window start − 1 s, then hold nearby in cover.                                                                                                                                                                                                                                                                                                           | new                                                                            |
| Camp/Ambush(spot) (1)         | A · (danger_norm(next area)/div − base_aggr); div = 3 (Balanced) or 2. At most once per 60 s (Sniper: 20 s). 15% gate per node advance, converted to a rate. Not for Rusher.                                                                                                                                                                                                                                                                              | `navigate.cpp:2486-2523`, `camp_` `tasks.cpp:400-526`                          |
| PlantTrap(spot, kind) (1)     | A · chokepoint_value(traffic, narrow) · has_mines / (1 + mines_there)                                                                                                                                                                                                                                                                                                                                                                                     | `tasks.cpp:424-462`, `navigate.cpp:2379-2462`                                  |
| Roam (0)                      | Constant 0.2. Tactic draw as in findBestGoal: goal = rand100 + aggr·100 (0 in GG), forward = rand100 + aggr·100, backoff = rand100 + fear·100, camp = (rand100 + fear·100)·0.3 (camp guns only). Targets: items / high-traffic / low-danger / vantage nodes. 4 candidates exclude recent goals; sorted by goal_value (Rusher: random).                                                                                                                    | `navigate.cpp:14-181`                                                          |
| Scenario (module)             | Rank and weight from the BehaviorModule                                                                                                                                                                                                                                                                                                                                                                                                                   | new                                                                            |

Throws, detonations and dodges are not goals. They are combat-layer options (§6.6).

### 4.3 Commitment
- Minimum hold: Engage 1.0 s; Hunt 3 s; Retreat 2 s; CollectItem until reached or invalid, capped at 1.5·ETA + 3 s; ControlItem until the window closes; Camp for its duration; Roam 5 s.
- Switch when `best.w > cur.w·1.15 + 0.05`. Retreat→Engage requires a 25% margin.
- Rank 2 preempts rank ≤1 immediately.
- Failure cooldown per target: 8–15 s.
- Death, loss of the goal's resource, or an invalid goal cancels the hold.
- This replaces yapb's no-op hysteresis (`botlib.cpp:1407-1416`) and the rule "keep max(final, current)" (`1431-1433`).

### 4.4 TDM modifiers
- **Relations** come from TeamInfo.
- **Friendly fire:** see 6.8.
- **TeamBoard (bots only):** `reservations: ItemSpotId -> {bot, eta, expires(ttl 6 s)}`. CollectItem ×0.2 if a teammate's ETA is more than 1 s better. Roam destinations can be reserved the same way to spread bots out.
- **No observation sharing.**
- **Yielding** at narrow spots and ladders goes through nav's local avoidance.
- **FFA:** no board. Bots do not coordinate.

### 4.5 GunGame modifiers
See §9. Weapon and ammo goals are zeroed; health, battery and longjump stay.

### 4.6 Travel boosts
These are route policy, not goals.
```rust
pub struct BoostPolicy { allow: BoostKinds /*LJ,Gauss,Rocket,Grenade,Satchel,Bhop*/, floors: ResourceFloors
  /*uranium>=30 before gauss jump, rockets>=2, hp_after>=40*/, opp: Opportunistic /*lj_runway, gauss_far{p:0.33,every:10..18s,
  min_dist:1400,min_nodes:12}, bhop{max_speed,cap_margin:0.97,jitter_ms}*/, risk_tolerance: f32 }
```
Passed with every path request. Nav validates and executes links. The opportunistic gauss jump ports `checkGaussJumpTravel` (`combat.cpp:1518-1557`). The longjump runway ports `checkLongJump` (`navigate.cpp:810-990`).

### 4.7 Scenario plug-in interface
Used by overlays now and by Lua later.
```rust
pub trait BehaviorModule: Send + Sync {
  fn id(&self) -> &str;  fn applies(&self, map: &MapInfo, mode: ModeKind) -> bool;
  fn propose(&self, ctx: &DecisionContext, out: &mut dyn GoalSink);          // GoalSpec{key,rank,weight,factors,target,hold,params}
  fn instantiate(&self, g: &GoalSpec, lib: &PrimitiveLibrary) -> Result<Box<dyn Action>, BuildError>;
  fn on_event(&mut self, _ev: &BotEvent) {} }
impl PrimitiveLibrary { fn follow_path(..); fn hold_position(..); fn look_at(..); fn use_entity(ent, UseMode, EffectCheck);
  fn touch_volume(..); fn shoot_entity(..); fn wait_until(ObservableCondition, timeout); fn sequence(..); fn select_first(..); }
```
`ObservableCondition` can only be evaluated from DecisionContext, for example "entity observed Open", "map time > 180 s", "item p > 0.7".

### 4.8 yapb desire mapping (so behavior stays recognizable)
| yapb                                       | Value   | New                                                           |
|--------------------------------------------|---------|---------------------------------------------------------------|
| Attack                                     | 90      | Engage, rank 2                                                |
| SeekCover (threshold 40), subsumed by Hide | 91/92   | Retreat, rank 2, threshold 0.40, with Hide as a phase         |
| Hunt (threshold 60)                        | ≤89     | Hunt, rank 1, threshold 0.60                                  |
| Pickup                                     | 50–500  | CollectItem floor 0.5–0.95 (near items beat Hunt, as in yapb) |
| Camp / Normal                              | 37 / 35 | Camp rank 1 / Roam rank 0                                     |
| Throw / Detonate                           | 99 / 98 | Combat-layer protocols, arbiter priority P85                  |
| GaussJump                                  | 99      | Boost action                                                  |
| ShootBreakable                             | 100     | Nav-mandatory primitive, P90                                  |

### 4.9 What the AI needs from nav (`lb-nav-api`)
- `project_anchor`
- `estimate_travel(from, to, caps) -> Known|Pending`
- `request_path(PathRequest{…, BoostPolicy, risk_weights}) -> ticket`, then `poll`
- `vis(a, b, stance)` (vistable)
- `nodes_within`
- `move_safety(from, to) -> Safe|Drop{h}|Deadly|Blocked` (fixes the `isDeadlyMove` loop that never runs, `navigate.cpp:3114-3157`)
- `lateral_clearance(pos, dir, 134)`
- `cover_candidates(threat_nodes, from, k)`
- `validate_launch(kind, pos, dir) -> LaunchVerdict` (longjump, gauss, rocket, satchel)
- `walkable_line`
- `chokepoints()`
- blocking-breakable reports

---

## 5. Actions (`lb-actions`)

### 5.1 Runtime
```rust
pub enum Step { Continue, Complete, Fail(FailReason), Replace(Box<dyn Action>, Why), SuspendFor(Box<dyn Action>, Why) }
pub trait Action: Send {
  fn kind(&self) -> ActionKind; fn channels(&self, cx: &ActionCx) -> ChannelSet;
  fn start(&mut self, cx: &mut ActionCx) -> Result<(), FailReason>;
  fn update(&mut self, cx: &mut ActionCx) -> Step;               // reads cx.feedback: granted/denied/executed
  fn on_suspend(&mut self, _: &mut ActionCx) {} fn on_resume(&mut self, cx: &mut ActionCx) -> Result<(), FailReason>;
  fn cancel(&mut self, cx: &mut ActionCx, why: CancelReason) -> CancelProtocol; // Immediate | Graceful{deadline}
  fn deadline(&self) -> Option<SimTime>; fn progress(&self) -> Progress; fn lane(&self) -> Lane /*Fast|Slow*/ }
```
- Slots: **Primary** is a stack with suspend/resume and is driven by the goal. Concurrent **layers** each hold at most one action: Combat (AimAndFire / Throw* / Detonate*), WeaponProtocol (GaussCharge), Reflex (Dodge), Watchers (satchel and tripmine detonation), Vigilance (look).
- Transitions are collected and applied atomically after all updates.
- Phases advance only on **executed** feedback, never on proposals.

### 5.2 Catalogue
Channels: L = Locomotion, K = Look, S = Stance, W = Weapon, U = Use.

| Action                            | Phases                                                                                                                                                                                                                  | Channels                                                  | Cancel / deadline                                                                                       | Success                                                          |
|-----------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------|---------------------------------------------------------------------------------------------------------|------------------------------------------------------------------|
| FollowPath                        | Request, WaitPath, Traverse (nav executor), Arrive; Replan (≤3); Fail                                                                                                                                                   | L (+K/S while nav traversal is mandatory, P90)            | Immediate outside mandatory phases. Deadline 2·ETA + 5 s, plus a progress monitor.                      | Inside the arrival region                                        |
| Fight (Engage)                    | Approach, Strafe, Stay, Backpedal, Dodge, Melee, Reposition, SniperHold                                                                                                                                                 | L, S                                                      | Immediate                                                                                               | Goal-driven. Exits after reacquire_grace.                        |
| AimAndFire                        | Acquire, Track, Fire, Hold (protected / not permitted), SwitchOrReload, Cease                                                                                                                                           | K, W                                                      | Graceful while a zoom or switch is in flight                                                            | –                                                                |
| ThrowGrenade                      | Select, Aim (pitch solve), PullPin (hold IN_ATTACK), Cook, Release (both attack buttons up, aim steady ≥50 ms), Confirm (own projectile seen or ammo−1), Restore (lastinv)                                              | W, K (+L stop if the target is not visible)               | **After pin pull: must throw.** Safe dump (45° up, away from self and friends) before fuse − 0.3 s.     | Grenade observed                                                 |
| ThrowSnark                        | Select, Aim (target +14z; 24–60u free-space trace), Press, Confirm, Restore, AvoidOwnSnarks hint                                                                                                                        | W, K                                                      | Immediate before the press                                                                              | Snark seen. One live snark is enough (`tasks.cpp:838-844`).      |
| ThrowSatchel                      | Select, Aim (274 u/s lob solve), Press (DllProfile throw button), WaitCharge (retry after 1.1 s), Restore                                                                                                               | W, K (+L stop)                                            | Immediate                                                                                               | Own charge observed                                              |
| DetonateSatchel (watcher, 2–3 Hz) | Condition: (see below). Then BackOff if within 320u, Select (radio), Press detonate button, Confirm, Restore. Charges must be within 4096u.                                                                             | W (+L, K when backing off)                                | Deadline 4 s                                                                                            | Explosions observed                                              |
| SatchelJump                       | 0 run-up (select, sprint ≥0.6·maxspeed, ≥0.9 s), 1 fresh jump, 2 airborne ≥0.25 s: aim target + clamp(0.18d, 24, 140) and throw (retry after 0.4 s), 3 back off + detonate watcher                                      | L, K, S, W (atomic)                                       | Abort if target < 150u before commit. Deadline 4 s.                                                     | Port of `tasks.cpp:969-1127`, `combat.cpp:1341-1367`             |
| PlaceTripmine{Static,Corner}      | Approach (≤100u, ≤10°), Select, Aim wall point, Press, Confirm ≤0.75 s, Register (armed at +2.5 s), Restore                                                                                                             | L, K, W                                                   | Abort if an enemy is recognized. Deadline 6 s.                                                          | Mine seen or ammo−1 (`tasks.cpp:1203-1256`)                      |
| PlaceTripmineMoving (layer)       | Floor spot 40u ahead (normal.z ≥ 0.7, ≤120u, 96u from known mines), Dip look (P85, ≤0.2 s), Press within 20°, Confirm                                                                                                   | K, W (FollowPath keeps L)                                 | Deadline 2 s                                                                                            | Port of `tasks.cpp:1258-1329`                                    |
| DetonateTripmine (watcher)        | Condition (below), then Select (glock or any hitscan), Aim, Fire at cone ≥0.97                                                                                                                                          | K, W, L(stop)                                             | Abort if the victim is beyond 160u or a friend within 200u. Deadline 4 s.                               | Explosion observed (`botlib.cpp:370-436`, `tasks.cpp:1640-1701`) |
| GaussCharge (protocol layer)      | Idle, ChargeRoam, ChargeCombat, AimForRelease, Releasing (0.2 s), Cooldown                                                                                                                                              | W (Never-preempt while charging; K only in AimForRelease) | Graceful safe dump ≤0.6 s                                                                               | Port of `combat.cpp:1401-1516`, with fixes (6.5)                 |
| GaussJump                         | 0 spin ≥1.6 s, revalidate, 1 aim settle (pitch 32–40° down-back, yaw ±8°; cone ≥0.97, or ≥0.90 after 1.2 s), 2 fresh jump, release when airborne (or after 0.25 s), 3 hold aim through the window, 4 flight steer, land | L, K, S, W (atomic)                                       | Deadline 8 s                                                                                            | Landed (`tasks.cpp:1443-1577`, `combat.cpp:1283-1339`)           |
| RocketJump / GrenadeJump          | Look down 80–89°, jump, fire at liftoff (rocket) / cooked drop (grenade), flight                                                                                                                                        | L, K, S, W                                                | Only if nav validated and the self-damage policy allows                                                 | Landed                                                           |
| UseCharger                        | Approach use anchor, Face (dot > 0.7, aim ≤10°), Hold IN_USE, Monitor                                                                                                                                                   | L, K, U                                                   | Stop when full (≥99), no gain for 1 s (mark drained), enemy recognized, or 15 s                         | `tasks.cpp:1331-1369`                                            |
| Camp / Ambush / Hide              | Travel, Settle (crouch per `selectCampButtons` port `botlib.cpp:2554-2573`), Watch (alternate camp directions every 1–4 s, pre-aim reappearance points, random far visible node), Exit                                  | L, K(P3), S                                               | Time 5–15 s (+4 s walk; style-scaled), non-bullet damage, enemy recognized (Sniper stays in SniperHold) | `tasks.cpp:400-584`                                              |
| ShootBreakable                    | Aim, Fire at cone ≥0.90, give up after 1.5 s without progress (crowbar within 32u if dry)                                                                                                                               | K, W, L(stop)                                             | –                                                                                                       | Nav reports passable (`tasks.cpp:1579-1638`)                     |
| UseEntity / Touch                 | Approach (64u sphere), Face (dot > 0.7), edge-press IN_USE (or walk into the volume), Verify effect, Retry per the object's toggle policy                                                                               | L, K, U                                                   | –                                                                                                       | Observed effect                                                  |
| Dodge (reflex)                    | Perpendicular away from the predicted blast, no duck, `move_safety` checked                                                                                                                                             | L (P70, ≤1 s)                                             | –                                                                                                       | Outside the radius                                               |
| Vigilance (P20)                   | Nav look-ahead, danger node (`vision.cpp:518-540`), reappearance points, sound bearing, last victim for 1–2 s (Normal+; `vision.cpp:622-628`), lost-enemy stare for 0.75–1.25 s (`vision.cpp:377-387`)                  | K                                                         | Free                                                                                                    | –                                                                |
| Respawn                           | Delay U(0.5, 1.5) s, fresh press, repeat after 1 s                                                                                                                                                                      | all (P100)                                                | –                                                                                                       | `botlib.cpp:1762-1777`                                           |

**DetonateSatchel condition:** a recognized or fresh enemy is within 160u of an own charge and closer to it than we are; no visible friend within 200u of any own charge (`tasks.cpp:861-914`).

**DetonateTripmine condition:** a known mine (any owner; shooting it credits the shooter, `tripmine.cpp:317-323`) at 350–1200u; a recognized or fresh enemy within 140u of it; no visible friend within 200u; LOS to the mine with fraction ≥0.9.

### 5.3 yapb bugs not ported, and their fixes
- **Zombie/chicken creature mode** (`botlib.cpp:2594-2621`): dropped.
- **Emotion over-clamp +1** (`2155-2164`): clamp to [0,1].
- **startTask clears the path per stack element** (`1446-1456`): explicit goal change only.
- **Hysteresis no-op:** replaced by 4.3.
- **HearingEnemy never cleared** (`1182-1203`): stimuli decay.
- **Aim flags reset at 10 Hz:** intents expire per frame.
- **`m_enemyParts` from the last tested player:** per-target results.
- **Head roll repeated every tick** (`combat.cpp:698-699`): one roll per contact.
- **`isFriendInLineOfFire` normalizes the angle triple** (`combat.cpp:795`): view forward vector (6.8).
- **Dark-area check always false:** darkness is off in v1.
- **`pev->fov == 0` degenerate checks** (`botlib.cpp:87,211`): real frustum.
- **Grenade avoidance strafes toward the grenade** (`botlib.cpp:106-111`): away.
- **takeBlind restore inverted:** not applicable.
- **Seek-cover gate always true in FFA:** removed.
- **Low-approach bots charge forward** (`combat.cpp:1852`, move speed stays maxspeed): approach < 30 now means strafe + backpedal −0.3·maxspeed.
- **checkReload only looks at the lowest-id weapon** (`combat.cpp:2377-2382`): all candidates.
- **selectBestWeapon ignores a loaded clip with no reserve:** clip > 0 counts.
- **rateGroundWeapon compares a pref index to a table index** (`combat.cpp:2103`): slot class by WeaponId.
- **Gauss dump aims and releases on the same tick** (`combat.cpp:1369-1399`): AimForRelease.
- **`"enade.mdl"` never matches:** projectile class comes from integration.
- **attack_monsters targets own snarks and hornets:** only approaching snarks within 300u; never own hornets.
- **Crowbar drawn before jumps** (`navigate.cpp:2595-2602`): removed.
- **Practice saved once, FFA collapsed:** 3.6.
- **Lead scaled by frameInterval** (`combat.cpp:668`, fps-dependent): latency- and flight-time-based lead.

---

## 6. Combat and weapons (`lb-rules::mechanics`, `lb-combat`)

### 6.1 WeaponMechanicsModel (BHL defaults; damage read from `mp_dmg_*` and `sk_*`)
| Weapon      | Clip/Max                 | Primary                                                                                                                                                 | Secondary                                             | Cycle P/S           | Dmg          | Notes                                         |
|-------------|--------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------|-------------------------------------------------------|---------------------|--------------|-----------------------------------------------|
| crowbar     | –                        | melee 32u (+head_hull)                                                                                                                                  | –                                                     | 0.5 miss / 0.25 hit | 25           |                                               |
| glock       | 17 / 250 9mm             | hitscan spread 0.01                                                                                                                                     | "rapid" spread 0.1                                    | 0.3 / 0.2           | 12           | Reload 1.5; works underwater                  |
| python      | 6 / 36                   | hitscan 1°                                                                                                                                              | zoom fov 40 (MP)                                      | 0.75 / 0.5          | 50           | Reload 2.0; not underwater                    |
| mp5         | 50 / 250 + 10 AR         | hitscan 6° (MP)                                                                                                                                         | M203 contact grenade 800 u/s, g·0.5, dmg 100, r = 250 | 0.1 / 1.0           | 12           | Reload 1.5                                    |
| shotgun     | 8 / 125                  | 4 pellets 10°×5°, range 2048                                                                                                                            | 8 pellets 20°×5°                                      | 0.75 / 1.5          | 20/pellet    | Shell reload 0.6 + 0.5/shell, interruptible   |
| crossbow    | 5 / 50                   | zoomed: hitscan (xbow_scope 120); unzoomed: bolt 2000 u/s, no gravity, direct `sk_plr_xbow_bolt_client` + 40 dmg r = 128                                | zoom fov 20 (1.0 s toggle)                            | 0.75                | –            | Reload 4.5                                    |
| rpg         | 1 / 5                    | Rocket 250 u/s at view +30° up with g·0.5 for 0.4 s, then accelerates (+400/0.1 s) to 2000; follows the laser spot; detonates if speed < 1500 after 1 s | laser toggle 0.2 s (on by default)                    | 1.5                 | 120, r = 300 | No holster while guiding; reload 2 s          |
| gauss       | – / 100 U                | 20 dmg, 2 U                                                                                                                                             | charge (§0.4)                                         | 0.2                 | 20 / ≤200    | Punches thin walls; reflects at n < 0.5       |
| egon        | – / 100 U                | wide beam 2048: 20/0.1 s plus radius 5 (r = 128); 1 U/0.2 s                                                                                             | –                                                     | continuous          | 200/s        | Not underwater                                |
| hornetgun   | – / 8 (regen 1/0.3 s MP) | homing: 300 u/s, then 600/800 tracking in a 512u / ±25° cone, 3.5 s life                                                                                | dart 1200 u/s straight                                | 0.25 / 0.1          | 10           |                                               |
| handgrenade | 10                       | cook and throw (§0.2), friction 0.8                                                                                                                     | –                                                     | 0.5                 | 100, r = 250 |                                               |
| satchel     | 5                        | lob 274 u/s + own velocity, g·0.5, friction 0.8                                                                                                         | detonate (DllProfile)                                 | 1.0 / 0.5           | 120, r = 300 |                                               |
| tripmine    | 5                        | place within 128u along aim; arms at 2.5 s; beam along the normal up to 2048                                                                            | –                                                     | 0.3                 | 150, r = 375 | Shooting it detonates and credits the shooter |
| snark       | 15                       | 200 u/s + own velocity; hunts within 512u; bite 10; pops at 15 s                                                                                        | –                                                     | 0.3                 | –            | Bites its owner after bouncing                |

Shared formulas:
- Radius damage: `D·(1 − d/R)`, R = 2.5·D unless stated otherwise; requires LOS from the blast.
- `EHP_bullet = h + 2a`, `EHP_blast = h + a`.
- Knockback `Δv = min(1000, 5·raw)`.
- Fall damage: `mp_falldamage 0` means a fixed 10.

```rust
pub struct DllProfile { name: DllKind /*BHL|HL25|SDK23|AG*/, satchel: SatchelButtons{throw: Btn, detonate: Btn},
  hg_mult: f32 /*6.5 BHL/HL25; 4.0 SDK23*/, hg_cap: f32 /*1000; 500*/, dmg_cvars: bool, selfgauss_cvar: Option<&'static str>,
  bhop: BhopRule{cvar:"mp_bunnyhop", cap_when:0, factor:1.7, penalty:0.65} }
```
Default rows for SDK23 are marked "verify". A mechanics self-test runs on the stand at map start: throw, detonate and zoom, confirmed by observation.

### 6.2 WeaponPolicy
```
S(m) = E_dps(m,d) · A_style(w) · ammo_factor · env − switch_pen − self_risk
```
- `E_dps = dmg·rate·P_hit`.
- Hitscan `P_hit ≈ clamp((r_t / (d·tanσ + σ_aim(d)))², 0, 1)`, with r_t = 16 (body) or 5 (head). The shotgun sums per-pellet probabilities. Projectiles multiply by (1 − min(1, v_t·t_f/32)) unless they splash. Splash uses R_eff/(σ_pos + v·t).
- `switch_pen = (0.5 s deploy) · incoming_dps / EHP`. After a switch the weapon is committed for `switch_commitment_s`.
- `self_risk = E[self dmg]/EHP · aversion`. GG self-kill descore: aversion ×4.
- `env`: 0 underwater for python, mp5, shotgun, gauss and egon; ladders exclude charge and throw weapons.
- When scores are within 10%, fall back to yapb's ordering (Egon > Gauss > RPG > MP5 > Shotgun > Crossbow > Python > Hornet > Glock > Crowbar, `combat.cpp:1610-1641`), including `isWeaponBadAtDistance` bands (`1683-1712`) and crowbar stabbing (Normal+, HP > 80, d < 100, not in a group, not camping; `1594-1604`).
- Distance bands (yapb, refined): crowbar 0–48; glock 0–1200 (rapid ≤300); hornet 150–900 (dart ≤250, ≥4 hornets); python 300–3000 (zoom ≥800); mp5 0–900 (M203 300–700, refill-aware); shotgun 0–450 (double 32–300); crossbow zoom ≥700, bolt 200–700; rpg 350–2500 (never < 300); gauss 0–1500 (charge 500–3000); egon 128–1200; grenade 300–800; satchel 150–400 (jump to 800); snark 150–800.
- **Pistol swap:** if the clip is empty, a recognized threat is within 800u and the secondary is loaded, switch instead of reloading.

### 6.3 Target selection (10 Hz)
```
prio(e) = 1/(1+(d_eff/600)^2) · (1 + .5·aims_at_me + .5·firing + .3·dangerous_weapon + .3·[dmg_est ≥ 50%])
        · w_mode · 1.3^[current] · (0.2 if spawn-protected)
```
- GG: the leader gets `d_eff = d·√0.25` (yapb `cv_gungame_leader_priority` on distance², `combat.cpp:445-481`). A crowbar carrier while I'm high-level gets ×1.5. A grudge (killed me last) gets ×1.2.
- Snarks are targets only when approaching within 300u.

### 6.4 Aim model
1. **Body part**: one roll per contact.
   - p_head = headshot%(diff) × weapon factor: sniper/python ×1.2; shotgun 0 beyond 272u; MP5 ×0.5 beyond 544u; RPG and explosives aim at the feet when the target is on ground (`combat.cpp:741-743`).
   - Expert body z +9.8 plus `getCustomHeight` (`combat.cpp:714-716,753-786`). Sniper head −9.8.
   - Re-roll only on a weapon class change.
2. **Perceptual latency** (jk_botti ping emulation, `JK/bot_combat.cpp:402-490`): `p = obs(t−L) + v_est·L·(1+N(0, vel_noise))`. L = 0.30/0.24/0.18/0.12/0.06 s.
3. **Lead**: intercept solve with t_f from the mechanics (straight or ballistic).
4. **Error**: 3D Ornstein-Uhlenbeck noise, σ = aimError_xyz(diff)·(1 + d/(1280·clamp(diff,1,4))), τ = 0.35–0.6 s. This replaces the 0.4–0.8 s re-rolls (`combat.cpp:622-642`) and applies to all difficulties. Expert gets OU σ = 1.5u.
5. Between perception ticks the LookController follows `p(t) = p_obs + v_est·(t − t_obs)` every frame.

### 6.5 Fire control
- Preconditions: weapon ready (deployed, not switching or reloading, zoom as desired), permission (mode, FF, protection).
- Threshold: `θ = atan((r_part + d·tanσ)/d)·tol(diff)`. yapb fallback (`combat.cpp:1714-1775`): d < 128 → 36.9°; if the target faces me → 25.8°, else 8.1°; d < 90 → always.
- Semi-auto cadence: `max(cycle, 0.1 + U(min,max))`, e.g. Noob .7–.8 … Expert .1–.2 (`combat.cpp:1212-1218`).
- Automatic weapons are held.
- Trigger discipline (MP5 beyond 544u: bursts of 3–6, pauses 0.2–0.45 s) is cosmetic, per §0.3.
- Sniper stand-still: crossbow/zoomed python beyond 600u stops for 2 − 0.35·diff s, Sniper style only (`combat.cpp:1080-1096`).
- Post-kill overfire for clamp(aggr·1.25, .15, .25) s if the target was visible (`combat.cpp:559-577`).
- **Gauss rules:**
  - Never press IN_ATTACK while charging.
  - Combat release at age ≥ 1.3 s, or at the charge target.
  - Release only when the look error is < 2°.
  - Predicted reflection check: if the beam hits a surface at n < 0.5 within 2.5·dmg·n + 16 of self, re-aim.
  - Recoil-direction `move_safety` check.
  - Dump at 8 s (on the floor) or 9 s regardless; dump also on water ≥2 or an approaching ladder.
- **RPG guidance:** after firing, the Look channel stays with AimAndFire at P85 until impact (≤6 s). The laser tracks the target with lead. If the laser is off, aim straight. Never fire under 300u.

### 6.6 Combat-layer options (10 Hz)
- **Throws** (`checkGrenadesThrow` port, `combat.cpp:2553-2817`):
  - Check probability max(50, 25·diff)% per tick (`botlib.cpp:1207`).
  - Gates: not narrow, not reloading, not melee mode, not holding a gauss charge, 0.3 s cooldown, a track or hypothesis exists, the target is Suspect/Heard (not directly visible), the enemy is not firing at me, not just seen within 0.12 s, the enemy is not airborne above me or more than 500u higher.
  - Windows: HE 300–800; satchel 150–400 (jump to 800); snark 150–800; grenade war 96.
  - Cancel probability: 3% if aggr > fear, else 10%; satchel 10%; snark 25%.
  - `U_throw = Σ_n p(n)·dmg(|n − land|)/100 · (1 − cancel) · A_throw − self/friend risk`, compared with `U_shoot`.
- **Dodge** when an observed projectile threat is predicted.
- **Snark shooting.**

### 6.7 Combat movement (Fight port of `attackMovement`, `combat.cpp:1777-2003`)
- `approach = HP·aggr` (knife 100, suspect 49, reloading 29, sniper cap 49) plus style bias.
- Style re-check every U(1,3) s:
  - d < 768 → Strafe.
  - 768–1024 → Stay with chance {60,40,20,8,0}%.
  - Beyond 1024 → Stay {85,70,45,20,10}%.
  - Forced Stay when ducking, in a narrow spot below Normal, or with a partial view.
  - Forced Strafe when approach < 30 (with the fix above), when firing would hurt a friend, or with a pistol/shotgun within 1632u while the enemy faces me.
- Crouch tap {0,0,4,6,8}% for U(.25,.5) s.
- Strafe side: away from the enemy's observed aim side, 30% swap, every U(.3,.8) s, 134u lateral clearance from nav.
- Circle-strafe (Normal+): approach ≥ 60 and d > 400 → forward 0.5·maxspeed; approach < 60 and d < 300 → back 0.5·maxspeed.
- Cornered → backpedal (Normal+).
- Dodge-hop: Easy+, d < 1088, cooldown 6.5 − 1.25·diff, 30% per tick, speed2d > 150, not sniper or RPG, enemy facing me, **not on a ladder, and `move_safety` OK**.
- d < 96 (non-melee) → backpedal. Reloading → backpedal, no duck.
- **Band keeping** from WeaponPolicy, plus GG matchup: preferred distance = argmax_d(myDPS(d) − theirDPS(d)).
- **Ledge guard:** every component checked with `move_safety(pos, pos + v·0.2 + vel·dt)`. An unsafe component is zeroed and the other side tried.
- **Attack hop** (`navigate.cpp:765-808`): enemy at 400–750u, dz −64..+40, HP·aggr ≥ 30, view dot ≥ 0.95, pitch |·| ≤ 15° (LJ speed depends on pitch), `validate_launch`, then `StanceIntent::LongJump`.
- **Melee chase:** direct, or a path to the enemy's node (`botlib.cpp:965-998`).

### 6.8 Friendly fire and reload
- Line of fire: trace along the actual view forward (not the angle triple) to the target distance, with monsters on. A hit on a visible friend holds fire.
- Cone check against friends visible or seen ≤1 s ago: `atan(20/d_f) + spread`. Hold if the friend is inside and closer than the target.
- Explosives: the predicted blast radius must not include a known friend. Applies whenever mp_friendlyfire is on; bullets are also held when not, because a friend's body absorbs them.
- Reload when: clip < 25% and no recognized threat for 2 s; or clip < 5 while idle (`tasks.cpp:67-69`); or hiding. Only for weapons in the policy's top 2. Shotgun partial reloads are allowed when d > 500 and no shot is possible. Cancel if the clip is full (`botlib.cpp:1019-1036`).

---

## 7. Motor and arbiter (`lb-motor`)

### 7.1 Intents
```rust
pub enum Channel { Locomotion, Look, Stance, Weapon, Use }
pub struct IntentHeader { author: ActionId, prio: Prio, expires: SimTime, preempt: Preempt /*Free|Protocol{grace}|Never{until}*/, group: Option<ClaimGroup> }
pub enum MoveIntent { Stop, WorldVel{v: Vec3, max: f32}, Corridor(CorridorRef), AirStrafe{dir: Vec3, max_speed: f32, cap: Option<f32>}, Ladder{dir: ClimbDir} }
pub enum LookIntent { Point{p: Vec3, precision: f32}, Track{t: TrackId, part: BodyPart, lead: LeadSpec}, Angles(Angles), Sweep{c: Angles, amp: f32, period: f32} }
pub enum StanceIntent { Stand, Crouch, CrouchTap{dur: f32}, Jump(JumpKind /*Normal|DuckJump|LongJump*/) }
pub enum WeaponIntent { Select(WeaponId), LastInv, Fire{btn: Btn, pattern: Trigger /*Hold|Tap{min_interval}*/}, Charge, Release, Reload, Zoom(bool), Laser(bool) }
pub enum UseIntent { Press, Hold, Release }
```
- Priorities: **P100** lifecycle; **P90** mandatory traversal; **P85** mandatory weapon protocol; **P70** threat reaction (AimAndFire, dodge, damage snap); **P60** alert look (an unrecognized glimpse, a lost enemy, a shot or pain heard nearby); **P50** current goal; **P20** optional look.
- Per channel, the highest live priority wins; ties go to the incumbent.
- Atomic groups are granted all-or-nothing.
- Preempting a Protocol holder sends it a CancelRequest. It keeps the channel until it releases or its grace expires (≤0.3 s; gauss ≤0.6 s).
- Denials are reported back with `{by, until}`.

### 7.2 Frame order
fast-lane actions → arbiter → Look → Locomotion (projected on the **new** yaw) → Stance → Weapon → Use → Encoder → `MotorFeedback{sent_buttons, angles, jump_at, fire_at, cmd_sent_at}`.

### 7.3 Look controller (fps-independent)
- The spring integrates each frame's own dt in sub-steps of at most 1 ms, so the view moves on every frame at any server rate (at 1100 fps with 100 commands/s a fixed 1/120 s step froze it for 8 frames of 9). The newbie model steps at exactly 90 Hz for parity.
- Spring (port of `vision.cpp:113-209`), scaled by the skill's turn acceleration A (`turn_accel`):
  - `acc = clamp(k·Δ − c·ω, ±A)`, k = A/15, c = 2ζ√k with ζ 0.884 (yapb's 200 / 25 at A 3000). Pitch uses 2k.
  - Snap when |yaw error| < 1°.
  - Urgency from the granted priority: an aim at an enemy is Engaged, anything from P60 up is Alert (full A), lower looks are Calm (half A).
  - Engaged on the combat model (Hard/Expert): k ×1.5, ζ 0.7.
  - Per-difficulty A: 3000 / 5000 / 9000 / 15000 / 24000 °/s²; yaw rate cap: 300 / 600 / 1000 / 1600 / 2500 °/s. A 90° flick takes about 0.3 / 0.22 / 0.13 / 0.1 s from easy to expert.
  - The obstacle courses and bots sent by hand (`lb do`) keep yapb's look (`LookParams::NAV`: A 3000, 900 °/s).
  - Reverse-facing guard when navigating (`vision.cpp:163-186`): not implemented.
- Newbie model (`vision.cpp:211-292`) for Noob: spring 13, damper 0.22, influence (.25,.17)·(100−off)/100, randomization (2, .18)·(100−off)/100, re-randomized every U(.4, 1.2) s.
- Command viewangles equal the actual look. yapb's separate move angles (`botlib.cpp:2390-2397`) are gone.

### 7.4 Locomotion, stance, weapon controller, encoder
- **Locomotion:** fwd = v·f̂, side = v·r̂ on the current yaw, clamped to maxspeed. Ducking scales by 0.333 in pm. Silent walk ≤ 210 u/s when `mp_footsteps` is on.
- **Stance:**
  - Jump needs IN_JUMP absent in the previous *sent* command.
  - LongJump = IN_DUCK newly pressed + IN_JUMP newly pressed in the same command. The previous command had neither. On ground, |v| > 50.
  - DuckJump = jump, then hold duck on the next command.
  - IN_JUMP is suppressed on ladders unless the ladder executor asks for it.
- **Weapon controller:**
  - Select sends the canonical client command from WeaponList (`weapon_9mmhandgun`, `weapon_357`, `weapon_9mmAR` …). Confirm via CurWeapon (state=1, id) within 1.0 s; retry once, then fail with a reason (for example, RPG guiding).
  - Deploy lock 0.5 s.
  - Fire buttons are suppressed while switching.
  - Unexpected CurWeapon messages (auto-switch, GG re-equip) are handled.
  - Zoom confirmed via own fov. Reload is an edge press.
  - IN_ATTACK and IN_ATTACK2 are never both set unless intended.
- **Encoder:** sets IN_FORWARD/BACK/MOVELEFT/MOVERIGHT consistent with the analog signs (ladders use buttons only), press/release edges relative to the last sent command, impulse (100 = flashlight).
- **Bhop hook:** `AirStrafe` owns L+K atomically.
  - Optimal wish angle θ = acos((30 − sv_airaccelerate·30·dt)/|v|).
  - Landing jump press with jitter from the difficulty table.
  - Target speed ≤ 0.97·1.7·maxspeed when capped (verify `mp_bunnyhop`). When uncapped, style/difficulty limit: 1.25 / 1.5 / 1.7·maxspeed.
- At 1000 fps the per-frame cost is O(1) per bot. No traces run in the motor.

---

## 8. Styles, difficulty, emotions, RNG (`lb-styles`)

The table below is the plan the presets started from. They have since been sped up to Half-Life's pace (recognition,
its floor, aim latency, turn rate and acceleration, semi-automatic pauses, the scope's settling, fight movement): the
shipped values are in `data/config/difficulty.yaml`, `docs/perception.md` and `docs/behavior.md`.

```yaml
# config/ai/difficulty.yaml  (yapb shipped difficulty.cfg + combat.cpp tables + jk_botti bot_skill.cpp latency)
schema: lambdabots.difficulty/1
default: normal
presets:
  noob:   {perception: {recognition_delay_s: [1.5,2.0], reacquire_grace_s: 1.0, reacquire_delay_s: 0.35, latency_s: 0.30,
                        vel_noise: 0.10, hear_threshold: 0.07, bearing_sigma_min_deg: 35, item_view_range: 900, periph_gain: 0.35},
           aim: {model: newbie, headshot_pct: 15, error_units: [20,20,40], error_tau_s: 0.6, max_yaw_rate: 180,
                 fire_tolerance: 1.6, semi_auto_extra_s: [0.7,0.8], burst_tolerance: 40},
           combat: {stay_mid_pct: 60, stay_long_pct: 85, crouch_tap_pct: 0, dodge_hop_cooldown_s: null, sniper_stand_s: 2.0,
                    distance_switch: false, stab_close: false, circle_strafe: false, grenade_check_pct: 50, preaim: false},
           tricks: {gauss_precharge_pct: 0, gauss_jump: false, satchel_jump: false, bhop: off, lj_attack_hop: false},
           tactics: {cover_candidates: 4, item_timing_sigma_s: 6.0, forget_horizon_s: 4, use_experience: false}}
  easy:   {perception: {recognition_delay_s: [1.0,1.5], reacquire_grace_s: 1.5, reacquire_delay_s: 0.25, latency_s: 0.24, vel_noise: 0.075,
                        hear_threshold: 0.055, bearing_sigma_min_deg: 28, item_view_range: 1100},
           aim: {model: spring, spring: {k: 200, c: 25, accel: 3000}, headshot_pct: 20, error_units: [15,15,30], max_yaw_rate: 300,
                 fire_tolerance: 1.3, semi_auto_extra_s: [0.5,0.6], burst_tolerance: 35},
           combat: {stay_mid_pct: 40, stay_long_pct: 70, crouch_tap_pct: 0, dodge_hop_cooldown_s: 5.25, sniper_stand_s: 1.65, grenade_check_pct: 50},
           tricks: {gauss_precharge_pct: 50}, tactics: {cover_candidates: 6, item_timing_sigma_s: 4.0, forget_horizon_s: 6}}
  normal: {perception: {recognition_delay_s: [0.5,1.0], reacquire_grace_s: 2.0, reacquire_delay_s: 0.15, latency_s: 0.18, vel_noise: 0.05,
                        hear_threshold: 0.04, bearing_sigma_min_deg: 20, item_view_range: 1400},
           aim: {model: spring, spring: {k: 200, c: 25, accel: 3000}, headshot_pct: 25, error_units: [10,10,20], max_yaw_rate: 450,
                 fire_tolerance: 1.0, semi_auto_extra_s: [0.4,0.5], burst_tolerance: 30},
           combat: {stay_mid_pct: 20, stay_long_pct: 45, crouch_tap_pct: 4, dodge_hop_cooldown_s: 4.0, sniper_stand_s: 1.3,
                    distance_switch: true, stab_close: true, circle_strafe: true, grenade_check_pct: 50, preaim: true},
           tricks: {gauss_precharge_pct: 60, gauss_jump: true, satchel_jump: true, bhop: {max_speed_mult: 1.25, jitter_ms: 40}, lj_attack_hop: true},
           tactics: {cover_candidates: 8, item_timing_sigma_s: 2.5, forget_horizon_s: 8}}
  hard:   {perception: {recognition_delay_s: [0.25,0.5], reacquire_grace_s: 2.5, reacquire_delay_s: 0.10, latency_s: 0.12, hear_threshold: 0.03, bearing_sigma_min_deg: 14, item_view_range: 1700},
           aim: {model: spring, spring: {k: 200, c: 25, accel: 3000}, engaged: {k: 300, c: 20, accel: 3000}, headshot_pct: 50, error_units: [5,5,10],
                 max_yaw_rate: 650, fire_tolerance: 0.85, semi_auto_extra_s: [0.3,0.4], burst_tolerance: 25},
           combat: {stay_mid_pct: 8, stay_long_pct: 20, crouch_tap_pct: 6, dodge_hop_cooldown_s: 2.75, sniper_stand_s: 0.95, grenade_check_pct: 75},
           tricks: {gauss_precharge_pct: 70, bhop: {max_speed_mult: 1.5, jitter_ms: 20}}, tactics: {cover_candidates: 12, item_timing_sigma_s: 1.5, forget_horizon_s: 10}}
  expert: {perception: {recognition_delay_s: [0.1,0.25], reacquire_grace_s: 3.0, reacquire_delay_s: 0.05, latency_s: 0.06, hear_threshold: 0.02, bearing_sigma_min_deg: 10, item_view_range: 2000},
           aim: {model: spring, engaged: {k: 300, c: 20, accel: 3300}, headshot_pct: 75, error_units: [0,0,0], ou_floor_units: 1.5,
                 max_yaw_rate: 900, fire_tolerance: 0.7, semi_auto_extra_s: [0.1,0.2], burst_tolerance: 20},
           combat: {stay_mid_pct: 0, stay_long_pct: 10, crouch_tap_pct: 8, dodge_hop_cooldown_s: 1.5, sniper_stand_s: 0.6, grenade_check_pct: 100},
           tricks: {gauss_precharge_pct: 80, bhop: {max_speed_mult: 1.7, jitter_ms: 10}}, tactics: {cover_candidates: 16, item_timing_sigma_s: 1.0, forget_horizon_s: 12}}
```

```yaml
# config/ai/styles/balanced.yaml (base; others list only deltas)
schema: lambdabots.style/1
id: balanced
emotion: {aggression_base: [0.4,0.7], fear_base: [0.4,0.7]}          # manager.cpp:1282-1286
risk_aversion: 1.0
goal_affinity: {engage: 1.0, hunt: 1.0, investigate: 1.0, retreat: 1.0, collect_item: 1.0, control_item: 0.6, use_charger: 1.0,
                roam: 1.0, camp: 0.3, ambush: 0.5, plant_trap: 0.5}
roam_tactics: {goal: 1.0, forward: 1.0, backoff: 1.0, camp: 0.3}      # navigate.cpp:32-63
weapons: {order: [crowbar,tripmine,satchel,snark,handgrenade,glock,hornetgun,python,crossbow,shotgun,mp5,rpg,gauss,egon], # config.h:58
          ammo_thrift: 0.5, switch_commitment_s: 1.5, self_damage_aversion: 1.0, throwables: 1.0}
item_value: {healthkit: 1.0, battery: 1.0, longjump: 1.0, weapon: 1.0, ammo: 1.0}
combat: {approach_bias: 0, chase_persistence: 1.0, retreat_mult: 1.0, silent_walk_near_threat: 0.3}
tricks: {lj_travel: 0.8, lj_attack_hop: 0.6, gauss_jump_p: 0.33, satchel_jump_life_p: 0.40, rocket_jump: 0.1, grenade_jump: 0.0, bhop: 0.5}
traps: {tripmine_camp_guard_p: 0.50, narrow_bonus: 0.20, corner_cooldown_s: [20,30], satchel_trap: 0.3, rush_miner_life_p: 0.50}
gungame: {leader_priority: 0.25, behind_aggression: 0.2, leading_fear: 0.15, crowbar_avoid_distance: 350}
```

| Delta           | Rusher                                        | Sniper/Camper                                      | Map Controller                               | Explosives/Trapper                                               |
|-----------------|-----------------------------------------------|----------------------------------------------------|----------------------------------------------|------------------------------------------------------------------|
| aggr / fear     | [.7,1] / [0,.4]                               | [.2,.5] / [.7,1]                                   | [.45,.7] / [.4,.6]                           | [.3,.6] / [.5,.8]                                                |
| risk_aversion   | 0.3                                           | 1.5                                                | 1.0                                          | 1.2                                                              |
| affinities      | hunt 1.4, camp 0, retreat 0.6, engage 1.2     | camp 2.0, ambush 1.5, retreat 1.4, hunt 0.3        | control_item 2.0, collect 1.3, roam.goal 1.6 | plant_trap 2.0, ambush 1.2, throwables 1.6                       |
| weapon order    | yapb Rusher (`config.h:59`)                   | yapb Careful with python moved just below crossbow | …, egon, rpg, gauss last                     | guns: glock…mp5, rpg; throwables 1.6                             |
| prefer range    | 0–250–800                                     | 400–1200–4000                                      | 0–600–2000                                   | 150–500–1500                                                     |
| retreat HP knee | 25                                            | 55                                                 | 40                                           | 40                                                               |
| tricks          | lj_attack_hop 1.0, satchel_jump 0.6, bhop 0.9 | satchel_jump 0.2, bhop 0.2                         | gauss_jump 0.5, lj_travel 1.0                | satchel_jump 0.7, grenade_jump 0.05                              |
| traps           | camp_guard 0.3, rush_miner 0.8                | camp_guard 0.7, rush_miner 0.25                    | camp_guard 0.4, rush_miner 0.4               | camp_guard 0.9, corner [10,18], satchel_trap 0.9, rush_miner 0.6 |
| item values     | –                                             | –                                                  | battery 1.5, LJ 1.4, gauss 1.3, rpg 1.3      | –                                                                |

```yaml
# config/ai/profiles/viper.yaml
schema: lambdabots.profile/1
name: Viper
model: gordon
style: rusher
difficulty: hard
overrides: {perception.recognition_delay_s: [0.3,0.45], aim.headshot_pct: 40}
style_overrides: {emotion.aggression_base: [0.85,0.95], tricks.bhop: 0.9}
seed: 12345                      # optional master seed
```
Resolution order: preset, then profile overrides, then style, then style overrides. Every value records its provenance.

**Emotions** (fixed version of `botlib.cpp:930-958,1149-1160,2152-2165`):
- Every 0.5 s: if an enemy was seen within 1 s, aggr += .05; if none for 5 s, aggr and fear step 0.05 toward base.
- Kill: aggr += .1.
- Damaged by an enemy: if HP > 60, aggr += .1, else fear += .03.
- Everything is clamped to [0,1].
- GG offsets are applied at resolution time and never stored.

**RNG:** PCG32 streams `{Perception, Decision, Combat, Motor, Cosmetic, Profile}`, each seeded as hash(master, domain). Draws are keyed by purpose (the contact delay is drawn at contact creation). Seeds and the profile version go to the trace. Cosmetic draws never touch the other streams.

---

## 9. Game modes (`lb-modes`)
```rust
pub trait GameMode: Send + Sync {
  fn kind(&self) -> ModeKind;                                           // Ffa | Tdm | GunGame{team}
  fn relation(&self, me: PlayerKey, other: PlayerKey, pubs: &PublicState) -> Relation;
  fn allows_pickup(&self, c: ItemClass) -> bool;
  fn forced_weapon(&self, s: &SelfState, gg: Option<&GunGameView>) -> Option<WeaponId>;
  fn goal_modifiers(&self, ctx: &DecisionContext, cands: &mut [GoalCandidate]);
  fn target_modifier(&self, ctx: &DecisionContext, t: &TrackView) -> f32;
  fn fire_permission(&self, ctx: &DecisionContext, w: WeaponId, tgt: TargetKind) -> FirePermission;
  fn self_damage_policy(&self, ctx: &DecisionContext) -> SelfDamagePolicy;
  fn on_public(&mut self, ev: &PublicEvent); }
pub struct GunGameView { source: GgSource /*Bridge|Inferred*/, warmup: Warmup, levels: Arc<[GgLevel]>, me: GgPlayer,
  players: SmallVec<[GgPlayer; 32]>, leader: Option<PlayerKey>, descore: DescoreMode /*SelfKill|CrowbarSteal|None*/, team: bool }
```

**Inference**, without the bridge:
- GG is detected via `gg_enabled`.
- Level = ⌊frags/100⌋ and kills_in_level = frags mod 100 (the plugin sets frags = level·100 on each change).
- The level list comes from `rules/gungame.yaml`, which mirrors `GG/configs/gungame/gungame.ini`: hornet 4, 9mmAR 4 (M203 +1 every 20 s), shotgun 3, crossbow 3, gauss 4, egon 3, rpg 4, 357 3, glock 3, tripmine+glock 2 (limit 10 mines, glock damage blocked), handgrenade 2, crowbar+LJ+2 batteries 1. Team mode multiplies kills by `gg_teamplay_multigoal`.
- Own class from inventory (ports `isGunGameMeleeLevel`, `isGunGameTripmineLevel`, `isGrenadeWar`, `refreshForcedWeapon` in `botlib.cpp:304-320,475-496` and `combat.cpp:2144-2146`).
- Enemies' weapons: level-list prediction, cross-checked against the observed weaponmodel.
- Warmup: `gg_warmup` seconds from map start, plus crowbar+LJ inventory and all frags 0.

**Bridge** (optional AMXX shim; transport chosen by integration): exact per-player level, kills and weapon, leader (`api_get_leader_id`, `gungame.sma:4298-4305`), warmup, descore mode.

**Behaviors:**
- Ported: forced weapon (`combat.cpp:2152-2169,1606-1623`); no weapon/ammo/weaponbox goals, but health/battery/LJ kept.
- Tripmine level: fire permission denied against players. Proactive plant with wall ≤90u, every 2.5–4 s, not while seeing an enemy (`botlib.cpp:322-368`). Rush miner rolled per life (Balanced 50 / Rusher 80 / Sniper 25 / Controller 40 / Trapper 60) at 1–1.5 s cadence. Corner mines every 8–12 s. DetonateTripmine at 350–1200u, enemy ≤140u from the mine, abort at 160u. Enemy mines also count, since the shooter gets the credit.
- Melee level: hunt ≥ 0.7.
- Grenade war: relaxed throw gates, minimum 96u, but self-risk is respected.
- Leader priority 0.25 (`combat.cpp:445-481`).
- New: avoiding crowbar carriers. Active when `descore = CrowbarSteal` and I am in the top third of levels or at least 2 levels above the carrier. Triggered by an observed p_crowbar or an inferred melee level. Keep ≥350u, kite back while firing, prioritize the kill at range, avoid his hypothesis region (path cost).
- New: descore on self-kill (the current plugin). Self-damage aversion ×4; RPG never within 350u; grenades land ≥280u away; no rocket or grenade jumps; gauss jump only at HP ≥ 80.
- New: warmup. Aggressive melee Hunt, LJ attack-hop chance ×2, no Camp, LJ travel everywhere.
- New: level tactics. Two or more levels behind the leader: aggr +0.2, ρ ×0.7. Leading: fear +0.15, ρ ×1.3, prefer ambush suited to my weapon's range. Weapon matchup sets Fight bands (6.7). One kill from a level-up with low HP: aggr +0.15 (level-up restores 100 HP and sometimes grants armor, `gungame.sma:2742-2750`).
- Team GG: TDM relations plus the board.
- WSET_BOTCANT skips are handled because the weapon controller copes with sudden re-equips.

**FFA/TDM:** relations; FF (6.8); board (4.4); spawn-protection handling applies in every mode.

---

## 10. Tests and metrics

**Pure logic (`lb-sim-tests`):**
- **Hidden-state invariance:** property tests (proptest) perturb hidden state (enemy positions behind walls, out-of-view item pickups, door states, enemy HP) while keeping the sensor-visible state fixed. The PerceptBatch, DecisionTrace and CmdOut must be byte-identical, with fixed seeds.
- **Dependency and provenance guards:** the crate-graph check and the provenance assertions from 1.1.
- **Recognition:**
  - One delay draw per contact.
  - LOS gaps shorter than the grace do not resample.
  - A K-S test against the difficulty table.
- **Hearing:**
  - The footstep rule matches pm_shared (220 threshold, cadences).
  - Attenuation and PAS gates.
  - Localization error distribution.
  - The HUD compass matches `health.cpp`.
- **Arbiter conflict tests:**
  - Enemy appears while the bot is on a ladder: no IN_JUMP; Look stays with traversal or goes through the yield protocol; no firing without the look.
  - Retreat while shooting: path progress ≥ 90% of speed while looking backward.
  - Gauss: IN_ATTACK is never sent while charging; release comes only after convergence; the 8 s limit holds; switching is denied.
  - Cooked grenade with the target lost: safe throw before fuse − 0.3 s.
  - LongJump: the press edges are exact.
- **Goal inertia:** under oscillating utilities, ≤6 switches/min, minimum holds respected, urgent preemption works.
- **Weapon protocols:**
  - Missing CurWeapon: retry, then fail.
  - Zoom confirmed via fov.
  - RPG switch is blocked while guiding.
  - Shotgun reload is interruptible.
- **Look controller:** trajectories at 60/100/250/500/1000 fps agree within 0.1° RMS; the newbie model matches yapb at 90 Hz.
- **Encoder:** button and analog signs are consistent; ladders use buttons.
- **Ballistics fixtures:** grenade pitch→speed formula, satchel, M203, rocket, all checked against recorded server trajectories.
- **GG inference:** ScoreInfo to levels, leader, level classifiers, descore policies.
- **Experience partitions:** FFA and TDM persistence.

**Stand scenarios** (scripted dummy clients on test maps plus crossfire, stalkyard, boot_camp):
- Reaction chain across distance×angle grids per difficulty, N = 50.
- Hidden pickup: belief unchanged until observed or heard.
- Sound localization and investigation.
- Grenade dodge; satchel trap; corner tripmine; gauss jump (no reflection self-damage); LJ attack hop; bhop cap compliance.
- Ladder encounter; retreat-and-shoot.
- No shots at spawn-protected targets.
- Full GG match with 8 bots: level progression, no weapon pickups, tripmine-level behavior, self-kill descores per hour.
- FPS matrix 100/500/1000 with the same outcomes.
- DllProfile self-test for satchel, grenade and zoom.

**Metrics:**
- Chain t_first_evidence → t_recognized → t_decision → t_weapon_ready → t_aim_ready → t_first_shot (p50/p95 per difficulty), new contacts apart from re-acquisitions. Targets (`crates/lb-brain/tests/reaction.rs`): an enemy coming into view near the crosshair is shot at in p50 0.10–0.16 s by an expert, 0.15–0.24 s hard, 0.18–0.36 s normal, 0.25–0.65 s easy.
- Accuracy by weapon × distance band.
- Goal switches/min; goals abandoned before the minimum hold.
- stuck/min.
- Self-damage deaths/h; teamkills; wasted shots at protected targets.
- Item-control timing error.
- K/D per style against a reference human group.
- AI time p50/p95/p99 per rate class.
- Honesty violations = 0.

---

## 11. AI milestones and risks

| Stage             | Scope                                                                                                                                                                                                                                                                                                                                        | Verification                                                                                                                             |
|-------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------|
| **A0** (with M0)  | Crates plus the dependency guard; SelfState/messages; BotBrain API; Encoder; basic look and move; lifecycle and respawn                                                                                                                                                                                                                      | Encoder and arbiter unit tests; add/kill/respawn without stuck buttons                                                                   |
| **A1** (M1 slice) | Honest vision (FOV/LOS/evidence), gunfire events and footsteps, damage compass; tracks without diffusion; basic items; arbiter plus channels; spring look; glock (primary+rapid), shotgun (primary), MP5 primary; Engage, Hunt (last known), Retreat (simple), CollectItem, Roam; FollowPath, Fight, AimAndFire, Reload; FFA; decision trace | Invariance tests pass; bot notices, attacks and loses contact without hidden knowledge; reaction chain measured; no conflicting commands |
| **A2**            | Node-belief diffusion, negative observations, pre-aim; respawn windows plus learning; UseCharger; ExperienceMap; Camp, Ambush, Investigate; emotions; 5 styles and 5 presets in YAML; TDM (FF, board)                                                                                                                                        | Goal thrashing and accuracy targets; hidden pickup test; TDM teamkill rate 0                                                             |
| **A3**            | Full arsenal (python zoom, crossbow modes, RPG guidance, gauss controller, egon, hornet, M203, double shotgun, grenade cook, satchel with DllProfile, tripmine static/corner, snark); projectile perception and dodge; ShootBreakable                                                                                                        | Per-weapon scenarios; self-damage deaths/h below threshold                                                                               |
| **A4**            | Tricks: LJ travel and attack hop, gauss jump, satchel jump, rocket/grenade jump (with nav), cap-aware bhop; spawn protection                                                                                                                                                                                                                 | Scenario success ≥90%; no cap violations; no protected-target shots                                                                      |
| **A5**            | GunGame (inference plus bridge), every GG behavior, team GG                                                                                                                                                                                                                                                                                  | Full matches; descore rate; leader focus measured                                                                                        |
| **A6**            | Calibration and blind tests; 8/12 bots at 1000 fps within budget                                                                                                                                                                                                                                                                             | v2 §12 metrics documented                                                                                                                |
| **A7**            | BehaviorModule overlays, then Lua bindings                                                                                                                                                                                                                                                                                                   | A map scenario added via the API with no invariant breaks                                                                                |

**Risks:**
- Hook completeness: PlaybackEvent, SV_StartSound and TE hooks for fake clients on ReHLDS; PAS query API. Mitigation: the footstep synthesizer and stand tests.
- Semantics that differ per DLL: satchel buttons, grenade formula, crossbow damage. Mitigation: DllProfile and the map-start self-test.
- GG plugin: its descore semantics are self-kill, while crowbar-steal is requested. Handled as a config switch, but it must be confirmed with the server owner.
- `mp_bunnyhop` defaults to "uncapped" in BHL; confirm the production config.
- Spawn-protection plugin cue specifics: learn them, but seed the YAML.
- Gauss reflection and floor punch-through geometry: needs stand validation of the 32–40° pitch window.
- Cost of graph diffusion on large maps (bounded by node caps).
- Coupling to nav APIs: `move_safety`, `validate_launch`, vistable, travel estimates.
- Human-likeness tuning is empirical.
- The fixed-step look plus RunPlayerMove timing at high fps depends on integration's msec accumulation.

### Critical Files for Implementation
- /Users/nikita/Git/half-life/yapb-halflife/src/combat.cpp
- /Users/nikita/Git/half-life/yapb-halflife/src/botlib.cpp
- /Users/nikita/Git/half-life/yapb-halflife/src/tasks.cpp
- /Users/nikita/Git/half-life/yapb-halflife/src/vision.cpp
- /Users/nikita/Git/half-life/BugfixedHL-Rebased/src/game/server/gauss.cpp (plus satchel.cpp, handgrenade.cpp, pm_shared/pm_shared.cpp for the mechanics model)
