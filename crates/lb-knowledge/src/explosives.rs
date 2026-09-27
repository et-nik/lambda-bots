//! What a bot knows of explosives around it.
//!
//! - **Its own satchels:** where it threw them (the solved landing point), moved to where it sees them; gone when
//!   they go off or when the bot dies (the game removes a dead player's satchels).
//! - **Tripmines:** its own where it placed them, and any it has seen (the mine or its beam) with the beam's line;
//!   forgotten when an explosion goes off at the mine.
//! - **Projectiles** in flight or lying about that it sees: where they are and how they move, and for grenades,
//!   rockets and satchels where they are going to blow up.

use lb_core::Vec3;
use lb_core::time::SimTime;
use lb_game::entities::ProjectileKind;
use lb_game::mechanics::{PROJECTILE_GRAVITY, TRIPMINE_ARM};

/// A projectile or placed explosive in view on this vision tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectileSighting {
    pub t: SimTime,
    pub kind: ProjectileKind,
    /// Entity index, to tell one from another while in view.
    pub index: u16,
    pub pos: Vec3,
    pub vel: Vec3,
    /// Thrown or placed by the bot itself.
    pub own: bool,
    /// A tripmine's beam: its direction and where it ends.
    pub beam: Option<(Vec3, Vec3)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flying {
    pub kind: ProjectileKind,
    pub index: u16,
    pub pos: Vec3,
    pub vel: Vec3,
    pub own: bool,
    pub seen: SimTime,
    /// Speed when first seen: a grenade from the MP5 leaves at 800 and explodes on contact, a hand grenade is slower.
    pub launch_speed: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Charge {
    pub pos: Vec3,
    pub since: SimTime,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mine {
    pub pos: Vec3,
    /// Direction of the beam (the wall's normal).
    pub dir: Vec3,
    pub beam_end: Option<Vec3>,
    pub own: bool,
    pub armed_at: SimTime,
    pub seen: SimTime,
}

/// A predicted explosion: where, how far it reaches, and roughly when.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blast {
    pub at: Vec3,
    pub radius: f32,
    pub kind: ProjectileKind,
}

/// A seen projectile is followed for this long after it was last in view.
const FLYING_MEMORY: f64 = 0.4;
/// A satchel or snark lying about is remembered longer.
const LYING_MEMORY: f64 = 10.0;
/// Two sightings of explosives this close are the same one.
const SAME_SPOT: f32 = 24.0;
/// An own satchel seen this close to where one was thrown is that one.
const OWN_CHARGE_MATCH: f32 = 200.0;
/// An explosion this close to a mine or charge set it off.
const BLOWN_WITH: f32 = 64.0;
/// Faster than this when first seen: an MP5 grenade.
const CONTACT_GRENADE: f32 = 650.0;
const GRENADE_RADIUS: f32 = 250.0;
const ROCKET_RADIUS: f32 = 300.0;
const SATCHEL_RADIUS: f32 = 300.0;
/// A rocket passing this close is coming for the bot.
const ROCKET_CLOSE: f32 = 160.0;

#[derive(Clone, Debug, Default)]
pub struct Explosives {
    pub charges: Vec<Charge>,
    pub mines: Vec<Mine>,
    pub flying: Vec<Flying>,
}

impl Explosives {
    pub fn thrown_satchel(&mut self, landing: Vec3, now: SimTime) {
        self.charges.push(Charge {
            pos: landing,
            since: now,
        });
    }

    /// The bot's satchels went off.
    pub fn detonated(&mut self) {
        self.charges.clear();
    }

    pub fn placed_mine(&mut self, pos: Vec3, dir: Vec3, now: SimTime) {
        self.mines.retain(|m| m.pos.distance(pos) > SAME_SPOT);
        self.mines.push(Mine {
            pos,
            dir,
            beam_end: None,
            own: true,
            armed_at: now + f64::from(TRIPMINE_ARM),
            seen: now,
        });
    }

    pub fn on_sighting(&mut self, s: &ProjectileSighting) {
        match s.kind {
            ProjectileKind::Satchel if s.own => {
                let nearest = self
                    .charges
                    .iter_mut()
                    .filter(|c| c.pos.distance(s.pos) <= OWN_CHARGE_MATCH)
                    .min_by(|a, b| a.pos.distance(s.pos).total_cmp(&b.pos.distance(s.pos)));
                match nearest {
                    Some(c) => c.pos = s.pos,
                    None => self.charges.push(Charge { pos: s.pos, since: s.t }),
                }
            }
            ProjectileKind::Tripmine => {
                let (dir, end) = s.beam.map_or((Vec3::ZERO, None), |(d, e)| (d, Some(e)));
                match self.mines.iter_mut().find(|m| m.pos.distance(s.pos) <= SAME_SPOT) {
                    Some(m) => {
                        m.seen = s.t;
                        if end.is_some() {
                            m.dir = dir;
                            m.beam_end = end;
                        }
                    }
                    None => self.mines.push(Mine {
                        pos: s.pos,
                        dir,
                        beam_end: end,
                        own: s.own,
                        armed_at: s.t,
                        seen: s.t,
                    }),
                }
            }
            _ => match self.flying.iter_mut().find(|f| f.index == s.index && f.kind == s.kind) {
                Some(f) => {
                    f.pos = s.pos;
                    f.vel = s.vel;
                    f.seen = s.t;
                }
                None => self.flying.push(Flying {
                    kind: s.kind,
                    index: s.index,
                    pos: s.pos,
                    vel: s.vel,
                    own: s.own,
                    seen: s.t,
                    launch_speed: s.vel.length(),
                }),
            },
        }
    }

    /// An explosion was seen or heard at `pos`: what lay there is gone.
    pub fn on_explosion(&mut self, pos: Vec3) {
        self.mines.retain(|m| m.pos.distance(pos) > BLOWN_WITH);
        self.charges.retain(|c| c.pos.distance(pos) > BLOWN_WITH);
        self.flying.retain(|f| f.pos.distance(pos) > BLOWN_WITH);
    }

    /// The bot died: the game removes its satchels.
    pub fn on_own_death(&mut self) {
        self.charges.clear();
        self.flying.clear();
    }

    pub fn update(&mut self, now: SimTime) {
        self.flying.retain(|f| {
            let memory = match f.kind {
                ProjectileKind::Satchel | ProjectileKind::Snark => LYING_MEMORY,
                _ => FLYING_MEMORY,
            };
            now.since(f.seen) <= memory
        });
    }

    /// Explosions others' projectiles the bot has seen are about to set off, `sv_gravity` for their fall.
    pub fn blasts(&self, now: SimTime, sv_gravity: f32, floor: f32) -> impl Iterator<Item = Blast> + '_ {
        self.flying
            .iter()
            .filter(|f| !f.own)
            .filter_map(move |f| predict(f, now, sv_gravity * PROJECTILE_GRAVITY, floor))
    }

    /// A rocket seen flying at `me` passes this close; `None` when none comes near.
    pub fn rocket_at(&self, me: Vec3) -> Option<Blast> {
        self.flying
            .iter()
            .filter(|f| !f.own && f.kind == ProjectileKind::Rocket)
            .filter_map(|f| {
                let dir = f.vel.normalize_or_zero();
                let along = (me - f.pos).dot(dir);
                let closest = f.pos + dir * along.max(0.0);
                (along > 0.0 && closest.distance(me) <= ROCKET_CLOSE).then_some(Blast {
                    at: closest,
                    radius: ROCKET_RADIUS,
                    kind: ProjectileKind::Rocket,
                })
            })
            .next()
    }
}

