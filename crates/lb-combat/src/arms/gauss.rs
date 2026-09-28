//! The gauss charge (yapb's `updateGaussCharge`, with its bugs fixed).
//!
//! - **In a fight** skilled players fight the gauss charged (about four shots in five; the plain primary attack is a
//!   beginner's): the bot charges as often as its skill's `gauss_charge`, at any distance, and lets go once the view
//!   is on the target and the charge has built for its distance: half a second or a little more up close (70–100
//!   damage), about a second at mid range, the full 1.5 s (200) far away, less when uranium runs low. It lets go when
//!   both attack buttons are up: the game fires on its next idle frame, at least half a second after the charge
//!   began. Pressing the primary attack while charging fires a plain shot and loses the charge, so a charging gauss
//!   keeps the weapon channel. The next charge starts right after, and plain shots are fired only after a roll for
//!   them or while no charge can start ([`Gauss::plain_allowed`]): a plain shot keeps the gun from charging for a
//!   fifth of a second.
//! - **Before an expected fight** (hunting an enemy lost moments ago) the bot may hold a charge ready, as often as
//!   its skill's `gauss_precharge`, and lets it go at the first target in its sights.
//! - **Dump:** a charge held 8 s on the ground (9 s anywhere; the gun shocks its holder at 10 s), in water or on a
//!   ladder is fired back along the way the bot came and a little down, so the recoil pushes it on. yapb aimed and
//!   released on the same frame; here the view turns first.
//! - The charged shot throws its shooter back at five times its damage, further when the view is below the horizon
//!   (the push lifts it off the ground): a charge is kept small enough, or not started, for the throw to stop short of
//!   a drop behind ([`recoil_throw`]).
//! - A charged beam that meets a wall punches through and bursts where it comes out, 1.75 times its damage around in
//!   multiplayer, or glances off with a burst of its own. It goes through players, so the wall may be behind the
//!   target: a charge is kept small enough, or not started, for no wall along the line of fire to be that close
//!   ([`wall_blast`]).

use lb_core::math::{dir_to_view_angles, view_angle_vectors};
use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3};
use lb_game::mechanics::{Attack, GAUSS_FULL_CHARGE, GAUSS_MIN_CHARGE, Trigger};
use lb_game::weapons::WeaponId;
use lb_motor::LookIntent;

use super::{Hands, Request, hold, press, settled};

/// Uranium a combat charge and a charge held ready need.
const COMBAT_URANIUM: i32 = 8;
const READY_URANIUM: i32 = 25;
/// How long a combat charge builds by distance: up close, at mid range, far away (seconds, [min, max]).
const CLOSE: f32 = 350.0;
const MID: f32 = 900.0;
const RELEASE_CLOSE: [f32; 2] = [0.5, 0.75];
const RELEASE_MID: [f32; 2] = [0.8, 1.2];
const RELEASE_FAR: [f32; 2] = [1.3, 1.6];
/// A charge takes a uranium cell every tenth of a second.
const CELLS_PER_SECOND: f32 = 10.0;
/// After a plain-shot roll, the next chance to charge comes this much later.
const PLAIN_FOR: [f32; 2] = [0.8, 1.6];
/// After a charged shot the next charge may start this soon.
const RECHARGE: [f32; 2] = [0.1, 0.3];
const DUMP_ON_GROUND: f32 = 8.0;
const DUMP_ANYWHERE: f32 = 9.0;
/// A charge held ready without a fight coming is dumped after this.
const READY_HOLD: f32 = 6.0;
/// The game starts spinning within this after the press, or it will not (no ammo, under water).
const START_WITHIN: f64 = 0.5;
/// After a plain shot the game takes no attack for a fifth of a second: a charge pressed for meanwhile starts after.
const PLAIN_WAIT: f32 = 0.25;
/// While no charge can start, plain shots are allowed this long before the next look.
const BLOCKED_RECHECK: f64 = 0.1;
/// Buttons stay up this long for the game to fire.
const RELEASE_FOR: f64 = 0.2;
/// The game fires a charge a command or two after the bot lets go: the checks look that far ahead, and a charge kept
/// small is let go twice that early.
const RELEASE_LAG: f32 = 0.05;
/// A dump waits this long at most for the view to turn.
const DUMP_TURN: f64 = 0.6;
/// A combat charge whose target is gone this long is held on as a ready one.
const TARGET_LOST: f64 = 2.0;

