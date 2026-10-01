//! Aim: where to point the crosshair at a target.
//!
//! - **Body part:** head or body is rolled once per contact from the skill's headshot chance. Shotguns beyond 272
//!   units and the MP5 beyond 544 aim at the body (yapb's spray distances).
//! - **Latency:** the bot aims at what it saw `aim_latency` seconds ago, carried forward with the velocity it had
//!   estimated then; a target that changes direction is missed for that long (jk_botti's ping emulation).
//! - **Error:** an Ornstein–Uhlenbeck drift per axis that grows with distance and shrinks with skill, replacing
//!   yapb's error re-rolls every 0.4–0.8 s.
//! - **Projectiles** lead the target by their flight time along the velocity seen; explosives go for the feet of a
//!   target on the ground, where a near miss still catches it in the blast.

use std::collections::VecDeque;

use lb_core::Vec3;
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AimSkill {
    pub headshot: f32,
    pub latency: f32,
    /// Error amplitude per axis, units.
    pub error: [f32; 3],
    /// 0..100.
    pub skill: u8,
}

#[derive(Clone, Debug, Default)]
pub struct Aim {
    target: Option<PlayerKey>,
    head: bool,
    history: VecDeque<Sample>,
    error: Vec3,
    last_update: Option<SimTime>,
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
    }

    /// The point to aim at now, from `eye`, for `shot`.
    pub fn point(&mut self, now: SimTime, eye: Vec3, shot: &Shot, skill: &AimSkill, rng: &mut Pcg32) -> Option<Vec3> {
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
        let weapon = shot.weapon;
        let head = self.head
            && !(weapon == Some(WeaponId::Shotgun) && distance > SPRAY_DISTANCE)
            && !(weapon == Some(WeaponId::Mp5) && distance > 2.0 * SPRAY_DISTANCE);
        let z = match (head, s.crouched) {
            _ if shot.feet && s.on_ground => {
                if s.crouched {
                    -FEET_CROUCHED
                } else {
                    -FEET
                }
            }
            (true, false) => 22.0,
            (true, true) => 10.0,
            (false, false) => 8.0,
            (false, true) => 0.0,
        };
        self.drift(now, distance, skill, rng);
        let error = if shot.steady {
            self.error * SCOPE_STEADY
        } else {
            self.error
        };
        Some(origin + Vec3::Z * z + error)
    }

    fn drift(&mut self, now: SimTime, distance: f32, skill: &AimSkill, rng: &mut Pcg32) {
        let dt = self.last_update.map_or(0.0, |t| now.since(t).clamp(0.0, 0.25)) as f32;
        self.last_update = Some(now);
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

#[cfg(test)]
mod tests {
    use super::*;
    use lb_knowledge::*;

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

    #[test]
    fn latency_aims_behind_a_target_that_turned() {
        let skill = AimSkill {
            headshot: 0.0,
            latency: 0.3,
            error: [0.0; 3],
            skill: 50,
        };
        let mut aim = Aim::default();
        let mut rng = Pcg32::new(1, 1);
        // Ran along +X up to x = 440 at t = 1.0, then turned back and is at x = 380 at t = 1.3.
        aim.follow(&track(1.0, 440.0, 200.0), &skill, &mut rng);
        aim.follow(&track(1.3, 380.0, -200.0), &skill, &mut rng);
        let hitscan = Shot::default();
        let p = aim.point(SimTime(1.3), Vec3::ZERO, &hitscan, &skill, &mut rng).unwrap();
        assert!(p.x > 450.0, "0.3 s behind, it still sees the target running on: {p}");
        assert!((p.z - 8.0).abs() < 1e-3, "body height");
        let quick = AimSkill { latency: 0.0, ..skill };
        let p = aim.point(SimTime(1.3), Vec3::ZERO, &hitscan, &quick, &mut rng).unwrap();
        assert!(p.x < 400.0, "without latency it aims at the turned target: {p}");
        let rocket = Shot {
            weapon: Some(WeaponId::Rpg),
            speed: Some(1000.0),
            feet: true,
            steady: false,
        };
        let p = aim.point(SimTime(1.3), Vec3::ZERO, &rocket, &quick, &mut rng).unwrap();
        assert!(p.x < 380.0 - 50.0, "leads the target it runs back: {p}");
        assert!((p.z + FEET).abs() < 1e-3, "at the feet: {p}");
    }

    #[test]
    fn head_or_body_is_rolled_once_per_contact() {
        let skill = AimSkill {
            headshot: 0.5,
            latency: 0.0,
            error: [0.0; 3],
            skill: 50,
        };
        let mut rng = Pcg32::new(9, 9);
        let mut aim = Aim::default();
        let t = track(1.0, 400.0, 0.0);
        aim.follow(&t, &skill, &mut rng);
        let first = aim.aims_at_head();
        for _ in 0..50 {
            aim.follow(&t, &skill, &mut rng);
            assert_eq!(aim.aims_at_head(), first);
        }
        let shotgun = Shot {
            weapon: Some(WeaponId::Shotgun),
            ..Shot::default()
        };
        let p = aim.point(SimTime(1.0), Vec3::ZERO, &shotgun, &skill, &mut rng).unwrap();
        assert!((p.z - 8.0).abs() < 1e-3, "shotguns aim at the body beyond 272 units");
    }

    #[test]
    fn the_error_grows_with_distance() {
        let skill = AimSkill {
            headshot: 0.0,
            latency: 0.0,
            error: [10.0, 10.0, 20.0],
            skill: 50,
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
                        &Shot::default(),
                        &skill,
                        &mut rng,
                    )
                    .unwrap();
                sum += (p.y).powi(2);
            }
            (sum / 2000.0f32).sqrt()
        };
        let (near, far) = (spread(200.0), spread(2500.0));
        assert!(near > 5.0 && near < 20.0, "{near}");
        assert!(far > near * 1.5, "{near} {far}");
    }
}