/// Where a projectile blows up: a grenade where it comes down to `floor` (bouncing a little further on), an MP5
/// grenade where it lands, a satchel where it lies.
fn predict(f: &Flying, now: SimTime, gravity: f32, floor: f32) -> Option<Blast> {
    let elapsed = now.since(f.seen) as f32;
    let pos = f.pos + f.vel * elapsed - Vec3::Z * (0.5 * gravity * elapsed * elapsed);
    let vel = f.vel - Vec3::Z * (gravity * elapsed);
    match f.kind {
        ProjectileKind::Grenade => {
            let (a, b, c) = (-0.5 * gravity, vel.z, pos.z - floor);
            let disc = b * b - 4.0 * a * c;
            let t = if disc >= 0.0 && gravity > 0.0 {
                ((-b - disc.sqrt()) / (2.0 * a)).max(0.0)
            } else {
                0.0
            };
            let mut at = pos + vel.truncate().extend(0.0) * t;
            at.z = floor.min(pos.z);
            if f.launch_speed <= CONTACT_GRENADE {
                // A hand grenade bounces and rolls on a little.
                at += vel.truncate().extend(0.0) * 0.2;
            }
            Some(Blast {
                at,
                radius: GRENADE_RADIUS,
                kind: f.kind,
            })
        }
        ProjectileKind::Satchel => Some(Blast {
            at: pos,
            radius: SATCHEL_RADIUS,
            kind: f.kind,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(kind: ProjectileKind, index: u16, pos: Vec3, vel: Vec3, own: bool, t: f64) -> ProjectileSighting {
        ProjectileSighting {
            t: SimTime(t),
            kind,
            index,
            pos,
            vel,
            own,
            beam: None,
        }
    }

    #[test]
    fn own_satchels_move_to_where_they_are_seen_and_go_off() {
        let mut e = Explosives::default();
        e.thrown_satchel(Vec3::new(100.0, 0.0, 0.0), SimTime(1.0));
        e.on_sighting(&seen(
            ProjectileKind::Satchel,
            40,
            Vec3::new(180.0, 20.0, 0.0),
            Vec3::ZERO,
            true,
            1.5,
        ));
        assert_eq!(e.charges.len(), 1);
        assert_eq!(e.charges[0].pos, Vec3::new(180.0, 20.0, 0.0));
        e.detonated();
        assert!(e.charges.is_empty());
    }

    #[test]
    fn mines_are_remembered_until_they_blow() {
        let mut e = Explosives::default();
        let mut s = seen(
            ProjectileKind::Tripmine,
            50,
            Vec3::new(0.0, 100.0, 0.0),
            Vec3::ZERO,
            false,
            1.0,
        );
        s.beam = Some((Vec3::NEG_Y, Vec3::new(0.0, -100.0, 0.0)));
        e.on_sighting(&s);
        e.on_sighting(&s);
        assert_eq!(e.mines.len(), 1);
        assert_eq!(e.mines[0].beam_end, Some(Vec3::new(0.0, -100.0, 0.0)));
        e.update(SimTime(100.0));
        assert_eq!(e.mines.len(), 1, "mines stay where they are");
        e.on_explosion(Vec3::new(10.0, 100.0, 0.0));
        assert!(e.mines.is_empty());
    }

    #[test]
    fn grenades_and_rockets_are_seen_coming() {
        let mut e = Explosives::default();
        // Lobbed from 600 units away toward the bot at the origin, standing on a floor at z = -36.
        e.on_sighting(&seen(
            ProjectileKind::Grenade,
            60,
            Vec3::new(600.0, 0.0, 30.0),
            Vec3::new(-500.0, 0.0, 150.0),
            false,
            1.0,
        ));
        let blast = e.blasts(SimTime(1.0), 800.0, -36.0).next().unwrap();
        assert!(blast.at.x.abs() < 150.0, "comes down near the bot: {blast:?}");
        e.on_sighting(&seen(
            ProjectileKind::Rocket,
            61,
            Vec3::new(1000.0, 50.0, 0.0),
            Vec3::new(-2000.0, 0.0, 0.0),
            false,
            1.0,
        ));
        assert!(e.rocket_at(Vec3::ZERO).is_some());
        assert!(e.rocket_at(Vec3::new(0.0, 600.0, 0.0)).is_none());
        e.update(SimTime(2.0));
        assert!(e.flying.is_empty(), "out of view, forgotten");
    }
}
