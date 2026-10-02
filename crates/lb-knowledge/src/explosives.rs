//! What a bot knows of explosives around it.
//!
//! - **Its own satchels:** where it threw them (the solved landing point), moved to where it sees them, flying or
//!   lying (each by its entity once seen); gone when they go off or when the bot dies (the game removes a dead
//!   player's satchels).
//! - **Tripmines:** its own where it placed them, and any it has seen (the mine or its beam) with the beam's line,
//!   where it is seen; forgotten when an explosion goes off at the mine (the game bursts it out along the way the
//!   mine faces) and when it is not there where the bot looks for it.
//! - **Projectiles** in flight or lying about that it sees: where they are and how they move, and for grenades,
//!   rockets and satchels where they are going to blow up.
//! - **Its own hand grenades:** where each should come down and when it goes off (the bot pulled the pin), seen or
//!   not, until then.
//! - **Hand grenades heard bouncing:** where the last bounce seemed to be, as well as the ear tells it, for a while
//!   after; whoever threw them, the bot's own that came back off a wall out of sight among them.

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
    /// A tripmine's beam is on: it is armed.
    pub armed: bool,
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
    /// The satchel's entity, once seen.
    pub index: Option<u16>,
    /// How it moved and when, when last seen.
    pub vel: Vec3,
    pub seen: Option<SimTime>,
}

impl Charge {
    /// Where it is at `now`: where it was seen, carried on by its motion for a moment (a satchel falls at half
    /// gravity).
    pub fn at(&self, now: SimTime, sv_gravity: f32) -> Vec3 {
        let Some(seen) = self.seen else { return self.pos };
        let dt = (now.since(seen) as f32).clamp(0.0, CHARGE_CARRY);
        self.pos + self.vel * dt - Vec3::Z * (0.5 * sv_gravity * PROJECTILE_GRAVITY * dt * dt)
    }
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
    /// When navigation was last told to keep off the beam.
    pub avoided_at: Option<SimTime>,
    /// How a player gets past the beam, once its line was looked along.
    pub pass: Option<BeamPass>,
}

/// How a player gets past a tripmine's beam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeamPass {
    /// It stands up from a mine on the floor: walked round, with room beside it.
    Around,
    /// High enough over the floor to duck under.
    Under,
    /// Neither: the way keeps off it.
    Blocked,
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
/// An own satchel not yet seen is taken for the first one seen within this of where it should land, or thrown this
/// recently (it is still flying there).
const OWN_CHARGE_MATCH: f32 = 200.0;
const OWN_CHARGE_FLYING: f64 = 3.0;
/// A seen satchel's motion is carried on this long at most.
const CHARGE_CARRY: f32 = 0.2;
/// A satchel first seen this shortly before its throw is noted is the one thrown (the game shows it at once).
const JUST_THROWN: f64 = 0.5;
/// An explosion this close to a mine or charge set it off.
const BLOWN_WITH: f32 = 64.0;
/// A tripmine bursts out along the way it faces ((damage − 24) × 0.6 off its wall, some 68 units past the mine at 150
/// damage): an explosion this close to that line, this long, was the mine going off.
const MINE_BURST_LINE: f32 = 96.0;
const MINE_BURST_SLACK: f32 = 32.0;
/// A mine that moved this much on a sighting has its beam told to navigation again.
const MINE_MOVED: f32 = 4.0;
/// A mine seen with its beam off is not taken for armed before the next look.
const ARMING_LOOK: f64 = 0.05;
/// Faster than this when first seen: an MP5 grenade.
const CONTACT_GRENADE: f32 = 650.0;
const GRENADE_RADIUS: f32 = 250.0;
/// An own grenade is kept this long past when it should have gone off (the game's fuse is its own); a sighting of it
/// this soon after the throw (still at the hand) says nothing of where it goes.
const OWN_GRENADE_LATE: f64 = 0.3;
const OWN_GRENADE_SETTLE: f64 = 0.2;
const ROCKET_RADIUS: f32 = 300.0;
const SATCHEL_RADIUS: f32 = 300.0;
/// A rocket passing this close is coming for the bot.
const ROCKET_CLOSE: f32 = 160.0;
/// A grenade heard bouncing is kept in mind this long after its last bounce (its fuse runs three seconds from the pin,
/// and the last bounces come as it rolls out); bounces heard this close are the same grenade, and its blast reaches as
/// far again as the ear may be off, up to this.
const HEARD_FOR: f64 = 2.5;
const HEARD_SAME: f32 = 200.0;
const HEARD_SLACK: f32 = 120.0;

