# Behavior

What a bot does with what it perceives (see `docs/perception.md` for the senses): it picks a goal, moves toward it,
and fights whenever an enemy is in sight.

## Goals

Five times a second, and at once when a new enemy is recognized or the bot takes damage, every possible goal is
scored:

| Goal          | Rank | When                                                                         | Weight                                                                            |
|---------------|------|------------------------------------------------------------------------------|-----------------------------------------------------------------------------------|
| `engage`      | 2    | an enemy is in sight (or was, half a second ago)                             | 0.9 × (0.6 + 0.4 × aggression)                                                    |
| `retreat`     | 2    | the bot is hurt and scared, and was in a fight in the last 10 s              | (100 − health) × fear, fading with time since the fight; counts only above 0.4    |
| `hunt`        | 1    | an enemy was lost moments ago and its position is still certain              | higher for close enemies and aggressive bots; counts only above 0.6               |
| `investigate` | 1    | a shot, steps, pain, a pickup or a glimpse heard or seen 200–2500 units away | 0.6 × (0.3 + loudness) × freshness × (0.5 + aggression); counts only above 0.25   |
| `collect`     | 1    | an item is worth taking (rank 2 for health when badly hurt)                  | the item's value × the chance it is there on arrival × a travel penalty           |
| `charger`     | 1    | health under 60 or armor under 40 and a charger believed to give             | like `collect`, with 4 s of charging added to the travel (rank 2 as for health)   |
| `control`     | 1    | an item worth having comes back soon (armor, the long jump, a big gun)       | its value × how well the wait fits: up to 8 s is as good as any, none beyond 25 s |
| `camp`        | 1    | calm for 3 s, 50 health or more, a gun for the spot                          | the spot's score × (1.1 − aggression) × a travel penalty; counts only above 0.15  |
| `trap`        | 1    | calm for 5 s, carrying tripmines or two satchels or more                     | the spot's traffic (or score) × a travel penalty; counts only above 0.15          |
| `roam`        | 0    | always                                                                       | 0.2                                                                               |

The weights are multiplied by the style's goal weights (`config/styles/*.yaml`, see `docs/personas.md`); holding
spots, waiting for items and laying traps are rare for every style but the one they suit. Health here counts armor
twice, as armor absorbs bullets. Aggression and fear are the bot's mood (see *Moods*).

The highest rank present wins. Among its candidates within 90% of the best weight, one is drawn at random. A chosen
goal is held for a while: 1 s for `engage`, 3 s for `hunt`, 2 s for `retreat`, until arrival for `collect` and
`investigate`, the time to get there and hold for `camp` and `trap`, the time until the item is back for `control`,
and 5 s for `roam`. A higher rank takes over at once. The same rank takes over only when its weight beats the
current one by 15% plus 0.05. Going from `retreat` back to `engage` needs a 25% margin. A goal that fails (no path,
stuck) is not picked again for 8–15 s; a spot held is not held again for 50–70 s (15–25 s for a style fond of it).

Item values:
- **Health** below 85 and **armor** below 90: the less the bot has, the more it wants them.
- **Weapons** it does not own: the better the weapon, the more it wants it.
- **Ammo** for weapons it owns: when it is short.
- **Grenades, satchels, snarks and tripmines** (each its own ammo): the fewer the bot carries of the most the game
  lets it (10 grenades, 5 satchels, 15 snarks, 5 mines), the more it wants them.
- **Long jump:** when it does not have one.

An item seen close by (under 450 units) is taken almost always, as in yapb. Items are known where the map places
them; whether one is there comes from looking, and from hearing it taken or come back when only one item spot is near
where the sound came from. An item seen missing is expected back after its respawn time (items 30 s, weapons and ammo
20 s). A bot that saw or heard an item go and come back, both to within a second, has timed its respawn: after two
such timings it trusts its own over the game's. What the bots timed is kept for the server
(`data/learned/respawn.json`) and every bot starts from it on the next map.

**Wall chargers.** The bot walks to a spot in reach and sight of the charger (found in the map for every charger of
the standard maps), faces it and holds the use key while its health or armor rises. It stops when full, after 15 s,
or when the charger gives nothing for 1.5 s: then the charger is believed spent for its recharge time (60 s for
health, 30 s for the suit), as it is when the bot sees its display dark.

