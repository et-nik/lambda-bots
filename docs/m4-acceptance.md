# M4 acceptance: the arsenal (M4.1), knowledge, goals and styles (M4.2), tricks (M4.3)

State as of 2026-09-29, after sub-stages M4.1, the arsenal, M4.2, knowledge, goals and styles, and M4.3, the tricks,
with long jumps by skill after its feedback, and the 60-minute run. Test stand: Xash3D FWGS 0.21 (arm64) + Metamod-FWGS +
hlsdk-portable, macOS, crossfire at 1000 fps, bots of the normal preset (skill 43–61); the feedback round after M4.2
and the tricks ran on dm_snow, a small map where a run takes a minute or two, and stalkyard. The ReHLDS server with
BugfixedHL has run none of the sub-stages yet.

## Results against the plan's criteria

| Criterion                                | Status | How it was checked                                                          |
|------------------------------------------|--------|-----------------------------------------------------------------------------|
| A scenario for every weapon              | yes    | `scripts/stand/weapon-scenarios.sh`: each weapon on its own, table below    |
| Deaths by own hand an hour below a limit | yes    | M4.2's last mixed game: 0.7 a bot-hour (one snark); the games before, below |
| Accuracy tables                          | yes    | `lb stats`: hit rate by distance for every weapon and fire mode             |
| The game DLL's weapon rules              | partly | `lb selftest` on hlsdk-portable; the classic SDK in tests, not run live     |
| Target switches ≤ 6 a minute             | yes    | M4.2's last mixed game: 1.2–3.3 a minute a bot (7.8 before the fix below)   |
| Tricks succeed in ≥ 90% of scenarios     | yes    | long jumps 41/42 and boosts 349/352 offline, 52/52 boosts live; see M4.3    |
| The 60-minute run                        | yes    | 8 bots, five maps, 12 minutes each; see *The 60-minute run*                 |

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

## Tricks (M4.3)

What the bots do and how is in `docs/behavior.md` (*Tricks*) and `docs/navigation.md` (*Long jumps and gauss
boosts*). The owner's choices for this sub-stage:
- **Long jump:** links across gaps in the graph (only for a bot with the module), and long jumps along straight
  stretches of the way for speed, as yapb did, the landing checked by following the flight.
- **Gauss jump:** on the way somewhere far (the gauss in hand, as yapb), and gauss boost links in the graph onto
  ledges and across (the bot draws the gauss for one).
- **Long jump at the enemy:** wider and more often than yapb: 300–900 units, a will to close in of 20.
- **Skill and styles** as the plan says: from the normal preset up (long jumps on the way for everyone), with the
  styles' chances; the satchel from a jump goes by the same switch.
- **Health:** 60 to start a gauss jump, 40 left after its landing.
- **Switches:** `tricks` in `config/lambdabots.yaml` (`longjump`, `gauss_jump`, `gauss_boost`, `satchel_jump`, `grenade_jump`).
- **Stand:** dm_snow with the module and the gauss given on spawn, plus the offline courses.

**The graphs.** Long jump links are made only where every takeoff of the check lands (a long jump near its full reach
falls short when it takes off a little early): dm_snow has 1, crossfire 3, stalkyard 8. Gauss boosts go onto ledges
the graph reaches only by a way at least 1.3 times as costly: dm_snow 248 (its snowy rises are 88–192 units high),
crossfire 68, stalkyard 74. Both stages together take 90–120 ms of the generator's time on these maps (crossfire in
0.46 s all told).

**Offline** (`generated_course`, 12 standard maps, the bot with the module and a gauss; `obstacles`):

| Traversal                          | Result                                                          |
|------------------------------------|-----------------------------------------------------------------|
| long jump links                    | 41/42 (bounce: one lands short)                                 |
| gauss boost links                  | 349/352                                                         |
| a long jump across a gap           | made with the module; without it the bot goes round             |
| a gauss boost onto a 200-unit rise | made; without the gun there is no way                           |
| long jumps along a straight run    | 1800 units 15% faster at least than running                     |
| a gauss jump over a wall           | lands off the path nearer the goal, the way on is planned again |

**Live `lb nav test`** (one hard bot, the module and the gauss given, health and uranium topped up before each link):

