# Testing how bots get about a map

A test gives a bot items, names a spot and lets the bot try to get there. It says whether the bot got there and, when
it did not, why:
- no way;
- a link failed on the way;
- a trick came down elsewhere;
- out of time;
- the bot died.

The work goes in a loop:
1. Try a spot live with `lb do`.
2. Save the tries worth keeping as the map's tests with `lb test add`.
3. Change the code, and run the tests again: `lb-cli nav try` offline first, then `lb test run` on the server.
4. Compare the new run with the last one.

## What the bot does

The bot on command is taken off its behavior. It does not fight and it does not pick things up. The other bots stand
still until the command ends, as with `lb_freeze 1`.

- **The spot** is where a player stands: the floor under the point named, with a standing player's origin 36 units
  over it. The bot has got there when it stands within the radius of the spot (32 units by default), its feet within
  24 units of the spot's height. Getting to the graph node nearest the spot does not count.
- **Along the graph.**
  1. Among the nodes within 320 units of the spot, the bot takes the nearest ones with a clear straight walk to it.
     Of those it can get to, the cheapest way wins.
  2. The bot walks the graph there, then straight to the spot.
  3. Links that fail on the way are avoided and the way is planned again, as in play.
- **A trick found on the spot.** When no such node can be got to, the bot looks for a trick onto the spot from the
  nodes it can get to, cheapest first (the way there plus a rough price of the trick):
  - a jump (up to 64 units up, 320 away);
  - a long jump, with the module (200–700 units away, up to 48 up);
  - a gauss boost, with the gauss, a full charge's uranium and 60 health.

  The search follows flights through the map's BSP with doors and lifts at rest, one check a frame on the server, so
  it takes a moment. Each trick is the kind the graph's links use (`docs/navigation.md`), landing off the graph. A
  trick that comes down on the spot's floor but short of it walks the rest when the way is clear.
- **The gauss boost is charged for the push it takes.** The recoil grows over the 1.5 s of a full charge, and the game
  lets a charge go after half a second at the soonest. So the search tries pitches from 30° to 70° down. For each it
  takes the least push that throws the bot past the spot in the open: 32 units and 8% of the way further, because
  steering in the air only brakes. The gentlest push that holds is taken. It holds when:
  - the steered flight comes down within 64 units of the spot;
  - it still does 3° off to either side, 2° up or down, 8 units along or with the push 4% off;
  - the unsteered flight comes down safely;
  - the beam does not come back at the bot;
  - the bot keeps 40 health after the landing.

  The bot stops within 6 units of the takeoff. The weapons turn round first and charge after, so a partial charge
  goes on time.

Which tricks a test may use is its choice (`tricks`). What the bot can do is what it carries: the long jump module, the
gauss with its uranium, its health.

## Commands

The game gives items only with `sv_cheats 1`, set before the map starts. The commands say so when it is off. On a
GunGame server, GunGame takes weapons away as it hands out levels: test on a map without it.

| Command                                                                  | Action                                                   |
|--------------------------------------------------------------------------|----------------------------------------------------------|
| `lb give <name\|#userid\|all> <item>...`                                 | items for bots now                                       |
| `lb do <name\|#userid\|all> go <spot> [radius R] [timeout T] [tricks …]` | send bots to a spot; the outcome goes to the console     |
| `lb do`                                                                  | what the bots on command are doing                       |
| `lb do [<name\|all>] stop`                                               | call the commands off                                    |
| `lb test [list]`                                                         | the map's tests                                          |
| `lb test add <id> <spot> [options]`                                      | add a test to the map's file, or replace the one with the id |
| `lb test remove <id>`                                                    | remove a test                                            |
| `lb test run [<id>...\|all] [bot <name>] [repeat N]`                     | run tests with one bot                                   |
| `lb test stop`                                                           | end the run (the report is written)                      |
| `lb test results`                                                        | what the last run came to                                |

- **Items:**
  - `gauss` (with 100 uranium) and the other weapons by name (`crossbow`, `mp5`, `357`, …), with ammo;
  - `uranium`, `longjump`, `health`, `armor`;
  - any `weapon_`, `ammo_` or `item_` classname as it is.
- **Spots:**
  - `x y z`;
  - `node <n>`;
  - `@me`: where the player typing the command stands;
  - `@aim`: the floor where the player looks, 16 units back from the wall looked at;
  - `place <name>`: a place of the map's overlays.

  `@me` and `@aim` need the game console. With `sv_cheats 1`, `noclip` takes you onto a ledge to mark it.
- **Tricks:** `jump`, `longjump`, `gauss`, or `any` (the default), or `none` (the graph only).
- **Options of `lb test add`:**
  - `from <spot>`;
  - `give <item>...`;
  - `tricks <trick>...`;
  - `radius R`, `timeout T`, `repeat N`;
  - `expect arrive|no_way`;
  - `note <text>`, which takes the rest of the line.

  Typed in the game without `from`, the test starts where the player stands.

```text
lb give Hitman gauss longjump
lb do Hitman go @aim tricks gauss
lb test add bridge_ledge @aim give gauss tricks gauss repeat 3 note onto the ledge over the bridge
lb test run bridge_ledge
```

