//! Weapons beyond the trigger: which protocol runs, when a throw, a mine, a launched grenade or a detonation is worth
//! it (ten times a second, from what the bot believes), and dodging what it sees about to blow up.
//!
//! - **Throws:** three times a second at the nearest enemy in sight, or lost up to 3 s ago with its position still
//!   tight: a grenade 300–1000 units away when a planned throw hurts it and the blast spares the bot, a satchel at one in
//!   sight 350–550 away (800 with the long jump module) with a clear line to it, a snark at 150–800 likewise. A
//!   grenade that fits is thrown almost always, a satchel or a snark more rarely (the chance per look is the kind's
//!   base times the skill's `throw_rate`); a grenade rests the arm about a second, a satchel 3–6 s. With no gun but
//!   the crowbar, throws are the weapon (yapb's grenade war).
//! - **A grenade at an enemy** is planned to go off where the enemy will be ([`lb_combat::grenade`]): the flight and
//!   the bounces the game gives it, cooked so it bursts on the tick it gets there. It goes as players throw it: on
//!   the run at the enemy (or where one is expected) from 340 units, from a jump at one 500 or more away or above (skills
//!   with tricks, as often as the style likes; from a long jump with the module), the bot backing off after it.
//! - **A satchel at an enemy** goes as players throw it: on the run at the enemy (from a jump for skills with tricks,
//!   as often as the style likes; from a long jump at one 550–800 away with the module), the bot backing off after
//!   it, and set off as it comes by the enemy. Thrown from where the bot stands it would fly some 200 units and lie
//!   in the bot's own blast.
//! - **Grenades with no enemy known:** now and then one where an enemy is expected: where a lost one would come into
//!   view, a sound heard lately, the busiest way into the bot's sight. Skills with `throw_series` throw one after
//!   another until none is left, the grenade kept in hand between them; an enemy in sight within 250 units ends the
//!   series for the gun.
//! - **The MP5's grenade** at a target in sight 300–700 units away, every 2.5–4 s, when the lob is clear; the bot does
//!   not close in on the target while its grenade or rocket is on the way.
//! - **Satchels** go off when an enemy is within 160 units of one and closer to it than the bot; a bot too close to
//!   be spared backs off first.
//! - **Tripmines** are shot (see [`crate::trail`]) when an enemy is by one, what the whole chain it sets off would do
//!   to the enemy and to the bot reckoned; a known enemy mine ahead is shot to clear the way when nothing else goes
//!   on. When quiet the bot now and then lays a mine across a corridor it walks along, never near a spawn point and
//!   not next to another mine; a trail drops them on the floor on the run. On the GunGame tripmine level, where mines
//!   are the weapon, it lays them with enemies in sight further off; elsewhere in GunGame a mine is not shot at an
//!   enemy (the plugin keeps its blast off other players).
//! - **Dodge:** a grenade coming down (its own too), an MP5 grenade landing or a rocket passing near the bot makes it
//!   run away from the blast (yapb ran toward it), checking for ledges.
//! - **Gauss:** its charge runs whenever the gauss is in hand; nothing else starts while it charges.

use lb_combat::arms::boost::GaussBoost;
use lb_combat::arms::detonate::{Airburst, Burst, MineShot, SatchelTrigger};
use lb_combat::arms::gauss::{Gauss, GaussInput};
use lb_combat::arms::launcher::Lob;
use lb_combat::arms::mine::{FloorPlanter, Planter};
use lb_combat::arms::scope::{LOST_HOLD as SCOPE_LOST_HOLD, Scope, Sight};
use lb_combat::arms::throw::{Barrage, Kind, Thrower};
use lb_combat::arms::{Hands, Request, Status};
use lb_combat::ballistics;
use lb_combat::fight::drops;
use lb_combat::grenade::{self, Aim};
use lb_combat::policy::XBOW_UNZOOM;
use lb_core::dmath;
use lb_core::math::view_angle_vectors;
use lb_core::rng::BotRng;
use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3};
use lb_decision::GoalKind;
use lb_game::gungame::Kit;
use lb_game::mechanics::{Attack, WeaponClass, blast_radius, spec};
use lb_game::sounds::SoundKind;
use lb_game::weapons::WeaponId;
use lb_knowledge::{BeamPass, EnemyTrack, HypothesisKind, PlayerKey, Relation, TrackState};
use lb_motor::{LookIntent, MoveIntent, Prio, StanceIntent, WeaponIntent};
use lb_nav_api::NavService;
use lb_worldq::{Trace, TraceQuery, Tracer};

use crate::BotBrain;
use crate::mind::{Body, Character};
use crate::trail::TrailPlan;

/// Throw windows (horizontal distance).
const GRENADE_BAND: [f32; 2] = [300.0, 1000.0];
/// A grenade at an enemy this far away goes on the run, from this far from a jump (or at an enemy this far above),
/// from this far from a long jump. Players dash at an enemy even closer, and back off at once; half their grenades
/// go from a jump (the GunGame server, 2026-10-02).
const GRENADE_RUN_MIN: f32 = 150.0;
const GRENADE_JUMP_MIN: f32 = 250.0;
const GRENADE_JUMP_ABOVE: f32 = 48.0;
const GRENADE_LEAP_MIN: f32 = 500.0;
/// How fast where an enemy may be spreads while the grenade is out (units a second): in sight, and lost a while.
const GRENADE_SPREAD: f32 = 110.0;
const GRENADE_SPREAD_LOST: f32 = 130.0;
/// A grenade is thrown only when its plan should do this much damage (it costs little: a bot carries up to ten);
/// handed back as fast as they go, when it should do this much.
const GRENADE_WORTH: f32 = 10.0;
const REFILLED_WORTH: f32 = 5.0;
/// The pin out this long at most: at an enemy in sight (the bot holding a grenade is not shooting, and is shot at;
/// players cook about a second), and with grenades handed back as fast as they go (GunGame's grenade level: players
/// let go at once, one every 1.2 s, as many kills a throw as the bots' cooked ones and far more a minute) at an enemy
/// this far off or more. Closer, a grenade cooked to burst as it gets there did best on the GunGame server (0.22 kills
/// a throw against 0.08 let go at once, and fewer suicides), and goes so.
const SEEN_COOK: f32 = 1.2;
const QUICK_COOK: f32 = 0.75;
const QUICK_FROM: f32 = 300.0;
/// After a grenade the bot backs off from where it went this long, as players do.
const GRENADE_BACK_OFF: f64 = 0.8;
/// The bot runs at about this speed when it lets go on the run.
const GRENADE_RUN_SPEED: f32 = 270.0;
const SNARK_BAND: [f32; 2] = [200.0, 1000.0];
const LOB_BAND: [f32; 2] = [300.0, 700.0];
/// A throw's blast must land at least this far from the thrower.
const SELF_CLEAR: f32 = 300.0;
/// With nothing but throws, closer ones are worth it (the plan still keeps the blast off the bot as it backs off).
const WAR_GRENADE_MIN: f32 = 150.0;
/// Seconds before a quick throw leaves the hand (the game's least).
const WAR_LEAD: f32 = 0.5;
/// An MP5 grenade bursts on the first thing it touches and the bot moves on while it flies: it must land further off,
/// and a hurt bot keeps it for far targets.
const LOB_CLEAR: f32 = 400.0;
const LOB_HURT: f32 = 40.0;
const LOB_HURT_CLEAR: f32 = 550.0;
/// Enemies out of sight are thrown at while lost this recently, placed this tightly.
const THROW_TRACK_AGE: f64 = 3.0;
const THROW_TRACK_SIGMA: f32 = 400.0;
/// Throws are weighed this often.
const THROW_CHECK: f64 = 0.3;
/// Chance per check to take a throw (times the skill's `throw_rate`): at an enemy out of sight and in sight.
const UNSEEN_GRENADE: f32 = 0.9;
const UNSEEN_SNARK: f32 = 0.6;
const SEEN_GRENADE: f32 = 0.9;
const SEEN_SATCHEL: f32 = 0.15;
const SEEN_SNARK: f32 = 0.6;
/// The chance per check at most.
const THROW_CHANCE_MAX: f32 = 0.95;
/// Seconds before the next throw is weighed after one: a grenade, satchels, snarks (which cost nothing to let go).
const GRENADE_REST: [f32; 2] = [0.8, 1.2];
const THROW_REST: [f32; 2] = [3.0, 6.0];
const SNARK_REST: [f32; 2] = [1.0, 2.5];
/// A grenade where an enemy is expected, none known: the chance per check (times the skill's `throw_rate` and the
/// style's liking), how far off, how far from the spot it may land, and how recent a sound it goes at.
const BLIND_GRENADE: f32 = 0.03;
const BLIND_BAND: [f32; 2] = [350.0, 1000.0];
const BLIND_SCATTER: f32 = 48.0;
const BLIND_HEARD: f64 = 4.0;
/// Seconds between the grenades of a series; an enemy in sight this close ends it (the gun), and so does one of its
/// grenades coming down this close.
const SERIES_GAP: f64 = 0.3;
const SERIES_CLOSE: f32 = 250.0;
const SERIES_SAFE: f32 = 350.0;
/// At an enemy in sight, snarks go in a stream of this many (as many as the bot carries).
const SNARK_STREAM: [u32; 2] = [1, 3];
/// An enemy this far above is out of throwing reach.
const TOO_HIGH: f32 = 500.0;
const SNARK_TOO_HIGH: f32 = 200.0;
/// Satchels are set off when their blasts together would do an enemy this much damage (one satchel 200 units away).
const SATCHEL_WORTH: f32 = 40.0;
/// The bot keeps this far beyond a satchel's blast before setting it off.
const SATCHEL_SPARED: f32 = 24.0;
/// Drawing the satchel radio takes a second; with it in hand the press takes a moment. An enemy is judged where it
/// will be by the time the charges go off.
const RADIO_DRAW: f32 = 1.0;
const RADIO_PRESS: f32 = 0.1;
/// An enemy coming into the blast within this long gets the radio drawn; the radio waits for an enemy in the blast
/// this long at most.
const SATCHEL_COMING: f32 = 1.5;
const SATCHEL_WAIT: f64 = 2.5;
/// An enemy in sight away from the charges this close needs a gun rather than the radio.
const SATCHEL_THREAT: f32 = 700.0;
/// A sound heard this recently this close to a charge: someone by it.
const SATCHEL_HEARD: f64 = 1.0;
const SATCHEL_HEARD_NEAR: f32 = 200.0;
/// Hurt this badly just now, the bot sets its satchels off at an enemy by them at once: they go with it when it dies.
const SATCHEL_DYING: f32 = 30.0;
const SATCHEL_DYING_HURT: f64 = 0.5;
/// Satchels go off once an enemy has been out of sight this long after it was seen by them (for so long it may still
/// be there), or once they have lain this long with nobody in sight.
const SATCHEL_LOST_WAIT: [f32; 2] = [1.0, 2.5];
const SATCHEL_LOST_NEAR: f64 = 6.0;
const SATCHEL_LIE: [f32; 2] = [8.0, 15.0];
/// A pile is this many satchels (as many as the bot carries).
pub(crate) const SATCHEL_PILE: [u32; 2] = [2, 4];
/// At an enemy in sight this far away, a satchel is thrown on the run and set off as it comes by; it flies about
/// this fast then. With the long jump module it goes from a long jump this far away: the leap carries it far, and
/// the bot stays out of its blast.
pub(crate) const AIRBURST_BAND: [f32; 2] = [350.0, 550.0];
const SATCHEL_RUN: f32 = 500.0;
const LEAP_BAND: [f32; 2] = [550.0, 800.0];
/// All the snarks at an enemy in sight this close: the chance per look (times the skill's `throw_rate`), and the
/// fewest worth it.
const BARRAGE_BAND: [f32; 2] = [60.0, 200.0];
const BARRAGE_CHANCE: f32 = 0.5;
const BARRAGE_SNARKS: i32 = 2;
/// After a barrage the bot runs from the swarm this long; with less health it does not start one.
const BARRAGE_RUN: f64 = 2.0;
const BARRAGE_HEALTH: f32 = 50.0;
/// A satchel flying at the enemy is watched while seen this recently; the enemy while lost this recently.
const AIRBURST_SEEN: f64 = 0.25;
/// Backing off a pile takes this long at most; the bot does not close in on the enemy for this long after a throw.
const PILE_BACK_OFF: f32 = 1.5;
/// Seconds of stepping along the wall after laying a mine (it arms in 2.5 s).
const MINE_STEP_AWAY: f64 = 0.6;
/// Satchels thrown as a trap lie this long with nobody by them before they go off anyway.
const TRAP_LIE: [f32; 2] = [60.0, 90.0];
/// A charged gauss shot goes through a wall this thick at most, with this much of its damage left beyond it.
const WALLBANG_THICK: f32 = 48.0;
const WALLBANG_LEFT: f32 = 80.0;
/// At an enemy lost behind a wall this recently, placed this closely.
const WALLBANG_AGE: f64 = 1.0;
const WALLBANG_SIGMA: f32 = 120.0;
const WALLBANG_CHECK: f64 = 0.05;
const SATCHEL_HOLD: f64 = 3.0;
/// After letting snarks go at an enemy the bot does not close in on it for this long: they bite their owner too.
const SNARK_HOLD: f64 = 3.0;
/// A satchel just thrown is still in the air, not where it lands: none is set off for this long after a throw (but
/// one flying by the enemy, which is watched).
const SATCHEL_SETTLE: f64 = 1.0;
/// A corridor this wide at most gets a mine; its nearer wall must be this close.
const CORRIDOR: f32 = 300.0;
const WALL_NEAR: f32 = 90.0;
const MINE_SPACING: f32 = 96.0;
/// Seconds before another mine is laid along the way.
const MINE_REST: [f32; 2] = [20.0, 30.0];
/// On the tripmine level only an enemy in sight this close stops a mine going down.
const MINES_NEAR: f32 = 700.0;
pub(crate) const SPAWN_CLEAR: f32 = 256.0;
/// Quiet this long before laying a mine or clearing one.
pub(crate) const QUIET: f64 = 5.0;
/// A dodge lasts this long once started; a run from a snark is renewed while it is near.
const DODGE_FOR: f64 = 0.5;
/// A long jump away from a blast lands this much further off it than the bot stands.
const LEAP_AWAY: f32 = 150.0;
const SNARK_RUN_FOR: f64 = 0.3;
/// Someone else's snark this close is run from (or burnt with the egon); the bot's own when it comes back this close.
const SNARK_NEAR: f32 = 300.0;
const OWN_SNARK_NEAR: f32 = 250.0;
/// With the egon in hand, a snark is burnt unless a player in sight is closer than this.
const SNARK_OVER_PLAYER: f32 = 300.0;
/// Navigation keeps off a known beam this long, told again after `BEAM_REPORT`.
const BEAM_AVOID: f32 = 60.0;
const BEAM_REPORT: f64 = 30.0;
/// A path passing this close to a beam the bot can get past costs this many seconds more (a detour of a few hundred
/// units is taken rather than go by a mine anyone may set off), told once the bot is this far from the mine.
const SHUN_RADIUS: f32 = 64.0;
const SHUN_COST: f32 = 1.5;
const SHUN_FROM: f32 = 256.0;
const BEAM_LENGTH: f32 = 2048.0;
/// The bot's own fresh mines: navigation keeps off their beams from this long before they arm (once the bot has run
/// on from them), and a bot standing in one gets out of it while it arms within this.
const OWN_BEAM_SOON: f64 = 1.5;
/// A beam this high over the floor where it crosses the way is ducked under (a crouched player is 36 tall), a lower
/// one is not got past (a jump over it at a careful pace clears it by a few units, if at all); its floor is looked for
/// this far below it.
const DUCK_UNDER: f32 = 42.0;
const FLOOR_BELOW: f32 = 256.0;
/// A body walking round a beam keeps its box this far off it (16 touches it; a turn takes the bot wider of the way it
/// asked for for a moment); its way round is looked along this far, at this height over its middle (over steps and
/// ramps) by its middle and both its sides, in steps of this.
const BEAM_KEEP: f32 = 24.0;
const BEAM_TOUCH: f32 = 17.0;
const ROOM_AHEAD: f32 = 48.0;
const ROOM_HEIGHT: f32 = 20.0;
const ROOM_SIDE: f32 = 15.0;
const BEAM_STEP: f32 = 8.0;
/// Ways round a beam: turned this many degrees more each time either side of the way asked for, up to a little past
/// square to it (further back is navigation's to find), the side taken last first while it was taken within this.
const BEAM_TURN: f32 = 25.0;
const BEAM_TURNS: u32 = 4;
const BEAM_SIDE_FOR: f64 = 0.6;
/// Near a beam (its way passing this close to one within this, or a beam to duck under within reach) the bot goes
/// no faster than this: a turn at a run takes it wide, a run stops late. Its run carries it this long before a turn
/// takes; slower than this it has stopped. Braking, it pushes against its run (some ten units from a run instead of
/// sliding fifty).
const GOVERN_GAP: f32 = 40.0;
const GOVERN_REACH: f32 = 160.0;
const CAREFUL: f32 = 160.0;
const DRIFT: f32 = 0.08;
const SLIDE: f32 = 40.0;
/// A high beam is ducked under from this far before it (ducking takes 0.4 s).
const DUCK_FROM: f32 = 130.0;
/// Closer than this to a high beam (beyond where its run would carry it) the bot waits until it is down.
const DUCKED_NEAR: f32 = 40.0;
const DODGE_MARGIN: f32 = 40.0;

#[derive(Clone, Debug)]
pub enum Active {
    Throw(Thrower),
    Mine(Planter),
    /// A mine dropped on the floor on the run (a trail).
    Drop(FloorPlanter),
    Lob(Lob),
    Detonate(SatchelTrigger),
    Airburst(Airburst),
    Barrage(Barrage),
    Shoot(MineShot),
    Scope(Scope),
    GaussBoost(GaussBoost),
}

impl Active {
    pub fn name(&self) -> &'static str {
        match self {
            Active::Throw(t) => t.kind.as_str(),
            Active::Mine(_) => "tripmine",
            Active::Drop(_) => "trail mine",
            Active::Lob(_) => "m203",
            Active::Detonate(_) => "detonate",
            Active::Airburst(_) => "satchel in flight",
            Active::Barrage(b) if b.limit.is_some() => "snark stream",
            Active::Barrage(_) => "snark barrage",
            Active::Shoot(_) => "shoot a mine",
            Active::Scope(_) => "scope",
            Active::GaussBoost(_) => "gauss boost",
        }
    }
}