| Map       | Links           | Result                                                                          |
|-----------|-----------------|---------------------------------------------------------------------------------|
| dm_snow   | 40 gauss boosts | 40/40, 3.0 s each                                                               |
| stalkyard | 20 gauss boosts | 12/12; 8 entries not reached (the way there fails on the map's jumps)           |
| stalkyard | 8 long jumps    | 6/7, 1 entry not reached; the failure (a run-up start a step below) fixed after |
| dm_snow   | 2 long jumps    | 1/2 before the run-up was reworked (see below)                                  |

**Live games on dm_snow**, 8 hard bots (a normal bot below skill 50 does no fighting tricks), 150 s a set
(`scripts/stand/tricks-scenarios.sh`; `longjump`: the module and the map's weapons; `plain`: the same without the
module):

| Set                       | Kills | Deaths by own hand | Long jumps on the way landed | Long jumps at enemies |
|---------------------------|-------|--------------------|------------------------------|-----------------------|
| longjump                  | 93    | 0                  | 25/26                        | 278                   |
| plain                     | 102   | 0                  | —                            | —                     |
| longjump                  | 93    | 0                  | 24/26                        | 254                   |
| plain                     | 90    | 1                  | —                            | —                     |
| longjump, snark rule      | 73    | 0                  | 25/25                        | 249                   |
| both (module, gauss only) | 121   | 0                  | 23/27                        | 145                   |

The long jumps that missed left the ground at full speed and were stopped in the air: by another bot in the way or,
in the gauss set, by the knock of a gauss hit. With the gauss alone and 8 bots, boosts hardly happen: the uranium
goes on the fight (a boost link wants 40, a gauss jump 30) and there are never two calm seconds; with one bot
alone the gauss jump on the way came 1 of 1 landed, and in the `both` set one was started and called off when an
enemy came into sight (its charge was held on for the enemy).

**A mixed game** on dm_snow, 5 minutes, 8 normal bots, no cheats (dm_snow has no long jump module, so the tricks there
are the gauss's), the build before M4.3 and after it:

| In the game                                        | Before | After  |
|----------------------------------------------------|--------|--------|
| Kills a minute                                     | 24.0   | 23.4   |
| Deaths by own hand                                 | 0      | 0      |
| Share of the time a bot stood still, out of fights | 16%    | 13%    |
| Share of the time a bot stood still, in fights     | 10%    | 7%     |
| Core time p99                                      | 158 µs | 179 µs |

### Found on the stand and fixed

- **A long jump link's bot walked off the ledge** while its view was still turning to the landing (the entry is
  often at the edge), or took off from a floor below the entry into the ledge's wall: 51/83 links offline. Now it
  stops at the start of the run-up behind the takeoff (only as far back as there is floor), turns there, runs and
  takes off within the window the check tried, on the entry's floor; a link is made only when every takeoff of the
  check lands: 41/42.
- **A trick's flight was lost** when the goal changed in the air (the path was replaced) or a fight began (nobody
  steered): it is flown to its end now whatever the brain does.
- **A gauss jump on the way was hardly ever found**: its landing had to be by a node of the path, and paths bend.
  Now any node along the flight's line the planner reckons two seconds nearer the goal will do; the way on is planned
  again after the landing.
- **A dumped charge killed its bot**: no level way was clear of the beam's burst on a wall (372 units off, the burst
  reaching 382). A dump goes straight up now when the sky or a high ceiling is farther than every wall around.
- **Snarks and leaps**: a leap at the enemy is not taken with a snark seen by the bot, the enemy or the landing.
- **Giving every weapon and its ammo** on every spawn filled dm_snow with ammo the bots could not take until the
  server ran out of entities; the trick sets use the map's weapons.

## Long jumps by skill (feedback after M4.3)

The owner's feedback: skilled bots long jumped too rarely; at the high levels they should long jump on straight
stretches almost always, on the way and at enemies, as good players get about by long jumps whenever they have the
module. On the stand a hard bot long jumped along its way about once in 46 seconds. The owner's choices:
- **A smooth scale:** a skill parameter `longjump` (noob 0.15, easy 0.35, normal 0.6, hard 0.9, expert 1.0), times
  the style's liking against a balanced bot's.
- **Bold long jumps on the way** from hard up: one after another, round corners, down drops, over short stretches
  (250–400 units), and onto a landing that hurts with more than 60 health (40 left after it).
- **In a fight:** at the enemy when closing in, and to dodge (aside or away, the view back on the enemy in the air).

