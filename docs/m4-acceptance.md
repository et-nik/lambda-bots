# M4 acceptance: the arsenal (M4.1), knowledge, goals and styles (M4.2)

State as of 2026-09-28, after sub-stages M4.1, the arsenal, and M4.2, knowledge, goals and styles. Still to come:
the tricks (M4.3) and the M4 acceptance runs. Test stand: Xash3D FWGS 0.21 (arm64) + Metamod-FWGS + hlsdk-portable,
macOS, crossfire at 1000 fps, bots of the normal preset (skill 43–61); the feedback round after M4.2 ran on dm_snow, a
small map where a run takes a minute or two. The ReHLDS server with BugfixedHL has run neither sub-stage yet.

## Results against the plan's criteria

| Criterion                                | Status | How it was checked                                                          |
|------------------------------------------|--------|-----------------------------------------------------------------------------|
| A scenario for every weapon              | yes    | `scripts/stand/weapon-scenarios.sh`: each weapon on its own, table below    |
| Deaths by own hand an hour below a limit | yes    | M4.2's last mixed game: 0.7 a bot-hour (one snark); the games before, below |
| Accuracy tables                          | yes    | `lb stats`: hit rate by distance for every weapon and fire mode             |
| The game DLL's weapon rules              | partly | `lb selftest` on hlsdk-portable; the classic SDK in tests, not run live     |
| Target switches ≤ 6 a minute             | yes    | M4.2's last mixed game: 1.2–3.3 a minute a bot (7.8 before the fix below)   |
| Tricks, the 60-minute run                | later  | M4.3 and the M4 acceptance runs                                             |

The plan leaves the limit on deaths by own hand open. Here it is set at one per bot-hour in a mixed game. The mixed
games of the day came to 0.75 and 1.5 a bot-hour (two grenades and one charged gauss shot), then 2.25 (three snarks,
with the first snark barrages; since fixed) and none in the last.

## The weapons

What the bots do with every weapon is in `docs/behavior.md` (*Fighting*, *Explosives*). In short:
- Every gun is scored by the damage a second it is expected to deal at the target's distance, from the server's
  damage cvars, the spread and the bot's own aim error. No weapon is taken whose blast would reach the bot.
- Secondary attacks where they pay: the glock's rapid fire up close, the shotgun's two barrels, hornet darts, the
  MP5's grenade launcher.
- Weapon protocols, which advance on what the game reports:
  - the gauss charge;
  - the crossbow's snap scope;
  - grenades, satchels and snarks, and setting satchels off;
  - tripmines, and shooting mines;
  - the MP5's grenade launcher.
- Projectiles, mines and explosions are seen and heard honestly. The bots dodge grenades and rockets, keep off known
  tripmine beams and shoot snarks.
- Wall chargers: the `charger` goal (all 40 chargers of the 12 standard maps have a spot to use them from).

## The game DLL's rules

`lb selftest` with one bot on the stand's hlsdk-portable:
- the satchel's primary attack throws another charge while one is out, and the secondary attack sets them off;
- the crossbow's scope narrows the view to 20°;
- a grenade thrown level leaves at 647 units per second.

So hlsdk-portable plays by the 2023 update's rules, as BugfixedHL-Rebased does. The classic SDK's rules differ: the
secondary attack always throws a satchel and the primary sets them off, and a grenade leaves at 400 units per second.
Since the feedback after M4.2 the bots take any DLL but BugfixedHL-Rebased (told by its cvars) to throw grenades by the
2023 rules and to work satchels the classic way, as the production server does; `game.dll` in
`config/lambdabots.yaml` can name it. The bots check the satchel buttons as they use them (see *Satchels, snarks and
going for the enemy*): on the stand the first bot to throw a satchel found hlsdk-portable's buttons within half a
minute of the map's start, in every run.

In multiplayer the game sets explosive damage itself, whatever `skill.cfg` says:

| Explosive          | Damage | Blast radius |
|--------------------|--------|--------------|
| hand grenade, M203 | 100    | 250          |
| rocket, satchel    | 120    | 300          |
| tripmine           | 150    | 375          |