/// What the gauss needs to know beyond the hands.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GaussInput {
    /// The target in sight: its distance, and whether the view is on it for the shot.
    pub target: Option<(f32, bool)>,
    /// A fight is expected soon (an enemy lost moments ago).
    pub expected: bool,
    /// Share of chances a charge is held ready before an expected fight, 0..1.
    pub precharge: f32,
    /// How far the bot may be thrown back before it would drop further than is safe (walls stop the throw).
    pub recoil_room: f32,
    /// How far along the line of fire (through players) the first wall is; unbounded when none is near.
    pub wall_ahead: f32,
    /// Damage of the fully charged shot.
    pub full_damage: f32,
    /// Horizontal direction the bot is going; zero when it stands.
    pub heading: Vec2,
    /// The bot may use the gauss (`lb weapons`).
    pub allowed: bool,
    /// Share of fights fought with the charged shot, 0..1.
    pub charge_share: f32,
    /// View angles to dump a charge along, found clear of walls ahead and drops behind; `None`: back the way the bot
    /// goes.
    pub dump: Option<lb_core::Vec3>,
    /// A charged shot of this much damage or less, missing, comes back at its shooter: in vanilla HLDM a beam that
    /// fails to punch through a wall met square starts over from the gun, the shooter no longer left out. Zero where
    /// it never does (BugfixedHL by default) or the wall along the view is punched through or glanced off.
    pub backfire: f32,
    /// The shot at the target needs this much damage at least: one through a wall, to come out with enough left.
    pub min_damage: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum State {
    #[default]
    Idle,
    Charging {
        pressed: SimTime,
        /// When the game began spinning.
        started: Option<SimTime>,
        combat: bool,
        release_after: f32,
        target_seen: SimTime,
    },
    /// Turning to dump the charge; still holding it.
    Dumping { angles: lb_core::Vec3, since: SimTime },
    Releasing {
        since: SimTime,
        angles: Option<lb_core::Vec3>,
    },
}

#[derive(Clone, Debug, Default)]
pub struct Gauss {
    state: State,
    next_roll: SimTime,
    /// Plain shots are allowed until then.
    plain_until: SimTime,
    /// Charges fired at a target and dumped, rolls for plain shots, and tenths of a second with a target in sight
    /// that no charge could start for a drop behind or a wall ahead, for `lb brain`.
    pub fired: u32,
    pub dumped: u32,
    pub plain_rolls: u32,
    pub cramped: u32,
    /// The last charge let go, for the log when a bot dies by its own gauss.
    pub last: Option<Release>,
}

/// A charge let go: when, how strong, whether dumped or at a target (and how far), the view, and how far the first
/// wall along it was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Release {
    pub at: SimTime,
    pub damage: f32,
    pub dump: bool,
    pub target: Option<f32>,
    pub view: lb_core::Vec3,
    pub wall_ahead: f32,
    pub recoil_room: f32,
}

impl Gauss {
    pub fn reset(&mut self) {
        *self = Gauss {
            state: State::Idle,
            next_roll: SimTime::ZERO,
            plain_until: SimTime::ZERO,
            ..*self
        };
    }

    /// The gun may fire plain shots at the target now: a roll said so, or no charge can start.
    pub fn plain_allowed(&self, now: SimTime) -> bool {
        self.state == State::Idle && now < self.plain_until
    }

    /// Holding a charge or letting it go: the weapon channel is taken.
    pub fn active(&self) -> bool {
        self.state != State::Idle
    }

    /// Seconds the charge has built; zero when not charging.
    pub fn charge(&self, now: SimTime) -> f32 {
        match self.state {
            State::Charging {
                started: Some(started), ..
            } => now.since(started) as f32,
            _ => 0.0,
        }
    }

