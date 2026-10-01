//! Movement in a fight, after yapb's `attackMovement`.
//!
//! - **Style:** every 1–3 s the bot picks strafing or standing. Closer than 768 units it strafes; further away it
//!   stands with the skill's `stay_mid` / `stay_far` chance. A low will to approach (health × aggression below
//!   30), or a pistol or shotgun against an enemy facing the bot, makes it strafe.
//! - **Strafe side:** away from the side the enemy aims at, swapped 30% of the time, re-decided every 0.3–0.8 s, down
//!   to every 0.2–0.45 s for the best.
//!   Walls within 134 units on a side turn it around; with walls that close on both sides it strafes toward the one
//!   farther off while there is room, and only in a corridor too narrow for that does it go back and forth instead.
//! - **Distance:** further off than the weapon in hand does well at ([`close_in`]), a bot with the will to (health ×
//!   aggression 30 or more) closes in at a run, along the way there when navigation gives one, strafing as it goes;
//!   it does not stand then. Otherwise skilled bots drift in when they feel strong and far, back off when weak and
//!   close, and back off when cornered. Everyone backs off under 96 units, while reloading and closer than the fight
//!   allows (its own weapon's blast, a GunGame player one kill from winning); melee charges. Nobody closes in while
//!   its own rocket or launched grenade is on the way to the target.
//! - **Extras:** crouch taps and dodge jumps by skill; a dodge jump when the enemy aims at the bot or the bot is being
//!   hit.
//! - **Ledges:** a move that would drop more than 160 units is reversed.
//! - **Stuck:** a move on the ground that hardly gets anywhere for a third of a second (a box the wall traces pass
//!   over, a player, the wall behind a ledge turned from) is backed out of for a moment, and the strafe goes the other
//!   way; aside rather than at the enemy while keeping away from it.

use lb_core::dmath;
use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3};
use lb_game::mechanics::WeaponClass;
use lb_game::weapons::WeaponId;
use lb_worldq::{TraceQuery, Tracer};

const WALL_DISTANCE: f32 = 134.0;
/// Closing in takes this much will (health × aggression) at least; with less the bot keeps its distance, as yapb's.
pub const PUSH_WILL: f32 = 30.0;
/// Strafing while closing in, as a share of the run.
const PUSH_STRAFE: f32 = 0.6;
const SAFE_DROP: f32 = 160.0;
const CHECK_PERIOD: f64 = 0.1;
/// With walls close on both sides, a strafe toward the farther one needs this much room, and stops this short of it.
const ROOM_MIN: f32 = 64.0;
const ROOM_KEEP: f32 = 24.0;
/// A move asked this fast that gets less than this share of it, this long, is stuck; it is backed out of this long.
const STUCK_ASK: f32 = 100.0;
const STUCK_SHARE: f32 = 0.25;
const STUCK_FOR: f64 = 0.35;
const ESCAPE_FOR: f64 = 0.3;
/// Backing out while keeping away from the enemy goes no more toward it than this (a cosine).
const KEEP_OFF_TOWARD: f32 = 0.2;

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
    /// The bot was hurt a moment ago: someone has it in their sights.
    pub under_fire: bool,
    /// Health × aggression, 0..100.
    pub approach: f32,
    pub weapon: WeaponClass,
    pub reloading: bool,
    /// The bot's own explosive is on its way to the target: closing in would take the bot into the blast.
    pub hold_ground: bool,
    /// ... and the target is close: backing off keeps the bot out of the blast.
    pub back_off: bool,
    pub on_ground: bool,
    /// How the bot moves now, horizontally.
    pub velocity: Vec2,
    pub maxspeed: f32,
    /// Further off than this the weapon in hand does poorly ([`close_in`]).
    pub close_in: f32,
    /// Closer than this the bot backs off: its own weapon's blast would reach it there, or the enemy is one to keep
    /// off.
    pub keep_away: f32,
    /// The way toward the enemy along the navigation path, when there is one.
    pub path: Option<Vec2>,
}

/// How far off `weapon` still does well: pellets and bullets spread, darts home in close, beams are hard to hold on
/// a target far off. Guns that hit as well far off (the 357, the crossbow, the RPG) never close in.
pub fn close_in(weapon: Option<WeaponId>) -> f32 {
    match weapon {
        Some(WeaponId::Shotgun) => 350.0,
        Some(WeaponId::Hornetgun | WeaponId::Egon) => 600.0,
        Some(WeaponId::Glock | WeaponId::Mp5) => 700.0,
        Some(WeaponId::Gauss) => 1200.0,
        _ => f32::INFINITY,
    }
}