/// What the weapon protocols did, for `lb brain` and the stand statistics.
#[derive(Clone, Debug, Default)]
pub struct ArmsStats {
    pub grenades: u32,
    /// Of them, thrown where an enemy was expected (none known), and in series.
    pub blind: u32,
    pub series: u32,
    pub satchels: u32,
    pub snarks: u32,
    pub mines: u32,
    pub lobs: u32,
    pub detonations: u32,
    /// Why satchels were set off.
    pub satchel_offs: Vec<(&'static str, u32)>,
    pub barrages: u32,
    pub mine_shots: u32,
    /// Why mines were shot.
    pub shot_whys: Vec<(&'static str, u32)>,
    /// Trails laid; mines dropped on the run; trails left lying with nobody by them.
    pub trails: u32,
    pub dropped: u32,
    pub trails_left: u32,
    pub dodges: u32,
    /// Runs from snarks.
    pub snark_runs: u32,
    /// Zoomed crossbow shots, the times the scope went on, and why it came off.
    pub scoped: u32,
    pub zooms: u32,
    pub scope_ends: Vec<(&'static str, u32)>,
    /// Charged gauss shots through a wall at an enemy lost behind it.
    pub wallbangs: u32,
    pub failed: u32,
    /// Failures by protocol and reason.
    pub failures: Vec<(&'static str, &'static str, u32)>,
}

impl ArmsStats {
    fn satchel_off(&mut self, why: &'static str) {
        self.detonations += 1;
        match self.satchel_offs.iter_mut().find(|(w, _)| *w == why) {
            Some(e) => e.1 += 1,
            None => self.satchel_offs.push((why, 1)),
        }
    }

    fn shot(&mut self, why: &'static str) {
        self.mine_shots += 1;
        match self.shot_whys.iter_mut().find(|(w, _)| *w == why) {
            Some(e) => e.1 += 1,
            None => self.shot_whys.push((why, 1)),
        }
    }

    fn failure(&mut self, protocol: &'static str, why: &'static str) {
        self.failed += 1;
        match self.failures.iter_mut().find(|(p, w, _)| *p == protocol && *w == why) {
            Some(f) => f.2 += 1,
            None => self.failures.push((protocol, why, 1)),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Arms {
    pub gauss: Gauss,
    pub active: Option<Active>,
    pub stats: ArmsStats,
    pub last_failure: Option<&'static str>,
    /// The button that set this bot's satchels off, for the server to learn its satchel buttons from.
    pub satchel_fact: Option<Attack>,
    /// When the bot let its last grenade go, with the view pitch and its velocity then: what the grenade does as it
    /// leaves tells the server's grenade speed.
    pub grenade_launch: Option<(SimTime, f32, Vec3)>,
    /// The side the bot last turned to round a beam, kept until then.
    pub(crate) beam_side: Option<(SimTime, f32)>,
    /// The satchels out: whom they were thrown at and when they go off anyway.
    satchels: Option<SatchelPlan>,
    /// Why the satchels being set off go off.
    detonate_why: &'static str,
    /// The radio up for the satchels waits for an enemy in their blast until then; pressed at once when `None`.
    detonate_wait: Option<SimTime>,
    /// An enemy is expected by the satchels until then (a trap watched): the radio is up for it.
    pub(crate) radio_until: SimTime,
    /// The satchel under way is thrown at this enemy, to go off as it comes by; how many of the throw's satchels are
    /// noted as thrown.
    throw_aim: Option<PlayerKey>,
    /// The throw under way lays a trap: its satchels lie long.
    trap_throw: bool,
    landed: usize,
    next_barrage: SimTime,
    /// A fired rocket is guided until then; the point it was fired at and the target.
    pub guide: Option<(SimTime, Vec3, PlayerKey)>,
    /// The bot's own rocket or launched grenade is on its way to the target until then.
    pub hold_until: SimTime,
    /// Both shotgun barrels on the next shot.
    pub double: bool,
    /// When the next zoom toggle may be pressed.
    pub zoom_ready: SimTime,
    /// When the view was last seen to zoom in or out: the game lets the scope toggle again a second after.
    pub zoom_flip: SimTime,
    zoomed: bool,
    next_throw: SimTime,
    /// Grenades go one after another until none is left.
    pub series: bool,
    next_lob: SimTime,
    next_mine: SimTime,
    /// How far back the bot may be thrown before it would drop further than is safe.
    recoil_room: f32,
    /// When the gauss's line of fire was last looked along, how far its first wall was, and the most damage a missed
    /// charged shot along it would come back at the bot with.
    gauss_wall: Option<(SimTime, f32, f32)>,
    /// When a shot through a wall was last looked for, and the point to shoot at and the damage the shot needs if
    /// there was one.
    wallbang: Option<(SimTime, Option<(Vec3, f32)>)>,
    /// When a way to dump a charge was last looked for, and the view angles found.
    dump_way: Option<(SimTime, Option<Vec3>)>,
    /// Running from a blast until then, along this direction.
    pub(crate) dodge: Option<(SimTime, Vec2)>,
    /// A mine just laid on a wall with this normal: the bot steps along the wall, out of where its beam will be.
    step_off: Option<Vec3>,
    /// The trail being laid or watched.
    pub trail: Option<TrailPlan>,
    /// Why the mine being shot is shot; the most of its health the bot gives to the chain it sets off, the chain's
    /// mines (where each is and the way it faces), and when the bot looks again whether it is still spared.
    pub(crate) shot_why: &'static str,
    pub(crate) shot_spare: f32,
    pub(crate) shot_chain: smallvec::SmallVec<[(Vec3, Vec3); 16]>,
    pub(crate) shot_check: SimTime,
    /// A chain set off goes off until then, mine by mine: its blasts are kept out of.
    pub(crate) blowing: Option<(SimTime, smallvec::SmallVec<[Vec3; 16]>)>,
    /// Why the last look for where a trail's next mine goes found none.
    pub drop_why: &'static str,
}

impl Arms {
    /// A new life: what was going on stops; statistics stay.
    pub fn reset(&mut self) {
        let stats = std::mem::take(&mut self.stats);
        let fact = self.satchel_fact;
        let mut gauss = std::mem::take(&mut self.gauss);
        gauss.reset();
        *self = Arms {
            gauss,
            stats,
            satchel_fact: fact,
            ..Arms::default()
        };
    }

    /// Every frame: notes when the view zooms in or out.
    pub fn watch_zoom(&mut self, zoomed: bool, now: SimTime) {
        if zoomed != self.zoomed {
            self.zoomed = zoomed;
            self.zoom_flip = now;
        }
    }

    /// A protocol runs or the gauss charges: nothing else starts.
    pub fn busy(&self) -> bool {
        self.active.is_some() || self.gauss.active()
    }

    pub fn describe(&self, now: SimTime) -> String {
        let mut parts = Vec::new();
        if let Some(a) = &self.active {
            parts.push(a.name().to_string());
        }
        if self.gauss.active() {
            parts.push(format!("gauss {} {:.1} s", self.gauss.phase(), self.gauss.charge(now)));
        }
        if self.guide.is_some_and(|(until, _, _)| now < until) {
            parts.push("guiding a rocket".into());
        }
        if self.dodge.is_some_and(|(until, _)| now < until) {
            parts.push("dodging".into());
        }
        if parts.is_empty() { "-".into() } else { parts.join(", ") }
    }
}

/// The view is on the aim point closely enough for a shot that must count: within the angle a body's width makes at
/// that distance.
pub(crate) fn on_target(view: Vec3, eye: Vec3, aim: Vec3) -> bool {
    let (forward, _, _) = view_angle_vectors(view);
    let to = aim - eye;
    let d = to.length().max(1.0);
    let off = lb_core::dmath::acos(forward.dot(to / d).clamp(-1.0, 1.0));
    off <= lb_core::dmath::atan(ON_TARGET_HALF_WIDTH / d).max(ON_TARGET_MIN.to_radians())
}

/// Half a body's width less a margin, and the least angle counted on target, degrees.
const ON_TARGET_HALF_WIDTH: f32 = 12.0;
const ON_TARGET_MIN: f32 = 0.4;

/// A move is checked this far ahead against known beams and blasts, seconds.
const BEAM_LOOKAHEAD: f32 = 0.3;
const BLAST_LOOKAHEAD: f32 = 0.3;

/// Where a mine's beam ends: as traced, or for one of the bot's own not traced yet, the way it faces.
fn beam_of(m: &lb_knowledge::explosives::Mine) -> Option<Vec3> {
    m.beam_end
        .or_else(|| (m.own && m.dir != Vec3::ZERO).then(|| m.pos + m.dir * BEAM_LENGTH))
}

/// How the beam from `a` to `b` is got past: walked round when it stands up from the floor, ducked under when it runs
/// high enough over the floor (at its middle), else not at all (walked round its end when it has one in the open).
fn beam_pass(tracer: &mut dyn Tracer, a: Vec3, b: Vec3) -> BeamPass {
    let d = b - a;
    if d.z.abs() > d.truncate().length() {
        return BeamPass::Around;
    }
    let mid = a + d * 0.5;
    let floor = tracer.trace(&TraceQuery::line(mid, mid - Vec3::Z * FLOOR_BELOW));
    if floor.fraction < 1.0 && mid.z - floor.end.z >= DUCK_UNDER {
        BeamPass::Under
    } else {
        BeamPass::Blocked
    }
}

/// How the bot gets past a mine's beam: one standing up from the floor is walked round (unless no room was found
/// beside it), others as their line showed, kept off until it was looked along.
fn pass_of(m: &lb_knowledge::explosives::Mine) -> BeamPass {
    let default = if m.dir.z.abs() > 0.7 {
        BeamPass::Around
    } else {
        BeamPass::Blocked
    };
    m.pass.unwrap_or(default)
}

/// How far along a move from `origin` along `dir` (unit) its line meets the beam from `a` to `b` at body height,
/// within `reach`.
fn crossing(origin: Vec3, dir: Vec2, reach: f32, a: Vec3, b: Vec3) -> Option<f32> {
    let p = origin.truncate();
    let r = dir * reach;
    let s = (b - a).truncate();
    let denom = r.perp_dot(s);
    if denom.abs() < 1e-6 {
        return None;
    }
    let q = a.truncate() - p;
    let t = q.perp_dot(s) / denom;
    let u = q.perp_dot(r) / denom;
    let z = a.z + (b.z - a.z) * u.clamp(0.0, 1.0);
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) && (z - origin.z).abs() <= 36.0).then_some(t * reach)
}

/// How far a player running at `speed` slides once it lets go (sv_friction 4, sv_stopspeed 100).
fn stopping(speed: f32) -> f32 {
    (speed - 100.0).max(0.0) / 4.0 + speed.min(100.0).powi(2) / 800.0
}

/// The stretch of the beam from `a` to `b` within the height of a body at `origin` (`half` its half height), flat.
fn beam_slice(origin: Vec3, half: f32, a: Vec3, b: Vec3) -> Option<(Vec2, Vec2)> {
    let (lo, hi) = (origin.z - half, origin.z + half);
    let d = b - a;
    if d.z.abs() < 1e-3 {
        return (lo..=hi).contains(&a.z).then(|| (a.truncate(), b.truncate()));
    }
    let (t0, t1) = (((lo - a.z) / d.z).clamp(0.0, 1.0), ((hi - a.z) / d.z).clamp(0.0, 1.0));
    let (p, q) = (a + d * t0.min(t1), a + d * t0.max(t1));
    if p.z.max(q.z) < lo - 0.5 || p.z.min(q.z) > hi + 0.5 {
        return None;
    }
    Some((p.truncate(), q.truncate()))
}

/// How far the box of a player at `c` is from the flat stretch `p → q`, the way a box is (the larger of the two axes):
/// under 16 it touches it.
fn box_gap(c: Vec2, p: Vec2, q: Vec2) -> f32 {
    let off = |t: f32| {
        let o = p + (q - p) * t - c;
        o.x.abs().max(o.y.abs())
    };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let (a, b) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
        if off(a) <= off(b) {
            hi = b;
        } else {
            lo = a;
        }
    }
    off((lo + hi) * 0.5)
}

/// The point of `p → q` nearest `c`.
fn nearest_on(c: Vec2, p: Vec2, q: Vec2) -> Vec2 {
    let s = q - p;
    p + s * ((c - p).dot(s) / s.length_squared().max(1e-6)).clamp(0.0, 1.0)
}

/// A player standing at `p` touches the beam from `a` to `b`.
fn near_beam(p: Vec3, a: Vec3, b: Vec3) -> bool {
    let s = (b - a).truncate();
    // A beam standing up from a mine on the floor: touched at its foot, from the floor up to its top.
    if s.length_squared() < 1.0 {
        return (a.truncate() - p.truncate()).length() <= 20.0
            && p.z + 36.0 >= a.z.min(b.z)
            && p.z - 36.0 <= a.z.max(b.z);
    }
    let len = s.length_squared().max(1e-6);
    let u = ((p - a).truncate().dot(s) / len).clamp(0.0, 1.0);
    let on = a + (b - a) * u;
    (on.truncate() - p.truncate()).length() <= 20.0 && (on.z - p.z).abs() <= 36.0
}

/// How a throw is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Way {
    Grenade,
    /// A satchel on the run, set off as it comes by the enemy.
    Airburst,
    Snark,
}

/// The bot's satchels out: when they go off without a sight of an enemy by them.
#[derive(Clone, Copy, Debug)]
struct SatchelPlan {
    /// Off once an enemy has been out of sight this long near them.
    lost_wait: f64,
    /// Off by then with nobody in sight.
    lie_until: SimTime,
}

impl SatchelPlan {
    fn roll(now: SimTime, rng: &mut BotRng) -> SatchelPlan {
        SatchelPlan {
            lost_wait: f64::from(rng.combat.range_f32(SATCHEL_LOST_WAIT[0], SATCHEL_LOST_WAIT[1])),
            lie_until: now + f64::from(rng.combat.range_f32(SATCHEL_LIE[0], SATCHEL_LIE[1])),
        }
    }
}

/// Near the shooter's end of the line of fire: an explosive would burst on whoever stands there.
const CROWD_REACH: f32 = 350.0;
const CROWD_RADIUS: f32 = 64.0;

/// Someone the bot sees (other than `target`) stands near the first stretch of the line from `eye` to `at`, where an
/// explosive would burst on them close to the bot.
pub(crate) fn crowded(
    beliefs: &lb_knowledge::Beliefs,
    eye: Vec3,
    at: Vec3,
    target: Option<lb_knowledge::PlayerKey>,
) -> bool {
    let dir = (at - eye).normalize_or_zero();
    beliefs
        .tracks
        .iter()
        .filter(|t| t.state == TrackState::Visible && Some(t.who) != target)
        .any(|t| {
            let along = (t.pos - eye).dot(dir).clamp(0.0, CROWD_REACH);
            (eye + dir * along).distance(t.pos) <= CROWD_RADIUS
        })
}

/// An enemy in sight, or lost half a second ago with a tight position: sure enough to set off a trap on.
pub(crate) fn fresh(t: &EnemyTrack, now: SimTime) -> bool {
    t.state == TrackState::Visible || (t.age(now) <= 0.5 && t.sigma < 60.0)
}

/// Where the enemy will be `secs` from now, going on as it goes (where it is when its motion is not known).
pub(crate) fn ahead(t: &EnemyTrack, now: SimTime, secs: f32) -> Vec3 {
    if t.velocity_known(now) {
        t.pos + t.vel * secs
    } else {
        t.pos
    }
}

/// The damage satchels lying at `charges`, `damage` each, would do together to a player at `p` (walls not reckoned).
/// What a grenade at `t` is planned at: where it is and goes, how far off that may already be and how fast it
/// spreads while the grenade is out.
fn grenade_aim(t: &EnemyTrack, now: SimTime) -> Aim {
    let seen = t.state == TrackState::Visible;
    Aim {
        pos: t.pos,
        vel: if t.velocity_known(now) { t.vel } else { Vec3::ZERO },
        sigma: t.sigma.min(THROW_TRACK_SIGMA),
        spread: if seen { GRENADE_SPREAD } else { GRENADE_SPREAD_LOST },
    }
}

fn satchel_damage(charges: &[Vec3], p: Vec3, damage: f32) -> f32 {
    let radius = blast_radius(damage);
    charges
        .iter()
        .map(|c| (damage * (1.0 - c.distance(p) / radius)).max(0.0))
        .sum()
}

/// Room behind for a charged gauss shot's throw: how far along `back` the bot can slide before the floor drops away
/// further than is safe; unbounded when a wall stops the throw first or no drop is near.
fn recoil_room(tracer: &mut dyn Tracer, origin: Vec3, back: Vec2) -> f32 {
    const STEP: f32 = 80.0;
    const REACH: f32 = 720.0;
    let dir = back.extend(0.0);
    let wall = tracer.trace(&TraceQuery::line(origin, origin + dir * REACH)).fraction * REACH;
    let mut d = STEP;
    while d < wall {
        // `drops` looks a fifth of a second along a velocity.
        if drops(tracer, origin, back * d * 5.0) {
            return d - STEP;
        }
        d += STEP;
    }
    f32::INFINITY
}

/// How far a gauss beam goes.
const BEAM_REACH: f32 = 8192.0;

/// The most damage a charged gauss beam fired from `eye` along `dir` may carry for a miss to come back at the shooter
/// (vanilla HLDM): the first wall along it (`hit`, traced through players) met square, and thick enough not to be
/// punched through. The game then starts the beam over from the gun with the shooter no longer left out. Worked out
/// as the game does: on from inside the wall to the beam's end, back to where it would come out.
fn backfire(tracer: &mut dyn Tracer, eye: Vec3, dir: Vec3, hit: &Trace) -> f32 {
    if hit.fraction >= 1.0 || hit.start_solid || -hit.normal.dot(dir) < 0.5 {
        return 0.0;
    }
    let on = tracer.trace(&TraceQuery::line(hit.end + dir * 8.0, eye + dir * BEAM_REACH));
    if on.all_solid {
        return 0.0;
    }
    let out = tracer.trace(&TraceQuery::line(on.end, hit.end)).end;
    out.distance(hit.end).max(1.0)
}

/// On GunGame's grenade level the game hands every grenade back as it goes: many grenades, not a few good ones.
fn grenades_refilled(body: &Body) -> bool {
    body.gungame
        .is_some_and(|g| g.kit == Kit::Throwable(WeaponId::HandGrenade))
}

/// How many of the throwable `w` the bot carries and may use.
pub(crate) fn carried(body: &Body, w: WeaponId) -> i32 {
    if body.allows(w) {
        body.arsenal
            .iter()
            .find(|a| a.id == w)
            .and_then(|a| a.reserve)
            .unwrap_or(0)
    } else {
        0
    }
}

fn clear_line(tracer: &mut dyn Tracer, from: Vec3, to: Vec3) -> bool {
    tracer.trace(&TraceQuery::line(from, to)).fraction >= 0.95
}

impl BotBrain {
    pub(crate) fn hands<'a>(&self, body: &'a Body) -> Hands<'a> {
        let dll = body.dll;
        Hands {
            now: body.now,
            eye: body.eye,
            origin: body.origin,
            velocity: body.velocity,
            view: self.motor.view,
            on_ground: body.on_ground,
            on_ladder: body.on_ladder,
            waterlevel: body.waterlevel,
            fov: body.fov,
            weapon: body.weapon,
            arsenal: &body.arsenal,
            prediction: body.prediction.as_ref(),
            dll,
            gravity: body.gravity,
            deploying: self.motor.weapon.deploying(body.now, body.weapon),
        }
    }

    /// Ten times a second: start a detonation, a mine shot, a launched grenade, a throw or a mine when one is worth
    /// it.
    pub(crate) fn weapon_options(&mut self, body: &Body, ch: &Character, nav: &mut dyn NavService, rng: &mut BotRng) {
        self.keep_off_beams(body, nav);
        let (forward, _, _) = view_angle_vectors(self.motor.view);
        let back = -forward.truncate().normalize_or(Vec2::X);
        self.mind.arms.recoil_room = if body.weapon == Some(WeaponId::Gauss) {
            recoil_room(nav, body.origin, back)
        } else {
            0.0
        };
        let satchel_out = body
            .prediction
            .and_then(|p| p.weapons[WeaponId::Satchel as usize])
            .map(|p| p.charge_ready);
        if satchel_out == Some(0) && !self.mind.arms.busy() {
            // The game reports none out: the charges went off or were taken away with the satchel.
            self.explosives.detonated();
        }
        if self.mind.arms.busy() || body.on_ladder {
            return;
        }
        if satchel_out == Some(1) && self.detonation(body, rng) {
            return;
        }
        if self.mine_shots(body, nav) || self.trail_drop(body, nav) {
            return;
        }
        if self.lob(body, nav, rng) {
            return;
        }
        if self.barrage(body, ch, rng) {
            return;
        }
        if self.throw(body, ch, nav, rng) {
            return;
        }
        self.lay_mine(body, nav, rng);
    }

    /// Tells navigation of every armed tripmine beam the bot knows of (its own mines' beams are traced once), again
    /// before the last word runs out; the beams of mines gone are lifted (and the others told again: a link may have
    /// been under two).
    fn keep_off_beams(&mut self, body: &Body, nav: &mut dyn NavService) {
        let now = body.now;
        if !self.explosives.lifted.is_empty() {
            for (a, b) in std::mem::take(&mut self.explosives.lifted) {
                nav.clear_line(a, b);
            }
            for m in &mut self.explosives.mines {
                m.avoided_at = None;
            }
        }
        for m in &mut self.explosives.mines {
            let from = if m.own {
                SimTime(m.armed_at.0 - OWN_BEAM_SOON)
            } else {
                m.armed_at
            };
            if now < from || m.avoided_at.is_some_and(|t| now.since(t) < BEAM_REPORT) {
                continue;
            }
            let end = match m.beam_end {
                Some(e) => e,
                None if m.dir != Vec3::ZERO => {
                    let e = nav.trace(&TraceQuery::line(m.pos, m.pos + m.dir * BEAM_LENGTH)).end;
                    m.beam_end = Some(e);
                    e
                }
                None => continue,
            };
            // Beams the bot cannot get past (see `beam_guard`) keep the way off them; paths by the others cost more,
            // told once the bot is off from the mine (not to turn it back along the way it is on).
            if *m.pass.get_or_insert_with(|| beam_pass(nav, m.pos, end)) == BeamPass::Blocked {
                nav.avoid_line(m.pos, end, BEAM_AVOID);
            } else if m.pos.distance(body.origin) >= SHUN_FROM {
                nav.shun_line(m.pos, end, SHUN_RADIUS, SHUN_COST, BEAM_AVOID);
            } else {
                continue;
            }
            m.avoided_at = Some(now);
        }
    }