## Every weapon on its own

Each set 2.5 minutes, 8 bots, crossfire. The bots may use only these weapons and the crowbar, and get them on every
spawn. "Kills with it" counts the set's weapons in the kill feed. The rest of the kills are crowbar kills, when
explosives run out or the enemy is too close for them.

The hit rate is the damage dealt over the rounds fired times the damage of a hit. For explosives it is the share of
full damage a round dealt; every snark bite counts, so snarks reach 100%. Damage is credited to the bot standing
where the game reports it came from (bullets). For a projectile it goes to whoever threw or fired the one seen there.
"Own blast" is damage bots took from their own explosives.

| Set            | Kills with it (all kills) | A minute | By own hand | Own blast damage | Rounds and hit rate <300 / 300–800 / 800–1500 units |
|----------------|---------------------------|----------|-------------|------------------|-----------------------------------------------------|
| crowbar        | 61 (61)                   | 24.4     | 0           | 10               | —                                                   |
| glock          | 20 (20)                   | 8.0      | 0           | 0                | 1208: 24% / 16% / 12%                               |
| python         | 34 (34)                   | 13.6     | 0           | 0                | 525: 27% / 14% / 10%                                |
| mp5            | 50 (50)                   | 20.0     | 0           | 0                | 2785: 19% / 10% / 11%; grenades 61: — / 27% / —     |
| shotgun        | 45 (45)                   | 18.0     | 0           | 0                | 559: 38% / 11% / 8%                                 |
| crossbow       | 42 (50)                   | 16.8     | 0           | 0                | scoped 130: 0% / 27% / 100%; bolts 15: 25% / — / —  |
| rpg            | 24 (51)                   | 9.6      | 1           | 61               | 49: — / 47% / 100%                                  |
| gauss          | 51 (54)                   | 20.4     | 0           | 0                | 2262 cells: 72% / 39% / 40%                         |
| egon           | 64 (78)                   | 25.6     | 0           | 0                | 840: 61% / 40% / 38%                                |
| hornetgun      | 58 (61)                   | 23.2     | 0           | 20               | 1178: 37% / 59% / 63%                               |
| handgrenade    | 4 (52)                    | 1.6      | 0           | 124              | 56: 20% / 10% / 8%                                  |
| satchel        | 7 (42)                    | 2.8      | 1           | 315              | 81: 12% / 12% / 100%                                |
| tripmine+glock | 16 (16)                   | 6.4      | 0           | 0                | glock 1155: 18% / 12% / 15%; mines 1                |
| snark          | 19 (24)                   | 7.6      | 3           | 276              | 115: 57% / 100% / 100%                              |

