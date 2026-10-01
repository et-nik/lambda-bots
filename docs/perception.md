# Perception

Bots perceive the game the way a player does. They never read another player's position, health or ammo from
the server. Everything they know comes from three senses and from the public information every client receives
(the kill feed, the scoreboard, server cvars).

## Vision

A bot looks 20 times a second; bots take turns so they do not all look on the same frame.

1. **Candidates.** Other living players within 4096 units. The player's box must be in the PVS of the bot's eye
   (the set the engine uses to decide what to send a client) and inside the view frustum. The frustum is built from
   the bot's actual view angles. The field of view is the HL default: 90° at 4:3, which is 106° on a 16:9
   screen. A zoomed weapon narrows it.
2. **Line of sight.** Up to six body points are traced from the eye: chest, head, pelvis, both sides and knees.
   Other players block the view; glass does not. A water surface between the eye and the target halves what is
   visible. A look spends at most 12 traces: players already in sight come first (two points found are enough for
   one recognized), then the others, least lately looked at first. While one of those is in view, three more are
   spent on it, so a player stepping out close in front gets a look while others are followed (with two players
   noticed far off it used to go unlooked at for seconds).
3. **Recognition.** Evidence builds up at a rate that depends on:
   - how much of the body is visible;
   - where the player is in the view: center, middle band or edge (`peripheral_gain`);
   - how the player moves: running players are noticed sooner than players standing still or crouching;
   - distance: within the 1000 units a deathmatch fight mostly takes place at it makes no difference; further off
     recognition slows down, to half the rate from 3000 units on;
   - cues: a muzzle flash; a sound or the damage compass pointing there within the last 2 s; being in a fight.

   Close by a player is plain to see: within 200 units fully, fading out by 600, it counts as running however it
   moves, and it is recognized 3.5 times sooner in the middle of the view, twice in the middle band, 1.5 times at
   the edge.

   At full rate, recognition takes a delay drawn once per contact from `recognition_delay`. Nobody new is recognized
   sooner than `recognition_floor` after coming into view, however plain to see: a normal bot recognizes an enemy
   standing 200 units in front in 0.16–0.2 s, an expert in 0.1 s. A fifth of the way there the bot notices
   *something* in that direction and looks at it at once, without knowing who it is (`docs/behavior.md`,
   *Attention*): a player coming into view at the edge is soon in the middle of it, where it is recognized soonest.
   A player lost for less than `reacquire_grace` seconds near where it was expected is recognized again after
   `reacquire_delay` wherever it is in the view, and draws no glance; nor does a teammate seen in the last 3 s about
   where it comes into view. A recognized player out of sight for no more than 0.25 s (behind a pillar or another
   player) is followed on without being recognized again.
4. **What a recognized player shows:**
   - position, with a small error;
   - velocity, from successive sightings;
   - stance, ladder and water;
   - facing;
   - the weapon in its hands;
   - firing;
   - how it is drawn (a spawn-protection glow).

   Health, armor and ammo are never visible.

### Projectiles, mines and chargers

On the same looks the bot sees projectiles and placed explosives (a few traces per look, nearest first) within a range
by kind: a rocket's glow 3000 units, a crossbow bolt 1500, a grenade 1200, a tripmine 1000, a snark 900, a satchel or
a hornet 800. They must be in the PVS and the view frustum with a clear line to them. What it sees is where they are
and how they move; a grenade coming down, an MP5 grenade and a rocket are followed to where they will blow up, and
forgotten 0.4 s after they leave the view (a satchel or a snark lying about after 10 s). A tripmine in view shows its
beam: the line it faces, traced once to the first wall. Mines are remembered until an explosion goes off at them.

The bot knows its own satchels and mines as its own: it threw or placed them and remembers where; seeing them moves
them to where they are. The game removes a dead player's satchels, and so does the bot's memory.

Every second look also checks one wall charger in view: a spent charger's display is dark.

## Hearing

A sound reaches a bot when the engine would deliver it to that client (the PAS of the source, or everyone for
global sounds). The bot hears it when the volume the client would play it at, `volume × (1 − distance ×
attenuation / 1000)`, is above `hearing_threshold`. A gunshot at normal attenuation carries about 1200 units.

The bot's own shot masks quieter sounds for 0.3 s (the threshold triples); running raises the threshold by a
fifth.

