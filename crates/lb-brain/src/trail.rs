//! The tripmine trail, the way a GunGame player lays it (one was watched doing it on the stand): a straight stretch
//! of level floor (a lane of the map) run along at full speed with the view down, a mine dropped as fast as the
//! tripmine allows, some 85 units apart, each one's blast setting the next off; on along the lane past the last one,
//! then off to a place 450–800 units from every mine of it (out of its blast, never back past it), the view back on
//! its last mine and a pistol in hand; a moment's watch, a shot at once at an enemy by any mine (the chain takes the
//! rest), and with nobody by it, on the tripmine level, where the plugin hands mines back as they go off, a shot
//! anyway and on to the next trail; elsewhere it is left lying as a trap. On the tripmine level nothing else is taken
//! up meanwhile, and a bot short of mines sets an old trail of its own off first, to get them back.
//!
//! Which mine to shoot, and when, for every mine the bot knows of: at an enemy by it (what the whole chain it sets off
//! would do to it), at someone heard by the bot's own, at a trail nobody came by after its watch (the tripmine level),
//! at an enemy's mine in the way of a calm bot, and on the tripmine level at the bot's own old ones to get them back.
//! Never when the chain's blasts would cost the bot more than a wound, nor while a trail is being laid.

use lb_combat::arms::detonate::MineShot;
use lb_combat::arms::mine::{FloorPlanter, LAY_PITCH, floor_spot};
use lb_core::rng::BotRng;
use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3, dmath};
use lb_game::gungame::Kit;
use lb_game::mechanics::{blast_damage, blast_radius};
use lb_game::sounds::SoundKind;
use lb_game::weapons::WeaponId;
use lb_knowledge::explosives::Mine;
use lb_knowledge::{HypothesisKind, Relation, TrackState};
use lb_motor::{LookIntent, MoveIntent, Prio, WeaponIntent};
use lb_nav_api::{MapView, NavService, NavStatus};
use lb_worldq::{HullKind, TraceQuery, Tracer, contents};
use smallvec::SmallVec;

use crate::BotBrain;
use crate::arms::{Active, QUIET, ahead, carried, fresh};
use crate::goals::{FOND, FOND_TRAP_REST, MINES_LEVEL_GIVE_UP_REST, TRAP_REST, Task, at_spot};
use crate::mind::{Body, Character};

/// Mines a trail has (a player carries five; the GunGame plugin hands back those gone), and the fewest a new one is
/// begun with: short of them on the tripmine level, an old trail of the bot's own is set off first.
const TRAIL_MINES: u32 = 5;
const TRAIL_LEAST: i32 = 3;
/// Mines are dropped running at least this fast along the way, no closer than this to the last one of the trail, nor
/// to another mine, a player in sight (the placement trace would put it on him) or a spawn point.
const RUN_SPEED: f32 = 100.0;
const TRAIL_GAP: f32 = 64.0;
const MINE_CLEAR: f32 = 96.0;
const SPAWN_CLEAR: f32 = 128.0;
const PLAYER_CLEAR: f32 = 64.0;
/// Nor this close to a ladder's place this far above or below (a beam standing up at its foot runs up the ladder).
const LADDER_CLEAR: f32 = 96.0;
const LADDER_HEIGHT: f32 = 256.0;
/// A mine on the floor bursts some 77 units up: under a lower ceiling its blast would hurt nobody.
const HEADROOM: f32 = 84.0;
/// The goal asks for mines this far ahead, every frame.
const WINDOW: f64 = 0.3;
/// Lanes are looked for this many seconds of running away at most; one passing this close is one the bot is on (the
/// run begins where it is). A trail needs this much of a lane left (its mines and the run on past them), none of its
/// first `LANE_CLEAR` units by a known mine.
const LANE_REACH: f32 = 8.0;
const ON_LANE: f32 = 48.0;
const LANE_NEED: f32 = 512.0;
const LANE_CLEAR: f32 = 800.0;
const LANE_MINES: f32 = 128.0;
/// A lane costs this many seconds of running more: one heading toward an enemy in mind this close, one down a
/// corridor (no room either side), and a little chance; this much less along a busy way, times its flow.
const ENEMY_NEAR: f32 = 1200.0;
const TOWARD_ENEMY: f32 = 4.0;
const CORRIDOR: f32 = 128.0;
const CORRIDOR_COST: f32 = 1.0;
const LANE_CHANCE: f32 = 1.5;
const LANE_FLOW: f32 = 1.0;
/// A lane is taken only with a place to watch its trail from: the trail as it will lie (the first mine this far on
/// from the start, the next ones this far apart) and the bot where it runs on to.
const LAY_AHEAD: f32 = 75.0;
const LAY_GAP: f32 = 85.0;
const RUN_ON_FAR: f32 = 190.0;
/// The best this many lanes are looked at for a place to watch from that the bot walks to in a straight line.
const LANE_TRIES: usize = 4;
/// The run begins this close to the lane's start, the tripmine in hand (waited for this long at most there); it
/// steers onto the lane through a point this far on along it. The lane is to be reached by the graph's time half as
/// long again and this many seconds more.
const LANE_ENTER: f32 = 40.0;
const DRAW_WAIT: f64 = 1.5;
const RUN_AHEAD: f32 = 96.0;
const APPROACH_SLACK: f64 = 4.0;
/// The first mine is down this soon after the run began, or the trail is given up; the run lasts this long at most,
/// and stops this short of the lane's end.
const FIRST_MINE: f64 = 1.5;
const RUN_FOR: f64 = 4.0;
const RUN_OUT: f32 = 160.0;
/// Laid, the bot runs on along the lane this long, then off to where it watches the trail from: this far from every
/// mine of it (out of its blast; a player stops some 500–800 units off), no further back than this past its last
/// one, not passing this close to another mine of it on the way. Where there is no such place (the lane ends at a
/// wall), one back past the trail on another line will do, and failing that one where the trail's blast only wounds
/// the bot, this far off at least and at most. Stepping off takes this long at most.
const RUN_ON: f64 = 1.0;
const STAND_BAND: [f32; 2] = [450.0, 800.0];
const STAND_BEHIND: f32 = 64.0;
const STAND_PASS: f32 = 64.0;
/// A way back past the trail keeps this far from it: out of most of its blast, should someone set it off meanwhile.
const STAND_PASS_BACK: f32 = 250.0;
const WOUND_BAND: [f32; 2] = [250.0, 900.0];
const OFF_FOR: f64 = 5.0;
/// Of the places to watch from, this many of the nearest are tried for a straight walk there (a stair step at most).
const STRAIGHT_TRIES: usize = 6;
const STEP_UP: f32 = 18.0;
/// A place to watch from this close to an enemy in mind costs this many units more.
const STAND_ENEMY: f32 = 400.0;
const STAND_ENEMY_COST: f32 = 600.0;
/// Seconds the trail is watched once its mines armed: on the tripmine level a moment (a player waits about a second,
/// then shoots), elsewhere longer, then it is left lying as a trap.
const MINES_LEVEL_WATCH: [f32; 2] = [0.5, 1.5];
const WATCH: [f32; 2] = [6.0, 10.0];
/// After the watch, a trail being set off with nobody by it is waited for this long at most.
const BLOW_WAIT: f64 = 4.0;
/// A trail with nobody by it is shot at this many times at most.
const BLOWS: u8 = 2;
/// Rest after a trail on the tripmine level: the next one is begun at once.
const MINES_LEVEL_REST: [f32; 2] = [0.0, 0.5];
/// A chain going off costs the bot this much health at most, leaving it this much at least; where a suicide costs a
/// kill (GunGame's descore) it takes less.
const SELF_LOSS: f32 = 60.0;
const SELF_LEFT: f32 = 25.0;
const DESCORE_LOSS: f32 = 40.0;
/// An enemy where it will be this soon (the shot, and the chain's own delays) taking this much from the chain is worth
/// a shot.
const VICTIM_LEAD: f32 = 0.6;
const VICTIM: f32 = 80.0;
/// A friend in sight that would take this much holds the shot.
const FRIEND_SPARE: f32 = 20.0;
/// A sound this close to one of the bot's own mines in the last second: someone by it.
const HEARD_NEAR: f32 = 160.0;
const HEARD_FOR: f64 = 1.0;
/// Mines are shot this far off at most; lines of sight looked along for one per look.
const SHOOT_RANGE: f32 = 800.0;
const SIGHT_LINES: usize = 4;
/// On the tripmine level, with fewer mines than this left, an own mine lying this long with nobody about is shot to
/// get it back (the plugin hands mines back as they go off), the bot taking this much of the blast at most.
const RECYCLE_BELOW: i32 = 3;
const RECYCLE_AGE: f64 = 15.0;
const RECYCLE_LOSS: f32 = 10.0;
/// The game aims a blast at this height over a player's origin, on average.
const BODY_AIM: f32 = 22.0;
/// A player's box: half its width and half its height, standing.
const BODY_HALF: Vec3 = Vec3::new(16.0, 16.0, 36.0);
/// A mine seen this close to one of the trail's is that one.
const SAME_MINE: f32 = 24.0;
/// While a mine is shot, whether the bot is still spared by its chain is looked at this often; once it went off, the
/// rest of the chain goes off for this long (a mine sets the next off 0.1–0.3 s after the blast reaches it).
const SHOT_CHECK: f64 = 0.1;
const CHAIN_BURNS: f64 = 1.5;
/// Guns that set a mine off.
const MINE_GUNS: [WeaponId; 4] = [WeaponId::Python, WeaponId::Glock, WeaponId::Mp5, WeaponId::Gauss];

/// A trail being laid or watched, kept with the bot's weapons.
#[derive(Clone, Debug)]
pub struct TrailPlan {
    /// Its mines still there, in the order dropped: where each is and the way it faces.
    pub mines: SmallVec<[(Vec3, Vec3); 10]>,
    /// Mines dropped so far, and how many it is to have.
    pub dropped: u32,
    pub wanted: u32,
    /// The way the run goes; zero for an old trail set off to get its mines back.
    pub dir: Vec2,
    /// Mines are dropped until then: the goal asks for more every frame of the run.
    pub lay_until: SimTime,
    /// Set off with nobody by it from then on (the tripmine level), and the shots taken at it so far.
    pub blow_at: Option<SimTime>,
    pub blows: u8,
}

impl TrailPlan {
    pub(crate) fn new(wanted: u32) -> TrailPlan {
        TrailPlan {
            mines: SmallVec::new(),
            dropped: 0,
            wanted,
            dir: Vec2::ZERO,
            lay_until: SimTime::ZERO,
            blow_at: None,
            blows: 0,
        }
    }

    /// Mines are being dropped now.
    pub fn laying(&self, now: SimTime) -> bool {
        now < self.lay_until && self.dropped < self.wanted
    }

    /// The mine watched and shot at with nobody by the trail: the last one dropped, the nearest the bot as it stepped
    /// off on along the run; of an old trail, the nearest one when it was taken up.
    fn target(&self) -> Option<Vec3> {
        let m = if self.dir == Vec2::ZERO {
            self.mines.first()
        } else {
            self.mines.last()
        };
        m.map(|(p, _)| *p)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrailPhase {
    /// On the way to the lane's start, the tripmine drawn.
    Approach,
    /// Running along the lane, dropping mines.
    Run,
    /// On past the last one, then off to where the trail is watched from.
    Off,
    /// Watching it.
    Watch,
}

impl TrailPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            TrailPhase::Approach => "going to its lane",
            TrailPhase::Run => "laying",
            TrailPhase::Off => "stepping off",
            TrailPhase::Watch => "watching",
        }
    }
}