- **Gauss:** each bot fired 13–26 charged shots against 4–13 rolls for plain fire, 70% of its choices (the normal
  preset's `gauss_charge` is 0.75). No bot died by its own shot.
- **Crossbow:** 130 of 145 shots went through the scope. After a miss the bot stays zoomed and fires again. Of the 105
  times the scope went on, it came off after the kill 56 times, with the target out of sight for a second 33 times,
  with the view not on the target in time 9 times, the target too close 5 times and the clip empty twice.
- **Throws:** in their sets each bot threw 5–10 grenades, 1–6 satchels and 7–15 snarks in 2.5 minutes.
- **Tripmines:** laid only when quiet and walking a corridor, 3 in the set. Most kills are the glock's.

## A mixed game

10 minutes, 8 bots, crossfire, every weapon allowed, the build before the satchel and snark changes below. The two
mixed games since are under the criteria above.

| Measure                       | Value                                               |
|-------------------------------|-----------------------------------------------------|
| Kills by bots                 | 141, 14.1 a minute                                  |
| Deaths by own hand            | 2 (a grenade, a charged gauss shot): 1.5 a bot-hour |
| Damage from own explosives    | 40 (snark bites)                                    |
| Core time a frame at 1000 fps | average 70 µs; the last 4 s: p50 86 µs, p99 234 µs  |

Kills by weapon: crossbow 33, glock 28, gauss 22, MP5 20, egon 9, grenades 8 (thrown and launched), hornet gun 6,
shotgun 5, RPG 4, the rest 1–3 each.

| Weapon       | Rounds     | <300 | 300–800 | 800–1500 |
|--------------|------------|------|---------|----------|
| glock        | 2049       | 19%  | 15%     | 13%      |
| 357          | 100        | 12%  | 12%     | 9%       |
| MP5          | 1301       | 19%  | 13%     | 7%       |
| MP5 grenades | 19         | —    | 42%     | —        |
| crossbow     | 154 scoped | 100% | 20%     | 24%      |
| shotgun      | 35         | 31%  | 8%      | —        |
| RPG          | 19         | —    | 25%     | 34%      |
| gauss        | 980 cells  | 35%  | 48%     | 32%      |
| egon         | 121        | 36%  | 40%     | 73%      |
| hornet gun   | 148        | 36%  | 59%     | 100%     |

The gauss's rate counts a cell as 10 damage; charged shots deal more a cell.

The game before, on the build without the crossbow's follow-up shots: 142 kills, 1 death by own hand (a grenade), core
time p50 56 µs and p99 167 µs. Throwing depends on what the bots pick up: 7 grenades, 1 satchel and 16 snarks in that
game, 36 grenades, 4 satchels and 23 snarks in the one before it; a 12.7-minute game before explosives were topped up
saw 8 grenades, 8 satchels and 26 snarks.

## After the stand feedback

The first per-weapon run (earlier the same day) led to these changes:

| What                  | Before                                                         | After                                                                                                                   |
|-----------------------|----------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------|
| Crossbow              | long aim through the scope, many misses                        | snap scope in about a second; scoped shots from 250 units (was 600); 33 kills a set (was 30), no failed scopes (was 17) |
| Gauss                 | plain primary attack most of the time                          | charged in 70–75% of its choices; plain shots only after a roll for them or when no charge can start                    |
| Grenades and satchels | only at enemies out of sight; 22 grenades thrown, 1 kill a set | at enemies in sight too, with a lead; about 60 grenades, 4–5 kills a set; bots top up explosives                        |
| Glock                 | rapid fire only under 150 units                                | rapid fire where it lands more bullets a second than aimed clicks, given the bot's aim and clicking speed               |
| MP5 grenades          | 2–7 deaths by own hand a set                                   | none in the last two runs: the arc is looked along again at the moment of firing                                        |
| Rockets               | 2 deaths by own hand a set, 237 own blast damage               | 1 death and 61: launch line checked from where the rocket leaves, guided on its own target, no closing in               |
| Gauss self-kills      | 2 in a 10-minute mixed game                                    | none in the gauss set, one in the mixed game above: the charge is kept small for the nearest wall along the line        |
| Crossbow after a miss | the scope came off after every shot, hit or miss               | stays zoomed and fires again until the kill, keeping to its target; 37–46 crossbow kills a set (was 30–33)              |

## Satchels and snarks

After the report, the stand and the production server showed that bots hardly ever set their satchels off, that they
fought snarks with any gun, and that they did not empty their snarks at an enemy close by. The changes:

| What                 | Before                                                              | After                                                                                                                             |
|----------------------|---------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------|
| Setting satchels off | only with an enemy within 160 units of a charge, the bot 324 away   | also once the enemy they were thrown at is out of sight by them, and after 8–15 s with nobody in sight; an enemy within 200 units |
| Throwing satchels    | one at a time                                                       | a pile of 2–4 at once, then out of the blast; or one from a jump at an enemy in sight, set off as it comes by                     |
| The satchel buttons  | learned by each bot, lost at a map change                           | checked by the first bot to set its satchels off, for every bot, kept over map changes; `lb compat` shows them                    |
| Snarks near          | shot with the gun in hand (or a rocket), not with a player in sight | run from; burnt only with the egon in hand                                                                                        |
| An enemy close by    | one snark every 3–6 s                                               | all the snarks at once (60–200 units, 50 health or more), then away from the swarm                                                |

The satchel set (crowbar and satchels, 2.5 min) went from 17–31 satchels thrown, 9–12 set off and 2–9 kills to 73–91
thrown, 20–27 set off and 7–18 kills in five runs, with one death by own hand in all. Why they went off, in one run:
an enemy by them 16 times, the enemy out of sight by them 8, lying long enough 7, in flight by the enemy once. The
jump throw is the hard one: of 14 tries in one run 2 went off by the enemy; the satchel lands short of a moving
enemy, or the bot, carried on by its run-up, is still too close to it (a bot with 70 health or more now takes up to
a quarter of the blast rather than let it pass, and backs off for a second after the throw).

On the stand the check of the satchel buttons came from the first bot to set its satchels off in every run: "the
secondary attack sets the charges off, the primary throws another (checked by thunder in the game)".

The snark set (crowbar and snarks) went from 85–93 snarks thrown and 11–14 kills to 115–191 thrown and 19–42 kills.
With nothing but a crowbar and fifteen snarks each, the swarms also turn on their owners: 3 deaths by own hand in the
last run, 11 before the barrage was kept to 200 units with a longer run away. With the egon as well, the egon burns
the snarks near: 1 death by own hand, 53 egon kills. In the mixed game: none.

## Found on the stand and fixed

- **hlsdk-portable names a flying bolt `crossbow_bolt`**, not the SDK's `bolt`. Bolts went unseen and unscored. Both
  names are tracked now.
- **A held plain gauss shot kept the charge from starting.** Each plain shot stops the gun for a fifth of a second,
  and the charge only started in the gaps between them.
- **Charged gauss shots killed their shooter.** A charged beam goes through players and bursts where it punches out
  of a wall, 1.75 times its damage around. With a wall close behind a close target, the burst reached the bot. A
  charge is now kept small enough for the nearest wall along the line of fire, or not started.
- **The crossbow's scope toggle was pressed too early.** The weapon data comes 50 times a second and may miss a
  toggle; the bot now also waits a second from the zoom it saw change.
- **The bot gave up zoomed shots too soon.** The view follows a strafing target a few degrees behind and catches it
  mostly as it turns; a scoped shot waited only 0.4 s for that. It waits a second now, and the bot keeps to the target
  it is zoomed on.
- **Rockets burst next to the shooter.**
  - A rocket leaves 16 units ahead, 8 to the right and 8 below the eye, and goes for the target's feet. The line of
    fire was checked from the eye to the chest.
  - The laser that guides the rocket followed a new, closer target.
  - The bot ran into its own blast while the rocket flew.
- **MP5 grenades burst at the muzzle.** The bot fired a second after deciding. By then it had strafed next to a wall,
  or the target or someone else had come close.
- **Satchels were set off 250 units away.** The blast of the multiplayer satchel reaches 300.
- **Snarks were shot with whatever the bot fought with,** a rocket launcher included, and never with a player in
  sight. The bot now runs from them, and burns them only with the egon in hand.
- **Satchels were thrown again right after they went off,** while the game was still in its half-second reload. The
  game leaves it only on a frame with both buttons up, so a press held from that moment threw nothing.
- **The satchel buttons learned in a game were forgotten at the next map,** and so was the verdict of `lb selftest`.
- **Explosives were not topped up.** An owned grenade, satchel or snark was worth nothing to pick up, so bots carried
  one handful a life.
- **`lb stats` missed hits and throws.**
  - Bolts, rockets and grenades hit where no bot stands, so their hits went uncredited. They are credited to the
    projectile's owner now.
  - Thrown explosives were counted only while in hand, and the bot switches back at once. They are counted by what
    it carries now.
  - The scope's shots and the MP5's grenades now have rows of their own.

## Knowledge, goals and styles (M4.2)

What the bots now know and do (see `docs/behavior.md` for the details):
- **The map, worked out once:** who sees whom between the graph's places, where players pass, chokepoints, spots to
  hold (overwatch and ambush) with the directions to watch, walls for tripmines across corridors and round corners,
  and cover from a threat. `lb map` shows it.
- **What the bots learn by playing:** where they got hurt and died and where from (kept per map in
  `data/experience/<map>.json`), and item respawn times they timed themselves (kept for the server in
  `data/learned/respawn.json`).
- **Lost enemies:** spread over the places they could have reached, less the places in sight since; the bot searches
  from the place that sees most of them and watches where they would come into view.
- **New goals:** seeing about a sound, holding a spot, waiting by an item about to come back, laying a trap (a tripmine
  on a mine spot, satchels at a chokepoint watched from an ambush spot); backing off goes to cover.
- **Moods:** aggression and fear sway with fights, kills and damage around the personality's own.
- **Styles:** goal weights for the new goals, the guns each style favours and how readily it throws; a personality's
  favourite weapons count on top.
- **Hard and expert bots** shoot the gauss through thin walls at an enemy lost behind one.
- Items are heard taken and coming back; a calm bot glances where the damage at its place used to come from.

### The map's tactics

Worked out by `lb-cli nav tactics <map.bsp>` from the generated graph (on the server in the map loader, the same way):

| Map           | Places | Pairs in sight | Chokepoints | Spots to hold | Tripmine spots | ms (8 threads) |
|---------------|--------|----------------|-------------|---------------|----------------|----------------|
| boot_camp     | 2826   | 3.4%           | 67          | 24            | 48             | 25             |
| bounce        | 782    | 21.3%          | 12          | 20            | 32             | 10             |
| crossfire     | 890    | 11.8%          | 38          | 24            | 48             | 7              |
| datacore      | 581    | 9.4%           | 22          | 17            | 23             | 2              |
| frenzy        | 492    | 12.6%          | 17          | 19            | 35             | 2              |
| gasworks      | 1418   | 9.2%           | 26          | 24            | 48             | 13             |
| lambda_bunker | 707    | 10.5%          | 14          | 20            | 25             | 3              |
| rapidcore     | 437    | 10.3%          | 22          | 20            | 44             | 2              |
| snark_pit     | 399    | 10.7%          | 19          | 18            | 42             | 1              |
| stalkyard     | 703    | 16.4%          | 25          | 19            | 14             | 4              |
| subtransit    | 898    | 8.2%           | 39          | 24            | 48             | 6              |
| undertow      | 784    | 12.9%          | 29          | 24            | 48             | 4              |

It takes so little that nothing is kept on disk: the tactics are worked out anew with every map load.

### The styles on the stand

`scripts/stand/style-scenarios.sh` fills the server with bots of one style (trappers get tripmines and satchels on
every spawn) for 3 minutes each. With 8 bots on crossfire someone is always in a fight, and holding a spot or waiting
for an item is cut short almost every time; with 3 bots the goals come through:

| 3 bots, 3 min | Kills | By own hand | Spots held | Items waited for | Traps laid | Target changes a minute | Goal changes a minute (no fight in them) |
|---------------|-------|-------------|------------|------------------|------------|-------------------------|------------------------------------------|
| snipers       | 10    | 1           | 5          | 1                | 0          | 0–0.7                   | 12–15 (1.7–3.7)                          |
| controllers   | 9     | 0           | 0          | 3                | 0          | 0.3–2.0                 | 13–15 (3.3–5.3)                          |
| trappers      | 4     | 1           | 6          | 0                | 9          | 0                       | 9–10 (6–8.7)                             |

In the 8-bot runs (on a build before the fixes below) rushers never backed off to cover and went to see about
sounds most, snipers backed off to cover most (12–35 times a bot in 3 minutes), and every style changed its target
0.3–5.7 times a minute.

**A mixed game** on the final build: 10.5 minutes, 8 bots on crossfire (three balanced, three trappers, a controller,
a sniper), every weapon allowed:

| Measure                         | Value                                                          |
|---------------------------------|----------------------------------------------------------------|
| Kills by bots                   | 155, 14.7 a minute                                             |
| Deaths by own hand              | 1 (a snark): 0.7 a bot-hour                                    |
| Target changes a minute, a bot  | 1.2–3.3                                                        |
| Goal changes a minute, a bot    | 20–28, of them 2.1–5.5 with no fight in them                   |
| Spots held, items waited for    | 9 (the sniper 4), 1                                            |
| Sounds seen about, covers found | 0–10 and 1–20 a bot                                            |
| Core time a frame at 1000 fps   | average 78 µs; the last 4 s: p50 62 µs, p95 135 µs, p99 213 µs |

Kills by weapon: crossbow 41, glock 32, gauss 25, MP5 16, RPG 10, egon 8, the rest 3–7 each. The six mixed games of
the day before it, on earlier builds, came to 13.7–15.5 kills a minute and 1.2–2.8 deaths by own hand a bot-hour, most
of them the gauss's (see below). What the map knowledge costs a bot, measured on crossfire
(`examples/map_costs.rs`): a look at the places in sight 2 µs, a lost enemy's spread worked out again 12.5 µs, a cover
query 27 µs; on boot_camp (2826 places) 2, 20 and 40 µs.

**Replay.** A recording of 120 s of an 8-bot game on crossfire replays with every one of its 88,849 bot commands the
same (`lb-cli replay`), the map's tactics included.

### Found on the stand and fixed

- **The map's tactics differed between a graph read back from the cache and one just made.** Making the graph moves
  doors and lifts about; a replay makes the graph again, and its bots saw other places from each other. The tactics
  are worked out with every mover where the map starts it now.
- **Goals picked and dropped on every frame.** An item or a charger that would be back by the time the bot got there
  was chosen, then given up because it was not back yet, then chosen again. Both now reckon with the way there as the
  decision does, and a spot just reached rests a moment.
- **Nobody held a spot.** Holding one wanted 50 health counting armor as the goals do (a third of health plus twice
  the armor), which a bot without armor never has.
- **Satchel traps fell short.** A satchel flies some 200 units; they were thrown from the ambush spot, up to 650 units
  from the chokepoint, and some landed by the thrower. The bot now goes up to the chokepoint, throws, and watches from
  a spot 350 units or more away.
- **Laying a mine, the bot stood in its beam.** It steps along the wall out of the beam's way before the mine arms.
- **A sound, a place to see it from, straight through a wall.** Places were compared by straight distance; a place
  across a wall looked near. They are compared by the run there now.
- **A gauss charge that had to go was dumped with no look at the walls,** back the way the bot went and a little down.
  It goes level along the way whose first wall is farthest now, with no drop behind within the recoil's throw.
- **A missed charged gauss shot killed its shooter.** The stand's DLL (hlsdk-portable) works the gauss as vanilla
  HLDM does: a beam that fails to punch through a wall met square starts over from the gun with the shooter no
  longer left out. A shot that missed a target far away, with a thick wall behind it, came back at full damage (the
  log now tells: `killed itself with the gauss`, with the charge, the distance and the walls). Where the DLL does it
  (`mp_selfgauss 1`, and every DLL without that cvar; BugfixedHL's default 0 does not), the charge is not let go while
  the first wall along the view, at any distance, would stop it, nor dumped toward one.
- **Targets flicked between two enemies.** Facing and firing flicker as enemies strafe; the target in sight is kept
  for a second and another takes over only when 1.6 times as pressing (was 1.3).

## Satchels, snarks and going for the enemy (feedback after M4.2)

On the production server the bots hardly ever set their satchels off, and never once they had thrown several around
the map; they spared their snarks; they did not always go for the enemy, as if waiting for something; and with a
throwable in hand they kept aiming at the enemy, which got in the way of what they were doing.

**Why the satchels stayed.** The bots took any DLL but BugfixedHL-Rebased to work satchels by the 2023 update, setting
them off with the secondary attack. The production server works them the classic way, as yapb played: there the
secondary attack throws. A detonation press threw another satchel, or did nothing with the pocket empty. The check
that should have caught it gave up too early: after a throw the game holds the primary attack back for a second,
and the check waited 0.6 s for the charges to go off. Nothing was learned, and the satchels scattered over the map
were never set off.

| What                 | Before                                                                                                  | After                                                                                                                                                                                                            |
|----------------------|---------------------------------------------------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Satchel buttons      | the 2023 update's: the primary throws, the secondary sets them off                                      | the classic ones: the secondary always throws, the primary sets them off; BugfixedHL-Rebased (told by its cvars) keeps its own                                                                                   |
| Checking the buttons | a detonation press that threw a satchel, given up after 0.6 s                                           | every press: a detonation that throws or does nothing, a throw that does nothing or sets the charges off; a button is pressed once the game takes it                                                             |
| When satchels go off | an enemy within 200 units of a charge; the enemy they were thrown at out of sight by them; 8–15 s lying | 40 damage to an enemy from the blasts together, where it will be when they go off, or one coming into them; someone heard by them; the bot about to die; an enemy seen in their blast a moment ago; 8–15 s lying |
| The satchel radio    | drawn (a second) when the charges were to go off                                                        | up after a pile thrown at an enemy and while a trap is watched, watching the charges, and pressed the moment an enemy is in the blast                                                                            |
| Snarks               | 0.25 a weighing at an enemy in sight, 0.35 out of sight; 150–800 units; the next throw 3–6 s later      | 0.6; 200–1000 units; a stream of one to three at an enemy in sight; the next 1–2.5 s later, and no closing in for 3 s                                                                                            |
| The snark barrage    | 0.35 a weighing                                                                                         | 0.5                                                                                                                                                                                                              |
| Fighting far off     | strafing where it stands; only skilled bots feeling strong drifted in                                   | a run at the enemy along the path, strafing, when further off than the weapon in hand does well at (shotgun 350, glock and MP5 700, …) and health × aggression is 30 or more                                     |
| A lost enemy         | hunted for about 2 s                                                                                    | about 6 s; a search place is looked from for half a second before the next                                                                                                                                       |
| Standing still       | spots 8–15 s (ambush 10–20 s), a sound looked at for 1.5 s, a satchel trap watched 20–30 s              | 6–10 s (8–14 s), 0.8 s, 12–20 s; balanced, controller and trapper bots hold spots and wait for items less                                                                                                        |
| A throwable in hand  | aimed at the enemy                                                                                      | not aimed: throws turn to their own arcs, the radio watches the charges, a satchel in flight is watched with its enemy                                                                                           |

On the stand (hlsdk-portable, which works satchels the 2023 way) the first bot to throw a satchel found the buttons
within half a minute of the map's start in every run, by the press that did nothing: "satchel buttons checked by
bisTEK in the game: the primary attack throws, the secondary sets the charges off".

The sets on dm_snow, 8 bots, 90 s each:
- **Satchels** (the crowbar and satchels): 87 thrown, 24 set off, 8 kills, no death by own hand, 81 damage from the
  bots' own blasts. Why they went off: someone heard by them 15 times, an enemy seen by them a moment ago 5, in flight
  by the enemy 3, an enemy in their blast once.
- **Snarks** (the crowbar and snarks): 293 thrown, 38 kills, 10 deaths by own hand: with nothing but a crowbar and
  fifteen snarks each the swarms turn on their owners.

A mixed game on dm_snow, 5 minutes, 8 bots, every weapon, the build before the feedback and after it (the
personalities are drawn afresh for each game; nobody picked up snarks or grenades on this map):

| In the game                                        | Before      | After      |
|----------------------------------------------------|-------------|------------|
| Kills a minute                                     | 21.6        | 23.6       |
| Deaths by own hand                                 | 0           | 0          |
| Share of the time a bot stood still, out of fights | 19% (14–31) | 14% (9–19) |
| Share of the time a bot stood still, in fights     | 9% (6–12)   | 8% (4–19)  |
| Hunts taken                                        | 159         | 238        |
| Satchels thrown, set off                           | 1, 1        | 4, 3       |
| Core time p99                                      | 161 µs      | 180 µs     |

### Found on the stand and fixed

- **The radio came up and went away again**, up to 40 times a bot in 2.5 minutes: an enemy by the satchels, judged
  1.5 s ahead, had walked out of their blast and counted as a threat away from them. Now an enemy in or coming into
  the blast is never such a threat, and the radio does not come up with a real threat near.
- **Satchels went off half a second after the throw**, still in the air close to the bot (it took them for lying where
  they would land): 654 damage from the bots' own blasts in a 150 s satchel set on crossfire, 3 deaths by own hand.
  None goes off within a second of a throw now, but one flying at an enemy, which is watched.
- **A satchel flying by an enemy** was set off with the bot out of the blast of each charge rather than of all of
  them together.
- **A wider snark barrage** (300 units, 40 health) turned the swarms on their owners: 19 deaths by own hand in a snark
  set. Back to 200 units and 50 health.

## Tools

- `lb weapons <weapon>… give`, `lb weapons all`: a scenario's weapons, handed out on every spawn (needs
  `sv_cheats 1`).
- `lb stats [reset]`: rounds, hit rate by distance, damage and kills per weapon and fire mode; own blast damage.
- `lb selftest [name]`: the DLL's satchel buttons, crossbow zoom and grenade speed, with one bot (`sv_cheats 1`).
- `lb compat` shows the satchel buttons the bots use and who checked them in a game.
- `lb brain` shows each bot's weapon protocol and counts:
  - throws and snark barrages, satchels set off and why, scoped shots and zooms with why each zoom ended, charged
    and plain gauss rolls, runs from snarks;
  - time no charge could start for a drop behind or a wall ahead;
  - dodges and failures;
  - the explosives it carries.
- `scripts/stand/weapon-scenarios.sh [--seconds 150] [set…]`: the per-weapon runs above, reports in
  `stand-runs/current/weapons/`.
- Own blast incidents are logged with the distances involved (`own blast:` in `logs/lambdabots.*.log`), and a death
  by the bot's own gauss with its last charge (`killed itself with the gauss`).
- `lb map [spots|mines|danger]`: the map's tactics and where the bots got hurt.
- `lb brain` also shows each bot's goal task, mood, goals taken, goal and target changes, what came of the new
  goals, and the share of the time it stood still out of fights and in them; `lb profile <name>` shows a personality's goal weights, favourite weapons and whether it shoots the gauss
  through walls.
- `scripts/stand/style-scenarios.sh [--seconds 180] [--bots 8] [style…]`: the per-style runs above, reports in
  `stand-runs/current/styles/`.

## Not done yet

- **A satchel from a jump** goes off by the enemy about one time in seven.
- **A bot's own snarks** still turn on it, most with only a crowbar and a pocketful of snarks: 10 deaths by own hand
  in the snark set on dm_snow. Left as it is.
- **A tripmine's planter** may still be near when an enemy trips it (two deaths in four runs of its set).
- **Holding spots and waiting for items** rarely come through in a crowded game: with 8 bots on crossfire a fight
  cuts them short almost every time.
- **Spawn protection** and the rest of the plan's M4 knowledge (the prior over spawn points after a death, TDM sharing)
  are later stages.
- **Deaths by own hand** vary from game to game: 0.7 a bot-hour in M4.2's last mixed game, 1.2–2.8 in the games
  before the gauss fix. One charged gauss shot still killed its shooter in the gauss set after it (fired in a jump,
  looking a little up, no wall near along the view): a beam glancing off more than one surface may come back.
- **Shots through walls** came 3 times in 3 minutes of four expert bots with the gauss only (55 kills, no death by own
  hand); whether they hit is not counted yet.
- **Game DLLs:** BugfixedHL on ReHLDS and the classic SDK's rules have not run live; the classic satchel buttons are
  checked against a model of the three DLLs in the tests.
