# M2 acceptance: traversal contracts

State as of 2026-09-27. Test stand: Xash3D FWGS 0.21 (arm64) + Metamod-FWGS + hlsdk-portable, macOS, map crossfire
with its yapb graph, 1000 fps. The ReHLDS server has not run M2 yet (package `lambdabots-0.1.0-m2-linux-i386.tar.gz`).

## Results against the plan's criteria

| Criterion                                                               | Status  | How it was checked                                                    |
|-------------------------------------------------------------------------|---------|-----------------------------------------------------------------------|
| Obstacle set: every traversal has a tested success and a tested failure | partial | offline courses on crossfire and synthetic worlds; live `lb nav test` |
| A long frame leaves no button pressed                                   | yes     | offline course with a 300 ms frame at 100 and 1000 fps                |
| Replay reproduces decisions                                             | yes     | stand recordings replayed by `lb-cli replay`: 0 differing commands    |

## Traversals

Details of the contracts, executors and failure handling: `docs/navigation.md`.

**Offline** (simulated bot, the engine's movement, every special link of crossfire; a link counts only if no link
failed on the way):

| Course                                               | Result                                                                  |
|------------------------------------------------------|-------------------------------------------------------------------------|
| lifts                                                | 26/26                                                                   |
| jumps from rest, 100 fps / 1000 fps                  | 156/156 / 156/156                                                       |
| jumps entered running, 1000 fps                      | 155/156 (the miss: stuck on the way to it)                              |
| drops from rest / entered running                    | 134/135 / 126/127                                                       |
| ladder routes up and down                            | 11/12                                                                   |
| walks (sample)                                       | 149/149                                                                 |
| sample at 100, 500, 1000 fps and with a 300 ms frame | 23/23 each; no button left pressed                                      |
| 60 random routes                                     | 59 arrive (1 around a failed link); time / plan median 1.03, worst 1.57 |

Synthetic worlds (`crates/lb-testkit/tests/obstacles.rs`); a dash means no failure test:

| Traversal                 | Success                      | Failure tested                                     |
|---------------------------|------------------------------|----------------------------------------------------|
| touch door                | opens when walked into       | —                                                  |
| use door                  | opened with the use key      | a door that never opens: `WaitingForInteraction`   |
| door with a remote button | the button is pressed first  | —                                                  |
| platform                  | rides up when stood on       | —                                                  |
| teleport                  | walked into, arrival checked | —                                                  |
| breakable                 | shot out of the way          | —                                                  |
| swimming                  | across a pool and out        | —                                                  |
| jump                      | (crossfire)                  | a jump too far: `ControllerFailure`, walked around |
| walk                      | (crossfire)                  | a walled-up passage: `GeometryInvalid`             |
| drop                      | (crossfire)                  | not enough health: `MissingCapability` (live)      |

Lifts and ladders are tested for success only (crossfire).

**Live** (`lb nav test all 60`, one bot, 60 special links spread over the map):

| Kind   | Carried out | Not reached |
|--------|-------------|-------------|
| jump   | 22/22       | 12          |
| drop   | 15/16       | 2           |
| ladder | 3/3         | 1           |
| lift   | 4/4         | —           |

"Not reached" means the planner had no way to the entry: most of these entries are in the bunker, which the yapb
graph does not connect (below). Walking the rejected links into it fails on the live server too. The failed drop is 1048 → 1289: entered running, the bot lands short of the node.
In an earlier live run with four bots fighting, a drop was refused for low health (`MissingCapability`), as it
should be.

The live check confirms all 334 special links against the server's own traces.

## Recording and replay

Details: `docs/replay.md`.

| Recording (crossfire, 1000 fps)          | Frames  | Bot commands compared | Differences | Replay time |
|------------------------------------------|---------|-----------------------|-------------|-------------|
| 180 s, 4 bots, one running the course    | 179 716 | 68 551                | 0           | 1.6 s       |
| 60 s, 4 bots, with an injected bot fault | 59 751  | 20 207                | 0           | 0.6 s       |
| 60 s, 8 bots, final build                | 59 651  | 38 171                | 0           | 0.9 s       |

The first replays differed in the last bit of 2% of move commands. Fixed by `lb_core::dmath`: the plugin and
`lb-cli` computed sine and cosine through different libm paths.

## Weapon prediction

`get_weapon_data` in the host API reads what the game DLL would send the bot's client about its weapons: clip, next
attack and reload state. It reads through the original game DLL table, without Metamod hooks. `lb brain` shows it
(`client prediction: weapon_9mmhandgun ready, clip 9`), and `SelfState::ready` answers whether a weapon can fire
now. Combat does not use it yet: the gauss charge and reload decisions of M4 will.

## Stand soak: 8 bots, crossfire, 1000 fps, 5 minutes

| Measure                             | Value                                          |
|-------------------------------------|------------------------------------------------|
| Kills                               | 70 (about 13 per minute), every bot 3–14 frags |
| Faults, dropped events, stale moves | 0                                              |
| Core time per frame                 | p50 35 µs, p95 86 µs, p99 186 µs, avg 52 µs    |

The core takes more time per frame than in M1 (avg 19 µs). The profile spreads it over decisions (item scoring on
urgent re-decisions), vision, the executors, and the engine's own player moves for the bots. Mover state was read
every frame (avg 68 µs); it is now read 100 times a second, and the adapter interns engine strings by pointer.
Budgets are M6's work.

## What M2 contains

- **Movement model** (`lb-kin`): a port of `PM_PlayerMove`. It covers friction and edge friction, acceleration,
  stepping, gravity, jumping with a fresh press, longjump, ducking in the air, ladders, water, water jumps and fall
  damage. The validator simulates walks, drops and running jumps with it.
- **Graph import:** every yapb link is classified again by simulation. Lifts are added from the map's mechanisms.
  Sliding walks and floor links at ladder nodes are walks. Jumps are validated with the run-up a bot makes.
- **Contracts and executors:** jump, drop, ladder, swim, door (touch, use, remote button), lift, teleport,
  breakable. Each has phases, a deadline, and slows the bot before its entry.
- **Failures:** five reasons with their block times; links that fail for three bots are switched off; stuck recovery
  and `kill`; live checks of special links.
- **Obstacle courses:** offline (`lb-testkit`) and live (`lb nav test`, `lb nav test link`).
- **Recorder and replay:** `lb record`, `lb-cli replay`, deterministic math.
- **Weapon prediction channel.**

## Known limitations

- The yapb graph of crossfire leaves the bunker unconnected: its links into it pass through a wall. The live server
  agrees (`lb nav test link 1565 694` fails with `GeometryInvalid`). Bots cannot plan into 237 of 1598 nodes, and
  items there cost them re-decisions. The graph generator (M3) replaces imported graphs.
- One drop (1048 → 1289) fails when entered running; one drop (934 → 1252) fails always. On a narrow ledge along a
  ladder shaft (5 → 391) bots can slip off.
- Replays were checked on the recording machine only; a Linux recording replayed on macOS has not been tried.
- Trains, conveyors, push triggers and `multisource` gates are not traversed (M3, with map annotations).
