# yapb-halflife analysis: behavior, combat, weapons

> Original design note (in English), prepared during planning on 2026-09-26.
> A condensed version with the final decisions lives in the implementation plan; in case of conflict, the plan
> and later decisions in the repository take precedence. `file:line` references reflect the sources as of the note's date.

# YaPB-HL: behavior, combat and weapon spec

**Source files** (line references below use the bare filename; all lines are from HEAD `3f62b3a`):
- `/Users/nikita/Git/half-life/yapb-halflife/src/botlib.cpp`
- `/Users/nikita/Git/half-life/yapb-halflife/src/tasks.cpp`
- `/Users/nikita/Git/half-life/yapb-halflife/src/combat.cpp`
- `/Users/nikita/Git/half-life/yapb-halflife/src/vision.cpp`
- `/Users/nikita/Git/half-life/yapb-halflife/src/navigate.cpp` (behavior parts only)
- `/Users/nikita/Git/half-life/yapb-halflife/src/manager.cpp`
- `/Users/nikita/Git/half-life/yapb-halflife/src/config.cpp`
- `/Users/nikita/Git/half-life/yapb-halflife/src/chatlib.cpp`
- `/Users/nikita/Git/half-life/yapb-halflife/src/sounds.cpp`
- `/Users/nikita/Git/half-life/yapb-halflife/src/engine.cpp`
- `/Users/nikita/Git/half-life/yapb-halflife/src/message.cpp`
- `/Users/nikita/Git/half-life/yapb-halflife/inc/constant.h`
- `/Users/nikita/Git/half-life/yapb-halflife/inc/yapb.h`
- `/Users/nikita/Git/half-life/yapb-halflife/inc/config.h`
- `/Users/nikita/Git/half-life/yapb-halflife/inc/support.h`
- `/Users/nikita/Git/half-life/yapb-halflife/inc/vision.h`
- `/Users/nikita/Git/half-life/yapb-halflife/cfg/addons/yapb/conf/{difficulty.cfg,weapon.cfg,yapb.cfg,lang/en_chat.cfg}`

**Notation**
- `rg(a,b)` is an inclusive uniform random value (float or int). `chance(p)` means `rand[0..99] < p`.
- "Heavy tick" means `setConditions` (10 Hz). "Think tick" means `logic()` (default 90 Hz).
- **The shipped `yapb.cfg` overrides several code defaults:** `yb_attack_monsters 1` (code default 0), `yb_pickup_ammo_and_kits 1` (code default 0), `yb_difficulty 3`, and camping time 5–15 s.

---

## 1. Main think loop and update frequencies

### 1.1 Server frame (`linkage.cpp:364-420`, StartFrame hook)

Each server frame runs these steps in order:
1. `util.updateClients()` (`support.cpp:235`). Refreshes the per-client origin, used/alive flags and FFA team (`team = index+1`). For every alive client it calls `sounds.simulateNoise`.
2. `game.slowFrame()` (`engine.cpp:1048`), roughly once per second (`clamp(75*frametime, 0.5, 1)`):
   - bot difficulty sync, autokill, leader selection, light levels, narrow places;
   - `applyGameModes()`: teamplay/FFA and **GunGame detection** from cvar `gg_enabled` (configurable) or `yb_force_gungame`;
   - `bots.updateTripmines()`: rescans every `monster_tripmine`.
3. `gameState.updateActiveGrenade()` every ≥0.25 s: all `grenade` plus `monster_satchel` entities (`engine.cpp:1702`).
4. `updateInterestingEntities()` every ≥0.5 s: `weapon_*`, `ammo_*`, `weaponbox*`, `grenade*`, `item_*`, chargers, buttons, and FL_MONSTER entities if `attack_monsters` (`engine.cpp:1724`).
5. `bots.frame()` calls `Bot::frame()` for each bot.

The "round" concept: `GameState::roundStart()` runs **once per map** (`engine.cpp:1670`).
- roundMid = start + 90 s; roundEnd = start + mp_timelimit (or 1 day).
- It calls `bots.initRound()`, which resets leaders, clears the tripmine registry, calls `newRound()` on all bots and runs `practice.update()`.

### 1.2 `Bot::frame()` (`botlib.cpp:1653`)
- **Think gate:** `update()` runs when `m_thinkTimer.time < now`, then time += interval.
  - interval = `1/clamp(yb_think_fps, 30, 90)`; default 90 → 11.1 ms (`manager.cpp:1747-1760`).
  - Xash with fps < 50 forces 1/50. Frame-skip disabled means 0.
- **Slow section every 0.5 s** (runs even when dead):
  - `checkSpawnConditions()`, `checkForChat()`, `checkBreakablesAround()`;
  - `m_hasLongJump = physinfo "slj" == "1"` when alive;
  - rotation kick.

### 1.3 `Bot::update()` (per think; `botlib.cpp:1684-1760`)
1. Set `m_canSetAimDirection=true`, `m_isAlive`, `m_team`, `m_healthValue = clamp(health)`, `m_isCreature` (model name starts with "zo"/"ch"; see §14), `refreshForcedWeapon()`.
2. Clear the expired damage timestamp and re-set FL_FAKECLIENT.
3. If `maxspeed < 10` and task is Normal, set `pev->maxspeed = sv_maxspeed`.
4. If `m_notStarted`, run `updateTeamJoin()` (20% sets the welcome-chat flag).
5. Else if dead: tkpunish path (`vote %d`/`votemap`, CS-only commands). With `tkpunish==2` it slays the human teamkiller and gives them +1 frag. Note this branch `return`s, skipping `runMovement`.
6. Else movement is allowed if `maxspeed ≥ 10 && !freeze_bots && !graph.hasChanged()`.
7. `checkMsgQueue()` (chat send).
8. If movement is allowed, run `logic()`. Otherwise `resetMovement()`, and if dead call `checkRespawn()`.
9. `runMovement()`: `translateInput()` then `pfnRunPlayerMove(getRpmAngles(), moveSpeed, strafeSpeed, buttons, impulse, msec)`.
   - `getRpmAngles` (`botlib.cpp:2390`) returns `v_angle` if stuck, approaching a ladder, or **task == Attack**. Otherwise it returns `m_moveAngles` (the direction to `m_destOrigin`).

### 1.4 `Bot::logic()` (per think, alive; `botlib.cpp:1814-1956`), exact order
1. `resetMovement()`: `button=0`, speeds 0, moveAngles cleared.
2. `m_actualReactionTime += 0.3`, clamped to `m_idealReactionTime`.
3. `m_viewDistance += 3`, clamped to max. If blind, `maxViewDistance=4096`.
4. `m_moveSpeed = pev->maxspeed`. Every 0.2 s: moved-distance sample.
5. **If `canRunHeavyWeight()` (0.1 s): `setConditions()`.** Else if `m_enemy` is set: `trackEnemies()`. So enemy tracking runs at 90 Hz while engaged and 10 Hz otherwise.
6. `m_checkTerrain=true; m_moveToGoal=true; m_wantsToFire=false`.
7. `avoidGrenades()` if cvar and not creature.
8. `m_isUsingGrenade=false`.
9. **`executeTasks()`**: calls the top task's function.
10. **`setAimDirection()`**, then **`updateLookAngles()`**.
11. **`doFireWeapons()`**: calls `fireWeapons()` if `shootAtDeadTime > now || (wantsToFire && !usingGrenade && shootTime ≤ now)`.
12. **`updateGaussCharge()`**.
13. `checkReload()` if `m_reloadCheckTime ≤ now`.
14. `setIdealReactionTimers()`: a new random ideal reaction every tick.
15. `m_moveAngles = angles(m_destOrigin − (origin + vel*frameInterval))`, with pitch inverted.
16. `overrideConditions()`: knife chase, sniper stop, reload-done.
17. If `m_moveToGoal`: `moveToGoal()`.
18. If `m_checkTerrain`: `checkBreakable(nullptr)`, `doPlayerAvoidance()`, `checkTerrain()`.
19. `checkFall()`, then `checkDarkness()` (flashlight).
20. Grenade-avoid movement: if `m_needAvoidGrenade≠0`, un-duck and set `move=-maxspeed`, `strafe=±maxspeed`, unless that spot `isDeadlyMove`.
21. Pickup-stuck guard: if moving and `rg(2.5,3.5) + navTimeset + dist2d(dest)²/moveSpeed² < now` and not seeing an enemy, call `ensurePickupEntitiesClear()`.
22. `checkParachute()`, `checkLongJump()`, `checkGaussJumpTravel()`, debug overlay, `m_prevSpeed`, `m_lastDamageType=-1`.

### 1.5 `setConditions()` (10 Hz; `botlib.cpp:1138-1224`)
1. **`m_aimFlags = 0`**. This is the only per-cycle reset, so aim flags set by tasks are sticky for up to 100 ms (see §14).
2. `updateEmotions()`: internally every 0.5 s.
3. `trackEnemies()`, then last-victim processing: aggression +0.1 (cap 1), 10% Kill chat, or TeamKill chat.
4. `m_numFriendsLeft/m_numEnemiesLeft`: global alive counts.
5. Invalidate `m_lastEnemy` if dead and `shootAtDead` has expired.
6. Hearing: `updateHearing()` if `soundUpdateTime<now && !blind && seeEnemyTime+0.5<now`, then `soundUpdateTime=now+0.05`. The `else if` clear branch is effectively unreachable (§14).
7. `refreshEnemyPredict()`.
8. `checkGrenadesThrow()` with `chance(max(50, diff*25))` if not creature.
9. `checkProactiveTripminePlant()`, `checkTripmineDetonate()`, `checkSatchelDetonate()`.
10. `updatePickups()` if `itemCheckTime<now` or a pickup is targeted, then `itemCheckTime=now+0.5`.
11. `filterTasks()`.

### 1.6 Timers and cadences

| Timer | Value | Ref |
|---|---|---|
| Enemy full rescan | 0.85 s (knife 1.25) | combat.cpp:484 |
| Enemy reachability recheck | 1 s | botlib.cpp:1604 |
| Fight-style re-roll | rg(1,3) s | combat.cpp:1866 |
| Strafe direction | rg(0.3,0.8) s | combat.cpp:1887 |
| Aim error refresh | rg(0.4,0.8) s | combat.cpp:639 |
| Camp look direction | rg(1,4) s | tasks.cpp:517 |
| Retreat (seek-cover) sampling | rg(1,4) s | botlib.cpp:1296 |
| Reload check | 3 s | combat.cpp:2355, 1025 |
| Grenade check | 0.3 s after pass; 15 s if no throwables; 1.2 s after self-danger abort; 1.5 s after a successful HE throw | combat.cpp:2578, 2593; tasks.cpp:745; combat.cpp:2541 |
| Gauss roam precharge re-roll | rg(6,10)/rg(6,12) s | combat.cpp:1455, 1421 |
| Gauss travel-jump check | rg(4,6) s (not far) / rg(10,18) s | combat.cpp:1545, 1548 |
| Proactive tripmine | rg(2.5,4) s (rush miner rg(1,1.5)) | botlib.cpp:333, 346 |
| Tripmine detonate scan | rg(0.4,0.6) s, after abort rg(1,2) | botlib.cpp:377; tasks.cpp:1643 |
| Satchel detonate scan | rg(0.3,0.5) s, after abort rg(0.6,1.0) | botlib.cpp:445; tasks.cpp:1133 |
| Corner mine | rg(8,12) s (tripmine level) / rg(20,30) s | navigate.cpp:2458 |
| Satchel-jump retry | rg(6,12) s at start, rg(3,6) s after end/abort | combat.cpp:2802; tasks.cpp:983 |
| Longjump | 0.5 s retry, rg(0.9,1.4) s cooldown, 1.0 s flight window | navigate.cpp:729, 758-759 |
| Emotions | 0.5 s | botlib.cpp:957 |
| Darkness/flashlight | rg(2,4) s | vision.cpp:97 |
| Logo spray | first at spawn+rg(5,30), then rg(60,90) s | manager.cpp:1718; tasks.cpp:145 |
| Random crowbar swing | rg(2.5,6) s | tasks.cpp:63 |
| Respawn press | death + rg(0.5,1.5) s | manager.cpp:1687 |

---

## 2. Task system

### 2.1 Stack mechanics (`botlib.cpp:1437-1529`; filter table `manager.cpp:601-631`)

`BotTask {func, id, desire, data (node index or scratch), time (expiry), resume}`.

