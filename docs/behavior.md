# Behavior

What a bot does with what it perceives (see `docs/perception.md` for the senses): it picks a goal, moves toward it,
and fights whenever an enemy is in sight.

## Goals

Five times a second, and at once when a new enemy is recognized or the bot takes damage, every possible goal is
scored:

| Goal      | Rank | When                                                             | Weight                                                                            |
|-----------|------|------------------------------------------------------------------|-----------------------------------------------------------------------------------|
| `engage`  | 2    | an enemy is in sight (or was, half a second ago)                 | 0.9 × (0.6 + 0.4 × aggression)                                                    |
| `retreat` | 2    | the bot is hurt and scared, and was in a fight in the last 10 s  | (100 − health) × fear, fading with time since the fight; counts only above 0.4    |
| `hunt`    | 1    | an enemy was lost moments ago and its position is still certain  | higher for close enemies and aggressive bots; counts only above 0.6               |
| `collect` | 1    | an item is worth taking (rank 2 for health when badly hurt)      | the item's value × the chance it is there on arrival × a travel penalty           |
| `roam`    | 0    | always                                                           | 0.2                                                                               |

The weights are multiplied by the style's goal weights (`config/styles/*.yaml`, see `docs/personas.md`). Health
here counts armor twice, as armor absorbs bullets.

The highest rank present wins. Among its candidates within 90% of the best weight, one is drawn at random. A chosen
goal is held for a while: 1 s for `engage`, 3 s for `hunt`, 2 s for `retreat`, until arrival for `collect`, and
5 s for `roam`. A higher rank takes over at once. The same rank takes over only when its weight beats the current
one by 15% plus 0.05. Going from `retreat` back to `engage` needs a 25% margin. A goal that fails (no path, stuck)
is not picked again for 8–15 s.

Item values:
- **Health** below 85 and **armor** below 90: the less the bot has, the more it wants them.
- **Weapons** it does not own: the better the weapon, the more it wants it.
- **Ammo** for weapons it owns: when it is short.
- **Long jump:** when it does not have one.

An item seen close by (under 450 units) is taken almost always, as in yapb. Items are known where the map places
them; whether one is there comes only from looking. An item seen missing is expected back after its respawn time
(items 30 s, weapons and ammo 20 s).

## Fighting

Ten times a second the bot picks the enemy to fight: the nearest counts most. An enemy aiming at the bot or firing
counts more, and the current target keeps a bonus.

**Weapon.** The weapon with the best expected damage per second at the target's distance wins. Expected damage
accounts for spread and for the bot's own aim error. Outside a weapon's good range (for example the shotgun
beyond 750 units) only a third counts. When all weapons are empty the bot reloads; with nothing left it takes the
crowbar.

**Aim.**
- Head or body is decided once per contact, from the skill's `headshot` chance. The shotgun aims at the body beyond
  272 units, the MP5 beyond 544.
- The bot aims where it saw the enemy `aim_latency` seconds ago, carried forward with the velocity it saw then. A
  sudden turn is missed for that long.
- A slowly drifting error grows with distance and shrinks with skill.
- The view turns like a damped spring, stiffer in a fight for hard and expert bots. It is capped by `turn_speed`;
  noob bots use yapb's wandering mouse model.

**Trigger.**
- The bot fires when the view is on the target closely enough (yapb's cones):
  - under 90 units: always;
  - under 128 units: within about 37°;
  - further away: within 8°, or within 26° when the enemy is looking at it.
- It never fires a rocket under 300 units.
- Automatic weapons are held down. Others are clicked, with a pause from `semi_auto_delay`.
- After a weapon switch it waits for the game to confirm it and 0.5 s more for the deploy.

**Movement** (yapb's `attackMovement`):
- **Style.** Every 1–3 s the bot decides between strafing and standing still. Closer than 768 units it strafes.
  Further away it stands with the skill's `stay_mid` / `stay_far` chance.
- **Strafe side.** It strafes away from the side the enemy aims at, swaps sides now and then, and turns around at
  walls.
- **Distance.** Skilled bots move in when strong and back off when weak. All bots back off under 96 units and while
  reloading. With the crowbar they charge.
- **Extras.** Crouch taps and dodge jumps come with skill.
- **Ledges.** A move that would drop off a ledge is reversed.

Whatever the goal, an enemy in sight is shot at: a bot running for health or backing off fires back. When nothing
is in sight for 2 s, a low clip is reloaded.

## Priorities

Behavior asks for what it wants on four channels (look, movement, stance, weapon), and the highest priority on each
wins:
- **Traversal (90):** jumps and ladders on the path.
- **Threat (70):** aiming and firing at an enemy in sight, turning toward damage.
- **Goal (50):** the goal's movement.
- **Optional (20):** looking along the path and glancing at sounds.

### Where a bot looks

- **On the move** it looks along its path: at a point 256 units ahead, past the small bends of the graph and round
  a corner before it gets there, tilted no more than 12° up or down. Nearer than 48 units such a point says little
  (the path's end, the top of a ladder), and the view keeps its heading. The pitch is exact only where it steers the
  move or aims: on ladders, in water, at a button to press.
- **Sounds** draw a glance only when they matter and are not in front of the bot already:
  - a shot within 1500 units, pain within 1000, steps, jumps and pickups within 700, weapon noises within 500;
  - more than 40° off the view;
  - 2.5–5 s after the last glance.

  A glance lasts 0.8 s and stays within 15° of level: how high a sound was is a guess.
- **A glimpse** of someone not recognized yet is looked at directly, and so are a lost enemy's last known position and
  the direction damage came from.
- A point right above or below the eyes (closer than 16 units across) gives no direction: the view holds still
  instead of spinning to it, unless it is an enemy.

## Inspecting

`lb brain [name|#userid]` prints for each bot:
- the current goal, its rank, weight and how long it has been held;
- the candidates of the last decision;
- the target and the weapon choice;
- which priority owns each channel;
- reaction times: from the first glimpse of an enemy, and from recognizing it, to the first shot at it.

`lb list` shows every bot's goal. With telemetry on, the `frame` messages carry every bot's goal, candidates,
target and firing state. The observer (`tools/observer`) colors bots by goal and shows the candidates of the
selected bot.
