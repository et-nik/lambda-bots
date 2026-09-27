//! Movement in a fight, after yapb's `attackMovement`.
//!
//! - **Style:** every 1–3 s the bot picks strafing or standing. Closer than 768 units it strafes; further away it
//!   stands with the skill's `stay_mid` / `stay_far` chance. A low will to approach (health × aggression below
//!   30), or a pistol or shotgun against an enemy facing the bot, makes it strafe.
//! - **Strafe side:** away from the side the enemy aims at, swapped 30% of the time, re-decided every 0.3–0.8 s.
//!   Walls within 134 units on a side turn it around.
//! - **Distance:** skilled bots drift in when they feel strong and far, back off when weak and close, and back off
//!   when cornered. Everyone backs off under 96 units or while reloading; melee charges.
//! - **Extras:** crouch taps and dodge jumps by skill.
//! - **Ledges:** a move that would drop more than 160 units is reversed.

use lb_core::dmath;
use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3};
use lb_game::mechanics::WeaponClass;
use lb_worldq::{TraceQuery, Tracer};

const WALL_DISTANCE: f32 = 134.0;
const SAFE_DROP: f32 = 160.0;
const CHECK_PERIOD: f64 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FightSkill {
    /// 0..100.
    pub skill: u8,
    pub stay_mid: f32,
    pub stay_far: f32,
    pub crouch_tap: f32,
    pub dodge_hop_cooldown: Option<f32>,
}

