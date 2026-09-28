//! Navigation interface visible to the AI: NavQuery trait and request types.
//!
//! Behavior asks navigation for movement toward a point, for wandering, or for a place to fall back to; it never
//! walks the graph itself. The service is also the bot's tracer, so behavior code can check walls and ledges
//! through the same handle.
//!
//! What an experienced player knows of a map comes through [`MapView`]: its places (graph nodes) and the ways
//! between them, who sees whom from where, where players pass and where the way narrows, spots to watch from and
//! walls to set tripmines on, and where the bots got hurt before.

#![forbid(unsafe_code)]

use lb_core::rng::Pcg32;
use lb_core::{Vec2, Vec3};
use lb_worldq::Tracer;
use serde::{Deserialize, Serialize};

/// A place on the map: a node of the navigation graph.
pub type NodeId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CampKind {
    /// Long sightlines over ways players take, little in close.
    Overwatch,
    /// Out of the way, close to a chokepoint players come through.
    Ambush,
}

impl CampKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CampKind::Overwatch => "overwatch",
            CampKind::Ambush => "ambush",
        }
    }
}

/// A place worth holding for a while, and where to look from it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CampSpot {
    pub node: NodeId,
    /// Player origin standing there.
    pub pos: Vec3,
    pub kind: CampKind,
    /// World yaws worth watching, the best first (the same twice when there is one).
    pub watch: [f32; 2],
    /// Pitch toward what is watched, degrees (negative up).
    pub pitch: f32,
    /// Typical distance of what is watched.
    pub range: f32,
    /// How good the spot is of its kind, 0..1.
    pub score: f32,
    /// The chokepoint an ambush spot watches.
    pub guards: Option<NodeId>,
}

/// A wall a tripmine can go on, its beam across a way players take.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MineSpot {
    pub node: NodeId,
    /// Where to stand (player origin) to put the mine on the wall.
    pub stand: Vec3,
    /// Where on the wall to aim, and the wall's normal.
    pub wall: Vec3,
    pub normal: Vec3,
    /// Where the beam ends across the way.
    pub beam_end: Vec3,
    /// How much players pass there, 0..1.
    pub flow: f32,
    /// Behind a turn: players round a corner into the beam.
    pub corner: bool,
}

/// The map as an experienced player knows it. Static knowledge worked out from the map once, plus what the bots
/// learned by playing it; nothing here tells where anyone is now.
pub trait MapView {
    fn node_count(&self) -> usize;
    /// Player origin standing (crouched for crouch-only places) at `n`.
    fn node_origin(&self, n: NodeId) -> Vec3;
    /// The node nearest to `p` within `max` units, vertical distance counting double.
    fn nearest_node(&self, p: Vec3, max: f32) -> Option<NodeId>;
    /// Every usable way out of `n`: where it leads and the least time it takes, seconds.
    fn for_each_link(&self, n: NodeId, f: &mut dyn FnMut(NodeId, f32));
    /// A player at `a` sees one at `b` (eye to eye, through glass; the same both ways).
    fn visible(&self, a: NodeId, b: NodeId) -> bool;
    /// Every node a player at `n` sees.
    fn for_each_visible(&self, n: NodeId, f: &mut dyn FnMut(NodeId));
    /// How much players pass through `n`, 0..1.
    fn flow(&self, n: NodeId) -> f32;
    /// How much traffic sees `n` from close by, 0..1.
    fn exposure(&self, n: NodeId) -> f32;
    /// Not a place to stop at: a ladder, water, mid-air, a lift.
    fn transit(&self, n: NodeId) -> bool;
    /// How much bots got hurt at `n` before, 0..1.
    fn danger(&self, n: NodeId) -> f32;
    /// Where the damage taken at `n` mostly came from.
    fn danger_from(&self, n: NodeId) -> Option<NodeId>;
    fn camp_spots(&self) -> &[CampSpot];
    fn mine_spots(&self) -> &[MineSpot];
    /// Narrow places many players pass, busiest first.
    fn chokepoints(&self) -> &[NodeId];
}

