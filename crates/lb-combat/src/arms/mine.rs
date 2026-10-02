//! Placing a tripmine: stop, draw the mine, look at the spot on the wall and press once when the view's line meets
//! the wall there within reach (the game places the mine where a 128-unit trace from the gun hits); confirmed by one
//! mine fewer, or taken for placed when the count does not show it in time (mines handed back by GunGame can hide
//! it): never pressed again, which would put another mine on the same spot. The mine arms 2.5 s after the press and
//! its beam runs along the wall's normal.
//!
//! [`FloorPlanter`] drops one on the floor under the bot on the run (a trail): the view down for a moment, one press
//! once the game's trace meets the floor, the bot running on; its beam stands up from the floor behind it.

use lb_core::math::{dir_to_view_angles, view_angle_vectors};
use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3, dmath};
use lb_game::mechanics::{Attack, TRIPMINE_REACH, Trigger};
use lb_game::weapons::WeaponId;
use lb_motor::LookIntent;
use lb_worldq::{TraceQuery, Tracer};

use super::{Hands, Request, Status, hold, press, settled, stop, takes};

/// The view's line must meet the spot this soon.
const TIMEOUT: f64 = 3.0;
/// After the press the count of mines is waited for this long at most.
const CONFIRM: f64 = 1.0;
/// One press.
const TAP: f32 = 1.0;
/// The view's trace must meet the wall this close to the spot.
const ON_SPOT: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Draw,
    Pressed { at: SimTime, before: i32 },
}

#[derive(Clone, Debug)]
pub struct Planter {
    /// The point on the wall and the wall's normal.
    pub spot: Vec3,
    pub normal: Vec3,
    phase: Phase,
    started: SimTime,
    /// The mine pressed for is in the bot's memory already.
    pub noted: bool,
}

impl Planter {
    /// When the mine was pressed for: it arms 2.5 s after.
    pub fn pressed_at(&self) -> Option<SimTime> {
        match self.phase {
            Phase::Pressed { at, .. } => Some(at),
            Phase::Draw => None,
        }
    }
}

impl Planter {
    pub fn new(spot: Vec3, normal: Vec3, now: SimTime) -> Planter {
        Planter {
            spot,
            normal,
            phase: Phase::Draw,
            started: now,
            noted: false,
        }
    }

    /// Where the placed mine sits (the game puts it 8 units off the wall).
    pub fn mine(&self) -> Vec3 {
        self.spot + self.normal * 8.0
    }

    pub fn update(&mut self, h: &Hands<'_>, tracer: &mut dyn Tracer) -> Status {
        let now = h.now;
        let w = WeaponId::Tripmine;
        let count = h.reserve(w);
        let angles = dir_to_view_angles(self.spot - h.eye);
        match self.phase {
            Phase::Draw => {
                if now.since(self.started) > TIMEOUT || count <= 0 {
                    return Status::Failed("no tripmine placed");
                }
                let mut weapon = hold(w);
                if h.ready(w) && settled(h.view, angles, 3.0) && meets_spot(h, self.spot, tracer) {
                    weapon = press(w, Attack::Primary, Trigger::Tap, TAP);
                    self.phase = Phase::Pressed { at: now, before: count };
                }
                Status::Running(Request {
                    weapon: Some(weapon),
                    look: Some(LookIntent::Angles(angles)),
                    movement: Some(stop()),
                    jump: false,
                })
            }
            Phase::Pressed { at, before } => {
                if count < before || now.since(at) > CONFIRM {
                    return Status::Done;
                }
                Status::Running(Request {
                    weapon: Some(hold(w)),
                    look: Some(LookIntent::Angles(angles)),
                    movement: Some(stop()),
                    jump: false,
                })
            }
        }
    }
}

/// The game's own placement trace (128 units along the view from the gun) meets the wall at the spot.
fn meets_spot(h: &Hands<'_>, spot: Vec3, tracer: &mut dyn Tracer) -> bool {
    let (forward, _, _) = view_angle_vectors(h.view);
    let tr = tracer.trace(&TraceQuery::line(h.eye, h.eye + forward * TRIPMINE_REACH));
    tr.fraction < 1.0 && tr.end.distance(spot) <= ON_SPOT
}

/// A mine dropped on the run is given up this soon if it does not go down (the next one is aimed afresh); a press is
/// taken when the count of mines goes down or the tripmine's clock starts (mines handed back at once, as GunGame does,
/// can hide the one fewer), and given up this long after with neither.
const DROP_TIMEOUT: f64 = 1.0;
const DROP_CONFIRM: f64 = 0.3;
/// The view is on the lay pitch within this before the press.
const DROP_SETTLE: f32 = 4.0;
/// Presses come no closer than this: the game takes one mine every 0.3 s, and each press waits for it to.
const DROP_TAP: f32 = 0.25;
/// The view down along the run while a trail is laid: the game puts the mine where its 128-unit placement trace meets
/// the floor, some 75 units ahead of a standing player, who runs over it long before it arms. A player lays a trail
/// so, pressing as fast as the tripmine allows: the mines go down some 85 units apart at a run.
pub const LAY_PITCH: f32 = 40.0;

