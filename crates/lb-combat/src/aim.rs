//! Aim: where to point the crosshair at a target.
//!
//! - **Body part:** head or body is rolled once per contact from the skill's headshot chance. Shotguns beyond 272
//!   units and the MP5 beyond 544 aim at the body (yapb's spray distances).
//! - **Height:** a contact is first aimed at the height the crosshair already crosses it at, when that is on the
//!   body, else at the chest; once the view is on the body the aim climbs to the head (or settles on the chest) over
//!   `head_climb` seconds, as a player flicks to what is in front of them and then lifts the crosshair.
//! - **Latency:** the bot aims at what it saw `aim_latency` seconds ago, carried forward with the velocity it had
//!   estimated then; a target that changes direction is missed for that long (jk_botti's ping emulation).
//! - **Error:** an Ornstein–Uhlenbeck drift per axis that grows with distance and shrinks with skill, replacing
//!   yapb's error re-rolls every 0.4–0.8 s.
//! - **Projectiles** lead the target by their flight time along the velocity seen; explosives go for the feet of a
//!   target on the ground, where a near miss still catches it in the blast.

use std::collections::VecDeque;

use lb_core::Vec3;
use lb_core::dmath;
use lb_core::math::view_angle_vectors;
use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use lb_game::weapons::WeaponId;
use lb_knowledge::{EnemyTrack, PlayerKey, Stance};

/// Shotguns spray, and semi-automatic weapons are fired as fast as they go, within this distance (yapb).
pub const SPRAY_DISTANCE: f32 = 272.0;
/// A scope's share of the aim error.
pub const SCOPE_STEADY: f32 = 0.5;
/// Below the origin of a standing (and a crouched) player, a little above the floor.
const FEET: f32 = 28.0;
const FEET_CROUCHED: f32 = 12.0;
const HISTORY: f64 = 1.0;
const ERROR_TAU: f32 = 0.45;
/// Heights above the origin of a standing and a crouched player: the head, the chest, and the span of the body a
/// crosshair already there is kept at.
const HEAD: [f32; 2] = [22.0, 10.0];
const CHEST: [f32; 2] = [8.0, 0.0];
const BODY_SPAN: [[f32; 2]; 2] = [[-6.0, 24.0], [-8.0, 12.0]];
/// The climb is critically damped and 95% done in `head_climb` seconds: this many of its time constants.
const CLIMB: f32 = 4.74;

#[derive(Clone, Copy, Debug)]
struct Sample {
    t: SimTime,
    pos: Vec3,
    vel: Vec3,
    crouched: bool,
    on_ground: bool,
}

/// How the shot flies.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shot {
    pub weapon: Option<WeaponId>,
    /// Speed of the projectile to lead the target by; `None` for hitscan.
    pub speed: Option<f32>,
    /// Aim at the feet of a target on the ground.
    pub feet: bool,
    /// Through a scope: the aim error is halved.
    pub steady: bool,
}

/// A target as the aim sees it ([`Aim::seen`]): its box's center and velocity, whether it stands on the ground and
/// crouches, and the aim error to add to the point chosen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    pub center: Vec3,
    pub vel: Vec3,
    pub on_ground: bool,
    pub crouched: bool,
    pub error: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AimSkill {
    pub headshot: f32,
    pub latency: f32,
    /// Error amplitude per axis, units.
    pub error: [f32; 3],
    /// 0..100.
    pub skill: u8,
    /// Seconds the aim takes from the chest to the head.
    pub head_climb: f32,
}

/// Where to aim and how fast that point moves, units per second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AimPoint {
    pub at: Vec3,
    pub vel: Vec3,
}

#[derive(Clone, Debug, Default)]
pub struct Aim {
    target: Option<PlayerKey>,
    head: bool,
    history: VecDeque<Sample>,
    error: Vec3,
    last_update: Option<SimTime>,
    /// The height aimed at above the target's origin and how fast it changes, from the contact's first point on.
    height: Option<(f32, f32)>,
    /// The view has been on the body since: the height goes where it is aimed.
    climbing: bool,
}

impl Aim {
    pub fn target(&self) -> Option<PlayerKey> {
        self.target
    }

    pub fn aims_at_head(&self) -> bool {
        self.head
    }

