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
   - Tricks (see "Long jumps and gauss boosts" below):
     - long jumps across gaps: from nodes near an edge to nodes 160–560 units off (48 up to 240 down) that the graph
       joins only by a way more than twice as long, where the jump saves a second and a third of the way round and
       costs no fall damage;
     - gauss boosts: from every standing node with 128 units of room overhead, the flight along each of twelve ways,
       34° down, is followed to where it comes down, and the node there gets a boost when the graph's way to it is
       1.3 times as costly as the boost at least (or there is none).
5. **Report.** The coverage report (`lb-cli nav coverage`) counts the floor and the items a bot gets to from the
   spawn points and back, and lists the rest.

On crossfire this takes 0.46 s on the stand's cores (890 nodes, 4713 links, 3 of them long jumps and 68 gauss
boosts); on the largest map, boot_camp, 1.1 s.
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
| gauss_boost | approach, charge, air                    | stops at the takeoff; the weapons protocol draws the gauss, charges it, turns round and jumps letting it go; steers the flight onto the landing                                                                                                              |

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
  multiplayer. The bot looks back and 30–38° down (so the beam neither glances off the floor behind nor, too thick to
  punch through, comes back at it), jumps, and the charge goes on the next command: a full charge adds some 850
  units/s forward and 550 up. The check tries pitches 34°, 30° and 38°, steering onto the landing; 3° off to either
  side, 2° up or down and 8 units along must land too, and the unsteered flight must come down safely (a fight may take
  the bot's mind off the steering).
- **On the way.** Two tricks are taken off the graph's links, as shortcuts of the follower:
  - a long jump along a straight, level stretch of the path at least 400 units long (yapb's runway), onto the node
    300–470 units ahead, when the bot runs faster than 150 along it and the flight, followed through the server's
    traces, comes down there without fall damage; looked for every half second, 1.1 s apart at least;
  - a gauss boost the brain asks for (`NavService::gauss_leap`): along the next few nodes and toward the goal the
    unsteered flights are followed, and the node along a flight's line (from 40% of its reach on) the planner reckons
    most seconds nearer the goal, two at least, is steered for. Off the path, the way on is planned again after the
    landing.
- **In the air** a trick's flight is flown to its end: the path is not replaced until the bot lands, and when the
  brain does something else meanwhile (a fight) it still gets the steering (`NavService::flight`).

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
- a gauss boost toward a far goal over a wall, landing off the path, and the way on planned after.

Failures tested (the other traversals have none yet):
- a use door that never opens, reported as `WaitingForInteraction` and walked around;
- a jump too far, reported as `ControllerFailure` and walked around;
- a walled-up passage, reported as `GeometryInvalid`.

**Generated graphs** (`cargo test -p lb-testkit --test generated_course -- --ignored`). On every standard map, a
sample of each kind of special link (up to 40) is carried out from its entry (the bot with the long jump module and a
gauss), and a bot walks from the nearest spawn point to every item the coverage report counts as reachable. Results
are in `docs/m3-acceptance.md` and, for the tricks, `docs/m4-acceptance.md`.

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
