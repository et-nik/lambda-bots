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
   platforms, teleports, drop edges and landings, push-field entries and landings, a spot in reach of every button.
   Then nodes fill the rest of the floor, farther apart where the floor is open. Every span belongs to the node
   nearest to it on foot.
3. **Walks.** Nodes whose floors touch are joined when a straight hull check passes; longer links are added only
   where the graph would otherwise make a bot go more than 15% out of its way (a greedy spanner, at most 12 links a
   node). A walk never passes through a push field, nor brushes past a ladder at a ledge's edge.
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
5. **Report.** The coverage report (`lb-cli nav coverage`) counts the floor and the items a bot gets to from the
   spawn points and back, and lists the rest.

On crossfire this takes 1.5 s on the stand's cores (2096 nodes, 13012 links); on the largest map, boot_camp, 5.4 s.
A kept graph loads in under a millisecond (96 ms on the stand, with the BSP and the visibility sets). The file
(`.lbnav`: postcard, LZ4, CRC-32C) is named by the key of what the graph was made from: the BSP (BLAKE3 and size),
the generator's version, the physics, the rules and the overlay. Only a graph with exactly the same key is used; the
four most recently used are kept. `lb nav regen` throws a map's graphs away and makes it again.

After the graph is loaded, the map's overlay patches are applied (see `docs/overlays.md`) and landmarks for the
planner are computed.

| Kind        | Classified as                                                                     | Contract                          |
|-------------|-----------------------------------------------------------------------------------|-----------------------------------|
| `walk`      | a straight hull check passes, or a simulated run slides around what is in the way | —                                 |
| `crouch`    | the same, crouched                                                                | —                                 |
| `drop`      | a simulated walk off the edge lands at the node                                   | speed, fall damage, health needed |
| `jump`      | a simulated running jump (see below) lands at the node                            | speed, duck, robustness           |
| `ladder`    | either end is on a ladder (and it is not a walk along a floor)                    | ladder normal, mount point        |
| `swim`      | either end is in the water: a simulated swim gets there                           | —                                 |
| `door`      | a door blocks the way; the map's mechanism graph says how it opens                | touch, use, or a remote button    |
| `lift`      | added from the map: platforms and doors that carry a player up                    | the mover, where to call it       |
| `teleport`  | a `trigger_teleport` stands between the nodes                                     | the trigger, the destination      |
| `breakable` | a `func_breakable` blocks the way                                                 | the brush to shoot                |
| `push`      | a simulated run into a push field, steered in the air, lands at the node          | the run, jump, holding still      |

Every special link is also checked on the live server once the map has loaded. The check fires a few traces per link
(the floor at both ends, the way between), at most 64 per frame, and compares them with the offline world. A link
the server disagrees with is switched off. On crossfire, all 1507 special links of the generated graph are confirmed.
`lb nav` shows the result.

## Contracts

A special link carries a `TraversalSpec`:

- **Entry and exit anchors:** where the traversal starts and ends, with a radius and a stance.
- **Action:** what to do — jump at a speed, possibly ducking; drop; climb; open a door; ride a lift; step into a
  teleport; break a brush.
- **Needs:** for a drop, the health the fall costs plus a reserve.
- **Deadline:** the time the whole traversal may take.
- **Cost:** the time and damage the planner charges for the link.

The planner runs A* on travel time with these costs. Links the bot failed recently are left out (see below). Its
heuristic is the larger of straight-line distance at running speed (off on maps with teleports) and the ALT bound:
exact costs from and to 8–16 landmarks spread over the graph, which never overestimate. On crossfire that cuts a
search from 473 expanded nodes to 88 on average, on boot_camp from 1196 to 204. Searches run in slices: all bots
together expand at most 2000 nodes a frame and 200 000 a second; a search that runs out goes on in the next frame
while the bot keeps to its old path.

## Carrying a link out