/// What the trail goal is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrailTask {
    /// The lane: where the run begins, the way it goes and how much of it there is (zero for an old trail set off).
    pub start: Vec3,
    pub dir: Vec2,
    pub length: f32,
    pub phase: TrailPhase,
    /// When the phase began, when the first mine went down, and when the lane is to be reached by.
    pub since: SimTime,
    pub started: Option<SimTime>,
    pub reach_by: SimTime,
    /// Where the trail is watched from, once chosen, and whether the bot runs there in a straight line.
    pub stand: Option<Vec3>,
    pub straight: bool,
    /// The watch ends then.
    pub until: Option<SimTime>,
}

/// Where a tripmine at `pos` facing `dir` bursts: the game pulls the blast out of its wall by (damage − 24) × 0.6.
pub(crate) fn burst_at(pos: Vec3, dir: Vec3, damage: f32) -> Vec3 {
    let dir = if dir == Vec3::ZERO { Vec3::Z } else { dir };
    pos + dir * ((damage - 24.0) * 0.6 - 8.0) + Vec3::Z
}

/// How far `p` is from a standing player's box around `origin` (where the game measures a blast's falloff to).
fn box_distance(p: Vec3, origin: Vec3) -> f32 {
    ((p - origin).abs() - BODY_HALF).max(Vec3::ZERO).length()
}

/// The game traces a blast to a random point of a player between its chest and its eyes and hits whatever part of it
/// the line meets first; on the head the damage is tripled (`sk_player_head` of HL's skill.cfg). Now and then it is.
const HEAD: f32 = 3.0;

/// The health blasts of `hits` damage cost a player with `armor`, one after another: in multiplayer armor takes up to
/// 0.8 of a blast, point for point.
pub(crate) fn health_loss(hits: &[f32], armor: f32) -> f32 {
    let mut armor = armor;
    hits.iter()
        .map(|&r| {
            let taken = (0.8 * r).min(armor);
            armor -= taken;
            r - taken
        })
        .sum()
}

/// The health the bot may lose to blasts of `hits` damage: the worst of them on its head.
fn worst_loss(hits: &mut [f32], armor: f32) -> f32 {
    if let Some(top) = hits.iter_mut().max_by(|a, b| a.total_cmp(b)) {
        *top *= HEAD;
    }
    health_loss(hits, armor)
}

/// The mines `start` sets off going off, and those they set off in turn (a blast of one damage kills a mine; walls not
/// reckoned).
fn chain_of(mines: &[Mine], start: usize, damage: f32) -> SmallVec<[usize; 16]> {
    let reach = blast_radius(damage) - 2.5;
    let mut chain: SmallVec<[usize; 16]> = SmallVec::new();
    chain.push(start);
    let mut k = 0;
    while k < chain.len() {
        let m = &mines[chain[k]];
        let at = burst_at(m.pos, m.dir, damage);
        for (j, other) in mines.iter().enumerate() {
            if !chain.contains(&j) && at.distance(other.pos) < reach {
                chain.push(j);
            }
        }
        k += 1;
    }
    chain
}

/// What `mines` going off together do to a player standing at `p` (walls not reckoned; measured to its centre, so no
/// more than it takes).
fn chain_damage(mines: &[(Vec3, Vec3)], p: Vec3, damage: f32) -> f32 {
    mines
        .iter()
        .map(|&(pos, dir)| blast_damage(damage, burst_at(pos, dir, damage).distance(p)))
        .sum()
}

/// Nothing stands between `a` and `b`.
fn clear(tracer: &mut dyn Tracer, a: Vec3, b: Vec3) -> bool {
    tracer.trace(&TraceQuery::line(a, b)).fraction >= 1.0
}

/// A mine at `pos` is in sight from `eye`.
fn in_sight(tracer: &mut dyn Tracer, eye: Vec3, pos: Vec3) -> bool {
    let tr = tracer.trace(&TraceQuery::line(eye, pos));
    tr.fraction >= 1.0 || tr.end.distance(pos) <= 8.0
}