What is heard is anonymous: a kind (step, jump, pain, shot, reload, pickup, item respawn, explosion, a grenade
bouncing), sometimes a weapon, and a guessed position. Explosions reach the bots as the engine sends them to clients
(`TE_EXPLOSION`, to the PAS of the blast): heard like a loud shot, and a mine or satchel the bot knew of at the spot is
gone. A hand grenade's bounce (`weapons/grenade_hit*`, played by the grenade at a quarter of full volume, heard out
to about 1000 units) tells where it may go off, not where anyone is: the bot keeps away from it, and it never joins
what the bot believes of players. The bearing error grows from `sound_bearing_sigma` for loud sounds up to 35° for
faint ones, and faint sounds are sometimes placed behind instead of in front. The range is off by about 30%, the
height by about 10°.

Footsteps follow the multiplayer rule of `pm_shared`: steps are audible only with `mp_footsteps 1`, and only on a
ladder or above 220 units per second. Walking is silent. With ReHLDS the steps come from the engine's
`SV_StartSound`. Other engines play steps where Metamod cannot see them, so the bot reconstructs them from the
step counter every player carries.

A sound never makes a bot recognize anyone. It can only:
- draw the bot's eyes;
- speed up recognition in that direction;
- refresh the memory of the one known enemy it fits. The enemy must have been located within the last few
  seconds, the sound must be close to where the enemy should be, and no other known enemy may fit.

## Damage

A bot that takes damage learns what a player sees on the HUD: the four compass indicators. From them it guesses
the direction the damage came from, with a 15° error. It gets no distance and no attacker. Damage from within 50
units lights every indicator and gives no direction.

## Memory

A recognized player becomes a track:

| State     | When                                                                |
|-----------|---------------------------------------------------------------------|
| visible   | seen on the latest look                                             |
| lost      | out of sight for up to 1 s; position extrapolated for 0.3 s at most |
| predicted | not located for up to `track_forget` seconds                        |
| stale     | older than that; dropped 30 s after the last position fix           |

The position error grows by 60% of the server's maximum speed per second since the last fix. A player who dies in
the kill feed is forgotten at once.

**Places.** With the map's graph loaded, a bot also keeps in mind the map's places (graph nodes):
- when it last had each place in sight, looked at four times a second: the places its own place sees (the map's
  table of who sees whom), within 1500 units and in its view, or within 128 units wherever it looks;
- where each enemy out of sight may be now: the places it could have run to since it was last placed (by sight or by
  a sound tied to it), not through a place the bot has watched all along since before the enemy could have got there,
  weighted by the way it ran, by the traffic there, and less for places in sight since (see `docs/behavior.md`,
  *Looking for a lost enemy*).

**Items** are seen at their spots, and heard: a pickup or a respawn heard where only one item spot is within reach of
the sound's place (a third of its distance, 96 units at least) is that item's. `lb brain` shows what the bot does with
all this.

## Skill parameters

The parameters live in `config/difficulty.yaml` and can be overridden per personality (see `docs/personas.md`):

| Parameter             | noob    | easy    | normal    | hard      | expert    |
|-----------------------|---------|---------|-----------|-----------|-----------|
| `recognition_delay`   | 0.7–1.0 | 0.4–0.6 | 0.22–0.35 | 0.14–0.22 | 0.08–0.14 |
| `recognition_floor`   | 0.30    | 0.22    | 0.16      | 0.13      | 0.10      |
| `peripheral_gain`     | 0.40    | 0.45    | 0.50      | 0.55      | 0.60      |
| `reacquire_delay`     | 0.25    | 0.15    | 0.10      | 0.06      | 0.04      |
| `reacquire_grace`     | 1.0     | 1.5     | 2.0       | 2.5       | 3.0       |
| `hearing_threshold`   | 0.07    | 0.055   | 0.04      | 0.03      | 0.02      |
| `sound_bearing_sigma` | 35      | 28      | 20        | 14        | 10        |
| `track_forget`        | 4       | 6       | 8         | 10        | 12        |

`bots.reflex` in `config/lambdabots.yaml` (cvar `lb_reflex`, 0.5–2) scales every bot's recognition and aim
latencies and turns on top of its skill (`docs/behavior.md`, *Aim*).

## Inspecting

`lb vision [name|#userid]` prints what each bot is looking at and the following:
- vision counters: looks, traces per look, recognitions with their mean and worst time;
- the players it is noticing (evidence so far) or sees;
- its tracks;
- what it heard and felt in the last seconds.

With `lb_log_level debug` every recognition is logged with its time and distance. With telemetry on, every
recognition is also sent as a `seen` event, and the `frame` messages carry every bot's tracks.