#[derive(Clone, Debug, Default)]
pub struct Explosives {
    pub charges: Vec<Charge>,
    pub mines: Vec<Mine>,
    pub flying: Vec<Flying>,
    pub own_grenades: Vec<OwnGrenade>,
    pub heard: Vec<HeardGrenade>,
    /// Beams of mines forgotten that navigation was told to keep off: to be lifted.
    pub lifted: Vec<(Vec3, Vec3)>,
}

impl Mine {
    /// An explosion at `pos` was this mine going off, or set it off.
    fn blown_by(&self, pos: Vec3) -> bool {
        let dir = if self.dir == Vec3::ZERO { Vec3::Z } else { self.dir };
        let along = (pos - self.pos).dot(dir).clamp(0.0, MINE_BURST_LINE);
        self.pos.distance(pos) <= BLOWN_WITH || (self.pos + dir * along).distance(pos) <= MINE_BURST_SLACK
    }
}

/// A hand grenade heard bouncing: where the last bounce seemed to be, 1σ of that, and when.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeardGrenade {
    pub at: Vec3,
    pub sigma: f32,
    pub t: SimTime,
}

/// A hand grenade the bot threw: where it should come down (where it was seen going when it was), and when it goes
/// off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OwnGrenade {
    pub at: Vec3,
    pub goes_off: SimTime,
    thrown: SimTime,
    /// The fall of projectiles, for where a sighting of it says it is going.
    gravity: f32,
}

impl Explosives {
    pub fn thrown_satchel(&mut self, landing: Vec3, now: SimTime) {
        // Seen leaving the hand before the throw was noted: that one.
        if let Some(c) = self
            .charges
            .iter_mut()
            .filter(|c| c.index.is_some() && now.since(c.since) <= JUST_THROWN)
            .max_by(|a, b| a.since.0.total_cmp(&b.since.0))
        {
            c.since = now;
            return;
        }
        self.charges.push(Charge {
            pos: landing,
            since: now,
            index: None,
            vel: Vec3::ZERO,
            seen: None,
        });
    }

    /// The bot let a grenade go at `now` that should come down at `at` and goes off at `goes_off`; `sv_gravity` for its
    /// fall.
    pub fn thrown_grenade(&mut self, at: Vec3, goes_off: SimTime, now: SimTime, sv_gravity: f32) {
        self.own_grenades.push(OwnGrenade {
            at,
            goes_off,
            thrown: now,
            gravity: sv_gravity * PROJECTILE_GRAVITY,
        });
    }

    /// The bot's satchels went off.
    pub fn detonated(&mut self) {
        self.charges.clear();
    }

    pub fn placed_mine(&mut self, pos: Vec3, dir: Vec3, now: SimTime) {
        self.forget_mines(|m| m.pos.distance(pos) <= SAME_SPOT);
        self.mines.push(Mine {
            pos,
            dir,
            beam_end: None,
            own: true,
            armed_at: now + f64::from(TRIPMINE_ARM),
            seen: now,
            avoided_at: None,
            pass: None,
        });
    }

