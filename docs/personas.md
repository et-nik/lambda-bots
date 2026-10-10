# Bot personalities

A personality binds a bot's nickname to a play style, a skill and a look. The bot with that nickname always plays
the same way: on any map, after a kick and a rejoin, after a server restart.

## Where personalities come from

| File                 | Written by | Contents                                                             |
|----------------------|------------|----------------------------------------------------------------------|
| `profiles/*.yaml`    | you        | your personalities; any number of files                              |
| `data/profiles.yaml` | the server | personalities created for new nicknames from `names/<language>.yaml` |

- A nickname is unique across all files, case-insensitively. An entry in `profiles/` wins over an entry with the
  same nickname in `data/profiles.yaml`. To take over a personality the server created, copy its entry into
  `profiles/`.
- The server only appends new entries to the end of `data/profiles.yaml`. It never touches existing entries, your
  edits or comments, so `bots:` must stay the last key of the file.
- If `data/profiles.yaml` is broken (a YAML error), the server leaves it alone: new personalities live until the
  server stops, and `lb roster` shows the error.

## Fields

Only `name` is required. Missing fields are derived from the nickname once and never change afterwards.

| Field       | Meaning                                                                                                  |
|-------------|----------------------------------------------------------------------------------------------------------|
| `name`      | nickname, up to 31 bytes, without quotes, `;`, `%`, `\`                                                  |
| `style`     | `balanced`, `rusher`, `sniper`, `controller`, `trapper`; default `balanced`                              |
| `skill`     | 0–100 or `noob`/`easy`/`normal`/`hard`/`expert` (= 0/25/50/75/100); default 50                           |
| `model`     | player model; default: one of `bots.models`                                                              |
| `colors`    | `[top, bottom]`, 0–255                                                                                   |
| `traits`    | `{ aggression: 0–1, fear: 0–1 }`; default: drawn within the style's range                                |
| `weapons`   | favourite weapons, best first: `[crossbow, "357", shotgun]`                                              |
| `weight`    | how often it joins relative to others; default 1; 0 = only on `lb add <name>`                            |
| `tags`      | free labels                                                                                              |
| `seed`      | seed of its own random habits; default: derived from the nickname                                        |
| `overrides` | single skill parameters from `config/difficulty.yaml`: `{ turn_speed: 1200 }`                            |
| `chat`      | how it talks in the chat: `{ chattiness: 0–1, profanity, typing_cpm, style, about }`; see `docs/chat.md` |

Example `profiles/roster.yaml`:

```yaml
schema: lambdabots/profiles@1
bots:
  - name: "Kleiner"
    style: sniper
    skill: 70
    model: scientist
    colors: [160, 40]
    traits: { aggression: 0.35, fear: 0.8 }
    weapons: [crossbow, "357"]
    weight: 2
  - name: "Barney"
    style: rusher
    skill: hard
    overrides: { turn_speed: 2000 }