## Knowing the map

What an experienced player knows of a map is worked out once, when the map's graph is loaded (a few milliseconds to a
few tens of them on the standard maps; `lb map` shows it):
- **Who sees whom:** for every two places of the graph (nodes), whether a player standing at one sees one at the other,
  eye to eye, glass see-through, closed doors not.
- **Where players pass:** the share of the shortest ways between spawn points, items and places spread over the map
  that go through each place.
- **Chokepoints:** busy places where the way narrows to 192 units or less.
- **Spots to hold:** *overwatch* spots see much of the traffic far away (700–3000 units) and little in close;
  *ambush* spots are out of the way, little seen, 150–650 units from a chokepoint they see. Each comes with the
  directions worth watching.
- **Walls for tripmines:** across busy corridors (the beam at 20 units above the floor trips standing and crouching
  players alike) and just past turns, where a player rounds the corner into the beam (yapb's corner mines, worked out
  ahead for the whole map). None within 256 units of a spawn point.
- **Cover:** asked for when a bot backs off, a place out of sight of where the threat is and of every place next to it,
  that the bot gets to before the threat could, not where bots got hurt, with more than one way out. A live trace from
  the threat's eye makes sure (crates, doors).

**Experience.** Where bots got hurt and died, and where the damage came from when they saw who fired, is learned by
playing (yapb's practice data, from the bots' own senses only). It fades by half over 15 minutes of play, is kept per
map in `data/experience/<map>.json` every 5 minutes and at the map's end, and survives a new graph. Cover avoids
dangerous places, and a calm bot at a place where bots got hurt glances now and then where that came from.

## Looking for a lost enemy

An enemy out of sight may have gone as far as a player runs in the time since, but not through a place the bot has
watched all along since before it could have got there. The places it could have reached are weighted by how likely
a player goes there: on along the way it was running, where players pass, farther on the longer it has been gone. The
places the bot has looked at since are ruled out while in sight and come back slowly after (someone may walk in
again). A sound tied to the enemy starts it all over from where it was heard.

- **Hunting**, the bot first runs to where it lost the enemy; after a second it goes to the place that sees most of
  where the enemy may be now (a short run away counts more), and picks again every 1.5 s and on arriving.
- **Watching:** on the way, and wherever it stands, the bot looks where the enemy would come into view: the places in
  its sight next to where the enemy most likely is.
- **Through a wall:** hard and expert bots (`gauss_walls`) with the gauss in hand shoot a charged beam through a thin
  wall (48 units at most, square enough not to glance off) at an enemy lost behind it within the last second, when
  80 damage or more is left beyond the wall and the burst where the beam comes out spares the bot.

## Seeing about sounds

A sound nobody tracked makes a sound worth going to see about (the bot turns its head at it as before): the bot goes
to a place in sight of where it came from, the one it gets to soonest, and looks there for 1.5 s. A new sound near the
one being seen about adds to it rather than starting over; a sound seen about is not gone to again.

## Backing off

A retreating bot heads for cover from the nearest threat (see *Knowing the map*), or just away from it when the map
has none within 4 s of running. There it stops and watches the way the threat would come, shooting at anyone in
sight, until the retreat no longer wins.

## Holding spots

A calm, healthy bot with a gun for the spot (the crossbow, the 357, the gauss or the RPG for overwatch; the shotgun,
the MP5, the egon or the gauss for an ambush) may hold one of the map's spots: 8–15 s at an overwatch spot, 10–20 s
crouched at an ambush, half as long again for a style fond of it. It looks one way and the other every 1.5–4 s, or
where a lost enemy would come into view, and holds its weapons for the distance it watches. Rushers never hold a spot;
snipers hold overwatch spots most, trappers ambush spots.

## Waiting for items

An item seen or heard taken comes back at a known time. A bot that wants it (armor, the long jump, a big gun it lacks,
health) and gets there in time waits close by, in sight of it and out of the way, watching it, and takes it as it
comes back. Controllers do it most.

## Traps