#[derive(Clone, Copy, Debug)]
pub struct FightInput {
    pub now: SimTime,
    pub origin: Vec3,
    pub enemy: Vec3,
    /// Observed facing yaw of the enemy, degrees.
    pub enemy_facing: f32,
    pub enemy_faces_me: bool,
    /// Health × aggression, 0..100.
    pub approach: f32,
    pub weapon: WeaponClass,
    pub reloading: bool,
    pub on_ground: bool,
    pub maxspeed: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FightMove {
    /// World direction scaled by speed.
    pub velocity: Vec2,
    pub duck: bool,
    pub jump: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Style {
    #[default]
    Strafe,
    Stay,
}

#[derive(Clone, Debug, Default)]
pub struct Fight {
    style: Style,
    style_until: SimTime,
    /// +1 right, -1 left, 0 none.
    side: f32,
    side_until: SimTime,
    walls: (bool, bool),
    duck_until: SimTime,
    hop_after: SimTime,
    next_check: SimTime,
    reverse: bool,
}

impl Fight {
    pub fn reset(&mut self) {
        *self = Fight::default();
    }

    pub fn update(&mut self, i: &FightInput, s: &FightSkill, tracer: &mut dyn Tracer, rng: &mut Pcg32) -> FightMove {
        let to = (i.enemy - i.origin).truncate();
        let distance = i.enemy.distance(i.origin);
        let forward = to.normalize_or(Vec2::X);
        let right = Vec2::new(forward.y, -forward.x);
        let melee = i.weapon == WeaponClass::Melee;
        let skilled = s.skill >= 50;
        if i.now >= self.style_until {
            self.style = if melee || distance < 768.0 {
                Style::Strafe
            } else if rng.next_f32() < if distance < 1024.0 { s.stay_mid } else { s.stay_far } {
                Style::Stay
            } else {
                Style::Strafe
            };
            let close_gun = matches!(i.weapon, WeaponClass::Pistol | WeaponClass::Shotgun) && distance < 1632.0;
            if i.approach < 30.0 || (close_gun && i.enemy_faces_me) {
                self.style = Style::Strafe;
            }
            if self.style == Style::Strafe && !melee && i.on_ground && rng.next_f32() < s.crouch_tap {
                self.duck_until = i.now + f64::from(rng.range_f32(0.25, 0.5));
            }
            self.style_until = i.now + f64::from(rng.range_f32(1.0, 3.0));
        }
        let mut out = FightMove::default();
        let (mut ahead, mut sideways) = (0.0f32, 0.0f32);
        if self.style == Style::Strafe {
            if i.now >= self.side_until {
                let (sin, cos) = dmath::sin_cos(i.enemy_facing.to_radians());
                let enemy_right = Vec2::new(sin, -cos);
                self.side = if (i.origin - i.enemy).truncate().dot(enemy_right) < 0.0 {
                    1.0
                } else {
                    -1.0
                };
                if rng.next_f32() < 0.3 {
                    self.side = -self.side;
                }
                self.side_until = i.now + f64::from(rng.range_f32(0.3, 0.8));
                let wall = |t: &mut dyn Tracer, dir: Vec2| {
                    let end = i.origin + (dir * WALL_DISTANCE).extend(0.0);
                    t.trace(&TraceQuery::line(i.origin, end)).fraction < 1.0
                };
                self.walls = (wall(tracer, -right), wall(tracer, right));
            }
            let (left_wall, right_wall) = self.walls;
            if self.side < 0.0 && left_wall || self.side > 0.0 && right_wall {
                let other = if self.side < 0.0 { right_wall } else { left_wall };
                self.side = if other { 0.0 } else { -self.side };
            }
            sideways = self.side * i.maxspeed;
            if !melee {
                if skilled && i.approach >= 60.0 && distance > 400.0 {
                    ahead = 0.5 * i.maxspeed;
                } else if skilled && i.approach < 60.0 && distance < 300.0 {
                    ahead = -0.5 * i.maxspeed;
                }
                if left_wall && right_wall {
                    sideways = 0.0;
                    ahead = if skilled { -i.maxspeed } else { 0.0 };
                }
            }
            if let Some(cooldown) = s.dodge_hop_cooldown
                && s.skill >= 25
                && !melee
                && distance < 1088.0
                && i.now >= self.hop_after
                && i.on_ground
                && i.enemy_faces_me
                && !matches!(i.weapon, WeaponClass::Sniper | WeaponClass::Launcher)
            {
                self.hop_after = i.now + f64::from(cooldown * rng.range_f32(0.8, 1.2));
                out.jump = true;
            }
        }
        if melee {
            ahead = i.maxspeed;
        } else if distance < 96.0 {
            ahead = -i.maxspeed;
        }
        if i.reloading {
            ahead = -i.maxspeed;
            self.duck_until = i.now;
        }
        let velocity = forward * ahead + right * sideways;
        if i.now >= self.next_check {
            self.next_check = i.now + CHECK_PERIOD;
            self.reverse = velocity != Vec2::ZERO && drops(tracer, i.origin, velocity);
        }
        out.velocity = if self.reverse { -velocity } else { velocity };
        out.duck = i.now < self.duck_until;
        out
    }
}

/// Moving along `velocity` for a fifth of a second would step off a ledge higher than a safe drop.
pub fn drops(tracer: &mut dyn Tracer, origin: Vec3, velocity: Vec2) -> bool {
    let spot = origin + (velocity * 0.2).extend(0.0);
    let tr = tracer.trace(&TraceQuery::line(spot, spot - Vec3::Z * (SAFE_DROP + 36.0)));
    !tr.start_solid && tr.fraction >= 1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_worldq::{Trace, contents};

    /// Flat floor at z = -36 everywhere, walls where asked.
    struct Floor {
        wall_y: Option<f32>,
    }

    impl Tracer for Floor {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            let mut t = Trace::clear(q.end);
            if let Some(y) = self.wall_y
                && (q.start.y - y) * (q.end.y - y) < 0.0
            {
                t.fraction = 0.5;
            }
            if q.end.z < -36.0 {
                t.fraction = t.fraction.min(0.1);
            }
            t
        }

        fn point_contents(&mut self, _p: Vec3) -> i32 {
            contents::EMPTY
        }
    }

    fn input(now: f64, distance: f32) -> FightInput {
        FightInput {
            now: SimTime(now),
            origin: Vec3::ZERO,
            enemy: Vec3::new(distance, 0.0, 0.0),
            enemy_facing: 180.0,
            enemy_faces_me: true,
            approach: 50.0,
            weapon: WeaponClass::Smg,
            reloading: false,
            on_ground: true,
            maxspeed: 300.0,
        }
    }

    const SKILL: FightSkill = FightSkill {
        skill: 50,
        stay_mid: 0.2,
        stay_far: 0.45,
        crouch_tap: 0.0,
        dodge_hop_cooldown: None,
    };

    #[test]
    fn strafes_close_and_turns_at_walls() {
        let mut f = Fight::default();
        let mut rng = Pcg32::new(1, 1);
        let mut open = Floor { wall_y: None };
        let m = f.update(&input(0.0, 400.0), &SKILL, &mut open, &mut rng);
        assert!(
            m.velocity.y.abs() > 250.0 && m.velocity.x.abs() < 1.0,
            "sideways: {m:?}"
        );
        // A wall 100 units to the side the bot strafes to: the next decision turns it around.
        let side = m.velocity.y.signum();
        let mut walled = Floor {
            wall_y: Some(side * 100.0),
        };
        let mut f = Fight::default();
        let mut rng = Pcg32::new(1, 1);
        let m = f.update(&input(0.0, 400.0), &SKILL, &mut walled, &mut rng);
        assert!(m.velocity.y * side <= 0.0, "{m:?}");
    }

    #[test]
    fn backs_off_point_blank_and_while_reloading() {
        let mut f = Fight::default();
        let mut rng = Pcg32::new(2, 2);
        let mut open = Floor { wall_y: None };
        let m = f.update(&input(0.0, 80.0), &SKILL, &mut open, &mut rng);
        assert!(m.velocity.x < -250.0, "{m:?}");
        let mut i = input(0.0, 600.0);
        i.reloading = true;
        let m = f.update(&i, &SKILL, &mut open, &mut rng);
        assert!(m.velocity.x < -250.0, "{m:?}");
        let mut melee = input(0.0, 300.0);
        melee.weapon = WeaponClass::Melee;
        let m = f.update(&melee, &SKILL, &mut open, &mut rng);
        assert!(m.velocity.x > 250.0, "charge: {m:?}");
    }

    #[test]
    fn never_steps_off_a_ledge() {
        struct Cliff;
        impl Tracer for Cliff {
            fn trace(&mut self, q: &TraceQuery) -> Trace {
                let mut t = Trace::clear(q.end);
                // Floor only where x > -10: backing off would fall.
                if q.end.z < -36.0 && q.start.x > -10.0 {
                    t.fraction = 0.1;
                }
                t
            }
            fn point_contents(&mut self, _p: Vec3) -> i32 {
                contents::EMPTY
            }
        }
        let mut f = Fight::default();
        let mut rng = Pcg32::new(3, 3);
        let m = f.update(&input(0.0, 80.0), &SKILL, &mut Cliff, &mut rng);
        assert!(m.velocity.x > 0.0, "reversed at the edge: {m:?}");
    }
}