impl FightInput {
    /// Far off for the weapon in hand, with the will to close in and nothing holding the bot back.
    pub fn wants_closer(&self, distance: f32) -> bool {
        self.weapon != WeaponClass::Melee
            && !self.reloading
            && !self.hold_ground
            && self.approach >= PUSH_WILL
            && distance > self.close_in
    }
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
    /// Clear room to the left and to the right at the last look, up to `WALL_DISTANCE`.
    room: (f32, f32),
    duck_until: SimTime,
    hop_after: SimTime,
    next_check: SimTime,
    reverse: bool,
    /// The move asked last, and since when it has hardly got anywhere.
    asked: Vec2,
    stuck_since: Option<SimTime>,
    /// Backing out of a stuck move: until when, and which way.
    escape: Option<(SimTime, Vec2)>,
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
        let push = i.wants_closer(distance);
        if let Some(out) = self.unstick(i, tracer, rng) {
            return out;
        }
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
        if push {
            self.style = Style::Strafe;
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
                // The better the bot, the less a strafe can be read and led.
                let quick = f32::from(s.skill.min(100)) / 100.0;
                self.side_until = i.now + f64::from(rng.range_f32(0.3 - 0.1 * quick, 0.8 - 0.35 * quick));
                let room = |t: &mut dyn Tracer, dir: Vec2| {
                    let end = i.origin + (dir * WALL_DISTANCE).extend(0.0);
                    t.trace(&TraceQuery::line(i.origin, end)).fraction * WALL_DISTANCE
                };
                self.room = (room(tracer, -right), room(tracer, right));
                let (left, right) = self.room;
                let walled = |room: f32| room < WALL_DISTANCE;
                if walled(left) && walled(right) && left.max(right) >= ROOM_MIN {
                    // Toward the farther wall, stopping short of it.
                    self.side = if right >= left { 1.0 } else { -1.0 };
                    let secs = (left.max(right) - ROOM_KEEP) / i.maxspeed.max(1.0);
                    self.side_until = self.side_until.min(i.now + f64::from(secs));
                }
            }
            let (left, right) = self.room;
            let (left_wall, right_wall) = (left < WALL_DISTANCE, right < WALL_DISTANCE);
            let cornered = left_wall && right_wall && left.max(right) < ROOM_MIN;
            if !cornered && (self.side < 0.0 && left_wall || self.side > 0.0 && right_wall) {
                let other = if self.side < 0.0 { right_wall } else { left_wall };
                if !other {
                    self.side = -self.side;
                }
            }
            sideways = self.side * i.maxspeed;
            if !melee {
                if push {
                    ahead = i.maxspeed;
                    sideways *= PUSH_STRAFE;
                } else if skilled && i.approach >= 60.0 && distance > 400.0 {
                    ahead = 0.5 * i.maxspeed;
                } else if skilled && i.approach < 60.0 && distance < 300.0 {
                    ahead = -0.5 * i.maxspeed;
                }
                if cornered {
                    sideways = 0.0;
                    ahead = if push {
                        i.maxspeed
                    } else if skilled {
                        -i.maxspeed
                    } else {
                        -0.5 * i.maxspeed
                    };
                }
            }
            if let Some(cooldown) = s.dodge_hop_cooldown
                && s.skill >= 25
                && !melee
                && distance < 1088.0
                && i.now >= self.hop_after
                && i.on_ground
                && (i.enemy_faces_me || i.under_fire)
                && !matches!(i.weapon, WeaponClass::Sniper | WeaponClass::Launcher)
            {
                self.hop_after = i.now + f64::from(cooldown * rng.range_f32(0.8, 1.2));
                out.jump = true;
            }
        }
        if melee {
            ahead = i.maxspeed;
        } else if distance < 96.0 || distance < i.keep_away {
            ahead = -i.maxspeed;
        }
        if i.hold_ground {
            ahead = ahead.min(0.0);
        }
        if i.back_off {
            ahead = -i.maxspeed;
        }
        if i.reloading {
            ahead = -i.maxspeed;
            self.duck_until = i.now;
        }
        // Closing in goes the way navigation gives (round walls and drops), the strafe across it.
        let (forward, right) = match i.path.filter(|p| push && ahead > 0.0 && *p != Vec2::ZERO) {
            Some(p) => {
                let p = p.normalize();
                (p, Vec2::new(p.y, -p.x))
            }
            None => (forward, right),
        };
        let velocity = forward * ahead + right * sideways;
        if i.now >= self.next_check {
            self.next_check = i.now + CHECK_PERIOD;
            self.reverse = velocity != Vec2::ZERO && drops(tracer, i.origin, velocity);
        }
        out.velocity = if self.reverse { -velocity } else { velocity };
        out.duck = i.now < self.duck_until;
        self.asked = out.velocity;
        out
    }

    /// A move stuck on the ground is backed out of (sideways where straight back is a drop), and the strafe turns
    /// the other way; `Some` while backing out.
    fn unstick(&mut self, i: &FightInput, tracer: &mut dyn Tracer, rng: &mut Pcg32) -> Option<FightMove> {
        if let Some((until, dir)) = self.escape {
            if i.now < until {
                self.asked = dir * i.maxspeed;
                return Some(FightMove {
                    velocity: self.asked,
                    ..FightMove::default()
                });
            }
            self.escape = None;
        }
        let asked = self.asked.length();
        if !i.on_ground || asked < STUCK_ASK || i.velocity.length() >= STUCK_SHARE * asked {
            self.stuck_since = None;
            return None;
        }
        let since = *self.stuck_since.get_or_insert(i.now);
        if i.now.since(since) < STUCK_FOR {
            return None;
        }
        self.stuck_since = None;
        self.side = -self.side;
        self.side_until = i.now + f64::from(rng.range_f32(0.5, 1.0));
        let back = -self.asked / asked;
        // Not at the enemy while keeping away from it (its own blast on the way, a reload): aside then.
        let toward = (i.enemy - i.origin).truncate().normalize_or_zero();
        let keep_off = i.hold_ground || i.back_off || i.reloading || i.enemy.distance(i.origin) < i.keep_away;
        let way = [back, Vec2::new(-back.y, back.x), Vec2::new(back.y, -back.x)]
            .into_iter()
            .filter(|d| !keep_off || d.dot(toward) <= KEEP_OFF_TOWARD)
            .find(|d| !drops(tracer, i.origin, *d * i.maxspeed))?;
        self.escape = Some((i.now + ESCAPE_FOR, way));
        self.unstick(i, tracer, rng)
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
            under_fire: false,
            approach: 50.0,
            weapon: WeaponClass::Smg,
            reloading: false,
            hold_ground: false,
            back_off: false,
            on_ground: true,
            velocity: Vec2::ZERO,
            maxspeed: 300.0,
            close_in: f32::INFINITY,
            keep_away: 0.0,
            path: None,
        }
    }

    /// Flat floor at z = -36, walls along the x axis at the given y, traced to where a line meets them.
    struct Walls(Vec<f32>);

    impl Tracer for Walls {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            let mut t = Trace::clear(q.end);
            for &y in &self.0 {
                if (q.start.y - y) * (q.end.y - y) < 0.0 {
                    t.fraction = t.fraction.min((y - q.start.y) / (q.end.y - q.start.y));
                }
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
    fn keeps_out_of_its_own_rockets_way() {
        let mut open = Floor { wall_y: None };
        let strong = |hold_ground| FightInput {
            approach: 90.0,
            hold_ground,
            ..input(0.0, 700.0)
        };
        let mut f = Fight::default();
        let m = f.update(&strong(false), &SKILL, &mut open, &mut Pcg32::new(5, 5));
        assert!(m.velocity.x > 100.0, "a strong bot drifts in: {m:?}");
        let mut f = Fight::default();
        let m = f.update(&strong(true), &SKILL, &mut open, &mut Pcg32::new(5, 5));
        assert!(m.velocity.x <= 0.0, "not while its rocket flies: {m:?}");
        let close = FightInput {
            back_off: true,
            ..strong(true)
        };
        let m = f.update(&close, &SKILL, &mut open, &mut Pcg32::new(5, 5));
        assert!(m.velocity.x < -200.0, "away from its rocket's blast close by: {m:?}");
    }

    #[test]
    fn backs_off_from_what_it_keeps_away_from() {
        let mut open = Floor { wall_y: None };
        let keep = FightInput {
            keep_away: 350.0,
            approach: 90.0,
            ..input(0.0, 250.0)
        };
        let mut f = Fight::default();
        let m = f.update(&keep, &SKILL, &mut open, &mut Pcg32::new(6, 6));
        assert!(m.velocity.x < -250.0, "back to where it keeps the enemy: {m:?}");
        let out = FightInput {
            keep_away: 350.0,
            ..input(0.0, 400.0)
        };
        let m = f.update(&out, &SKILL, &mut open, &mut Pcg32::new(6, 6));
        assert!(m.velocity.x > -200.0, "far enough, the fight goes on as ever: {m:?}");
    }

    #[test]
    fn closes_in_when_the_weapon_wants_it_closer() {
        let mut open = Floor { wall_y: None };
        let shotgun = |distance: f32, approach: f32| FightInput {
            approach,
            weapon: WeaponClass::Shotgun,
            close_in: close_in(Some(WeaponId::Shotgun)),
            ..input(0.0, distance)
        };
        // Far off with a shotgun: at a run, strafing a little, and never standing (even where the skill stays).
        let stays = FightSkill { stay_far: 1.0, ..SKILL };
        let mut f = Fight::default();
        let m = f.update(&shotgun(1200.0, 50.0), &stays, &mut open, &mut Pcg32::new(4, 4));
        assert!(m.velocity.x > 250.0 && m.velocity.y.abs() > 100.0, "{m:?}");
        // Close enough, or without the will (hurt or timid): strafing where it is.
        for i in [shotgun(300.0, 50.0), shotgun(1200.0, 20.0)] {
            let mut f = Fight::default();
            let m = f.update(&i, &SKILL, &mut open, &mut Pcg32::new(4, 4));
            assert!(m.velocity.x.abs() < 1.0, "{m:?}");
        }
        // The way there goes round a wall: along it.
        let round = FightInput {
            path: Some(Vec2::new(0.0, 1.0)),
            ..shotgun(1200.0, 50.0)
        };
        let mut f = Fight::default();
        let m = f.update(&round, &SKILL, &mut open, &mut Pcg32::new(4, 4));
        assert!(m.velocity.y > 250.0, "{m:?}");
        // A crossbow does as well far off.
        assert_eq!(close_in(Some(WeaponId::Crossbow)), f32::INFINITY);
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

    #[test]
    fn in_a_corridor_it_strafes_toward_the_farther_wall_and_only_goes_back_and_forth_in_a_narrow_one() {
        // The enemy ahead along x: the bot's left is +y. Walls 100 units to the left and 60 to the right.
        let mut f = Fight::default();
        let m = f.update(
            &input(0.0, 400.0),
            &SKILL,
            &mut Walls(vec![100.0, -60.0]),
            &mut Pcg32::new(1, 1),
        );
        assert!(m.velocity.y > 250.0 && m.velocity.x.abs() < 1.0, "{m:?}");
        assert!(
            f.side_until <= SimTime((100.0 - f64::from(ROOM_KEEP)) / 300.0 + 1e-6),
            "stops short of the wall"
        );
        // Too narrow to strafe: a skilled bot backs off, an unskilled one too, slower (it used to stand).
        let narrow = |skill: u8| {
            let mut f = Fight::default();
            let s = FightSkill { skill, ..SKILL };
            f.update(
                &input(0.0, 400.0),
                &s,
                &mut Walls(vec![50.0, -50.0]),
                &mut Pcg32::new(1, 1),
            )
        };
        let (skilled, unskilled) = (narrow(50), narrow(30));
        assert!(
            skilled.velocity.x < -250.0 && skilled.velocity.y.abs() < 1.0,
            "{skilled:?}"
        );
        assert!(
            unskilled.velocity.x < -100.0 && unskilled.velocity.y.abs() < 1.0,
            "{unskilled:?}"
        );
    }

    #[test]
    fn a_strafe_that_gets_nowhere_is_backed_out_of_and_turned() {
        let mut f = Fight::default();
        let mut rng = Pcg32::new(1, 1);
        let mut open = Floor { wall_y: None };
        let first = f.update(&input(0.0, 400.0), &SKILL, &mut open, &mut rng);
        let side = first.velocity.y.signum();
        assert!(first.velocity.y.abs() > 250.0, "{first:?}");
        // Held where it is (a box the wall traces pass over): out the other way within half a second.
        let mut out = None;
        for k in 1..=6 {
            let m = f.update(&input(f64::from(k) * 0.1, 400.0), &SKILL, &mut open, &mut rng);
            if m.velocity.y * side < -250.0 {
                out = Some(k);
                break;
            }
        }
        assert!(out.is_some_and(|k| k <= 5), "{out:?}");
        // Moving freely again after backing out, it strafes on that way.
        let free = FightInput {
            velocity: Vec2::new(0.0, -side * 300.0),
            ..input(1.0, 400.0)
        };
        let m = f.update(&free, &SKILL, &mut open, &mut rng);
        assert!(m.velocity.y * side < -250.0, "{m:?}");
        // In the air a slow move is no sign of being stuck.
        let mut f = Fight::default();
        let first = f.update(&input(0.0, 400.0), &SKILL, &mut open, &mut rng);
        for k in 1..=6 {
            let flying = FightInput {
                on_ground: false,
                ..input(f64::from(k) * 0.1, 400.0)
            };
            let m = f.update(&flying, &SKILL, &mut open, &mut rng);
            assert!(m.velocity.dot(first.velocity) > -1.0 || f.escape.is_none(), "{m:?}");
        }
        assert!(f.escape.is_none());
    }

    #[test]
    fn backing_off_from_its_own_blast_it_never_backs_out_toward_the_enemy() {
        let mut f = Fight::default();
        let mut rng = Pcg32::new(1, 1);
        let mut open = Floor { wall_y: None };
        let away = |now: f64| FightInput {
            back_off: true,
            ..input(now, 300.0)
        };
        let first = f.update(&away(0.0), &SKILL, &mut open, &mut rng);
        assert!(first.velocity.x < -250.0, "{first:?}");
        // A wall behind: held where it is, it gets out sideways, not back at the enemy and its rocket's blast.
        let mut backed_out = false;
        for k in 1..=8 {
            let m = f.update(&away(f64::from(k) * 0.1), &SKILL, &mut open, &mut rng);
            assert!(m.velocity.x <= 0.2 * 300.0 + 1.0, "{k}: {m:?}");
            backed_out |= f.escape.is_some();
        }
        assert!(backed_out);
    }

    #[test]
    fn a_bot_being_hit_dodges_whoever_aims_at_it() {
        let hops = FightSkill {
            dodge_hop_cooldown: Some(1.0),
            ..SKILL
        };
        // The enemy seems to look elsewhere (its facing is a noisy guess).
        let unaimed = |now: f64, under_fire: bool| FightInput {
            enemy_faces_me: false,
            under_fire,
            ..input(now, 500.0)
        };
        let jumps = |under_fire: bool| {
            let mut f = Fight::default();
            let mut rng = Pcg32::new(8, 8);
            let mut open = Floor { wall_y: None };
            (0..30)
                .filter(|&k| {
                    f.update(&unaimed(f64::from(k) * 0.1, under_fire), &hops, &mut open, &mut rng)
                        .jump
                })
                .count()
        };
        assert_eq!(jumps(false), 0);
        assert!(jumps(true) >= 2, "{}", jumps(true));
    }

    #[test]
    fn the_best_decide_their_strafe_more_often() {
        // Seconds a strafe side is kept before it is decided again, on average.
        let mean_side = |skill: u8| {
            let s = FightSkill { skill, ..SKILL };
            let mut f = Fight::default();
            let mut rng = Pcg32::new(3, 3);
            let mut open = Floor { wall_y: None };
            let mut decided = 0;
            let mut until = SimTime::ZERO;
            // The bot moves as asked.
            let mut velocity = Vec2::ZERO;
            for k in 0..600 {
                let i = FightInput {
                    velocity,
                    ..input(f64::from(k) * 0.01, 500.0)
                };
                velocity = f.update(&i, &s, &mut open, &mut rng).velocity;
                if f.side_until != until {
                    decided += 1;
                    until = f.side_until;
                }
            }
            6.0 / f64::from(decided)
        };
        let (best, worst) = (mean_side(100), mean_side(0));
        assert!(
            (0.2..0.45).contains(&best) && (0.3..0.8).contains(&worst) && best < worst * 0.7,
            "{best} {worst}"
        );
    }
}