    pub fn phase(&self) -> &'static str {
        match self.state {
            State::Idle => "idle",
            State::Charging { combat: true, .. } => "charging",
            State::Charging { combat: false, .. } => "charged",
            State::Dumping { .. } => "dumping",
            State::Releasing { .. } => "releasing",
        }
    }

    pub fn update(&mut self, h: &Hands<'_>, i: &GaussInput, rng: &mut Pcg32) -> Option<Request> {
        let now = h.now;
        if self.active() && h.weapon != Some(WeaponId::Gauss) {
            // Switched away (or taken away): the charge is gone.
            self.state = State::Idle;
        }
        match self.state {
            State::Idle => {
                self.start(h, i, rng);
                None
            }
            State::Charging {
                pressed,
                started,
                combat,
                release_after,
                target_seen,
            } => {
                let spinning = h.predicted(WeaponId::Gauss).is_some_and(|p| p.in_attack != 0);
                let started = match started {
                    Some(t) => t,
                    None if spinning => now,
                    None if now.since(pressed) > START_WITHIN => {
                        self.state = State::Idle;
                        self.next_roll = now + 2.0;
                        self.plain_until = self.next_roll;
                        return None;
                    }
                    None => return Some(charge_request()),
                };
                let age = now.since(started) as f32;
                let target_seen = if i.target.is_some() { now } else { target_seen };
                let combat = combat && now.since(target_seen) <= TARGET_LOST;
                let must_dump = (age >= DUMP_ON_GROUND && h.on_ground)
                    || age >= DUMP_ANYWHERE
                    || h.waterlevel >= 2
                    || h.on_ladder
                    || (!combat && !i.expected && i.target.is_none() && age >= READY_HOLD);
                if must_dump {
                    let angles = i.dump.unwrap_or_else(|| dump_angles(h, i.heading));
                    self.state = State::Dumping { angles, since: now };
                    self.dumped += 1;
                    self.last = Some(Release {
                        at: now,
                        damage: charged_damage(age, i.full_damage),
                        dump: true,
                        target: None,
                        view: angles,
                        wall_ahead: i.wall_ahead,
                        recoil_room: i.recoil_room,
                    });
                    return Some(charge_request());
                }
                let wait = if combat { release_after } else { GAUSS_MIN_CHARGE };
                let (forward, _, _) = view_angle_vectors(h.view);
                let damage = charged_damage(age + RELEASE_LAG, i.full_damage);
                let throw = recoil_throw(damage, forward, h.gravity);
                if let Some((_, on_target)) = i.target
                    && on_target
                    && age >= wait
                    && throw <= i.recoil_room
                    && i.wall_ahead >= wall_blast(damage)
                    && damage > i.backfire
                    && damage >= i.min_damage
                {
                    self.state = State::Releasing {
                        since: now,
                        angles: None,
                    };
                    self.fired += 1;
                    self.last = Some(Release {
                        at: now,
                        damage,
                        dump: false,
                        target: i.target.map(|t| t.0),
                        view: h.view,
                        wall_ahead: i.wall_ahead,
                        recoil_room: i.recoil_room,
                    });
                    return Some(Request {
                        weapon: Some(hold(WeaponId::Gauss)),
                        ..Request::default()
                    });
                }
                self.state = State::Charging {
                    pressed,
                    started: Some(started),
                    combat,
                    release_after,
                    target_seen,
                };
                Some(charge_request())
            }
            State::Dumping { angles, since } => {
                if settled(h.view, angles, 3.0) || now.since(since) >= DUMP_TURN {
                    self.state = State::Releasing {
                        since: now,
                        angles: Some(angles),
                    };
                }
                Some(Request {
                    look: Some(LookIntent::Angles(angles)),
                    ..charge_request()
                })
            }
            State::Releasing { since, angles } => {
                let fired = h.predicted(WeaponId::Gauss).is_none_or(|p| p.in_attack == 0);
                if now.since(since) >= RELEASE_FOR && (fired || now.since(since) >= 3.0 * RELEASE_FOR) {
                    self.state = State::Idle;
                    self.next_roll = now + f64::from(rng.range_f32(RECHARGE[0], RECHARGE[1]));
                    return None;
                }
                Some(Request {
                    weapon: Some(hold(WeaponId::Gauss)),
                    look: angles.map(LookIntent::Angles),
                    movement: None,
                    jump: false,
                })
            }
        }
    }

    fn start(&mut self, h: &Hands<'_>, i: &GaussInput, rng: &mut Pcg32) {
        let now = h.now;
        if now < self.next_roll {
            return;
        }
        // In hand and deployed; a plain shot's wait may still run, the charge then starts after it.
        let in_hand = h.weapon == Some(WeaponId::Gauss)
            && h.prediction
                .is_none_or(|p| p.current == Some(WeaponId::Gauss) && p.next_attack <= PLAIN_WAIT);
        if !in_hand {
            return;
        }
        let (forward, _, _) = view_angle_vectors(h.view);
        // The longest charge whose throw stops short of a drop behind and whose burst on a wall ahead spares the bot.
        let room =
            max_charge(i.recoil_room, forward, h.gravity, i.full_damage).min(wall_charge(i.wall_ahead, i.full_damage));
        let room = room - 2.0 * RELEASE_LAG;
        let cramped = room < GAUSS_MIN_CHARGE;
        if !i.allowed || h.waterlevel >= 2 || h.on_ladder || cramped {
            if cramped && i.target.is_some() && now >= self.plain_until {
                self.cramped += 1;
            }
            self.plain_until = now + BLOCKED_RECHECK;
            return;
        }
        let uranium = h.reserve(WeaponId::Gauss);
        let charge = |combat: bool, d: f32, rng: &mut Pcg32| {
            let [lo, hi] = if d < CLOSE {
                RELEASE_CLOSE
            } else if d < MID {
                RELEASE_MID
            } else {
                RELEASE_FAR
            };
            // A cell at the start and one a tenth of a second on: low uranium lets go sooner.
            let affordable = ((uranium - 1) as f32 / CELLS_PER_SECOND).max(GAUSS_MIN_CHARGE);
            State::Charging {
                pressed: now,
                started: None,
                combat,
                release_after: rng.range_f32(lo, hi).min(affordable).min(room),
                target_seen: now,
            }
        };
        match i.target {
            Some((d, _)) if uranium >= COMBAT_URANIUM => {
                if rng.next_f32() < i.charge_share {
                    self.state = charge(true, d, rng);
                } else {
                    self.plain_rolls += 1;
                    self.next_roll = now + f64::from(rng.range_f32(PLAIN_FOR[0], PLAIN_FOR[1]));
                    self.plain_until = self.next_roll;
                }
            }
            Some(_) => self.plain_until = now + BLOCKED_RECHECK,
            None if i.expected && uranium >= READY_URANIUM => {
                if rng.next_f32() < i.precharge {
                    self.state = charge(false, MID, rng);
                } else {
                    self.next_roll = now + f64::from(rng.range_f32(6.0, 10.0));
                }
            }
            _ => {}
        }
    }
}

