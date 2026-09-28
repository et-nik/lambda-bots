# M4 acceptance: the arsenal (M4.1)

State as of 2026-09-28, after sub-stage M4.1, the arsenal. Still to come: knowledge, goals and styles (M4.2), the
tricks (M4.3) and the M4 acceptance runs. Test stand: Xash3D FWGS 0.21 (arm64) + Metamod-FWGS + hlsdk-portable,
macOS, crossfire at 1000 fps, 8 bots of the normal preset (skill 44–57). The ReHLDS server with BugfixedHL has not
run M4.1 yet.

## Results against the plan's criteria

| Criterion                                  | Status | How it was checked                                                           |
|--------------------------------------------|--------|------------------------------------------------------------------------------|
| A scenario for every weapon                | yes    | `scripts/stand/weapon-scenarios.sh`: each weapon on its own, table below     |
| Deaths by own hand an hour below a limit   | yes    | mixed game: 1 in 10 min of 8 bots, 0.75 a bot-hour; per weapon below         |
| Accuracy tables                            | yes    | `lb stats`: hit rate by distance for every weapon and fire mode              |
| The game DLL's weapon rules                | partly | `lb selftest` on hlsdk-portable; BugfixedHL and the classic SDK not run live |
| Target switches, tricks, the 60-minute run | later  | M4.2, M4.3 and the M4 acceptance runs                                        |

The plan leaves the limit on deaths by own hand open. Here it is set at one per bot-hour in a mixed game.

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
| crossbow       | 33 (39)                   | 13.2     | 0           | 0                | scoped 117: 75% / 28% / 16%; bolts 11: 33% / — / —  |
| rpg            | 24 (51)                   | 9.6      | 1           | 61               | 49: — / 47% / 100%                                  |
| gauss          | 51 (54)                   | 20.4     | 0           | 0                | 2262 cells: 72% / 39% / 40%                         |
| egon           | 64 (78)                   | 25.6     | 0           | 0                | 840: 61% / 40% / 38%                                |
| hornetgun      | 58 (61)                   | 23.2     | 0           | 20               | 1178: 37% / 59% / 63%                               |
| handgrenade    | 4 (52)                    | 1.6      | 0           | 124              | 56: 20% / 10% / 8%                                  |
| satchel        | 2 (34)                    | 0.8      | 0           | 56               | 17: 18% / 34% / 0%                                  |
| tripmine+glock | 16 (16)                   | 6.4      | 0           | 0                | glock 1155: 18% / 12% / 15%; mines 1                |
| snark          | 14 (66)                   | 5.6      | 1           | 270              | 85: 100% / 84% / 100%                               |

- **Gauss:** each bot fired 13–26 charged shots against 4–13 rolls for plain fire, 70% of its choices (the normal
  preset's `gauss_charge` is 0.75). No bot died by its own shot.
- **Crossbow:** 117 of 128 shots went through the scope. It is snapped on, the shot fired and taken off again: about a
  second in all, as the game lets the scope toggle only once a second.
- **Throws:** in their sets each bot threw 5–10 grenades, 1–6 satchels and 7–15 snarks in 2.5 minutes.
- **Tripmines:** laid only when quiet and walking a corridor, 3 in the set. Most kills are the glock's.

## A mixed game

10 minutes, 8 bots, crossfire, every weapon allowed, the final build.

| Measure                       | Value                                         |
|-------------------------------|-----------------------------------------------|
| Kills by bots                 | 142, 14.2 a minute                            |
| Deaths by own hand            | 1 (a grenade): 0.75 a bot-hour                |
| Damage from own explosives    | 120                                           |
| Core time a frame at 1000 fps | p50 56 µs, p95 109 µs, p99 167 µs, max 395 µs |

Kills by weapon: egon 35, gauss 26, glock 22, crossbow 21, MP5 15, RPG 9, grenades 7 (thrown and launched), snark 2,
the rest 1–2 each.

| Weapon       | Rounds     | <300 | 300–800 | 800–1500 |
|--------------|------------|------|---------|----------|
| glock        | 2069       | 18%  | 13%     | 14%      |
| 357          | 59         | 10%  | 9%      | 0%       |
| MP5          | 1203       | 18%  | 10%     | 4%       |
| MP5 grenades | 20         | —    | 36%     | —        |
| crossbow     | 84 scoped  | —    | 29%     | 21%      |
| shotgun      | 31         | 11%  | 8%      | —        |
| RPG          | 21         | —    | 30%     | 43%      |
| gauss        | 1342 cells | 100% | 35%     | 40%      |
| egon         | 403        | 64%  | 36%     | 63%      |
| hornet gun   | 37         | 100% | 50%     | 93%      |

The gauss's rate counts a cell as 10 damage; charged shots deal more a cell, so up close it caps at 100%.

Throwing depends on what the bots pick up. This run: 7 grenades, 1 satchel and 16 snarks. The run before on the same
build, without the last gauss fix: 148 kills, 1 death by own hand, and 36 grenades, 4 satchels and 23 snarks.
Before explosives were topped up, a 12.7-minute game saw 8 grenades, 8 satchels and 26 snarks.

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
| Gauss self-kills      | 2 in a 10-minute mixed game                                    | none in the gauss set and the mixed game: the charge is kept small for the nearest wall along the line                  |

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
- **Rockets burst next to the shooter.**
  - A rocket leaves 16 units ahead, 8 to the right and 8 below the eye, and goes for the target's feet. The line of
    fire was checked from the eye to the chest.
  - The laser that guides the rocket followed a new, closer target.
  - The bot ran into its own blast while the rocket flew.
- **MP5 grenades burst at the muzzle.** The bot fired a second after deciding. By then it had strafed next to a wall,
  or the target or someone else had come close.
- **Satchels were set off 250 units away.** The blast of the multiplayer satchel reaches 300.
- **Snarks were shot with whatever the bot fought with,** a rocket launcher included, and never with a player in
  sight. A snark is now shot with a gun without a blast, and first when the player in sight is 300 units away or
  more.
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
- `lb selftest [name]`: the DLL's satchel buttons, crossbow zoom and grenade speed, with one bot.
- `lb brain` shows each bot's weapon protocol and counts:
  - throws, scoped shots, charged and plain gauss rolls;
  - time no charge could start for a drop behind or a wall ahead;
  - dodges and failures;
  - the explosives it carries.
- `scripts/stand/weapon-scenarios.sh [--seconds 150] [set…]`: the per-weapon runs above, reports in
  `stand-runs/current/weapons/`.
- Own blast incidents are logged with the distances involved (`own blast:` in `logs/lambdabots.*.log`).

## Not done yet

- **Satchels** are thrown often and set off seldom (2–9 kills a set). The bot waits for an enemy within 160 units of
  the charge while it is itself out of the blast.
- **A bot's own snarks** still bite it in a close fight (10 a bite), when it keeps shooting the player. In the snark
  set the bots have only the crowbar to fend them off.
- **A tripmine's planter** may still be near when an enemy trips it (two deaths in four runs of its set).
- **Satchel traps and mines at chokepoints** (the `PlantTrap` goal) are M4.2.
- **Game DLLs:** BugfixedHL on ReHLDS and the classic SDK's rules have not run live.