    /// Where the bot's satchels lie now.
    fn charges_at(&self, body: &Body) -> smallvec::SmallVec<[Vec3; 5]> {
        self.explosives
            .charges
            .iter()
            .map(|c| c.at(body.now, body.gravity))
            .collect()
    }

    /// An enemy in sight close to the bot and away from its satchels (neither in their blast nor coming into it): it
    /// needs a gun rather than the radio.
    fn radio_threat(&self, body: &Body, charges: &[Vec3]) -> bool {
        let now = body.now;
        let damage = body.damages.satchel;
        self.beliefs.visible_enemies().any(|t| {
            let by_them = satchel_damage(charges, t.pos, damage).max(satchel_damage(
                charges,
                ahead(t, now, SATCHEL_COMING),
                damage,
            ));
            t.pos.distance(body.origin) < SATCHEL_THREAT && by_them < SATCHEL_WORTH / 2.0
        })
    }

    /// Where the bot watches its satchels from while it waits with the radio up: the one nearest to where an enemy is
    /// believed to be, else the middle of them.
    fn charges_watch(&self, body: &Body) -> Option<Vec3> {
        let charges = self.charges_at(body);
        let nearest_to = |p: Vec3| {
            charges
                .iter()
                .copied()
                .min_by(|a, b| a.distance(p).total_cmp(&b.distance(p)))
        };
        let gap = |t: &EnemyTrack| nearest_to(t.pos).map_or(f32::INFINITY, |c| c.distance(t.pos));
        let enemy = self
            .beliefs
            .enemies()
            .filter(|t| t.state != TrackState::Stale)
            .min_by(|a, b| gap(a).total_cmp(&gap(b)));
        let at = match enemy {
            Some(t) => nearest_to(t.pos)?,
            None if !charges.is_empty() => charges.iter().copied().sum::<Vec3>() / charges.len() as f32,
            None => return None,
        };
        Some(at + Vec3::Z * 16.0)
    }

    /// The bot is out of the blast of its satchels at `charges`, with a margin: a blast reaches a little further than
    /// its radius (it bursts above the floor and is measured to the near side of a body), and a satchel in flight is
    /// known a moment late.
    fn spared(body: &Body, charges: &[Vec3]) -> bool {
        let clear = blast_radius(body.damages.satchel) + SATCHEL_SPARED;
        charges.iter().all(|c| c.distance(body.origin) >= clear)
    }

    /// Sets the bot's satchels off, or draws the radio for them, once it is out of their blast (it backs off first):
    /// - with an enemy in their blast where it will be by the time they go off (the radio drawn, pressed while the
    ///   enemy is in the blast), or coming into it;
    /// - the bot dying with an enemy by them, someone heard by them, or an enemy seen by them a moment ago that may
    ///   still be there (pressed at once);
    /// - the radio up while an enemy is expected by them (a trap watched);
    /// - after they have lain a while with nobody in sight.
    fn detonation(&mut self, body: &Body, rng: &mut BotRng) -> bool {
        let now = body.now;
        if self.explosives.charges.is_empty() {
            self.mind.arms.satchels = None;
            return false;
        }
        let plan = *self
            .mind
            .arms
            .satchels
            .get_or_insert_with(|| SatchelPlan::roll(now, rng));
        if self
            .explosives
            .charges
            .iter()
            .any(|c| now.since(c.since) < SATCHEL_SETTLE)
        {
            return false;
        }
        let charges = self.charges_at(body);
        let damage = body.damages.satchel;
        let hurts = |p: Vec3| satchel_damage(&charges, p, damage);
        let lead = if body.weapon == Some(WeaponId::Satchel) {
            RADIO_PRESS
        } else {
            RADIO_DRAW
        };
        let victim = self
            .beliefs
            .enemies()
            .any(|t| fresh(t, now) && hurts(ahead(t, now, lead)) >= SATCHEL_WORTH);
        let coming = self
            .beliefs
            .enemies()
            .any(|t| fresh(t, now) && t.velocity_known(now) && hurts(ahead(t, now, SATCHEL_COMING)) >= SATCHEL_WORTH);
        let dying = body.health <= SATCHEL_DYING
            && self
                .beliefs
                .last_damage
                .is_some_and(|d| now.since(d.t) <= SATCHEL_DYING_HURT)
            && self
                .beliefs
                .enemies()
                .any(|t| t.state != TrackState::Stale && hurts(t.pos) >= SATCHEL_WORTH / 2.0);
        // In team games a sound may be a friend's: only one tied to an enemy counts there.
        let friends = self.beliefs.tracks.iter().any(|t| t.relation != Relation::Enemy);
        let heard = self.beliefs.hypotheses.iter().any(|h| {
            now.since(h.t) <= SATCHEL_HEARD
                && matches!(
                    h.kind,
                    HypothesisKind::Sound(
                        SoundKind::Step
                            | SoundKind::Jump
                            | SoundKind::Pain
                            | SoundKind::Shot
                            | SoundKind::WeaponNoise
                            | SoundKind::Pickup
                    )
                )
                && h.pos
                    .is_some_and(|p| charges.iter().any(|c| c.distance(p) <= SATCHEL_HEARD_NEAR))
                && (!friends
                    || h.track
                        .and_then(|k| self.beliefs.track(k))
                        .is_some_and(|t| t.relation == Relation::Enemy))
        });
        // Seen in their blast a moment ago: it may have come closer out of sight.
        let lost = self.beliefs.enemies().any(|t| {
            let gone = now.since(t.last_seen);
            t.state != TrackState::Visible && gone >= plan.lost_wait && gone <= SATCHEL_LOST_NEAR && hurts(t.pos) > 0.0
        });
        let calm = self.beliefs.visible_enemies().next().is_none();
        // The radio does not wait up with an enemy away from them at the bot.
        let free = !self.radio_threat(body, &charges);
        let (why, wait) = if victim && free {
            ("an enemy in their blast", Some(now + SATCHEL_WAIT))
        } else if dying {
            ("dying by them", None)
        } else if heard {
            ("someone heard by them", None)
        } else if lost {
            ("an enemy seen by them a moment ago", None)
        } else if coming && free {
            ("an enemy coming at them", Some(now + SATCHEL_WAIT))
        } else if now < self.mind.arms.radio_until && free {
            ("an enemy expected by them", Some(self.mind.arms.radio_until))
        } else if calm && now >= plan.lie_until {
            ("lying long enough", None)
        } else {
            return false;
        };
        // Pressed at once: from out of their blast.
        if wait.is_none() && !Self::spared(body, &charges) {
            self.back_off_charges(&charges, body);
            return true;
        }
        self.mind.arms.detonate_why = why;
        self.mind.arms.detonate_wait = wait;
        self.mind.arms.active = Some(Active::Detonate(SatchelTrigger::new(now)));
        true
    }

    /// Runs from the nearest of the bot's satchels, out of its blast.
    fn back_off_charges(&mut self, charges: &[Vec3], body: &Body) {
        let Some(nearest) = charges
            .iter()
            .copied()
            .min_by(|a, b| a.distance(body.origin).total_cmp(&b.distance(body.origin)))
        else {
            return;
        };
        let away = (body.origin - nearest).truncate().normalize_or(Vec2::X);
        self.mind.arms.dodge = Some((body.now + DODGE_FOR, away));
    }