The path follower walks plain links itself. For a special link it hands the bot to an executor. Every frame the
executor either returns a step (movement, look, pitch, buttons) with `Running`, returns `Waiting` (a mechanism is on
its way, which is not being stuck), or ends with `Done` or `Failed(reason)`. The follower enforces the deadline.

Before a special link the follower slows the bot to the speed the link takes: at most the jump's speed, 200 for a
drop, 150 for a ladder. Otherwise a bot running in at full speed passes a takeoff or runs off an edge before the
executor can act.

| Executor  | Phases                                   | How                                                                                                                                                                                                                                                          |
|-----------|------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| jump      | approach, run-up, takeoff, air           | backs up to the start of the run-up and stops there, runs at the planned speed, presses jump in the takeoff window, ducks in the air if planned                                                                                                              |
| drop      | edge, fall                               | walks off at the drop's speed, heading 48 units past the landing (slowing down at it stops on the ledge above it); fails when the landing is not the node's floor                                                                                            |
| ladder    | board, climb                             | from below: walks to the mount point and into the ladder facing it; from above: steps back over the edge facing it; climbs by pitch and forward or back; steps off at the top, onto a ledge behind or beside the ladder when the climb stops against its top |
| push      | approach, run, flight, landed            | stops at the entry, runs along the checked line (jumping where checked), keeps to the field's middle while it lifts, then steers the flight at the landing; walks or swims the rest                                                                          |
| door      | check, go-activate, activate, wait, pass | a touch door is walked into; a use door is pressed with the use key; a remote door's button is pressed; waits for it to open, then passes                                                                                                                    |
| lift      | wait, board, start, ride, exit           | waits for the platform to rest on its side, boards, starts it (standing on it or pressing its button), rides, steps off at the top                                                                                                                           |
| teleport  | —                                        | walks into the trigger; done when the bot finds itself at the destination                                                                                                                                                                                    |
| breakable | —                                        | shoots the brush (crowbar close up) until it is gone, then walks through                                                                                                                                                                                     |

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
- swimming across a pool and climbing out.

Failures tested (the other traversals have none yet):
- a use door that never opens, reported as `WaitingForInteraction` and walked around;
- a jump too far, reported as `ControllerFailure` and walked around;
- a walled-up passage, reported as `GeometryInvalid`.

**Generated graphs** (`cargo test -p lb-testkit --test generated_course -- --ignored`). On every standard map, a
sample of each kind of special link (up to 40) is carried out from its entry, and a bot walks from the nearest spawn
point to every item the coverage report counts as reachable. Results are in `docs/m3-acceptance.md`.

**Live** (`lb nav test`). A bot is taken off its behavior and runs chosen special links on the running server:

| Command                                 | Action                                               |
|-----------------------------------------|------------------------------------------------------|
| `lb nav test <kind\|all> [count] [bot]` | `count` links of a kind, spread over the map         |
| `lb nav test link <from> <to> ...`      | exactly these links                                  |
| `lb nav test`                           | results: per kind, and every failure with its phases |
| `lb nav test stop`                      | back to normal behavior                              |

The bot walks to each link's entry, then carries the link out. A link whose entry it cannot reach (no path for 5 s,
or 30 s of walking) counts as not reached, not as failed. Each result is logged as `nav test`. Results are in
`docs/m2-acceptance.md` (the imported graph) and `docs/m3-acceptance.md` (the generated one).

## Known limits

- A door opened by a remote button is carried out by walking straight to the button and back. Where the way to the
  button is not straight (snark_pit's hatches), the link fails and bots learn to go around it.
- Trains (`func_train`, `func_tracktrain`), conveyors and `multisource` gates are not traversed; items only they reach
  are listed by the coverage report (see `docs/m3-acceptance.md`) for the map's overlay.
- The graph is published whole: until it is made (1.5 s on crossfire on the stand's cores, up to a minute and a half
  on a huge map) bots stand still. Publishing a walk-only graph first is not done yet.
- The visibility table between nodes (for tactics) is not made yet.
- Water is swum straight between nodes in it; a current is only crossed with it (push links).