When calm, a bot with tripmines puts one on a wall of the map's mine spots near it: it walks to where the wall is in
reach, looks at the spot and lays the mine, then steps along the wall out of where the beam will be (it arms in
2.5 s). With two satchels or more it goes up to the chokepoint an ambush spot 350 units or more
away watches (a satchel flies some 200 units), throws two to four at it, and watches them crouched from the spot for
20–30 s, out of their blast: they go off when an enemy comes by them, and after 60–90 s with nobody by them. Trappers lay traps every 10–18 s; others at most every 20–30 s and rarely.

## Moods

Aggression and fear sway around the personality's own (at most 0.3 either way): an enemy in sight makes a bot bolder
by 0.02 every half second, a kill by 0.1; damage makes a healthy bot (over 60 health) bolder by 0.05 and a hurt one
warier by 0.05; after 5 s with no enemy both drift back by 0.05 every half second. The mood weighs the goals, how the
bot fights (how much it closes in) and how readily it throws.

## Fighting

Ten times a second the bot picks the enemy to fight: the nearest counts most. An enemy aiming at the bot or firing
counts more, and the current target keeps a bonus. Through the crossbow's scope the bot keeps to its target until the
kill, or until the target has been out of sight for a second.

**Weapon.** Every gun is scored by the damage per second it is expected to deal at the target's distance: the
server's damage (BugfixedHL's `mp_dmg_*` cvars, their defaults elsewhere), the weapon's spread and the bot's own aim
error, times how much the bot likes it: its style's likes (snipers favour the crossbow and the 357, rushers the
shotgun and the MP5, controllers the big guns) and its personality's favourites (the first one ×1.2, the others ×1.1). Projectiles (rockets, crossbow bolts, hornet darts) also lose what a moving target can step aside from
during their flight, and win back part of it with their blast. Outside a weapon's good range only a third counts.
A weapon whose blast would reach the bot is not taken: no rocket under 450 units, no crossbow bolt under 160, no
egon under 128. So the egon and the gauss lead up close, rockets and the zoomed crossbow far away, much as yapb's
fixed order had it. When all guns are empty the bot reloads; with nothing left it takes the crowbar. Grenades,
satchels, snarks and tripmines are not guns: see *Explosives*.

**Secondary attack** where it pays:
- the glock's rapid fire (held down, five shots a second in a cone ten times wider) where it lands more bullets a
  second than the clicked aimed shots, given the bot's own aim error and pause between clicks: up to 250–300 units
  for a normal bot, about 200 for an expert, who clicks faster and aims better, 400 or more for a beginner;
- the hornet gun's darts under 250 units with at least four hornets;
- both shotgun barrels at 32–300 units, half of the shots (as yapb).

**The crossbow's scope** is snapped on for the target and taken off after the kill, as good players do. Zoomed in
multiplayer the crossbow fires a hitscan bolt (120 damage); unzoomed a slow bolt that a moving target steps away from,
so from 250 units on every shot is a scoped one: with the target near the view's center (the zoomed view is 20° wide)
the bot puts the scope on, settles the aim for its skill's `scope_settle` (0.1–0.15 s for experts, 0.2–0.3 s hard,
0.35–0.55 s normal, up to 1.4 s for beginners) and fires once the view is on the target's body. Through the scope the
aim error is halved. After a miss the bot stays zoomed and fires again as soon as the crossbow is ready (0.75 s) and the
aim has settled again. The scope comes off after the kill, and when the target stays out of sight for a second or comes
closer than 200 units, when the view does not get onto it within a second, or when the clip is empty: at once with a
reload when two bolts or fewer are left (a reload takes the scope off), otherwise as soon as the game lets the secondary
attack toggle it again, a second after it went on. The 357's scope is never used: it narrows the view without steadying
the shot.

**Aim.**
- Head or body is decided once per contact, from the skill's `headshot` chance. The shotgun aims at the body beyond
  272 units, the MP5 beyond 544.
- The bot aims where it saw the enemy `aim_latency` seconds ago, carried forward with the velocity it saw then. A
  sudden turn is missed for that long.
- Projectiles lead the target by their flight time. Rockets go for the feet of a target on the ground, where a near
  miss still catches it in the blast.
- A slowly drifting error grows with distance and shrinks with skill.
- The view turns like a damped spring, stiffer in a fight for hard and expert bots. It is capped by `turn_speed`;
  noob bots use yapb's wandering mouse model.