The filter table rows are per task id and **shared by all bots**: desire 0, data −1, time 0. resume=true only for Normal, MoveToPosition, FollowUser, PickupItem and Camp.

**`startTask(id, desire, data, time, resume)`:**
- It iterates the stack from the bottom.
- If a task with the same id exists anywhere, it only updates `desire` and returns. It is **not moved to the top**.
- For every non-matching element passed during the iteration it calls `clearSearchNodes()` (quirk, §14).
- Otherwise it pushes on top, calls `ignoreCollision()`, and calls `selectBestWeapon()` if the new top is Camp.
- It sets `m_chosenGoalIndex = top.data`.

**`getTask()`:** pushes `Normal(35, resume)` if the stack is empty.

**`completeTask()`:** pops the top, then keeps popping while the new top has `resume==false`. It then clears the path.

**`clearTask(id)`:**
- No-op if the stack is empty or the current task is Normal.
- If the current task is `id`, it pops it (no resume logic).
- Else it removes every `id` from the stack.

Tasks are pushed directly by event code at their `TaskPri`. `filterTasks` only chooses between Attack, SeekCover, Hide, Hunt, PickupItem, Blind and the current task.

### 2.2 Priorities (`constant.h:280-300`) and who starts each task

| Task | Pri / desire | resume | Started by |
|---|---|---|---|
| Normal | 35 | yes | getTask / newRound |
| Pause | 36 | no | nav: jump-sequence cooldown rg(0.75,1.25) s (navigate.cpp:1150); door wait 0.5 s (1293); ladder occupied 3 s (2612) |
| Camp | 37 | yes | advanceMovement (navigate.cpp:2519) |
| Spraypaint | 38 | no | checkSpawnConditions (botlib.cpp:1804); normal_ on goal reach (tasks.cpp:86) |
| FollowUser | 39 | yes | decideFollowUser (combat.cpp:2309) |
| MoveToPosition | 50 (camp approach) or **92** (knife chase, sendBotToOrigin) | yes | navigate.cpp:2520; botlib.cpp:983/987, 2329 |
| UseCharger | 50 | no | pickupItem_ (tasks.cpp:1745) |
| PlaceTripmine | 60 | no | camp_ (tasks.cpp:457), proactive (botlib.cpp:367), corner (navigate.cpp:2461) |
| PlaceTripmineMoving | 60 | no | proactive rush miner (botlib.cpp:342) |
| PickupItem | max(50, 500−dist·0.2), or 50 for a button | yes | filterTasks |
| Hunt | computed, ≤89 (≥70 on melee GG level; 90 for creatures) | no | filterTasks |
| Attack | 90 | no | filterTasks |
| SeekCover | computed (can exceed 100) | no | filterTasks |
| Hide | 92 | no | seekCover_ on arrival (tasks.cpp:228) |
| DetonateTripmine | 98 | no | checkTripmineDetonate |
| DetonateSatchel | 98 | no | checkSatchelDetonate |
| ThrowExplosive / ThrowSnark / ThrowSatchel / ThrowSatchelJump | 99 | no | checkGrenadesThrow (combat.cpp:2792-2812) |
| DoubleJump | 99 | **yes** | startDoubleJump (menu) |
| GaussJump | 99 | no | updateGaussCharge, checkGaussJumpTravel |
| Blind | 100 | no | filterTasks |
| ShootBreakable | 100 | no | checkBreakable (every tick in terrain check; on touch) |

### 2.3 Per-task specification

**Normal** (`tasks.cpp:17-118`)
- Aim flag Nav.
- If `debug_goal` is set, force that node as the goal; slow to 0.4·max within 172 u; stop within 22 u when visible.
- **Random crowbar swing:** `random_knife_attacks`, holding crowbar, `m_lastEnemy` not alive, no enemy, `knifeAttackTime<now`, no friend within 96. Press ATTACK; next swing in rg(2.5,6) s.
- If reloadState is None and there is reserve ammo, clip < 5 and the weapon uses ammo: reloadState = Primary.
- **On goal reached** (`updateNavigation()==true`): complete. Optionally start Spraypaint if all of: not seeing/suspecting, seeEnemy > 5 s ago, no reload state, spray timer expired, cvar on, standing on worldspawn, moveSpeed ≥ shiftSpeed, no pickup.
- **Else if no active goal:** goal = task.data if valid, else `findBestGoal()`, else the farthest node within 1024. Then `findPath(current, goal, Fast)`.
- **Else:** if `m_minSpeed` (260 at spawn) differs from maxspeed, use it.

**Pause** (`tasks.cpp:309-340`)
- Stop, Nav aim.
- If `viewDistance<500` and diff ≥ Normal ("go mad"): backpedal at `(500−viewDist)/2` (capped at max), Override look 500 u forward, `wantsToFire=true`.
- Else hold `m_campButtons`.
- Complete on timeout or any damage (`m_lastDamageType>0`).

**MoveToPosition** (`tasks.cpp:586-631`)
- Nav aim. Complete on reach (and clear `m_position`).
- Goal = task.data, else nearest(`m_position`). If occupied and `m_position` is set, use `findDefendNode(m_position)`.
- Path type is Fast. Task `time` is **ignored**.

**FollowUser** (`tasks.cpp:633-713`)
- Ends if the target is dead or null.
- If the target is firing and its (buggy, §14) aim trace hits an enemy, adopt that as `m_lastEnemy` and complete.
- Match the target's speed if slower. Reload if possible.
- Within 130 u: stop, and give up after 3 s of waiting.
- Goal: a free node within 200 u of the target. Path Fast.

**PickupItem** (`tasks.cpp:1703-1788`)
- `m_destOrigin = item origin` (straight-line move).
- Weapon, DroppedBox, AmmoAndKits, Items: Nav aim; done within 50 u (touch pickup). `item_longjump` sets `m_hasLongJump=true`.
- Charger: Entity aim; within 100 u start UseCharger (+15 s).
- Button: Entity aim; within 90 u (24 on lifts) stop, face within 10° yaw, then `MDLL_Use` and set `m_buttonPushTime=now+3`.

**UseCharger** (`tasks.cpp:1331-1369`)
- Face the charger and hold IN_USE.
- Ends when health/armor ≥ 99, on timeout, or on seeing an enemy. It blacklists the charger only when it timed out without being full and without an enemy.

**Camp** (`tasks.cpp:400-526`)
- Ends if `camping_allowed=0` or knife mode.
- Camp aim, stop, `idealReaction *= 0.5`, `timeCamping=now`.
- **Once per camp** (task.data marker −2), if it owns tripmines and has no enemy: chance 50 (Careful 70, Rusher 30; +20 in a Narrow node) to trace 128 u forward from the eyes. On a hit that is not near a known mine, start PlaceTripmine at the hit point.
- Every rg(1,4) s pick a look target:
  - on a Camp-flagged node, alternate `path->start`/`path->end` directions; if that trace fraction < 0.5, fall back;
  - fallback: the predicted node (if lastEnemy is alive and prediction is valid/visible), else `getRandomCampDir()`.
- Hold `campButtons`. Complete on timeout (camping_time_min..max + 4 s, set by advanceMovement) or any damage.
- Also `takeDamage` from an enemy clears Camp.

**Attack** (`tasks.cpp:284-307`)
- `moveToGoal=false`, `checkTerrain=false`, ignoreCollision.
- With an enemy: `attackMovement()` (§7). Knife sets `destOrigin=m_enemyOrigin`.
- Without an enemy: complete, `findNextBestNode`, `destOrigin=lastEnemyOrigin`.
- **Firing is not tied to Attack.** Any task that ends up with AimFlags::Enemy fires via `focusEnemy`/`doFireWeapons`.

**Hunt** (`tasks.cpp:161-211`)
- Nav aim.
- Cleared if a new enemy appears or lastEnemy is null, or if lastEnemy became a teammate.
- Complete on reach and clear `lastEnemyOrigin`.
- Goal = task.data, else nearest(`lastEnemyOrigin`), fixed once. Path Fast.

**SeekCover** (`tasks.cpp:213-282`)
- Nav aim. Complete if lastEnemy is dead.
- On reach: push Hide (92, rg(2,5) s); `lookAtSafe = getCampDirection(lastEnemyOrigin)`; `campButtons=0` (HL: stand); reload if clip < 5.
- Goal = data or `findCoverNode(1024)` (2048 when `m_infectedEnemyTeam`, never set). If there is no cover node, set `retreatTime=now+rg(1,2)` and complete.

**Hide** (`tasks.cpp:528-584`)
- Creatures complete immediately.
- Camp aim, stop, half reaction.
- If seeing an enemy and not on a Camp node, leave. If `lastEnemyOrigin` is empty, leave.
- Hold camp buttons, `checkReload()`. Complete on timeout or damage.

**Blind** (`tasks.cpp:342-398`)
- 50% per tick, if diff ≥ Normal, lastEnemy is a player and not sniper: look at `lastEnemyOrigin ± 272·dist/2048` (x,y) and set `wantsToFire=true`.
- diff ≥ Normal with a valid `m_blindNodeIndex`: navigate there, then set Suspect.
- Otherwise use the blind move/strafe speeds and buttons chosen in `takeBlind` (`botlib.cpp:2191-2236`):
  - diff ≤ Normal: duck in place;
  - diff > Normal: cover node within 900 u; backpedal; random ± strafe; health < 85 backpedal, Careful ducks still, otherwise forward.
- Complete when `blindTime<now`.
- Triggered only by a full-white ScreenFade with alpha > 180; duration `(alpha−180)/16` s. Rare in HL.

**Spraypaint** (`tasks.cpp:120-159`)
- Entity aim at eyes+forward·128 (or 128 lower if there is no wall).
- At `task.time−0.5`: play the sprayer sound and decal trace; next spray in rg(60,90) s. Always stationary.

**ShootBreakable** (`tasks.cpp:1579-1638`)
- Ends if there is an enemy or the breakable is gone.
- If the origin→breakable trace hits something else, blacklist that entity and complete.
- Override aim at the breakable, hold duck state.
- When facing (cone ≥ 0.90): stop, `wantsToFire`, `shootTime=now`, edge-press ATTACK if not knife, not reloading and clip > 0.
- Knife with no clip ammo and > 32 u away: complete.
- Not facing: keep moving.
- Detection: `lookupBreakable()` traces toward `m_destOrigin` from origin and eyes over 72–256 u (32 for knife/ladder). Breakables are `func_breakable`/`func_pushable`(breakable)/`func_wall` with 1 ≤ health < 500 (`engine.cpp:1325`).

**DoubleJump** (`tasks.cpp:1371-1441`)
- Human-requested via menu: go to the requester, duck, jump when they jump.
- Aborts on seeing an enemy or after travel time + 11 s. Team-says "Ok %s, i will help you!".

The remaining tasks (throws, tripmines, satchels, gauss jump) are specified in §8, §10 and §11.

---

## 3. Desire computation: `filterTasks()` exact (`botlib.cpp:1226-1435`)

```
tempFear = fear; tempAggr = aggression
if lastEnemyOrigin set:
    friendly = friendsNear(self, 500) - enemiesNear(lastEnemyOrigin, 500)
    if friendly > 0: tempFear *= 0.5
if usesSniper: tempFear *= 1.5; tempAggr *= 0.5

PickupItem.desire = pickupItem ? (button ? 50 : max(50, 500 - dist*0.2)) : 0

Attack.desire = (!gungameTripmineLevel && !usesThrowable && SeeingEnemy && reactOnEnemy()) ? 90 : 0

if lastEnemy is player && lastEnemyOrigin set:
    retreatLevel = (max_health - health) * tempFear
    if creature || (enemiesLeft > friendsLeft/2 && retreatTime < now && seeEnemyTime - rg(2,4) < now /*always true*/):
        timeSeen = seeEnemyTime - now; timeHeard = heardSoundTime - now
        ratio = (timeSeen > timeHeard) ? (timeSeen+10)*0.1 : (timeHeard+10)*0.1   // 1.0 if just now, 0 at 10 s
        retreatTime = now + rg(1,4)
        if creature: ratio = 0
        if stuck || knife:                          ratio /= 3
        elif reloading || (sniperStop>now && clip<18% && sniper): ratio *= 3
        else:                                       ratio = 0      // "cover doesn't pay in DM"
        SeekCover = retreatLevel * ratio
    else SeekCover = 0            // non-zero only on the sampling tick

    melee = gungameMeleeLevel
    if !enemy && !tripmineLevel && (melee || now > roundMid) && !usingGrenade
       && currentNode != nearest(lastEnemyOrigin) && (melee || personality != Careful) && !ignore_enemies:
        d = (4096 - (1 - tempAggr) * dist(lastEnemyOrigin)) * 100/4096 - retreatLevel
        d = min(d, 89); if melee: d = max(d, 70)
        Hunt = d
    else Hunt = 0
else Hunt = SeekCover = 0

if creature && Hunt > 16: Hunt = 90; SeekCover = 0
Blind = blindTime > now ? 100 : 0
```

