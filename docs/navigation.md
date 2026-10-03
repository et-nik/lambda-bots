# Navigation

How bots get around: the graph they plan on, the contract each special link carries, how a bot carries a link out,
and what happens when it cannot. The design behind it is in `docs/design/nav.md`.

## The graph

By default the graph is made from the map itself (`lb-navgen`) on a worker when the map starts, and kept in
`addons/lambdabots/nav/<map>/` for the next time. `lb_nav_source yapb` (or `nav.source: yapb` in the main config)
imports the map's yapb `.graph` instead (`addons/lambdabots/nav/<map>.graph` or `addons/yapb/data/graph/<map>.graph`);
a generated graph also falls back to it when generation fails.

Either way every link is classified by simulation: `lb-kin` is a port of the engine's player movement
(`PM_PlayerMove`): friction, acceleration, stepping, gravity, jumping, ducking, ladders, water, and push fields
(`trigger_push`: while the player is in the field its horizontal push is added to the move and kept as momentum after,
and its vertical push accelerates the player against gravity, at that many units/s² — one on the ground is not lifted).

### Making the graph

1. **Floor.** The floor is flooded on a 16-unit grid from the spawn points, items, ladder ends, lift platforms,
   teleport exits, buttons and push-field landings, the way the engine walks a player (straight, or a step up and
   over, then down onto the floor). A column of the grid holds a span per floor (a room above a room). Walking off an
   edge is a fall; a ledge too high to step onto but at most 60 units up is a ledge (a crouch jump reaches 63). Doors
   stand open and breakables are gone while the floor is flooded; the spots they close are marked. Spans in lava,
   slime or a strong `trigger_hurt`, in push fields, and where a player would touch a ladder are marked too: no node
   goes there.
2. **Nodes.** Required spots get a node first: spawn points, items, ladder ends and rungs every 64 units, lift
   platforms, teleports, drop edges and landings, push-field entries and landings, a spot in reach of every button, a
   spot inside every trigger that sets a door or a lift off, and one on each side of every door. Then nodes fill the
   rest of the floor, the middles of corridors first, twice the room around them apart (112 to 224 units). Every span
   belongs to the node nearest to it on foot. Where two nodes whose floors meet cannot walk straight to each other
   (round a corner, through a doorway, past a pillar), a node goes on their border where it is widest, and the floor
   is shared out again; four rounds at most.
3. **Walks.** Nodes whose floors touch are joined when walking in a straight line from one to the other gets there:
   stepping up stairs is fine, but nothing on the way may push the walker off the line (a link that only slides
   along a wall is not made), so a link is clear to walk and to look along. Longer links are added only where the
   graph would otherwise make a bot go more than 15% out of its way (a greedy spanner, at most 12 links a node). A
   walk never passes through a push field, nor brushes past a ladder at a ledge's edge.