What held long jumps on the way back: a roll of the style's chance every 8–12 s, a look every half second and 1.1 s
between long jumps, and a straight stretch whose every leg ran within 20° of the jump; the first leg, from the bot to
the next node, often did not. The stretch is now a corridor (the nodes within 24 units of the jump's line), and bold
long jumps look every 0.05 s for the node furthest along the path the flight comes down on (`docs/navigation.md`).

**Offline** (`longjumps`, 60 routes a map between random places at least 1000 units apart by the way, a bot without the
module, with long jumps (not bold) and with bold ones; long jumps a minute of the way, the share of the time in the
air, the route's time against running):

| Map       | Before: a minute | Not bold: a minute, time | Bold: a minute, in the air, time | Bold: missed |
|-----------|------------------|--------------------------|----------------------------------|--------------|
| dm_snow   | 11.2             | 11.0, 89%                | 29.5, 43%, 86%                   | 0/194        |
| crossfire | 3.5              | 4.2, 97%                 | 17.9, 27%, 88%                   | 2/189        |
| stalkyard | 2.0              | 4.1, 96%                 | 17.4, 25%, 88%                   | 8/182        |
| datacore  | —                | 5.9, 95%                 | 24.1, 37%, 87%                   | 4/264        |
| frenzy    | —                | 10.0, 92%                | 29.7, 43%, 84%                   | 1/240        |

A long jump flies about 0.8 s whatever its length (its rise is fixed), so a short one saves nothing over running; the
time saved comes from the long ones and from lining up quickly: 0.04–0.09 s from the check to the takeoff.

**Live games on dm_snow**, 8 bots, the module and the map's weapons, 150 s a set (`tricks-scenarios.sh --difficulty`;
`lb_difficulty hard` lets in skills 63–87, of which only 75 and up long jump boldly and dodge):

| Set               | Kills | Deaths by own hand | Long jumps on the way | At enemies | To dodge |
|-------------------|-------|--------------------|-----------------------|------------|----------|
| M4.3, hard        | 73–93 | 0                  | 25/26                 | 249–278    | —        |
| hard              | 73    | 1 (a rocket)       | 64/65                 | 57         | 88       |
| hard, no module   | 84    | 0                  | —                     | —          | —        |
| expert            | 93    | 0                  | 109/112               | 46         | 277      |
| expert, no module | 122   | 0                  | —                     | —          | —        |

A bold hard bot long jumped along its way 12–19 times in 150 s and dodged by a long jump 19–26 times, one below hard
1–3 times; an expert 10–20 times and 29–49. Fewer long jumps at enemies than before: a bot now leaps only when it
closes in (the weapon in hand does poorly this far off), not with a gun that does well where it is. With every bot
long jumping the games have fewer kills: a bot in the air is harder to hit, and one turning to dodge does not shoot.

Whether dodging by long jumps pays was checked head to head: 8 expert bots of one style and the same traits, all with
the module and bold long jumps on the way, four of them with `longjump_dodge` off in their personality's overrides,
10.5 minutes on dm_snow (kills and deaths from the server's log):

| Bots        | Long jumps on the way (a bot) | To dodge (a bot) | Kills | Deaths | Kills a death |
|-------------|-------------------------------|------------------|-------|--------|---------------|
| dodging     | 31–42 in the first 5.5 min    | 77–99            | 230   | 228    | 1.01          |
| not dodging | 19–35 in the first 5.5 min    | 0                | 258   | 260    | 0.99          |

Even: a bot dodging by long jumps dies about 12% less and kills about 11% less (it does not shoot while its view turns
along the jump). After the first 5.5 minutes the dodging bots were ahead, 1.11 to 0.92; the second half evened it out.

### Found on the stand and fixed

- **A long jump along the way that took off late overshot a near landing** or met what the flight had cleared from
  where the check stood (the takeoff was allowed 128 units on): 5–7% of bold long jumps missed offline. It takes off
  within 32 units of the check and 10 of its line now, or not at all; the view may be 10° off the landing (the air
  kills the speed across in a few hundredths of a second), and in the air it turns along the way on, lined up for the
  next one: lining up went from 0.27 s to under 0.1 s.
- **Doorways threaded with a few units to spare** were missed by a takeoff a little off the line: the flight must have
  8 units on either side now.
- **A long jump onto the start of a ladder or a jump** of the path left its executor a bot coming in at speed: those
  nodes are no landings.
- **Lava and slime under a landing** went unseen: the check looked for them at a standing player's feet, 18 units into
  the floor under a bot that comes down ducked (the leaps at enemies had the same fault).
- **`lb stats` counted the tricks since a reset against the sums of the bots there then**: after bots were kicked and
  others joined, 28 dodges showed instead of some 350. Each bot's are counted from its own at the reset now.

## Grenades, rockets and sight (feedback after M4.3)

The owner's feedback: bots at times look stuck: one with a grenade stands or walks with an enemy right in front and
throws nothing; rockets are fired too carefully, not at an enemy close by though out of the blast; bots now and
then do not see an enemy right in front of them, maybe decisions in conflict. Grenades should go much more often, at
the high levels all of them in a row, and with no enemy chosen too, where one is expected; satchels the same way.
The owner's choices:
- **Grenades:** almost always when a throw fits (it lands there and the blast spares the bot), about a second apart;
  hard and expert throw them all in a series (an enemy in sight within 250 units ends it); with no enemy known,
  where one is expected (where a lost one would come into view, a sound heard, the busiest way into sight), a little
  off the spot.
- **Satchels:** thrown where an enemy is expected, watched from cover nearby with the radio up, set off when one
  comes by; after 15–25 s the bot moves on and the charges stay.
- **Rockets:** from 200 units, taking up to 40 of the blast with 80 health.
- **Sight:** an enemy within some 500 units in the middle of the view recognized 3–4 times sooner, standing or ducked
  as noticeable as running.

**The stall watch** (new, `lb stats`, `lb brain`, `stall:` in the log) looked at a normal game first: dm_snow, 8
normal bots, the map's weapons, 5 minutes (40 bot-minutes). It uses where every player really is, for the log only.
Games differ by who joins: a fearful lot hides more and kills less (20–21 kills a minute with 14–17 stands in cover,
23–25 with 1–10), so the counts below are two games of one lot.

| Stall                                      | Before    | After (two games)  | What is left                                        |
|--------------------------------------------|-----------|--------------------|-----------------------------------------------------|
| An enemy close in front not seen for 0.6 s | 26, 19 s  | 2, 1.4 s; 0        | still recognizing it                                |
| A target in sight not fought for a second  | 88, 169 s | 41, 68 s; 41, 79 s | reloading with every gun empty (half), aim, deploys |
| Standing still for 2 s                     | 16, 56 s  | 18, 53 s; 14, 43 s | hiding in cover with nobody in sight (all but one)  |

What it showed and what was done:
- **A weapon the game would not draw.** A bot believing its gauss loaded (the uranium not known) chose it; the game
  refused the switch (no ammo), the motor tried three times and then neither switched nor fired: the bot held its
  MP5 at an enemy in sight and did nothing, again and again. A weapon refused after three tries is now left alone
  for 8 s and the bot fights with the one in hand: 13 such stalls (43 s) came down to 1.
- **The aim with a throwable in hand** waited for the gun to come out, so after every throw the bot looked
  elsewhere for half a second or more. It stays on the enemy now (a throw's own look still comes first).
- **Recognition close by** took 1–2 s for a normal bot at an enemy standing 200 units in front (3.3 s ducked, up to
  4 s at the edge of the view); now 0.15–0.3 s (see `docs/perception.md`).
- **No look at all.** The few enemies close in front left unseen had no contact at all: a look's 12 traces went to
  players already noticed (up to six each), and one stepping out close by waited behind them (at hard a look left
  0.15–0.37 players out on average). A player in view not in contact yet now gets three traces on top of the 12.
- **Pressed against a wall in a fight.** The strafe looks for walls with a line at chest height; a box lower than
  that, a player, or the wall behind a ledge the move had turned from held the bot where it stood for seconds (the
  watch's probe: the strafe asked 270 units/s, "the world in the way 0 units ahead"). A move on the ground that gets
  under a quarter of its speed for a third of a second is now backed out of, and the strafe turned.
- **Standing in corridors.** With walls within 134 units on both sides an unskilled bot stood still in a fight
  (yapb's rule); it now strafes toward the farther wall while there is room, and only in a narrow corridor goes
  back and forth instead.
- **Standing in a cover it was found in.** A retreating bot at its cover stood still with an enemy 80 units off; it
  now fights back from there (strafing, backing off, never closing in) and goes back to the spot once the enemy is
  out of sight.
- **The launcher and the MP5 in turn.** Near the least distance for a rocket, which moves with the enemy's pace, the
  bot switched between the two, half a second's deploy each time; the launcher is taken up again only 100 units over
  it.
- **"The weapon channel is someone else's"** turned out to be weapon deploys and the pause between clicks; the watch
  now tells the motor's reason, and no longer counts an enemy behind another player or outside a zoomed scope as
  unseen.

**Weapon sets** (`weapon-scenarios.sh`, dm_snow, 8 bots, the weapon and the MP5 given on spawn, 150 s; the build
before this round, the first cut of it, and the last run of each set with the fixes below; two runs of rockets at
hard):

| Set                     | Before: thrown, damage, own, suicides | First cut         | Now                                |
|-------------------------|---------------------------------------|-------------------|------------------------------------|
| grenades, normal        | 71, 791, 130, 2                       | 272, 1738, 68, 1  | 273, 1616, 89, 0                   |
| grenades, hard (series) | —                                     | 289, 2032, 250, 3 | 266, 1766, 31, 0                   |
| rockets, normal         | 53, 2039, 72, 1                       | 109, 4092, 663, 3 | 109, 3857, 125, 1                  |
| rockets, hard           | —                                     | —                 | 106, 4640, 0, 0; 101, 3900, 121, 0 |
| satchels, normal        | 113, 1724, 439, 4                     | 106, 1088, 313, 3 | 100, 1066, 195, 0                  |

The grenade sets kill less than before (61 kills against 91 at normal): a grenade thrown almost every time one fits
takes the MP5 out of the bot's hands for a second or so each time. Satchels hurt their throwers less than half as
much as before, and none killed its thrower.

### Found on the stand and fixed

- **Rockets at close range hurt their shooters nine times as much** in the first cut (663 against 72): a rocket
  flies 250 units/s for 0.4 s before it ignites, and an enemy running at the bot meanwhile meets it 100–150 units
  off instead of 200–300. The least distance now counts the enemy's and the bot's closing speed over the rocket's
  flight (about 400 units at an enemy charging in, 200 at one strafing), and the bot backs off while its rocket
  flies at a target within 450 units. Later runs at hard still found 212–351 own damage and 2–3 suicides in 150 s,
  from three things: the hold on closing in ended before slow close rockets got there (it was reckoned at 1500
  units/s: now the rocket's own pace and 0.4 s more); backing out of a wall went straight at the enemy (now aside);
  and a bot on its way to an item or a hunt fired at an enemy it passed and walked on into the blast (its rocket's
  spot is now kept out of like a grenade's, whatever the goal). Two runs at hard after that: 0 and 121 own damage, no
  suicide.
- **Grenades followed by their throwers.** A bot threw at an enemy 300–600 units off, then chased it into its own
  blast 1–2.6 s later; a throw that grazed an edge came back and went off 80–260 units from the thrower. The bot now
  knows where each of its grenades goes and when it goes off, seen or not (where it is seen going when it is: one
  coming back is run from), does not close in on the target until it has gone off, ends a series with one down
  within 350 units, and throws only along arcs clear by 8 units on every side; grenades where an enemy is expected
  go uncooked. Its own grenades still hurt it (374 damage and 3 suicides at hard in 150 s) until the last step: any
  move deeper into the reach of a grenade about to go off (the hunt for the enemy leads right to where it was
  thrown) keeps only its part along the edge. At hard 98 damage and 1 suicide are left.

### Left

- **A grenade that comes back unseen** (off a wall, below the view) is believed where it was meant to land: 4 of the
  6 own grenades that hurt their throwers in the last runs were believed 490–1540 units from where they went off. A
  player hears the grenade bounce close by: the bots hear the bounces since M5.1 (`docs/m5-acceptance.md`).
- **A gun chosen with no ammo** (the gauss, its uranium not known) is given up only after the game refused the
  switch three times, up to 3 s. The wait for the switch cannot be shorter: the launcher stays in hand while its
  rocket flies.
- Most of what is left of "not fought" is reloading with every gun empty.

## The 60-minute run

Eight bots of the normal preset, the map's weapons, 1000 fps, a map change every 12 minutes: crossfire, stalkyard,
boot_camp, dm_snow, bounce (the script checks the bots are back after each change; `tools/observer/soak_assert.py`
watches the telemetry).

The first try stopped at the second map change: no bot could join, the server full. Every spawn clears a player's
flags but `FL_PROXY` (`pev->flags &= FL_PROXY` in `CBasePlayer::Spawn`, hlsdk-portable and BugfixedHL alike), the
fake client flag among them, and Xash3D drops fake clients on a level change by that flag: bots that had died once
stayed on as clients no one moves, their slots taken, 8 more with every change. The 20 map changes of M0 passed
because they came seconds apart, before any bot died. The adapter now sets the flag again on its bots every frame and
after every move (the plan allows it: it is what the engine's network layer would keep). ReHLDS drops fake clients by
its own client record and was not hit.

The second run:

| Map       | Kills | Kills a minute | Suicides | Core time p99, µs |
|-----------|-------|----------------|----------|-------------------|
| crossfire | 187   | 15.6           | 4        | 875               |
| stalkyard | 91    | 7.6            | 2        | 532               |
| boot_camp | 80    | 6.7            | 0        | 429               |
| dm_snow   | 259   | 21.6           | 1        | 491               |
| bounce    | 124   | 10.3           | 4        | 664               |

- After every map change the 8 bots were back in slots 1–8 within 20 s.
- 3.7 million frames, 2.9 million bot commands; no stale move, no failed move call, no dropped or malformed event, no
  bot fault, no safe mode; no error in the log (63 warnings, all navigation links the server did not confirm).
- The watchdog's one complaint is the moment of the first map change with no bot in it; the worst p99 of the core,
  1172 µs, came in the first 12 minutes, while the next sub-stage was being compiled on the same machine.
- 11 suicides, 1.4 a bot-hour: six by the bots' own grenades, three by their own snarks, two by rockets. The
  grenades are thrown three to four times as often since the feedback after M4.3; hearing grenades bounce (M5.1)
  answers the ones that came back unseen.

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
- Own blast incidents are logged with the distances involved, where the bot believed its grenades and what moved
  it then (`own blast:` in `logs/lambdabots.*.log`), and a death by the bot's own gauss with its last charge (`killed
  itself with the gauss`).
- `lb map [spots|mines|danger]`: the map's tactics and where the bots got hurt.
- `lb brain` also shows each bot's goal task, mood, goals taken, goal and target changes, what came of the new
  goals, and the share of the time it stood still out of fights and in them; `lb profile <name>` shows a personality's goal weights, favourite weapons and whether it shoots the gauss
  through walls.
- `scripts/stand/style-scenarios.sh [--seconds 180] [--bots 8] [style…]`: the per-style runs above, reports in
  `stand-runs/current/styles/`.
- `lb items <item>…` hands items out on every spawn (`longjump`), `scripts/stand/tricks-scenarios.sh [--seconds 150]
  [--bots 8] [--difficulty hard] [set…]` runs the trick sets, reports in `stand-runs/current/tricks/`.
- `cargo test --release -p lb-testkit --test longjumps -- --ignored --nocapture`: routes on the maps with and without
  long jumps along the way (see `docs/navigation.md`).
- The stall watch: `lb stats` (by cause, since the reset), `lb brain` (per bot), `stall:` lines in the log with the
  details (see `docs/behavior.md`, *Inspecting*).
- `lb brain` shows each bot's tricks, `lb stats` their sums; missed tricks are logged (`missed:`) with how fast they
  left the ground and where they came down, gauss boosts when thrown and when given up.
- `lb nav test longjump|gauss_boost [count]` runs the trick links live; `cargo run -p lb-nav --example tricks_debug
  <graph.lbnav>` lists a graph's trick links and how often random plans take them.

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
- **Gauss boosts in a fight-heavy game** are rare: the uranium goes on the fight and a boost wants two calm seconds.
- **A long jump at the enemy** is checked before it is taken, but whether it helped (a kill, a hit) is not counted.
- **Tricks and GunGame** (twice the leaps in warmup, gauss jumps only at 80 health with descore) came with M5.1
  (`docs/m5-acceptance.md`).
- **The long jump links are few** (1–8 a map): only those every checked takeoff makes; the rest of the gaps are
  walked round.
- **Other players** are not in a long jump's check (it follows the flight through the map only): one on the way stops
  a flight in the air, most of the few misses on the stand.
- **Bold long jumps and dodging** come at skill 75 (switches keep the lower preset's value): of the bots `lb_difficulty
  hard` lets in (63–87), those below 75 long jump along the way only on straight stretches, 1–3 times in 150 s of a
  crowded game.