**Combination step:**
- `maxDesire(a,b)` returns a if `a>b`, else b (ties go to b).
- `subsume(a,b)` returns a if `a>0`, else b.
- `threshold(t,th,d)` sets `t.desire=d` if `< th`.
- Hysteresis: `oldCombat = (cur ≤ 40 || cur ≥ 90) ? cur : oldCombat`; `Attack = oldCombat`. **This is a no-op** because Attack is always 0 or 90.

```
survive  = subsume(Hide /*never set → 0*/, threshold(SeekCover, 40, 0))
def      = threshold(Hunt, 60, 0)
offense  = subsume(Attack, PickupItem)      // attacking suppresses pickup
sub      = maxDesire(offense, def)
final    = subsume(Blind, maxDesire(survive, sub))
final    = maxDesire(final, currentTask)    // current task wins ties = the only inertia
startTask(final.id, final.desire, final.data, final.time, final.resume)
```

Consequences:
- Any pickup (≥50, typically about 410) beats Hunt (≤89) except buttons.
- Attack (90) loses to throw/detonate/jump tasks (98–100) and to the knife-chase MoveToPosition (92).
- Other inertia sources: task desire stored in the stack, the 1–4 s retreat sampling, the 1 s reachability cache, the 0.85 s rescan, and the style/strafe timers.

