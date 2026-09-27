# Navigation

How bots get around: the graph they plan on, the contract each special link carries, how a bot carries a link out,
and what happens when it cannot. The design behind it is in `docs/design/nav.md`.

## The graph

The graph is imported from the map's yapb `.graph` (`addons/lambdabots/nav/<map>.graph` or
`addons/yapb/data/graph/<map>.graph`). Its nodes are projected onto the floor; its links are thrown away and
classified again by simulation. `lb-kin` is a port of the engine's player movement (`PM_PlayerMove`): friction,
acceleration, stepping, gravity, jumping, ducking, ladders and water.

| Kind        | Classified as                                                                           | Contract                          |
|-------------|-----------------------------------------------------------------------------------------|-----------------------------------|
| `walk`      | a straight hull check passes, or a simulated run slides around what is in the way       | —                                 |
| `crouch`    | the same, crouched                                                                      | —                                 |
| `drop`      | walking there falls more than 20 units: a simulated walk off the edge lands at the node | speed, fall damage, health needed |
| `jump`      | a simulated running jump (see below) lands at the node                                  | speed, duck, robustness           |
| `ladder`    | either end is on a ladder (and it is not a walk along a floor)                          | ladder normal, mount point        |
| `swim`      | either end is under water                                                               | —                                 |
| `door`      | a door blocks the way; the map's mechanism graph says how it opens                      | touch, use, or a remote button    |
| `lift`      | added from the map: platforms and doors that carry a player up                          | the mover, where to call it       |
| `teleport`  | a `trigger_teleport` stands between the nodes                                           | the trigger, the destination      |
| `breakable` | a `func_breakable` blocks the way                                                       | the brush to shoot                |

yapb's own link flags are only hints. A link yapb marks as a jump that a bot can simply walk (around a corner) is a
walk. A link that fits no kind is kept but not planned through. On crossfire, 32 of 7888 links are rejected (walls,
32-unit steps); 26 lift links are added.

Every special link is also checked on the live server once the map has loaded. The check fires a few traces per link
(the floor at both ends, the way between), at most 64 per frame, and compares them with the offline world. A link
the server disagrees with is switched off. On crossfire, all 334 special links are confirmed. `lb nav` shows the
result.

## Contracts

A special link carries a `TraversalSpec`:

- **Entry and exit anchors:** where the traversal starts and ends, with a radius and a stance.
- **Action:** what to do — jump at a speed, possibly ducking; drop; climb; open a door; ride a lift; step into a
  teleport; break a brush.
- **Needs:** for a drop, the health the fall costs plus a reserve.
- **Deadline:** the time the whole traversal may take.
- **Cost:** the time and damage the planner charges for the link.

The planner runs A* on travel time with these costs. Links the bot failed recently are left out (see below).

## Carrying a link out

The path follower walks plain links itself. For a special link it hands the bot to an executor. Every frame the
executor either returns a step (movement, look, pitch, buttons) with `Running`, returns `Waiting` (a mechanism is on
its way, which is not being stuck), or ends with `Done` or `Failed(reason)`. The follower enforces the deadline.

Before a special link the follower slows the bot to the speed the link takes: at most the jump's speed, 200 for a
drop, 150 for a ladder. Otherwise a bot running in at full speed passes a takeoff or runs off an edge before the
executor can act.

| Executor  | Phases                                   | How                                                                                                                                                                           |
|-----------|------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| jump      | approach, run-up, takeoff, air           | backs up to the start of the run-up and stops there, runs at the planned speed, presses jump in the takeoff window, ducks in the air if planned                               |
| drop      | edge, fall                               | walks off at the drop's speed; fails when the landing is not the node's floor                                                                                                 |
| ladder    | board, climb                             | from below: walks to the mount point and into the ladder facing it; from above: steps back over the edge facing it; climbs by pitch and forward or back; steps off at the top |
| door      | check, go-activate, activate, wait, pass | a touch door is walked into; a use door is pressed with the use key; a remote door's button is pressed; waits for it to open, then passes                                     |
| lift      | wait, board, start, ride, exit           | waits for the platform to rest on its side, boards, starts it (standing on it or pressing its button), rides, steps off at the top                                            |
| teleport  | —                                        | walks into the trigger; done when the bot finds itself at the destination                                                                                                     |
| breakable | —                                        | shoots the brush (crowbar close up) until it is gone, then walks through                                                                                                      |

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

`tests/obstacles.rs` builds small worlds for what crossfire lacks. Each traversal is tested for success and for
failure:
- a touch door;
- a use door;
- a door opened by a remote button;
- a platform;
- a teleport;
- a breakable;
- swimming across a pool and climbing out;
- a door that never opens, reported as `WaitingForInteraction` and walked around;
- a jump too far, reported as `ControllerFailure`;
- a walled-up passage, reported as `GeometryInvalid`.

**Live** (`lb nav test`). A bot is taken off its behavior and runs chosen special links on the running server:

| Command                                 | Action                                               |
|-----------------------------------------|------------------------------------------------------|
| `lb nav test <kind\|all> [count] [bot]` | `count` links of a kind, spread over the map         |
| `lb nav test link <from> <to> ...`      | exactly these links                                  |
| `lb nav test`                           | results: per kind, and every failure with its phases |
| `lb nav test stop`                      | back to normal behavior                              |

The bot walks to each link's entry, then carries the link out. A link whose entry it cannot reach (no path for 5 s,
or 30 s of walking) counts as not reached, not as failed. Each result is logged as `nav test`. Results are in `docs/m2-acceptance.md`.

## Known limits

- Only yapb graphs: a map without one leaves bots standing (they still see and shoot). The generator is M3.
- One drop on crossfire (934 → 1252, off a ramp's side) passes the simulation but not the executor.
- yapb places some nodes on the very edge of a ledge. A bot walking along it (crossfire's ladder shaft, 5 → 391)
  can slip off.
- Water is swum straight across; there are no swim links through tunnels yet.
- Trains, conveyors, push triggers and `multisource` gates are not traversed (M3, with the map annotations).
