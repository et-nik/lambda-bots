//! The gauss charge (yapb's `updateGaussCharge`, with its bugs fixed).
//!
//! - **In a fight**, a target 500–3000 units away is charged at, six times in ten; the charge goes once it has built
//!   for 1.3–1.6 s and the view is on the target. It goes when both attack buttons are up: the game fires on its
//!   next idle frame, at least half a second after the charge began. Pressing the primary attack while charging
//!   fires a plain shot and loses the charge, so a charging gauss keeps the weapon channel.
//! - **Before an expected fight** (hunting an enemy lost moments ago) the bot may hold a charge ready, as often as
//!   its skill's `gauss_precharge`, and lets it go at the first target in its sights.
//! - **Dump:** a charge held 8 s on the ground (9 s anywhere; the gun shocks its holder at 10 s), in water or on a
//!   ladder is fired back along the way the bot came and a little down, so the recoil pushes it on. yapb aimed and
//!   released on the same frame; here the view turns first.
//! - The charged shot throws its shooter back at five times its damage: no charge is started with a drop behind.

use lb_core::math::{dir_to_view_angles, view_angle_vectors};
use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3};
use lb_game::mechanics::{Attack, GAUSS_MIN_CHARGE, Trigger};
use lb_game::weapons::WeaponId;
use lb_motor::LookIntent;

use super::{Hands, Request, hold, press, settled};

/// Uranium a combat charge and a charge held ready need.
const COMBAT_URANIUM: i32 = 20;
const READY_URANIUM: i32 = 25;
const COMBAT_BAND: [f32; 2] = [500.0, 3000.0];
const COMBAT_CHANCE: f32 = 0.6;
const RELEASE_AFTER: [f32; 2] = [1.3, 1.6];
const DUMP_ON_GROUND: f32 = 8.0;
const DUMP_ANYWHERE: f32 = 9.0;
/// A charge held ready without a fight coming is dumped after this.
const READY_HOLD: f32 = 6.0;
/// The game starts spinning within this after the press, or it will not (no ammo, under water).
const START_WITHIN: f64 = 0.4;
/// Buttons stay up this long for the game to fire.
const RELEASE_FOR: f64 = 0.2;
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
    /// The floor behind the bot would catch it if the recoil threw it back.
    pub safe_behind: bool,
    /// Horizontal direction the bot is going; zero when it stands.
    pub heading: Vec2,
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
    /// Charges fired at a target and dumped, for `lb brain`.
    pub fired: u32,
    pub dumped: u32,
}

impl Gauss {
    pub fn reset(&mut self) {
        let (fired, dumped) = (self.fired, self.dumped);
        *self = Gauss {
            fired,
            dumped,
            ..Gauss::default()
        };
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
                    self.state = State::Dumping {
                        angles: dump_angles(h, i.heading),
                        since: now,
                    };
                    self.dumped += 1;
                    return Some(charge_request());
                }
                let wait = if combat { release_after } else { GAUSS_MIN_CHARGE };
                if let Some((_, on_target)) = i.target
                    && on_target
                    && age >= wait
                {
                    self.state = State::Releasing {
                        since: now,
                        angles: None,
                    };
                    self.fired += 1;
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
                    self.next_roll = now + f64::from(rng.range_f32(1.0, 2.0));
                    return None;
                }
                Some(Request {
                    weapon: Some(hold(WeaponId::Gauss)),
                    look: angles.map(LookIntent::Angles),
                    movement: None,
                })
            }
        }
    }

    fn start(&mut self, h: &Hands<'_>, i: &GaussInput, rng: &mut Pcg32) {
        let now = h.now;
        if now < self.next_roll
            || !h.ready(WeaponId::Gauss)
            || h.waterlevel >= 2
            || h.on_ladder
            || !h.on_ground
            || !i.safe_behind
        {
            return;
        }
        let uranium = h.reserve(WeaponId::Gauss);
        let charge = |combat: bool, rng: &mut Pcg32| State::Charging {
            pressed: now,
            started: None,
            combat,
            release_after: rng.range_f32(RELEASE_AFTER[0], RELEASE_AFTER[1]),
            target_seen: now,
        };
        match i.target {
            Some((d, _)) if (COMBAT_BAND[0]..=COMBAT_BAND[1]).contains(&d) && uranium >= COMBAT_URANIUM => {
                if rng.next_f32() < COMBAT_CHANCE {
                    self.state = charge(true, rng);
                } else {
                    self.next_roll = now + f64::from(rng.range_f32(1.0, 2.0));
                }
            }
            None if i.expected && uranium >= READY_URANIUM => {
                if rng.next_f32() < i.precharge {
                    self.state = charge(false, rng);
                } else {
                    self.next_roll = now + f64::from(rng.range_f32(6.0, 10.0));
                }
            }
            _ => {}
        }
    }
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

    fn run(target: impl Fn(f64) -> Option<(f32, bool)>, expected: bool, secs: f64) -> (Gauss, Game, Vec<Fire>) {
        let mut g = Gauss::default();
        let mut game = Game::default();
        let mut rng = Pcg32::new(4, 4);
        let arsenal = [Armed::new(WeaponId::Gauss, None, Some(60))];
        let mut presses = Vec::new();
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
                weapon: Some(WeaponId::Gauss),
                arsenal: &arsenal,
                prediction: Some(&prediction),
                dll: DllProfile::default(),
                gravity: 800.0,
            };
            let input = GaussInput {
                target: target(t),
                expected,
                precharge: 1.0,
                safe_behind: true,
                heading: Vec2::X,
            };
            let fire = g
                .update(&h, &input, &mut rng)
                .and_then(|r| r.weapon)
                .map_or(Fire::Primary, |w| w.fire);
            presses.push(fire);
            game.frame(t, fire);
            t += 0.01;
        }
        (g, game, presses)
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
