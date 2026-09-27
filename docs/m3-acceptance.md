# M3 acceptance: the graph made from the map

State as of 2026-09-27. Offline checks: the twelve standard HLDM maps, release build, Apple M-series (8 cores). Test
stand: Xash3D FWGS 0.21 (arm64) + Metamod-FWGS + hlsdk-portable, macOS, crossfire at 1000 fps. The ReHLDS server
has not run M3 yet. No BSP of the GunGame server's maps has been received yet.

## Results against the plan's criteria

| Criterion                                                    | Status  | How it was checked                                                                  |
|--------------------------------------------------------------|---------|-------------------------------------------------------------------------------------|
| Graphs of all standard HLDM maps are made in the target time | yes     | `crates/lb-navgen/tests/maps.rs`: 0.3–1.5 s a map, 5.4 s on boot_camp (target 15 s) |
| Floor covered from the spawn points and back ≥ 95%           | yes     | the coverage report, every map 97.0–100%                                            |
| Spawn ↔ item routes checked                                  | partly  | offline course: 756 of 823 routes arrive (91.9%); unreachable items listed below    |
| The cache is invalidated correctly                           | yes     | unit tests (every part of the key, LRU, damaged files), `changelevel` on the stand  |
| GG server maps                                               | not yet | no BSPs yet                                                                         |

## Making graphs

| Map           | Time, ms | Nodes | Links | Floor covered | Items (reachable/all) | Special links                                                   |
|---------------|----------|-------|-------|---------------|-----------------------|-----------------------------------------------------------------|
| boot_camp     | 5372     | 6540  | 39486 | 99.5%         | 148/160               | jump 1759, drop 2239, ladder 54, swim 109, breakable 7          |
| bounce        | 463      | 1722  | 10920 | 99.5%         | 64/64                 | jump 446, drop 527, ladder 62, swim 2300, door 2, push 57       |
| crossfire     | 1532     | 2096  | 13012 | 100.0%        | 121/121               | jump 934, drop 537, ladder 36, door 2, lift 80                  |
| datacore      | 621      | 1268  | 8013  | 98.2%         | 57/62                 | jump 808, drop 164, ladder 26                                   |
| frenzy        | 564      | 1298  | 8294  | 97.0%         | 40/43                 | jump 274, drop 545, ladder 12, door 6                           |
| gasworks      | 990      | 2820  | 19461 | 100.0%        | 73/73                 | jump 1570, drop 459, ladder 292, swim 4815, door 12, teleport 3 |
| lambda_bunker | 657      | 1519  | 10781 | 98.8%         | 44/44                 | jump 917, drop 198, ladder 48, swim 2766, breakable 7, push 7   |
| rapidcore     | 673      | 1078  | 6851  | 97.6%         | 58/64                 | jump 284, drop 104, ladder 22, swim 348, door 16                |
| snark_pit     | 348      | 945   | 6046  | 98.4%         | 45/62                 | jump 239, drop 199, ladder 12, swim 681, door 9                 |
| stalkyard     | 563      | 1157  | 7212  | 97.4%         | 68/73                 | jump 573, drop 522, ladder 6, door 6, lift 16, breakable 4      |
| subtransit    | 987      | 2001  | 13723 | 97.1%         | 64/66                 | jump 1649, drop 518, ladder 14, swim 1947, door 1, lift 8       |
| undertow      | 873      | 1930  | 12122 | 99.3%         | 41/42                 | jump 394, drop 847, ladder 84, swim 780, door 2, lift 15        |

Items count item nodes (items close together share one); floor covered counts the 16-unit spans of floor whose node
a bot gets to from a spawn point and back.

On crossfire the generator spends 0.7 s on walks (the spanner) and 0.7 s on jumps; the rest takes 60 ms. A kept
graph (`.lbnav`, 172 KB for crossfire) loads in under a millisecond; on the stand's dev build the whole map load from
the cache takes 96–115 ms, against 15 s for making the graph in that build.

## Items the graph does not get to

`lb-cli nav coverage <map.bsp>` lists them with what is missing (`in`: nothing leads there; `out`: nothing leads
back; `no floor`: the item floats or rests on an entity). What they have in common:

| Map        | Items                        | Why                                                                             |
|------------|------------------------------|---------------------------------------------------------------------------------|
| boot_camp  | 3 gauss clips, battery       | inside water tanks closed on every side                                         |
| boot_camp  | 4 satchels                   | on a stand 45–50 units up: the jump checks fail                                 |
| boot_camp  | egon, battery, 2 health kits | a room next to a corridor on the same level, with no way in the generator finds |
| datacore   | rpg, 3 rpg clips             | shelves 120–220 units up                                                        |
| datacore   | tripmine, 357, rpg clip      | no floor under them (they rest on entities)                                     |
| datacore   | snarks                       | in a pit with no way in or out                                                  |
| frenzy     | long jump                    | on the central pillar, behind a railing                                         |
| rapidcore  | gauss, rpg, 3 batteries      | 78–412 units up                                                                 |
| snark_pit  | 4 satchels, 4 grenades       | the secret room: its button is inside it (`secret_gate`)                        |
| snark_pit  | 4 gauss clips, 4 batteries   | a pocket beside the fan's vent at the top of the shaft                          |
| snark_pit  | rpg                          | behind the vent of `fan1`, a push field switched off at the start               |
| stalkyard  | gauss, 3 batteries           | 128–192 units up                                                                |
| stalkyard  | long jump                    | its room is left through door `*6`, which the generator finds no opener for     |
| stalkyard  | hand grenade                 | no floor under it                                                               |
| subtransit | 2 batteries                  | a flooded pit 376 units down, behind `func_rot_button *16`                      |
| subtransit | health kit                   | no floor under it                                                               |
| undertow   | rpg                          | in the water channel, against its current                                       |