**`reactOnEnemy()`** (`botlib.cpp:1564-1612`) requires `isEnemyThreat()`:
- `isEnemyThreat` is false if there is no enemy, **SuspectEnemy is set**, or the task is SeekCover or Camp.
- Otherwise it is true if the enemy is within 256 u, or (not knife and enemy inside the bot's view cone).
- Creature: reachable only within 128 u 2D.
- Otherwise every 1 s: reachable if `isEnemyNoticeable(lineDist)`, else if `pathDist − lineDist ≤ 112` and not on a ladder.
- If reachable, set `navTimeset=now`.

**`isEnemyNoticeable`** (`combat.cpp:2830-2917`, port of regamedll):
- If on a ladder, false.
- cover = Body 40 + Head 10 + Other rg(10,25).
- rangeMod = clamp((range−300)/700, 0, 1).
- Enemy speed > 200 → true immediately.
- Walking (> 30 u/s): crouched close/far 90/60, standing 100/75. Still: crouched 80/5, standing 100/10.
- `p = lerp(close,far,rangeMod)*cover/100 + 0.5 + 12.5·diff (+50 if aggr>fear)`.
- `p = max(0.1, p·|aggr−fear|)`; return `rand(0,100) < p`.

---

## 4. Personalities and difficulty

### 4.1 Personality (`manager.cpp:236-258, 1267-1306`)
- Random pick: 50% Normal, 25% Rusher, 25% Careful, unless `yb_preferred_personality` is set. The `bots.json` roster can set personality, difficulty, and aggression/fear overrides.

| | Normal | Rusher | Careful |
|---|---|---|---|
| base aggression | rg(0.4,0.7) | rg(0.7,1.0) | rg(0.2,0.5) |
| base fear | rg(0.4,0.7) | rg(0.0,0.4) | rg(0.7,1.0) |
| Weapon pref (pickup rating; row indices, ascending desirability; config.h:58-60) | 0,7,6,4,5,1,2,3,8,9,10,11,12,13 | 0,7,6,4,5,3,1,2,8,11,12,10,9,13 | 0,4,7,6,5,1,2,3,9,10,13,12,11,8 |
| Goal choice (navigate.cpp:95) | practice-sorted | **random of 4** | practice-sorted |
| Camp on dangerous path (navigate.cpp:2487) | kills/3 | never | kills/2 |
| Hunt allowed | yes | yes | **no** (except GG melee level) |
| Camp-tripmine chance (+20 narrow) | 50 | 30 | 70 |
| Rush-miner roll per life (GG tripmine level) | 50 | 80 | 25 |
| Satchel-jump roll per life | 40 | 60 | 20 |
| Blind at ≥85 HP (diff > Normal) | charge forward | charge forward | duck in place |

History: `70b3698` raised the rush roll from 35/65/15 (Normal/Rusher/Careful) to the current 50/80/25, and the satchel-jump roll from 25/45/10 to 40/60/20.

**Emotions** (`botlib.cpp:930-958`, every 0.5 s):
- Seen an enemy within 1 s: aggression +0.05 (cap 1).
- Seen > 5 s ago: aggression and fear step 0.05 toward base.
- Kill: aggression +0.1 (cap 1).
- Enemy damage: HP > 60 gives aggression +0.1, otherwise fear +0.03. There is a **bug**: when the value exceeds 1 it adds +1.0 more (§14).

### 4.2 Difficulty data (`config.cpp:594-612` defaults, overridden by the shipped `difficulty.cfg`)

| Level | Reaction min–max (cfg) | Headshot % | SeenThru % | HearThru % | maxRecoil | aimError x,y,z | Code defaults if cfg missing |
|---|---|---|---|---|---|---|---|
| 0 Noob | 1.5–2.0 | 15 | 0 | 0 | 40 | 20,20,40 | 0.8–1.0, 5, 0, 0, 38, (30,30,40) |
| 1 Easy | 1.0–1.5 | 20 | 0 | 0 | 35 | 15,15,30 | 0.6–0.8, 30, 10, 10, 32, (15,15,24) |
| 2 Normal | 0.5–1.0 | 25 | 0 | 25 | 30 | 10,10,20 | 0.4–0.6, 50, 30, 40, 26, (5,5,10) |
| 3 Hard | 0.25–0.5 | 50 | 50 | 50 | 25 | 5,5,10 | 0.2–0.4, 75, 60, 70, 23, 0 |
| 4 Expert | 0.1–0.25 | 75 | 75 | 75 | 20 | 0,0,0 | 0.1–0.2, 100, 90, 90, 21, 0 |

- SeenThru and HearThru are **dead** in HL (no penetration, §5.7).
- aimError is applied only for diff < Normal (§6.2).
- Creation difficulty: request, then roster, then `yb_difficulty` (default 3). An invalid cvar gives rg(3,4). `difficulty_min/max` randomize. Auto-balance by KPD is optional.

### 4.3 Hard-coded difficulty-dependent constants

- **Grenade check chance per heavy tick:** `max(50, 25·diff)`% → 50/50/50/75/100.
- **Semi-auto fire delay at range** (`combat.cpp:1212-1217`):
  - `idx = |diff·25/20 − 5|` → 5,4,3,2,0;
  - delay tables {0,.1,.2,.3,.4,.6} to {.1,.2,.3,.4,.5,.7};
  - `shootTime = now − frameInterval + 0.1 + rg(min,max)`.
  - Result: Noob 0.7–0.8, Easy 0.5–0.6, Normal 0.4–0.5, Hard 0.3–0.4, Expert 0.1–0.2 s.
- **Recoil pause tolerance** `(100−25·diff)/99` → 1.01/0.76/0.51/0.25/0.
- **Aim error divisor** `clamp(diff,1,4)·1280`. **Head-aim drop distance:** 2000 u for diff ≥ Normal, else 1000 u.
- **Expert only:** body aim +0.35·view_ofs.z; `getCustomHeight` table (§6.2); snap aim with `whose_your_daddy`.
- **diff < Hard:** "crosshair-stick" (keeps aiming at the current hit point).
- **Noob:** spring/damper "newbie" aim model.
- **Hard/Expert:** stiffer aim when engaged.
- **Fight::Stay chance** (`combat.cpp:1831-1832`):
  - 768–1024 u: {60, 40, 20, 8, 0}%;
  - > 1024 u: {85, 70, 45, 20, 10}%;
  - < 768 u: always Strafe.
- **Dodge-duck ("crouch tap")** {0, 0, 4, 6, 8}% per style roll (every 1–3 s) while strafing on the floor, not knife, jumpTime+0.9 < now. Duck for rg(0.25,0.5) s (`combat.cpp:1859-1865`).
- **Dodge-hop:** diff ≥ Easy, cooldown `6.5 − 1.25·diff` s → Easy 5.25, Normal 4.0, Hard 2.75, Expert 1.5. 30% per think tick once allowed (§7).
- **diff ≥ Normal:**
  - circle-strafe drift and cornered backpedal;
  - narrow-place strafing allowed (diff < Normal forces Stay in narrow nodes);
  - weapon "bad distance" switching;
  - crowbar stab of close enemies;
  - knife aims at the visible point;
  - blind firing; gauss jump; satchel jump;
  - look at the last victim.
- **Sniper stand-still** before a long shot: `2 − 0.35·diff` s.
- **Gauss roam precharge:** diff ≥ Easy, chance 40+10·diff.
- **Kill-cam notification** (teammate killed): Hard+ only.
- **Camping time:** `rg(camping_time_min=5, camping_time_max=15) + 4` s.

---

## 5. Enemy detection and knowledge

### 5.1 Vision primitives
- **View cone** (`support.h:80-87`): `dot(v_angle.forward, normalize(pos−eyes)) ≥ cos(fov/2)`, with fov = `pev->fov`, or 90 when 0 (HL default). A zoomed crossbow gives fov 20, so a ±10° cone.
- **Frustum** (`vision.h`, `vision.cpp:294-354`): vertical FOV 75°, aspect 16:9 (≈107° horizontal), near 2, far 4096. The object test is a 60×16 cylinder at origin−5z. Recomputed in `updateBodyAngles`.
- **`isInFOV`** is the yaw-only angle difference (used by placement and item code).
- **`seesEnemy(p)`** (`combat.cpp:337`) = (cone, or ignored when `whose_your_daddy` and the bot is being hurt by an enemy) && frustum && `checkBodyParts`.
- **`checkBodyParts`** first rejects hidden / invincible / notarget / dark targets:
  - render check needs cvar (off);
  - invincible check needs cvar (off);
  - `FL_NOTARGET`;
  - dark area (effectively dead, §14).
- **Offset mode** (default; `combat.cpp:191-268`). Traces from the eyes with **TraceIgnore::Everything, which ignores glass and monsters**; creatures use None. "Hit" means fraction ≥ 1 or pHit==target. Points:
  1. origin → Body;
  2. origin+25z → Head (both 1 and 2 are always tested; return if any);
  3. feet: origin−34 (standing) or −14 (ducking) → Other;
  4. origin ± 13 u perpendicular (2D) → Other.
- `m_enemyParts` gets the bits and `m_enemyOrigin` the endpoint of the last successful trace.
- Hitbox mode (`use_hitbox_enemy_targeting`, off) uses stomach, head, arms and feet hitboxes.
- `m_viewDistance` is 4096; blinding shrinks it (see §14 on its inverted restore).

### 5.2 `lookupEnemies()` (`combat.cpp:367-620`)
1. Return false if the ignore timer is active, the bot is blind, or `ignore_enemies`.
2. Suspect bookkeeping:
   - If an enemy is held and SeeingEnemy, clear Suspect.
   - Else if no enemy, seen < 4 s ago and lastEnemy alive: set **Suspect**, and add AimFlags::LastEnemy if there is LOS from the **body origin** to `lastEnemyOrigin`, unless "denyLastEnemy" (moving, within 256 u, shot within 1.5 s).
3. **Keep the current enemy** if `m_enemyUpdateTime > now`, within view distance, alive, and `seesEnemy`.
4. Otherwise do a **full scan**:
   - monsters first (if `attack_monsters`), scored `dist² / (entArea/botArea)` so larger monsters look closer;
   - then enemy clients (`team != mine`), scored `dist²`, **×`gungame_leader_priority` (0.25) if the target is the GunGame leader**;
   - nearest score wins (max = viewDistance²);
   - `whose_your_daddy`: an attacking player expands view to max;
   - `enemyUpdateTime = now + (knife ? 1.25 : 0.85)`.
5. **Found, same as current:** `seeEnemyTime=now`, `actualReaction=0`, `lastEnemy/lastEnemyOrigin` updated.
6. **Found, new enemy:**
   - `m_targetEntity=null` (stop following);
   - `enemySurpriseTime = now + actualReactionTime` (×0.5 with `whose_your_daddy`), then `actualReaction=0`;
   - set enemy, lastEnemy, lastEnemyOrigin; `enemyReachableTimer=0`;
   - **if the bot was holding ATTACK last frame: teammate alarm.** Alive teammate bots that have not seen an enemy for 2 s, have no lastEnemy, have LOS to this bot and have it in their view cone get `lastEnemy`, the exact enemy origin, `seeEnemyTime=now`, Suspect|Hearing states and AimFlags::LastEnemy.
7. **Not found but still holding an enemy:**
   - If it died and was seen < 0.1 s ago (not sniper): `shootAtDeadTime = now + clamp(aggr·1.25, 0.15, 0.25)`, Suspect, return true (keeps firing).
   - Else the shoot-through-walls check (dead in HL).
8. Reload trigger when idle: `m_aimFlags ≤ PredictPath` (numeric compare), seen > 3 s ago, no enemies, not ShootBreakable → reloadState = Primary.
9. Crossbow: if zoomed with no target and `zoomCheckTime+1 < now`, press ATTACK2 to unzoom.

`trackEnemies()` sets or clears SeeingEnemy. On failure it nulls `m_enemy` and `m_enemyBodyPartSet`.

### 5.3 Reaction time
- `m_idealReactionTime = rg(reaction[0], reaction[1])`, re-rolled every think tick.
- `m_actualReactionTime` ramps +0.3 per think tick up to ideal. So it is ≈ ideal unless an enemy was acquired within the last few ticks.
- On a new enemy, surprise = actual. During surprise, `focusEnemy` sets the look target but **does not set `wantsToFire`**.
- Camp and Hide halve the ideal reaction.

### 5.4 Memory
- `m_enemy`, `m_lastEnemy`, `m_lastEnemyOrigin`, `m_seeEnemyTime`, `m_heardSoundTime`.
- States: SeeingEnemy, HearingEnemy (sticky, §14) and SuspectEnemy.
  - Suspect is set on losing an enemy within 4 s, by the teammate alarm, by Blind, and by shoot-at-dead.
  - Suspect is only cleared at the start of a lookup that still holds a seen enemy. While set, it **blocks `isEnemyThreat`**, so there is no Attack for one heavy tick on re-acquisition.
- `getEnemyBodyOffset` overwrites `m_lastEnemyOrigin` with the current aim spot every tick (`combat.cpp:744`).
- `lastEnemy` is cleared once dead and shoot-at-dead has expired.
- **Prediction** (`botlib.cpp:1039-1126`):
  - With no enemy and a lastEnemy within 2048 u: PredictPath aim; or LastEnemy aim if there is LOS from the body and no deny condition.
  - A worker job walks the planner path from the node nearest `lastEnemyOrigin` toward the bot's node and picks the first node visible from the bot's node (vistable) that is 128–2048 u from the bot. It stores `m_lastPredictIndex` and the path length.

### 5.5 Hearing (`botlib.cpp:2430-2552`; noise model `sounds.cpp`)

**Noise registry per client:**
- `simulateNoise` each server frame, from input and movement:
  - IN_ATTACK (button|oldbuttons): 2048 u for 0.3 s;
  - IN_USE: 512 u / 0.5 s; IN_RELOAD: 512 u / 0.5 s;
  - ladder with |vz| > 50: 1024 u / 0.3 s;
  - else with `mp_footsteps`: `1280·speed2d/260` for 0.3 s.
  - **IN_ATTACK2 makes no noise.** A louder sound overrides an active one.
- EmitSound hook (`acquire`), attributed to the nearest alive client:
  - pain/fall 768 u / 0.52 s;
  - weapon/item/charger pickup 768 / 0.45;
  - ammo 512 / 0.25;
  - breakage 1024 / 2;
  - door 1024 / 3;
  - dist scaled by volume.

**`updateHearing`:**
1. Select the nearest (by noise position) alive enemy with an active noise, not invincible/notarget, inside the bot's **PAS**, with the noise position within noise.dist.
2. If heard:
   - `selectBestWeapon()` if not shot for 5 s, on the floor, not holding a throwable, not knife mode;
   - `heardSoundTime=now`; set HearingEnemy;
   - the heard origin is **the enemy's true current origin**. Beyond 384 u it is exact. Within 384 u it gets ±(272·d/2048) x/y jitter.
3. LastEnemy assignment:
   - adopt if there is no lastEnemy;
   - if it is the same enemy, update the origin (unless seeing);
   - if it is a different enemy, switch only if it is nearer and not seen for 1 s.
4. **If `checkBodyPartsWithOffsets(heard)` succeeds (LOS only, no cone/frustum/dark check):** `m_enemy = heard`, SeeingEnemy, `seeEnemyTime=now`. There is no surprise delay (360° acquisition by sound).
5. Else shoot-through (dead).

### 5.6 Other knowledge sources
- **`takeDamage`** (`botlib.cpp:2117-2189`):
  - From a hitscan attacker (dmg_inflictor is a player; projectiles and explosives have non-player inflictors): if the bot has no enemy, set `lastEnemy`, the attacker's exact origin, and `seeEnemyTime=now` (code comment "FIXME - Bot doesn't necessary sees this enemy").
  - Records practice danger. Clears Camp.
  - From a human teammate with tkpunish: attacks him immediately and sends TeamAttack chat.
- **`handleDeath`** (`manager.cpp:1427-1458`): Hard+ bots on the victim's team with no enemy and no lastEnemy, that have LOS (no FOV check) to the killer, get `m_enemy = killer` directly with zero reaction time.

### 5.7 Shooting through walls
`isPenetrableObstacle` returns false (`penetratePower=0`, `combat.cpp:831-836`). The following are therefore all dead:
- `shoots_thru_walls`, `seenThruPct`, `hearThruPct`;
- `lastEnemyShootable` (so the bot never fires at the last-enemy aim);
- the door-penetration enemy (`navigate.cpp:1306`).

### 5.8 Cheating knowledge (information a human would not have)
1. **Hearing** uses input-derived noise (attack button, footsteps velocity). It returns the target's **true current origin** (exact beyond 384 u). A heard enemy with LOS is acquired regardless of facing (§5.5).
2. **Grenade decisions** read `m_lastEnemy->v.origin / velocity / flags / waterlevel` even when the enemy is unseen: HE lead point = origin + velocity (1 s), height and airborne gates, snark height gate (`combat.cpp:2615-2622, 2688-2691, 2770`).
3. **`takeDamage`** gives the exact attacker position (hitscan only).
4. **`handleDeath`** gives Hard+ teammates the killer as a direct enemy (LOS only).
5. **Teammate alarm** passes exact enemy positions.
6. **Global alive counts** (`m_numEnemiesLeft/FriendsLeft`) used in seek-cover gating and Nav danger-look gating.
7. **`numEnemiesNear` with omniscient positions:**
   - predicted-node aim only if a real enemy is within 1024 u of the node (`vision.cpp:484-485`);
   - satchel trap trigger: nearest enemy to own charge (`tasks.cpp:885-894`);
   - tripmine detonation: enemy within 140 u / abort at 160 u of a mine;
   - fear halving (enemies near `lastEnemyOrigin`).
8. **Global tripmine registry** (`manager.cpp:888-896`): every live mine including enemy mines, refreshed each second. Used for spacing and as detonation targets (LOS trace still required).
9. **Own satchel liveness** via entity ownership; remote detonation needs no LOS.
10. **HE velocity override:** the thrown grenade's velocity is rewritten to the computed ideal (`combat.cpp:2530-2551`).
11. **Jump links:** `pev->velocity` is set directly (navigation, `navigate.cpp:1111-1139`).
12. **Sees through glass and other players/monsters** (trace ignores them), unless `aim_trace_consider_glass`.
13. **360° awareness of airborne enemy grenades and satchels** with LOS (FOV check degenerate, §14).
14. **Enemy view direction** (`m_enemy->v.v_angle`) used for strafe side and "enemy faces me" fire gating.
15. **`whose_your_daddy`:** FOV ignored when hurt; full view range vs attackers; snap aim.
16. **`followUser`** uses the leader's aim trace; practice/danger data across lives and maps.

---

## 6. Aim

### 6.1 `setAimDirection()` (`vision.cpp:356-634`)

**Pre-processing:**
- If none of Grenade/Enemy/Entity is set:
  - strip LastEnemy|PredictPath when ducked in a narrow place, on a ladder, in water, on a ladder node, or on a jump link (also `canSetAimDirection=false`);
  - "don't switch view right away": add LastEnemy if `(shootTime + rg(0.75,1.25) > now || seeEnemyTime + rg(1,1.25) > now)`, the forget-victim timer has elapsed, lastEnemy is a player and the current enemy is not a player.
- **Throwable exception (commit `0de6bcc`):** if holding a throwable (HE, satchel, snark, tripmine) and the task is not ThrowSatchelJump, strip Enemy|LastEnemy|PredictPath. The same commit made `filterTasks` give Attack desire 0 while holding a throwable.

**Priority chain** (first match sets `m_lookAt`):
1. **Override:** `m_lookAtSafe`.
2. **Grenade:** `m_throw`. With d = distance: 100 < d < 800 gives corr = 0.25·(throw.z − origin.z); d ≥ 800 gives corr = d·tan(min(37·(d−800)/800, 45°)) + 0.25·dz. Then `lookAt.z += corr·0.5`.
3. **Enemy:** `focusEnemy()`.
4. **Entity:** `m_entity` (+72 z for Weapon pickups).
5. **LastEnemy:** `lastEnemyOrigin`. The fire-if-shootable condition exists but is dead in HL.
6. **PredictPath:** predicted node origin when applicable:
   - node exists; eyes→node trace fraction ≥ 0.5; 256 < dist < 2048;
   - vistable-visible from the current and previous node; ≠ current;
   - path length < `max_nodes_for_predict` (22); an enemy really within 1024 u.
   - Re-validated with a 0.75 s tracking window; failure clears the flag after a 0.5 s grace.
7. **Camp:** `m_lookAtSafe`.
8. **Nav:** dest + view_ofs, with refinements:
   - if enemies are alive, not seen for 4 s and not predicting: look at the practice "danger" node (visible both ways, not crouch, > 240 u) and add the Danger flag;
   - else when safe, look 1–2 path nodes ahead;
   - look up ladders;
   - look back at the previous node when facing a wall (80 u trace);
   - keep eye height;
   - diff ≥ Normal: look at the last victim for 1–2 s if the kill was > 384 u away.
- Fallback: empty lookAt becomes `m_destOrigin`.

### 6.2 Aim point: `getEnemyBodyOffset()` (`combat.cpp:644-751`)
- If there are no visibility parts, return `m_enemyOrigin`.
- `distance = |enemy.origin − bot.origin|`.
- Drop the Head flag when Body is visible and distance > (2000 for diff ≥ Normal, else 1000) for non-sniper, or distance < 800 with a sniper.
- **Lead:** non-sniper, non-knife, distance > 272: `(enemy.vel − bot.vel)·frameInterval·2.8`, z=0. At 90 Hz that is about 0.031 s of relative velocity. This is the only lead; there is no projectile-speed lead.
- **Head** = (x, y, absmin.z + 0.81·size.z) + `getCustomHeight`.
- **Players with Head|Body visible:**
  - `headshotPct` from the table; forced to 0 when (distance > 272 and (recoil high or shotgun)) or (≤ 272 and recoil high);
  - aim at the head if already head-locked on this enemy or `chance(pct)` (rolled **every tick**), then lock (`m_enemyBodyPartSet`);
  - sniper head z −= 0.35·view_ofs.z;
  - otherwise origin (Expert +0.35·view_ofs.z).
- Body only: origin. Other only: the trace point. Head only: head.
- **diff < Hard:** if the crosshair ray (model trace along v_angle) already hits the enemy, spot = that hit point + 0.5% toward the desired spot.
- Add the lead. Knife with diff ≥ Normal: `m_enemyOrigin`.
- **RPG:** enemy on the ground aims at origin −30 z (feet, splash).
- `m_lastEnemyOrigin = spot`.
- **diff < Normal:** add `getBodyOffsetError(distance)`. This applies only beyond 544 u, refreshed every 0.4–0.8 s:
  - bbox-scaled error with `hitError = dist/(clamp(diff,1,4)·1280)` (z halved);
  - plus uniform ±aimError from the table.
- **`getCustomHeight`** (Expert only, enemy not ducking), z-offset by weapon type for [Long 544–2048 | Middle 272–544 | Short otherwise]:
  - pistol {0.5, −0.1, −1.5}; shotgun {6.5, 6, −2}; SMG {0.5, −7.5, −9.5}; sniper {0, −2.5, −6}; heavy {1.5, −4, −9}; others 0.

### 6.3 Turning: `updateLookAngles()` (`vision.cpp:113-209`)
- `delta = clamp(now − last, ε, 1/25)`. Target direction = angles(lookAt − eyes), pitch inverted.
- **Noob** without Grenade aim uses the newbie model (below). Commit `70b3698` excluded throw arcs from it.
- **Expert** with Enemy flag, (wantsToFire or sniper) and `whose_your_daddy`: snap.
- **Otherwise a spring model:** accel clamp 3000, stiffness 200, damping 25.
  - If (Enemy|Grenade flags or wantsToFire) and (diff > Normal **or grenade aim**): stiffness 300, damping 20, Expert accel 3300. Commit `70b3698` added the grenade case at any difficulty.
  - Yaw: if |diff| < 1°, snap and zero velocity; else `vel += dt·clamp(k·diff − c·vel, ±A)`, `angle += dt·vel`.
  - Pitch: same with **2k** and no snap. Pitch clamped to ±89°.
  - Reverse-facing guard while navigating.
  - Body angles: `angles.x = −v_angle.x/3`; the frustum is recomputed.
- **Newbie model** (`vision.cpp:211-292`, offset = 25):
  - spring 13, damper 0.22; influence (0.25, 0.17)·0.75; randomization (2.0, 0.18)·0.75;
  - targeting (Enemy|Entity): stiffness = 13·0.4, no randomization;
  - otherwise: randomized ideal angles every rg(0.4, 1.2) s (smaller when standing); stiffness multiplier 0.3 or recent-fire-based, ×deviation·0.05, minimum 0.35;
  - `aimSpeed = k·dev − 0.22·aimSpeed`, cross-axis coupling, `v_angle += dt·aimSpeed`.

### 6.4 Fire decision: `focusEnemy()` (`combat.cpp:1714-1775`)
- `lookAt = body offset`. During surprise, return without firing.
- `dot` = own cone toward `m_enemyOrigin`; `enemyDot` = enemy's cone toward the bot.
- **< 128 u 2D (not sniper):** knife fires if < 48 u (not > 72); others fire if dot > 0.80.
- **Else:** no fire if dot < 0.90; knife fires; else fire if the enemy faces the bot (enemyDot ≥ 0.90) or dot > 0.99. Always fire under 90 u.

---

## 7. Combat movement (`attackMovement`, `combat.cpp:1777-2003`; Attack task only; rpm angles = view angles)

1. Knife or creature: `destOrigin = enemy origin`.
2. **approach:**
   - knife or creature: 100;
   - Suspect and not seeing: 49;
   - reloading: 29;
   - else `health × aggression` (sniper capped at 49).
3. **Style re-roll** every rg(1,3) s:
   - Sniper: Stay if in the shoot window (`shootTime−0.4 ≤ now < shootTime+0.1`, `sniperStopTime > now`) and dot > 0.9; if the window fails, fall through to the tables.
   - Knife: Strafe.
   - Else by 3D distance to the aim point: < 768 Strafe; 768–1024 Stay with `kStayChanceMid[diff]`; > 1024 Stay with `kStayChanceLong[diff]`.
   - Force Stay if ducking, (Narrow node and diff < Normal), or no head/body visible.
   - Force Strafe if approach < 30, friend in the line of fire (previous frame), or (pistol or shotgun) within 1632 u with the enemy in the cone.
   - Dodge-duck roll (§4.3).
4. Within 96 u (not knife): backpedal at full speed.
5. Knife/creature with the enemy in the cone: Strafe; beyond 100 u style **None**, which leaves moveSpeed at maxspeed: charge straight forward.
6. **Strafe:**
   - Every rg(0.3,0.8) s choose a side: Right if the bot is on the enemy's left side (`dot(bot−enemy, enemy.right) < 0`), else Left; 30% swap.
   - Walls are probed 134 u left and right (from `pev->angles`). If blocked, swap; if both are blocked, strafe 0.
   - Forward drift (not knife):
     - diff ≥ Normal, approach ≥ 60 and dist > 400: +0.5·max (closing circle-strafe);
     - diff ≥ Normal, approach < 60 and dist < 300: −0.5·max;
     - else if approach ≥ 30: 0;
     - **else keep +max** (quirk: low-health or timid bots run at the enemy).
   - Cornered (both walls): strafe 0; move −max for diff ≥ Normal, else 0; `strafeSetTime+1`; dodge None.
   - **Dodge-hop** (`combat.cpp:1965-1974`): diff ≥ Easy, dist < 1088, `jumpTime + 6.5 − 1.25·diff < now`, on floor, `rg(0,100) < 30` per tick, speed2d > 150, not sniper or launcher, enemy in the cone → IN_JUMP. `translateInput` adds duck in the air for 0.85 s after a jump (duck-jump).
7. **Stay:** move and strafe 0. The old "crouch while standing" behavior was removed in `4edb7f8`.
8. Reloading: move −max and cancel duck.
9. **Ledge guard:** if not in water or on a ladder and moving, predict spot = origin + fwd·move·0.2 + right·strafe·0.2 + vel·dt. If `isNotSafeToMove` (startsolid or drop > 160 u), negate both speeds and clear jump.
10. `ignoreCollision()`.

**Other combat movement:**
- **Knife chase** (`overrideConditions`, `botlib.cpp:960-998`): in knife mode vs a player or monster, if 2D distance > (node-to-enemy distance + 48 u)² and seeing, push MoveToPosition(**92**) to the enemy's nearest node. Within reach, clear it (except creatures).
- **Sniper stop:** `overrideConditions` zeroes speed during the shoot window when facing (dot > 0.95). `handleWeapons` stops and delays fire while moving for shots > 600 u.
- **Grenade avoidance** (§1.4 step 20).
- **Retreat:** SeekCover or Hide via desires (effectively only while reloading, stuck, or with a knife; §3). Camping via navigation (§8.7). Chasing is Hunt (§3) or Attack's closing drift.
- **Attack longjump** (§11).

---

## 8. Weapons

### 8.1 Weapon table (`config.cpp:900-929`)

Row order is ascending desirability; "last valid row wins" everywhere.

| Row | Weapon (id) | Classname (alias) | Ground model | maxClip | Type | Hold fire | Underwater | min–max u | Secondary | Secondary range |
|---|---|---|---|---|---|---|---|---|---|---|
| 0 | Crowbar (1) | weapon_crowbar | crowbar.mdl | −1 | Melee | ✓ | ✓ | 0–64 | – | – |
| 1 | Glock (2) | weapon_9mmhandgun (weapon_glock) | 9mmhandgun.mdl | 17 | Pistol | ✗ | ✓ | 0–1500 | Rapid | 32–300 |
| 2 | Hornet (11) | weapon_hornetgun | hgun.mdl | −1 | Pistol | ✓ | ✓ | 0–1500 | Rapid (≥ 4 hornets) | 32–250 |
| 3 | Python (3) | weapon_357 (weapon_python) | 357.mdl | 6 | Pistol | ✗ | ✗ | 0–4000 | – | – |
| 4 | Snark (15) | weapon_snark | sqknest.mdl | −1 | Throwable | ✗ | ✗ | (128–800) | – | – |
| 5 | HandGrenade (12) | weapon_handgrenade | grenade.mdl | −1 | Throwable | ✓ | ✓ | (300–800) | – | – |
| 6 | Satchel (14) | weapon_satchel | satchel.mdl | −1 | Throwable | ✗ | ✓ | (200–600) | Detonate (marker only) | – |
| 7 | Tripmine (13) | weapon_tripmine | tripmine.mdl | −1 | Throwable | ✗ | ✓ | (0–128) | – | – |
| 8 | Crossbow (6) | weapon_crossbow | crossbow.mdl | 5 | Sniper | ✗ | ✓ | 400–4000 | Zoom | 700–4000 |
| 9 | Shotgun (7) | weapon_shotgun | shotgun.mdl | 8 | Shotgun | ✗ | ✗ | 0–750 | Double | 32–300 |
| 10 | MP5 (4) | weapon_9mmAR (weapon_mp5) | 9mmAR.mdl | 50 | SMG | ✓ | ✗ | 0–2000 | AltAmmo (M203) | 300–700 |
| 11 | RPG (8) | weapon_rpg | rpg.mdl | 1 | Launcher | ✗ | ✓ | 300–5000 | – | – |
| 12 | Gauss (9) | weapon_gauss | gauss.mdl | −1 | Heavy | ✓ | ✗ | 0–3000 | Charge | 500–3000 |
| 13 | Egon (10) | weapon_egon | egon.mdl | −1 | Heavy | ✓ | ✗ | 128–2000 | – | – |

- `minPrimaryAmmo` is 1 for every row except the crowbar (0).
- `optimalDistance` (40, 300, 300, 750, 300, 450, 400, 64, 1000, 400, 600, 700, 500, 350) is **unused**.
- The min/max ranges of throwables are unused; the throw windows are hard-coded (§8.6).
- Ammo indices and maxima come at runtime from the WeaponList message (`message.cpp:10-32`); clip and current weapon from CurWeapon.
- Masks (`constant.h:334-350`):
  - primary = MP5, Crossbow, Shotgun, RPG, Gauss, Egon;
  - secondary = Glock, Python, Hornet;
  - throwable = HE, Snark, Satchel, Tripmine.

### 8.2 Combat weapon choice: `fireWeapons()` (`combat.cpp:1559-1681`)

Runs when `doFireWeapons` allows (§1.4).
1. Return if using a grenade, or on the GG tripmine level.
2. `distance = |lookAt − eyes|`. A friend in the line of fire sets `m_fireHurtsFriend` and returns.
3. Knife mode gives the crowbar.
4. **Stab:** `stab_close_enemies`, diff ≥ Normal, HP > 80, enemy, distance < 100, not ≥ 2 visible enemies within 768 u, not camping → crowbar.
5. Loop rows, skipping throwables and (GG) non-forced weapons. Pick the **last** row that is owned, has ammo (clip > 0, or reserve ≠ 0 for clip-less weapons), is `!isWeaponBadAtDistance`, and is not blocked underwater (waterlevel 3). The effective order is Egon > Gauss > RPG > MP5 > Shotgun > Crossbow > Python > Hornet > Glock > Crowbar; **personality prefs are not used here**.
6. If nothing but row 0 qualifies: if the current weapon has reserve ammo, set `isReloading` and reload Primary and return; else use the crowbar.

**`isWeaponBadAtDistance`** (`combat.cpp:1683-1712`):
- False for diff < Normal, or when no secondary weapon is owned, or for non-gun types, or when the best owned pistol has a clip at 0.
- Otherwise bad outside [min, max]: shotgun > 750, crossbow < 400, RPG < 300, MP5 > 2000, gauss > 3000, egon < 128 or > 2000.

### 8.3 Firing: `handleWeapons()` (`combat.cpp:1019-1221`)
1. If not reloading: reloadState None, `reloadCheckTime = now+3`.
2. **Gauss charge held:**
   - weapon changed: reset state;
   - enemy present, task not GaussJump, and (state ≠ ChargeCombat or age ≥ 1.3 s): `releaseGaussCharge()`;
   - otherwise return (the controller holds the button).
3. Select the weapon if needed (reset fire pause); return that tick.
4. `checkZoom` (crossbow): > 700 u and unzoomed, or < 400 u and zoomed → ATTACK2, `shootTime = now+0.15`, `zoomCheckTime = now+1`; skip the shot.
5. **Crossbow stand-still:** aimed at Enemy or LastEnemy, > 600 u, not reloading, moving → zero speeds. If |v| > 5 on any axis: `sniperStopTime = now + 2 − 0.35·diff`; skip the shot.
6. **Gauss combat charge:** state Idle, 500–3000 u (and > 500), enemy, uranium ≥ 20, `chance(60)` per call → ChargeCombat, release planned rg(1.4,1.8) s (only used for demotion), press ATTACK2, return.
7. **MP5 M203:** 300–700 u, AR grenades > 0, `altFireTime<now`, no friend in the line of fire → edge-press ATTACK2, `altFireTime = now + rg(2.5,4)`.
8. **Shotgun double:** 32–300 u, clip ≥ 2, `altFireTime<now`, `chance(50)` → edge-press ATTACK2, `altFireTime = now+1.5`.
9. **Rapid:** Glock (32–300) or Hornet (32–250, needs ≥ 4 hornets) use ATTACK2 as the attack input.
10. **RPG under 300 u:** never fire (`shootTime=now−dt`; return).
11. **Close branch** (distance < 272 and recoil not high, or blind, or knife):
    - crowbar swings only within 64 u (80 for creatures);
    - hold-fire weapons hold the input; others toggle (press only if not held last frame);
    - `shootTime = now−dt` (fires every other think tick).
12. **Far branch:**
    - crowbar never swings;
    - `needToPauseFiring` (below) may skip the tick;
    - hold weapons: hold;
    - semi-auto: toggle plus the difficulty delay (§4.3).

**`needToPauseFiring`** (`combat.cpp:954-990`):
- Skip for sniper, grenade, or Suspect within 400 u.
- If a pause is active, pause.
- < 272 u: no pause.
- offset = 2.75 (< 544 u) or 4.25.
- If `tan(√(rad(punch.x)² + rad(punch.y)²))·distance > offset + maxRecoil + tolerance`: `firePause = now + rg(0.55, 0.55 + maxRecoil·0.01·tolerance) − dt`.
- `isRecoilHigh` = `punchangle.x < −1.45`.

### 8.4 Per-weapon behavior
- **Crowbar:**
  - swing < 64 u (80 creature);
  - used in knife mode (`jasonmode`, only-crowbar, creature, or seeing an enemy with no clip ammo anywhere);
  - stab when close and healthy;
  - random idle swings;
  - drawn before long jump links (CS leftover, §14);
  - fallback when out of ammo.
- **Glock:** semi-auto toggle; Rapid secondary at 32–300; on the GG tripmine level only used by DetonateTripmine (edge taps at cone ≥ 0.97; reload via reloadState Secondary).
- **Python:** semi-auto; no secondary (zoom not used); not underwater.
- **MP5:** auto hold; M203 at 300–700 with a 2.5–4 s cooldown and friend check; SMG aim-height table; ammo shared with the glock (9mm).
- **Shotgun:** semi-auto; double blast at 32–300 (50%, 1.5 s cooldown, clip ≥ 2); headshot disabled beyond 272 u; bad beyond 750 u.
- **Crossbow:** zoom > 700 / unzoom < 400 or when idle; stand still for > 600 u shots; no head aim < 800 u; head aim lowered 0.35·view_ofs; approach capped at 49; fear ×1.5 and aggression ×0.5; zoom shrinks the view cone to 20°.
- **RPG:** never < 300 u (switches down if Normal+ with a sidearm); aims 30 u below the origin of grounded enemies. Laser guidance is implicit: the bot keeps aiming at the target; there is no explicit laser-toggle code.
- **Gauss:** primary is auto hold. Secondary charge (§8.5). Never switches weapon while charged (`selectBestWeapon` refuses).
- **Egon:** auto hold; bad < 128 u.
- **Hornet:** auto hold (primary, homing); Rapid secondary 32–250 when ≥ 4 hornets; clip-less.
- **HandGrenade, Snark, Satchel, Tripmine:** only through tasks (§8.6, §10, §11). Never selected as guns; `selectBestWeapon` skips them except on the GG tripmine level.

### 8.5 Gauss charge controller (`updateGaussCharge`, `combat.cpp:1401-1516`; constants `constant.h:306-310`)

States: Idle, ChargeRoam, ChargeCombat, Releasing. Full charge 1.6 s, roam hold max 6 s, absolute max 8 s, release window 0.2 s.

- **Releasing:** strip ATTACK and ATTACK2 (unless a grenade task is active). For dump releases (task not GaussJump and `gaussJumpLookAt` set), Override aim at it. After 0.2 s: Idle, `gaussCheckTime = now + rg(6,12)`.
- **Idle, roam precharge** requires all of:
  - `gauss_precharge`, diff ≥ Easy, not creature, holding the gauss, timer ready;
  - no enemy and not seeing;
  - uranium ≥ 25; on the floor; not in water or on a ladder; not throwing;
  - task ∈ {Normal, MoveToPosition, Hunt, Camp}.
  
  Then with `chance(40+10·diff)` → ChargeRoam (release plan +6 s), press ATTACK2; else retry in rg(6,10) s.
- **Sanity:** weapon switched away or 0 ammo → Idle.
- **Age ≥ 8 s** and (on the floor or age ≥ 9 s) → `startGaussDumpRelease`.
- **Water ≥ 2 or approaching a ladder** (not on it) → dump.
- **ChargeCombat:** no enemy, or `now > plannedRelease + 1` → demote to ChargeRoam.
- **ChargeRoam** age ≥ 6 s (not already jumping) → GaussJump task if `canStartGaussJump()`.
- Otherwise keep holding ATTACK2.
- A roam-charged gun releases **immediately** at the first aimed enemy (`handleWeapons`).
- **`startGaussDumpRelease`** (`combat.cpp:1369-1399`): direction = travel direction (or view forward). If there is a deadly drop 192 u that way, flip it; if both ways are deadly, look = eyes − dir·55 + 80 z (steep up-back). Else look = eyes − dir·95 − 12 z (flat back shove), then release.

### 8.6 Throwables: `checkGrenadesThrow()` (`combat.cpp:2553-2817`)

**Grenade-war mode** (`isGrenadeWar`): `yb_grenadier_mode`, or GunGame with no primary/secondary weapons owned.

Steps:
1. **Abort** (clear throw states) if normal mode and any of: Narrow node, `ignore_enemies`, using a grenade, reloading, knife mode, gauss charge held, check timer active, or no `lastEnemyOrigin`. War mode aborts only without `lastEnemyOrigin`.
2. Timer = now + 0.3.
3. lastEnemy must be alive, and in normal mode Suspect or Hearing must be set (Hearing is sticky).
4. **Choice** `bestGrenadeCarried(dist²)`: HE > Satchel > Snark. With both HE and satchel and no live satchel: < 300 u gives satchel; 300–400 u gives 50% satchel. None: timer = now + 15.
5. Normal-mode cancel roll: 3% (aggression > fear) or 10%; satchel 10%; snark 25%.
6. Distance = 2D. Make it infinite if:
   - the enemy is airborne (not in water) above the bot's head;
   - the enemy is > 500 u above;
   - (normal mode) the enemy is attacking with LOS and has the bot in its cone;
   - (normal mode) **the enemy was seen within 0.12 s**, so no normal-mode throws while an enemy is in view.
7. **Windows:** default 300–800; satchel 150–400 (800 if jump-eligible); snark 150–800; war-mode minimum 96.25.
8. **HE:**
   - with friendly fire, skip if friends are within 256 u of the enemy;
   - lead point = origin + velocity2d, radius = max(192, speed);
   - up to 12 graph nodes near it; the first with a feasible `calcThrow`/`calcToss` becomes `m_throw` (+110 z).
9. **Satchel:**
   - skip if a live own satchel exists, friendly-fire friends are near, or there is no LOS (from the body);
   - beyond 400 u only as a jump-throw (§11) if eligible and `canStartSatchelJump`;
   - `m_throw = lastEnemyOrigin`.
10. **Snark:** skip if in water or the enemy is > 200 u above; needs LOS; `m_throw = lastEnemyOrigin`.
11. **Start:** ThrowExplosive, ThrowSatchelJump (+4 s) or ThrowSatchel, or ThrowSnark, at priority 99 with expiry now + 2.16 s.

**ThrowExplosive** (`tasks.cpp:715-801`):
- Target = `m_throw`, or enemy origin + velocity2d when visible. Stop if not seeing.
- Expired and not cooking (or cooked > 1 s ago): finish.
- Normal mode: target within 385 u (3D) and not cooking → abort, check timer +1.2 s.
- `m_grenade = calcThrow(eyes, dest)` or `calcToss`; if infeasible in normal mode, abort.
- Grenade aim. If its own grenade entity (w_grenade) exists: **set its velocity = m_grenade·(1 + 4·dt)**, check timer +1.5 s, best weapon, complete.
- Else select the HE. Pull the pin: cook `clamp(1.1 − dist/800, 0.1, 0.75)` s holding ATTACK, then release and shorten the task to 0.5 s.

**calcThrow / calcToss** (`combat.cpp:2425-2528`): gravity = `sv_gravity·0.55`.
- calcThrow: time = dist/195 (capped at 1.2 if > 2); `v = Δ/t`, `v.z += g·(t/2)²`; apex hull traces; ×0.7793.
- calcToss: apex ≤ 500 u above the midpoint (ceiling-limited); ×0.777.

**ThrowSnark** (`tasks.cpp:803-846`): abort if in water, no snark, target > 200 u above, or expired. Grenade aim at `m_throw` (the computed `m_grenade` direction is unused). Select the snark, one ATTACK edge (`data=1`), complete 0.5 s later.

**ThrowSatchel** (`tasks.cpp:916-967`): finish if out of satchels, expired, or **a live own charge exists** (confirmation). Grenade aim at `m_throw`. Select the satchel, press **ATTACK2** (always throws), retry every 1.1 s. The deployed charge becomes a trap owned by `checkSatchelDetonate`.

**Satchel trap** (`botlib.cpp:438-473`, `tasks.cpp:848-914, 1129-1201`, constants `constant.h:316-319`):
- Scan every 0.3–0.5 s in any task except the throw/detonate tasks. No live charge releases the throw latch. Skipped underwater, without the satchel weapon, or with a gauss charge held.
- `satchelVictimNear`: abort if any teammate is within 200 u of any own charge; trigger if an enemy is within 160 u of a charge and closer to it than the bot.
- **DetonateSatchel** (98, 4 s): abort if there is no charge, it expired, or there is no victim. If within 320 u of any own charge, face the nearest charge and backpedal. Otherwise select the satchel radio and edge-press **ATTACK** (detonate).

**Tripmine** (both modes): camp mine (§2.3), corner mine (§10). **PlaceTripmine** (`tasks.cpp:1203-1256`):
- Stop, Override aim at `m_position`. Abort on seeing an enemy or timeout (6 s).
- Select the tripmine. When within 10° yaw and 100 u: ATTACK edge, `data=1`, then after 0.75 s `registerTripmine(pos)`, best weapon, complete.
- Placement success is not verified.

### 8.7 Reload, switching and ammo
- **`checkReload`** (`combat.cpp:2335-2413`):
  - Skipped during pickup, throw and plant tasks, using a grenade, or holding the crowbar (sets reloadState None).
  - Sets `isReloading=false`, `reloadCheckTime+3`.
  - For Primary, then Secondary: take the **lowest-id owned weapon in that mask** (§14). If it has a clip, clip < 75% of max and reserve > 0: select it, release ATTACK, press RELOAD (edge), `isReloading=true`.
  - Otherwise, if seeing/hearing or seen within 5 s, stop; else advance to the next slot.
- **`selectBestWeapon`** (`combat.cpp:2148-2235`):
  - GG tripmine level: hold the tripmine. GG forced weapon: select it.
  - Knife mode: crowbar. Keeps the gauss while charging. No-op while reloading.
  - Otherwise the last non-throwable row that is owned with reserve ≥ min (or is current with clip ≥ min).
  - Called at: camp start, hearing an enemy, after throws/plants/detonations, and after landing a jump link with the knife out.
- No proactive ammo management beyond pickups. HL auto-reloads empty clips.

---

## 9. Item pickup (`updatePickups`, `botlib.cpp:498-816`)

**Blocked** when: creature; using a grenade, cooking, or throw latch; in DetonateTripmine, PlaceTripmineMoving, DetonateSatchel or ThrowSatchelJump; seeing an enemy; on a ladder; `jasonmode`; no interesting entities.

**Keep the current target** while it is valid, visible, within radius and not owned.

**Scan** the interesting list in **entity order; the first acceptable wins** (not the nearest or best). Candidates must be:
- not NODRAW, not ignored;
- |dz| ≤ 96, within 450 u (`object_pickup_radius`);
- in GunGame, `weapon_*`, `ammo_*` and `weaponbox*` are skipped;
- visible (`seesItem`).

**Acceptance rules:**
- `weaponbox*`: always (DroppedBox).
- `weapon_*` already owned: ammo refill only if `mp_weaponstay` ≤ 0, `pickup_ammo_and_kits`, and reserve < 70% of max.
- `weapon_*` not owned: `pickup_best` and `rateGroundWeapon`. The ground model's position in the personality pref list must exceed the best owned: positions < 4 compare with `bestSecondaryCarried` (first owned pistol position), else `bestPrimaryCarried` (highest owned non-throwable position). Quirk: throwables are effectively never picked up when a pistol is owned.
- `ammo_*`: `pickup_ammo_and_kits` and useful, meaning the linked weapon or its ammo-sharing partner is owned and below 70% (M203 grenades below max). Links: 9mm ↔ glock/MP5, AR grenades → MP5 secondary, buckshot, 357, bolts, rockets, uranium ↔ gauss/egon.
- `item_healthkit`: HP < 85. `item_battery`: armor < 90 (both need `pickup_ammo_and_kits`).
- `item_longjump`: if not owned (cvar-independent).
- `item_suit`, `item_antidote`, `item_security`: ignored.
- `func_healthcharger`: HP < `charger_health_threshold` (60). `func_recharge`: armor < 40. Both need `use_chargers`.
- Other `item_*` only with `pickup_custom_items`.

**After a match:**
- Drop it if another alive bot targets it.
- Ignore it permanently (this life) if it is higher than eyes + 20 u (charger: 50 u) or `isDeadlyMove`.

The desire and task are in §3 and §2.3. The stuck guard (§1.4 step 21) blacklists and sets a 5 s cooldown.

---

## 10. GunGame (external plugin; detection `engine.cpp:1035-1045`, re-evaluated about once per second)

**Level inference** from `pev->weapons` only; the plugin's level number is never read:
- `refreshForcedWeapon` (`botlib.cpp:475-496`, every think): the forced weapon is the single owned non-melee, non-throwable gun, if exactly one is owned.
- `isGunGameMeleeLevel`: only crowbar (+ suit bit 31).
- `isGunGameTripmineLevel`: tripmine owned and no primary weapon (the glock sidearm is allowed).
- `isGrenadeWar`: no primary or secondary weapons at all.

**Effects:**
- **Weapon use:**
  - `selectBestWeapon`: tripmine level with mines holds the mine; else the forced weapon.
  - `fireWeapons`: candidates are restricted to the forced weapon, with crowbar fallback; the stab rule still uses the crowbar.
- **Attack gating (tripmine level):**
  - `fireWeapons` returns immediately (`combat.cpp:1567-1571`);
  - `filterTasks`: Attack desire 0 and Hunt 0 (`botlib.cpp:1269-1278, 1332`);
  - the glock fires only in DetonateTripmine.
- **Melee level:** Hunt has no 90 s round-mid gate, Careful bots also hunt, and hunt desire ≥ 70.
- **Leader priority:** `bots.getGunGameLeader()` (`manager.cpp:1045-1071`) caches the highest-frag alive client (frags > 0) for 1 s. `lookupEnemies` multiplies that player's dist² by `gungame_leader_priority` (default 0.25; 1.0 disables).
- **Goals and pickups:** `findBestGoal` sets goal-node desire 0 (weapon spots are useless). Pickups skip weapons, ammo and weaponboxes.
- **Grenade war:** relaxed throw rules (§8.6).

**Proactive plant** (`checkProactiveTripminePlant`, `botlib.cpp:322-368`, tripmine level only):
- **Rush miner** (`m_rushTripmineMode` per-life roll 50/80/25 by Normal/Rusher/Careful): every rg(1,1.5) s, if the task is Normal, it has mines and it is on the floor, push PlaceTripmineMoving (60, 2 s).
- **Others:** every rg(2.5,4) s, if not seeing an enemy, not already planting, mines > 0 and on the floor: trace 128 u along the view. A wall within 90 u of the eyes, not within 96 u of a known mine, starts PlaceTripmine (6 s).

**PlaceTripmineMoving** (`tasks.cpp:1258-1329`):
- Abort without mines or off the level. Advances the route itself; complete on goal reach.
- Keeps running at maxspeed.
- Each frame, trace from the eyes to eyes + moveDir·40 − 96 z. Needs a floor (normal.z ≥ 0.7) within 120 u and no known mine within 96 u.
- Override aim at the spot. Within 20° yaw: ATTACK edge; confirm after 0.75 s → register, best weapon, complete. 2 s window.

**Detonation** (`checkTripmineDetonate`, `botlib.cpp:370-436`, tripmine level only):
- Every 0.4–0.6 s, not while detonating or planting. Needs the glock with clip or reserve.
- For each known mine (any owner): 350–1200 u away; ≥ 1 enemy within 140 u of the mine; no friend within 200 u; eyes→mine trace fraction ≥ 0.9; resolve a live `monster_tripmine` within 48 u.
- Start DetonateTripmine (98, 4 s).

**DetonateTripmine** (`tasks.cpp:1640-1701`):
- Abort (cooldown rg(1,2) s) if the mine is gone or expired, there is no enemy within 160 u, or a friend is within 200 u.
- Select the glock, stop, Override aim at the mine.
- Empty clip: reload via reloadState Secondary (abort if there is no reserve).
- Edge-tap ATTACK when cone ≥ 0.97.

**Corner stealth mine** (`checkCornerTripminePlant`, `navigate.cpp:2379-2462`; all modes, called on each node advance):
- Skip if rush miner on the tripmine level, or cooldown active.
- Needs Normal task, not seeing, mines > 0, on the floor.
- Turn angle ≥ 60° (`incoming·outgoing ≤ 0.5`); the previous and next nodes must not see each other (vistable, or a trace).
- Probe 80 u toward the inner side at corner + outgoing·45. Needs a near-vertical wall (|n.z| ≤ 0.3), no known mine within 96 u, and an opposite wall within 250 u.
- Cooldown rg(8,12) s on the tripmine level, rg(20,30) s otherwise. Start PlaceTripmine (6 s).

---

## 11. Longjump, satchel jump, gauss jump

### 11.1 Longjump (`navigate.cpp:727-990`; per think; not a task)
- The module is known from physinfo `slj` (every 0.5 s) or a pickup, and is lost on death.
- **Common gates:**
  - `use_longjump`, owned, not creature, cooldown elapsed, not stuck, gauss not Releasing, not approaching a ladder;
  - **JUMP and DUCK not held this or last frame** (the engine needs a fresh press of both);
  - `duckTime < now`, `jumpTime + 1 < now`;
  - on the floor, not on a ladder, in water or ducking;
  - not avoiding or using a grenade; speed2d ≥ 150.
- **Attack hop** (`checkAttackLongJump`):
  - SeeingEnemy and task Attack; enemy 2D distance 400–750 and dz ∈ [−64, +40];
  - `moveSpeed ≥ 0`; `health·aggression ≥ 30`;
  - view forward 2D · dir-to-enemy ≥ 0.95;
  - launch along the view.
- **Path hop:**
  - skip if an enemy is within 400 u; needs `m_moveToGoal`;
  - task ∈ {Normal, Hunt, MoveToPosition, SeekCover};
  - moveSpeed ≥ max − 10 with no strafe;
  - not a jump link; current node not Crouch, Ladder, Lift, Button or DoubleJump;
  - on a settled frame (dest == pathOrigin within 1 u), dest ≥ 50 u away, velocity·dir ≥ 0.93, dest dz ∈ [−64, +40];
  - build a runway from the bot (+ current node if > 24 u ahead with dot > 0.5) plus up to 10 path nodes that are flat, have no bad flags and plain links;
  - need ≥ 3 points and runway ≥ 400 u; corridor·dir ≥ 0.92; every leg·corridor ≥ 0.94.
- **`tryLongJumpAlong(dir, maxDrop)`:**
  - retry throttle 0.5 s;
  - head-hull trace origin + 17 z → origin + dir·250 + 50 z must be clear;
  - no deadly drop at 445 u (also 520 u if maxDrop > 16);
  - press DUCK|JUMP; cooldown rg(0.9,1.4) s; flight window 1 s.
- During flight: 2D node reach with ≥ 50 u radius, parachute suppressed, gauss travel-jump suppressed.

### 11.2 Satchel jump-throw (ThrowSatchelJump, `tasks.cpp:969-1127`)
- **Eligibility** (§8.6): per-life roll 40/60/20, not war mode, `use_satchel_jump`, diff ≥ Normal, not creature, timer, on the floor, not in water, no live satchel, target 400–800 u with LOS. `canStartSatchelJump`: 72 u head clearance and a hull corridor to dir·280 + 120 z (fraction ≥ 0.85).
- **Abort** (retry in rg(3,6) s) if the satchel is lost, 4 s expire, or (stage < 2 and target within 150 u 2D).
- **Stage 0:** Override look at the target at eye height; run straight at it at maxspeed (`moveToGoal` off). Draw the satchel; hold 0.9 s for the deploy delay; then jump when speed2d > 0.6·max and on the floor.
- **Stage 1:** keep running and edge-press JUMP. Airborne → stage 2. No lift-off in 1 s → abort.
- **Stage 2:** keep running; look at target + lift z, where lift = clamp(0.18·dist2d, 24, 140). After 0.25 s airborne, press ATTACK2 (retry every 0.4 s). A live charge → stage 3 (2.5 s). Landing with nothing thrown → abort.
- **Stage 3:** backpedal at full speed with LastEnemy aim. Detonate (ATTACK edge) when `satchelVictimNear && satchelSelfSafe`. End when the charge is gone or after 2.5 s: clear path, best weapon, complete. A remaining charge stays a trap.

### 11.3 Gauss jump (GaussJump task, priority 99, 8 s; `tasks.cpp:1443-1577`, `combat.cpp:1242-1339, 1518-1557`)
- **Triggers:**
  - A stale roam charge (≥ 6 s).
  - Deliberate travel (`checkGaussJumpTravel`): cvar, Idle, timer, task ∈ {Normal, Hunt, MoveToPosition}, gauss in hand with ≥ 30 uranium, not in longjump flight. The goal must be far: chosen goal > 1400 u 2D or path > 12 nodes; otherwise recheck in rg(4,6) s. Then timer rg(10,18) s, `chance(33)` and `canStartGaussJump`.
- **`validateGaussJumpLaunch(dir)`:**
  - cvar, diff ≥ Normal, not creature, HP ≥ 60;
  - on the floor, dry, no ladder; no enemy and not seeing;
  - 72 u head clearance; hull corridor origin + 32 z → dir·280 + 150 z with fraction ≥ 0.85; no deadly drop at 768 u.
  - **Look-at** = eyes + back·96·cos α − z·96·sin α, with α = rg(25°,38°) below horizontal and back = −dir rotated rg(−8°,8°) in yaw.
  - Target = origin + dir·768 + 64 z.
  - Travel direction = 2D direction to path node [2] (≥ 128 u), else to the chosen goal.
- **Abort** on expiry, or (not released and (not holding the gauss, or stage > 0 with the charge lost)).
- **Stage 0:** stand still. If no charge, start ChargeRoam (abort if Releasing). After ≥ 1.6 s of charge and on the floor, re-validate toward the stored target (≥ 128 u), else complete (keeping the charge).
- **Stage 1** (≤ 1.2 s): Override aim at the look-at; proceed at cone ≥ 0.97 (or ≥ 0.90 after the timeout), else abort.
- **Stage 2** (0.25 s): edge JUMP; release when airborne or on timeout.
- **Stage 3:** hold the aim until the 0.2 s release window passes.
- **Stage 4** (3 s): look at target + 32 z, steer `m_destOrigin`=target at maxspeed. Complete on landing after ≥ 0.3 s, or on timeout; clear the path.

---

## 12. Chat (`chatlib.cpp`, `config.cpp:407-506`)

**Files:** `conf/lang/<yb_language>_chat.cfg`, falling back to `en_chat.cfg`. A missing file sets `yb_chat 0`.

**Sections:**
- `[KILLED]` → Kill; `[DEADCHAT]` → Dead; `[TEAMATTACK]`; `[TEAMKILL]`; `[WELCOME]` → Hello; `[UNKNOWN]` → NoKeyword.
- `[REPLIES]` holds `@KEY "K1","K2"` lines followed by reply lines. Keywords are upper-cased; inner spaces act as word boundaries.
- Unknown sections are ignored. **en/de files have no TEAMATTACK/TEAMKILL banks**, so those events are silent.

**When bots talk:**

| Event | Chance / gate | Ref |
|---|---|---|
| Welcome on join | 20% flag, said on the first alive slow frame | manager.cpp:1837; botlib.cpp:1792 |
| Kill of an enemy | 10% (processed at 10 Hz) | botlib.cpp:1158 |
| Teamkill | always, team say | botlib.cpp:1163 |
| Hurt by a human teammate (tkpunish) | always | botlib.cpp:2148 |
| Dead chat | **only while dead**, per 0.5 s slow frame: `chance(chat_percent=30)`, own last chat > rg(6,10) s ago, global last bot chat > rg(2.5,5) s ago, and not replying; skips phrases prefixing recent ones; recent list reset when longer than rg(4,6) | chatlib.cpp:364-407 |
| Replies | also only while dead | see below |
| DoubleJump acknowledgement | team say "Ok %s, i will help you!" | botlib.cpp:2320 |

**Replies:**
- Target: the last `say` from a player in the same alive/dead state. Humans are captured in `captureChat` (`manager.cpp:1842`); bots propagate through `pushMsgQueue` to bots with an equal alive state.
- Timing gate `timeNextChat < now + rg(delay/2, delay)`.
- Chance `chatProbability` (rg(10,100) per bot) + rg(40,70).
- Keyword match gives a random unused reply (the used list resets at 1/4 of the replies). No match: 70% a NoKeyword line.

**Formatting:**
- Placeholders (max 6 replacements):
  - `%f` top-frag player; `%m` map name; `%r` time left (mm:ss);
  - `%s` replied-to player (or top-frag); `%v` last victim;
  - `%d` "HLDM" (30%) or "Half-Life";
  - `%t` alive teammate (or teammate damage inflictor); `%e` alive enemy; `%g` graph author.
- Names: 80% strip clan tags, else trim; 8% lowercase (en).
- Typos: 8% lowercase the whole line (en). If length > 15: `(len/2)%` chance to drop one char, and `(len/4)%` to swap two adjacent chars (alphanumeric-only for chs/cht).
- Sent as `say "…"` or `say_team "…"`. Creatures never chat.

---

## 13. Team play

- **Teams:**
  - FFA: `client.team = index+1` (everyone is an enemy).
  - Teamplay (mp_teamplay, GameMode message, or `yb_game_mode`): TeamInfo strings map to 0/1 in first-seen order; extra teams collapse into 1 (`message.cpp:142-176`).
  - `numFriendsNear` is 0 in FFA.
- **Friendly-fire avoidance** (only with `mp_friendlyfire` and not FFA):
  - `isFriendInLineOfFire`: a direct trace (buggy direction, §14) plus any teammate within distance whose cone dot exceeds `d²/(d²+33²)`. Blocks firing and makes the next style roll Strafe.
  - M203 withheld; HE and satchel not thrown with a teammate within 256 u of the target.
  - Satchel never detonated with a teammate within 200 u of any own charge; mines not shot with a teammate within 200 u.
- **Avoidance and sharing:**
  - `doPlayerAvoidance` (teamplay only; navigation): priority-based strafe around teammates. Also completes Camp/Hide/Pause on a shared node.
  - `isOccupiedNode`: node sharing.
  - Priority: humans > bots on jump links or MoveToPosition/SeekCover/Camp/Hide > others (`manager.cpp:1377`).
- **Information sharing:** teammate alarm (§5.2); Hard+ kill notification (§5.6).
- **Following:** at spawn + rg(5,7.5) s: 20% (`user_follow_percent`) × 50% of non-leaders follow a random visible human teammate.
- **Leaders:** chosen once per map by frags; the flag is reset on death, so it is effectively dead.
- **tkpunish:** a human teammate damaging the bot becomes its enemy immediately. A human teamkill makes the dead bot send `vote` (CS-only); with tkpunish 2 it slays the killer and gives them +1 frag.
- **Hunt** drops a lastEnemy who became a teammate. Teammates' grenades are not avoided. Practice/danger data is per team (FFA clamps into team slot 1).

---

## 14. Quirks, bugs and dead code (do not port blindly)

### Core logic bugs
1. **Default FFA skins turn bots into "creatures".** `kBuiltinSkins` includes `"zombie"` (`manager.cpp:1185`), and `isCreature()` matches model names starting with "zo" or "ch" (`botlib.cpp:2615-2621`). About 10% of FFA bots, and any teamplay team named zombie, become CS-zombie-mod creatures:
   - knife-only;
   - no pickups, grenades, chat, hiding, gauss, satchel or longjump;
   - special hunt desire (90) and 128 u reach;
   - traces through nothing.
2. **`m_previousNodes[1]` is never written** (`navigate.cpp:2011-2014`), so `[1..4]` stay −1. The ladder-tower avoidance (`navigate.cpp:1226`) and the lift fallbacks (`1735`, `1757`) are dead.
3. **Emotion clamp bug:** `if (level > 1) level += 1.0f` instead of clamping (`botlib.cpp:2155-2164`). Fear and aggression can exceed 2; seek-cover desire can explode.
4. **`startTask` calls `clearSearchNodes()` for every non-matching stack element** before finding a match (`botlib.cpp:1446-1455`). Re-asserting a deeper task (every heavy tick in `filterTasks`) wipes the path at 10 Hz (Hunt, MoveToPosition and others re-path constantly).
5. **Hysteresis is a no-op** (Attack is binary 0/90). Hide's filter desire is never set, so `subsume(Hide, …)` is dead.
6. **HearingEnemy and the timeout-clear of `lastEnemyOrigin` never run.** The `else if (soundUpdateTime ≥ now …)` branch (`botlib.cpp:1189-1203`) is unreachable because heavy ticks are 0.1 s apart and the cooldown is 0.05 s. HearingEnemy stays set until death. That permanently satisfies the grenade sense gate and blocks secondary reloads.
7. **`m_aimFlags` is reset only at 10 Hz** (`botlib.cpp:1142`). Task-set flags (Override, Enemy and others) linger up to 100 ms after the task ends.
8. **`m_enemyParts`/`m_enemyOrigin` belong to the last in-cone player tested**, not necessarily the chosen enemy, during full rescans (`combat.cpp:449-483` with `checkBodyPartsWithOffsets:205`). Aim and "full view" can be wrong for a tick.
9. **Head selection re-rolls `chance(headshotPct)` every tick** until it locks (`combat.cpp:698-709`), so it converges to head within a fraction of a second. Once locked, the recoil and shotgun exclusions are bypassed.
10. **`isFriendInLineOfFire`** traces along `v_angle.normalize()`, normalizing the angle triple as if it were a direction (`combat.cpp:795`).
11. **`isDeadlyMove` only checks the target point.** Its walk-back loop never runs because the distance starts at 0 (`navigate.cpp:3128-3155`). Longjump, gauss and grenade-avoid safety all depend on it.
12. **`isEnemyInDarkArea`:**
    - the cvar test uses `&&` instead of `||`;
    - the flashlight logic is inverted (flashlight on means hidden);
    - the final trace ignores monsters, so `pHit` is never the enemy and the function always returns false for non-creatures (`combat.cpp:134-172`).
13. **`pev->fov` is 0 in HL:**
    - `checkBreakablesAround` requires `angle < 0`, so it never fires (dead) (`botlib.cpp:211`);
    - `avoidGrenades`' FOV test degenerates to "any visible grenade in 360°" (`botlib.cpp:87`).
14. **`avoidGrenades`:** the strafe sign moves toward the grenade side (`botlib.cpp:106-111`, looks inverted). Landed grenades stop being avoided; M203 grenades and rockets are never avoided.
15. **`takeBlind`:** after the first 5 s of the map the view distance is restored immediately (inverted condition, `botlib.cpp:2198`). Normal difficulty never sets `m_blindNodeIndex` but `blind_` uses it for diff ≥ Normal (stale index).
16. **Seek-cover gate** `m_seeEnemyTime − rg(2,4) < now` is always true (`botlib.cpp:1290`).
17. **`attackMovement`:**
    - approach < 30 (not reloading) while far keeps `moveSpeed = maxspeed`, so low-health or timid bots charge;
    - the early return on `m_lastUsedNodesTime` is dead (`combat.cpp:1788`);
    - "isEnemyCone" is the bot's own cone, not the enemy's.
18. **`needToPauseFiring`:** the `(SuspectEnemy && distance < 544)` branch is unreachable (`combat.cpp:972`). In `getEnemyBodyOffset`, `!m_enemyParts && Suspect` is unreachable (`combat.cpp:681`).
19. **`checkReload` only inspects the lowest-id owned weapon in the slot mask** (`combat.cpp:2377-2382`). For example, owning an MP5 means the shotgun is never topped up.
20. **`selectBestWeapon`** ignores weapons with a loaded clip but no reserve unless held.
21. **`rateGroundWeapon`** compares a preference position with `kPrimaryWeaponMinIndex` (a table-row constant). `bestSecondaryCarried` returns the least preferred pistol.
22. **`getCampDirection`** never returns `dest` when LOS is clear (returns the danger node or null) (`botlib.cpp:856-895`).
23. **Knife-chase MoveToPosition time** uses dist²/speed² and is ignored by `moveToPos_`. The pickup-stuck guard likewise uses squared time.
24. **`followUser_` aim trace** end point lacks the origin (`tasks.cpp:643`).
25. **`findValidNode`** writes both teams' incremented danger into `m_team` (`navigate.cpp:1987-2002`). `selectBestNextNode` tests `PathFlag::Jump` (bit 0) against node flags, which is really Button (`navigate.cpp:2366`). `isOccupiedNode` tests `Used|Alive` with OR (`navigate.cpp:3275`).
26. **Gauss dump release:** the Override aim at the dump vector is set in the same tick the button is released (`combat.cpp:1369-1399` → `1405-1417`), so the shot goes along the current view, not the intended vector.
27. **Chat parser** never flushes the last `@KEY` block before the next `[SECTION]` or EOF (`config.cpp:473-493`); that reply group is dropped.
28. **`m_gunGameLeaderTime` is never reset on map change**, so a stale cached edict can survive (`manager.cpp:1053`).
29. **Hand-grenade target** `m_throw.z += 110` is fed into `calcThrow` in the task. The 385 u self-damage check uses 3D distance, so throws under about 370 u 2D are aborted.
30. **Attack monsters** (`yapb.cfg` enables it) targets any FL_MONSTER with no owner or team check: own and teammates' snarks and hornets (scaled priority).
31. **Engine detail to verify:** non-Attack tasks pass `m_moveAngles` to `pfnRunPlayerMove` while aiming via `pev->v_angle` (`botlib.cpp:2390-2397`). Check how your engine layer applies `cmd.viewangles` before relying on "shoot while navigating".

### Dead or CS-leftover code

**Unused (write-only or never set):**
- `selectCampButtons`, `sendToChatLegacy`
- `bots.canPause()/setCanPause`
- `m_checkWeaponSwitch`, `m_askCheckTime`, `m_changeViewTime`, `m_preventFlashing`, `m_voicePitch`
- `m_satchelJumpLookAt`, `m_killsCount`, `m_killsInterval`, `m_lastVictimTime`, `m_difficultyChange`
- `m_isOnInfectedTeam`, `m_infectedEnemyTeam` (never set)
- `cv_restricted_weapons`, `WeaponInfo::optimalDistance`
- `SecondaryFire::Detonate/Zoom` (markers only)
- `m_grenade` for snark and satchel
- `getShiftSpeed` (only the spray gate)
- All penetration functions and cvars (`shoots_thru_walls`, `seen/hearThruPct`)

**Doing nothing in HL:**
- `vote`/`votemap` commands
- `mp_freezetime` usage
- leaders (reset on death)
- `m_minSpeed` 260 at spawn
- drawing the crowbar before long jump links (`navigate.cpp:2595-2602`): a CS speed trick with no benefit in HL, costing switch time

**Removed by this port** (context for "faithful" porting; commit `4edb7f8`):
- walking / shift-speed near enemies
- crouch-while-Stay
- frequent camping (now 15% chance, 60 s cooldown, 5–15 s duration)
- seek cover outside reload, stuck and knife
- camp-crouch at hide spots
