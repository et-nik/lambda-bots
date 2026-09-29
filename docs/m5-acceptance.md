# M5 acceptance: game modes and bot management

State as of 2026-09-29, after sub-stage M5.1, GunGame. Still to come: M5.2, team play (TDM, team GunGame, joining a
team), and M5.3, bot management (rotation, the quota's cvars, disguise).

The production server plays FFA GunGame, so GunGame came first. By the user's decisions:
- The bots know GunGame from the scoreboard alone: a player's level is its frags over 100 and the leader is the
  scoreboard's first line. The order of the levels is not needed, so the plan's AMXX bridge (`lambdabots_gg.sma`) and
  a copy of the plugin's level list are not made.
- GunGame is tested by the user on the production server; the macOS stand has no AMX Mod X (it does not run on
  arm64), so the plugin cannot run there.
- There is no spawn protection on the production server: the bots need not recognize one.

## Results against the plan's criteria

| Criterion                                                                                           | Status   | How it is checked                         |
|-----------------------------------------------------------------------------------------------------|----------|-------------------------------------------|
| A GunGame match: levels going up, no weapon pickups, the tripmine level, the last level, the warmup | the user | on the production server (hl-gungame 2.3) |
| TDM with no team kills                                                                              | M5.2     |                                           |
| Every disguise switch on its own                                                                    | M5.3     |                                           |

## GunGame (M5.1)

### What the plugin does

From `hl-gungame` (`gungame.sma` 2.3), what shapes the bots:
- **Levels.** A map's own file (`configs/gungame/<map>.ini`) replaces the default list; crossfire has 14 levels. Each
  level gives its weapon alone (the tripmine level a glock too, the last one the crowbar, a long jump and two
  batteries); on every level change the player is stripped and given the next kit. Bots skip the levels marked
  `botcant` (the satchels and the tripmines on most maps).
- **The scoreboard.** On every level change the plugin sets the player's frags to level × 100; the game's own
  scoring adds and takes a frag for each kill and suicide in between.
- **Damage.** Only the level's weapons hurt other players: hits and projectiles of any other weapon are blocked
  (the attacker hears an electric sound). A player's own blasts always hurt it.
- **Pickups.** Weapons, ammo and weapon boxes cannot be picked up; the map's are hidden. Health, batteries, the long
  jump and chargers work as ever. Ammo is endless: the plugin tops the reserve up and swallows the `AmmoX` message.
- **Suicides.** With `gg_descore 1` (FFA only) a suicide takes a kill off, and at none a level. A fall or the world
  takes nothing.
- **The warmup.** The first human to join starts a 30 s warmup with the crowbar and a long jump, where kills do not
  count; with only bots on the server the warmup never ends.
- **The win.** The first to kill on the last level wins; then everyone is frozen and the map changes.

### What the bots do

In short (the whole of it in `docs/behavior.md`, *GunGame*):
- **Knowledge:** every player's level and the leader from the scoreboard; what the bot's own level gave it from what
  it carries: one gun, a throwable, tripmines and a glock, or the crowbar alone (the last level, or the warmup on the
  first).
- **Weapons:** only what the level gave; no crowbar where the level gave none. A rocket too close, or a bolt or the
  egon beam's end bursting on the bot, makes it back off with the gun in hand. Throwables are the weapon on their
  levels; the glock of the tripmine level only sets mines off.
- **No weapon or ammo pickups.**
- **Targets:** the leader counts as if half as far; a player one kill from winning (the crowbar in hand past the
  first level) counts 1.5 times more and is kept 300 units off; the one who killed the bot last counts 1.2 times more.
- **The duel:** against an enemy whose gun it sees, a bot keeps to the distance where its gun does clearly better
  than the enemy's.
- **Levels:** with the crowbar alone a lost enemy is hunted; on the tripmine level enemies are got away from, mines
  go down whenever no enemy is in sight, and a mine is shot when an enemy is by it.
- **Moods:** two levels or more behind the leader a bot pushes on, the leader takes care, and in the warmup nothing
  holds it back, its long jumps at enemies twice as ready.
- **Suicides:** with `gg_descore`, no satchels thrown from a jump and gauss jumps only with 80 health.
- **Tools:** `lb gg` prints every player's level, the leader and each bot's kit; `lb brain` each bot's level.

### How it was checked

- **Tests.** `lb-game` tells the kit of every level from the weapons carried, and the leader and levels from frags
  and deaths. `lb-decision`: weapons and ammo left alone in GunGame, enemies got away from on the tripmine level, a
  lost enemy hunted with the crowbar alone. `lb-combat`: a launcher too close stays in hand, the fight backs off from
  what it keeps away from. `lb-brain` (the whole bot on a scene): a bot with grenades asks for no other weapon and
  pulls the pin at an enemy; a bot with a launcher 160 units from an enemy fires no rocket and backs off.
- **The stand** (no plugin: `lb_gungame on` with the game's spawn kit, the glock and the crowbar, which reads as the
  glock's level), dm_snow, 8 bots, 3 minutes: no fault; `lb gg` and `lb brain` read every level and the leader from
  the scoreboard; no goal went for a weapon or ammo (bots still took the ones they ran over, which a server with the
  plugin does not let them), no suicide. The ordinary game after it on the same build: 22.5 kills a minute (20–25 at
  M4's end), no suicide, no enemy close in front unseen, core p99 247 µs.
- **The production server:** the user's match (to come).

### Hearing grenades bounce

The bots now hear a hand grenade bounce (`weapons/grenade_hit*`), whoever threw it, and keep out of where it may go
off (`docs/behavior.md`, *Dodging*). Six of the eleven suicides of M4's 60-minute run were the bots' own grenades,
most of them come back unseen; with `gg_descore` each costs a GunGame kill. The grenade set on dm_snow (hand grenades
and the MP5, 8 normal bots, 150 s):

| Run                    | Thrown | Damage | Own blasts on the bots | Suicides | Kills |
|------------------------|--------|--------|------------------------|----------|-------|
| M4, after its feedback | 273    | 1616   | 89                     | 0        | 61    |
| M5.1, first            | 199    | 1326   | 11                     | 0        | 74    |
| M5.1, second           | 235    | 970    | 40                     | 0        | 60    |

The bots hurt themselves two to eight times less; they throw about a fifth less (they back away from bounces heard
close by), and kill as many.

### Found on the way

- **The test server crashed** four times in 13 hours (`PF_MessageEnd_Intercept` in ReHLDS, called from AMXX's
  `ShowSyncHudMsg`; the cores are in `/var/lib/apport/coredump`). The adapter hooked every message it captures
  through ReHLDS's message manager, temp entities included, and the server's ReHLDS (API 3.15, build 4419) predates
  the April 2025 fix of that manager: a hooked message of more than 16 parameters (a HUD text is a temp entity of
  17–18) overflows it. The adapter now hooks through the manager only what other plugins send and the bots need,
  `ScoreInfo` and `TeamInfo`; everything else comes from the game DLL through Metamod as on any engine. A GunGame
  match played to its end on the test server (the plugin shows the final standings to every player, bots
  included) no longer takes the server down.

- **Slots lost on a map change** (Xash3D): every spawn clears the fake client flag, and Xash drops fake clients on a
  level change by it; the bots that had died stayed on as clients no one moves. Found by M4's 60-minute run, fixed
  in the adapter (`docs/m4-acceptance.md`, *The 60-minute run*).
- **`lb stats` over map changes** counted kills a minute by the current map's clock; it now runs on over the change.

### Left

- **Team GunGame** (`gg_teamplay`: levels shared by the team, no descore) waits for M5.2's teams; the bots read
  its levels from frags already.
- **A bot's own snarks** still turn on it now and then (3 of 4 suicides in the first 12 minutes of M4's 60-minute
  run, a mixed game); on the snark level with `gg_descore` each costs a kill.
- The kills a bot still needs for its next level are not used (the scoreboard shows them as frags over the
  level's hundreds).