/// Where the game's placement trace along the view meets a floor: the point and the floor's normal.
pub fn floor_spot(eye: Vec3, view: Vec3, tracer: &mut dyn Tracer) -> Option<(Vec3, Vec3)> {
    let (forward, _, _) = view_angle_vectors(view);
    let tr = tracer.trace(&TraceQuery::line(eye, eye + forward * TRIPMINE_REACH));
    (tr.fraction < 1.0 && !tr.start_solid && tr.normal.z >= 0.7 && tr.hit.is_none_or(|e| e == 0))
        .then_some((tr.end, tr.normal))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DropPhase {
    Aim,
    Pressed { at: SimTime, before: i32 },
}

/// Dropping a tripmine on the floor ahead of the bot while it runs on: the view along the run at the lay pitch, one
/// press as soon as the game takes one (a mine every 0.3 s) and the placement trace meets the floor; done once the
/// game took it. The bot's movement is left alone.
#[derive(Clone, Debug)]
pub struct FloorPlanter {
    /// The way the bot runs.
    pub heading: Vec2,
    phase: DropPhase,
    started: SimTime,
    /// Where the trace met the floor when the mine went down, and the floor's normal.
    pub placed: Option<(Vec3, Vec3)>,
}

impl FloorPlanter {
    pub fn new(heading: Vec2, now: SimTime) -> FloorPlanter {
        FloorPlanter {
            heading,
            phase: DropPhase::Aim,
            started: now,
            placed: None,
        }
    }

    /// Where the dropped mine sits (8 units off the floor) and the way its beam goes.
    pub fn mine(&self) -> Option<(Vec3, Vec3)> {
        self.placed.map(|(p, n)| (p + n * 8.0, n))
    }

    /// When the mine was pressed for: it arms 2.5 s after.
    pub fn pressed_at(&self) -> Option<SimTime> {
        match self.phase {
            DropPhase::Pressed { at, .. } => Some(at),
            DropPhase::Aim => None,
        }
    }

    /// `heading`: the way the bot runs now.
    pub fn update(&mut self, h: &Hands<'_>, tracer: &mut dyn Tracer, heading: Vec2) -> Status {
        let now = h.now;
        let w = WeaponId::Tripmine;
        let count = h.reserve(w);
        if heading != Vec2::ZERO {
            self.heading = heading;
        }
        let yaw = dmath::atan2(self.heading.y, self.heading.x).to_degrees();
        let angles = Vec3::new(LAY_PITCH, yaw, 0.0);
        let weapon = match self.phase {
            DropPhase::Aim => {
                if now.since(self.started) > DROP_TIMEOUT || count <= 0 {
                    return Status::Failed("no mine dropped");
                }
                let spot = (h.ready(w)
                    && takes(h.predicted(w), Attack::Primary)
                    && h.on_ground
                    && settled(h.view, angles, DROP_SETTLE))
                .then(|| floor_spot(h.eye, h.view, tracer))
                .flatten();
                match spot {
                    Some(spot) => {
                        self.placed = Some(spot);
                        self.phase = DropPhase::Pressed { at: now, before: count };
                        press(w, Attack::Primary, Trigger::Tap, DROP_TAP)
                    }
                    None => hold(w),
                }
            }
            // One press: another would drop another mine further on.
            DropPhase::Pressed { at, before } => {
                if count < before || !takes(h.predicted(w), Attack::Primary) {
                    return Status::Done;
                }
                if now.since(at) > DROP_CONFIRM {
                    return Status::Failed("the press was not taken");
                }
                hold(w)
            }
        };
        Status::Running(Request {
            weapon: Some(weapon),
            look: Some(LookIntent::Angles(angles)),
            movement: None,
            jump: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Armed;
    use lb_game::dll::DllProfile;
    use lb_game::self_state::{PredictedWeapon, Prediction};
    use lb_motor::Fire;
    use lb_worldq::{Trace, contents};

    /// A floor at z = 0 everywhere.
    struct Floor;

    impl Tracer for Floor {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            if q.end.z < 0.0 && q.start.z >= 0.0 {
                let f = q.start.z / (q.start.z - q.end.z);
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

    fn hands<'a>(t: f64, view: Vec3, arsenal: &'a [Armed], p: &'a Prediction) -> Hands<'a> {
        Hands {
            now: SimTime(t),
            eye: Vec3::new(0.0, 0.0, 64.0),
            origin: Vec3::new(0.0, 0.0, 36.0),
            velocity: Vec3::new(270.0, 0.0, 0.0),
            view,
            on_ground: true,
            on_ladder: false,
            waterlevel: 0,
            fov: 0.0,
            weapon: Some(WeaponId::Tripmine),
            arsenal,
            prediction: Some(p),
            dll: DllProfile::default(),
            gravity: 800.0,
            deploying: false,
        }
    }

    fn carrying(n: i32) -> Prediction {
        Prediction {
            current: Some(WeaponId::Tripmine),
            primary_ammo: n,
            ..Prediction::default()
        }
    }

    /// A wall at x = 100 facing -x.
    struct Wall;

    impl Tracer for Wall {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            if q.end.x > 100.0 && q.start.x <= 100.0 {
                let f = (100.0 - q.start.x) / (q.end.x - q.start.x);
                let mut t = Trace::clear(q.start + (q.end - q.start) * f);
                t.fraction = f;
                t.normal = Vec3::NEG_X;
                return t;
            }
            Trace::clear(q.end)
        }

        fn point_contents(&mut self, _p: Vec3) -> i32 {
            contents::EMPTY
        }
    }

    #[test]
    fn a_mine_goes_on_the_wall_with_one_press_whether_the_count_shows_it_or_not() {
        let arsenal = [Armed::new(WeaponId::Tripmine, None, Some(5))];
        let five = carrying(5);
        let spot = Vec3::new(100.0, 0.0, 64.0);
        let mut p = Planter::new(spot, Vec3::NEG_X, SimTime(0.0));
        let at = Vec3::ZERO;
        let Status::Running(r) = p.update(&hands(0.1, at, &arsenal, &five), &mut Wall) else {
            panic!()
        };
        let w = r.weapon.unwrap();
        assert_eq!((w.fire, w.trigger), (Fire::Primary, Trigger::Tap));
        assert_eq!(p.pressed_at(), Some(SimTime(0.1)));
        // Waiting for the count: no other press, which would put a second mine on the spot.
        let Status::Running(r) = p.update(&hands(0.5, at, &arsenal, &five), &mut Wall) else {
            panic!()
        };
        assert_eq!(r.weapon.unwrap().fire, Fire::None);
        // Mines handed back hid the one fewer: taken for placed all the same.
        assert_eq!(p.update(&hands(1.2, at, &arsenal, &five), &mut Wall), Status::Done);
    }

    #[test]
    fn a_mine_goes_down_on_the_floor_ahead_of_the_bot_while_it_runs_on() {
        let arsenal = [Armed::new(WeaponId::Tripmine, None, Some(5))];
        let five = carrying(5);
        let mut p = FloorPlanter::new(Vec2::X, SimTime(0.0));
        // Looking along the way at the horizon: no press, the view goes down, the bot is not stopped.
        let Status::Running(r) = p.update(&hands(0.0, Vec3::ZERO, &arsenal, &five), &mut Floor, Vec2::X) else {
            panic!()
        };
        assert_eq!(r.weapon.unwrap().fire, Fire::None);
        assert_eq!(r.movement, None);
        let down = Vec3::new(LAY_PITCH, 0.0, 0.0);
        assert_eq!(r.look, Some(LookIntent::Angles(down)));
        // Down: one press, the trace meeting the floor ahead.
        let Status::Running(r) = p.update(&hands(0.1, down, &arsenal, &five), &mut Floor, Vec2::X) else {
            panic!()
        };
        let w = r.weapon.unwrap();
        assert_eq!((w.fire, w.trigger), (Fire::Primary, Trigger::Tap));
        // Once only, while the count of mines catches up.
        let Status::Running(r) = p.update(&hands(0.3, down, &arsenal, &five), &mut Floor, Vec2::X) else {
            panic!()
        };
        assert_eq!(r.weapon.unwrap().fire, Fire::None);
        // Handed back at once, the one fewer hidden: the tripmine's clock running tells the game took it.
        let mut clock = carrying(5);
        clock.weapons[WeaponId::Tripmine as usize] = Some(PredictedWeapon {
            next_primary: 0.25,
            ..PredictedWeapon::default()
        });
        assert_eq!(
            p.clone()
                .update(&hands(0.15, down, &arsenal, &clock), &mut Floor, Vec2::X),
            Status::Done
        );
        // Neither: the press was not taken.
        assert_eq!(
            p.clone()
                .update(&hands(0.45, down, &arsenal, &five), &mut Floor, Vec2::X),
            Status::Failed("the press was not taken")
        );
        // No press while the tripmine's clock runs from the last one.
        let mut next = FloorPlanter::new(Vec2::X, SimTime(0.5));
        let Status::Running(r) = next.update(&hands(0.5, down, &arsenal, &clock), &mut Floor, Vec2::X) else {
            panic!()
        };
        assert_eq!(r.weapon.unwrap().fire, Fire::None);
        // One fewer: done, the mine some 75 units ahead, 8 units up.
        let four = carrying(4);
        assert_eq!(
            p.update(&hands(0.12, down, &arsenal, &four), &mut Floor, Vec2::X),
            Status::Done
        );
        let (at, dir) = p.mine().unwrap();
        assert!(
            (60.0..90.0).contains(&at.x) && (at.z - 8.0).abs() < 1e-3 && dir == Vec3::Z,
            "{at:?}"
        );
        // Never down: given up.
        let mut never = FloorPlanter::new(Vec2::X, SimTime(0.0));
        assert_eq!(
            never.update(&hands(1.2, Vec3::ZERO, &arsenal, &five), &mut Floor, Vec2::X),
            Status::Failed("no mine dropped")
        );
    }
}