/// How far `p` is from the segment `a → b`.
fn off_segment(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

/// The gun to set mines off with: loaded and allowed.
fn mine_gun(body: &Body) -> Option<WeaponId> {
    MINE_GUNS
        .into_iter()
        .find(|w| body.arsenal.iter().any(|a| a.id == *w && a.loaded()) && body.allows(*w))
}

/// How much of its health the bot gives to its own mines' blast: a wound, never its life (less where a suicide costs a
/// kill).
fn mine_spare(body: &Body) -> f32 {
    let most = if body.gungame.is_some_and(|g| g.descore()) {
        DESCORE_LOSS
    } else {
        SELF_LOSS
    };
    (body.health - SELF_LEFT).clamp(0.0, most)
}

fn mines_level(body: &Body) -> bool {
    body.gungame.is_some_and(|g| g.kit == Kit::Mines)
}

fn yaw_of(dir: Vec2) -> f32 {
    dmath::atan2(dir.y, dir.x).to_degrees()
}

/// A trail as it would lie along a lane, and where the bot would be after running on past it.
type Planned = (SmallVec<[(Vec3, Vec3); 10]>, Vec3);

/// A run along a lane: where it begins, the way it goes, how much of the lane is left and the seconds to its start.
type LaneRun = (Vec3, Vec2, f32, f32);

impl BotBrain {
    /// The trail's mines as the bot knows them now: moved to where they were seen, gone once forgotten.
    fn trail_upkeep(&mut self) {
        let Some(plan) = self.mind.arms.trail.as_mut() else {
            return;
        };
        let known = &self.explosives.mines;
        plan.mines.retain(
            |(p, _)| match known.iter().find(|m| m.own && m.pos.distance(*p) <= SAME_MINE) {
                Some(m) => {
                    *p = m.pos;
                    true
                }
                None => false,
            },
        );
    }

    /// Ten times a second while a trail is laid: drops the next mine as soon as the tripmine is ready, on a floor fit
    /// for one: not by a spawn point, a ladder, another mine or a player in sight, under a ceiling high enough for the
    /// blast, out of water.
    pub(crate) fn trail_drop(&mut self, body: &Body, nav: &mut dyn NavService) -> bool {
        let Some(plan) = self.mind.arms.trail.as_ref() else {
            return false;
        };
        if !plan.laying(body.now) {
            return false;
        }
        match self.drop_spot(body, nav) {
            Ok(heading) => {
                self.mind.arms.active = Some(Active::Drop(FloorPlanter::new(heading, body.now)));
                self.mind.arms.drop_why = "-";
                true
            }
            Err(why) => {
                self.mind.arms.drop_why = why;
                false
            }
        }
    }

    /// Whether the next mine of a trail being laid goes down ahead of the bot now, as the way it runs; or why not.
    fn drop_spot(&self, body: &Body, nav: &mut dyn NavService) -> Result<Vec2, &'static str> {
        let Some(plan) = self.mind.arms.trail.as_ref() else {
            return Err("no trail");
        };
        let speed = body.velocity.truncate().length();
        if self.mind.nav_mandatory {
            return Err("a jump or a ladder on the way");
        }
        if !body.on_ground || body.on_ladder || body.waterlevel > 0 {
            return Err("off the floor");
        }
        if carried(body, WeaponId::Tripmine) <= 0 {
            return Err("no mine left");
        }
        let heading = if plan.dir == Vec2::ZERO && speed > 0.0 {
            body.velocity.truncate() / speed
        } else {
            plan.dir
        };
        if body.velocity.truncate().dot(heading) < RUN_SPEED {
            return Err("not running along the way");
        }
        let view = Vec3::new(LAY_PITCH, yaw_of(heading), 0.0);
        let (floor, normal) = floor_spot(body.eye, view, nav).ok_or("no floor ahead")?;
        let mine = floor + normal * 8.0;
        if plan
            .mines
            .last()
            .is_some_and(|(last, _)| last.distance(mine) < TRAIL_GAP)
        {
            return Err("too close to the last one");
        }
        if self.spawns.iter().any(|s| s.distance(mine) < SPAWN_CLEAR) {
            return Err("by a spawn point");
        }
        let by_ladder =
            |l: &Vec3| (*l - mine).truncate().length() < LADDER_CLEAR && (l.z - mine.z).abs() < LADDER_HEIGHT;
        if self.ladders.iter().any(by_ladder) {
            return Err("by a ladder");
        }
        let in_trail = |p: Vec3| plan.mines.iter().any(|(q, _)| q.distance(p) <= SAME_MINE);
        if self
            .explosives
            .mines
            .iter()
            .any(|m| !in_trail(m.pos) && m.pos.distance(mine) <= MINE_CLEAR)
        {
            return Err("by another mine");
        }
        let in_the_way = self
            .beliefs
            .tracks
            .iter()
            .any(|t| t.state == TrackState::Visible && off_segment(t.pos, body.eye, floor) <= PLAYER_CLEAR);
        if in_the_way {
            return Err("a player in the way");
        }
        if !clear(nav, floor + Vec3::Z * 2.0, floor + Vec3::Z * HEADROOM) {
            return Err("a low ceiling");
        }
        if nav.point_contents(mine) != contents::EMPTY {
            return Err("in water");
        }
        Ok(heading)
    }

    /// A mine was dropped on the run: it joins the trail.
    pub(crate) fn trail_dropped(&mut self, pos: Vec3, dir: Vec3, now: SimTime) {
        tracing::debug!(
            "slot {} dropped a trail mine at {:.0} {:.0} {:.0}",
            self.slot,
            pos.x,
            pos.y,
            pos.z
        );
        self.explosives.placed_mine(pos, dir, now);
        if let Some(plan) = self.mind.arms.trail.as_mut() {
            plan.mines.push((pos, dir));
            plan.dropped += 1;
        }
    }

    /// Holds the run: the view down along `dir` at the lay pitch (a glance at a shot or a cry waits: the run is short),
    /// the tripmine in hand, and mines asked for a moment more.
    fn lay(&mut self, body: &Body, dir: Vec2) {
        if let Some(plan) = self.mind.arms.trail.as_mut() {
            plan.lay_until = body.now + WINDOW;
            plan.dir = dir;
        }
        self.intents.look(
            Prio::Protocol,
            LookIntent::Angles(Vec3::new(LAY_PITCH, yaw_of(dir), 0.0)),
        );
        if carried(body, WeaponId::Tripmine) > 0 {
            self.intents.weapon(Prio::Goal, WeaponIntent::hold(WeaponId::Tripmine));
        }
    }

    /// The health the bot may lose to `mines` going off, each blast with a clear line to it.
    fn chain_loss(&self, body: &Body, tracer: &mut dyn Tracer, mines: &[(Vec3, Vec3)]) -> f32 {
        let damage = body.damages.tripmine;
        let reach = blast_radius(damage);
        let aim = body.origin + Vec3::Z * BODY_AIM;
        let mut hits: SmallVec<[f32; 16]> = mines
            .iter()
            .filter_map(|&(pos, dir)| {
                let at = burst_at(pos, dir, damage);
                let d = box_distance(at, body.origin);
                if d >= reach {
                    return None;
                }
                let tr = tracer.trace(&TraceQuery::line(at, aim));
                (tr.fraction >= 1.0 || tr.start_solid).then(|| blast_damage(damage, d))
            })
            .collect();
        worst_loss(&mut hits, body.armor)
    }

    /// Ten times a second: shoots a mine the bot knows of when it is worth it and the whole chain it sets off costs
    /// the bot no more than a wound: an enemy by it, someone heard by the bot's own, a trail nobody came by after its
    /// watch, an old mine of its own on the tripmine level when short of mines, an enemy's mine in a calm bot's way.
    pub(crate) fn mine_shots(&mut self, body: &Body, nav: &mut dyn NavService) -> bool {
        let now = body.now;
        self.trail_upkeep();
        if self.mind.arms.trail.as_ref().is_some_and(|p| p.laying(now)) {
            return false;
        }
        // Stepping off a trail it does not stop for anything but an enemy by a mine.
        let stepping_off = matches!(&self.mind.task, Some(Task::Trail(t)) if t.phase == TrailPhase::Off);
        let Some(gun) = mine_gun(body) else {
            return false;
        };
        if self.explosives.mines.is_empty() {
            return false;
        }
        let damage = body.damages.tripmine;
        let mines: SmallVec<[Mine; 16]> = self.explosives.mines.iter().copied().collect();
        let spare = mine_spare(body);
        // In GunGame a mine's blast hurts others only for a player on the tripmine level.
        if body.gungame.is_none_or(|g| g.kit == Kit::Mines) {
            let enemies: SmallVec<[Vec3; 4]> = self
                .beliefs
                .enemies()
                .filter(|t| fresh(t, now))
                .map(|t| ahead(t, now, VICTIM_LEAD))
                .collect();
            for p in enemies {
                let Some(near) = mines
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| burst_at(m.pos, m.dir, damage).distance(p) < blast_radius(damage))
                    .min_by(|a, b| a.1.pos.distance(p).total_cmp(&b.1.pos.distance(p)))
                    .map(|(i, _)| i)
                else {
                    continue;
                };
                let chain = chain_of(&mines, near, damage);
                let blasts = blasts(&mines, &chain);
                if chain_damage(&blasts, p, damage) >= VICTIM
                    && self.shoot_chain(body, nav, &mines, &chain, p, spare, gun, "an enemy by it")
                {
                    return true;
                }
            }
            // Someone heard by its own mines (in team games, only a sound tied to an enemy).
            let friends = self.beliefs.tracks.iter().any(|t| t.relation != Relation::Enemy);
            let heard: SmallVec<[Vec3; 4]> = self
                .beliefs
                .hypotheses
                .iter()
                .filter(|h| {
                    now.since(h.t) <= HEARD_FOR
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
                        && (!friends
                            || h.track
                                .and_then(|k| self.beliefs.track(k))
                                .is_some_and(|t| t.relation == Relation::Enemy))
                })
                .filter_map(|h| h.pos)
                .collect();
            if stepping_off {
                return false;
            }
            for p in heard {
                let Some(i) = mines.iter().position(|m| m.own && m.pos.distance(p) <= HEARD_NEAR) else {
                    continue;
                };
                let chain = chain_of(&mines, i, damage);
                if self.shoot_chain(body, nav, &mines, &chain, p, spare, gun, "someone heard by it") {
                    return true;
                }
            }
        }
        if stepping_off {
            return false;
        }
        let calm = self.beliefs.visible_enemies().next().is_none();
        // A trail nobody came by after its watch: its mines come back as they go off.
        if calm
            && let Some(plan) = self.mind.arms.trail.as_ref()
            && plan.blow_at.is_some_and(|t| now >= t)
            && plan.blows < BLOWS
        {
            let mut chain: SmallVec<[usize; 16]> = SmallVec::new();
            for (p, _) in &plan.mines {
                if let Some(i) = mines.iter().position(|m| m.pos.distance(*p) <= SAME_MINE)
                    && !chain.contains(&i)
                {
                    for j in chain_of(&mines, i, damage) {
                        if !chain.contains(&j) {
                            chain.push(j);
                        }
                    }
                }
            }
            if !chain.is_empty()
                && self.shoot_chain(body, nav, &mines, &chain, body.origin, spare, gun, "nobody came by it")
            {
                if let Some(plan) = self.mind.arms.trail.as_mut() {
                    plan.blows += 1;
                }
                return true;
            }
        }
        // Short of mines on the tripmine level: an old one of its own goes off to come back.
        if calm && mines_level(body) && carried(body, WeaponId::Tripmine) < RECYCLE_BELOW {
            let in_trail = |m: &Mine| {
                self.mind
                    .arms
                    .trail
                    .as_ref()
                    .is_some_and(|p| p.mines.iter().any(|(q, _)| q.distance(m.pos) <= SAME_MINE))
            };
            let oldest = mines
                .iter()
                .enumerate()
                .filter(|(_, m)| m.own && now.since(m.armed_at) >= RECYCLE_AGE && !in_trail(m))
                .min_by(|a, b| a.1.armed_at.0.total_cmp(&b.1.armed_at.0))
                .map(|(i, _)| i);
            if let Some(i) = oldest {
                let chain = chain_of(&mines, i, damage);
                let at = mines[i].pos;
                if self.shoot_chain(
                    body,
                    nav,
                    &mines,
                    &chain,
                    at,
                    spare.min(RECYCLE_LOSS),
                    gun,
                    "to get it back",
                ) {
                    return true;
                }
            }
        }
        // An enemy's mine ahead of a calm bot is cleared from out of its blast.
        if self.calm_for(now) >= QUIET {
            let heading = body.velocity.truncate().normalize_or_zero();
            let ahead_of: SmallVec<[usize; 4]> = mines
                .iter()
                .enumerate()
                .filter(|(_, m)| !m.own && heading.dot((m.pos - body.origin).truncate().normalize_or_zero()) > 0.7)
                .map(|(i, _)| i)
                .collect();
            for i in ahead_of {
                let chain = chain_of(&mines, i, damage);
                let at = mines[i].pos;
                if self.shoot_chain(body, nav, &mines, &chain, at, 0.0, gun, "in the way") {
                    return true;
                }
            }
        }
        false
    }

    /// Shoots the mine of `chain` nearest `to` that is armed, in range and in sight, when the chain's blasts cost the
    /// bot no more than `spare` and a friend in sight next to nothing.
    #[allow(clippy::too_many_arguments)]
    fn shoot_chain(
        &mut self,
        body: &Body,
        nav: &mut dyn NavService,
        mines: &[Mine],
        chain: &[usize],
        to: Vec3,
        spare: f32,
        gun: WeaponId,
        why: &'static str,
    ) -> bool {
        let now = body.now;
        let damage = body.damages.tripmine;
        let blasts = blasts(mines, chain);
        let friend = self.beliefs.tracks.iter().any(|t| {
            t.relation != Relation::Enemy
                && t.state == TrackState::Visible
                && chain_damage(&blasts, t.pos, damage) >= FRIEND_SPARE
        });
        if friend {
            return false;
        }
        let mut order: SmallVec<[Vec3; 16]> = chain
            .iter()
            .map(|&i| &mines[i])
            .filter(|m| now >= m.armed_at && m.pos.distance(body.eye) <= SHOOT_RANGE)
            .map(|m| m.pos)
            .collect();
        if order.is_empty() {
            return false;
        }
        let loss = self.chain_loss(body, nav, &blasts);
        if loss > spare {
            return false;
        }
        order.sort_by(|a, b| a.distance(to).total_cmp(&b.distance(to)));
        let Some(target) = order
            .into_iter()
            .take(SIGHT_LINES)
            .find(|&p| in_sight(nav, body.eye, p))
        else {
            return false;
        };
        tracing::info!(
            "slot {} shoots a mine {why}: at {:.0} {:.0} {:.0}, {:.0} units off, {} mines in its chain, {loss:.0} of its \
             {:.0} health to lose",
            self.slot,
            target.x,
            target.y,
            target.z,
            target.distance(body.eye),
            chain.len(),
            body.health
        );
        let arms = &mut self.mind.arms;
        arms.shot_why = why;
        arms.shot_spare = spare;
        arms.shot_chain = blasts;
        arms.shot_check = now + SHOT_CHECK;
        arms.active = Some(Active::Shoot(MineShot::new(target, gun, now)));
        true
    }

    /// While a mine is shot: the chain it sets off still spares the bot (it may have moved, or mines come in).
    pub(crate) fn shot_spared(&mut self, body: &Body, nav: &mut dyn NavService) -> bool {
        self.mind.arms.shot_check = body.now + SHOT_CHECK;
        let chain = std::mem::take(&mut self.mind.arms.shot_chain);
        let spared = self.chain_loss(body, nav, &chain) <= self.mind.arms.shot_spare;
        self.mind.arms.shot_chain = chain;
        spared
    }

    /// The shot mine went off: the rest of its chain goes off over the next moments.
    pub(crate) fn chain_set_off(&mut self, body: &Body) {
        let damage = body.damages.tripmine;
        let bursts = self
            .mind
            .arms
            .shot_chain
            .iter()
            .map(|&(pos, dir)| burst_at(pos, dir, damage))
            .collect();
        self.mind.arms.blowing = Some((body.now + CHAIN_BURNS, bursts));
    }

    /// The blasts of a chain going off now.
    pub(crate) fn chain_blasts(&self, body: &Body) -> impl Iterator<Item = lb_knowledge::Blast> + '_ {
        let radius = blast_radius(body.damages.tripmine);
        let now = body.now;
        self.mind
            .arms
            .blowing
            .iter()
            .filter(move |(until, _)| now < *until)
            .flat_map(move |(_, bursts)| {
                bursts.iter().map(move |&at| lb_knowledge::Blast {
                    at,
                    radius,
                    kind: lb_game::entities::ProjectileKind::Tripmine,
                })
            })
    }

    /// The trail goal: to a lane, along it dropping mines, on past them and off to a place out of their blast, a
    /// moment's watch, and the trail set off (left lying out of GunGame).
    pub(crate) fn trail(
        &mut self,
        body: &Body,
        ch: &Character,
        map: &dyn MapView,
        nav: &mut dyn NavService,
        rng: &mut BotRng,
    ) {
        if !matches!(self.mind.task, Some(Task::Trail(_))) && !self.begin_trail(body, map, nav, rng) {
            self.trail_failed(body, rng);
            return;
        }
        self.trail_upkeep();
        // A phase over goes on into the next one in the same frame.
        for _ in 0..4 {
            let Some(Task::Trail(task)) = self.mind.task else {
                return;
            };
            // Gone with the rest of what its weapons were doing (a GunGame level changed).
            let Some(plan) = self.mind.arms.trail.clone() else {
                self.done();
                return;
            };
            if task.started.is_none() && plan.dropped > 0 {
                self.trail_task(|t| t.started = Some(body.now));
            }
            // Set off: by the bot, or by someone else.
            if matches!(task.phase, TrailPhase::Off | TrailPhase::Watch) && plan.mines.is_empty() {
                self.trail_done(body, ch, rng, false);
                return;
            }
            let next = match task.phase {
                TrailPhase::Approach => self.trail_approach(body, nav, &task, rng),
                TrailPhase::Run => self.trail_run(body, &task, &plan, rng),
                TrailPhase::Off => self.trail_off(body, ch, map, nav, &task, &plan, rng),
                TrailPhase::Watch => self.trail_watch(body, ch, &task, &plan, rng),
            };
            match next {
                Some(phase) => self.trail_phase(phase, body, rng),
                None => return,
            }
        }
    }

    /// A new trail: along a lane, with mines enough; short of them on the tripmine level, an old trail of the bot's
    /// own set off from where it is watched, to get its mines back.
    fn begin_trail(&mut self, body: &Body, map: &dyn MapView, nav: &mut dyn NavService, rng: &mut BotRng) -> bool {
        let now = body.now;
        if carried(body, WeaponId::Tripmine) >= TRAIL_LEAST {
            let Some((start, dir, length, eta)) = self.pick_lane(body, map, nav, rng) else {
                tracing::debug!("slot {}: no lane to lay a trail along", self.slot);
                return false;
            };
            tracing::debug!(
                "slot {}: trail along a lane from {:.0} {:.0} {:.0} toward {:.0}°, {length:.0} units, {eta:.1} s away",
                self.slot,
                start.x,
                start.y,
                start.z,
                yaw_of(dir)
            );
            let mut plan = TrailPlan::new(TRAIL_MINES);
            plan.dir = dir;
            self.mind.arms.trail = Some(plan);
            self.mind.arms.stats.trails += 1;
            self.mind.task = Some(Task::Trail(TrailTask {
                start,
                dir,
                length,
                phase: TrailPhase::Approach,
                since: now,
                started: None,
                reach_by: now + f64::from(eta) * 1.5 + APPROACH_SLACK,
                stand: None,
                straight: false,
                until: None,
            }));
            return true;
        }
        if !mines_level(body) {
            return false;
        }
        let Some(old) = self.old_trail(body) else {
            return false;
        };
        tracing::debug!(
            "slot {}: setting its old trail of {} mines off, to get them back",
            self.slot,
            old.len()
        );
        let mut plan = TrailPlan::new(old.len() as u32);
        plan.dropped = old.len() as u32;
        plan.mines = old;
        self.mind.arms.trail = Some(plan);
        self.mind.task = Some(Task::Trail(TrailTask {
            start: body.origin,
            dir: Vec2::ZERO,
            length: 0.0,
            phase: TrailPhase::Off,
            since: now,
            started: Some(now),
            reach_by: now,
            stand: None,
            straight: false,
            until: None,
        }));
        true
    }

    /// To the lane's start with the tripmine drawn; there, the run begins once it is in hand.
    fn trail_approach(
        &mut self,
        body: &Body,
        nav: &mut dyn NavService,
        task: &TrailTask,
        rng: &mut BotRng,
    ) -> Option<TrailPhase> {
        let now = body.now;
        if carried(body, WeaponId::Tripmine) > 0 {
            self.intents.weapon(Prio::Goal, WeaponIntent::hold(WeaponId::Tripmine));
        }
        let there = (body.origin - task.start).truncate().length() <= LANE_ENTER
            && (body.origin.z - task.start.z).abs() <= 48.0;
        if there {
            if body.weapon == Some(WeaponId::Tripmine) || now > task.reach_by + DRAW_WAIT {
                return Some(TrailPhase::Run);
            }
            self.stand_still();
            self.intents.look(
                Prio::Goal,
                LookIntent::Angles(Vec3::new(LAY_PITCH, yaw_of(task.dir), 0.0)),
            );
            return None;
        }
        if now > task.reach_by {
            tracing::debug!("slot {}: trail given up, its lane not reached in time", self.slot);
            self.trail_failed(body, rng);
            return None;
        }
        match self.walk(task.start, body, nav) {
            NavStatus::Moving => None,
            // The graph's way ends short of the start: the run begins from here, steering onto the lane.
            NavStatus::Arrived => Some(TrailPhase::Run),
            NavStatus::NoPath => {
                tracing::debug!("slot {}: trail given up, no way to its lane", self.slot);
                self.trail_failed(body, rng);
                None
            }
        }
    }

    /// Along the lane at full speed, the view down along it, a mine dropped as soon as the tripmine is ready (the
    /// arms drop them); on past the last one once all are down, the pocket is empty, the lane runs out, the run took
    /// too long or the bot had to get away from a blast.
    fn trail_run(&mut self, body: &Body, task: &TrailTask, plan: &TrailPlan, rng: &mut BotRng) -> Option<TrailPhase> {
        let now = body.now;
        let dropping = matches!(self.mind.arms.active, Some(Active::Drop(_)));
        let along = (body.origin - task.start).truncate().dot(task.dir);
        let over = plan.dropped >= plan.wanted
            || (carried(body, WeaponId::Tripmine) <= 0 && !dropping)
            || along >= task.length - RUN_OUT
            || now.since(task.since) > RUN_FOR;
        if over && !dropping {
            if plan.dropped == 0 {
                tracing::debug!(
                    "slot {}: trail given up, no mine down along its lane ({})",
                    self.slot,
                    self.mind.arms.drop_why
                );
                self.trail_failed(body, rng);
                return None;
            }
            return Some(TrailPhase::Off);
        }
        // Off the lane to get away from a blast: the trail ends where it got to.
        let dodging = self.mind.arms.dodge.is_some_and(|(until, _)| now < until);
        if dodging && plan.dropped > 0 && !dropping {
            return Some(TrailPhase::Off);
        }
        if plan.dropped == 0 && !dropping && now.since(task.since) > FIRST_MINE {
            tracing::debug!(
                "slot {}: trail given up, no mine down in time ({})",
                self.slot,
                self.mind.arms.drop_why
            );
            self.trail_failed(body, rng);
            return None;
        }
        self.run_along(body, task, along);
        self.lay(body, task.dir);
        None
    }

    /// Full speed along the lane, steering onto its line.
    fn run_along(&mut self, body: &Body, task: &TrailTask, along: f32) {
        let ahead = task.start + (task.dir * (along.max(0.0) + RUN_AHEAD)).extend(0.0);
        let dir = (ahead - body.origin).truncate().normalize_or(task.dir);
        self.intents.movement(
            Prio::Goal,
            MoveIntent {
                dir,
                speed: body.maxspeed,
            },
        );
    }

    /// On along the lane a moment past the last mine, then off to where the trail is watched from, the gun drawn.
    #[allow(clippy::too_many_arguments)]
    fn trail_off(
        &mut self,
        body: &Body,
        ch: &Character,
        map: &dyn MapView,
        nav: &mut dyn NavService,
        task: &TrailTask,
        plan: &TrailPlan,
        rng: &mut BotRng,
    ) -> Option<TrailPhase> {
        let now = body.now;
        if let Some(gun) = mine_gun(body) {
            self.intents.weapon(Prio::Goal, WeaponIntent::hold(gun));
        }
        if now.since(task.since) > OFF_FOR {
            if self.chain_loss(body, nav, &plan.mines) <= mine_spare(body) {
                return Some(TrailPhase::Watch);
            }
            tracing::debug!(
                "slot {}: its trail left lying, no place to watch it from reached",
                self.slot
            );
            self.trail_done(body, ch, rng, true);
            return None;
        }
        let along = (body.origin - task.start).truncate().dot(task.dir);
        if task.stand.is_none()
            && task.dir != Vec2::ZERO
            && now.since(task.since) < RUN_ON
            && along < task.length - LANE_ENTER
        {
            self.run_along(body, task, along);
            self.intents
                .look(Prio::Goal, LookIntent::Angles(Vec3::new(0.0, yaw_of(task.dir), 0.0)));
            return None;
        }
        let (stand, straight) = match task.stand {
            Some(s) => (s, task.straight),
            None => {
                let Some((s, straight)) = self.stand_spot(body, map, nav, plan, task.dir) else {
                    // Watched from here only out of its blast; else left lying, the bot going on its way.
                    let spared = self.chain_loss(body, nav, &plan.mines) <= mine_spare(body);
                    tracing::debug!(
                        "slot {}: no place by the graph to watch its trail from{}",
                        self.slot,
                        if spared { "" } else { ", and too close to it here" }
                    );
                    if !spared {
                        self.trail_done(body, ch, rng, true);
                        return None;
                    }
                    return Some(TrailPhase::Watch);
                };
                self.trail_task(|t| {
                    t.stand = Some(s);
                    t.straight = straight;
                });
                (s, straight)
            }
        };
        if at_spot(body, stand) {
            return Some(TrailPhase::Watch);
        }
        if straight {
            // Straight there at full speed, the way a player runs off.
            let dir = (stand - body.origin).truncate().normalize_or_zero();
            self.intents.movement(
                Prio::Goal,
                MoveIntent {
                    dir,
                    speed: body.maxspeed,
                },
            );
            self.intents
                .look(Prio::Goal, LookIntent::Angles(Vec3::new(0.0, yaw_of(dir), 0.0)));
            return None;
        }
        match self.walk(stand, body, nav) {
            NavStatus::Moving => None,
            _ => Some(TrailPhase::Watch),
        }
    }

    /// Standing, the gun on the trail: a shot at once at an enemy by it (the arms see to it); after a moment, on the
    /// tripmine level, the trail set off anyway; elsewhere left lying as a trap.
    fn trail_watch(
        &mut self,
        body: &Body,
        ch: &Character,
        task: &TrailTask,
        plan: &TrailPlan,
        rng: &mut BotRng,
    ) -> Option<TrailPhase> {
        let now = body.now;
        self.stand_still();
        self.watch_trail(body, plan);
        let until = task.until.unwrap_or(now);
        if now < until {
            return None;
        }
        if !mines_level(body) {
            self.trail_done(body, ch, rng, true);
        } else if plan.blow_at.is_none() {
            if let Some(p) = self.mind.arms.trail.as_mut() {
                p.blow_at = Some(now);
            }
        } else if now.since(until) > BLOW_WAIT {
            self.trail_done(body, ch, rng, true);
        }
        None
    }

    /// The trail goes on into `phase`; the watch is timed from when the last of its mines arms.
    fn trail_phase(&mut self, phase: TrailPhase, body: &Body, rng: &mut BotRng) {
        let now = body.now;
        let mut until = None;
        if phase == TrailPhase::Watch
            && let Some(plan) = &self.mind.arms.trail
        {
            let span = if mines_level(body) { MINES_LEVEL_WATCH } else { WATCH };
            let armed = self
                .explosives
                .mines
                .iter()
                .filter(|m| plan.mines.iter().any(|(p, _)| p.distance(m.pos) <= SAME_MINE))
                .map(|m| m.armed_at)
                .fold(now, |a, b| if b.0 > a.0 { b } else { a });
            let u = armed + f64::from(rng.decision.range_f32(span[0], span[1]));
            let nearest = plan
                .mines
                .iter()
                .map(|(m, _)| m.distance(body.origin))
                .fold(f32::INFINITY, f32::min);
            tracing::info!(
                "slot {}: trail of {} mines watched from {:.0} {:.0} {:.0}, the nearest {nearest:.0} units off, {:.1} s",
                self.slot,
                plan.mines.len(),
                body.origin.x,
                body.origin.y,
                body.origin.z,
                u.since(now)
            );
            until = Some(u);
        }
        self.trail_task(|t| {
            t.phase = phase;
            t.since = now;
            if until.is_some() {
                t.until = until;
            }
        });
    }

    /// A trail that came to nothing: on the tripmine level, where mines are all the bot has, the next one comes soon;
    /// elsewhere the trail is not picked again for a while.
    fn trail_failed(&mut self, body: &Body, rng: &mut BotRng) {
        if mines_level(body) {
            let rest = rng
                .decision
                .range_f32(MINES_LEVEL_GIVE_UP_REST[0], MINES_LEVEL_GIVE_UP_REST[1]);
            self.mind.trap_rest_until = body.now + f64::from(rest);
            self.mind.arms.trail = None;
            self.done();
        } else {
            self.mind.arms.trail = None;
            self.give_up(body.now, rng);
        }
    }

    fn trail_task(&mut self, f: impl FnOnce(&mut TrailTask)) {
        if let Some(Task::Trail(t)) = self.mind.task.as_mut() {
            f(t);
        }
    }

    /// Watching the trail: the gun to set it off with in hand, the view on the mine nearest an enemy believed about,
    /// else on the one it is to be set off by.
    fn watch_trail(&mut self, body: &Body, plan: &TrailPlan) {
        let mine = self
            .beliefs
            .enemies()
            .filter(|t| t.state != TrackState::Stale)
            .flat_map(|t| plan.mines.iter().map(move |(m, _)| (*m, m.distance(t.pos))))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(m, _)| m)
            .or_else(|| plan.target());
        if let Some(m) = mine {
            self.look_at(m + Vec3::Z * 8.0);
        }
        if let Some(gun) = mine_gun(body) {
            self.intents.weapon(Prio::Goal, WeaponIntent::hold(gun));
        }
    }

    /// The trail is over: set off, or left lying. On the tripmine level the next one comes at once; elsewhere the next
    /// trap waits a little.
    fn trail_done(&mut self, body: &Body, ch: &Character, rng: &mut BotRng, left: bool) {
        if let Some(plan) = &self.mind.arms.trail {
            tracing::info!(
                "slot {}: trail over, {} of {} mines dropped, {} lying{}",
                self.slot,
                plan.dropped,
                plan.wanted,
                plan.mines.len(),
                if left { ", left as a trap" } else { "" }
            );
        }
        if left {
            self.mind.arms.stats.trails_left += 1;
        }
        self.mind.stats.traps += 1;
        let rest = if mines_level(body) {
            MINES_LEVEL_REST
        } else if ch.affinity.trap >= FOND {
            FOND_TRAP_REST
        } else {
            TRAP_REST
        };
        self.mind.trap_rest_until = body.now + f64::from(rng.decision.range_f32(rest[0], rest[1]));
        self.mind.arms.trail = None;
        self.done();
    }

    /// The lane to lay the next trail along: the one the bot stands on (the run begins where it is), else the start of
    /// one soon reached by the graph; with enough of it left, not along a known mine; not toward an enemy in mind,
    /// rather in the open than down a corridor, along a busy way, with a little chance. Its start, way, length left and
    /// the seconds of running to it.
    fn pick_lane(&self, body: &Body, map: &dyn MapView, nav: &mut dyn NavService, rng: &mut BotRng) -> Option<LaneRun> {
        let lanes = map.lanes();
        if lanes.is_empty() {
            return None;
        }
        let time = map
            .nearest_node(body.origin, 256.0)
            .map(|from| lb_knowledge::travel_tree(map, from, LANE_REACH).0)
            .unwrap_or_default();
        let enemies: SmallVec<[Vec3; 8]> = self
            .beliefs
            .enemies()
            .filter(|t| t.state != TrackState::Stale)
            .map(|t| t.pos)
            .collect();
        let mut found: Vec<(LaneRun, f32, Planned)> = Vec::new();
        for l in lanes {
            let rel = (body.origin - l.start).truncate();
            let along = rel.dot(l.dir);
            let on = along >= 0.0
                && rel.perp_dot(l.dir).abs() <= ON_LANE
                && (body.origin.z - l.start.z).abs() <= 24.0
                && l.length - along >= LANE_NEED;
            let (start, length, eta) = if on {
                (l.start + (l.dir * along).extend(0.0), l.length - along, 0.0)
            } else {
                let Some(&t) = time.get(l.node as usize).filter(|t| t.is_finite()) else {
                    continue;
                };
                (l.start, l.length, t)
            };
            if length < LANE_NEED {
                continue;
            }
            let end = start + (l.dir * length.min(LANE_CLEAR)).extend(0.0);
            if self
                .explosives
                .mines
                .iter()
                .any(|m| off_segment(m.pos, start, end) < LANE_MINES)
            {
                continue;
            }
            // As it should lie, and a stride further on (the first mine late).
            let planned = |late: f32| -> Planned {
                let mines: SmallVec<[(Vec3, Vec3); 10]> = (0..TRAIL_MINES)
                    .map(|k| {
                        let at = start + (l.dir * (LAY_AHEAD + late + k as f32 * LAY_GAP)).extend(0.0);
                        (at - Vec3::Z * (BODY_HALF.z - 8.0), Vec3::Z)
                    })
                    .collect();
                let along = (mines[mines.len() - 1].0 - start).truncate().dot(l.dir);
                let from = start + (l.dir * (along + RUN_ON_FAR).min(length - LANE_ENTER)).extend(0.0);
                (mines, from)
            };
            let watched = |(mines, from): &Planned| {
                let last = mines[mines.len() - 1].0;
                watch_place(map, mines, last, *from, l.dir, &enemies, None, None).is_some()
            };
            let (on_time, late) = (planned(0.0), planned(LAY_GAP));
            if !watched(&on_time) || !watched(&late) {
                continue;
            }
            let mut cost = eta - LANE_FLOW * l.flow + rng.decision.range_f32(0.0, LANE_CHANCE);
            if l.room[0].max(l.room[1]) < CORRIDOR {
                cost += CORRIDOR_COST;
            }
            for e in &enemies {
                let to = (*e - start).truncate();
                if to.length() < ENEMY_NEAR && to.dot(l.dir) > 0.5 * to.length() {
                    cost += TOWARD_ENEMY;
                }
            }
            found.push(((start, l.dir, length, eta), cost, on_time));
        }
        found.sort_by(|a, b| a.1.total_cmp(&b.1));
        // The best few are looked at for a place to watch from that the bot walks to in a straight line.
        let walked = found.iter().take(LANE_TRIES).find(|(lane, _, (mines, from))| {
            let last = mines[mines.len() - 1].0;
            watch_place(map, mines, last, *from, lane.1, &enemies, None, Some((nav, true))).is_some()
        });
        walked.or(found.first()).map(|f| f.0)
    }

    /// Where to watch the trail from, stepping off it (see `watch_place`), not by an enemy in mind; one where its
    /// blast only wounds the bot as the last resort. Whether the bot runs there in a straight line.
    fn stand_spot(
        &self,
        body: &Body,
        map: &dyn MapView,
        nav: &mut dyn NavService,
        plan: &TrailPlan,
        dir: Vec2,
    ) -> Option<(Vec3, bool)> {
        let enemies: SmallVec<[Vec3; 8]> = self
            .beliefs
            .enemies()
            .filter(|t| t.state != TrackState::Stale)
            .map(|t| t.pos)
            .collect();
        let wound = (body.damages.tripmine, body.armor, mine_spare(body));
        watch_place(
            map,
            &plan.mines,
            plan.target()?,
            body.origin,
            dir,
            &enemies,
            Some(wound),
            Some((nav, false)),
        )
        .map(|(p, _, straight)| (p, straight))
    }

    /// The bot's own mines lying nearest it, and those that go off with them: an old trail to get back.
    fn old_trail(&self, body: &Body) -> Option<SmallVec<[(Vec3, Vec3); 10]>> {
        let own: SmallVec<[Mine; 16]> = self.explosives.mines.iter().filter(|m| m.own).copied().collect();
        let nearest = own
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.pos.distance(body.origin).total_cmp(&b.1.pos.distance(body.origin)))?
            .0;
        let chain = chain_of(&own, nearest, body.damages.tripmine);
        Some(chain.iter().take(10).map(|&i| (own[i].pos, own[i].dir)).collect())
    }
}