The outcome of an attempt, on the console, in the log and in the console of the player who gave the command:

```text
lb do: Hitman go 848 256 -60: arrived in 4.8 s (plan 4.6 s)
  way: 3 links (walk 3)
  trick: gauss boost from node 8 (432 672 -252): pitch 46, push 640 (charge 0.96 s), flight 1.45 s, robustness 1.00
  came down at 845 259 -60, 4 u from the spot
  trick search: 4 takeoffs, 0 jumps, 0 long jumps, 13 gauss boosts checked
```

| Outcome   | Meaning                                                                                                    |
|-----------|------------------------------------------------------------------------------------------------------------|
| `arrived` | stands on the spot                                                                                         |
| `no way`  | no node with a clear walk to the spot can be got to, and no trick lands. Says why: the nearest node and how far above or below, what was tried, what the bot could not do |
| `stuck`   | the way along the graph broke: the last link that failed, and why                                          |
| `missed`  | the trick came down elsewhere: where, and how far from the spot                                            |
| `blocked` | the last straight stretch got no nearer for 2 s                                                            |
| `timeout` | out of time: how far off, and what the bot was doing                                                       |
| `died`    | the bot died on the way                                                                                    |

## A test run

`lb test run` takes the tests one after another with one bot: the one named, or the first alive. For each attempt:
1. The bot is given the test's items and health.
2. It goes to the start, any way it can. If it dies on the way, it tries once more after the respawn.
3. It stands there half a second.
4. It tries the goal.

Every attempt is reported as it ends. At the end come a line per test with the attempts that passed, against the last
run of the map on record (`was 1/3`), and the report's file.

The report is `addons/lambdabots/logs/tests/<map>-<YYYYMMDD-HHMMSS>.json` (UTC). It holds:
- the map, the bot and the core version;
- per test, every attempt: whether it passed, the outcome and its report.

An attempt's report holds:
- `planned`: seconds;
- `path`, `links`: the planned way and its links by kind;
- `trick`: the trick and its plan;
- `landing`: where the trick came down;
- `failures`: the links that failed;
- `search`: what the trick search checked;
- `events`: what happened, seconds from the start;
- `seconds`, `end`, `off`: how long it took, where the bot ended and how far from the spot.

A run ends when the map changes or the bot leaves; the report is written then too.

The outcome of every `lb do` command goes to `logs/tests/<map>-orders.jsonl` (the last 200). The map editor reads it
and the last test run, and marks the links bots failed and the tricks that missed in the graph (*Problems*,
`docs/editor.md`).

## The map's tests: `maps/<map>/tests.yaml`

`lb test add` writes the file; it can be written by hand too.

```yaml
schema: lambdabots/tests@1
map: dm_snow
tests:
  - id: bridge_ledge
    note: "onto the ledge over the bridge"
    start: [128, 864, -252]
    goal: [848, 256, -60]
    give: ["gauss"]
    tricks: ["gauss"]
    repeat: 3
  - id: sealed_room
    goal: [0, 0, -500]
    expect: no_way
```

| Field     | Default | Meaning                                                                        |
|-----------|---------|--------------------------------------------------------------------------------|
| `id`      |         | one word, unique in the file                                                   |
| `note`    |         | what it checks, for people                                                     |
| `start`   |         | where the bot stands before each attempt; without it, where it is              |
| `goal`    |         | the spot to get to: the origin of a player standing there                      |
| `radius`  | 32      | how near the goal it must stand                                                |
| `give`    | none    | items before each attempt (as `lb give` names them)                            |
| `tricks`  | `any`   | tricks it may use                                                              |
| `timeout` | 60      | seconds an attempt may take                                                    |
| `repeat`  | 1       | attempts                                                                       |
| `expect`  | arrive  | `arrive`: it passes when the bot gets there; `no_way`: when it finds no way    |

Spots are the origins of a standing player. `lb test add` works them out from what it is given.

## Offline

```sh
lb-cli nav try valve/maps/dm_snow.bsp                       # every test of the map
lb-cli nav try valve/maps/dm_snow.bsp bridge_ledge --repeat 5
lb-cli nav try valve/maps/dm_snow.bsp --from 128,864,-248 --to 848,256,-60 --give gauss
```

The same attempt on the offline course (`lb-testkit`): the server's navigation, trick search and executors, movement
by a port of `pm_shared`, doors and lifts simulated. A code change shows in seconds, without building the plugin and
starting the server again. The server stays the judge: the recoil of the gauss, the prediction of the weapons and the
timing of the commands are the game's.

- **Graph:** the server's (`addons/lambdabots/nav/`, when it has one of this build of the map) with the map's
  overlays; otherwise one made now with the default physics. The first line says which.
- **Server directory:** `addons/lambdabots` next to the map's `maps/` directory, or `--install <dir>`. The tests are
  read from it. Nothing is written.
- **Bot:** it starts at the test's start, or at the first spawn point. It has what the test gives: `gauss` means the
  gun with its uranium, `longjump` the module.
- **Other options:** `--fps` (100 by default), `--radius`, `--timeout`, `--tricks`.
- **Exit code:** 0 when every attempt passed, 1 when one did not, 2 on an error.