    /// Whether the radio up for the bot's satchels presses now: at once for a reason that holds on its own, else
    /// while an enemy is in their blast; never with the bot in it, which it may have come into since the radio went
    /// up (it backs off, or gives up a press meant at once). The wait ends when it runs out, and when an enemy in
    /// sight away from the charges comes close: that one needs a gun.
    fn detonate_go(&mut self, body: &Body) -> Result<bool, &'static str> {
        let now = body.now;
        let charges = self.charges_at(body);
        if charges.is_empty() {
            return Err("no charges known");
        }
        let spared = Self::spared(body, &charges);
        let Some(until) = self.mind.arms.detonate_wait else {
            return if spared {
                Ok(true)
            } else {
                Err("the bot in their blast")
            };
        };
        let damage = body.damages.satchel;
        let hurts = |p: Vec3| satchel_damage(&charges, p, damage);
        let victim = self
            .beliefs
            .enemies()
            .any(|t| fresh(t, now) && hurts(ahead(t, now, RADIO_PRESS)) >= SATCHEL_WORTH);
        if victim {
            if spared {
                return Ok(true);
            }
            self.back_off_charges(&charges, body);
        }
        if self.radio_threat(body, &charges) {
            self.mind.arms.radio_until = now;
            return Err("an enemy away from them");
        }
        // The radio stays up while an enemy is still expected by them (a trap watched renews it).
        if now > until.max(self.mind.arms.radio_until) {
            return Err("nobody came by them");
        }
        Ok(false)
    }

    /// Ten times a second: all the snarks at an enemy in sight close by, when the bot carries a few. Swarmed, the
    /// enemy is bitten where it stands.
    fn barrage(&mut self, body: &Body, ch: &Character, rng: &mut BotRng) -> bool {
        let now = body.now;
        if now < self.mind.arms.next_barrage {
            return false;
        }
        self.mind.arms.next_barrage = now + THROW_CHECK;
        let snarks = body.armed(WeaponId::Snark).and_then(|a| a.reserve).unwrap_or(0);
        // A hurt bot keeps away from a swarm that may turn on it.
        if snarks < BARRAGE_SNARKS
            || body.health < BARRAGE_HEALTH
            || !body.allows(WeaponId::Snark)
            || body.waterlevel >= 2
        {
            return false;
        }
        let Some(t) = self
            .mind
            .target
            .and_then(|k| self.beliefs.track(k))
            .filter(|t| t.state == TrackState::Visible)
        else {
            return false;
        };
        let d = t.pos.distance(body.origin);
        if !(BARRAGE_BAND[0]..=BARRAGE_BAND[1]).contains(&d) || (t.pos.z - body.origin.z).abs() > 72.0 {
            return false;
        }
        let bold = if ch.aggression > ch.fear { 1.1 } else { 0.9 };
        if rng.combat.next_f32() >= (BARRAGE_CHANCE * ch.skill.throw_rate * ch.weapons.throwables * bold).min(0.9) {
            return false;
        }
        self.mind.arms.active = Some(Active::Barrage(Barrage::new(t.who, now)));
        self.mind.arms.next_barrage = now + f64::from(rng.combat.range_f32(THROW_REST[0], THROW_REST[1]));
        true
    }

    fn lob(&mut self, body: &Body, nav: &mut dyn NavService, rng: &mut BotRng) -> bool {
        let now = body.now;
        let m = &self.mind;
        if body.weapon != Some(WeaponId::Mp5)
            || now < m.arms.next_lob
            || body.waterlevel >= 3
            || !body.allows(WeaponId::Mp5)
            || body
                .arsenal
                .iter()
                .find(|a| a.id == WeaponId::Mp5)
                .and_then(|a| a.reserve2)
                .unwrap_or(0)
                <= 0
        {
            return false;
        }
        let Some(t) = m
            .target
            .and_then(|k| self.beliefs.track(k))
            .filter(|t| t.state == TrackState::Visible)
        else {
            return false;
        };
        let d = t.pos.distance(body.eye);
        if !(LOB_BAND[0]..=LOB_BAND[1]).contains(&d) {
            return false;
        }
        let lead = if t.velocity_known(now) {
            t.vel * (d / 700.0)
        } else {
            Vec3::ZERO
        };
        let feet = t.pos + lead - Vec3::Z * 32.0;
        let clear = if body.health < LOB_HURT {
            LOB_HURT_CLEAR
        } else {
            LOB_CLEAR
        };
        if feet.distance(body.origin) < clear || crowded(&self.beliefs, body.eye, t.pos, Some(t.who)) {
            return false;
        }
        self.mind.arms.next_lob = now + f64::from(rng.combat.range_f32(2.5, 4.0));
        let Some(throw) = ballistics::m203(nav, body.eye, feet, body.gravity) else {
            return false;
        };
        self.mind.arms.active = Some(Active::Lob(Lob::new(throw, now)));
        true
    }

    fn throw(&mut self, body: &Body, ch: &Character, nav: &mut dyn NavService, rng: &mut BotRng) -> bool {
        let now = body.now;
        if now < self.mind.arms.next_throw || self.mind.reloading(now) {
            return false;
        }
        self.mind.arms.next_throw = now + THROW_CHECK;
        // A series ends with an enemy in sight close by (the gun then), or with one of its grenades come down near
        // the bot (off an edge on the way).
        let series = self.mind.arms.series
            && !self
                .beliefs
                .visible_enemies()
                .any(|t| t.pos.distance(body.origin) < SERIES_CLOSE)
            && !self
                .explosives
                .own_grenades
                .iter()
                .any(|g| g.at.distance(body.origin) < SERIES_SAFE);
        self.mind.arms.series = series;
        // With no gun but the crowbar, throws are the weapon (yapb's grenade war).
        let war = !body.arsenal.iter().any(|a| {
            let class = spec(a.id).class;
            body.allows(a.id) && a.loaded() && !matches!(class, WeaponClass::Melee | WeaponClass::Throwable)
        });
        let refilled = grenades_refilled(body);
        let Some(t) = self
            .beliefs
            .enemies()
            .filter(|t| {
                t.state == TrackState::Visible
                    || (matches!(t.state, TrackState::RecentlyLost | TrackState::Predicted)
                        && t.age(now) <= THROW_TRACK_AGE
                        && t.sigma <= THROW_TRACK_SIGMA)
            })
            .min_by(|a, b| a.pos.distance(body.origin).total_cmp(&b.pos.distance(body.origin)))
        else {
            return self.throw_blind(body, ch, nav, rng);
        };
        let seen = t.state == TrackState::Visible;
        let above = t.pos.z - body.origin.z;
        if above > TOO_HIGH || (!t.traits.on_ground && above > 72.0) {
            return false;
        }
        let d = (t.pos - body.origin).truncate().length();
        let count = |w: WeaponId| carried(body, w);
        let grenade_band = if war {
            [WAR_GRENADE_MIN, GRENADE_BAND[1]]
        } else {
            GRENADE_BAND
        };
        let in_band = |band: [f32; 2]| (band[0]..=band[1]).contains(&d);
        // The throws that fit, with their chance per look: a grenade; a satchel on the run at an enemy in sight, set
        // off as it comes by; a snark.
        let satchels = count(WeaponId::Satchel);
        let free = self.explosives.charges.is_empty();
        let mut ways: smallvec::SmallVec<[(Way, f32); 4]> = smallvec::SmallVec::new();
        let grenade = count(WeaponId::HandGrenade) > 0 && in_band(grenade_band);
        if grenade {
            ways.push((Way::Grenade, if seen { SEEN_GRENADE } else { UNSEEN_GRENADE }));
        }
        // From a jump: a trick, for skills that do tricks; not where a suicide costs a GunGame kill. With the long jump
        // module from a long jump, at an enemy further off.
        let jumps = ch.skill.tricks && body.tricks.satchel_jump && !body.gungame.is_some_and(|g| g.descore());
        let leaps = jumps && body.has_longjump && body.tricks.longjump;
        let satchel_band = [AIRBURST_BAND[0], if leaps { LEAP_BAND[1] } else { AIRBURST_BAND[1] }];
        if satchels > 0 && free && seen && body.on_ground && in_band(satchel_band) {
            ways.push((Way::Airburst, SEEN_SATCHEL));
        }
        if count(WeaponId::Snark) > 0 && body.waterlevel < 2 && above <= SNARK_TOO_HIGH && in_band(SNARK_BAND) {
            ways.push((Way::Snark, if seen { SEEN_SNARK } else { UNSEEN_SNARK }));
        }
        let way = if series && grenade {
            // The grenades of a series go without a second thought.
            Way::Grenade
        } else {
            let Some(best) = ways.iter().map(|w| w.1).max_by(f32::total_cmp) else {
                self.mind.arms.series = false;
                return false;
            };
            let bold = if ch.aggression > ch.fear { 1.1 } else { 0.9 };
            let rate = ch.skill.throw_rate * ch.weapons.throwables;
            let chance = (best * rate * bold * if war { 2.0 } else { 1.0 }).min(THROW_CHANCE_MAX);
            if rng.combat.next_f32() >= chance {
                return false;
            }
            // One of them, as likely as its chance.
            let mut pick = rng.combat.next_f32() * ways.iter().map(|w| w.1).sum::<f32>();
            ways.iter()
                .find(|w| {
                    pick -= w.1;
                    pick <= 0.0
                })
                .or(ways.last())
                .map_or(Way::Grenade, |w| w.0)
        };
        if way == Way::Snark && seen {
            // A stream of snarks at an enemy in sight: held down, the game lets one go every 0.3 s.
            let carried = count(WeaponId::Snark).max(1) as u32;
            let n = rng
                .combat
                .range_f32(SNARK_STREAM[0] as f32, SNARK_STREAM[1] as f32 + 0.99) as u32;
            self.mind.arms.active = Some(Active::Barrage(Barrage::new(t.who, now).stream(n.min(carried))));
            self.mind.arms.next_throw = now + f64::from(rng.combat.range_f32(SNARK_REST[0], SNARK_REST[1]));
            self.mind.arms.hold_until = self.mind.arms.hold_until.max(now + SNARK_HOLD);
            return true;
        }
        // A target in sight is led by where it will be when the throw comes down.
        let lead = if seen && t.velocity_known(now) {
            t.vel.truncate().extend(0.0) * (WAR_LEAD + d / 650.0)
        } else {
            Vec3::ZERO
        };
        let floor = t.pos + lead - Vec3::Z * 32.0;
        let mut planned = None;
        let (kind, throw) = match way {
            Way::Grenade => {
                // Planned to burst where the enemy will be; on the run, with the run in the throw, flat at an enemy in
                // sight. Cooked as long as it pays only at one out of sight, or close by on the grenade level; the bot
                // backs off from it once it is out.
                let aim = grenade_aim(t, now);
                let quick = refilled && d >= QUICK_FROM;
                let run = body.waterlevel < 2
                    && d >= if refilled && !quick {
                        QUICK_FROM
                    } else {
                        GRENADE_RUN_MIN
                    };
                let velocity = if run {
                    (t.pos - body.origin).truncate().normalize_or_zero().extend(0.0) * GRENADE_RUN_SPEED
                } else {
                    body.velocity
                };
                let delivery = grenade::Delivery {
                    latest: if quick {
                        QUICK_COOK
                    } else if seen && !refilled {
                        SEEN_COOK
                    } else {
                        grenade::LATEST
                    },
                    quick,
                    flat: run && seen,
                    retreat: seen || refilled,
                };
                let plan = grenade::plan(
                    nav,
                    body.eye,
                    body.origin,
                    velocity,
                    &aim,
                    body.gravity,
                    body.dll,
                    0.0,
                    0.0,
                    None,
                    delivery,
                );
                let worth = if refilled { REFILLED_WORTH } else { GRENADE_WORTH };
                match plan {
                    Some(p) if p.damage >= worth => {
                        planned = Some((aim, p, run, delivery));
                        (Kind::Grenade, p.throw)
                    }
                    _ => {
                        self.mind.arms.series = false;
                        return false;
                    }
                }
            }
            Way::Airburst | Way::Snark => {
                if !clear_line(nav, body.eye, floor + Vec3::Z * 8.0) {
                    return false;
                }
                let kind = if way == Way::Snark { Kind::Snark } else { Kind::Satchel };
                (
                    kind,
                    ballistics::satchel(nav, body.origin, body.velocity, floor, body.gravity),
                )
            }
        };
        let target = if kind == Kind::Snark { t.pos } else { floor };
        let mut thrower = Thrower::new(kind, target, throw, now);
        if let Some((aim, plan, run, delivery)) = planned {
            thrower = thrower.planned(aim, plan, delivery);
            if run {
                // From a jump at an enemy a little off or above, as often as the style likes; with the module a long
                // jump at one far off.
                let jump = ch.skill.tricks
                    && body.tricks.grenade_jump
                    && (d >= GRENADE_JUMP_MIN || above >= GRENADE_JUMP_ABOVE)
                    && rng.combat.next_f32() < ch.tricks.grenade_jump;
                let leap = jump && body.has_longjump && body.tricks.longjump && d >= GRENADE_LEAP_MIN;
                thrower = match (jump, leap) {
                    (true, true) => thrower.from_leap(),
                    (true, false) => thrower.from_jump(),
                    (false, _) => thrower.on_the_run(),
                };
            }
        }
        if way == Way::Airburst {
            // As often as the style likes jumps.
            let jump = jumps && rng.combat.next_f32() < ch.tricks.satchel_jump;
            thrower = match (jump, leaps && d >= LEAP_BAND[0]) {
                (true, true) => thrower.from_leap(),
                (true, false) => thrower.from_jump(),
                (false, _) => thrower.on_the_run(),
            };
        }
        // At an enemy in sight a grenade goes at once: it will not wait for a cooked one.
        if seen {
            thrower = thrower.quick();
        }
        self.mind.arms.throw_aim = matches!(way, Way::Airburst | Way::Grenade).then_some(t.who);
        self.mind.arms.landed = 0;
        self.mind.arms.active = Some(Active::Throw(thrower));
        let rest = match kind {
            Kind::Grenade => {
                self.mind.arms.series = ch.skill.throw_series && count(WeaponId::HandGrenade) > 1;
                GRENADE_REST
            }
            Kind::Satchel => THROW_REST,
            Kind::Snark => SNARK_REST,
        };
        self.mind.arms.next_throw = now + f64::from(rng.combat.range_f32(rest[0], rest[1]));
        true
    }

    /// A grenade where an enemy is expected when none is in sight or lost a moment ago: where a lost one would come
    /// into view, a sound heard lately, the busiest way into the bot's sight, a little off the spot. Now and then,
    /// or one after another in a series.
    fn throw_blind(&mut self, body: &Body, ch: &Character, nav: &mut dyn NavService, rng: &mut BotRng) -> bool {
        let now = body.now;
        let grenades = carried(body, WeaponId::HandGrenade);
        if grenades == 0 || body.waterlevel >= 2 {
            self.mind.arms.series = false;
            return false;
        }
        if !self.mind.arms.series {
            let chance = BLIND_GRENADE * ch.skill.throw_rate * ch.weapons.throwables;
            if rng.combat.next_f32() >= chance {
                return false;
            }
        }
        let heard = self
            .beliefs
            .hypotheses
            .iter()
            .filter(|h| matches!(h.kind, HypothesisKind::Sound(_)) && now.since(h.t) <= BLIND_HEARD)
            .max_by(|a, b| a.t.0.total_cmp(&b.t.0))
            .and_then(|h| h.pos);
        let spots = [self.expect.map(|(_, p)| p), heard, self.approach];
        for spot in spots.into_iter().flatten() {
            if !(BLIND_BAND[0]..=BLIND_BAND[1]).contains(&(spot - body.origin).truncate().length()) {
                continue;
            }
            let off = Vec2::new(rng.combat.range_f32(-1.0, 1.0), rng.combat.range_f32(-1.0, 1.0)) * BLIND_SCATTER;
            let aim = Aim {
                pos: spot + off.extend(0.0),
                vel: Vec3::ZERO,
                sigma: BLIND_SCATTER,
                spread: GRENADE_SPREAD_LOST,
            };
            if aim.pos.distance(body.origin) < SELF_CLEAR {
                continue;
            }
            let run = body.on_ground && (aim.pos - body.origin).truncate().length() >= GRENADE_RUN_MIN;
            let velocity = if run {
                (aim.pos - body.origin).truncate().normalize_or_zero().extend(0.0) * GRENADE_RUN_SPEED
            } else {
                body.velocity
            };
            // Cooked to burst as it gets there: whoever comes through has no time to get away from it. With grenades
            // handed back as they go, at once.
            let (delivery, worth) = if grenades_refilled(body) {
                let quick = grenade::Delivery {
                    latest: QUICK_COOK,
                    quick: true,
                    flat: false,
                    retreat: true,
                };
                (quick, REFILLED_WORTH)
            } else {
                (grenade::Delivery::COOKED, GRENADE_WORTH)
            };
            let Some(plan) = grenade::plan(
                nav,
                body.eye,
                body.origin,
                velocity,
                &aim,
                body.gravity,
                body.dll,
                0.0,
                0.0,
                None,
                delivery,
            )
            .filter(|p| p.damage >= worth) else {
                continue;
            };
            self.mind.arms.throw_aim = None;
            self.mind.arms.landed = 0;
            let mut thrower = Thrower::new(Kind::Grenade, plan.target, plan.throw, now).planned(aim, plan, delivery);
            if run {
                thrower = thrower.on_the_run();
            }
            self.mind.arms.active = Some(Active::Throw(thrower));
            self.mind.arms.series = ch.skill.throw_series && grenades > 1;
            self.mind.arms.stats.blind += 1;
            self.mind.arms.next_throw = now + f64::from(rng.combat.range_f32(GRENADE_REST[0], GRENADE_REST[1]));
            return true;
        }
        self.mind.arms.series = false;
        false
    }

    /// Puts a tripmine on the wall at `wall` (the trap goal brought the bot where it reaches it).
    pub(crate) fn plant_mine(&mut self, wall: Vec3, normal: Vec3, body: &Body) -> bool {
        if self.mind.arms.busy() {
            return false;
        }
        let now = body.now;
        self.mind.arms.active = Some(Active::Mine(Planter::new(wall, normal, now)));
        self.mind.arms.next_mine = self.mind.arms.next_mine.max(now + f64::from(MINE_REST[0]));
        true
    }

    /// Throws a pile of satchels at `at` (a chokepoint the trap goal watches); they lie long before going off with
    /// nobody by them.
    /// Throws a pile of `pile` satchels (as many as it carries) at `at` for a trap: they lie long.
    pub(crate) fn trap_satchels(
        &mut self,
        body: &Body,
        nav: &mut dyn NavService,
        at: Vec3,
        pile: [u32; 2],
        rng: &mut BotRng,
    ) -> bool {
        let carried = body.armed(WeaponId::Satchel).and_then(|a| a.reserve).unwrap_or(0);
        if self.mind.arms.busy() || carried < pile[0] as i32 || !body.allows(WeaponId::Satchel) {
            return false;
        }
        let now = body.now;
        let throw = ballistics::satchel(nav, body.origin, body.velocity, at, body.gravity);
        let pile = rng.combat.range_f32(pile[0] as f32, pile[1] as f32 + 0.99) as u32;
        let thrower = Thrower::new(Kind::Satchel, at, throw, now).pile(pile.min(carried as u32));
        self.mind.arms.throw_aim = None;
        self.mind.arms.trap_throw = true;
        self.mind.arms.landed = 0;
        self.mind.arms.active = Some(Active::Throw(thrower));
        self.mind.arms.next_throw = now + f64::from(rng.combat.range_f32(THROW_REST[0], THROW_REST[1]));
        true
    }

    fn lay_mine(&mut self, body: &Body, nav: &mut dyn NavService, rng: &mut BotRng) {
        let now = body.now;
        if now < self.mind.arms.next_mine {
            return;
        }
        self.mind.arms.next_mine = now + f64::from(rng.combat.range_f32(1.5, 3.0));
        // On the GunGame tripmine level mines go down in trails only, as a player lays them there.
        if body.gungame.is_some_and(|g| g.kit == Kit::Mines) {
            return;
        }
        let walking = matches!(
            self.mind.goal.map(|g| g.kind),
            Some(GoalKind::Roam | GoalKind::CollectItem(_))
        );
        let quiet = self.calm_for(now) >= QUIET;
        let speed = body.velocity.truncate().length();
        if !walking
            || speed < 100.0
            || !body.on_ground
            || body.waterlevel >= 2
            || !quiet
            || !body.allows(WeaponId::Tripmine)
            || body
                .arsenal
                .iter()
                .find(|a| a.id == WeaponId::Tripmine)
                .and_then(|a| a.reserve)
                .unwrap_or(0)
                <= 0
        {
            return;
        }
        let heading = body.velocity.truncate() / speed;
        let across = Vec2::new(-heading.y, heading.x).extend(0.0);
        let waist = body.origin + Vec3::Z * 8.0;
        let side = |tracer: &mut dyn Tracer, dir: Vec3| {
            let tr = tracer.trace(&TraceQuery::line(waist, waist + dir * CORRIDOR));
            (tr.fraction < 1.0 && tr.normal.z.abs() <= 0.3).then_some((tr.end, tr.normal, tr.fraction * CORRIDOR))
        };
        let (Some(left), Some(right)) = (side(nav, across), side(nav, -across)) else {
            return;
        };
        if left.2 + right.2 > CORRIDOR {
            return;
        }
        let (spot, normal, near) = if left.2 <= right.2 { left } else { right };
        if near > WALL_NEAR {
            return;
        }
        let too_close = self.spawns.iter().any(|s| s.distance(spot) < SPAWN_CLEAR)
            || self
                .explosives
                .mines
                .iter()
                .any(|m| m.pos.distance(spot) < MINE_SPACING);
        if too_close {
            return;
        }
        self.mind.arms.active = Some(Active::Mine(Planter::new(spot, normal, now)));
        self.mind.arms.next_mine = now + f64::from(rng.combat.range_f32(MINE_REST[0], MINE_REST[1]));
    }

    /// Every frame: the gauss charge, the running protocol and a rocket in flight get their say at `Prio::Protocol`.
    pub(crate) fn run_protocols(&mut self, body: &Body, ch: &Character, nav: &mut dyn NavService, rng: &mut BotRng) {
        let now = body.now;
        self.mind.arms.watch_zoom(body.zoomed(), now);
        let hands = self.hands(body);
        let mut requests: smallvec::SmallVec<[Request; 2]> = smallvec::SmallVec::new();
        // Navigation stands at a boost's takeoff and asks for it: the boost starts when nothing else runs.
        if let Some(call) = self.mind.nav_boost
            && !self.mind.arms.busy()
        {
            self.mind.arms.active = Some(Active::GaussBoost(GaussBoost::new(now, call.view, call.charge)));
            self.mind.tricks.stats.boosts += 1;
        }
        let boosting = matches!(self.mind.arms.active, Some(Active::GaussBoost(_)));
        if !boosting && (body.weapon == Some(WeaponId::Gauss) || self.mind.arms.gauss.active()) {
            let seen = self
                .mind
                .target
                .and_then(|k| self.beliefs.track(k))
                .filter(|t| t.state == TrackState::Visible)
                .map(|t| {
                    let aim = self.mind.last_aim.unwrap_or(t.pos);
                    (t.pos.distance(body.eye), on_target(self.motor.view, body.eye, aim))
                });
            let through = if seen.is_none() {
                self.wallbang(body, ch, nav)
            } else {
                None
            };
            let target =
                seen.or_else(|| through.map(|(p, _)| (p.distance(body.eye), on_target(self.motor.view, body.eye, p))));
            let expected = matches!(self.mind.goal.map(|g| g.kind), Some(GoalKind::Hunt(_))) || through.is_some();
            let fired = self.mind.arms.gauss.fired;
            let input = GaussInput {
                target,
                expected,
                precharge: ch.skill.gauss_precharge,
                recoil_room: self.mind.arms.recoil_room,
                wall_ahead: self.gauss_wall(body, nav),
                full_damage: body.damages.gauss_charged,
                heading: body.velocity.truncate().normalize_or_zero(),
                allowed: body.allows(WeaponId::Gauss),
                charge_share: ch.skill.gauss_charge,
                dump: self.safe_dump(body, nav),
                backfire: self.mind.arms.gauss_wall.map_or(0.0, |w| w.2),
                min_damage: through.map_or(0.0, |(_, need)| need),
            };
            if let Some(r) = self.mind.arms.gauss.update(&hands, &input, &mut rng.combat) {
                requests.push(r);
            }
            if through.is_some() && self.mind.arms.gauss.fired > fired {
                self.mind.arms.stats.wallbangs += 1;
                tracing::debug!("gauss shot through a wall at a lost enemy");
            }
        }
        if let Some(mut active) = self.mind.arms.active.take() {
            // On the tripmine level an enemy in sight further off does not keep a mine from going down.
            let near = if body.gungame.is_some_and(|g| g.kit == Kit::Mines) {
                MINES_NEAR
            } else {
                f32::INFINITY
            };
            let in_sight = self
                .beliefs
                .visible_enemies()
                .any(|t| t.pos.distance(body.origin) < near);
            let status = match &mut active {
                Active::Throw(t) => {
                    // A grenade's plan follows the enemy in sight; a satchel at an enemy in sight goes where the enemy
                    // is going, led by the satchel's flight.
                    if let Some(e) = self
                        .mind
                        .arms
                        .throw_aim
                        .and_then(|k| self.beliefs.track(k))
                        .filter(|e| e.state == TrackState::Visible)
                    {
                        if t.kind == Kind::Grenade {
                            t.retarget(grenade_aim(e, now));
                        } else {
                            let lead = if e.velocity_known(now) {
                                e.vel.truncate().extend(0.0) * ((e.pos - body.origin).truncate().length() / SATCHEL_RUN)
                            } else {
                                Vec3::ZERO
                            };
                            t.target = e.pos + lead - Vec3::Z * 32.0;
                        }
                    }
                    let status = t.update(&hands, nav);
                    // Each satchel of a pile is noted as it leaves the hand.
                    for landing in t.landings.iter().skip(self.mind.arms.landed) {
                        self.explosives.thrown_satchel(*landing, now);
                    }
                    self.mind.arms.landed = t.landings.len();
                    if let Some(detonate) = t.learned.take() {
                        self.mind.arms.satchel_fact = Some(detonate);
                    }
                    if std::mem::take(&mut t.set_off) {
                        self.mind.arms.stats.satchel_off("by a press meant to throw");
                        self.explosives.detonated();
                        self.mind.arms.satchels = None;
                    }
                    status
                }
                // Laying a mine with an enemy in sight is left for later.
                Active::Mine(_) if in_sight => Status::Failed("an enemy came into sight"),
                // A mine pressed for is remembered at once: the count of mines may show it late, or never.
                Active::Mine(p) => {
                    let status = p.update(&hands, nav);
                    if !p.noted
                        && let Some(at) = p.pressed_at()
                    {
                        p.noted = true;
                        self.explosives.placed_mine(p.mine(), p.normal, at);
                    }
                    status
                }
                Active::Drop(p) => {
                    let heading = match self.mind.arms.trail.as_ref().map(|t| t.dir) {
                        Some(dir) if dir != Vec2::ZERO => dir,
                        _ => body.velocity.truncate().normalize_or_zero(),
                    };
                    // Remembered once the game took the press (`finished`): a press it did not take leaves no mine.
                    p.update(&hands, nav, heading)
                }
                Active::Lob(l) => {
                    let gravity = body.gravity * lb_game::mechanics::PROJECTILE_GRAVITY;
                    let landing = ballistics::position_at(l.throw.start, l.throw.velocity, gravity, l.throw.flight);
                    let target = self.mind.target.and_then(|k| self.beliefs.track(k));
                    let clear = target.is_none_or(|t| t.pos.distance(body.eye) >= LOB_BAND[0])
                        && !crowded(&self.beliefs, body.eye, landing, target.map(|t| t.who));
                    l.update(&hands, nav, clear)
                }
                Active::Detonate(d) => match self.detonate_go(body) {
                    Ok(go) => {
                        // Waiting with the radio up: watching the charges, where the enemy is to come by them.
                        if !go && let Some(at) = self.charges_watch(body) {
                            self.intents
                                .look(Prio::Protocol, LookIntent::Point { at, engaged: false });
                        }
                        d.update(&hands, go)
                    }
                    Err(why) => Status::Failed(why),
                },
                Active::Airburst(a) => {
                    let burst = self.burst(a.target, body);
                    a.update(&hands, burst)
                }
                Active::Barrage(b) => {
                    let reach = if b.limit.is_some() {
                        SNARK_BAND[1]
                    } else {
                        BARRAGE_BAND[1]
                    } * 1.2;
                    let at = self
                        .beliefs
                        .track(b.target)
                        .filter(|t| t.state == TrackState::Visible && t.pos.distance(body.origin) <= reach)
                        .map(|t| t.pos);
                    b.update(&hands, at)
                }
                Active::Scope(sc) => {
                    let current = self.mind.target == Some(sc.target);
                    let track = self.beliefs.track(sc.target);
                    let seen = current && track.is_some_and(|t| t.state == TrackState::Visible);
                    // The kill feed takes a dead player's track away. The target is kept through the scope until it
                    // is out of sight for a moment: another one is picked only then.
                    let done = match track {
                        None => Some("the target died"),
                        Some(t) if now.since(t.last_seen) > SCOPE_LOST_HOLD => Some("out of sight"),
                        Some(_) if !current => Some("another target"),
                        Some(t) if t.pos.distance(body.eye) < XBOW_UNZOOM => Some("the target came close"),
                        Some(_) => None,
                    };
                    let on_target = seen
                        && self
                            .mind
                            .last_aim
                            .is_some_and(|aim| on_target(self.motor.view, body.eye, aim));
                    sc.update(&hands, Sight { on_target, seen, done })
                }
                Active::GaussBoost(b) => {
                    let call = self.mind.nav_boost;
                    b.update(&hands, call.is_some(), call.map(|c| c.view))
                }
                Active::Shoot(s) => {
                    if !self.explosives.mines.iter().any(|m| m.pos.distance(s.mine) < 24.0) {
                        Status::Done
                    } else if now >= self.mind.arms.shot_check && !self.shot_spared(body, nav) {
                        Status::Failed("the bot came into the chain's blast")
                    } else {
                        s.update(&hands, self.mind.click_interval.max(spec(s.weapon).cycle))
                    }
                }
            };
            match status {
                Status::Running(r) => {
                    requests.push(r);
                    self.mind.arms.active = Some(active);
                }
                Status::Done => self.finished(&active, body, rng),
                Status::Failed(why) => {
                    self.mind.arms.throw_aim = None;
                    self.mind.arms.trap_throw = false;
                    if matches!(&active, Active::Throw(t) if t.kind == Kind::Grenade) {
                        self.mind.arms.series = false;
                    }
                    if let Active::GaussBoost(b) = &active {
                        // A charge already building is the gauss protocol's now: fired at a target or dumped.
                        if let Some(started) = b.held {
                            self.mind.arms.gauss.adopt(started, now);
                        }
                        self.mind.tricks.stats.boost_failed(why);
                        tracing::info!(
                            "gauss boost given up in its {} phase: {why}{}",
                            b.phase(),
                            if b.held.is_some() {
                                "; the charge is held on"
                            } else {
                                ""
                            }
                        );
                    }
                    if let Active::Airburst(a) = &active {
                        tracing::info!(
                            "satchel in flight not set off: {why}; it came within {:?} of the enemy",
                            a.closest
                        );
                    }
                    tracing::debug!("{} failed: {why}", active.name());
                    self.mind.arms.stats.failure(active.name(), why);
                    self.mind.arms.last_failure = Some(why);
                }
            }
        }
        // The rocket follows the view.
        if self.mind.arms.guide.is_some() {
            match self.rocket_spot(body) {
                Some(point) => self.intents.look(
                    Prio::Protocol,
                    LookIntent::Point {
                        at: point,
                        engaged: true,
                    },
                ),
                None => self.mind.arms.guide = None,
            }
        }
        // Between the grenades of a series the grenade stays in hand.
        if self.mind.arms.active.is_none() && self.mind.arms.series && body.weapon == Some(WeaponId::HandGrenade) {
            self.intents
                .weapon(Prio::Protocol, WeaponIntent::hold(WeaponId::HandGrenade));
        }
        // A satchel thrown from a long jump: the keys of one once it is found to land safely, none till then (the
        // throw is given up when it never does).
        let leaps = matches!(&self.mind.arms.active, Some(Active::Throw(t)) if t.leaps());
        let leap = leaps && requests.iter().any(|r| r.jump) && nav.leap_lands(self.motor.view).is_some();
        for r in requests {
            if let Some(w) = r.weapon {
                self.intents.weapon(Prio::Protocol, w);
            }
            if let Some(l) = r.look {
                self.intents.look(Prio::Protocol, l);
            }
            if let Some(m) = r.movement {
                self.intents.movement(Prio::Protocol, m);
            }
            if r.jump && leap == leaps {
                self.intents.stance(
                    Prio::Protocol,
                    StanceIntent {
                        jump: !leap,
                        duck: false,
                        longjump: leap,
                    },
                );
            }
        }
    }

    /// What a satchel thrown at `target` in flight is watched for: where it is while seen, where the target is, and
    /// whether the bot is out of the blast of its satchels.
    fn burst(&self, target: PlayerKey, body: &Body) -> Burst {
        let now = body.now;
        let satchel = self
            .explosives
            .charges
            .iter()
            .filter(|c| c.seen.is_some_and(|t| now.since(t) <= AIRBURST_SEEN))
            .max_by(|a, b| a.since.0.total_cmp(&b.since.0))
            .map(|c| c.at(now, body.gravity));
        let enemy = self
            .beliefs
            .track(target)
            .filter(|t| t.state == TrackState::Visible || now.since(t.last_seen) <= AIRBURST_SEEN)
            .map(|t| t.pos);
        let spared = Self::spared(body, &self.charges_at(body));
        Burst { satchel, enemy, spared }
    }

    /// How far along the view the first wall is with the gauss in hand, through players (the beam goes through them),
    /// when within a full charge's burst on a wall; and where a missed charged shot would come back at the bot
    /// (vanilla `mp_selfgauss`), the most damage it may carry for that: a wall met square that a beam of so little
    /// cannot punch through. Looked along again every 30 ms at most.
    fn gauss_wall(&mut self, body: &Body, tracer: &mut dyn Tracer) -> f32 {
        const PERIOD: f64 = 0.03;
        if let Some((at, wall, _)) = self.mind.arms.gauss_wall
            && body.now.since(at) < PERIOD
        {
            return wall;
        }
        let reach = lb_combat::arms::gauss::wall_blast(body.damages.gauss_charged);
        let (forward, _, _) = view_angle_vectors(self.motor.view);
        let far = tracer.trace(&TraceQuery::line(body.eye, body.eye + forward * BEAM_REACH));
        let along = far.fraction * BEAM_REACH;
        let wall = if far.fraction < 1.0 && along <= reach {
            along
        } else {
            f32::INFINITY
        };
        let backfire = if body.selfgauss == 1 {
            backfire(tracer, body.eye, forward, &far)
        } else {
            0.0
        };
        self.mind.arms.gauss_wall = Some((body.now, wall, backfire));
        wall
    }

    /// A point to shoot a charged gauss beam at through a wall: the enemy just lost behind a thin wall, the beam
    /// meeting the wall square enough not to glance off, enough of its damage left beyond, and its burst where it
    /// comes out of the wall far enough from the bot; and the damage the beam needs for that, which the charge is let
    /// go with at least. Only for skills that do it (`gauss_walls`), looked for again every 50 ms.
    fn wallbang(&mut self, body: &Body, ch: &Character, tracer: &mut dyn Tracer) -> Option<(Vec3, f32)> {
        let now = body.now;
        if !ch.skill.gauss_walls || body.weapon != Some(WeaponId::Gauss) {
            return None;
        }
        if let Some((at, point)) = self.mind.arms.wallbang
            && now.since(at) < WALLBANG_CHECK
        {
            return point;
        }
        let point = self
            .mind
            .target
            .and_then(|k| self.beliefs.track(k))
            .filter(|t| {
                t.state != TrackState::Visible && now.since(t.last_seen) <= WALLBANG_AGE && t.sigma <= WALLBANG_SIGMA
            })
            .map(|t| t.pos + Vec3::Z * 8.0)
            .and_then(|p| {
                let dir = (p - body.eye).normalize_or_zero();
                let hit = tracer.trace(&TraceQuery::line(body.eye, p));
                if hit.fraction >= 1.0 || hit.start_solid || -hit.normal.dot(dir) < 0.5 {
                    return None;
                }
                // Out of the wall: on from inside it, then back to where the beam comes out.
                let through = tracer.trace(&TraceQuery::line(hit.end + dir * 8.0, p + dir * 32.0));
                if through.all_solid {
                    return None;
                }
                let exit = tracer.trace(&TraceQuery::line(through.end, hit.end)).end;
                let thick = exit.distance(hit.end);
                // What a full charge leaves: the most the burst beyond the wall may carry.
                let left = body.damages.gauss_charged - thick;
                let out = exit.distance(body.eye);
                (thick <= WALLBANG_THICK
                    && left >= WALLBANG_LEFT
                    && out >= lb_combat::arms::gauss::wall_blast(left)
                    && out + 16.0 < p.distance(body.eye))
                .then_some((p, thick + WALLBANG_LEFT))
            });
        self.mind.arms.wallbang = Some((now, point));
        point
    }

    /// Where to dump a gauss charge the bot must let go of with no target: level, along the one of eight ways whose
    /// first wall is farthest (its burst must spare the bot) with no drop behind within the throw of the recoil; or
    /// straight up (the recoil presses the bot to the floor) when the sky or a high ceiling is farther than every
    /// wall around. Looked for four times a second while charging.
    fn safe_dump(&mut self, body: &Body, tracer: &mut dyn Tracer) -> Option<Vec3> {
        const PERIOD: f64 = 0.25;
        // Looked for only once a dump draws near: some eighty traces each time.
        const SOON: f32 = 5.5;
        let soon = self.mind.arms.gauss.charge(body.now) >= SOON || body.waterlevel >= 2 || body.on_ladder;
        if !self.mind.arms.gauss.active() || !soon {
            return None;
        }
        if let Some((at, way)) = self.mind.arms.dump_way
            && body.now.since(at) < PERIOD
        {
            return way;
        }
        let full = body.damages.gauss_charged;
        let reach = lb_combat::arms::gauss::wall_blast(full);
        let throw = lb_combat::arms::gauss::recoil_throw(full, Vec3::X, body.gravity);
        let (forward, _, _) = view_angle_vectors(self.motor.view);
        let back = -forward.truncate().normalize_or(Vec2::X);
        let way = (0..8)
            .map(|k| {
                let yaw = k as f32 * 45.0;
                let (s, c) = lb_core::dmath::sin_cos(yaw.to_radians());
                let dir = Vec2::new(c, s);
                let far = tracer.trace(&TraceQuery::line(body.eye, body.eye + dir.extend(0.0) * BEAM_REACH));
                let wall = (far.fraction * BEAM_REACH).min(reach);
                let backfires = body.selfgauss == 1 && backfire(tracer, body.eye, dir.extend(0.0), &far) >= full;
                let room = if backfires {
                    0.0
                } else {
                    recoil_room(tracer, body.origin, -dir)
                };
                (yaw, dir, wall, room)
            })
            .filter(|w| w.3 >= throw)
            .max_by(|a, b| {
                // The farthest wall; among the clear ones the one nearest to back the way the bot looks from.
                let clear = |w: f32| w.min(reach);
                clear(a.2)
                    .total_cmp(&clear(b.2))
                    .then(a.1.dot(back).total_cmp(&b.1.dot(back)))
            })
            .map(|(yaw, _, wall, _)| (Vec3::new(0.0, lb_core::math::normalize_angle(yaw), 0.0), wall));
        let way = match way {
            Some((_, wall)) if wall >= reach => way.map(|w| w.0),
            _ => {
                let up = tracer.trace(&TraceQuery::line(body.eye, body.eye + Vec3::Z * BEAM_REACH));
                let high = (up.fraction * BEAM_REACH).min(reach);
                let backfires = body.selfgauss == 1 && backfire(tracer, body.eye, Vec3::Z, &up) >= full;
                // In the air the push down would slam the bot into the floor.
                if body.on_ground && !backfires && way.is_none_or(|(_, wall)| high > wall) {
                    Some(Vec3::new(-89.0, self.motor.view.y, 0.0))
                } else {
                    way.map(|w| w.0)
                }
            }
        };
        self.mind.arms.dump_way = Some((body.now, way));
        way
    }

    fn finished(&mut self, active: &Active, body: &Body, rng: &mut BotRng) {
        let now = body.now;
        let aim = self.mind.arms.throw_aim.take();
        let enemy = aim
            .and_then(|k| self.beliefs.track(k))
            .map(|e| (e.pos, e.vel, e.state == TrackState::Visible, e.who.userid));
        let stats = &mut self.mind.arms.stats;
        match active {
            Active::Throw(t) => match t.kind {
                Kind::Grenade => {
                    stats.grenades += 1;
                    self.mind.arms.grenade_launch = t.launch().map(|(pitch, velocity)| (now, pitch, velocity));
                    if let (Some(p), Some(goes_off)) = (t.plan(), t.goes_off()) {
                        let (pos, vel, seen, userid) = enemy.unwrap_or((p.target, Vec3::ZERO, false, 0));
                        tracing::info!(
                            "grenade thrown {}{}: view {:.1} {:.1}, the pin out {:.2} s, to burst in {:.1} s at {:.0} {:.0} {:.0}, the enemy expected {:.0} off it at {:.0} {:.0} {:.0}, {:.0} damage expected; the enemy #{userid} at {:.0} {:.0} {:.0} vel {:.0} {:.0}{}",
                            t.way(),
                            t.forced().map_or(String::new(), |f| format!(", {f}")),
                            p.throw.pitch,
                            p.throw.yaw,
                            p.release_held,
                            goes_off.since(now),
                            p.burst.x,
                            p.burst.y,
                            p.burst.z,
                            p.burst.distance(p.target),
                            p.target.x,
                            p.target.y,
                            p.target.z,
                            p.damage,
                            pos.x,
                            pos.y,
                            pos.z,
                            vel.x,
                            vel.y,
                            if seen { ", in sight" } else { "" }
                        );
                    }
                    // Players back off at once after a grenade.
                    if t.backs_off() {
                        let away = (body.origin - t.target).truncate().normalize_or_zero();
                        if away != Vec2::ZERO {
                            self.mind.arms.dodge = Some((now + GRENADE_BACK_OFF, away));
                        }
                    }
                    // Its blast is kept away from until it goes off, seen or not, and the target is not closed in on.
                    if let Some(goes_off) = t.goes_off() {
                        self.explosives
                            .thrown_grenade(t.landing(body.gravity), goes_off, now, body.gravity);
                        self.mind.arms.hold_until = self.mind.arms.hold_until.max(goes_off);
                    }
                    if self.mind.arms.series {
                        stats.series += 1;
                        // The next of the series as soon as the game hands over another grenade.
                        self.mind.arms.next_throw = now + SERIES_GAP;
                    }
                }
                Kind::Satchel => {
                    stats.satchels += t.landings.len() as u32;
                    let mut plan = SatchelPlan::roll(now, rng);
                    if std::mem::take(&mut self.mind.arms.trap_throw) {
                        plan.lie_until = now + f64::from(rng.combat.range_f32(TRAP_LIE[0], TRAP_LIE[1]));
                    }
                    self.mind.arms.satchels = Some(plan);
                    // Closing in would take the bot into its own blast.
                    self.mind.arms.hold_until = self.mind.arms.hold_until.max(now + SATCHEL_HOLD);
                    match aim {
                        Some(target) => self.mind.arms.active = Some(Active::Airburst(Airburst::new(target, now))),
                        None => self.back_off_satchels(&t.landings, body),
                    }
                }
                Kind::Snark => {
                    stats.snarks += 1;
                    self.mind.arms.hold_until = self.mind.arms.hold_until.max(now + SNARK_HOLD);
                }
            },
            Active::Mine(p) => {
                stats.mines += 1;
                if !p.noted {
                    self.explosives
                        .placed_mine(p.mine(), p.normal, p.pressed_at().unwrap_or(now));
                }
                // Out of the beam's way before it arms (see `dodge`).
                self.mind.arms.step_off = Some(p.normal);
            }
            Active::Drop(p) => {
                stats.mines += 1;
                stats.dropped += 1;
                if let Some((pos, dir)) = p.mine() {
                    self.trail_dropped(pos, dir, p.pressed_at().unwrap_or(now));
                }
            }
            Active::Lob(l) => {
                stats.lobs += 1;
                let landing = now + f64::from(l.throw.flight);
                self.mind.arms.hold_until = self.mind.arms.hold_until.max(landing);
            }
            Active::Detonate(d) => {
                tracing::info!(
                    "satchels set off: {}; {} of them, the nearest {:.0} units off, {:.0} damage to the bot expected",
                    self.mind.arms.detonate_why,
                    self.explosives.charges.len(),
                    self.charges_at(body)
                        .iter()
                        .map(|c| c.distance(body.origin))
                        .fold(f32::INFINITY, f32::min),
                    satchel_damage(&self.charges_at(body), body.origin, body.damages.satchel)
                );
                let stats = &mut self.mind.arms.stats;
                stats.satchel_off(self.mind.arms.detonate_why);
                self.explosives.detonated();
                self.mind.arms.satchels = None;
                self.mind.arms.satchel_fact = d.button.or(self.mind.arms.satchel_fact);
            }
            Active::Airburst(a) => {
                if a.burst() {
                    stats.satchel_off("in flight by the enemy");
                    self.explosives.detonated();
                    self.mind.arms.satchels = None;
                    self.mind.arms.satchel_fact = a.button.or(self.mind.arms.satchel_fact);
                }
            }
            Active::Barrage(b) => {
                stats.snarks += b.thrown;
                // Away from the swarm of a barrage before it turns: a snark bites its owner too.
                if b.limit.is_none() {
                    stats.barrages += 1;
                    if let Some(t) = self.beliefs.track(b.target) {
                        let away = (body.origin - t.pos).truncate().normalize_or(Vec2::X);
                        self.mind.arms.dodge = Some((now + BARRAGE_RUN, away));
                    }
                }
            }
            Active::Shoot(_) => {
                let why = self.mind.arms.shot_why;
                self.mind.arms.stats.shot(why);
                self.chain_set_off(body);
            }
            Active::GaussBoost(_) => self.mind.tricks.stats.boosts_fired += 1,
            Active::Scope(sc) => {
                stats.scoped += sc.shots;
                stats.zooms += 1;
                let why = sc.ended.unwrap_or("-");
                match stats.scope_ends.iter_mut().find(|(w, _)| *w == why) {
                    Some(e) => e.1 += 1,
                    None => stats.scope_ends.push((why, 1)),
                }
            }
        }
    }

    /// After a pile of satchels, out of their blast: away from where they land for as long as it takes.
    fn back_off_satchels(&mut self, landings: &[Vec3], body: &Body) {
        let Some(nearest) = landings
            .iter()
            .copied()
            .min_by(|a, b| a.distance(body.origin).total_cmp(&b.distance(body.origin)))
        else {
            return;
        };
        let short = blast_radius(body.damages.satchel) + SATCHEL_SPARED - nearest.distance(body.origin);
        if short > 0.0 {
            let away = (body.origin - nearest).truncate().normalize_or(Vec2::X);
            let secs = (short / body.maxspeed.max(1.0) + 0.2).min(PILE_BACK_OFF);
            self.mind.arms.dodge = Some((body.now + f64::from(secs), away));
        }
    }

    /// Every frame, a snark near (someone else's within 300 units, the bot's own coming back at it within 150: a
    /// snark bites its owner too). With the egon in hand the bot burns it, when no player in sight is closer than 300
    /// units; with anything else it runs from it, which works far better than shooting at a small, hopping snark.
    /// yapb shot at its own snarks and hornets whatever they did.
    pub(crate) fn snark_defense(&mut self, body: &Body, nav: &mut dyn NavService) {
        let now = body.now;
        let snark = self
            .explosives
            .flying
            .iter()
            .filter(|f| f.kind == lb_game::entities::ProjectileKind::Snark && now.since(f.seen) <= 0.3)
            .filter(|f| {
                let d = f.pos.distance(body.origin);
                let coming = f.vel.dot(body.origin - f.pos) > 0.0;
                if f.own {
                    d <= OWN_SNARK_NEAR && coming
                } else {
                    d <= SNARK_NEAR
                }
            })
            .min_by(|a, b| a.pos.distance(body.origin).total_cmp(&b.pos.distance(body.origin)))
            .copied();
        let Some(s) = snark else { return };
        let player = self
            .mind
            .target
            .and_then(|k| self.beliefs.track(k))
            .filter(|t| t.state == TrackState::Visible)
            .map(|t| t.pos.distance(body.origin));
        let egon = body.weapon == Some(WeaponId::Egon)
            && body.allows(WeaponId::Egon)
            && body.armed(WeaponId::Egon).is_some_and(|a| a.loaded());
        if egon && !self.mind.arms.busy() && player.is_none_or(|d| d >= SNARK_OVER_PLAYER) {
            // Over a player in sight, the snark must win the view and the trigger.
            let prio = if player.is_some() { Prio::Protocol } else { Prio::Threat };
            let (forward, _, _) = view_angle_vectors(self.motor.view);
            let on_it = forward.dot((s.pos - body.eye).normalize_or_zero()) > 0.97;
            self.intents.look(
                prio,
                LookIntent::Point {
                    at: s.pos,
                    engaged: true,
                },
            );
            let mut fire = WeaponIntent::hold(WeaponId::Egon);
            if on_it {
                fire.fire = lb_motor::Fire::Primary;
            }
            self.intents.weapon(prio, fire);
            return;
        }
        // Run from it, sideways when straight away is a drop. The run takes the legs only: the bot keeps fighting.
        let away = (body.origin - s.pos).truncate().normalize_or(Vec2::X);
        let options = [away, Vec2::new(-away.y, away.x), Vec2::new(away.y, -away.x)];
        if let Some(dir) = options
            .into_iter()
            .find(|d| !drops(nav, body.origin, *d * body.maxspeed))
        {
            if self.mind.arms.dodge.is_none_or(|(until, _)| now >= until) {
                self.mind.arms.stats.snark_runs += 1;
            }
            self.mind.arms.dodge = Some((now + SNARK_RUN_FOR, dir));
        }
    }

    /// Where the bot's rocket in flight is guided to, and so goes off: the target it was fired at while that stays far
    /// enough (a new target close by, or this one come close, would bring the rocket back to the bot), else the point
    /// it was fired at.
    fn rocket_spot(&self, body: &Body) -> Option<Vec3> {
        let (until, at, target) = self.mind.arms.guide?;
        if body.now >= until {
            return None;
        }
        let near = crate::mind::rocket_min(body);
        Some(
            self.mind
                .last_aim
                .filter(|p| self.mind.target == Some(target) && p.distance(body.eye) >= near)
                .unwrap_or(at),
        )
    }

    /// Every frame, after everything else asked to move: a move toward the beam of a tripmine the bot knows of (armed,
    /// or about to be) gets past it the way a player does: ducking under a high one; round any other by the way nearest
    /// the one asked for that keeps the bot's box off it (where
    /// the beam runs at body height: the foot of one standing up from the floor, a stretch of one slanting up a ramp),
    /// on the floor and clear of walls. Near a beam the bot goes at a careful pace, and the way is reckoned from where
    /// its run carries it first; where no way does, it brakes hard (pushing against its run, as a player does). A beam
    /// standing up with no way round is told to navigation, which keeps paths off the others it cannot get past.
    /// Strafing and dodging, which do not follow paths, are covered too, and so is a bot sliding to a stop.
    pub(crate) fn beam_guard(&mut self, body: &Body, nav: &mut dyn NavService) {
        let now = body.now;
        let Some((prio, mv)) = self.intents.movement else {
            return;
        };
        if prio >= Prio::Traversal {
            return;
        }
        let velocity = body.velocity.truncate();
        let run = velocity.length();
        let asked = mv.speed > 0.0 && mv.dir != Vec2::ZERO;
        if !asked && run < SLIDE {
            return;
        }
        // Standing still, the way it slides.
        let dir = if asked { mv.dir.normalize() } else { velocity / run };
        let half = if body.ducked { 18.0 } else { 36.0 };
        let (mut duck, mut brake, mut careful) = (false, false, false);
        let mut walls: smallvec::SmallVec<[(usize, Vec2, Vec2); 8]> = smallvec::SmallVec::new();
        for (i, m) in self.explosives.mines.iter().enumerate() {
            let Some(end) = beam_of(m).filter(|_| now.since(m.armed_at) > -OWN_BEAM_SOON) else {
                continue;
            };
            let inside = near_beam(body.origin, m.pos, end);
            match pass_of(m) {
                BeamPass::Around | BeamPass::Blocked => {
                    if let Some((p, q)) = beam_slice(body.origin, half, m.pos, end) {
                        walls.push((i, p, q));
                    }
                }
                BeamPass::Under => {
                    let at = crossing(body.origin, dir, DUCK_FROM, m.pos, end);
                    careful |= at.is_some();
                    if inside || at.is_some() {
                        duck = true;
                    }
                    // Ducking takes a moment: not into the beam before the bot is down.
                    let room = stopping(run) + DUCKED_NEAR;
                    if !body.ducked && (inside || at.is_some_and(|t| t <= room)) {
                        brake = true;
                    }
                }
            }
        }
        let mut steer = None;
        if !walls.is_empty() {
            let here = body.origin.truncate();
            // Where its run carries the bot before a turn takes, then along a way: as far as the move goes before it
            // is looked at again, or as far as the bot would slide.
            let drift = velocity * DRIFT;
            let reach = (mv.speed * BEAM_LOOKAHEAD).max(stopping(run)) + BEAM_STEP;
            let steps = (reach / BEAM_STEP).ceil() as u32;
            let path = |d: Vec2, k: u32| here + drift + d * (k as f32 * BEAM_STEP);
            let gaps: smallvec::SmallVec<[f32; 8]> = walls
                .iter()
                .map(|&(_, p, q)| box_gap(here, p, q).min(box_gap(here + drift, p, q)))
                .collect();
            // Carried clear of every beam, then each step out of reach of every beam, or no nearer one already too
            // near than now.
            let carried = gaps.iter().all(|&g| g >= BEAM_TOUCH);
            let fits = |d: Vec2| {
                carried
                    && (1..=steps).all(|k| {
                        let at = path(d, k);
                        walls.iter().zip(&gaps).all(|(&(_, p, q), &now_gap)| {
                            let g = box_gap(at, p, q);
                            g >= BEAM_KEEP || g >= now_gap - 0.5
                        })
                    })
            };
            let near = |d: Vec2| {
                let steps = (GOVERN_REACH / BEAM_STEP) as u32;
                (0..=steps).any(|k| walls.iter().any(|&(_, p, q)| box_gap(path(d, k), p, q) < GOVERN_GAP))
            };
            if !fits(dir) {
                let (nearest, _) = walls
                    .iter()
                    .zip(&gaps)
                    .min_by(|a, b| a.1.total_cmp(b.1))
                    .map(|(w, g)| (*w, *g))
                    .unwrap_or((walls[0], 0.0));
                // Away from the side the nearest beam is on, or the side taken last.
                let toward = nearest_on(here, nearest.1, nearest.2) - here;
                let away = if toward.perp_dot(dir) > 0.0 { 1.0 } else { -1.0 };
                let first = self
                    .mind
                    .arms
                    .beam_side
                    .filter(|(until, _)| now < *until)
                    .map_or(away, |(_, s)| s);
                let room = |d: Vec2, nav: &mut dyn NavService| {
                    let along = (d * reach.min(ROOM_AHEAD)).extend(0.0);
                    let side = Vec3::new(-d.y, d.x, 0.0) * ROOM_SIDE;
                    [-1.0, 0.0, 1.0].into_iter().all(|k| {
                        let from = body.origin + Vec3::Z * ROOM_HEIGHT + side * k;
                        clear_line(nav, from, from + along)
                    }) && !drops(nav, body.origin, d * mv.speed.max(SLIDE))
                };
                let way = if asked {
                    (1..=BEAM_TURNS)
                        .flat_map(|k| [(first, k), (-first, k)])
                        .map(|(side, k)| {
                            let (sin, cos) = dmath::sin_cos((side * BEAM_TURN * k as f32).to_radians());
                            (side, Vec2::new(dir.x * cos - dir.y * sin, dir.x * sin + dir.y * cos))
                        })
                        .find(|&(_, d)| fits(d) && room(d, nav))
                } else {
                    None
                };
                match way {
                    Some((side, d)) => {
                        steer = Some(d);
                        self.mind.arms.beam_side = Some((now + BEAM_SIDE_FOR, side));
                    }
                    None => {
                        brake = true;
                        let m = &mut self.explosives.mines[nearest.0];
                        if asked && pass_of(m) == BeamPass::Around {
                            m.pass = Some(BeamPass::Blocked);
                            m.avoided_at = None;
                        }
                    }
                }
            }
            careful |= asked && near(steer.unwrap_or(dir));
        }
        if duck {
            self.intents.stance(
                Prio::Protocol,
                StanceIntent {
                    jump: false,
                    duck,
                    longjump: false,
                },
            );
        }
        let way = if brake {
            Some(if run > SLIDE {
                MoveIntent {
                    dir: -velocity / run,
                    speed: body.maxspeed,
                }
            } else {
                lb_combat::arms::stop()
            })
        } else if asked && (steer.is_some() || careful) {
            Some(MoveIntent {
                dir: steer.unwrap_or(dir),
                speed: if careful { mv.speed.min(CAREFUL) } else { mv.speed },
            })
        } else {
            None
        };
        if let Some(m) = way {
            self.intents.movement(Prio::Protocol, m);
        }
    }

    /// Every frame, after everything else asked to move: a move that would take the bot deeper into the reach of a
    /// grenade about to go off (its own ones above all: it follows them to where it expects the enemy) or of where
    /// its rocket in flight is going keeps only its part along the edge, whatever the goal (a bot on its way to an
    /// item fires at an enemy on the way and walks on into the blast). The dodge runs from a blast the bot finds
    /// itself in; this keeps it from walking in.
    pub(crate) fn blast_guard(&mut self, body: &Body) {
        let Some((prio, mv)) = self.intents.movement else {
            return;
        };
        if prio >= Prio::Traversal || mv.speed <= 0.0 || mv.dir == Vec2::ZERO {
            return;
        }
        let floor = body.origin.z - 36.0;
        let asked = mv.dir.normalize() * mv.speed;
        let mut velocity = asked;
        let rocket = self
            .rocket_spot(body)
            .map(|at| (at, blast_radius(body.damages.primary(WeaponId::Rpg))));
        let grenades = self
            .explosives
            .blasts(body.now, body.gravity, floor)
            .filter(|b| b.kind == lb_game::entities::ProjectileKind::Grenade)
            .map(|b| (b.at, b.radius));
        let chain: smallvec::SmallVec<[(Vec3, f32); 16]> = self.chain_blasts(body).map(|b| (b.at, b.radius)).collect();
        for (at, radius) in grenades.chain(rocket).chain(chain) {
            let off = (body.origin - at).truncate();
            if (off + velocity * BLAST_LOOKAHEAD).length() >= radius + DODGE_MARGIN {
                continue;
            }
            let out = off.normalize_or_zero();
            velocity -= out * velocity.dot(out).min(0.0);
        }
        if velocity != asked {
            self.intents.movement(
                Prio::Protocol,
                MoveIntent {
                    dir: velocity,
                    speed: velocity.length(),
                },
            );
        }
    }

    /// Every frame: run from a blast about to go off near the bot (a skilled one with the module long jumps away),
    /// and out of the beam of a mine it just laid, or of one of its own about to arm (whatever a protocol would have
    /// the bot do: armed with the bot in it, the mine goes off as it moves).
    pub(crate) fn dodge(&mut self, body: &Body, ch: &Character, nav: &mut dyn NavService, rng: &mut BotRng) {
        let now = body.now;
        let arming = self.explosives.mines.iter().find_map(|m| {
            let end = beam_of(m)?;
            (m.own && now < m.armed_at && now.since(m.armed_at) > -OWN_BEAM_SOON && near_beam(body.origin, m.pos, end))
                .then_some((m.pos, end))
        });
        if let Some((a, b)) = arming {
            let s = b - a;
            let on = a + s * ((body.origin - a).dot(s) / s.length_squared().max(1e-6)).clamp(0.0, 1.0);
            let (forward, _, _) = view_angle_vectors(self.motor.view);
            let away = (body.origin - on)
                .truncate()
                .try_normalize()
                .unwrap_or_else(|| forward.truncate().normalize_or(Vec2::X));
            if let Some(dir) = [away, Vec2::new(-away.y, away.x), Vec2::new(away.y, -away.x)]
                .into_iter()
                .find(|d| !drops(nav, body.origin, *d * body.maxspeed))
            {
                self.intents.movement(
                    Prio::Protocol,
                    MoveIntent {
                        dir,
                        speed: body.maxspeed,
                    },
                );
            }
        }
        if let Some(normal) = self.mind.arms.step_off.take() {
            // Along the wall, the way the bot faces, or back the other way from a drop.
            let (forward, _, _) = view_angle_vectors(self.motor.view);
            let along = Vec2::new(-normal.y, normal.x).normalize_or(Vec2::X);
            let dir = if along.dot(forward.truncate()) >= 0.0 {
                along
            } else {
                -along
            };
            if let Some(safe) = [dir, -dir]
                .into_iter()
                .find(|d| !drops(nav, body.origin, *d * body.maxspeed))
            {
                self.mind.arms.dodge = Some((now + MINE_STEP_AWAY, safe));
            }
        }
        let floor = body.origin.z - 36.0;
        let chain: smallvec::SmallVec<[lb_knowledge::Blast; 16]> = self.chain_blasts(body).collect();
        let threat = self
            .explosives
            .blasts(now, body.gravity, floor)
            .chain(self.explosives.rocket_at(body.eye))
            .chain(chain)
            .filter(|b| b.at.distance(body.origin) < b.radius + DODGE_MARGIN)
            .min_by(|a, b| a.at.distance(body.origin).total_cmp(&b.at.distance(body.origin)));
        if let Some(b) = threat {
            let away = (body.origin - b.at).truncate();
            let dir = if away.length() > 16.0 {
                away.normalize()
            } else {
                let (_, right, _) = view_angle_vectors(self.motor.view);
                right.truncate().normalize_or(Vec2::Y)
            };
            let options = [dir, Vec2::new(-dir.y, dir.x), Vec2::new(dir.y, -dir.x)];
            if let Some(safe) = options
                .into_iter()
                .find(|d| !drops(nav, body.origin, *d * body.maxspeed))
            {
                if self.mind.arms.dodge.is_none_or(|(until, _)| now >= until) {
                    self.mind.arms.stats.dodges += 1;
                    let (at, off) = (b.at, body.origin.distance(b.at));
                    let away = move |landing: Vec3| landing.distance(at) > off + LEAP_AWAY;
                    self.dodge_leap(body, ch, &[safe], &away, nav, rng);
                }
                self.mind.arms.dodge = Some((now + DODGE_FOR, safe));
            }
        }
        match self.mind.arms.dodge {
            Some((until, dir)) if now < until => self.intents.movement(
                Prio::Threat,
                MoveIntent {
                    dir,
                    speed: body.maxspeed,
                },
            ),
            Some(_) => self.mind.arms.dodge = None,
            None => {}
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use lb_combat::Armed;
    use lb_config::skill::Presets;
    use lb_core::rng::Pcg32;
    use lb_knowledge::{BeliefParams, PlayerKey, Relation, RenderCue, Sighting, Stance, parts};
    use lb_nav_api::{NavStatus, NavStep};
    use lb_worldq::{Trace, contents};

    /// Open floor at z = -36 everywhere.
    pub(crate) struct Open;

    impl Tracer for Open {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            if q.end.z < -36.0 && q.start.z >= -36.0 {
                let f = (q.start.z + 36.0) / (q.start.z - q.end.z);
                let mut t = Trace::clear(q.start + (q.end - q.start) * f);
                t.fraction = f;
                t.normal = Vec3::Z;
                return t;
            }
            Trace::clear(q.end)
        }

        fn point_contents(&mut self, _p: Vec3) -> i32 {
            contents::EMPTY
        }
    }

    impl NavService for Open {
        fn go_to(&mut self, dest: Vec3) -> (NavStatus, Option<NavStep>) {
            (NavStatus::Moving, Some(NavStep::hold(dest)))
        }
        fn roam(&mut self, _rng: &mut Pcg32) -> Option<NavStep> {
            None
        }
        fn away_from(&mut self, _threat: Vec3) -> Option<Vec3> {
            None
        }
        fn available(&self) -> bool {
            true
        }
    }

    pub(crate) fn character() -> Character {
        Character {
            skill: Presets::default().at(100),
            level: 100,
            aggression: 0.8,
            fear: 0.2,
            affinity: lb_styles::StyleId::Balanced.goal_affinity(),
            weapons: crate::WeaponLike::default(),
            tricks: lb_styles::StyleId::Balanced.trick_likes(),
        }
    }

    pub(crate) fn body(now: f64) -> Body {
        Body {
            now: SimTime(now),
            dt: 0.01,
            origin: Vec3::ZERO,
            eye: Vec3::new(0.0, 0.0, 28.0),
            velocity: Vec3::ZERO,
            maxspeed: 300.0,
            health: 100.0,
            armor: 0.0,
            has_longjump: false,
            on_ground: true,
            ducked: false,
            on_ladder: false,
            underwater: false,
            waterlevel: 0,
            fov: 0.0,
            weapon: Some(WeaponId::Glock),
            arsenal: [
                Armed::new(WeaponId::Glock, Some(17), Some(50)),
                Armed::new(WeaponId::HandGrenade, None, Some(3)),
            ]
            .into_iter()
            .collect(),
            prediction: None,
            ammo_need: [0.0; 7],
            opponents: 2,
            damages: lb_game::mechanics::Damages::default(),
            dll: lb_game::dll::DllProfile::default(),
            gravity: 800.0,
            allowed: u32::MAX,
            gungame: None,
            selfgauss: 0,
            tricks: lb_config::main_config::TricksConfig::default(),
        }
    }

    pub(crate) fn seen(t: f64, pos: Vec3) -> Sighting {
        Sighting {
            who: PlayerKey { slot: 5, userid: 50 },
            relation: Relation::Enemy,
            t: SimTime(t),
            pos,
            sigma: 1.0,
            distance: pos.length(),
            visibility: 1.0,
            parts: parts::CHEST,
            stance: Stance::Standing,
            on_ground: true,
            on_ladder: false,
            in_water: false,
            facing: 180.0,
            weapon: None,
            firing: false,
            render: RenderCue::default(),
            first: true,
            noticed_at: SimTime(t),
        }
    }

    fn considered(brain: &mut BotBrain, now: f64, extra: &[Armed]) -> Option<&'static str> {
        let mut rng = BotRng::new(7, 7);
        for i in 0..20 {
            let t = now + f64::from(i) * 0.1;
            brain.update(
                SimTime(t),
                &BeliefParams {
                    track_forget: 12.0,
                    maxspeed: 300.0,
                },
                None,
                None,
            );
            let mut b = body(t);
            b.arsenal.extend(extra.iter().copied());
            brain.weapon_options(&b, &character(), &mut Open, &mut rng);
            if let Some(a) = &brain.mind.arms.active {
                return Some(a.name());
            }
        }
        None
    }

    pub(crate) fn params() -> BeliefParams {
        BeliefParams {
            track_forget: 12.0,
            maxspeed: 300.0,
        }
    }

    /// The satchel radio reports a charge out.
    fn radio(b: &mut Body, carried: i32) {
        use lb_game::self_state::{PredictedWeapon, Prediction};
        b.arsenal.push(Armed::new(WeaponId::Satchel, None, Some(carried)));
        let mut p = Prediction::default();
        p.weapons[WeaponId::Satchel as usize] = Some(PredictedWeapon {
            charge_ready: 1,
            ..PredictedWeapon::default()
        });
        b.prediction = Some(p);
    }

    /// Runs the weapon options every tenth of a second from `from` until a protocol starts or `until`.
    fn first_protocol(
        brain: &mut BotBrain,
        from: f64,
        until: f64,
        enemy: Option<(Vec3, f64)>,
        dress: &dyn Fn(&mut Body),
    ) -> Option<(f64, &'static str)> {
        let mut rng = BotRng::new(7, 7);
        let mut t = from;
        while t < until {
            if let Some((pos, seen_until)) = enemy
                && t <= seen_until
            {
                brain.beliefs.on_sighting(&seen(t, pos));
            }
            brain.update(SimTime(t), &params(), None, None);
            let mut b = body(t);
            dress(&mut b);
            brain.weapon_options(&b, &character(), &mut Open, &mut rng);
            if let Some(a) = &brain.mind.arms.active {
                return Some((t, a.name()));
            }
            t += 0.1;
        }
        None
    }

    /// A slab of solid between x = `from` and x = `to`, open all around; traced along x only.
    struct Slab {
        from: f32,
        to: f32,
    }

    impl Tracer for Slab {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            let inside = |x: f32| x > self.from && x < self.to;
            let (a, b) = (q.start.x, q.end.x);
            let len = (b - a).abs().max(1e-6);
            if inside(a) && inside(b) {
                let mut t = Trace::clear(q.start);
                t.all_solid = true;
                t.start_solid = true;
                t.fraction = 0.0;
                return t;
            }
            if inside(a) {
                let mut t = Trace::clear(q.end);
                t.start_solid = true;
                return t;
            }
            let face = if b > a { self.from } else { self.to };
            if (a - face) * (b - face) < 0.0 {
                let f = (face - a).abs() / len;
                let mut t = Trace::clear(q.start + (q.end - q.start) * f);
                t.fraction = f;
                t.normal = Vec3::new(if b > a { -1.0 } else { 1.0 }, 0.0, 0.0);
                return t;
            }
            Trace::clear(q.end)
        }

        fn point_contents(&mut self, _p: Vec3) -> i32 {
            contents::EMPTY
        }
    }

    #[test]
    fn a_missed_charge_comes_back_only_from_a_wall_too_thick_to_punch() {
        let eye = Vec3::ZERO;
        let hit_along = |slab: &mut Slab, dir: Vec3| {
            let far = slab.trace(&TraceQuery::line(eye, eye + dir * BEAM_REACH));
            backfire(slab, eye, dir, &far)
        };
        let mut thick = Slab { from: 500.0, to: 800.0 };
        assert!(
            (hit_along(&mut thick, Vec3::X) - 300.0).abs() < 1.0,
            "a 300-unit wall stops any charge"
        );
        let mut thin = Slab { from: 500.0, to: 516.0 };
        assert!(
            (hit_along(&mut thin, Vec3::X) - 16.0).abs() < 1.0,
            "a thin one only a charge under 16 damage"
        );
        let mut glancing = Slab { from: 500.0, to: 800.0 };
        let far = Trace {
            normal: Vec3::new(-0.3, 0.95, 0.0),
            fraction: 0.1,
            ..Trace::clear(Vec3::new(500.0, 0.0, 0.0))
        };
        assert_eq!(
            backfire(&mut glancing, eye, Vec3::X, &far),
            0.0,
            "a glancing beam reflects instead"
        );
    }

    #[test]
    fn satchels_lying_a_while_go_off_with_nobody_in_sight() {
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain
            .explosives
            .thrown_satchel(Vec3::new(600.0, 0.0, -36.0), SimTime(0.0));
        let (t, what) = first_protocol(&mut brain, 0.0, 20.0, None, &|b| radio(b, 1)).expect("set off");
        assert_eq!(what, "detonate");
        assert!((8.0..=15.1).contains(&t), "after lying 8–15 s: {t}");
        assert_eq!(brain.mind.arms.detonate_why, "lying long enough");
        // Lying next to the bot: it backs off first.
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain
            .explosives
            .thrown_satchel(Vec3::new(100.0, 0.0, -36.0), SimTime(0.0));
        assert_eq!(first_protocol(&mut brain, 0.0, 20.0, None, &|b| radio(b, 1)), None);
        let (_, away) = brain.mind.arms.dodge.expect("backing off");
        assert!(away.x < -0.9, "away from the charge: {away:?}");
    }

    #[test]
    fn satchels_go_off_once_an_enemy_in_their_blast_is_out_of_sight() {
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain
            .explosives
            .thrown_satchel(Vec3::new(560.0, 60.0, -36.0), SimTime(0.0));
        let mut rng = BotRng::new(3, 3);
        let plan = SatchelPlan::roll(SimTime(0.0), &mut rng);
        brain.mind.arms.satchels = Some(plan);
        // In sight 270 units from the charge until t = 1: in the blast, but too far out for it. No grenades to throw
        // at it meanwhile.
        let far = Vec3::new(500.0, 320.0, 0.0);
        let satchels_only = |b: &mut Body| {
            b.arsenal.retain(|a| a.id != WeaponId::HandGrenade);
            radio(b, 1);
        };
        let (t, what) = first_protocol(&mut brain, 0.0, 6.0, Some((far, 1.0)), &satchels_only).expect("set off");
        assert_eq!(what, "detonate");
        assert!(
            t >= 1.0 + plan.lost_wait - 0.11 && t <= 1.0 + plan.lost_wait + 0.25,
            "{t} {}",
            plan.lost_wait
        );
        assert_eq!(brain.mind.arms.detonate_why, "an enemy seen by them a moment ago");
        assert_eq!(brain.mind.arms.detonate_wait, None, "pressed at once");
        // Walking up to them in sight: the radio comes up at once, and presses while the enemy is in the blast.
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain
            .explosives
            .thrown_satchel(Vec3::new(560.0, 60.0, -36.0), SimTime(0.0));
        let enemy = Vec3::new(500.0, 0.0, 0.0);
        let (t, _) = first_protocol(&mut brain, 0.0, 6.0, Some((enemy, 6.0)), &satchels_only).expect("set off");
        assert!((1.0..1.3).contains(&t), "once the satchel lies there: {t}");
        assert_eq!(brain.mind.arms.detonate_why, "an enemy in their blast");
        assert!(brain.mind.arms.detonate_wait.is_some());
        let mut b = body(t);
        satchels_only(&mut b);
        assert_eq!(brain.detonate_go(&b), Ok(true));
    }

    #[test]
    fn two_satchels_reach_further_than_one() {
        let one = [Vec3::new(300.0, 0.0, -36.0)];
        let two = [Vec3::new(300.0, 0.0, -36.0), Vec3::new(320.0, 30.0, -36.0)];
        let at = Vec3::new(300.0, 230.0, 0.0);
        assert!(satchel_damage(&one, at, 120.0) < SATCHEL_WORTH);
        assert!(satchel_damage(&two, at, 120.0) >= SATCHEL_WORTH);
        assert_eq!(satchel_damage(&one, Vec3::new(700.0, 0.0, 0.0), 120.0), 0.0);
    }

    #[test]
    fn a_step_heard_by_the_satchels_sets_them_off() {
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain
            .explosives
            .thrown_satchel(Vec3::new(560.0, 60.0, -36.0), SimTime(0.0));
        let step = |t: f64| lb_knowledge::Hypothesis {
            id: 0,
            kind: HypothesisKind::Sound(SoundKind::Step),
            t: SimTime(t),
            pos: Some(Vec3::new(600.0, 150.0, -36.0)),
            bearing: 15.0,
            bearing_sigma: 10.0,
            weapon: None,
            strength: 0.3,
            track: None,
        };
        let heard_at_2 = |b: &mut Body| {
            b.arsenal.retain(|a| a.id != WeaponId::HandGrenade);
            radio(b, 1);
        };
        let mut rng = BotRng::new(7, 7);
        for i in 0..40 {
            let t = f64::from(i) * 0.1;
            if i == 20 {
                brain.beliefs.hypotheses.push(step(t));
            }
            brain.update(SimTime(t), &params(), None, None);
            let mut b = body(t);
            heard_at_2(&mut b);
            brain.weapon_options(&b, &character(), &mut Open, &mut rng);
            if brain.mind.arms.active.is_some() {
                assert!((2.0..2.25).contains(&t), "{t}");
                assert_eq!(brain.mind.arms.detonate_why, "someone heard by them");
                return;
            }
        }
        panic!("not set off");
    }

    #[test]
    fn a_dying_bot_sets_its_satchels_off_at_an_enemy_by_them() {
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain
            .explosives
            .thrown_satchel(Vec3::new(560.0, 60.0, -36.0), SimTime(0.0));
        // Seen by them a moment ago, 230 units off: not worth it for a healthy bot yet.
        brain.beliefs.on_sighting(&seen(1.5, Vec3::new(560.0, 290.0, 0.0)));
        let mut rng = BotRng::new(7, 7);
        brain.mind.arms.satchels = Some(SatchelPlan {
            lost_wait: 10.0,
            lie_until: SimTime(100.0),
        });
        brain.update(SimTime(2.0), &params(), None, None);
        let mut b = body(2.0);
        b.arsenal.retain(|a| a.id != WeaponId::HandGrenade);
        radio(&mut b, 0);
        brain.weapon_options(&b, &character(), &mut Open, &mut rng);
        assert!(brain.mind.arms.active.is_none());
        brain.beliefs.on_damage(&lb_knowledge::DamageStimulus {
            t: SimTime(2.05),
            bearing: None,
            amount: 30,
            bits: 0,
        });
        brain.update(SimTime(2.1), &params(), None, None);
        b.now = SimTime(2.1);
        b.health = 20.0;
        brain.weapon_options(&b, &character(), &mut Open, &mut rng);
        assert!(brain.mind.arms.active.is_some());
        assert_eq!(brain.mind.arms.detonate_why, "dying by them");
    }

    #[test]
    fn a_bot_in_the_blast_of_its_satchels_backs_off_before_setting_them_off() {
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain
            .explosives
            .thrown_satchel(Vec3::new(260.0, 0.0, -36.0), SimTime(0.0));
        brain.beliefs.on_sighting(&seen(2.0, Vec3::new(300.0, 100.0, 0.0)));
        brain.update(SimTime(2.0), &params(), None, None);
        let mut b = body(2.0);
        b.arsenal.retain(|a| a.id != WeaponId::HandGrenade);
        radio(&mut b, 0);
        b.weapon = Some(WeaponId::Satchel);
        let mut rng = BotRng::new(7, 7);
        brain.weapon_options(&b, &character(), &mut Open, &mut rng);
        assert_eq!(brain.mind.arms.detonate_why, "an enemy in their blast");
        // An enemy by them, but the bot 260 units off them: the radio waits while it backs off.
        assert_eq!(brain.detonate_go(&b), Ok(false));
        assert!(
            brain.mind.arms.dodge.is_some_and(|(_, away)| away.x < -0.9),
            "away from them"
        );
        b.origin = Vec3::new(-80.0, 0.0, 0.0);
        assert_eq!(brain.detonate_go(&b), Ok(true), "out of their blast");
    }

    /// Open floor where a long jump lands safely anywhere.
    struct Leaps;

    impl Tracer for Leaps {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            Open.trace(q)
        }

        fn point_contents(&mut self, p: Vec3) -> i32 {
            Open.point_contents(p)
        }
    }

    impl NavService for Leaps {
        fn go_to(&mut self, dest: Vec3) -> (NavStatus, Option<NavStep>) {
            Open.go_to(dest)
        }
        fn roam(&mut self, rng: &mut Pcg32) -> Option<NavStep> {
            Open.roam(rng)
        }
        fn away_from(&mut self, threat: Vec3) -> Option<Vec3> {
            Open.away_from(threat)
        }
        fn available(&self) -> bool {
            true
        }
        fn leap_lands(&mut self, view: Vec3) -> Option<Vec3> {
            let (forward, _, _) = view_angle_vectors(Vec3::new(0.0, view.y, 0.0));
            Some(forward * 420.0)
        }
    }

    #[test]
    fn with_the_module_a_satchel_at_an_enemy_further_off_goes_from_a_long_jump() {
        // The jump the satchel throw asks for, with the enemy `d` units off and long jumps landing safely or not.
        let jump_keys = |d: f32, safe: bool| {
            let enemy = Vec3::new(d, 0.0, 0.0);
            let mut ch = character();
            ch.tricks.satchel_jump = 1.0;
            let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&ch.skill));
            // The satchel in hand, drawn.
            let _ = brain.motor.weapon.update(
                SimTime(0.0),
                Some(WeaponId::Satchel),
                None,
                &mut smallvec::SmallVec::new(),
            );
            let mut rng = BotRng::new(7, 7);
            for i in 0..200 {
                let t = f64::from(i) * 0.05;
                brain.beliefs.on_sighting(&seen(t, enemy));
                brain.update(SimTime(t), &params(), None, None);
                let mut b = body(t);
                b.arsenal.clear();
                b.arsenal.push(Armed::new(WeaponId::Satchel, None, Some(3)));
                b.weapon = Some(WeaponId::Satchel);
                b.has_longjump = true;
                if brain.mind.arms.active.is_none() {
                    brain.weapon_options(&b, &ch, &mut Open, &mut rng);
                    continue;
                }
                // Running at the enemy; the view comes onto what the throw asks for.
                b.velocity = Vec3::new(270.0, 0.0, 0.0);
                brain.intents = Default::default();
                if safe {
                    brain.run_protocols(&b, &ch, &mut Leaps, &mut rng);
                } else {
                    brain.run_protocols(&b, &ch, &mut Open, &mut rng);
                }
                if let Some((Prio::Protocol, s)) = brain.intents.stance {
                    return Some(s);
                }
                if let Some((_, LookIntent::Angles(a))) = brain.intents.look {
                    brain.motor.view = a;
                }
            }
            None
        };
        let leap = jump_keys(650.0, true).expect("a jump");
        assert!(leap.longjump && !leap.jump, "{leap:?}");
        assert_eq!(
            jump_keys(650.0, false),
            None,
            "no long jump where it would not land safely, nor a plain one that would not get the satchel there"
        );
        let near = jump_keys(450.0, true).expect("a jump");
        assert!(near.jump && !near.longjump, "a plain jump closer: {near:?}");
    }

    #[test]
    fn the_radio_waits_for_an_enemy_coming_at_the_satchels() {
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain
            .explosives
            .thrown_satchel(Vec3::new(560.0, 60.0, -36.0), SimTime(0.0));
        let dress = |b: &mut Body| {
            b.arsenal.retain(|a| a.id != WeaponId::HandGrenade);
            radio(b, 1);
            b.weapon = Some(WeaponId::Satchel);
        };
        // Walking at them from 900 units off, at 250 units a second: 700 units away from them at t = 0.8 s.
        let walk = |t: f64| Vec3::new(560.0 + 900.0 - 250.0 * t as f32, 60.0, 0.0);
        let mut rng = BotRng::new(7, 7);
        let mut t = 0.0;
        let mut armed = None;
        let mut pressed = None;
        while t < 6.0 && pressed.is_none() {
            let mut sight = seen(t, walk(t));
            sight.first = t == 0.0;
            brain.beliefs.on_sighting(&sight);
            brain.update(SimTime(t), &params(), None, None);
            let mut b = body(t);
            b.origin = Vec3::new(0.0, -300.0, 0.0);
            b.eye = b.origin + Vec3::Z * 28.0;
            dress(&mut b);
            brain.weapon_options(&b, &character(), &mut Open, &mut rng);
            if armed.is_none() && brain.mind.arms.active.is_some() {
                armed = Some((t, brain.mind.arms.detonate_why));
            }
            if brain.mind.arms.active.is_some() && brain.detonate_go(&b) == Ok(true) {
                pressed = Some(walk(t).distance(Vec3::new(560.0, 60.0, -36.0)));
            }
            t += 0.1;
        }
        let (at, why) = armed.expect("the radio came up");
        assert_eq!(why, "an enemy coming at them");
        assert!(at < 2.9, "{at}");
        let d = pressed.expect("pressed");
        assert!((150.0..=230.0).contains(&d), "pressed as it came into the blast: {d}");
    }

    #[test]
    fn a_throwable_in_hand_still_looks_at_the_enemy() {
        let enemy = Vec3::new(400.0, 100.0, 0.0);
        let aimed = |weapon: WeaponId| {
            let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
            // No throws: only the aim is looked at.
            brain.mind.arms.next_throw = SimTime(100.0);
            brain.mind.arms.next_barrage = SimTime(100.0);
            let mut rng = BotRng::new(7, 7);
            for i in 0..10 {
                let t = f64::from(i) * 0.1;
                let mut sight = seen(t, enemy);
                sight.first = i == 0;
                brain.beliefs.on_sighting(&sight);
                brain.update(SimTime(t), &params(), None, None);
                let mut b = body(t);
                b.arsenal.retain(|a| a.id != WeaponId::HandGrenade);
                b.arsenal.push(Armed::new(WeaponId::Snark, None, Some(5)));
                b.weapon = Some(weapon);
                brain.act(&b, &character(), &mut Open, None, &mut rng);
            }
            matches!(
                brain.intents.look,
                Some((Prio::Threat, LookIntent::Point { engaged: true, .. }))
            )
        };
        assert!(aimed(WeaponId::Glock));
        assert!(aimed(WeaponId::Snark), "the snarks in hand, the glock coming out");
    }

    #[test]
    fn on_the_satchel_level_the_satchel_is_not_aimed_the_view_on_the_ground_toward_the_enemy() {
        use lb_game::gungame::GunGame;
        let enemy = Vec3::new(450.0, 100.0, 0.0);
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain.mind.arms.next_throw = SimTime(100.0);
        let mut rng = BotRng::new(7, 7);
        for i in 0..10 {
            let t = f64::from(i) * 0.1;
            let mut sight = seen(t, enemy);
            sight.first = i == 0;
            brain.beliefs.on_sighting(&sight);
            brain.update(SimTime(t), &params(), None, None);
            let mut b = body(t);
            b.arsenal.clear();
            b.arsenal.push(Armed::new(WeaponId::Satchel, None, Some(5)));
            b.weapon = Some(WeaponId::Satchel);
            b.gungame = Some(GunGame::drill(1, Kit::Throwable(WeaponId::Satchel)));
            b.allowed = Kit::Throwable(WeaponId::Satchel).weapons();
            brain.act(&b, &character(), &mut Open, None, &mut rng);
        }
        assert_eq!(brain.mind.hold_fire, Some("nothing to fire but throws"));
        let Some((Prio::Threat, LookIntent::Point { at, engaged: false })) = brain.intents.look else {
            panic!("{:?}", brain.intents.look);
        };
        let view = lb_core::math::dir_to_view_angles(at - Vec3::new(0.0, 0.0, 28.0));
        let toward = lb_core::math::dir_to_view_angles(enemy);
        assert!(
            (view.y - toward.y).abs() < 1.0 && (10.0..20.0).contains(&view.x),
            "toward the enemy, some 15° down at the ground: {view}"
        );
    }

    #[test]
    fn on_the_tripmine_level_no_mine_is_aimed_at_an_enemy() {
        use lb_game::gungame::GunGame;
        let enemy = Vec3::new(900.0, 100.0, 0.0);
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain.mind.arms.next_mine = SimTime(100.0);
        let mut rng = BotRng::new(7, 7);
        for i in 0..10 {
            let t = f64::from(i) * 0.1;
            let mut sight = seen(t, enemy);
            sight.first = i == 0;
            brain.beliefs.on_sighting(&sight);
            brain.update(SimTime(t), &params(), None, None);
            let mut b = body(t);
            b.arsenal.retain(|a| a.id != WeaponId::HandGrenade);
            b.arsenal.push(Armed::new(WeaponId::Tripmine, None, Some(5)));
            b.weapon = Some(WeaponId::Tripmine);
            b.gungame = Some(GunGame::drill(1, Kit::Mines));
            b.allowed = Kit::Mines.weapons();
            brain.act(&b, &character(), &mut Open, None, &mut rng);
        }
        assert_eq!(
            brain.mind.choice,
            Some(lb_combat::policy::Choice::Use(WeaponId::Tripmine))
        );
        assert!(
            !matches!(brain.intents.look, Some((Prio::Threat, _))),
            "{:?}",
            brain.intents.look
        );
        assert_eq!(brain.mind.hold_fire, Some("mines are laid, not fired"));
    }

    /// Open floor, with somewhere to run to away from any threat: 500 units along -x.
    struct Away;

    impl Tracer for Away {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            Open.trace(q)
        }

        fn point_contents(&mut self, p: Vec3) -> i32 {
            Open.point_contents(p)
        }
    }

    impl NavService for Away {
        fn go_to(&mut self, dest: Vec3) -> (NavStatus, Option<NavStep>) {
            let mut step = NavStep::hold(dest);
            step.move_dir = dest.truncate().normalize_or_zero();
            step.speed = 300.0;
            (NavStatus::Moving, Some(step))
        }
        fn roam(&mut self, _rng: &mut Pcg32) -> Option<NavStep> {
            None
        }
        fn away_from(&mut self, _threat: Vec3) -> Option<Vec3> {
            Some(Vec3::new(-500.0, 0.0, 0.0))
        }
        fn available(&self) -> bool {
            true
        }
    }

    #[test]
    fn on_the_tripmine_level_a_bot_found_in_its_cover_runs_on_not_back_past_its_mines() {
        use lb_game::gungame::GunGame;
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain.mind.task = Some(crate::goals::Task::Hide {
            dest: Vec3::ZERO,
            threat: Vec3::new(200.0, 0.0, 0.0),
            at: SimTime(0.5),
            arrived: true,
        });
        brain.beliefs.on_sighting(&seen(1.0, Vec3::new(150.0, 0.0, 0.0)));
        brain.update(SimTime(1.0), &params(), None, None);
        let mut b = body(1.0);
        b.arsenal.push(Armed::new(WeaponId::Tripmine, None, Some(5)));
        b.gungame = Some(GunGame::drill(1, Kit::Mines));
        b.allowed = Kit::Mines.weapons();
        brain.intents.clear();
        brain.retreat(&b, &character(), &mut Away, &mut BotRng::new(3, 3));
        let (_, mv) = brain.intents.movement.expect("running");
        assert!(mv.dir.x < -0.9 && mv.speed > 100.0, "{mv:?}");
        assert!(matches!(
            brain.mind.task,
            Some(crate::goals::Task::Hide { arrived: false, .. })
        ));
        // Not back past a mine of its own lying that way.
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain.mind.task = Some(crate::goals::Task::Hide {
            dest: Vec3::ZERO,
            threat: Vec3::new(200.0, 0.0, 0.0),
            at: SimTime(0.5),
            arrived: true,
        });
        brain
            .explosives
            .placed_mine(Vec3::new(-200.0, 30.0, -28.0), Vec3::Z, SimTime(-5.0));
        brain.beliefs.on_sighting(&seen(1.0, Vec3::new(150.0, 0.0, 0.0)));
        brain.update(SimTime(1.0), &params(), None, None);
        brain.intents.clear();
        brain.retreat(&b, &character(), &mut Away, &mut BotRng::new(3, 3));
        assert!(
            brain.intents.movement.is_none_or(|(_, mv)| mv.dir.x > -0.5),
            "{:?}",
            brain.intents.movement
        );
    }

    #[test]
    fn a_bot_gets_out_of_its_own_mines_beam_before_it_arms_and_keeps_out() {
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        // Dropped under the bot: it arms 2.5 s after.
        brain
            .explosives
            .placed_mine(Vec3::new(5.0, 0.0, -28.0), Vec3::Z, SimTime(0.0));
        let mut rng = BotRng::new(1, 1);
        brain.intents.clear();
        brain.dodge(&body(0.5), &character(), &mut Open, &mut rng);
        assert!(brain.intents.movement.is_none(), "not about to arm yet");
        brain.intents.clear();
        brain.dodge(&body(1.5), &character(), &mut Open, &mut rng);
        let (prio, mv) = brain.intents.movement.expect("out of it");
        assert_eq!(prio, Prio::Protocol, "whatever a protocol wants");
        assert!(mv.speed > 100.0 && mv.dir.x < -0.9, "{mv:?}");
        // Out of it, a move back at it goes round its foot, even before it arms.
        let mut b = body(1.6);
        b.origin.x = -60.0;
        brain.intents.clear();
        brain.intents.movement(
            Prio::Goal,
            MoveIntent {
                dir: Vec2::X,
                speed: 300.0,
            },
        );
        brain.beam_guard(&b, &mut Open);
        let (_, mv) = brain.intents.movement.expect("a move");
        assert!(mv.speed > 100.0 && mv.dir.y.abs() > 0.3, "round it: {mv:?}");
    }

    /// A floor at z = -36, with walls at y = ±`half` and a ceiling at `ceiling` when given.
    struct Room {
        half: Option<f32>,
        ceiling: Option<f32>,
    }

    impl Tracer for Room {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            let d = q.end - q.start;
            let mut hits: Vec<(f32, Vec3)> = Vec::new();
            let mut plane = |at: f32, from: f32, to: f32, normal: Vec3| {
                if (from - at) * (to - at) < 0.0 {
                    hits.push(((at - from) / (to - from), normal));
                }
            };
            plane(-36.0, q.start.z, q.end.z, Vec3::Z);
            if let Some(c) = self.ceiling {
                plane(c, q.start.z, q.end.z, Vec3::NEG_Z);
            }
            if let Some(h) = self.half {
                plane(h, q.start.y, q.end.y, Vec3::NEG_Y);
                plane(-h, q.start.y, q.end.y, Vec3::Y);
            }
            match hits.into_iter().min_by(|a, b| a.0.total_cmp(&b.0)) {
                Some((f, normal)) => {
                    let mut t = Trace::clear(q.start + d * f);
                    t.fraction = f;
                    t.normal = normal;
                    t
                }
                None => Trace::clear(q.end),
            }
        }

        fn point_contents(&mut self, _p: Vec3) -> i32 {
            contents::EMPTY
        }
    }

    impl NavService for Room {
        fn go_to(&mut self, dest: Vec3) -> (NavStatus, Option<NavStep>) {
            (NavStatus::Moving, Some(NavStep::hold(dest)))
        }
        fn roam(&mut self, _rng: &mut Pcg32) -> Option<NavStep> {
            None
        }
        fn away_from(&mut self, _threat: Vec3) -> Option<Vec3> {
            None
        }
        fn available(&self) -> bool {
            true
        }
    }

    #[test]
    fn a_beam_is_walked_round_ducked_under_or_kept_off_as_it_runs() {
        let mut open = Room {
            half: None,
            ceiling: None,
        };
        let across = |z: f32| (Vec3::new(0.0, -60.0, z), Vec3::new(0.0, 60.0, z));
        assert_eq!(
            beam_pass(&mut open, Vec3::new(0.0, 0.0, -28.0), Vec3::new(0.0, 0.0, 200.0)),
            BeamPass::Around,
            "standing up from the floor"
        );
        let (a, b) = across(14.0);
        assert_eq!(beam_pass(&mut open, a, b), BeamPass::Under, "50 over the floor");
        let (a, b) = across(-16.0);
        assert_eq!(beam_pass(&mut open, a, b), BeamPass::Blocked, "20 over the floor");
    }

    #[test]
    fn a_bot_ducks_under_a_high_beam_keeps_off_a_low_one_and_goes_round_one_standing_up() {
        let guard = |pos: Vec3, end: Vec3, nav: &mut Room, airborne: bool| {
            let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
            let dir = (end - pos).normalize();
            brain.explosives.placed_mine(pos, dir, SimTime(-10.0));
            brain.explosives.mines[0].own = false;
            let mut b = body(1.0);
            b.on_ground = !airborne;
            brain.keep_off_beams(&b, nav);
            brain.intents.clear();
            brain.intents.movement(
                Prio::Goal,
                MoveIntent {
                    dir: Vec2::X,
                    speed: 270.0,
                },
            );
            brain.beam_guard(&b, nav);
            brain
        };
        let mut open = Room {
            half: None,
            ceiling: None,
        };
        // Across the way 100 ahead, 50 over the floor: ducked under, the move kept at a careful pace.
        let b = guard(
            Vec3::new(100.0, -60.0, 14.0),
            Vec3::new(100.0, 60.0, 14.0),
            &mut open,
            false,
        );
        assert!(matches!(b.intents.stance, Some((Prio::Protocol, s)) if s.duck && !s.jump));
        let (_, mv) = b.intents.movement.unwrap();
        assert!(mv.dir == Vec2::X && mv.speed == CAREFUL, "{mv:?}");
        // 20 over the floor 50 ahead: not jumped, the way turned off it.
        let b = guard(
            Vec3::new(50.0, -60.0, -16.0),
            Vec3::new(50.0, 60.0, -16.0),
            &mut open,
            false,
        );
        assert!(b.intents.stance.is_none_or(|(_, s)| !s.jump));
        let (_, mv) = b.intents.movement.unwrap();
        assert!(mv.dir.x < 0.5, "{mv:?}");
        // A high one close by while still standing: wait until down.
        let b = guard(
            Vec3::new(30.0, -60.0, 14.0),
            Vec3::new(30.0, 60.0, 14.0),
            &mut open,
            false,
        );
        assert_eq!(b.intents.movement.map(|(_, m)| m.speed), Some(0.0));
        assert!(matches!(b.intents.stance, Some((Prio::Protocol, s)) if s.duck));
        // Standing up from the floor 60 ahead: round it.
        let b = guard(
            Vec3::new(60.0, 0.0, -28.0),
            Vec3::new(60.0, 0.0, 200.0),
            &mut open,
            false,
        );
        let (_, mv) = b.intents.movement.unwrap();
        assert!(mv.speed == CAREFUL && mv.dir.y.abs() > 0.3, "{mv:?}");
        // With no room beside it the bot stops, and the way is told to keep off it.
        let mut narrow = Room {
            half: Some(20.0),
            ceiling: None,
        };
        let b = guard(
            Vec3::new(60.0, 0.0, -28.0),
            Vec3::new(60.0, 0.0, 200.0),
            &mut narrow,
            false,
        );
        assert_eq!(b.intents.movement.map(|(_, m)| m.speed), Some(0.0));
        assert_eq!(b.explosives.mines[0].pass, Some(BeamPass::Blocked));
    }

    /// The bot running at `wish` from `from` for `frames` hundredths of a second with the game's ground movement
    /// (friction 4 below a stop speed of 100, acceleration 10), its moves past the beam guard; where it went, and
    /// whether its box ever met the stretch of a beam at its height.
    fn run_by(brain: &mut BotBrain, nav: &mut Room, from: Vec3, wish: Vec2, frames: usize) -> (Vec3, Option<Vec3>) {
        let dt = 0.01;
        let (mut at, mut v) = (from, wish * 270.0);
        let mut met = None;
        for i in 0..frames {
            let mut b = body(1.0 + i as f64 * f64::from(dt));
            b.origin = at;
            b.eye = at + Vec3::Z * 28.0;
            b.velocity = v.extend(0.0);
            brain.keep_off_beams(&b, nav);
            brain.intents.clear();
            brain.intents.movement(
                Prio::Goal,
                MoveIntent {
                    dir: wish,
                    speed: 270.0,
                },
            );
            brain.beam_guard(&b, nav);
            let (_, mv) = brain.intents.movement.unwrap();
            let speed = v.length();
            if speed > 0.1 {
                v *= (1.0 - dt * 4.0 * speed.max(100.0) / speed).max(0.0);
            }
            let wishdir = mv.dir.normalize_or_zero();
            let add = mv.speed - v.dot(wishdir);
            if add > 0.0 {
                v += wishdir * add.min(10.0 * dt * mv.speed);
            }
            at += (v * dt).extend(0.0);
            for m in &brain.explosives.mines {
                let end = beam_of(m).unwrap();
                if let Some((p, q)) = beam_slice(at, 36.0, m.pos, end)
                    && box_gap(at.truncate(), p, q) < 16.0
                {
                    met.get_or_insert(at);
                }
            }
        }
        (at, met)
    }

    #[test]
    fn a_bot_walking_at_the_foot_of_a_beam_never_touches_it_and_gets_past() {
        let mut open = Room {
            half: None,
            ceiling: None,
        };
        for foot in [
            Vec3::new(60.0, 4.0, -28.0),
            Vec3::new(30.0, -10.0, -28.0),
            Vec3::new(18.0, 0.0, -28.0),
        ] {
            let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
            brain.explosives.placed_mine(foot, Vec3::Z, SimTime(-10.0));
            brain.explosives.mines[0].own = false;
            // Coming up to it at a run, as a bot does.
            let (at, met) = run_by(&mut brain, &mut open, Vec3::new(-100.0, 0.0, 0.0), Vec2::X, 160);
            assert_eq!(met, None, "foot {foot:?}");
            assert!(at.x > foot.x + 40.0, "got past it: {at:?} (foot {foot:?})");
        }
        // A mine on a ramp: its beam slants up through body height over 37 units of the way.
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain
            .explosives
            .placed_mine(Vec3::new(80.0, 6.0, -28.0), Vec3::new(0.5, 0.0, 0.866), SimTime(-10.0));
        brain.explosives.mines[0].own = false;
        let (at, met) = run_by(&mut brain, &mut open, Vec3::ZERO, Vec2::X, 120);
        assert_eq!(brain.explosives.mines[0].pass, Some(BeamPass::Around));
        assert_eq!(met, None);
        assert!(at.x > 160.0, "got past it: {at:?}");
    }

    #[test]
    fn a_bot_running_at_a_beam_it_cannot_get_past_never_slides_into_it() {
        // Across the way at body height under a ceiling too low to jump it: walked along to its end and round.
        let mut low = Room {
            half: None,
            ceiling: Some(60.0),
        };
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain
            .explosives
            .placed_mine(Vec3::new(120.0, -60.0, 0.0), Vec3::Y, SimTime(-10.0));
        brain.explosives.mines[0].own = false;
        brain.explosives.mines[0].beam_end = Some(Vec3::new(120.0, 60.0, 0.0));
        let (at, met) = run_by(&mut brain, &mut low, Vec3::ZERO, Vec2::X, 200);
        assert_eq!(brain.explosives.mines[0].pass, Some(BeamPass::Blocked));
        assert_eq!(met, None);
        assert!(at.x > 150.0, "round its end: {at:?}");
        // Between walls it spans: the bot stops short of it.
        let mut shut = Room {
            half: Some(64.0),
            ceiling: Some(60.0),
        };
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain
            .explosives
            .placed_mine(Vec3::new(120.0, -64.0, 0.0), Vec3::Y, SimTime(-10.0));
        brain.explosives.mines[0].own = false;
        brain.explosives.mines[0].beam_end = Some(Vec3::new(120.0, 64.0, 0.0));
        let (at, met) = run_by(&mut brain, &mut shut, Vec3::ZERO, Vec2::X, 200);
        assert_eq!(met, None);
        assert!(at.x < 120.0 - 16.0, "{at:?}");
        // Told to stand still while running at the foot of one: it pushes against its run rather than slide on.
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain
            .explosives
            .placed_mine(Vec3::new(50.0, 0.0, -28.0), Vec3::Z, SimTime(-10.0));
        brain.explosives.mines[0].own = false;
        let mut b = body(1.0);
        b.velocity = Vec3::new(300.0, 0.0, 0.0);
        brain.keep_off_beams(&b, &mut low);
        brain.intents.clear();
        brain.intents.movement(Prio::Goal, lb_combat::arms::stop());
        brain.beam_guard(&b, &mut low);
        let (prio, mv) = brain.intents.movement.unwrap();
        assert!(prio == Prio::Protocol && mv.dir.x < -0.99 && mv.speed > 200.0, "{mv:?}");
    }

    #[test]
    fn a_trap_over_keeps_its_rest_and_one_given_up_waits_a_little() {
        use lb_decision::Trap;
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        let mut rng = BotRng::new(1, 1);
        brain.mind.trap_rest_until = SimTime(6.0);
        brain.left_goal(GoalKind::PlantTrap(Trap::Mine(0)), &character(), SimTime(5.0), &mut rng);
        assert_eq!(brain.mind.trap_rest_until, SimTime(6.0), "over: its task done");
        brain.mind.task = Some(crate::goals::Task::Trap {
            trap: Trap::Mine(0),
            mines_before: 0,
            started: None,
            until: None,
        });
        brain.left_goal(GoalKind::PlantTrap(Trap::Mine(0)), &character(), SimTime(5.0), &mut rng);
        assert_eq!(brain.mind.trap_rest_until, SimTime(13.0), "given up");
    }

    #[test]
    fn snarks_are_run_from_unless_the_egon_is_in_hand() {
        use lb_knowledge::ProjectileSighting;
        let snark = |brain: &mut BotBrain, t: f64| {
            brain.explosives.on_sighting(&ProjectileSighting {
                t: SimTime(t),
                kind: lb_game::entities::ProjectileKind::Snark,
                index: 70,
                pos: Vec3::new(150.0, 0.0, -20.0),
                vel: Vec3::new(-200.0, 0.0, 0.0),
                own: false,
                beam: None,
                armed: false,
            });
        };
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        snark(&mut brain, 1.0);
        brain.snark_defense(&body(1.0), &mut Open);
        let (_, away) = brain.mind.arms.dodge.expect("running");
        assert!(away.x < -0.9, "away from the snark: {away:?}");
        assert!(brain.intents.weapon.is_none(), "the gun is left to the fight");
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        snark(&mut brain, 1.0);
        let mut b = body(1.0);
        b.weapon = Some(WeaponId::Egon);
        b.arsenal.push(Armed::new(WeaponId::Egon, None, Some(50)));
        brain.motor.view = lb_core::math::dir_to_view_angles(Vec3::new(150.0, 0.0, -20.0) - b.eye);
        brain.snark_defense(&b, &mut Open);
        assert!(brain.mind.arms.dodge.is_none());
        let (_, w) = brain.intents.weapon.expect("burning it");
        assert_eq!((w.select, w.fire), (Some(WeaponId::Egon), lb_motor::Fire::Primary));
    }

    #[test]
    fn all_the_snarks_go_at_an_enemy_close_by() {
        let enemy = Vec3::new(120.0, 20.0, 0.0);
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain.beliefs.on_sighting(&seen(0.0, enemy));
        brain.mind.target = Some(PlayerKey { slot: 5, userid: 50 });
        let snarks = |b: &mut Body| b.arsenal.push(Armed::new(WeaponId::Snark, None, Some(6)));
        let (t, what) = first_protocol(&mut brain, 0.0, 3.0, Some((enemy, 3.0)), &snarks).expect("a barrage");
        assert_eq!(what, "snark barrage");
        assert!(t < 2.0, "{t}");
    }

    #[test]
    fn grenades_go_after_a_lost_enemy_and_snarks_at_one_in_sight() {
        let enemy = Vec3::new(500.0, 150.0, 0.0);
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain.beliefs.on_sighting(&seen(0.0, enemy));
        assert_eq!(
            considered(&mut brain, 1.0, &[]),
            Some("grenade"),
            "lost a second ago, 500 units away"
        );
        // In sight, grenades and snarks both fit: each as likely as its chance, grenades (almost always taken) more
        // often.
        let (mut snarks, mut grenades) = (0, 0);
        for seed in 0..20u64 {
            let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
            let mut rng = BotRng::new(seed, seed);
            for i in 0..40 {
                let t = f64::from(i) * 0.1;
                brain.beliefs.on_sighting(&seen(t, enemy));
                brain.update(SimTime(t), &params(), None, None);
                let mut b = body(t);
                b.arsenal.push(Armed::new(WeaponId::Snark, None, Some(5)));
                brain.weapon_options(&b, &character(), &mut Open, &mut rng);
                match brain.mind.arms.active.as_ref().map(Active::name) {
                    Some("snark stream") => snarks += 1,
                    Some("grenade") => grenades += 1,
                    Some(other) => panic!("{other}"),
                    None => continue,
                }
                break;
            }
        }
        assert!(grenades > snarks && snarks > 0, "{snarks} snarks, {grenades} grenades");
    }

    #[test]
    fn skilled_bots_throw_their_grenades_in_a_series_until_an_enemy_comes_close() {
        let lost = Vec3::new(500.0, 150.0, 0.0);
        let start = |level: u8| {
            let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
            brain.beliefs.on_sighting(&seen(0.0, lost));
            let mut ch = character();
            ch.skill = Presets::default().at(level);
            let mut rng = BotRng::new(7, 7);
            for i in 0..20 {
                let t = 1.0 + f64::from(i) * 0.1;
                brain.update(SimTime(t), &params(), None, None);
                brain.weapon_options(&body(t), &ch, &mut Open, &mut rng);
                if brain.mind.arms.active.is_some() {
                    break;
                }
            }
            brain
        };
        let expert = start(100);
        assert_eq!(expert.mind.arms.active.as_ref().map(Active::name), Some("grenade"));
        assert!(expert.mind.arms.series, "an expert throws them all");
        assert!(!start(50).mind.arms.series, "a normal bot one at a time");
        // An enemy in sight close by ends the series: the gun then.
        let mut brain = start(100);
        brain.mind.arms.active = None;
        brain.mind.arms.next_throw = SimTime::ZERO;
        brain.beliefs.on_sighting(&seen(3.0, Vec3::new(200.0, 0.0, 0.0)));
        brain.update(SimTime(3.0), &params(), None, None);
        brain.weapon_options(&body(3.0), &character(), &mut Open, &mut BotRng::new(7, 7));
        assert!(!brain.mind.arms.series && brain.mind.arms.active.is_none());
    }

    #[test]
    fn a_grenade_goes_where_an_enemy_is_expected_with_none_known() {
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain.approach = Some(Vec3::new(600.0, 100.0, 0.0));
        // In a series every look throws; out of one, now and then.
        brain.mind.arms.series = true;
        assert_eq!(considered(&mut brain, 1.0, &[]), Some("grenade"));
        assert_eq!(brain.mind.arms.stats.blind, 1);
        let mut none = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        none.mind.arms.series = true;
        assert_eq!(considered(&mut none, 1.0, &[]), None, "nowhere an enemy is expected");
        assert!(!none.mind.arms.series);
    }

    #[test]
    fn a_bot_keeps_out_of_its_own_grenade_about_to_go_off() {
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        brain
            .explosives
            .thrown_grenade(Vec3::new(320.0, 0.0, -36.0), SimTime(3.0), SimTime(1.0), 800.0);
        let going = |brain: &mut BotBrain, dir: Vec2, now: f64| {
            brain.explosives.update(SimTime(now));
            brain.intents.clear();
            brain.intents.movement(Prio::Goal, MoveIntent { dir, speed: 300.0 });
            brain.blast_guard(&body(now));
            let (_, mv) = brain.intents.movement.expect("a move");
            mv.dir.normalize_or_zero() * mv.speed
        };
        // Straight at it nothing is left of the move; across and at it, the part across.
        assert!(going(&mut brain, Vec2::X, 1.5).length() < 1.0);
        let v = going(&mut brain, Vec2::new(1.0, 1.0).normalize(), 1.5);
        assert!(v.x.abs() < 1.0 && v.y > 200.0, "{v:?}");
        assert_eq!(going(&mut brain, -Vec2::X, 1.5), -Vec2::X * 300.0, "away from it");
        // Once it has gone off the way is free again.
        assert_eq!(going(&mut brain, Vec2::X, 3.5), Vec2::X * 300.0);
    }

    #[test]
    fn a_bot_found_in_its_cover_fights_back_instead_of_standing() {
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        // Hidden from an enemy last known 200 units off.
        let hide = |brain: &mut BotBrain| {
            brain.mind.task = Some(crate::goals::Task::Hide {
                dest: Vec3::ZERO,
                threat: Vec3::new(200.0, 0.0, 0.0),
                at: SimTime(0.5),
                arrived: true,
            });
            brain.intents.clear();
            brain.retreat(&body(1.0), &character(), &mut Open, &mut BotRng::new(3, 3));
            brain.intents.movement.map(|(_, mv)| mv.speed)
        };
        // Nobody in sight: it stands and watches the way the threat would come.
        brain.beliefs.on_sighting(&seen(0.2, Vec3::new(200.0, 0.0, 0.0)));
        brain.update(SimTime(1.0), &params(), None, None);
        assert_eq!(hide(&mut brain), Some(0.0));
        // Found there, an enemy in sight 150 units off: it strafes and backs off rather than stand.
        brain.beliefs.on_sighting(&seen(1.0, Vec3::new(150.0, 0.0, 0.0)));
        brain.update(SimTime(1.0), &params(), None, None);
        assert!(hide(&mut brain).is_some_and(|speed| speed > 100.0));
    }

    #[test]
    fn a_bot_keeps_out_of_where_its_rocket_is_going_whatever_its_goal() {
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        let enemy = PlayerKey { slot: 5, userid: 50 };
        // Fired at an enemy 250 units off, the rocket guided there until 2 s.
        brain.mind.arms.guide = Some((SimTime(2.0), Vec3::new(250.0, 0.0, 0.0), enemy));
        let going = |brain: &mut BotBrain, dir: Vec2, now: f64| {
            brain.intents.clear();
            brain.intents.movement(Prio::Goal, MoveIntent { dir, speed: 300.0 });
            brain.blast_guard(&body(now));
            let (_, mv) = brain.intents.movement.expect("a move");
            mv.dir.normalize_or_zero() * mv.speed
        };
        assert!(
            going(&mut brain, Vec2::X, 1.0).length() < 1.0,
            "not on toward the blast"
        );
        assert_eq!(going(&mut brain, Vec2::Y, 1.0), Vec2::Y * 300.0, "aside is free");
        assert_eq!(going(&mut brain, Vec2::X, 2.5), Vec2::X * 300.0, "once it has gone off");
    }
}