/// A place by the graph to watch a trail of `mines` from, for a bot at `from` that laid it along `dir` (zero for an
/// old trail) and is to set it off by `target`: in sight of the place by `target`, out of the blast of every mine of
/// it and no further than `STAND_BAND[1]` from the nearest, not passing a mine of it on the way (but the last one
/// dropped, run over before it armed). The nearest to the bot, one by an enemy in `avoid` costing more, and of the
/// best kind there is: 0 not back past the trail's last mine along `dir`, 1 back past it out of most of its blast, 2 (only
/// with `wound`: the damage of a mine, the bot's armor and the health it may give) where the blast only wounds it.
/// With a `walk` tracer, one of the nearest few of each kind walked to in a straight line (no jump or detour on the
/// way) comes first, and back past the trail or in its blast only such a one will do; with `straight`, ahead of it
/// too. Returns the place, its kind and whether it is walked to in a straight line.
#[allow(clippy::too_many_arguments)]
fn watch_place(
    map: &dyn MapView,
    mines: &[(Vec3, Vec3)],
    target: Vec3,
    from: Vec3,
    dir: Vec2,
    avoid: &[Vec3],
    wound: Option<(f32, f32, f32)>,
    walk: Option<(&mut dyn Tracer, bool)>,
) -> Option<(Vec3, u8, bool)> {
    let last = mines.last().map(|(m, _)| *m)?;
    let at = map.nearest_node(target, 256.0)?;
    let mut found: Vec<(Vec3, usize, f32)> = Vec::new();
    map.for_each_visible(at, &mut |n| {
        if map.transit(n) {
            return;
        }
        let p = map.node_origin(n);
        let clear = mines
            .iter()
            .filter(|(m, _)| dir == Vec2::ZERO || *m != last)
            .map(|(m, _)| off_segment(*m, from, p))
            .fold(f32::INFINITY, f32::min);
        if clear < STAND_PASS {
            return;
        }
        let off = mines.iter().map(|(m, _)| m.distance(p)).fold(f32::INFINITY, f32::min);
        let ahead = dir == Vec2::ZERO || (p - last).truncate().dot(dir) >= -STAND_BEHIND;
        let level = if (STAND_BAND[0]..=STAND_BAND[1]).contains(&off) && ahead {
            0
        } else if (STAND_BAND[0]..=STAND_BAND[1]).contains(&off) && clear >= STAND_PASS_BACK {
            1
        } else if let Some((damage, armor, spare)) = wound
            && (WOUND_BAND[0]..=WOUND_BAND[1]).contains(&off)
        {
            let mut hits: SmallVec<[f32; 10]> = mines
                .iter()
                .map(|&(q, d)| blast_damage(damage, box_distance(burst_at(q, d, damage), p)))
                .collect();
            if worst_loss(&mut hits, armor) > spare {
                return;
            }
            2
        } else {
            return;
        };
        let mut cost = p.distance(from);
        if avoid.iter().any(|e| e.distance(p) < STAND_ENEMY) {
            cost += STAND_ENEMY_COST;
        }
        found.push((p, level, cost));
    });
    found.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.total_cmp(&b.2)));
    let Some((tracer, straight)) = walk else {
        return found.first().map(|&(p, level, _)| (p, level as u8, false));
    };
    let up = Vec3::Z * STEP_UP;
    for level in 0..3 {
        let walked = found
            .iter()
            .filter(|(_, l, _)| *l == level)
            .take(STRAIGHT_TRIES)
            .find(|(p, _, _)| {
                let tr = tracer.trace(&TraceQuery::hull(from + up, *p + up, HullKind::Stand));
                !tr.start_solid && tr.fraction >= 1.0
            });
        if let Some(&(p, _, _)) = walked {
            return Some((p, level as u8, true));
        }
        // Ahead of the trail the graph's way will do; back past it only a straight run keeps clear of it.
        if level == 0
            && !straight
            && let Some(&(p, _, _)) = found.iter().find(|(_, l, _)| *l == 0)
        {
            return Some((p, 0, false));
        }
    }
    None
}

