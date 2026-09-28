# M4 acceptance: the arsenal (M4.1)

State as of 2026-09-28, after sub-stage M4.1, the arsenal. Still to come: knowledge, goals and styles (M4.2), the
tricks (M4.3) and the M4 acceptance runs. Test stand: Xash3D FWGS 0.21 (arm64) + Metamod-FWGS + hlsdk-portable,
macOS, crossfire at 1000 fps, 8 bots of the normal preset (skill 44–57). The ReHLDS server with BugfixedHL has not
run M4.1 yet.

## Results against the plan's criteria

| Criterion                                  | Status | How it was checked                                                           |
|--------------------------------------------|--------|------------------------------------------------------------------------------|
| A scenario for every weapon                | yes    | `scripts/stand/weapon-scenarios.sh`: each weapon on its own, table below     |
| Deaths by own hand an hour below a limit   | yes    | the last mixed game: none in 10 min of 8 bots; the games before, below       |
| Accuracy tables                            | yes    | `lb stats`: hit rate by distance for every weapon and fire mode              |
| The game DLL's weapon rules                | partly | `lb selftest` on hlsdk-portable; BugfixedHL and the classic SDK not run live |
| Target switches, tricks, the 60-minute run | later  | M4.2, M4.3 and the M4 acceptance runs                                        |

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

So hlsdk-portable plays by the 2023 update's rules, as BugfixedHL does. The classic SDK's rules differ: the primary
attack sets satchels off, and a grenade leaves at 400 units per second. The bots take any DLL that is not BugfixedHL
(told by its cvars) to play by the 2023 rules; `game.dll` in `config/lambdabots.yaml` can name it. A bot that sees
the detonate button throw a satchel learns the buttons are the other way round.

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
- Own blast incidents are logged with the distances involved (`own blast:` in `logs/lambdabots.*.log`).

## Not done yet

- **A satchel from a jump** goes off by the enemy about one time in seven.
- **A bot's own snarks** still turn on it now and then, most with only a crowbar and a pocketful of snarks.
- **A tripmine's planter** may still be near when an enemy trips it (two deaths in four runs of its set).
- **Satchel traps and mines at chokepoints** (the `PlantTrap` goal) are M4.2.
- **A charged gauss shot** still kills its shooter now and then (one in the mixed game above). What it hit is not known
  yet: damage a bot takes from its own gauss is logged since (`unattributed damage with the gauss in hand`).
- **Game DLLs:** BugfixedHL on ReHLDS and the classic SDK's rules have not run live.
