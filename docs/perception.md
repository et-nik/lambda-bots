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
   visible.
3. **Recognition.** Evidence builds up at a rate that depends on:
   - how much of the body is visible;
   - where the player is in the view: center, middle band or edge (`peripheral_gain`);
   - how the player moves: running players are noticed sooner than players standing still or crouching;
   - distance;
   - cues: a muzzle flash; a sound or the damage compass pointing there within the last 2 s; being in a fight.

   At full rate, recognition takes a delay drawn once per contact from `recognition_delay`. Halfway there the bot
   notices *something* in that direction and may glance at it, without knowing who it is. A player lost for less
   than `reacquire_grace` seconds near where it was expected is recognized again after `reacquire_delay`.
4. **What a recognized player shows:**
   - position, with a small error;
   - velocity, from successive sightings;
   - stance, ladder and water;
   - facing;
   - the weapon in its hands;
   - firing;
   - how it is drawn (a spawn-protection glow).

   Health, armor and ammo are never visible.

## Hearing

A sound reaches a bot when the engine would deliver it to that client (the PAS of the source, or everyone for
global sounds). The bot hears it when the volume the client would play it at, `volume × (1 − distance ×
attenuation / 1000)`, is above `hearing_threshold`. A gunshot at normal attenuation carries about 1200 units.

The bot's own shot masks quieter sounds for 0.3 s (the threshold triples); running raises the threshold by a
fifth.

What is heard is anonymous: a kind (step, jump, pain, shot, reload, pickup, item respawn, explosion), sometimes a
weapon, and a guessed position. The bearing error grows from `sound_bearing_sigma` for loud sounds up to 35° for
faint ones, and faint sounds are sometimes placed behind instead of in front. The range is off by about 30%.

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

| State         | When                                                                 |
|---------------|----------------------------------------------------------------------|
| visible       | seen on the latest look                                              |
| lost          | out of sight for up to 1 s; position extrapolated for 0.3 s at most  |
| predicted     | not located for up to `track_forget` seconds                         |
| stale         | older than that; dropped 30 s after the last position fix            |

The position error grows by 60% of the server's maximum speed per second since the last fix. A player who dies in
the kill feed is forgotten at once.

## Skill parameters

The parameters live in `config/difficulty.yaml` and can be overridden per personality (see `docs/personas.md`):

| Parameter             | noob     | easy     | normal   | hard      | expert    |
|-----------------------|----------|----------|----------|-----------|-----------|
| `recognition_delay`   | 1.5–2.0  | 1.0–1.5  | 0.5–1.0  | 0.25–0.5  | 0.1–0.25  |
| `peripheral_gain`     | 0.35     | 0.40     | 0.45     | 0.50      | 0.55      |
| `reacquire_delay`     | 0.35     | 0.25     | 0.15     | 0.10      | 0.05      |
| `reacquire_grace`     | 1.0      | 1.5      | 2.0      | 2.5       | 3.0       |
| `hearing_threshold`   | 0.07     | 0.055    | 0.04     | 0.03      | 0.02      |
| `sound_bearing_sigma` | 35       | 28       | 20       | 14        | 10        |
| `track_forget`        | 4        | 6        | 8        | 10        | 12        |

## Inspecting

`lb vision [name|#userid]` prints what each bot is looking at and the following:
- vision counters: looks, traces per look, recognitions with their mean and worst time;
- the players it is noticing (evidence so far) or sees;
- its tracks;
- what it heard and felt in the last seconds.

With `lb_log_level debug` every recognition is logged with its time and distance. With telemetry on, every
recognition is also sent as a `seen` event, and the `frame` messages carry every bot's tracks.