```

## Skill

Skill is a 0–100 scale with five presets at 0, 25, 50, 75 and 100. `config/difficulty.yaml` sets the parameters at
these points: recognition time and its floor, aim latency and error, turn speed and acceleration, hearing, memory,
tricks, how readily long jumps
are taken (`longjump`: 0.15, 0.35, 0.6, 0.9, 1.0) and whether they are bold and dodge (`longjump_bold`,
`longjump_dodge`, hard and expert), throwing grenades one after another until none is left (`throw_series`, hard and
expert), shooting the gauss through walls (`gauss_walls`, hard and expert), bunny hopping (`bhop_speed`, and
`bhop_speed_uncapped` on a server that does not crop fast jumps: hard 1.5 and 1.7 times maxspeed, expert 1.7 and 2.0)
and so on.
Between two presets numbers are mixed linearly, so skill 62 is about halfway between normal and hard. Switches (aim
model, tricks, bold long jumps, grenade series, dodge jumps, bunny hopping) keep the lower preset's value until the
next point: a bot bunny hops from skill 75 on.
`overrides` in a personality then change single parameters, and `bots.reflex` in `config/lambdabots.yaml` (cvar
`lb_reflex`, 0.5–2) makes every bot that many times as quick: recognition, aim latency and the scope's settling take
that share of the time, turns are that much faster.

The presets are set for Half-Life's pace: an expert answers an enemy near its crosshair in some 0.13 s and flicks 90°
in 0.1 s, a normal bot in 0.25–0.3 s and 0.2 s, a beginner in 0.5–1.5 s. A
`config/difficulty.yaml` kept from an older version keeps its older, slower table: the server warns about every
parameter that differs from the built-in one; replace the file to take the new one.

`lb profile <name>` prints the resulting parameters of a personality.

## Styles

A style is data in `config/styles/<style>.yaml`:
- `traits`: the ranges of aggression and fear that new personalities of the style draw from.
- `goals`: multipliers of the goal weights, where 1 is the balanced style's weight of fighting, chasing, backing off,
  collecting and wandering.
- `weapons`: `guns`, multipliers of how good the style finds each gun (by its classname without `weapon_`; 1 = as
  good as its damage says; a list given replaces the built-in one), and `throwables`, how readily it throws grenades,
  satchels and snarks.
- `tricks`: chances 0..1 of long jumps along the way (with the module), long jumps in a fight (at an enemy, and to
  dodge), gauss jumps on the way somewhere far, and satchels thrown from a jump. The long jump chances are the
  style's liking: how readily a bot takes long jumps is the skill's `longjump` times the style's chance over the
  balanced one's, no more than 1. Below the normal preset's skill only the long jumps on the way are done (see
  `docs/behavior.md`, *Tricks*).

The goals are (see `docs/behavior.md` for what each does):

| Goal          | What the bot does                                       |
|---------------|---------------------------------------------------------|
| `engage`      | fights an enemy in sight                                |
| `hunt`        | looks for an enemy lost moments ago                     |
| `retreat`     | backs off to cover when hurt and scared                 |
| `collect`     | goes for weapons, ammo, health, armor and the long jump |
| `roam`        | wanders around the map                                  |
| `investigate` | goes to see what made a sound                           |
| `camp`        | holds a spot with long sightlines                       |
| `ambush`      | waits out of the way by a chokepoint                    |
| `control`     | waits by an item about to come back                     |
| `trap`        | lays tripmines and satchels where players pass          |

Built-in values:

| Style        | aggression | fear      | engage | hunt | retreat | collect | investigate | camp | ambush | control | trap |
|--------------|------------|-----------|--------|------|---------|---------|-------------|------|--------|---------|------|
| `balanced`   | 0.40–0.70  | 0.40–0.70 | 1.0    | 1.0  | 1.0     | 1.0     | 1.0         | 0.25 | 0.3    | 0.4     | 0.4  |
| `rusher`     | 0.70–1.00  | 0.00–0.40 | 1.2    | 1.4  | 0.6     | 1.0     | 1.3         | 0    | 0.2    | 0.4     | 0.3  |
| `sniper`     | 0.20–0.50  | 0.70–1.00 | 1.0    | 0.3  | 1.4     | 1.0     | 0.6         | 2.0  | 1.2    | 0.6     | 0.5  |
| `controller` | 0.45–0.70  | 0.40–0.60 | 1.0    | 1.0  | 1.0     | 1.3     | 0.9         | 0.3  | 0.4    | 2.0     | 0.4  |
| `trapper`    | 0.30–0.60  | 0.50–0.80 | 1.0    | 0.8  | 1.2     | 1.0     | 0.8         | 0.4  | 1.2    | 0.6     | 2.0  |

| Style        | Guns                                                      | Throws |
|--------------|-----------------------------------------------------------|--------|
| `balanced`   | —                                                         | 1.0    |
| `rusher`     | shotgun 1.25, 9mmAR 1.15, egon 1.1, 357 0.9, crossbow 0.8 | 1.0    |
| `sniper`     | crossbow 1.35, 357 1.25, gauss 1.1, egon 0.9, shotgun 0.8 | 0.8    |
| `controller` | gauss 1.15, rpg 1.1, egon 1.1                             | 1.0    |
| `trapper`    | 9mmAR 1.1                                                 | 1.6    |

| Style        | `longjump` | `lj_attack` | `gauss_jump` | `satchel_jump` | `grenade_jump` |
|--------------|------------|-------------|--------------|----------------|----------------|
| `balanced`   | 0.8        | 0.6         | 0.33         | 0.4            | 0.5            |
| `rusher`     | 0.8        | 1.0         | 0.33         | 0.6            | 0.7            |
| `sniper`     | 0.8        | 0.6         | 0.33         | 0.2            | 0.3            |
| `controller` | 1.0        | 0.6         | 0.5          | 0.4            | 0.5            |
| `trapper`    | 0.8        | 0.6         | 0.33         | 0.7            | 0.6            |

A personality's own `weapons` are its favourites on top of the style's: the first one listed counts 1.2 times more,
the others 1.1 times.

A file may leave values out; they keep the built-in ones. Personalities that are already saved keep their traits:
a new trait range only affects personalities generated later and hand-written profiles without `traits`.
`lb config reload` applies changed style files to bots in the game.

## Who joins the server

A personality's skill never changes. The server chooses **who may join**:

| cvar / config key                     | Values                                                                                                                          |
|---------------------------------------|---------------------------------------------------------------------------------------------------------------------------------|
| `lb_difficulty` / `roster.difficulty` | `any`; a preset = a band around it (`hard` = 63..87); a number = ±12 (`60` = 48..72); a range `normal-hard` (38..87) or `40-70` |
| `lb_style` / `roster.styles`          | `any` or a comma list of styles: `rusher,sniper`                                                                                |

When the quota needs a bot, the choice goes in this order:
1. A personality requested with `lb add <name>`. It joins even outside the filters.
2. Bots that played before the map change (with `bots.save_names`).
3. A new personality, while fewer than `roster.generate.pool` personalities (16 by default) pass the filters.
4. A random admitted personality that is not on the server yet, by `weight`.
5. A new personality, when everyone admitted is already playing.

A new personality gets a free nickname from `names/<language>.yaml`. Its style is drawn by the weights in
`roster.generate.styles` (only styles allowed by `lb_style`), its skill inside the `lb_difficulty` band, more often
near the middle. Model, colors and traits come from the nickname. All of it is written to `data/profiles.yaml`
right away.

`pool` sets the size of the regular cast: with a quota of 8 and `pool: 16` the server first creates 16
personalities and then rotates among them. `pool: 8` gives exactly eight regulars; `generate.enabled: false`
means only your own personalities join.

## Commands

| Command             | Action                                                                                                  |
|---------------------|---------------------------------------------------------------------------------------------------------|
| `lb roster [all]`   | personalities admitted by the filters (or all): style, skill, weight, source, state                     |
| `lb profile <name>` | one personality and its resulting skill parameters                                                      |
| `lb add <name>`     | add a specific personality (raises the quota by 1)                                                      |
| `lb config reload`  | re-read the config, `difficulty.yaml`, names and profiles; bots in the game pick up the changes at once |
| `lb list`           | bots on the server with their style and skill                                                           |

At startup the server prints a summary to the console: how many personalities come from `profiles/`, how many were
created by the server, and how many pass the filters.