4. **Special links.**
   - Doors and breakables in the way of touching floors: `door` (through the middle of the doorway when its leaf
     slides across the way) and `breakable`.
   - Drops off every stretch of ledge; ladders, bottom to top; teleports; lifts.
   - Swims into and out of the water, onto ledges a swimmer at the surface can climb, and onto ladders rising out of
     it; ducked through low tunnels.
   - Jumps where walking round is more than twice as far, and onto every ledge the floor found.
   - Push fields: each field is run into from eight sides, walking and jumping in, drifting or keeping to its middle
     while it lifts; the flights land where nodes are put. Then flights are steered at the nodes near each field its
     entries cannot walk to, nearest first.
   - Tricks (see "Long jumps and gauss boosts" below): long jumps across gaps, from nodes near an edge to nodes
     160–560 units off (48 up to 240 down) that the graph joins only by a way more than twice as long, where the jump
     saves a second and a third of the way round and costs no fall damage. Gauss boosts are not made: they are put in
     by hand (the map editor's Link tool, or `add_link` with `kind: gauss_boost` in an overlay), and planned and
     checked then as a made one would be.
5. **Report.** The coverage report (`lb-cli nav coverage`) counts the floor and the items a bot gets to from the
   spawn points and back, and lists the rest.

On crossfire this takes 0.44 s on the stand's cores (890 nodes, 4645 links, 3 of them long jumps); on the largest
map, boot_camp, 1.3 s.
A kept graph loads in under a millisecond (96 ms on the stand, with the BSP and the visibility sets). The file
(`.lbnav`: postcard, LZ4, CRC-32C) is named by the key of what the graph was made from: the BSP (BLAKE3 and size),
the generator's version, the physics, the rules and the overlay. Only a graph with exactly the same key is used; the
four most recently used are kept. `lb nav regen` throws a map's graphs away and makes it again.

After the graph is loaded, the map's overlay patches are applied (see `docs/overlays.md`) and landmarks for the
planner are computed.

| Kind          | Classified as                                                                    | Contract                          |
|---------------|----------------------------------------------------------------------------------|-----------------------------------|
| `walk`        | a straight walk gets there (in an imported graph, also one sliding along a wall) | —                                 |
| `crouch`      | the same, crouched                                                               | —                                 |
| `drop`        | a simulated walk off the edge touches down within 96 units of the node           | speed, fall damage, health needed |
| `jump`        | a simulated running jump (see below) lands at the node                           | speed, duck, robustness           |
| `ladder`      | either end is on a ladder (and it is not a walk along a floor)                   | ladder normal, mount point        |
| `swim`        | either end is in the water: a simulated swim gets there                          | —                                 |
| `door`        | a door blocks the way; the map's mechanism graph says how it opens               | touch, use, or a remote button    |
| `lift`        | added from the map: platforms and doors that carry a player up                   | the mover, where to call it       |
| `teleport`    | a `trigger_teleport` stands between the nodes                                    | the trigger, the destination      |
| `breakable`   | a `func_breakable` blocks the way                                                | the brush to shoot                |
| `push`        | a simulated run into a push field, steered in the air, lands at the node         | the run, jump, holding still      |
| `longjump`    | a simulated long jump, steered in the air, lands at the node from every takeoff  | robustness, the module            |
| `gauss_boost` | a simulated gauss boost, steered in the air, lands at the node                   | the pitch, the gauss and health   |

Every special link is also checked on the live server once the map has loaded. The check fires a few traces per link
(the floor at both ends, the way between), at most 64 per frame, and compares them with the offline world. A link
the server disagrees with is switched off. On crossfire, all 1120 special links of the generated graph are confirmed.
`lb nav` shows the result.

## Contracts

A special link carries a `TraversalSpec`:

- **Entry and exit anchors:** where the traversal starts and ends, with a radius and a stance.
- **Action:** what to do — jump at a speed, possibly ducking; drop; climb; open a door; ride a lift; step into a
  teleport; break a brush.
- **Needs:** for a drop, the health the fall costs plus a reserve; the long jump module; a gauss with a full charge's
  uranium and the health a boost's landing leaves at 40 (60 at least).
- **Deadline:** the time the whole traversal may take.
- **Cost:** the time and damage the planner charges for the link.

The planner runs A* on travel time with these costs. Links the bot failed recently are left out (see below), and so
are the tricks it cannot do now: long jump links without the module, gauss boosts unless the bot's brain says it may
boost (see `docs/behavior.md`). Its heuristic is the larger of straight-line distance at the fastest any link goes
(running, or a long jump's 560 units/s; off on maps with teleports) and the ALT bound:
exact costs from and to 8–16 landmarks spread over the graph, which never overestimate. On crossfire that cuts a
search from 197 expanded nodes to 39 on average, on boot_camp from 643 to 153. Searches run in slices: all bots
together expand at most 2000 nodes a frame and 200 000 a second; a search that runs out goes on in the next frame
while the bot keeps to its old path.

## Following a path

On plain links the follower steers the bot itself:
- It heads for the next node and pushes against its own sideways drift. Friction alone takes the sideways speed down
  by two thirds in a quarter of a second, so a bot turning sharply at running speed would slide about 60 units wide
  of the new link, into the wall.
- Before it reaches a node, it heads on to the node after only if the way there from where it stands is clear (one
  trace per node). Otherwise it goes to the node first and does not cut the corner.
- A path starts at the node nearest the bot. When the next node is round a corner from where the bot stands, the bot
  goes to the nearest node first.
- It looks along the path, a stretch ahead and nearly level (see `docs/behavior.md`).

## Carrying a link out

The path follower walks plain links itself. For a special link it hands the bot to an executor. Every frame the
executor either returns a step (movement, look, pitch, buttons) with `Running`, returns `Waiting` (a mechanism is on
its way, which is not being stuck), or ends with `Done` or `Failed(reason)`. The follower enforces the deadline.

Before a special link the follower slows the bot to the speed the link takes: at most the jump's speed, 200 for a
drop, 150 for a ladder. Otherwise a bot running in at full speed passes a takeoff or runs off an edge before the
executor can act.

| Executor    | Phases                                   | How                                                                                                                                                                                                                                                          |
|-------------|------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| jump        | approach, run-up, takeoff, air           | backs up to the start of the run-up and stops there, runs at the planned speed, presses jump in the takeoff window, ducks in the air if planned                                                                                                              |
| drop        | edge, fall                               | walks off at the drop's speed, heading 48 units past the landing (slowing down at it stops on the ledge above it); walks the rest when it touches down within 96 units of the node on its floor, else fails                                                  |
| ladder      | board, climb                             | from below: walks to the mount point and into the ladder facing it; from above: steps back over the edge facing it; climbs by pitch and forward or back; steps off at the top, onto a ledge behind or beside the ladder when the climb stops against its top |
| push        | approach, run, flight, landed            | stops at the entry, runs along the checked line (jumping where checked), keeps to the field's middle while it lifts, then steers the flight at the landing; walks or swims the rest                                                                          |
| door        | check, go-activate, activate, wait, pass | a touch door is walked into; a use door is pressed with the use key; a remote door's button is pressed; waits for it to open, then passes                                                                                                                    |
| lift        | wait, board, start, ride, exit           | waits for the platform to rest on its side, boards, starts it (standing on it or pressing its button), rides, steps off at the top                                                                                                                           |
| teleport    | —                                        | walks into the trigger; done when the bot finds itself at the destination                                                                                                                                                                                    |
| breakable   | —                                        | shoots the brush (crowbar close up) until it is gone, then walks through                                                                                                                                                                                     |
| longjump    | approach, run, takeoff, air              | stops at the start of the run-up with the view level on the landing, runs at the takeoff, presses duck and jump together in the window, holds duck and steers the flight onto the landing                                                                    |
| gauss_boost | approach, charge, air                    | stops within 6 units of the takeoff; the weapons protocol draws the gauss, turns round, charges it for the link's push and jumps letting it go; steers the flight onto the landing                                                                           |

Buttons and doors are pressed the way a player presses them: from 64 units, looking at the target within 8°, with
a fresh press of use. A press counts only if the mechanism moves within 1.5 s; after 4 presses the link fails.

### Jumps

The validator and the executor follow one set of takeoff rules (`lb_kin::validate::takeoff`), so a jump that
validates is one a bot can make:

- **Run-up.** The bot backs up behind the takeoff point, as far as the speed needs (up to 40 + speed/10 units), stops,
  and runs at the planned speed.
- **Takeoff.** The jump is pressed when:
  - the bot is within 12 units before or after the takeoff point and 16 units of the line;
  - it has 90% of the planned speed (and not over 115%);
  - it is running along the line (sideways speed under a quarter of the speed).
- **Short run-up.** With under 16 units of room behind the takeoff, the bot gathers speed in the window itself. The
  window then reaches 8 units further, and 75% of the speed will do.
- **Validation.** The validator tries speeds of 0, 100, 150, 200 and 270 units/s, without and with ducking. Each try
  is simulated from rest at the start of the run-up, following those rules. The plan must also survive taking off at
  90% and 110% of the speed, 8 units early or late, and 3° off the line. The simplest plan that is robust enough is
  taken.
- **Landing.** A landing short of or past the node, on its floor and closer than the takeoff, is finished on foot. A
  landing back at the takeoff is retried, up to three tries in all.

### Long jumps and gauss boosts

`lb_kin::tricks` simulates both the way the executors make them.

- **Long jump.** With the module, duck and jump pressed together on the ground while moving faster than 50 units/s
  set the horizontal speed to 560 along the view and throw the player 56 units up: about 420 units over flat ground.
  The air takes speed away fast but gives only 30 units/s, so the bot holds duck and steers onto its landing
  (`air_steer`): a long jump comes down anywhere short of its full reach, never beyond it. A link's check runs up 16
  units from rest and takes off at the entry; the takeoff 12 units earlier or later and 8 units off the line must land
  too (near the reach a takeoff a little early falls short). The executor keeps to that: it stops at the start of the
  run-up behind the takeoff (up to 32 units back, only as far as there is floor) while its view turns level onto the
  landing, and takes off within 12 units of the entry, on its floor, with the view within 5° of the landing. The motor
  makes the press: both keys afresh in one command, a command with neither going out first when either is held.
- **Gauss boost.** A charged gauss shot pushes its shooter back at five times its damage, up and down too in
  multiplayer. The bot looks back and down (so the beam neither glances off the floor behind nor, too thick to punch
  through, comes back at it), jumps, and the charge goes on the next command: a full charge adds some 850 units/s
  forward and 550 up looking 34° down. The recoil grows over the 1.5 s of a full charge and the game lets a charge go
  after half a second at the soonest, so a boost is charged for the push it takes, a third of a full one or more: a
  lower arc under ceilings, a shorter fall, an unsteered flight that ends near the landing. The check tries pitches
  from 30° to 70°, each with the least push that throws the bot past the landing in the open (32 units and 8% of the
  way further: steering in the air only brakes), the gentlest first. The steered flight must come down within 64
  units of the landing; 3° off to either side, 2° up or down, 8 units along and the push 4% off must too; and the
  unsteered flight must come down safely (a fight may take the bot's mind off the steering). The push is part of the
  link's contract; the executor charges for it (`boost_charge`).
- **On the way.** Two tricks are taken off the graph's links, as shortcuts of the follower:
  - a long jump along the path, onto the node furthest along it that the flight, followed through the server's
    traces, comes down on, out of lava and slime, where the path goes on walking (not onto the start of a jump, a
    ladder or another link with an executor). The bot runs within 21° of the jump (bold: 30°) at 150 units/s at least
    (bold: 100). Not bold: along a straight, level stretch of the path at least 400 units long (yapb's runway), the
    nodes it passes over within 24 units of its line, onto a node 300–470 units ahead, no more than 64 below or 40
    above, without fall damage; looked for every half second, 1.1 s apart at least. Bold (the brain's
    `runway_bold`, skills from hard up): from 250 units off as far as the jump carries onto that floor
    (`longjump_reach`: some 450 units on the level, 600 off a 120-unit drop), round corners onto the path past them
    when the line is clear, over walks, drops and jumps of the path down to 400 units below, onto a landing that
    hurts no more than the brain allows (`runway_hurt`); looked for every 0.05 s and taken one after another. Before
    a flight is followed three crouched-hull sweeps under its arc, 8 units to either side, make sure a takeoff a
    little off the line still gets through, and a landing that did not come down right is not tried again from
    within 48 units. The bot takes off within 32 units of where the check stood and 10 of its line, the view within
    10° of the landing (the air kills the speed across in a few hundredths of a second), or not at all; in the air it
    looks along the way on from the landing, lined up for the next one. At most two flights are followed per look,
    and all the bots together follow 300 a second at most, two in one frame (`NavCtx::flights`; some dozens of
    microseconds each on the stand's cores, a few hundred at worst);
  - a gauss boost the brain asks for (`NavService::gauss_leap`): along the next few nodes and toward the goal the
    unsteered flights are followed, and the node along a flight's line (from 40% of its reach on) the planner reckons
    most seconds nearer the goal, two at least, is steered for. Off the path, the way on is planned again after the
    landing.
- **In the air** a trick's flight is flown to its end: the path is not replaced until the bot lands, and when the
  brain does something else meanwhile (a fight) it still gets the steering (`NavService::flight`). The movement keys
  alone steer, so the flight's look yields to any other (`NavStep::free_look`): a bot in the air shoots at an enemy
  in sight.

### Bunny hops

A bot that may bunny hop (from hard up, see `docs/behavior.md`) hops along straight stretches of walking links, on
every server (`lb_nav::hop`):
- **The jump** goes on the first command back on the ground: the game jumps before it brakes, so that command loses
  nothing to friction. The bot presses it only then; a jump held in the air would not go off on landing (the game
  wants a fresh press).
- **In the air** every command presses nearly square to the flight, on the side of the way (`lb_kin::hop::air_strafe`):
  the air adds speed along a press until the flight's own speed that way reaches 30, so pressed square every command
  adds some, and the side swings over every command when the flight is on its way already. With `sv_airaccelerate 10`
  and 10 ms commands (the air's gain grows with the command rate; the bots press for the commands they send) a run of
  270 grows to some 365 over the first hop, 440 over the second. At the speed kept to the presses only turn the
  flight; above it they brake (the air takes off up to 27 units/s a command). The movement is projected on the view,
  so the bot looks where it likes meanwhile. A server that crops a jump faster than 1.7 × maxspeed is hopped 3% under
  that.
- **Where.** From the node walked to on, along walking links (not crouched, not depending on a mover) up to the first
  of: a link walking does not do, a node crouched under, on a ladder, in water, mid-air or on a mover, a turn of the
  path sharper than 60°, a climb or a descent steeper than about 8° (stairs and ramps: a hop down beside narrow
  stairs would not get onto them again), the end of the way. There the bot runs on and hops again past it.
- **The check.** A hop is taken from a run (90% of maxspeed at least) with 46 units clear overhead, once its flight,
  steered the way the bot will steer it, is followed through the server's traces from the takeoff: it must come down
  on a floor of the way (within 40 units of its line across, 24 up or down), not in lava or slime, without fall damage
  and without bumping into a wall or a ceiling, where the way on to the next node is walked in a straight line, and
  short of where the hops stop by the run friction takes to bring the speed down to what is wanted there (a quarter of
  the speed to shed, plus 48 units: the entry speed of the link that starts there, a run at a corner or on stairs, 90
  at the end of the way). The checks share the long jumps' budget of followed flights; one that did not check out is
  not looked at again from within 48 units for 0.2 s on the same link.
- **The flight** is steered at the point of the way a quarter of a second ahead; the bot passes the path's nodes as
  it flies by them, never past where the hops stop, and nothing replans the way or ends it until the bot lands
  (`NavService::flight`, as for long jumps). A hop that comes down off the way (64 units across, 40 up or down, or
  where the way on is not walked) has the way planned again from there.

Followed through the movement code (`lb-testkit/tests/bhop.rs`): an expert on a server that crops crosses a straight
3072-unit corridor in 7.4 s where a run takes 11.3 (hard 7.9 s, an expert where the server does not crop 6.8 s), every
takeoff on the first command back and none cropped; an L of two 1536-unit legs in 7.8 s against 11.3. On the stand's
maps (an expert where the server crops, 40 routes each, 1000 frames a second) the routes took 84–93% of the time
walked (dm_snow 84%, bounce 90%, datacore 91%, stalkyard 92%, crossfire 93%), none of some 980 hops down off the way,
and links failing about as often as walked (69 against 66, nearly all jump links whose executor misses either way). On the live Xash stand the hops go off on the first command back and
gain as followed offline (270, 351, 446 at the takeoffs).

## When a link fails

A failed link is blocked for the bot that failed it, for a time that depends on why:

| Reason                  | Meaning                                               | Blocked for                  |
|-------------------------|-------------------------------------------------------|------------------------------|
| `TemporarilyOccupied`   | a player is in the way                                | 4 s, +2 s per repeat, max 20 |
| `WaitingForInteraction` | a door, lift or button did not do its part            | 15 s                         |
| `MissingCapability`     | the bot lacks what the link needs (health for a fall) | 30 s                         |
| `ControllerFailure`     | the bot could not carry the move out                  | 10 s, doubling, max 120      |
| `GeometryInvalid`       | the map does not let the move through                 | 120 s                        |

Only a bot's own failures block links for it: a door closed out of its sight does not change its plans. When three
bots fail the same link for geometry within 600 s, the link is switched off for everyone until the next map. The
planner then goes around.

A walking bot that stops making progress tries these in order:
- a side step;
- backing off;
- a jump if a step is ahead, or a crouch if the ceiling is low;
- declaring the link failed, with the reason from what blocks it (a player, the world, a mover).

A bot stuck in one place for `bots.stuck_kill_time` (20 s) uses the `kill` command. It does so only when it has
seen or heard no enemy for 5 s.

Executors only see the map's mechanisms (where a door or lift is now) to carry out their own link. The planner never
does: bots learn about doors only by running into them.

## Obstacle courses

**Offline** (`cargo test -p lb-testkit`, with `LB_MAPS_DIR` pointing at the maps). A simulated bot runs every
special link of crossfire's graph with the engine's movement. A link counts only when the bot arrives with no link
failed on the way: the planner goes around a failed link, so arriving alone proves nothing.

| Course                                                  | Result                                                                  |
|---------------------------------------------------------|-------------------------------------------------------------------------|
| lifts                                                   | 26/26                                                                   |
| jumps from rest, 100 fps and 1000 fps                   | 156/156, 156/156                                                        |
| jumps entered running (from the node before), 1000 fps  | 155/156 (the one miss: stuck on the way to it)                          |
| drops from rest / entered running                       | 134/135 / 126/127                                                       |
| ladder routes, floor → ladder → floor, up and down      | 11/12                                                                   |
| walks (sample)                                          | 149/149                                                                 |
| a sample at 100, 500, 1000 fps, and with a 300 ms frame | 23/23 each, no button left pressed                                      |
| 60 random routes across the map                         | 59 arrive, 1 around a failed link; time / plan: median 1.03, worst 1.57 |

`tests/obstacles.rs` builds small worlds for what crossfire lacks. Carried out:
- a touch door;
- a use door;
- a door opened by a remote button;
- a platform;
- a teleport;
- a breakable;
- swimming across a pool and climbing out;
- a long jump across a gap with the module, and the way round without it;
- a gauss boost onto a ledge (the course plays the weapons' part: charging, turning, the jump, the recoil on the
  command after leaving the ground), and none without the gun;
- long jumps along a straight run, and how much time they save;
- bold long jumps: one after another along a straight run, across a winding way with no straight stretch, down off
  a ledge (over the drop, and off a high one only with the fall damage allowed), onto the goal 320 units off;
- a gauss boost toward a far goal over a wall, landing off the path, and the way on planned after.

Failures tested (the other traversals have none yet):
- a use door that never opens, reported as `WaitingForInteraction` and walked around;
- a jump too far, reported as `ControllerFailure` and walked around;
- a walled-up passage, reported as `GeometryInvalid`.

**Long jumps along the way** (`cargo test --release -p lb-testkit --test longjumps -- --ignored --nocapture`,
`LB_MAPS=dm_snow,crossfire LB_ROUTES=40`, `LB_LOG=1` for the misses and failures). Routes between random places at
least 1000 units apart run by a bot without the module, with long jumps and with bold ones: arrivals, time, long jumps
a minute, time lining up and in the air, health lost. `debug_route_long_jumps` (`LB_MAP`, `LB_FROM`, `LB_TO`) prints
one route frame by frame while long jumping.

**Generated graphs** (`cargo test -p lb-testkit --test generated_course -- --ignored`). On every standard map, a
sample of each kind of special link (up to 40) is carried out from its entry (the bot with the long jump module and a
gauss), and a bot walks from the nearest spawn point to every item the coverage report counts as reachable. Results
are in `docs/m3-acceptance.md` and, for the tricks, `docs/m4-acceptance.md`.

**To a spot** (`lb do`, `lb test`, `lb-cli nav try`). A bot given items goes to a spot a command or the map's tests
name, finding a jump, a long jump or a gauss boost onto it where the graph has no way: `docs/testing.md`.

**Live** (`lb nav test`). A bot is taken off its behavior and runs chosen special links on the running server:

| Command                                 | Action                                               |
|-----------------------------------------|------------------------------------------------------|
| `lb nav test <kind\|all> [count] [bot]` | `count` links of a kind, spread over the map         |
| `lb nav test link <from> <to> ...`      | exactly these links                                  |
| `lb nav test`                           | results: per kind, and every failure with its phases |
| `lb nav test stop`                      | back to normal behavior                              |

The bot walks to each link's entry, then carries the link out. A link whose entry it cannot reach (no path for 5 s,
or 30 s of walking) counts as not reached, not as failed. Before each link the bot is given two health kits and two
uranium clips (`give`, with `sv_cheats 1`): a run of boosts or falls tests the links, not what the bot has left. A
gauss boost's weapons part is played by the brain's `GaussBoost` protocol. Each result is logged as `nav test`.
Results are in `docs/m2-acceptance.md` (the imported graph), `docs/m3-acceptance.md` (the generated one) and
`docs/m4-acceptance.md` (the tricks).

## Known limits

- A door opened by a remote button is carried out by walking straight to the button and back. Where the way to the
  button is not straight (snark_pit's hatches), the link fails and bots learn to go around it.
- Trains (`func_train`, `func_tracktrain`), conveyors and `multisource` gates are not traversed; items only they reach
  are listed by the coverage report (see `docs/m3-acceptance.md`) for the map's overlay.
- The graph is published whole: until it is made (0.4 s on crossfire on the stand's cores, up to a minute and a half
  on a huge map) bots stand still. Publishing a walk-only graph first is not done yet.
- The visibility table between nodes (for tactics) is not made yet.
- Water is swum straight between nodes in it; a current is only crossed with it (push links).
