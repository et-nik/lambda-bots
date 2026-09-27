# M3 acceptance: the graph made from the map

State as of 2026-09-28, after the graph was made sparser and every walk straight (see "Sparser graph, straight
walks" below). Offline checks: the twelve standard HLDM maps, release build, Apple M-series (8 cores). Test stand:
Xash3D FWGS 0.21 (arm64) + Metamod-FWGS + hlsdk-portable, macOS, crossfire at 1000 fps. The ReHLDS server has not
run M3 yet. No BSP of the GunGame server's maps has been received yet.

## Results against the plan's criteria

| Criterion                                                    | Status  | How it was checked                                                                  |
|--------------------------------------------------------------|---------|-------------------------------------------------------------------------------------|
| Graphs of all standard HLDM maps are made in the target time | yes     | `crates/lb-navgen/tests/maps.rs`: 0.1–0.4 s a map, 1.0 s on boot_camp (target 15 s) |
| Floor covered from the spawn points and back ≥ 95%           | yes     | the coverage report, every map 96.8–99.9%                                           |
| Spawn ↔ item routes checked                                  | partly  | offline course: 777 of 809 routes arrive (96.0%); unreachable items listed below    |
| The cache is invalidated correctly                           | yes     | unit tests (every part of the key, LRU, damaged files), `changelevel` on the stand  |
| GG server maps                                               | not yet | no BSPs yet                                                                         |

## Making graphs

| Map           | Time, ms | Nodes | Links | Floor covered | Items (reachable/all) | Special links                                                   |
|---------------|----------|-------|-------|---------------|-----------------------|-----------------------------------------------------------------|
| boot_camp     | 974      | 2826  | 15008 | 99.6%         | 150/160               | jump 1448, drop 1611, ladder 54, swim 62, breakable 10          |
| bounce        | 202      | 782   | 4360  | 96.8%         | 58/64                 | jump 330, drop 336, ladder 62, swim 932, door 2, push 65        |
| crossfire     | 359      | 890   | 4642  | 98.5%         | 109/121               | jump 694, drop 390, ladder 36, door 1, lift 62                  |
| datacore      | 186      | 581   | 3053  | 98.2%         | 57/62                 | jump 548, drop 124, ladder 26                                   |
| frenzy        | 116      | 492   | 2795  | 97.0%         | 40/43                 | jump 233, drop 337, ladder 12, door 6                           |
| gasworks      | 342      | 1418  | 8416  | 99.9%         | 73/73                 | jump 1313, drop 343, ladder 292, swim 2167, door 11, teleport 3 |
| lambda_bunker | 239      | 707   | 4195  | 98.8%         | 44/44                 | jump 670, drop 130, ladder 48, swim 754, breakable 28, push 10  |
| rapidcore     | 156      | 437   | 2161  | 97.6%         | 58/64                 | jump 177, drop 82, ladder 22, swim 23, door 12                  |
| snark_pit     | 124      | 399   | 2133  | 98.4%         | 45/62                 | jump 198, drop 155, ladder 12, swim 346, door 8, push 11        |
| stalkyard     | 215      | 703   | 3643  | 98.9%         | 70/73                 | jump 601, drop 401, ladder 6, door 6, lift 12, breakable 4      |
| subtransit    | 295      | 898   | 5297  | 97.2%         | 64/66                 | jump 867, drop 255, ladder 14, swim 855, door 3, lift 6         |
| undertow      | 209      | 784   | 4228  | 99.3%         | 41/42                 | jump 361, drop 456, ladder 84, swim 388, door 2, lift 9         |

Items count item nodes (items close together share one); floor covered counts the 16-unit spans of floor whose node
a bot gets to from a spawn point and back.

A kept graph loads in under a millisecond; on the stand's dev build the whole map load from the cache took 96–115 ms
(measured with the denser graph of 27.09).

## Items the graph does not get to

`lb-cli nav coverage <map.bsp>` lists them with what is missing (`in`: nothing leads there; `out`: nothing leads
back; `no floor`: the item floats or rests on an entity). What they have in common:

| Map        | Items                        | Why                                                                             |
|------------|------------------------------|---------------------------------------------------------------------------------|
| boot_camp  | 2 gauss clips                | inside water tanks closed on every side                                         |
| boot_camp  | 4 satchels                   | on a stand 45–50 units up: the jump checks fail                                 |
| boot_camp  | egon, battery, 2 health kits | a room next to a corridor on the same level, with no way in the generator finds |
| bounce     | 2 satchels, 2 grenades       | on top of a push-field shaft: no flight up it lands there                       |
| bounce     | battery, health kit          | on islands reached only by a long jump from where no node stands                |
| crossfire  | 12 items of the secret room  | its door opens from a plate behind it; the count of 27.09 took them as reached  |
| datacore   | rpg, 3 rpg clips             | shelves 120–220 units up                                                        |
| datacore   | tripmine, 357, rpg clip      | no floor under them (they rest on entities)                                     |
| datacore   | snarks                       | in a pit with no way in or out                                                  |
| frenzy     | long jump                    | on the central pillar, behind a railing                                         |
| rapidcore  | gauss, rpg, 3 batteries      | 78–412 units up                                                                 |
| snark_pit  | 4 satchels, 4 grenades       | the secret room: its button is inside it (`secret_gate`)                        |
| snark_pit  | 4 gauss clips, 4 batteries   | a pocket beside the fan's vent at the top of the shaft                          |
| snark_pit  | rpg                          | behind the vent of `fan1`, a push field switched off at the start               |
| stalkyard  | 2 batteries                  | 64–128 units up                                                                 |
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
| ladder   | 120/120     | lift      | 58/58       |
| push     | 53/54       | breakable | 39/42       |
| teleport | 3/3         | jump      | 437/466     |
| swim     | 284/287     | door      | 30/51       |
| drop     | 423/437     | routes    | 777/809     |

Routes by map: boot_camp 142/150, bounce 55/58, crossfire 109/109, datacore 56/57, frenzy 39/40, gasworks 71/73,
lambda_bunker 44/44, rapidcore 58/58, snark_pit 45/45, stalkyard 64/70, subtransit 59/64, undertow 35/41.

What most failures come down to:
- **Doors opened by a remote button.** The executor walks straight to the button and back; where the way is not
  straight (snark_pit's hatches), the link fails, and bots learn to go around it.
- **Jumps near odd geometry** (a railing at the edge of a ledge, a doorway frame): the check passes, the bot fails,
  marks the link, and plans around it.
- **Items reached only through a failing link:** such a route cannot arrive within the test's 120 s.

**Live** (27.09, with the denser graph of that day; stand, crossfire, one bot, `lb nav test all 60`). The generated
graph is made in the server, all 1507 special links pass the live check (0 disagree), and `changelevel` loads the
graph from the cache.

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

## Sparser graph, straight walks

On the stand the bots of 27.09 looked up and down a lot, turned their heads all the time on the move and ran into
walls; the graph drawn by the editor was dense, and some walk links crossed walls. What changed on 28.09:

- **Nodes** are placed twice the room around them apart (112–224 units, was 48–128). Where two nodes whose floors
  meet cannot walk straight to each other, a node goes on their border. Doors get a node on each side, triggers that
  open doors and lifts one inside, buttons one in reach found on the floor (probing from a button low in a recess
  found none).
- **Walks** are made only where walking in a straight line gets there. Sliding along a wall no longer makes a walk,
  and the links that only slid round a corner are gone. A drop has to touch down within 96 units of its node, as the
  executor needs.
- **Following** pushes against sideways drift, cuts a corner only when the way on is clear, and starts at the node
  nearest the bot when the next one is round a corner.
- **Looking.** On the move the bot looks 256 units ahead along the path, within 12° of level; the pitch is exact
  only on ladders, in water and at buttons. Sounds draw a glance only when near, not in front already, and 2.5–5 s
  apart; a sound's height is off by 10° (was 30°), and the glance stays within 15° of level. A look at a point
  right above or below the eyes holds the view (it used to snap it straight down, or up when ducked). On ladders the
  server now gets the right yaw (it used the direction to the point from the map's origin).

| Graph             | 27.09          | 28.09         |
|-------------------|----------------|---------------|
| crossfire         | 2096 / 13012   | 890 / 4642    |
| boot_camp         | 6540 / 39486   | 2826 / 15008  |
| all twelve maps   | 24374 / 155921 | 10917 / 59931 |
| time on crossfire | 1.5 s          | 0.36 s        |

Nodes / links. Walk links that pass only by sliding along a wall: 6–14% of the walks on crossfire, stalkyard and
datacore before; the generator makes none now.

**Offline course**, the same routes from spawn points to items on all twelve maps (`generated_course`):

| Measure                                   | 27.09          | 28.09          |
|-------------------------------------------|----------------|----------------|
| routes arrived                            | 749/819, 91.5% | 777/809, 96.0% |
| time walking on the routes                | 4898 s         | 3739 s         |
| share of walking against a wall           | 8.1%           | 1.5%           |
| share of walking looking > 20° up or down | 22.4%          | 0.2%           |
| view turning while walking                | 105°/s         | 72°/s          |

**Live**, crossfire, 8 bots, 1000 fps, 150 s recorded for each build and measured with `look_stats` (running
bots, ladders and water left out; "out of a fight" means no shot for 2 s):

| Measure                                           | 27.09 | 28.09 |
|---------------------------------------------------|-------|-------|
| looking > 20° up or down, all running             | 21.9% | 6.6%  |
| looking > 20° up or down, out of a fight          | 31.4% | 2.7%  |
| turns over 45° in 0.25 s a minute, out of a fight | 53.9  | 47.6  |
| away from the way run, out of a fight             | 63°   | 48°   |

The live check confirms all 1120 special links of the new crossfire graph (0 disagree). The view still turns often
on the move: a replay of the 28.09 recording with the look's owner logged puts 217 of the 688 turns over 45° on the
path look itself and about 190 on glances at sounds and back; the rest is fighting. Why the path look turns that
much is not known yet.

## Planning

| Map       | Landmarks | Nodes expanded per search: straight-line bound | with landmarks |
|-----------|-----------|------------------------------------------------|----------------|
| crossfire | 8         | 197                                            | 39             |
| boot_camp | 11        | 643                                            | 153            |

300 random pairs a map; the paths are the same cost either way (`crates/lb-navgen/tests/maps.rs`). All bots together
expand at most 2000 nodes a frame and 200 000 a second; a search that runs out continues in the next frame.

## Tools

- `lb-cli nav gen <map.bsp> [--out file.lbnav]`: makes and writes a graph, with times per stage and coverage.
- `lb-cli nav coverage <map.bsp> [--json]`: the coverage report with the unreachable items.
- `lb-cli nav path <map.bsp> <x,y,z> <x,y,z>`: a path with the kind of every link.
- `lb-cli nav validate-overlay <map.bsp> <overlay.yaml>...`: applies overlays and reports what does not apply.
- `cargo run --release -p lb-runtime --example look_stats -- <file.lbrec>...`: how the bots of a recording looked
  while running (steep looks, turning, snaps, how far from the way run).
- In the game: `lb nav regen`, `lb overlay [reload]`, `lb edit ...` (`docs/overlays.md`), `lb_nav_source`, `lb_editor`.

## Not done yet

- Doors opened from a remote button whose way is not straight: the executor needs to walk the graph to the button.
- Publishing a walk-only graph first, then the rest (staged publishing): the whole graph is published at once.
- The visibility table between nodes.
- The live failures above: short jumps onto ledges and drops that stop on the edge (measured with the graph of
  27.09).
- The view turns often on the move (the path look itself and glances at sounds, see above).
- The editor has not been driven by a player on the stand: it needs someone in the game.
- ReHLDS and the GG server's maps.