    /// Follows `track`; a new contact rolls head or body once.
    pub fn follow(&mut self, track: &EnemyTrack, skill: &AimSkill, rng: &mut Pcg32) {
        if self.target != Some(track.who) || self.history.back().is_some_and(|s| track.recognized_at > s.t) {
            self.target = Some(track.who);
            self.head = rng.next_f32() < skill.headshot;
            self.history.clear();
            self.height = None;
            self.climbing = false;
        }
        if self.history.back().is_none_or(|s| track.last_seen > s.t) {
            self.history.push_back(Sample {
                t: track.last_seen,
                pos: track.pos,
                vel: if track.velocity_known(track.last_seen) {
                    track.vel
                } else {
                    Vec3::ZERO
                },
                crouched: track.traits.stance == Stance::Crouched,
                on_ground: track.traits.on_ground,
            });
        }
        while self.history.len() > 2 && track.last_seen.since(self.history[1].t) > HISTORY {
            self.history.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.target = None;
        self.history.clear();
        self.height = None;
        self.climbing = false;
    }

    /// The point to aim at now from `eye`, with the view at `view`, for `shot`.
    pub fn point(
        &mut self,
        now: SimTime,
        eye: Vec3,
        view: Vec3,
        shot: &Shot,
        skill: &AimSkill,
        rng: &mut Pcg32,
    ) -> Option<AimPoint> {
        let seen_until = now + -f64::from(skill.latency);
        let s = self
            .history
            .iter()
            .rev()
            .find(|s| s.t <= seen_until)
            .or(self.history.front())
            .copied()?;
        let ahead = (now.since(s.t) as f32).clamp(0.0, 0.5);
        let mut origin = s.pos + s.vel * ahead;
        let distance = origin.distance(eye);
        if let Some(speed) = shot.speed {
            origin += s.vel * (distance / speed.max(1.0)).min(1.0);
        }
        let dt = self.last_update.map_or(0.0, |t| now.since(t).clamp(0.0, 0.25)) as f32;
        self.last_update = Some(now);
        let stance = usize::from(s.crouched);
        let (z, rate) = if shot.feet && s.on_ground {
            let feet = if s.crouched { -FEET_CROUCHED } else { -FEET };
            *self.height.insert((feet, 0.0))
        } else {
            let weapon = shot.weapon;
            let head = self.head
                && !(weapon == Some(WeaponId::Shotgun) && distance > SPRAY_DISTANCE)
                && !(weapon == Some(WeaponId::Mp5) && distance > 2.0 * SPRAY_DISTANCE);
            let goal = if head { HEAD[stance] } else { CHEST[stance] };
            let height = *self
                .height
                .get_or_insert_with(|| (crosshair(eye, view, origin, stance), 0.0));
            if self.climbing {
                *self.height.insert(climb(height, goal, dt, skill.head_climb))
            } else {
                height
            }
        };
        self.drift(dt, distance, skill, rng);
        let error = if shot.steady {
            self.error * SCOPE_STEADY
        } else {
            self.error
        };
        let at = origin + Vec3::Z * z + error;
        self.climbing |= crate::fire::on_body(view, eye, at);
        Some(AimPoint {
            at,
            vel: s.vel + Vec3::Z * rate,
        })
    }

    /// What the aim goes by for a shot it places itself (a rocket, see [`crate::rocket`]): the target as seen
    /// `latency` ago carried forward to now, and the aim error now. Called instead of [`Aim::point`].
    pub fn seen(&mut self, now: SimTime, eye: Vec3, skill: &AimSkill, rng: &mut Pcg32) -> Option<Seen> {
        let seen_until = now + -f64::from(skill.latency);
        let s = self
            .history
            .iter()
            .rev()
            .find(|s| s.t <= seen_until)
            .or(self.history.front())
            .copied()?;
        let ahead = (now.since(s.t) as f32).clamp(0.0, 0.5);
        let center = s.pos + s.vel * ahead;
        let dt = self.last_update.map_or(0.0, |t| now.since(t).clamp(0.0, 0.25)) as f32;
        self.last_update = Some(now);
        self.drift(dt, center.distance(eye), skill, rng);
        Some(Seen {
            center,
            vel: s.vel,
            on_ground: s.on_ground,
            crouched: s.crouched,
            error: self.error,
        })
    }

    /// The target's mean velocity along the ground over the `window` seconds up to what the aim goes by now (`latency`
    /// ago): what of its run goes on, the strafing back and forth averaged out; over what there is of it, from a fifth
    /// on.
    pub fn mean_velocity(&self, now: SimTime, latency: f32, window: f64) -> Option<Vec3> {
        let seen_until = now + -f64::from(latency);
        let last = self
            .history
            .iter()
            .rev()
            .find(|s| s.t <= seen_until)
            .or(self.history.front())?;
        let first = self
            .history
            .iter()
            .rev()
            .find(|s| last.t.since(s.t) >= window)
            .or(self.history.front())
            .filter(|s| last.t.since(s.t) >= 0.2 * window)?;
        let way = (last.pos - first.pos) / last.t.since(first.t) as f32;
        Some(Vec3::new(way.x, way.y, 0.0))
    }

    fn drift(&mut self, dt: f32, distance: f32, skill: &AimSkill, rng: &mut Pcg32) {
        if dt <= 0.0 {
            return;
        }
        let level = (f32::from(skill.skill) / 25.0).clamp(1.0, 4.0);
        let scale = 1.0 + distance / (1280.0 * level);
        let decay = dt / ERROR_TAU;
        let kick = (2.0 * decay).sqrt();
        for axis in 0..3 {
            let sigma = skill.error[axis] * scale;
            self.error[axis] += -self.error[axis] * decay + sigma * kick * rng.normal();
        }
    }
}

/// The height above `origin` that the view from `eye` crosses a target there at, when that is on the body; else the
/// chest.
fn crosshair(eye: Vec3, view: Vec3, origin: Vec3, stance: usize) -> f32 {
    let (forward, _, _) = view_angle_vectors(view);
    let flat = forward.truncate().length();
    let span = BODY_SPAN[stance];
    (flat > 0.1)
        .then(|| eye.z + forward.z / flat * (origin - eye).truncate().length() - origin.z)
        .filter(|z| (span[0]..=span[1]).contains(z))
        .unwrap_or(CHEST[stance])
}

/// The height and its rate after `dt` of a critically damped approach to `goal` that is 95% done in `time`.
fn climb((z, rate): (f32, f32), goal: f32, dt: f32, time: f32) -> (f32, f32) {
    if time <= 0.0 {
        return (goal, 0.0);
    }
    let w = CLIMB / time;
    let e = z - goal;
    let b = rate + w * e;
    let k = dmath::exp(-w * dt);
    (goal + (e + b * dt) * k, (rate - w * b * dt) * k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_knowledge::*;

    fn sighting(t: f64, pos: Vec3) -> Sighting {
        Sighting {
            who: PlayerKey { slot: 2, userid: 7 },
            relation: Relation::Enemy,
            t: SimTime(t),
            pos,
            sigma: 1.0,
            distance: 500.0,
            visibility: 1.0,
            parts: parts::CHEST,
            stance: Stance::Standing,
            on_ground: true,
            on_ladder: false,
            in_water: false,
            facing: 0.0,
            weapon: None,
            firing: false,
            render: RenderCue::default(),
            first: false,
            noticed_at: SimTime::ZERO,
        }
    }

    fn track(t: f64, x: f32, vx: f32) -> EnemyTrack {
        let mut b = Beliefs::default();
        let mut s = Sighting {
            who: PlayerKey { slot: 2, userid: 7 },
            relation: Relation::Enemy,
            t: SimTime(t - 0.05),
            pos: Vec3::new(x - vx * 0.05, 0.0, 0.0),
            sigma: 1.0,
            distance: 500.0,
            visibility: 1.0,
            parts: parts::CHEST,
            stance: Stance::Standing,
            on_ground: true,
            on_ladder: false,
            in_water: false,
            facing: 0.0,
            weapon: None,
            firing: false,
            render: RenderCue::default(),
            first: true,
            noticed_at: SimTime::ZERO,
        };
        for i in 0..8 {
            s.t = SimTime(t - 0.05 * f64::from(7 - i));
            s.pos = Vec3::new(x - vx * 0.05 * (7 - i) as f32, 0.0, 0.0);
            s.first = i == 0;
            b.on_sighting(&s);
        }
        b.tracks[0].clone()
    }

    /// Looking up and away: the crosshair crosses nothing of the target.
    const AWAY: Vec3 = Vec3::new(-45.0, 90.0, 0.0);

    fn skill(headshot: f32) -> AimSkill {
        AimSkill {
            headshot,
            latency: 0.0,
            error: [0.0; 3],
            skill: 50,
            head_climb: 0.3,
        }
    }

    #[test]
    fn latency_aims_behind_a_target_that_turned() {
        let skill = AimSkill {
            latency: 0.3,
            ..skill(0.0)
        };
        let mut aim = Aim::default();
        let mut rng = Pcg32::new(1, 1);
        // Ran along +X up to x = 440 at t = 1.0, then turned back and is at x = 380 at t = 1.3.
        aim.follow(&track(1.0, 440.0, 200.0), &skill, &mut rng);
        aim.follow(&track(1.3, 380.0, -200.0), &skill, &mut rng);
        let hitscan = Shot::default();
        let p = aim
            .point(SimTime(1.3), Vec3::ZERO, AWAY, &hitscan, &skill, &mut rng)
            .unwrap();
        assert!(
            p.at.x > 450.0,
            "0.3 s behind, it still sees the target running on: {p:?}"
        );
        assert!(p.vel.x > 150.0, "and moving on: {p:?}");
        assert!((p.at.z - 8.0).abs() < 1e-3, "at the chest");
        let quick = AimSkill { latency: 0.0, ..skill };
        let p = aim
            .point(SimTime(1.3), Vec3::ZERO, AWAY, &hitscan, &quick, &mut rng)
            .unwrap();
        assert!(p.at.x < 400.0, "without latency it aims at the turned target: {p:?}");
        let rocket = Shot {
            weapon: Some(WeaponId::Rpg),
            speed: Some(1000.0),
            feet: true,
            steady: false,
        };
        let p = aim
            .point(SimTime(1.3), Vec3::ZERO, AWAY, &rocket, &quick, &mut rng)
            .unwrap();
        assert!(p.at.x < 380.0 - 50.0, "leads the target it runs back: {p:?}");
        assert!((p.at.z + FEET).abs() < 1e-3, "at the feet at once: {p:?}");
    }

    /// The points aimed at a still target 500 units off every 10 ms for `secs`, with the view at `view` until the
    /// `look`th and on the aim point from then on.
    fn heights(skill: &AimSkill, shot: &Shot, view: Vec3, look: usize, secs: f32) -> Vec<(f32, AimPoint)> {
        let mut rng = Pcg32::new(5, 5);
        let mut aim = Aim::default();
        aim.follow(&track(1.0, 500.0, 0.0), skill, &mut rng);
        let mut view = view;
        (0..=(secs * 100.0).round() as usize)
            .map(|i| {
                let into = i as f32 * 0.01;
                let p = aim
                    .point(SimTime(1.0 + f64::from(into)), Vec3::ZERO, view, shot, skill, &mut rng)
                    .unwrap();
                if i >= look {
                    view = lb_core::math::dir_to_view_angles(p.at);
                }
                (into, p)
            })
            .collect()
    }

    #[test]
    fn the_aim_lands_on_the_chest_and_climbs_to_the_head() {
        let head = skill(1.0);
        let points = heights(&head, &Shot::default(), AWAY, 10, 0.6);
        for (into, p) in &points[..=11] {
            assert!(
                (p.at.z - CHEST[0]).abs() < 1e-3,
                "the chest until the view is on it: {into} {p:?}"
            );
        }
        for w in points.windows(2) {
            let (a, b) = (w[0].1.at.z, w[1].1.at.z);
            assert!(
                b >= a - 1e-4 && b <= HEAD[0] + 1e-3,
                "rises without overshooting: {a} {b}"
            );
            assert!(b - a < 1.0, "smoothly: {a} {b}");
        }
        let (_, start) = points[12];
        assert!(
            start.at.z < CHEST[0] + 0.5 && start.vel.z > 0.0,
            "starts slowly: {start:?}"
        );
        let (_, done) = points[12 + 30];
        assert!(done.at.z > HEAD[0] - 0.75, "{done:?}");
        let shotgun = Shot {
            weapon: Some(WeaponId::Shotgun),
            ..Shot::default()
        };
        let (_, last) = *heights(&head, &shotgun, AWAY, 0, 0.6).last().unwrap();
        assert!(
            (last.at.z - CHEST[0]).abs() < 0.01,
            "shotguns aim at the body beyond 272 units: {last:?}"
        );
    }

    #[test]
    fn a_crosshair_already_on_the_body_keeps_its_height() {
        // Looking a little up and away: the crosshair crosses the target's line at 18 units over its origin.
        let placed = Vec3::new(-lb_core::dmath::atan(18.0 / 500.0).to_degrees(), 90.0, 0.0);
        let points = heights(&skill(0.0), &Shot::default(), placed, 30, 1.0);
        assert!((points[0].1.at.z - 18.0).abs() < 0.05, "{:?}", points[0]);
        assert!(
            (points[25].1.at.z - 18.0).abs() < 0.05,
            "kept while the view comes over"
        );
        assert!(
            (points.last().unwrap().1.at.z - CHEST[0]).abs() < 0.05,
            "then settles on the chest"
        );
    }

    #[test]
    fn the_mean_velocity_averages_out_a_strafe() {
        let skill = skill(0.0);
        let mut aim = Aim::default();
        let mut rng = Pcg32::new(1, 1);
        aim.follow(&track(1.0, 400.0, 200.0), &skill, &mut rng);
        assert_eq!(aim.mean_velocity(SimTime(1.0), 0.0, 0.5), None, "a single sighting");
        let mut b = Beliefs::default();
        let mut aim = Aim::default();
        // Strafing 0.25 s each way at 200 units/s across +y, a sighting every 0.05 s.
        for i in 0..21 {
            let t = 0.05 * f64::from(i);
            let phase = (t / 0.25) as i32;
            let into = (t - 0.25 * f64::from(phase)) as f32;
            let y = if phase % 2 == 0 {
                200.0 * into
            } else {
                50.0 - 200.0 * into
            };
            let mut s = sighting(t, Vec3::new(600.0, y, 0.0));
            s.first = i == 0;
            b.on_sighting(&s);
            aim.follow(&b.tracks[0], &skill, &mut rng);
        }
        let v = aim.mean_velocity(SimTime(1.0), 0.0, 0.5).unwrap();
        assert!(v.length() < 30.0, "{v}");
        assert!(
            b.tracks[0].vel.length() > 100.0,
            "the track's own velocity swings: {}",
            b.tracks[0].vel
        );
    }

    #[test]
    fn head_or_body_is_rolled_once_per_contact() {
        let skill = skill(0.5);
        let mut rng = Pcg32::new(9, 9);
        let mut aim = Aim::default();
        let t = track(1.0, 400.0, 0.0);
        aim.follow(&t, &skill, &mut rng);
        let first = aim.aims_at_head();
        for _ in 0..50 {
            aim.follow(&t, &skill, &mut rng);
            assert_eq!(aim.aims_at_head(), first);
        }
    }

    #[test]
    fn the_error_grows_with_distance() {
        let skill = AimSkill {
            error: [10.0, 10.0, 20.0],
            ..skill(0.0)
        };
        let spread = |x: f32| {
            let mut rng = Pcg32::new(3, 3);
            let mut aim = Aim::default();
            aim.follow(&track(1.0, x, 0.0), &skill, &mut rng);
            let mut sum = 0.0;
            for i in 0..2000 {
                let p = aim
                    .point(
                        SimTime(1.0 + f64::from(i) * 0.01),
                        Vec3::ZERO,
                        Vec3::ZERO,
                        &Shot::default(),
                        &skill,
                        &mut rng,
                    )
                    .unwrap();
                sum += (p.at.y).powi(2);
            }
            (sum / 2000.0f32).sqrt()
        };
        let (near, far) = (spread(200.0), spread(2500.0));
        assert!(near > 5.0 && near < 20.0, "{near}");
        assert!(far > near * 1.5, "{near} {far}");
    }
}