/// Damage of a charged shot let go after `secs` (multiplayer: the full damage at 1.5 s).
fn charged_damage(secs: f32, full: f32) -> f32 {
    full * (secs / GAUSS_FULL_CHARGE).min(1.0)
}

/// How far a charged shot of `damage` throws its shooter back when fired along `forward`: the game pushes it at five
/// times the damage against the view. On the ground friction stops the push within a quarter of a second's travel;
/// a view below the horizon lifts the shooter off the ground and the push carries it through the air first.
pub fn recoil_throw(damage: f32, forward: Vec3, gravity: f32) -> f32 {
    let push = 5.0 * damage;
    let flat = push * forward.truncate().length();
    let up = push * (-forward.z).max(0.0);
    flat / 4.0 + flat * 2.0 * up / gravity.max(1.0)
}

/// How near a wall along the line of fire a charged beam of `damage` would burst on the shooter: the burst where it
/// punches out of the wall reaches 1.75 times the damage in multiplayer (a glancing burst reaches less).
pub fn wall_blast(damage: f32) -> f32 {
    1.75 * damage + 32.0
}

/// Seconds of charge whose burst on a wall `wall` units along the line of fire spares the shooter.
fn wall_charge(wall: f32, full: f32) -> f32 {
    if wall_blast(full) <= wall {
        return f32::INFINITY;
    }
    GAUSS_FULL_CHARGE * ((wall - 32.0) / 1.75 / full).max(0.0)
}