/// Movement for one frame along a path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavStep {
    /// Horizontal world direction; zero = stand still.
    pub move_dir: Vec2,
    pub speed: f32,
    /// Where to look to follow the path.
    pub look_at: Vec3,
    /// View pitch to hold exactly (ladders, swimming, aiming at a button); without it `look_at` is only the way the
    /// bot is going, and looking elsewhere for a moment does no harm.
    pub pitch: Option<f32>,
    pub jump: bool,
    pub duck: bool,
    /// Press the use key (buttons, use-only doors); the motor makes it a fresh press.
    pub use_key: bool,
    /// Shoot at this point to break an obstacle in the way.
    pub fire_at: Option<Vec3>,
    /// Break it with the crowbar.
    pub melee: bool,
    /// A traversal that must not be disturbed (a jump in flight, a ladder, aiming at a button): its look and
    /// stance win over combat.
    pub mandatory: bool,
    /// A long jump: duck and jump pressed together, both afresh.
    pub longjump: bool,
    /// A gauss boost the traversal waits for, the bot standing at its takeoff.
    pub boost: Option<BoostCall>,
}

/// A gauss boost a traversal needs now: the weapons are to charge the gauss fully, turn the view to `view`, jump and
/// let the charge go as the bot leaves the ground. The recoil throws it the way it looks away from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoostCall {
    /// View angles to let the charge go along: back the way and down.
    pub view: Vec3,
    /// Seconds the charge builds at least.
    pub charge: f32,
}

/// Tricks a bot may use on the way, as its brain allows them now.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tricks {
    /// It has the long jump module: the links only a long jump makes are open to it.
    pub longjump: bool,
    /// Long jumps along straight stretches of the way, for speed.
    pub runway: bool,
    /// Paths may take the links a gauss boost makes.
    pub gauss_boost: bool,
    /// A gauss boost can be made now: a gauss, a full charge's uranium, the health, no enemy about.
    pub boost_now: bool,
    /// Damage of the gauss's full charge (the recoil is five times it).
    pub gauss_damage: f32,
    /// A charged gauss beam that fails to punch through a wall comes back at its shooter (vanilla `selfgauss`).
    pub selfgauss: bool,
}

impl NavStep {
    /// Standing still, looking at `look_at`.
    pub fn hold(look_at: Vec3) -> NavStep {
        NavStep {
            move_dir: Vec2::ZERO,
            speed: 0.0,
            look_at,
            pitch: None,
            jump: false,
            duck: false,
            use_key: false,
            fire_at: None,
            melee: false,
            mandatory: false,
            longjump: false,
            boost: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavStatus {
    Moving,
    /// At the destination (or the graph node closest to it).
    Arrived,
    /// No way there, or navigation gave up after getting stuck.
    NoPath,
}

pub trait NavService: Tracer {
    /// Walks toward `dest`, planning a path when the destination changes.
    fn go_to(&mut self, dest: Vec3) -> (NavStatus, Option<NavStep>);
    /// Wanders between places worth visiting; `rng` picks them.
    fn roam(&mut self, rng: &mut Pcg32) -> Option<NavStep>;
    /// A place to fall back to, away from `threat`.
    fn away_from(&mut self, threat: Vec3) -> Option<Vec3>;
    /// A place near the bot out of sight of `threat` that the bot gets to before the threat could; `None` when there
    /// is none within reach.
    fn cover_from(&mut self, _threat: Vec3) -> Option<Vec3> {
        None
    }
    /// A navigation graph is loaded.
    fn available(&self) -> bool;
    /// Keeps paths off the line `a → b` at body height for `seconds` (a tripmine's beam the bot knows of).
    fn avoid_line(&mut self, _a: Vec3, _b: Vec3, _seconds: f32) {}
    /// What the bot may do on the way from now on.
    fn set_tricks(&mut self, _tricks: Tricks) {}
    /// Where a long jump taken now looking along `view` comes down, if it comes down safely: on a floor or in
    /// water, without fall damage, out of lava and slime.
    fn leap_lands(&mut self, _view: Vec3) -> Option<Vec3> {
        None
    }
    /// A gauss boost from where the bot stands along its way, landing safely further along it: the path takes it
    /// next. False when there is none (or no way followed).
    fn gauss_leap(&mut self) -> bool {
        false
    }
    /// How far the destination of the way followed is: in a straight line, and in nodes left on the path.
    fn way_left(&self) -> Option<(f32, usize)> {
        None
    }
    /// In the air on a long jump or a gauss boost of the way: the step that steers the flight onto its landing,
    /// whatever behavior does now. `None` when not flying.
    fn flight(&mut self) -> Option<NavStep> {
        None
    }
}