These are cases for the map's overlay (`docs/overlays.md`): an `add_link` where a route exists that the generator
misses, nothing where the item is a secret or needs a mechanism bots do not operate yet (trains).

## Carrying the links out

**Offline** (`cargo test -p lb-testkit --test generated_course -- --ignored`): on every map, a sample of up to 40
links of each special kind carried out from its entry, and a walk from the nearest spawn point to every item the
report counts as reachable.

| Kind     | Carried out | Kind      | Carried out |
|----------|-------------|-----------|-------------|
| ladder   | 120/120     | lift      | 63/66       |
| push     | 36/36       | breakable | 15/18       |
| teleport | 3/3         | jump      | 446/471     |
| swim     | 308/315     | door      | 34/56       |
| drop     | 447/460     | routes    | 756/823     |

Routes by map: boot_camp 141/148, bounce 61/64, crossfire 109/121, datacore 57/57, frenzy 35/40, gasworks 66/73,
lambda_bunker 42/44, rapidcore 57/58, snark_pit 42/45, stalkyard 62/68, subtransit 52/64, undertow 32/41.

What most failures come down to:
- **Doors opened by a remote button.** The executor walks straight to the button and back; where the way is not
  straight (snark_pit's hatches, crossfire's secret door), the link fails, and bots learn to go around it. crossfire's
  12 failed routes are all to the secret room behind such a door.
- **Jumps and walks near odd geometry** (a railing at the edge of a ledge, a doorway frame): the check passes, the bot
  fails, marks the link, and plans around it.
- **Items reached only through a failing link:** such a route cannot arrive within the test's 120 s.

**Live** (stand, crossfire, one bot, `lb nav test all 60`). The generated graph is made in the server, all 1507
special links pass the live check (0 disagree), and `changelevel` loads the graph from the cache.

| Kind   | Carried out (of those reached) | Not reached (the way to the entry failed) |
|--------|--------------------------------|-------------------------------------------|
| jump   | 23 of 32                       | 5                                         |
| drop   | 15 of 18                       | 2                                         |
| lift   | 2 of 2                         | —                                         |
| ladder | 1 of 1                         | —                                         |

41 of the 53 links reached were carried out (77%); the core took p50 6.7 µs, p99 47.8 µs a frame, with no faults.
This is below the offline course (95% of jumps, 97% of drops), and the failing links pass offline:
- 2 jumps start on the floor plates of crossfire's bunker door, which the server keeps in another place than the
  map's rest position; such jumps are now marked as depending on a mover;
- 7 jumps take off but do not land at the node (4 of them onto ledges the floor flood found, nodes 17xx); offline
  the same links land — to be looked into with a recording (`lb record`) of those links;
- 2 drops time out on the edge (the bot does not go over it), 1 was refused for lack of health (a correct refusal).

## Planning

| Map       | Landmarks | Nodes expanded per search: straight-line bound | with landmarks |
|-----------|-----------|------------------------------------------------|----------------|
| crossfire | 8         | 473                                            | 88             |
| boot_camp | 16        | 1196                                           | 204            |

300 random pairs a map; the paths are the same cost either way (`crates/lb-navgen/tests/maps.rs`). All bots together
expand at most 2000 nodes a frame and 200 000 a second; a search that runs out continues in the next frame.

## Tools

- `lb-cli nav gen <map.bsp> [--out file.lbnav]`: makes and writes a graph, with times per stage and coverage.
- `lb-cli nav coverage <map.bsp> [--json]`: the coverage report with the unreachable items.
- `lb-cli nav path <map.bsp> <x,y,z> <x,y,z>`: a path with the kind of every link.
- `lb-cli nav validate-overlay <map.bsp> <overlay.yaml>...`: applies overlays and reports what does not apply.
- In the game: `lb nav regen`, `lb overlay [reload]`, `lb edit ...` (`docs/overlays.md`), `lb_nav_source`, `lb_editor`.

## Not done yet

- Doors opened from a remote button whose way is not straight: the executor needs to walk the graph to the button.
- Publishing a walk-only graph first, then the rest (staged publishing): the whole graph is published at once.
- The visibility table between nodes.
- The live failures above: short jumps onto ledges and drops that stop on the edge.
- The editor has not been driven by a player on the stand: it needs someone in the game.
- ReHLDS and the GG server's maps.