**Trigger.**
- The bot fires when the view is on the target closely enough (yapb's cones):
  - under 90 units: always;
  - under 128 units: within about 37°;
  - further away: within 8°, or within 26° when the enemy is looking at it.
- Rockets wait until the view is right on the aim point; the egon's beam sweeps onto the target from 14° off.
- An explosive is not fired when it would burst close to the bot, looked at along the line it takes from where the
  game launches it (a rocket leaves below and to the right of the eye): a rocket with a wall less than 450 units along
  it, a bolt with one under 160, the egon's beam end under 128, and a rocket or a bolt with someone else standing near
  the first 350 units of it. An MP5 grenade's arc is looked along again from where the bot is when it fires, and the
  shot is called off if a wall, a player or the target itself (closer than 300 units) came in the way.
- Automatic weapons are held down. Others are clicked, with a pause from `semi_auto_delay`.
- After a weapon switch it waits for the game to confirm it and 0.5 s more for the deploy.
- **Rockets are guided:** the RPG's rocket follows the laser spot, which is where the bot looks, so after a shot the
  view stays on the target until the rocket gets there (6 s at most): on the target it was fired at, and only while
  that stays 450 units away or more (a new target close by, or this one come close, would bring the rocket back);
  otherwise on the point it was fired at. While its rocket or launched grenade is on the way, the bot does not close
  in on the target.

**The gauss** is fought charged, as players do (the plain primary attack is a beginner's): at a target at any distance
the bot charges as often as its skill's `gauss_charge` (0.2 for beginners, 0.75 normal, 0.85–0.9 hard and expert),
holding the secondary attack while the charge builds for the distance: 0.5–0.75 s up close (under 350 units, 70–100
damage), 0.8–1.2 s at mid range, the full 1.3–1.6 s (200) far away, less when uranium runs low. It lets go of both
buttons once the view is on the target's body; the game fires on its next idle frame, and the next charge starts
0.1–0.3 s later. A bot that rolled a plain shot uses the primary attack for 0.8–1.6 s before it weighs charging again;
otherwise plain shots are fired only while no charge can start (in water, on a ladder, low on uranium, a drop right
behind, a wall right ahead): a plain shot keeps the gun from charging for a fifth of a second. While charging the primary attack is never
pressed: it would fire a plain shot and lose the charge.
- Hunting an enemy lost moments ago, a bot may hold a charge ready (as often as its skill's `gauss_precharge`) and let
  it go at the first target in its sights.
- A charged beam that meets a wall punches through and bursts where it comes out (1.75 times its damage around, in
  multiplayer), or glances off with a burst of its own. It goes through players, so the wall may well be behind the
  target. A charge is kept small enough for no wall along the line of fire to be within its burst (some 380 units
  for a full charge, 150 for half a second's); with a wall closer than that, no charge starts and plain shots are
  fired.
- **A missed charge can come back.** In vanilla HLDM (and in BugfixedHL with `mp_selfgauss 1`; its default 0 fixes
  it) a charged beam that meets a wall square but too thick to punch through starts over from the gun, and the shooter
  is no longer left out of it: a shot that misses its target and ends on such a wall hits the bot with all its damage.
  Where that can happen the bot looks along its view for the first wall at any distance and does not let the charge go
  while that wall would stop it; the way to dump a charge is chosen clear of such walls too.
- The charged shot throws its shooter back at five times its damage (1000 units per second at full charge): on level
  ground it slides some 250 units, and with the view below the horizon the push lifts it off the ground and carries it
  much further (some 650 units at 10° down). The bot looks behind it for a drop (a wall stops the throw) and keeps the
  charge small enough for the throw to stop short of it; with the drop right behind no charge starts. A charge held 8 s
  on the ground (9 s anywhere; the gun shocks its holder at 10 s), in water or on a ladder is dumped: level, along
  the one of eight ways whose first wall is farthest (its burst must spare the bot) with no drop behind within the
  recoil's throw, back the way the bot looks from when several are clear. The view turns there first (yapb fired on
  the same frame, back along the way it came and a little down, whatever was there).

**The MP5's grenade launcher.** At a target in sight 300–700 units away, every 2.5–4 s, the bot turns to the lobbed
arc that lands on the target's feet (800 units per second, half gravity, checked for walls) and fires it. The
grenade bursts on the first thing it touches and the bot moves on while it flies: it must land 400 units or more
away (550 for a bot under 40 health), with nobody near the first stretch of the arc.

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
is in sight for 2 s, a low clip is reloaded, of the gun the bot would like in hand, not only the one it holds.

## Explosives

**Throws.** Three times a second a bot with grenades, satchels or snarks weighs a throw at the nearest enemy: one in
sight, or one lost up to 3 s ago whose position is still known to within 400 units. Each kind that fits the distance
has its chance per weighing: 0.5 for a grenade, 0.35 for satchels or a snark at an enemy out of sight, and 0.12, 0.15
and 0.25 at one in sight. The best of them, times the skill's `throw_rate` (0.5 for beginners up to 1.4 for experts),
a little more for bold bots and a little less for careful ones, is the chance to throw at all; which kind goes is
drawn as likely as its own chance. After a throw the next is weighed 3–6 s later. With no gun left but the crowbar,
throws are the weapon: twice as likely, and grenades from 220 units (yapb's grenade war).
- **Hand grenade**, 300–800 units away: the throw is solved for the target with the game's own rule (the view's pitch
  sets the throw's angle and speed, the bot's own velocity is added) and checked for walls along the arc; its blast
  must land 300 units or more from the bot. The bot pulls the pin (holds the primary attack; the game's own clock
  tells when the pin came out), cooks it so it goes off soon after landing, turns to the throw, stops for the last
  moment and lets go. At an enemy in sight it throws as soon as the game allows, uncooked, where the enemy will be.
  **Once the pin is out the grenade is always thrown**, at the latest shortly before the fuse runs out.
- **A pile of satchels**, 150–400 units away with a clear line to the spot, at an enemy out of sight or one in sight
  coming this way: two to four of them (as many as the bot carries) thrown one after another at the spot, as fast as
  the game allows (a second apart); then the bot backs off out of their blast.
- **A satchel from a jump**, at an enemy in sight 350–550 units away: the bot runs at the enemy, jumps and throws it a
  moment after its feet leave the ground, so the run and the jump's lift carry it on. The satchel radio stays in
  hand, and the satchel goes off as it comes within 150 units of the enemy (the game lets the radio work half a
  second after a throw), like a grenade that goes off when told. Rather than let it pass, a bot with 70 health or
  more takes up to a quarter of its blast itself; a hurt one waits until it is out of the blast. A satchel that does
  not come by within 2.5 s is left lying. For a second after the throw the bot backs off from the enemy (the run-up
  carried it after the satchel), and it does not close in while its satchels are fresh.
- **The satchels go off** once the bot is out of their blast (300 units for the multiplayer satchel's 120 damage; it
  backs off first):
  - with an enemy within 200 units of one of them;
  - once the enemy they were thrown at has been out of sight for 1–2.5 s after it was last seen near them (it may
    well still be there), up to 6 s after;
  - after they have lain 8–15 s with nobody in sight, whatever else.
  The bot draws the satchel radio (a second) and presses the detonate button.
- **Snark**, 150–800 units away with a clear line, not in water nor at an enemy far above: thrown at the enemy when
  there is room in front, as the game requires.
- **All the snarks** at an enemy in sight 60–200 units away, when the bot carries two or more and has 50 health or more:
  the bot holds the attack down at the enemy and the game lets one go every 0.3 s while there is room in front (not with
  the enemy right against it). They swarm the enemy and bite it where it stands. The chance is 0.35 per weighing, times
  the skill's `throw_rate`. Then the bot runs from the swarm for 2 s before it turns: a snark bites its owner too.

**Tripmines.** When quiet (no enemy for 5 s) and walking along a corridor at most 300 units wide, now and then (at
most once in 20–30 s) the bot lays a mine on the nearer wall within 90 units, so its beam runs across the corridor;
never within 256 units of a spawn point nor within 96 of another mine. A known mine, its own or anyone's, 400–1200
units away is shot (with the 357, the glock, the MP5 or the gauss) when an enemy is within 140 units of it; an
enemy's mine ahead of a quiet bot is shot to clear the way. Shooting a mine credits the shooter.

**Dodging.** A grenade the bot sees coming down near it (its own too), an MP5 grenade about to land, a rocket passing
within 160 units or an enemy satchel lying close make it run from the blast for half a second (away from it: yapb's
sign was wrong), checking the ground for ledges and taking a side when straight away is a drop.

**Snarks.** Someone else's snark within 300 units, or the bot's own coming back at it within 250 (a snark bites its
owner too), is run from: shooting at a small hopping snark wastes time, a bot on the run outpaces it, and its life is
short. The run takes the legs only, so the bot keeps fighting. With the egon in hand the bot burns the snark instead,
when no player in sight is closer than 300 units.

**Tripmine beams** the bot knows of (its own and those it has seen) keep its paths off them, and any move that would
take it into one (strafing and dodging included) is stopped.

**Game DLLs differ** in how satchels and grenades are worked: in the classic SDK the primary attack sets off the
satchels that are out and a grenade thrown level leaves at 400 units per second; since Valve's 2023 update and in
BugfixedHL-Rebased the secondary attack sets them off and the grenade leaves at 650. BugfixedHL is told by its cvars,
any other DLL is taken to play by the 2023 update (hlsdk-portable does), and `game.dll` in
`config/lambdabots.yaml` can name it. `lb selftest` checks it on a live server with one bot (satchel buttons,
crossbow zoom, grenade speed; it needs `sv_cheats 1` to hand out the weapons) and corrects the session. In a game,
the first bot to set its satchels off checks their buttons for every bot: when the detonate button throws another
satchel instead, it presses the other one, and the server's satchel buttons are that way from then on. What was
found is kept over map changes; `lb compat` shows the buttons and who checked them.

## Priorities

Behavior asks for what it wants on five channels (look, movement, stance, weapon, use key), and the highest
priority on each wins:
- **Traversal (90):** jumps and ladders on the path.
- **Protocol (85):** a weapon's own sequence: a charging gauss, a pulled pin, a throw, a mine placed, satchels set
  off, a rocket guided. A shot at an enemy never breaks it.
- **Threat (70):** aiming and firing at an enemy in sight, turning toward damage, dodging a blast.
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
- **A lost enemy** is watched for where it would come into view (see *Looking for a lost enemy*), for up to 8 s.
- **Danger:** a bot with no enemy in sight for 4 s, at a place where bots got hurt before, glances where that came
  from, as it glances at sounds.
- A point right above or below the eyes (closer than 16 units across) gives no direction: the view holds still
  instead of spinning to it, unless it is an enemy.

## Inspecting

`lb brain [name|#userid]` prints for each bot:
- the current goal, its rank, weight and how long it has been held;
- the candidates of the last decision;
- what the goal is doing (searching from where, hiding, holding a spot, waiting for an item, laying a trap), the mood
  against the personality's own, the goals taken so far, the goal changes (those before a hold ran out and those with
  no fight in them) and the changes of the enemy aimed at while the first was still in sight, and what came of the
  goals: sounds seen about, covers found, spots held, items waited for, traps laid;
- the target and the weapon choice;
- the weapon protocols: what runs now, throws, launched grenades, mines, detonations, gauss charges fired and dumped,
  dodges, and failures by reason;
- which priority owns each channel;
- reaction times: from the first glimpse of an enemy, and from recognizing it, to the first shot at it.

`lb map [spots|mines|danger]` prints what the bots know of the map: how many places see each other, the chokepoints,
the spots to hold, the walls for tripmines and where the bots got hurt most.

`lb list` shows every bot's goal. With telemetry on, the `frame` messages carry every bot's goal, candidates,
target and firing state. The observer (`tools/observer`) colors bots by goal and shows the candidates of the
selected bot.

For weapon tests on a stand server started with `sv_cheats 1`:
- `lb weapons <weapon>… give` lets the bots use only those weapons (and the crowbar) and hands them out on every
  spawn; `lb weapons all` lifts it. `scripts/stand/weapon-scenarios.sh` runs every weapon on its own this way.
- `lb stats [reset]` counts, per weapon, the rounds fired and the damage they did by distance (the hit rate):
  bullets are credited to the bot standing where the damage came from, bolts, rockets, grenades, satchels, mines,
  snarks and hornets to whoever threw or fired the one seen there. The crossbow's zoomed shots and the MP5's grenades
  have rows of their own. It also counts what the bots' own explosives did to them, and kills and suicides from the
  kill feed.
- `lb selftest` checks the game DLL's weapon rules with one bot while the others stand still.