/// The blasts of `chain`'s mines: where each is and the way it faces.
fn blasts(mines: &[Mine], chain: &[usize]) -> SmallVec<[(Vec3, Vec3); 16]> {
    chain.iter().map(|&i| (mines[i].pos, mines[i].dir)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arms::tests::{Open, body, character, params, seen};
    use lb_combat::Armed;
    use lb_core::rng::Pcg32;
    use lb_game::gungame::GunGame;
    use lb_knowledge::Hypothesis;
    use lb_nav_api::{CampSpot, Lane, MineSpot, NavStep, NodeId};
    use lb_worldq::Trace;

    fn brain() -> BotBrain {
        BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill))
    }

    /// Carrying `mines` tripmines and a loaded glock, the tripmine in hand; on the tripmine level when `level`.
    fn miner_with(t: f64, level: bool, mines: i32) -> Body {
        let mut b = body(t);
        b.arsenal
            .retain(|a| a.id != WeaponId::HandGrenade && a.id != WeaponId::Glock);
        b.arsenal.push(Armed::new(WeaponId::Tripmine, None, Some(mines)));
        b.arsenal.push(Armed::new(WeaponId::Glock, Some(17), Some(68)));
        b.weapon = Some(WeaponId::Tripmine);
        if level {
            b.gungame = Some(GunGame::drill(1, Kit::Mines));
            b.allowed = Kit::Mines.weapons();
        }
        b
    }

    fn miner(t: f64, level: bool) -> Body {
        miner_with(t, level, 5)
    }

    /// Running along +x at `x`.
    fn running(t: f64, x: f32) -> Body {
        let mut b = miner(t, false);
        b.origin.x = x;
        b.eye.x = x;
        b.velocity = Vec3::new(270.0, 0.0, 0.0);
        b
    }

    /// A trail of floor mines (z = -28) laid along +x at `xs`, armed.
    fn trail_at(brain: &mut BotBrain, xs: &[f32]) {
        let mut plan = TrailPlan::new(xs.len() as u32);
        plan.dir = Vec2::X;
        for &x in xs {
            let pos = Vec3::new(x, 0.0, -28.0);
            brain.explosives.placed_mine(pos, Vec3::Z, SimTime(0.0));
            plan.mines.push((pos, Vec3::Z));
            plan.dropped += 1;
        }
        brain.mind.arms.trail = Some(plan);
    }

    fn task(phase: TrailPhase, since: f64) -> Task {
        Task::Trail(TrailTask {
            start: Vec3::ZERO,
            dir: Vec2::X,
            length: 1024.0,
            phase,
            since: SimTime(since),
            started: Some(SimTime(0.0)),
            reach_by: SimTime(since),
            stand: None,
            straight: false,
            until: None,
        })
    }

    fn phase(brain: &BotBrain) -> Option<TrailPhase> {
        match &brain.mind.task {
            Some(Task::Trail(t)) => Some(t.phase),
            _ => None,
        }
    }

    /// Open floor at z = -36 under a ceiling at z = 40: too low for a floor mine's blast.
    struct Low;

    impl Tracer for Low {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            if q.end.z > 40.0 && q.start.z <= 40.0 {
                let f = (40.0 - q.start.z) / (q.end.z - q.start.z);
                let mut t = Trace::clear(q.start + (q.end - q.start) * f);
                t.fraction = f;
                t.normal = Vec3::NEG_Z;
                return t;
            }
            Open.trace(q)
        }

        fn point_contents(&mut self, p: Vec3) -> i32 {
            Open.point_contents(p)
        }
    }

    impl NavService for Low {
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
    }

    const YARD_X: u32 = 21;
    const YARD_Y: u32 = 13;
    const YARD_MID: u32 = YARD_Y / 2;

    /// An open yard: places every 128 units over x = 0..2560, y = -768..768 (player origins at z = 0 over the floor
    /// at -36), all in sight of each other, and lanes along it.
    struct Yard(Vec<Lane>);

    impl Yard {
        /// The lane along +x from its west end.
        fn new() -> Yard {
            Yard(vec![Yard::lane(0, 1024.0, Vec2::X)])
        }

        /// A lane from the place `x` along the yard's middle row, `length` long.
        fn lane(x: u32, length: f32, dir: Vec2) -> Lane {
            Lane {
                node: YARD_MID * YARD_X + x,
                start: Vec3::new(x as f32 * 128.0, 0.0, 0.0),
                dir,
                length,
                room: [256.0; 2],
                flow: 0.5,
            }
        }
    }

    impl MapView for Yard {
        fn node_count(&self) -> usize {
            (YARD_X * YARD_Y) as usize
        }
        fn node_origin(&self, n: NodeId) -> Vec3 {
            Vec3::new(
                (n % YARD_X) as f32 * 128.0,
                ((n / YARD_X) as f32 - YARD_MID as f32) * 128.0,
                0.0,
            )
        }
        fn nearest_node(&self, p: Vec3, max: f32) -> Option<NodeId> {
            (0..self.node_count() as NodeId)
                .map(|n| (n, self.node_origin(n).distance(p)))
                .filter(|(_, d)| *d <= max)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(n, _)| n)
        }
        fn for_each_link(&self, n: NodeId, f: &mut dyn FnMut(NodeId, f32)) {
            let (x, y) = (n % YARD_X, n / YARD_X);
            if x > 0 {
                f(n - 1, 0.5);
            }
            if x + 1 < YARD_X {
                f(n + 1, 0.5);
            }
            if y > 0 {
                f(n - YARD_X, 0.5);
            }
            if y + 1 < YARD_Y {
                f(n + YARD_X, 0.5);
            }
        }
        fn visible(&self, _a: NodeId, _b: NodeId) -> bool {
            true
        }
        fn for_each_visible(&self, n: NodeId, f: &mut dyn FnMut(NodeId)) {
            for m in 0..self.node_count() as NodeId {
                if m != n {
                    f(m);
                }
            }
        }
        fn flow(&self, _n: NodeId) -> f32 {
            0.5
        }
        fn exposure(&self, _n: NodeId) -> f32 {
            0.0
        }
        fn transit(&self, _n: NodeId) -> bool {
            false
        }
        fn danger(&self, _n: NodeId) -> f32 {
            0.0
        }
        fn danger_from(&self, _n: NodeId) -> Option<NodeId> {
            None
        }
        fn camp_spots(&self) -> &[CampSpot] {
            &[]
        }
        fn mine_spots(&self) -> &[MineSpot] {
            &[]
        }
        fn chokepoints(&self) -> &[NodeId] {
            &[]
        }
        fn lanes(&self) -> &[Lane] {
            &self.0
        }
    }

    /// A hall along x = 0..1280 with places every 128 units (no way round anything in it), all in sight of each
    /// other, and lanes along it.
    struct Hall(Vec<Lane>);

    impl MapView for Hall {
        fn node_count(&self) -> usize {
            11
        }
        fn node_origin(&self, n: NodeId) -> Vec3 {
            Vec3::new(n as f32 * 128.0, 0.0, 0.0)
        }
        fn nearest_node(&self, p: Vec3, max: f32) -> Option<NodeId> {
            let n = (p.x / 128.0).round().clamp(0.0, 10.0) as NodeId;
            (self.node_origin(n).distance(p) <= max).then_some(n)
        }
        fn for_each_link(&self, n: NodeId, f: &mut dyn FnMut(NodeId, f32)) {
            if n > 0 {
                f(n - 1, 0.5);
            }
            if n < 10 {
                f(n + 1, 0.5);
            }
        }
        fn visible(&self, _a: NodeId, _b: NodeId) -> bool {
            true
        }
        fn for_each_visible(&self, n: NodeId, f: &mut dyn FnMut(NodeId)) {
            for m in (0..11).filter(|m| *m != n) {
                f(m);
            }
        }
        fn flow(&self, _n: NodeId) -> f32 {
            0.5
        }
        fn exposure(&self, _n: NodeId) -> f32 {
            0.0
        }
        fn transit(&self, _n: NodeId) -> bool {
            false
        }
        fn danger(&self, _n: NodeId) -> f32 {
            0.0
        }
        fn danger_from(&self, _n: NodeId) -> Option<NodeId> {
            None
        }
        fn camp_spots(&self) -> &[CampSpot] {
            &[]
        }
        fn mine_spots(&self) -> &[MineSpot] {
            &[]
        }
        fn chokepoints(&self) -> &[NodeId] {
            &[]
        }
        fn lanes(&self) -> &[Lane] {
            &self.0
        }
    }

    #[test]
    fn a_lane_into_a_dead_end_is_passed_over() {
        let lane = |x: f32, length: f32| Lane {
            node: (x / 128.0) as NodeId,
            start: Vec3::new(x, 0.0, 0.0),
            dir: Vec2::X,
            length,
            room: [256.0; 2],
            flow: 0.5,
        };
        // The bot stands on the second lane; laid along it, its trail would lie between it and the only way back.
        let hall = Hall(vec![lane(0.0, 1024.0), lane(640.0, 640.0)]);
        let mut b = miner(1.0, true);
        b.origin = Vec3::new(640.0, 0.0, 0.0);
        b.eye = b.origin + Vec3::Z * 28.0;
        let picked = brain().pick_lane(&b, &hall, &mut Open, &mut BotRng::new(3, 3));
        assert_eq!(
            picked.map(|l| l.0),
            Some(Vec3::ZERO),
            "the one with room to step off ahead"
        );
        assert_eq!(
            brain().pick_lane(&b, &Hall(vec![lane(640.0, 640.0)]), &mut Open, &mut BotRng::new(3, 3)),
            None
        );
    }

    #[test]
    fn mines_go_down_ahead_of_the_run_as_fast_as_the_tripmine_allows() {
        let mut brain = brain();
        let mut plan = TrailPlan::new(5);
        plan.dir = Vec2::X;
        brain.mind.arms.trail = Some(plan);
        let mut drops = Vec::new();
        for i in 0..60 {
            let t = f64::from(i) * 0.1;
            let b = running(t, 27.0 * i as f32);
            brain.mind.arms.trail.as_mut().unwrap().lay_until = SimTime(t + WINDOW);
            if brain.trail_drop(&b, &mut Open) {
                let Some(Active::Drop(p)) = brain.mind.arms.active.take() else {
                    panic!("a drop")
                };
                let view = Vec3::new(LAY_PITCH, yaw_of(p.heading), 0.0);
                let (floor, normal) = floor_spot(b.eye, view, &mut Open).unwrap();
                assert!((60.0..90.0).contains(&(floor.x - b.origin.x)), "ahead of the bot");
                brain.trail_dropped(floor + normal * 8.0, normal, SimTime(t));
                drops.push(floor.x);
            }
        }
        assert_eq!(drops.len(), 5, "as many as wanted: {drops:?}");
        assert!(
            drops.windows(2).all(|w| (64.0..100.0).contains(&(w[1] - w[0]))),
            "{drops:?}"
        );
        assert!(!brain.mind.arms.trail.as_ref().unwrap().laying(SimTime(6.0)));
    }

    #[test]
    fn no_mine_drops_by_a_spawn_point_a_ladder_another_mine_or_under_a_low_ceiling() {
        let drops = |brain: &mut BotBrain, nav: &mut dyn NavService| {
            if brain.mind.arms.trail.is_none() {
                brain.mind.arms.trail = Some(TrailPlan::new(5));
            }
            let plan = brain.mind.arms.trail.as_mut().unwrap();
            plan.dir = Vec2::X;
            plan.lay_until = SimTime(1.3);
            brain.trail_drop(&running(1.0, 0.0), nav)
        };
        assert!(drops(&mut brain(), &mut Open));
        let mut by_spawn = brain();
        by_spawn.spawns.push(Vec3::new(100.0, 0.0, -36.0));
        assert!(!drops(&mut by_spawn, &mut Open));
        let mut by_ladder = brain();
        by_ladder.ladders.push(Vec3::new(60.0, 40.0, 100.0));
        assert!(!drops(&mut by_ladder, &mut Open));
        by_ladder.ladders[0].z = 400.0;
        assert!(drops(&mut by_ladder, &mut Open), "one going up from a floor far above");
        let mut by_mine = brain();
        by_mine
            .explosives
            .placed_mine(Vec3::new(120.0, 0.0, -28.0), Vec3::Z, SimTime(0.0));
        assert!(!drops(&mut by_mine, &mut Open), "by someone else's mine");
        // The trail's own last one may lie closer, behind.
        let mut on = brain();
        trail_at(&mut on, &[0.0]);
        on.mind.arms.trail.as_mut().unwrap().wanted = 5;
        assert!(drops(&mut on, &mut Open), "the trail's own mine behind");
        assert!(!drops(&mut brain(), &mut Low), "its blast would go into the ceiling");
        // Pushed back off the way (getting away from a blast), none either.
        let mut back = brain();
        let mut plan = TrailPlan::new(5);
        plan.dir = Vec2::X;
        plan.lay_until = SimTime(1.3);
        back.mind.arms.trail = Some(plan);
        let mut b = running(1.0, 0.0);
        b.velocity = Vec3::new(-270.0, 0.0, 0.0);
        assert!(!back.trail_drop(&b, &mut Open));
        assert_eq!(back.mind.arms.drop_why, "not running along the way");
    }

    #[test]
    fn getting_away_from_a_blast_ends_the_run_where_it_got_to() {
        let mut brain = brain();
        trail_at(&mut brain, &[80.0, 160.0]);
        brain.mind.arms.trail.as_mut().unwrap().wanted = 5;
        brain.mind.task = Some(task(TrailPhase::Run, 0.0));
        brain.mind.arms.dodge = Some((SimTime(1.5), -Vec2::X));
        let mut rng = BotRng::new(3, 3);
        let mut b = running(1.0, 200.0);
        b.gungame = Some(GunGame::drill(1, Kit::Mines));
        brain.trail(&b, &character(), &Yard::new(), &mut Open, &mut rng);
        assert_eq!(phase(&brain), Some(TrailPhase::Off));
        assert!(!brain.mind.arms.trail.as_ref().unwrap().laying(SimTime(1.1)));
    }

    #[test]
    fn the_run_begins_where_the_bot_stands_on_a_lane_at_full_speed_the_view_down_along_it() {
        let mut brain = brain();
        let mut b = miner(1.0, true);
        b.origin = Vec3::new(100.0, 10.0, 0.0);
        b.eye = b.origin + Vec3::Z * 28.0;
        let mut rng = BotRng::new(3, 3);
        brain.trail(&b, &character(), &Yard::new(), &mut Open, &mut rng);
        assert_eq!(phase(&brain), Some(TrailPhase::Run));
        let Some(Task::Trail(t)) = brain.mind.task else {
            panic!()
        };
        assert!(t.start.distance(Vec3::new(100.0, 0.0, 0.0)) < 1.0, "{:?}", t.start);
        let (_, mv) = brain.intents.movement.expect("running");
        assert!(mv.dir.x > 0.99 && mv.speed == b.maxspeed, "{mv:?}");
        assert_eq!(
            brain.intents.look,
            Some((Prio::Protocol, LookIntent::Angles(Vec3::new(LAY_PITCH, 0.0, 0.0))))
        );
        assert!(brain.mind.arms.trail.as_ref().unwrap().laying(SimTime(1.1)));
        // With another weapon in hand it waits at the start for the tripmine.
        let mut brain = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        b.weapon = Some(WeaponId::Glock);
        brain.trail(&b, &character(), &Yard::new(), &mut Open, &mut rng);
        assert_eq!(phase(&brain), Some(TrailPhase::Approach));
        assert_eq!(
            brain.intents.weapon.map(|(_, w)| w),
            Some(WeaponIntent::hold(WeaponId::Tripmine))
        );
    }

    #[test]
    fn a_lane_toward_an_enemy_or_past_a_mine_is_passed_over() {
        let mut rng = BotRng::new(3, 3);
        let mut b = miner(5.0, true);
        b.origin = Vec3::new(640.0, 0.0, 0.0);
        b.eye = b.origin + Vec3::Z * 28.0;
        // The bot stands on both, in the middle of the yard.
        let yard = Yard(vec![Yard::lane(4, 768.0, Vec2::X), Yard::lane(6, 768.0, -Vec2::X)]);
        let way = |brain: &BotBrain, rng: &mut BotRng| brain.pick_lane(&b, &yard, &mut Open, rng).map(|l| l.1);
        let mut toward = brain();
        toward.beliefs.on_sighting(&seen(5.0, Vec3::new(1100.0, 0.0, 0.0)));
        toward.update(SimTime(5.0), &params(), None, None);
        assert_eq!(way(&toward, &mut rng), Some(-Vec2::X), "away from the enemy");
        let mut mined = brain();
        mined
            .explosives
            .placed_mine(Vec3::new(300.0, 20.0, -28.0), Vec3::Z, SimTime(0.0));
        assert_eq!(way(&mined, &mut rng), Some(Vec2::X), "not along the mine");
    }

    #[test]
    fn laid_it_runs_on_then_steps_off_ahead_out_of_the_blast_and_watches_the_last_mine() {
        let mut brain = brain();
        trail_at(&mut brain, &[80.0, 160.0, 240.0, 320.0, 400.0]);
        brain.mind.task = Some(task(TrailPhase::Run, 0.0));
        let mut rng = BotRng::new(3, 3);
        let yard = Yard::new();
        let at = |t: f64, x: f32| {
            let mut b = miner_with(t, true, 0);
            b.origin = Vec3::new(x, 0.0, 0.0);
            b.eye = b.origin + Vec3::Z * 28.0;
            b.velocity = Vec3::new(270.0, 0.0, 0.0);
            b.weapon = Some(WeaponId::Glock);
            b
        };
        // All down: on along the lane, the gun drawn.
        brain.trail(&at(1.5, 420.0), &character(), &yard, &mut Open, &mut rng);
        assert_eq!(phase(&brain), Some(TrailPhase::Off));
        let (_, mv) = brain.intents.movement.expect("running on");
        assert!(mv.dir.x > 0.9, "{mv:?}");
        assert_eq!(
            brain.intents.weapon.map(|(_, w)| w),
            Some(WeaponIntent::hold(WeaponId::Glock))
        );
        // Then off to a place out of the blast, never back past the trail.
        brain.intents.clear();
        brain.trail(&at(2.6, 600.0), &character(), &yard, &mut Open, &mut rng);
        let Some(Task::Trail(t)) = brain.mind.task else {
            panic!()
        };
        let stand = t.stand.expect("a place to watch from");
        let off = [80.0, 160.0, 240.0, 320.0, 400.0]
            .iter()
            .map(|x| stand.distance(Vec3::new(*x, 0.0, -28.0)))
            .fold(f32::INFINITY, f32::min);
        assert!((STAND_BAND[0]..=STAND_BAND[1]).contains(&off), "{stand:?} {off}");
        assert!(stand.x >= 400.0 - STAND_BEHIND, "{stand:?}");
        // There: the view on the last mine, a moment's watch (its mines armed already).
        brain.intents.clear();
        let mut there = at(3.0, stand.x);
        there.origin = stand;
        there.eye = stand + Vec3::Z * 28.0;
        there.velocity = Vec3::ZERO;
        brain.trail(&there, &character(), &yard, &mut Open, &mut rng);
        assert_eq!(phase(&brain), Some(TrailPhase::Watch));
        let Some(Task::Trail(t)) = brain.mind.task else {
            panic!()
        };
        let until = t.until.unwrap();
        assert!((0.5..=1.5).contains(&until.since(SimTime(3.0))), "{until:?}");
        assert_eq!(
            brain.intents.look,
            Some((
                Prio::Goal,
                LookIntent::Point {
                    at: Vec3::new(400.0, 0.0, -20.0),
                    engaged: false
                }
            ))
        );
        // The watch over with nobody by it, on the tripmine level: shot from where it stands.
        let late = SimTime(until.0 + 0.1);
        let mut b = there.clone();
        b.now = late;
        brain.trail(&b, &character(), &yard, &mut Open, &mut rng);
        assert!(brain.mine_shots(&b, &mut Open));
        assert_eq!(brain.mind.arms.shot_why, "nobody came by it");
        let Some(Active::Shoot(shot)) = &brain.mind.arms.active else {
            panic!("a shot")
        };
        assert_eq!(
            shot.mine,
            Vec3::new(400.0, 0.0, -28.0),
            "the nearest, the last one dropped"
        );
    }

    /// Places all in sight of each other, no ways between them.
    struct Spots(Vec<Vec3>);

    impl MapView for Spots {
        fn node_count(&self) -> usize {
            self.0.len()
        }
        fn node_origin(&self, n: NodeId) -> Vec3 {
            self.0[n as usize]
        }
        fn nearest_node(&self, p: Vec3, max: f32) -> Option<NodeId> {
            (0..self.0.len() as NodeId)
                .map(|n| (n, self.0[n as usize].distance(p)))
                .filter(|(_, d)| *d <= max)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(n, _)| n)
        }
        fn for_each_link(&self, _n: NodeId, _f: &mut dyn FnMut(NodeId, f32)) {}
        fn visible(&self, _a: NodeId, _b: NodeId) -> bool {
            true
        }
        fn for_each_visible(&self, n: NodeId, f: &mut dyn FnMut(NodeId)) {
            for m in (0..self.0.len() as NodeId).filter(|m| *m != n) {
                f(m);
            }
        }
        fn flow(&self, _n: NodeId) -> f32 {
            0.5
        }
        fn exposure(&self, _n: NodeId) -> f32 {
            0.0
        }
        fn transit(&self, _n: NodeId) -> bool {
            false
        }
        fn danger(&self, _n: NodeId) -> f32 {
            0.0
        }
        fn danger_from(&self, _n: NodeId) -> Option<NodeId> {
            None
        }
        fn camp_spots(&self) -> &[CampSpot] {
            &[]
        }
        fn mine_spots(&self) -> &[MineSpot] {
            &[]
        }
        fn chokepoints(&self) -> &[NodeId] {
            &[]
        }
    }

    #[test]
    fn a_trail_is_watched_from_ahead_or_back_past_it_only_out_of_its_blast() {
        // A trail along +x at x = 880..1200, the bot just past its end.
        let mines: Vec<(Vec3, Vec3)> = [880.0, 960.0, 1040.0, 1120.0, 1200.0]
            .iter()
            .map(|x| (Vec3::new(*x, 0.0, -28.0), Vec3::Z))
            .collect();
        let last = mines[4].0;
        let from = Vec3::new(1250.0, 0.0, 0.0);
        let by_last = Vec3::new(1200.0, 0.0, 0.0);
        let place = |spots: Vec<Vec3>, wound: Option<(f32, f32, f32)>| {
            let mut all = vec![by_last];
            all.extend(spots);
            watch_place(
                &Spots(all),
                &mines,
                last,
                from,
                Vec2::X,
                &[],
                wound,
                Some((&mut Open, false)),
            )
        };
        // Aside, ahead of its end.
        let aside = Vec3::new(1300.0, 500.0, 0.0);
        assert_eq!(place(vec![aside], None), Some((aside, 0, true)));
        // Back past it close by its line: not even out of reach of its blast.
        assert_eq!(place(vec![Vec3::new(300.0, 120.0, 0.0)], None), None);
        // Back past it, the way there out of most of its blast.
        let wide = Vec3::new(700.0, 520.0, 0.0);
        let far_from = Vec3::new(1250.0, 300.0, 0.0);
        let found = watch_place(
            &Spots(vec![by_last, wide]),
            &mines,
            last,
            far_from,
            Vec2::X,
            &[],
            None,
            Some((&mut Open, false)),
        );
        assert_eq!(found, Some((wide, 1, true)));
        // Where the blast only wounds it, the last resort.
        let near = Vec3::new(1330.0, 330.0, 0.0);
        assert_eq!(place(vec![near], None), None);
        assert_eq!(place(vec![near], Some((150.0, 0.0, 60.0))), Some((near, 2, true)));
    }

    #[test]
    fn short_of_mines_on_the_tripmine_level_its_old_trail_is_set_off_first() {
        let mut brain = brain();
        for x in [600.0, 680.0, 760.0] {
            brain
                .explosives
                .placed_mine(Vec3::new(x, 0.0, -28.0), Vec3::Z, SimTime(0.0));
        }
        let mut rng = BotRng::new(3, 3);
        let b = miner_with(10.0, true, 1);
        brain.trail(&b, &character(), &Yard::new(), &mut Open, &mut rng);
        let plan = brain.mind.arms.trail.as_ref().expect("its old trail");
        assert_eq!(plan.mines.len(), 3);
        assert_eq!(plan.target(), Some(Vec3::new(600.0, 0.0, -28.0)), "the nearest");
        let Some(Task::Trail(t)) = brain.mind.task else {
            panic!()
        };
        let stand = t.stand.unwrap_or(b.origin);
        assert!(
            stand.distance(Vec3::new(600.0, 0.0, -28.0)) >= STAND_BAND[0],
            "{stand:?}"
        );
        // Out of GunGame it is not.
        let mut dm = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        dm.explosives
            .placed_mine(Vec3::new(600.0, 0.0, -28.0), Vec3::Z, SimTime(0.0));
        dm.trail(
            &miner_with(10.0, false, 1),
            &character(),
            &Yard::new(),
            &mut Open,
            &mut rng,
        );
        assert!(dm.mind.arms.trail.is_none() && dm.mind.task.is_none());
    }

    #[test]
    fn an_enemy_by_a_trail_gets_the_mine_nearest_it_shot_from_out_of_the_blast() {
        let mut brain = brain();
        trail_at(&mut brain, &[500.0, 640.0, 780.0]);
        brain.beliefs.on_sighting(&seen(5.0, Vec3::new(800.0, 60.0, 0.0)));
        brain.update(SimTime(5.0), &params(), None, None);
        assert!(brain.mine_shots(&miner(5.0, false), &mut Open));
        let Some(Active::Shoot(shot)) = &brain.mind.arms.active else {
            panic!("a shot")
        };
        assert_eq!(shot.mine, Vec3::new(780.0, 0.0, -28.0), "the one by the enemy");
        assert_eq!(brain.mind.arms.shot_why, "an enemy by it");
        // Standing in the middle of the trail the chain would kill the bot: no shot.
        let mut close = BotBrain::new(1, lb_perception::PerceptionParams::from_skill(&character().skill));
        trail_at(&mut close, &[500.0, 640.0, 780.0]);
        close.beliefs.on_sighting(&seen(5.0, Vec3::new(800.0, 60.0, 0.0)));
        close.update(SimTime(5.0), &params(), None, None);
        let mut inside = miner(5.0, false);
        inside.origin.x = 560.0;
        inside.eye.x = 560.0;
        assert!(!close.mine_shots(&inside, &mut Open));
        // Armour lets it take a little more, but not that.
        inside.armor = 100.0;
        assert!(!close.mine_shots(&inside, &mut Open));
        // Nor while a trail is being laid.
        close.mind.arms.trail.as_mut().unwrap().wanted = 5;
        close.mind.arms.trail.as_mut().unwrap().lay_until = SimTime(5.2);
        assert!(!close.mine_shots(&miner(5.0, false), &mut Open));
    }

    #[test]
    fn stepping_off_it_stops_for_nothing_but_an_enemy_by_a_mine() {
        let mut brain = brain();
        trail_at(&mut brain, &[500.0, 640.0, 780.0]);
        // An old one of its own lying long, the bot short of mines on the tripmine level: not now.
        brain
            .explosives
            .placed_mine(Vec3::new(-600.0, 0.0, -28.0), Vec3::Z, SimTime(0.0));
        brain.mind.task = Some(task(TrailPhase::Off, 29.0));
        let b = miner_with(30.0, true, 0);
        assert!(!brain.mine_shots(&b, &mut Open));
        brain.mind.task = Some(task(TrailPhase::Watch, 29.0));
        assert!(brain.mine_shots(&b, &mut Open), "watching, it gets that one back");
        assert_eq!(brain.mind.arms.shot_why, "to get it back");
        // An enemy by the trail is shot at once.
        brain.mind.arms.active = None;
        brain.mind.task = Some(task(TrailPhase::Off, 29.0));
        brain.beliefs.on_sighting(&seen(30.0, Vec3::new(800.0, 60.0, 0.0)));
        brain.update(SimTime(30.0), &params(), None, None);
        assert!(brain.mine_shots(&b, &mut Open));
        assert_eq!(brain.mind.arms.shot_why, "an enemy by it");
    }

    #[test]
    fn a_step_heard_by_its_own_trail_sets_it_off() {
        let mut brain = brain();
        trail_at(&mut brain, &[500.0, 640.0]);
        brain.beliefs.hypotheses.push(Hypothesis {
            id: 0,
            kind: HypothesisKind::Sound(SoundKind::Step),
            t: SimTime(4.9),
            pos: Some(Vec3::new(700.0, 80.0, -36.0)),
            bearing: 5.0,
            bearing_sigma: 10.0,
            weapon: None,
            strength: 0.3,
            track: None,
        });
        assert!(brain.mine_shots(&miner(5.0, false), &mut Open));
        assert_eq!(brain.mind.arms.shot_why, "someone heard by it");
    }

    #[test]
    fn a_trail_nobody_came_by_goes_off_on_the_tripmine_level_and_is_left_elsewhere() {
        let watched = |level: bool| {
            let mut brain = brain();
            trail_at(&mut brain, &[500.0, 640.0, 780.0]);
            let mut t = task(TrailPhase::Watch, 1.0);
            if let Task::Trail(t) = &mut t {
                t.stand = Some(Vec3::ZERO);
                t.until = Some(SimTime(6.0));
            }
            brain.mind.task = Some(t);
            let mut rng = BotRng::new(3, 3);
            let b = miner(6.5, level);
            brain.trail(&b, &character(), &Yard::new(), &mut Open, &mut rng);
            let shot = brain.mine_shots(&b, &mut Open);
            (brain, shot)
        };
        let (gg, shot) = watched(true);
        assert!(shot, "set off on the tripmine level");
        assert_eq!(gg.mind.arms.shot_why, "nobody came by it");
        let (dm, shot) = watched(false);
        assert!(
            !shot && dm.mind.arms.trail.is_none() && dm.mind.task.is_none(),
            "left as a trap"
        );
        assert_eq!(dm.mind.arms.stats.trails_left, 1);
        assert_eq!(dm.explosives.mines.len(), 3, "its mines lie on");
    }

    #[test]
    fn a_blast_costs_armour_first_point_for_point() {
        assert_eq!(health_loss(&[150.0], 100.0), 50.0);
        assert_eq!(health_loss(&[150.0], 0.0), 150.0);
        // The second blast finds less armour left.
        assert_eq!(health_loss(&[50.0, 50.0], 60.0), 10.0 + 30.0);
        assert_eq!(
            worst_loss(&mut [10.0, 30.0], 0.0),
            10.0 + 90.0,
            "the worst blast on the head"
        );
    }

    #[test]
    fn a_floor_mine_bursts_up_and_its_chain_carries_along_a_trail() {
        let burst = burst_at(Vec3::new(0.0, 0.0, 8.0), Vec3::Z, 150.0);
        assert!((burst.z - 76.6).abs() < 1e-3, "{burst:?}");
        let mine = |x: f32| Mine {
            pos: Vec3::new(x, 0.0, 8.0),
            dir: Vec3::Z,
            beam_end: None,
            own: true,
            armed_at: SimTime::ZERO,
            seen: SimTime::ZERO,
            avoided_at: None,
            pass: None,
        };
        // A trail 150 apart, and one far off.
        let mines = [mine(0.0), mine(150.0), mine(300.0), mine(450.0), mine(1500.0)];
        let chain = chain_of(&mines, 0, 150.0);
        assert_eq!(chain.len(), 4, "{chain:?}");
        // A player 100 units past its end takes a deadly blast; one 400 off only a wound.
        let blasts = blasts(&mines, &chain);
        assert!(chain_damage(&blasts, Vec3::new(550.0, 0.0, 36.0), 150.0) > 100.0);
        let far = chain_damage(&blasts, Vec3::new(850.0, 0.0, 36.0), 150.0);
        assert!(far < 10.0, "{far}");
    }
}