/// Seconds of charge whose throw fits in `room`, looking along `forward` (zero when not even half a second's does).
fn max_charge(room: f32, forward: Vec3, gravity: f32, full: f32) -> f32 {
    if recoil_throw(full, forward, gravity) <= room {
        return f32::INFINITY;
    }
    // The throw grows with the damage: halve the interval.
    let (mut lo, mut hi) = (0.0f32, GAUSS_FULL_CHARGE);
    for _ in 0..12 {
        let mid = 0.5 * (lo + hi);
        if recoil_throw(charged_damage(mid, full), forward, gravity) <= room {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    if lo < GAUSS_MIN_CHARGE { 0.0 } else { lo }
}

fn charge_request() -> Request {
    Request {
        weapon: Some(press(WeaponId::Gauss, Attack::Secondary, Trigger::Hold, 0.0)),
        ..Request::default()
    }
}

/// Back along the way the bot is going and a little down (yapb's `startGaussDumpRelease`): the recoil pushes it on.
fn dump_angles(h: &Hands<'_>, heading: Vec2) -> lb_core::Vec3 {
    let back = if heading.length_squared() > 0.01 {
        -heading.normalize().extend(0.0)
    } else {
        let (forward, _, _) = view_angle_vectors(h.view);
        -forward.truncate().normalize_or(Vec2::X).extend(0.0)
    };
    dir_to_view_angles(back * 95.0 - Vec3::Z * 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Armed;
    use lb_game::dll::DllProfile;
    use lb_game::self_state::{PredictedWeapon, Prediction};
    use lb_motor::Fire;

    /// The game's side of the gauss: spins up on the secondary attack and fires when both buttons are up at
    /// least half a second after the spin began.
    #[derive(Default)]
    struct Game {
        since: Option<f64>,
        shots: Vec<(f64, f32)>,
    }

    impl Game {
        fn frame(&mut self, now: f64, fire: Fire) {
            match (fire, self.since) {
                (Fire::Secondary, None) => self.since = Some(now),
                (Fire::None, Some(t)) if now - t >= 0.5 => {
                    self.shots.push((now, (now - t) as f32));
                    self.since = None;
                }
                _ => {}
            }
        }
    }

    fn input() -> GaussInput {
        GaussInput {
            target: None,
            expected: false,
            precharge: 1.0,
            recoil_room: f32::INFINITY,
            wall_ahead: f32::INFINITY,
            full_damage: 200.0,
            heading: Vec2::X,
            allowed: true,
            charge_share: 1.0,
            dump: None,
            backfire: 0.0,
            min_damage: 0.0,
        }
    }

    fn run(target: impl Fn(f64) -> Option<(f32, bool)>, expected: bool, secs: f64) -> (Gauss, Game, Vec<Fire>) {
        let (g, game, presses, _) = run_with(GaussInput { expected, ..input() }, target, secs);
        (g, game, presses)
    }

    /// Also whether plain shots were allowed on each frame.
    fn run_with(
        base: GaussInput,
        target: impl Fn(f64) -> Option<(f32, bool)>,
        secs: f64,
    ) -> (Gauss, Game, Vec<Fire>, Vec<bool>) {
        let mut g = Gauss::default();
        let mut game = Game::default();
        let mut rng = Pcg32::new(4, 4);
        let arsenal = [Armed::new(WeaponId::Gauss, None, Some(60))];
        let mut presses = Vec::new();
        let mut plain = Vec::new();
        let mut t = 0.0;
        while t < secs {
            let mut prediction = Prediction {
                current: Some(WeaponId::Gauss),
                primary_ammo: 60,
                ..Prediction::default()
            };
            prediction.weapons[WeaponId::Gauss as usize] = Some(PredictedWeapon {
                in_attack: i32::from(game.since.is_some()),
                ..PredictedWeapon::default()
            });
            let h = Hands {
                now: SimTime(t),
                eye: Vec3::ZERO,
                origin: Vec3::ZERO,
                velocity: Vec3::ZERO,
                view: Vec3::ZERO,
                on_ground: true,
                on_ladder: false,
                waterlevel: 0,
                fov: 0.0,
                weapon: Some(WeaponId::Gauss),
                arsenal: &arsenal,
                prediction: Some(&prediction),
                dll: DllProfile::default(),
                gravity: 800.0,
            };
            let input = GaussInput {
                target: target(t),
                ..base
            };
            let fire = g
                .update(&h, &input, &mut rng)
                .and_then(|r| r.weapon)
                .map_or(Fire::Primary, |w| w.fire);
            presses.push(fire);
            plain.push(g.plain_allowed(SimTime(t)));
            game.frame(t, fire);
            t += 0.01;
        }
        (g, game, presses, plain)
    }

    #[test]
    fn plain_shots_only_after_a_roll_for_them() {
        let seen = |t: f64| Some((600.0, t >= 0.5));
        let (g, game, _, plain) = run_with(input(), seen, 6.0);
        assert!(game.shots.len() >= 3, "charge after charge: {:?}", game.shots);
        assert!(plain.iter().all(|p| !p), "no plain shot with every roll for a charge");
        assert_eq!(g.plain_rolls, 0);
        let beginner = GaussInput {
            charge_share: 0.0,
            ..input()
        };
        let (g, game, _, plain) = run_with(beginner, seen, 6.0);
        assert!(game.shots.is_empty() && g.plain_rolls >= 3);
        assert!(plain.iter().filter(|p| **p).count() > plain.len() * 9 / 10);
    }

    #[test]
    fn a_drop_behind_keeps_the_charge_small_or_off() {
        let seen = |t: f64| Some((1200.0, t >= 0.5));
        // A full charge on level ground slides the shooter some 250 units: 150 units of room hold a 0.9 s charge.
        let near_ledge = GaussInput {
            recoil_room: 150.0,
            ..input()
        };
        let (_, game, _, _) = run_with(near_ledge, seen, 4.0);
        assert!(!game.shots.is_empty());
        assert!(game.shots.iter().all(|(_, c)| *c <= 0.95), "{:?}", game.shots);
        let at_the_edge = GaussInput {
            recoil_room: 40.0,
            ..input()
        };
        let (g, game, _, plain) = run_with(at_the_edge, seen, 3.0);
        assert!(game.shots.is_empty() && g.cramped > 0);
        assert!(plain.iter().all(|p| *p), "plain shots while no charge can start");
    }

    #[test]
    fn a_wall_close_along_the_line_keeps_the_charge_small_or_off() {
        let seen = |t: f64| Some((1200.0, t >= 0.5));
        // A burst of 76 damage (0.57 s of charge) reaches 165 units: the charge is let go that small.
        let near = GaussInput {
            wall_ahead: 175.0,
            ..input()
        };
        let (_, game, _, _) = run_with(near, seen, 3.0);
        assert!(!game.shots.is_empty());
        assert!(game.shots.iter().all(|(_, c)| *c <= 0.6), "{:?}", game.shots);
        let against = GaussInput {
            wall_ahead: 100.0,
            ..input()
        };
        let (g, game, _, plain) = run_with(against, seen, 3.0);
        assert!(game.shots.is_empty() && g.cramped > 0);
        assert!(plain.iter().all(|p| *p), "plain shots against the wall");
        assert!(wall_blast(200.0) > 350.0 && wall_blast(70.0) < 160.0);
    }

    #[test]
    fn a_shot_through_a_wall_waits_for_the_damage_it_needs() {
        // Up close a charge is let go after 0.5–0.75 s (70–110 damage); through a wall it needs 150 (1.125 s).
        let close = |_: f64| Some((300.0, true));
        let (_, game, _, _) = run_with(input(), close, 3.0);
        assert!(!game.shots.is_empty());
        assert!(game.shots.iter().all(|(_, c)| *c < 0.8), "{:?}", game.shots);
        let through = GaussInput {
            min_damage: 150.0,
            ..input()
        };
        let (_, game, _, _) = run_with(through, close, 3.0);
        assert!(!game.shots.is_empty());
        assert!(game.shots.iter().all(|(_, c)| *c >= 1.05), "{:?}", game.shots);
    }

    #[test]
    fn looking_down_throws_the_shooter_further() {
        let level = recoil_throw(200.0, Vec3::X, 800.0);
        assert!((240.0..=260.0).contains(&level), "{level}");
        let pitch = 10f32.to_radians();
        let down = Vec3::new(lb_core::dmath::cos(pitch), 0.0, -lb_core::dmath::sin(pitch));
        let thrown = recoil_throw(200.0, down, 800.0);
        assert!((600.0..=720.0).contains(&thrown), "{thrown}");
        assert!(
            recoil_throw(200.0, Vec3::new(0.9, 0.0, 0.436), 800.0) < level,
            "looking up pins it down"
        );
    }

    #[test]
    fn charges_at_a_target_and_lets_go_on_it() {
        // In sight 1200 units away, the view on it from 1.0 s.
        let (g, game, presses) = run(|t| Some((1200.0, t >= 1.0)), false, 4.0);
        assert!(!game.shots.is_empty(), "fired: {:?}", g.phase());
        let (_, charge) = game.shots[0];
        assert!((1.25..=1.7).contains(&charge), "charged {charge} s");
        let mut charging = false;
        for fire in presses {
            if fire == Fire::Secondary {
                charging = true;
            }
            assert!(!(charging && fire == Fire::Primary), "no plain shot while charging");
            if fire == Fire::None {
                charging = false;
            }
        }
    }

    #[test]
    fn a_charge_held_ready_is_dumped_and_never_overheats() {
        // A fight is expected but never comes.
        let (g, game, _) = run(|_| None, true, 12.0);
        assert!(g.dumped >= 1, "{}", g.phase());
        assert!(game.shots.iter().all(|(_, charge)| *charge < 9.5), "{:?}", game.shots);
        // Nothing expected, nothing in sight: no charge at all.
        let (_, game, presses) = run(|_| None, false, 5.0);
        assert!(game.shots.is_empty() && presses.iter().all(|f| *f == Fire::Primary));
    }
}