    pub fn on_sighting(&mut self, s: &ProjectileSighting) {
        match s.kind {
            ProjectileKind::Satchel if s.own => {
                let known = self.charges.iter().position(|c| c.index == Some(s.index));
                // Not seen before: the charge thrown last that has not been seen, while it may still fly, or the one
                // meant to land nearest.
                let unseen = || {
                    let fresh = self
                        .charges
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| c.index.is_none() && s.t.since(c.since) <= OWN_CHARGE_FLYING)
                        .max_by(|a, b| a.1.since.0.total_cmp(&b.1.since.0))
                        .map(|(i, _)| i);
                    fresh.or_else(|| {
                        self.charges
                            .iter()
                            .enumerate()
                            .filter(|(_, c)| c.index.is_none() && c.pos.distance(s.pos) <= OWN_CHARGE_MATCH)
                            .min_by(|a, b| a.1.pos.distance(s.pos).total_cmp(&b.1.pos.distance(s.pos)))
                            .map(|(i, _)| i)
                    })
                };
                let seen = Charge {
                    pos: s.pos,
                    since: s.t,
                    index: Some(s.index),
                    vel: s.vel,
                    seen: Some(s.t),
                };
                match known.or_else(unseen) {
                    Some(i) => {
                        let c = &mut self.charges[i];
                        *c = Charge { since: c.since, ..seen };
                    }
                    None => self.charges.push(seen),
                }
            }
            ProjectileKind::Tripmine => {
                let (dir, end) = s.beam.map_or((Vec3::ZERO, None), |(d, e)| (d, Some(e)));
                let known = self.mines.iter().position(|m| m.pos.distance(s.pos) <= SAME_SPOT);
                match known.map(|i| &mut self.mines[i]) {
                    Some(m) => {
                        m.seen = s.t;
                        // Armed when its beam is seen to come on; not before a look that shows it off.
                        m.armed_at = if s.armed {
                            m.armed_at.min(s.t)
                        } else {
                            m.armed_at.max(s.t + ARMING_LOOK)
                        };
                        // Where it is seen: its own were where the bot aimed, a few units off.
                        let moved = s.pos - m.pos;
                        if moved.length() > MINE_MOVED
                            && let (Some(_), Some(old)) = (m.avoided_at.take(), m.beam_end)
                        {
                            self.lifted.push((m.pos, old));
                        }
                        m.pos = s.pos;
                        m.beam_end = m.beam_end.map(|e| e + moved);
                        if end.is_some() {
                            m.dir = dir;
                            m.beam_end = end;
                        }
                    }
                    // A mine seen with its beam off arms within 2.5 s.
                    None => self.mines.push(Mine {
                        pos: s.pos,
                        dir,
                        beam_end: end,
                        own: s.own,
                        armed_at: if s.armed { s.t } else { s.t + f64::from(TRIPMINE_ARM) },
                        seen: s.t,
                        avoided_at: None,
                        pass: None,
                    }),
                }
            }
            _ if s.own && s.kind == ProjectileKind::Grenade => {
                // Where it is going, as it is seen: a throw that met an edge may come back.
                let seen = Flying {
                    kind: s.kind,
                    index: s.index,
                    pos: s.pos,
                    vel: s.vel,
                    own: true,
                    seen: s.t,
                    launch_speed: 0.0,
                };
                if let Some(g) = self
                    .own_grenades
                    .iter_mut()
                    .filter(|g| s.t.since(g.thrown) >= OWN_GRENADE_SETTLE && s.t < g.goes_off)
                    .min_by(|a, b| a.at.distance(s.pos).total_cmp(&b.at.distance(s.pos)))
                    && let Some(b) = predict(&seen, s.t, g.gravity, g.at.z.min(s.pos.z))
                {
                    g.at = b.at;
                }
                self.track_flying(s);
            }
            _ => self.track_flying(s),
        }
    }

    fn track_flying(&mut self, s: &ProjectileSighting) {
        match self.flying.iter_mut().find(|f| f.index == s.index && f.kind == s.kind) {
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
        }
    }

    /// A hand grenade was heard bouncing at `at`, known to `sigma` (1σ).
    pub fn heard_bounce(&mut self, at: Vec3, sigma: f32, now: SimTime) {
        let heard = HeardGrenade { at, sigma, t: now };
        match self.heard.iter_mut().find(|h| h.at.distance(at) <= HEARD_SAME) {
            Some(h) => *h = heard,
            None => self.heard.push(heard),
        }
    }

    /// An explosion was seen or heard at `pos`: what lay there is gone.
    pub fn on_explosion(&mut self, pos: Vec3) {
        self.forget_mines(|m| m.blown_by(pos));
        self.charges.retain(|c| c.pos.distance(pos) > BLOWN_WITH);
        self.flying.retain(|f| f.pos.distance(pos) > BLOWN_WITH);
        self.heard.retain(|h| h.at.distance(pos) > HEARD_SAME);
    }

    /// The bot looked where a mine should be and it is not there (set off out of earshot, or taken away).
    pub fn mine_missing(&mut self, pos: Vec3) {
        self.forget_mines(|m| m.pos.distance(pos) <= SAME_SPOT);
    }

    /// Its own mines are gone: a GunGame level left (the plugin sets them all off).
    pub fn forget_own_mines(&mut self) {
        self.forget_mines(|m| m.own);
    }

    fn forget_mines(&mut self, gone: impl Fn(&Mine) -> bool) {
        let lifted = &mut self.lifted;
        self.mines.retain(|m| {
            if !gone(m) {
                return true;
            }
            if let (Some(_), Some(end)) = (m.avoided_at, m.beam_end) {
                lifted.push((m.pos, end));
            }
            false
        });
    }

    /// The bot died: the game removes its satchels.
    pub fn on_own_death(&mut self) {
        self.charges.clear();
        self.flying.clear();
        self.own_grenades.clear();
        self.heard.clear();
    }

    pub fn update(&mut self, now: SimTime) {
        self.own_grenades.retain(|g| now.since(g.goes_off) <= OWN_GRENADE_LATE);
        self.heard.retain(|h| now.since(h.t) <= HEARD_FOR);
        self.flying.retain(|f| {
            let memory = match f.kind {
                ProjectileKind::Satchel | ProjectileKind::Snark => LYING_MEMORY,
                _ => FLYING_MEMORY,
            };
            now.since(f.seen) <= memory
        });
    }

    /// Explosions the projectiles the bot has seen are about to set off, `sv_gravity` for their fall: others', and its
    /// own grenades (a grenade does not care who threw it); and grenades heard bouncing. Its own satchels go off only
    /// when it sets them off.
    pub fn blasts(&self, now: SimTime, sv_gravity: f32, floor: f32) -> impl Iterator<Item = Blast> + '_ {
        self.flying
            .iter()
            .filter(|f| !f.own || f.kind == ProjectileKind::Grenade)
            .filter_map(move |f| predict(f, now, sv_gravity * PROJECTILE_GRAVITY, floor))
            .chain(self.own_grenades.iter().map(|g| Blast {
                at: g.at,
                radius: GRENADE_RADIUS,
                kind: ProjectileKind::Grenade,
            }))
            .chain(self.heard.iter().map(|h| Blast {
                at: h.at,
                radius: GRENADE_RADIUS + h.sigma.min(HEARD_SLACK),
                kind: ProjectileKind::Grenade,
            }))
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
            armed: kind == ProjectileKind::Tripmine && !own,
        }
    }

    #[test]
    fn a_grenade_heard_bouncing_is_kept_away_from_until_it_goes_off() {
        let mut e = Explosives::default();
        e.heard_bounce(Vec3::new(100.0, 0.0, 0.0), 40.0, SimTime(1.0));
        e.heard_bounce(Vec3::new(140.0, 30.0, 0.0), 30.0, SimTime(1.4));
        assert_eq!(e.heard.len(), 1, "bounces close together are one grenade");
        let blast = e.blasts(SimTime(1.5), 800.0, 0.0).next().expect("a blast");
        assert_eq!(blast.at, Vec3::new(140.0, 30.0, 0.0));
        assert_eq!(blast.radius, GRENADE_RADIUS + 30.0);
        e.update(SimTime(3.0));
        assert_eq!(e.heard.len(), 1);
        e.on_explosion(Vec3::new(150.0, 20.0, 0.0));
        assert!(e.heard.is_empty(), "gone off");
        e.heard_bounce(Vec3::new(-300.0, 0.0, 0.0), 500.0, SimTime(4.0));
        assert_eq!(
            e.blasts(SimTime(4.0), 800.0, 0.0).next().map(|b| b.radius),
            Some(GRENADE_RADIUS + HEARD_SLACK),
            "an ear far off widens it only so much"
        );
        e.update(SimTime(6.6));
        assert!(e.heard.is_empty(), "forgotten well after its fuse ran out");
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
    fn a_satchel_seen_in_flight_is_the_one_just_thrown() {
        let mut e = Explosives::default();
        e.thrown_satchel(Vec3::new(500.0, 0.0, 0.0), SimTime(1.0));
        // Just out of the hand, far from where it is meant to land.
        let flying = Vec3::new(40.0, 0.0, 30.0);
        e.on_sighting(&seen(
            ProjectileKind::Satchel,
            41,
            flying,
            Vec3::new(300.0, 0.0, 100.0),
            true,
            1.05,
        ));
        assert_eq!(e.charges.len(), 1, "{:?}", e.charges);
        let later = e.charges[0].at(SimTime(1.15), 800.0);
        assert!((later.x - 70.0).abs() < 1.0 && later.z > flying.z, "{later:?}");
        // Seen again by its entity, wherever it is.
        e.on_sighting(&seen(
            ProjectileKind::Satchel,
            41,
            Vec3::new(420.0, 0.0, 0.0),
            Vec3::ZERO,
            true,
            2.2,
        ));
        assert_eq!(e.charges.len(), 1);
        assert_eq!(e.charges[0].pos, Vec3::new(420.0, 0.0, 0.0));
        assert_eq!(e.charges[0].since, SimTime(1.0), "thrown when it was thrown");
        // Seen leaving the hand before the throw is noted: still one.
        let mut e = Explosives::default();
        e.on_sighting(&seen(
            ProjectileKind::Satchel,
            42,
            flying,
            Vec3::new(300.0, 0.0, 100.0),
            true,
            2.0,
        ));
        e.thrown_satchel(Vec3::new(500.0, 0.0, 0.0), SimTime(2.01));
        assert_eq!(e.charges.len(), 1, "{:?}", e.charges);
        assert_eq!(e.charges[0].index, Some(42));
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
    fn a_mine_is_forgotten_by_its_own_burst_out_along_the_way_it_faces() {
        let mut e = Explosives::default();
        // On the floor at z = 0 (8 up), and on a wall at x = 500 facing -x; the game bursts each 75.6 off its surface.
        e.placed_mine(Vec3::new(0.0, 0.0, 8.0), Vec3::Z, SimTime(1.0));
        e.placed_mine(Vec3::new(492.0, 0.0, 20.0), Vec3::NEG_X, SimTime(1.0));
        e.placed_mine(Vec3::new(0.0, 200.0, 8.0), Vec3::Z, SimTime(1.0));
        e.on_explosion(Vec3::new(0.0, 0.0, 75.6));
        e.on_explosion(Vec3::new(500.0 - 75.6, 0.0, 20.0));
        assert_eq!(e.mines.len(), 1, "{:?}", e.mines);
        assert_eq!(e.mines[0].pos, Vec3::new(0.0, 200.0, 8.0), "one 200 units off stays");
    }

    #[test]
    fn a_mine_moves_to_where_it_is_seen_and_goes_when_it_is_not_there() {
        let mut e = Explosives::default();
        e.placed_mine(Vec3::new(100.0, 0.0, 8.0), Vec3::Z, SimTime(1.0));
        e.mines[0].beam_end = Some(Vec3::new(100.0, 0.0, 200.0));
        e.mines[0].avoided_at = Some(SimTime(3.5));
        e.on_sighting(&seen(
            ProjectileKind::Tripmine,
            50,
            Vec3::new(106.0, 0.0, 8.0),
            Vec3::ZERO,
            false,
            4.0,
        ));
        assert_eq!(e.mines.len(), 1);
        let m = e.mines[0];
        assert_eq!(m.pos, Vec3::new(106.0, 0.0, 8.0));
        assert!(m.own, "still its own");
        assert_eq!(m.beam_end, Some(Vec3::new(106.0, 0.0, 200.0)));
        assert_eq!(m.avoided_at, None, "its beam is told again from where it is");
        assert_eq!(e.lifted, [(Vec3::new(100.0, 0.0, 8.0), Vec3::new(100.0, 0.0, 200.0))]);
        e.lifted.clear();
        e.mines[0].avoided_at = Some(SimTime(4.1));
        e.mine_missing(Vec3::new(110.0, 0.0, 8.0));
        assert!(e.mines.is_empty());
        assert_eq!(e.lifted.len(), 1, "its beam lifted");
    }

    #[test]
    fn a_mine_seen_just_placed_arms_later_and_its_neighbour_is_not_taken_for_it() {
        let mut e = Explosives::default();
        e.placed_mine(Vec3::new(0.0, 0.0, 8.0), Vec3::Z, SimTime(1.0));
        // The next mine of a trail, a stride on, seen before the bot noted it.
        let mut s = seen(
            ProjectileKind::Tripmine,
            52,
            Vec3::new(100.0, 0.0, 8.0),
            Vec3::ZERO,
            true,
            1.1,
        );
        s.beam = Some((Vec3::Z, Vec3::new(100.0, 0.0, 200.0)));
        s.armed = false;
        e.on_sighting(&s);
        assert_eq!(e.mines.len(), 2, "{:?}", e.mines);
        assert_eq!(
            e.mines[0].pos,
            Vec3::new(0.0, 0.0, 8.0),
            "the first one stays where it is"
        );
        assert_eq!(
            e.mines[1].armed_at,
            SimTime(1.1 + f64::from(TRIPMINE_ARM)),
            "its beam off: it arms later"
        );
    }

    #[test]
    fn its_own_mines_go_with_a_gungame_level() {
        let mut e = Explosives::default();
        e.placed_mine(Vec3::new(0.0, 0.0, 8.0), Vec3::Z, SimTime(1.0));
        let mut s = seen(
            ProjectileKind::Tripmine,
            51,
            Vec3::new(400.0, 0.0, 8.0),
            Vec3::ZERO,
            false,
            1.0,
        );
        s.beam = Some((Vec3::Z, Vec3::new(400.0, 0.0, 200.0)));
        e.on_sighting(&s);
        e.forget_own_mines();
        assert_eq!(e.mines.len(), 1);
        assert!(!e.mines[0].own);
    }

    #[test]
    fn its_own_grenade_is_kept_away_from_until_it_goes_off_seen_or_not() {
        let mut e = Explosives::default();
        e.thrown_grenade(Vec3::new(500.0, 0.0, 0.0), SimTime(4.0), SimTime(1.0), 800.0);
        e.update(SimTime(2.0));
        let blasts: Vec<Blast> = e.blasts(SimTime(2.0), 800.0, 0.0).collect();
        assert_eq!(blasts.len(), 1);
        assert_eq!(blasts[0].at, Vec3::new(500.0, 0.0, 0.0));
        e.update(SimTime(4.5));
        assert_eq!(e.blasts(SimTime(4.5), 800.0, 0.0).count(), 0, "gone off");
        // One that met an edge and comes back is kept away from where it is seen going.
        let mut e = Explosives::default();
        e.thrown_grenade(Vec3::new(500.0, 0.0, 0.0), SimTime(4.0), SimTime(1.0), 800.0);
        e.on_sighting(&seen(
            ProjectileKind::Grenade,
            40,
            Vec3::new(150.0, 0.0, 30.0),
            Vec3::new(-200.0, 0.0, 0.0),
            true,
            1.1,
        ));
        assert_eq!(e.own_grenades[0].at.x, 500.0, "still at the hand: nothing said");
        e.on_sighting(&seen(
            ProjectileKind::Grenade,
            40,
            Vec3::new(150.0, 0.0, 30.0),
            Vec3::new(-200.0, 0.0, 0.0),
            true,
            1.5,
        ));
        assert!(e.own_grenades[0].at.x < 150.0, "{:?}", e.own_grenades[0].at);
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
