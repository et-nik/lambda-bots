//! Setting off explosives the bot knows of when an enemy walks up to them.
//!
//! - **Satchels** (yapb's `DetonateSatchel`): draw the satchel (its radio while charges are out) and press the DLL's
//!   detonate button once the game takes it. Confirmed when the game reports the charges gone off; the button that
//!   did it is reported, and holds for every bot on the server. If the press threw another satchel instead, or did
//!   nothing at all (the throw button with the pocket empty), the buttons are the other way round on this server,
//!   and the other button is pressed.
//! - **A satchel set off in flight** ([`Airburst`]): thrown at an enemy in sight, the radio stays in hand and the
//!   charge goes off as it comes by the enemy, like a grenade that goes off when told.
//! - **Tripmines** (yapb's `DetonateTripmine`): shooting a mine sets it off and credits the shooter, whoever placed
//!   it. Aim at the mine with a hitscan gun and fire while the view is on it; given up after 4 s.

use lb_core::Vec3;
use lb_core::math::dir_to_view_angles;
use lb_core::time::SimTime;
use lb_game::mechanics::{Attack, Trigger, spec};
use lb_game::weapons::WeaponId;
use lb_knowledge::PlayerKey;
use lb_motor::{LookIntent, MoveIntent};

use super::{Hands, Request, Status, hold, press, settled, takes};

/// Drawing the satchel radio takes a second (`CSatchel::Deploy`).
const DRAW_TIMEOUT: f64 = 2.0;
/// The game takes a button again this long after a throw at most (a second for the primary in the classic SDK).
const BUTTON_TIMEOUT: f64 = 1.5;
/// A press that shows nothing by then did nothing.
const NO_EFFECT: f64 = 0.3;
/// The press is held until the game shows the charges gone off, for this long at most.
const CONFIRM: f64 = 0.6;
const SHOOT_FOR: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Draw,
    /// Pressing `button`: since `at`, once the game takes it, with `before` satchels in the pocket then; `last`: the
    /// other button did not do it.
    Press {
        button: Attack,
        since: SimTime,
        at: Option<SimTime>,
        before: i32,
        last: bool,
    },
}

#[derive(Clone, Debug)]
pub struct SatchelTrigger {
    phase: Phase,
    started: SimTime,
    /// The button that set the charges off, once they went off.
    pub button: Option<Attack>,
}

impl SatchelTrigger {
    pub fn new(now: SimTime) -> SatchelTrigger {
        SatchelTrigger {
            phase: Phase::Draw,
            started: now,
            button: None,
        }
    }

    /// `go`: press once the radio is in hand; until then it is held up, waiting.
    pub fn update(&mut self, h: &Hands<'_>, go: bool) -> Status {
        let now = h.now;
        let w = WeaponId::Satchel;
        let game = h.predicted(w);
        let state = game.map_or(0, |p| p.charge_ready);
        let count = h.reserve(w);
        if state == 2 {
            if let Phase::Press {
                button, at: Some(_), ..
            } = self.phase
            {
                self.button = Some(button);
            }
            return Status::Done;
        }
        if state != 1 {
            return Status::Failed("no satchel out");
        }
        let waiting = Status::Running(Request {
            weapon: Some(hold(w)),
            ..Request::default()
        });
        match self.phase {
            Phase::Draw => {
                if !h.ready(w) {
                    return if now.since(self.started) > DRAW_TIMEOUT {
                        Status::Failed("the satchel radio was not drawn")
                    } else {
                        waiting
                    };
                }
                if !go {
                    return waiting;
                }
                self.phase = Phase::Press {
                    button: h.dll.satchel_detonate(),
                    since: now,
                    at: None,
                    before: count,
                    last: false,
                };
                self.update(h, go)
            }
            Phase::Press {
                button,
                since,
                at,
                before,
                last,
            } => {
                let Some(at) = at else {
                    if !takes(game, button) {
                        return if now.since(since) > BUTTON_TIMEOUT {
                            Status::Failed("the satchel radio took no press")
                        } else {
                            waiting
                        };
                    }
                    self.phase = Phase::Press {
                        button,
                        since,
                        at: Some(now),
                        before: count,
                        last,
                    };
                    return Status::Running(Request {
                        weapon: Some(press(w, button, Trigger::Hold, 0.0)),
                        ..Request::default()
                    });
                };
                if !last && (count < before || now.since(at) > NO_EFFECT) {
                    // Another satchel thrown, or nothing done: the other button sets them off here. Let go first,
                    // and press it once the game takes it (a throw holds the buttons back up to a second).
                    self.phase = Phase::Press {
                        button: button.other(),
                        since: now,
                        at: None,
                        before: count,
                        last: true,
                    };
                    return waiting;
                }
                if now.since(at) > CONFIRM {
                    return Status::Failed("the satchels did not go off");
                }
                Status::Running(Request {
                    weapon: Some(press(w, button, Trigger::Hold, 0.0)),
                    ..Request::default()
                })
            }
        }
    }
}

/// What an airburst watches on this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Burst {
    /// Where the satchel is now, while the bot sees it.
    pub satchel: Option<Vec3>,
    /// Where the enemy is, while in sight or just lost.
    pub enemy: Option<Vec3>,
    /// The bot is out of the blast of its satchels.
    pub spared: bool,
}

/// The satchel goes off with the enemy this close to it (60 damage of the multiplayer satchel's 120).
pub const AIRBURST_REACH: f32 = 150.0;
/// The radio sets satchels off this long after a throw (`CSatchel::Throw`).
const RADIO_READY: f64 = 0.5;
/// A satchel that has not come by the enemy by then is left lying.
const AIRBURST_WATCH: f64 = 2.5;
/// The bot backs off the enemy this long after the throw, while the satchel flies.
const BACK_OFF_FOR: f64 = 1.0;
const BACK_OFF_SPEED: f32 = 320.0;

/// A satchel thrown at an enemy in sight, set off as it comes by: the radio stays in hand from the throw.
#[derive(Clone, Debug)]
pub struct Airburst {
    pub target: PlayerKey,
    thrown: SimTime,
    trigger: Option<SatchelTrigger>,
    /// The satchel went off with this button.
    pub button: Option<Attack>,
    /// How close the satchel came to the enemy while both were in sight.
    pub closest: Option<f32>,
}

impl Airburst {
    pub fn new(target: PlayerKey, thrown: SimTime) -> Airburst {
        Airburst {
            target,
            thrown,
            trigger: None,
            button: None,
            closest: None,
        }
    }

    /// The satchel was set off (rather than left lying).
    pub fn burst(&self) -> bool {
        self.button.is_some()
    }

    pub fn update(&mut self, h: &Hands<'_>, b: Burst) -> Status {
        let now = h.now;
        if let Some(t) = &mut self.trigger {
            let status = t.update(h, true);
            self.button = t.button;
            return status;
        }
        if now.since(self.thrown) > AIRBURST_WATCH {
            return Status::Failed(if b.satchel.is_none() {
                "the satchel was not in sight"
            } else if !b.spared {
                "the bot was in the blast"
            } else {
                "it did not come by the enemy"
            });
        }
        let gap = b.satchel.zip(b.enemy).map(|(s, e)| s.distance(e));
        if let Some(g) = gap {
            self.closest = Some(self.closest.map_or(g, |c| c.min(g)));
        }
        let near = gap.is_some_and(|g| g <= AIRBURST_REACH);
        if near && b.spared && now.since(self.thrown) >= RADIO_READY {
            self.trigger = Some(SatchelTrigger::new(now));
            return self.update(h, b);
        }
        // Back from the enemy while the satchel flies at it: the run-up carried the bot after its satchel.
        let back = b
            .enemy
            .filter(|_| now.since(self.thrown) < BACK_OFF_FOR)
            .map(|e| MoveIntent {
                dir: (h.origin - e).truncate().normalize_or_zero(),
                speed: BACK_OFF_SPEED,
            });
        Status::Running(Request {
            weapon: Some(hold(WeaponId::Satchel)),
            movement: back,
            ..Request::default()
        })
    }
}

/// Shooting a tripmine with `weapon`.
#[derive(Clone, Debug)]
pub struct MineShot {
    pub mine: Vec3,
    pub weapon: WeaponId,
    started: SimTime,
}

impl MineShot {
    pub fn new(mine: Vec3, weapon: WeaponId, now: SimTime) -> MineShot {
        MineShot {
            mine,
            weapon,
            started: now,
        }
    }

    /// `interval`: the pause between clicks of a semi-automatic gun.
    pub fn update(&mut self, h: &Hands<'_>, interval: f32) -> Status {
        if h.now.since(self.started) > SHOOT_FOR {
            return Status::Failed("the mine did not go off");
        }
        let angles = dir_to_view_angles(self.mine - h.eye);
        let s = spec(self.weapon);
        let weapon = if h.ready(self.weapon) && settled(h.view, angles, 0.4) {
            press(self.weapon, lb_game::mechanics::Attack::Primary, s.trigger, interval)
        } else {
            hold(self.weapon)
        };
        Status::Running(Request {
            weapon: Some(weapon),
            look: Some(LookIntent::Angles(angles)),
            movement: Some(super::stop()),
            jump: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Armed;
    use lb_game::dll::DllProfile;
    use lb_game::mechanics::Attack;
    use lb_game::self_state::{PredictedWeapon, Prediction};
    use lb_motor::Fire;

    fn hands<'a>(t: f64, arsenal: &'a [Armed], prediction: &'a Prediction, dll: DllProfile) -> Hands<'a> {
        Hands {
            now: SimTime(t),
            eye: Vec3::ZERO,
            origin: Vec3::ZERO,
            velocity: Vec3::ZERO,
            view: Vec3::ZERO,
            on_ground: true,
            on_ladder: false,
            waterlevel: 0,
            fov: 0.0,
            weapon: Some(WeaponId::Satchel),
            arsenal,
            prediction: Some(prediction),
            dll,
            gravity: 800.0,
        }
    }

    fn predicted(state: i32, count: i32) -> Prediction {
        held_back(state, count, 0.0, 0.0)
    }

    /// The game's clocks for the two buttons: seconds until it takes each again.
    fn held_back(state: i32, count: i32, primary: f32, secondary: f32) -> Prediction {
        let mut p = Prediction {
            current: Some(WeaponId::Satchel),
            primary_ammo: count,
            ..Prediction::default()
        };
        p.weapons[WeaponId::Satchel as usize] = Some(PredictedWeapon {
            charge_ready: state,
            next_primary: primary,
            next_secondary: secondary,
            ..PredictedWeapon::default()
        });
        p
    }

    fn fire(status: Status) -> Fire {
        match status {
            Status::Running(r) => r.weapon.unwrap().fire,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_satchels_go_off_with_the_detonate_button() {
        let dll = DllProfile::default();
        assert_eq!(dll.satchel_detonate(), Attack::Primary);
        let arsenal = [Armed::new(WeaponId::Satchel, None, Some(1))];
        let mut trigger = SatchelTrigger::new(SimTime(0.0));
        // Just thrown: the game takes the primary again only after a second.
        let fresh = held_back(1, 1, 0.4, 0.0);
        assert_eq!(
            fire(trigger.update(&hands(0.0, &arsenal, &fresh, dll), true)),
            Fire::None
        );
        let out = predicted(1, 1);
        assert_eq!(
            fire(trigger.update(&hands(0.6, &arsenal, &out, dll), true)),
            Fire::Primary
        );
        let gone = predicted(2, 1);
        assert_eq!(trigger.update(&hands(0.62, &arsenal, &gone, dll), true), Status::Done);
        assert_eq!(trigger.button, Some(Attack::Primary));
    }

    #[test]
    fn the_radio_waits_up_until_told() {
        let dll = DllProfile::default();
        let arsenal = [Armed::new(WeaponId::Satchel, None, Some(1))];
        let out = predicted(1, 1);
        let mut trigger = SatchelTrigger::new(SimTime(0.0));
        for t in [0.0, 1.0, 5.0] {
            let Status::Running(r) = trigger.update(&hands(t, &arsenal, &out, dll), false) else {
                panic!()
            };
            let w = r.weapon.unwrap();
            assert_eq!(
                (w.select, w.fire),
                (Some(WeaponId::Satchel), Fire::None),
                "held up at {t} s"
            );
        }
        assert_eq!(
            fire(trigger.update(&hands(5.1, &arsenal, &out, dll), true)),
            Fire::Primary
        );
    }

    #[test]
    fn a_detonate_press_that_throws_is_followed_by_the_other_button() {
        // The primary throws on this server (BugfixedHL-Rebased, the 2023 update), not the classic SDK's way.
        let dll = DllProfile::default();
        let mut trigger = SatchelTrigger::new(SimTime(0.0));
        let arsenal = [Armed::new(WeaponId::Satchel, None, Some(3))];
        let out = predicted(1, 3);
        assert_eq!(
            fire(trigger.update(&hands(0.0, &arsenal, &out, dll), true)),
            Fire::Primary
        );
        // A satchel thrown: the game holds the secondary back for half a second.
        let fewer = [Armed::new(WeaponId::Satchel, None, Some(2))];
        let thrown = held_back(1, 2, 1.0, 0.5);
        assert_eq!(
            fire(trigger.update(&hands(0.1, &fewer, &thrown, dll), true)),
            Fire::None,
            "let go before the other button"
        );
        let later = held_back(1, 2, 0.5, 0.0);
        assert_eq!(
            fire(trigger.update(&hands(0.6, &fewer, &later, dll), true)),
            Fire::Secondary
        );
        let gone = predicted(2, 2);
        assert_eq!(trigger.update(&hands(0.62, &fewer, &gone, dll), true), Status::Done);
        assert_eq!(trigger.button, Some(Attack::Secondary), "the button that set them off");
    }

    #[test]
    fn a_detonate_press_that_does_nothing_is_followed_by_the_other_button() {
        // Every satchel out and the pocket empty: the throw button does nothing at all.
        let dll = DllProfile::default();
        let mut trigger = SatchelTrigger::new(SimTime(0.0));
        let empty = [Armed::new(WeaponId::Satchel, None, Some(0))];
        let out = predicted(1, 0);
        assert_eq!(
            fire(trigger.update(&hands(0.0, &empty, &out, dll), true)),
            Fire::Primary
        );
        assert_eq!(
            fire(trigger.update(&hands(0.2, &empty, &out, dll), true)),
            Fire::Primary
        );
        assert_eq!(fire(trigger.update(&hands(0.35, &empty, &out, dll), true)), Fire::None);
        assert_eq!(
            fire(trigger.update(&hands(0.36, &empty, &out, dll), true)),
            Fire::Secondary
        );
        let gone = predicted(2, 0);
        assert_eq!(trigger.update(&hands(0.4, &empty, &gone, dll), true), Status::Done);
        assert_eq!(trigger.button, Some(Attack::Secondary));
        // Neither button does it: given up.
        let mut stuck = SatchelTrigger::new(SimTime(0.0));
        for t in [0.0, 0.35, 0.36, 0.8] {
            let _ = stuck.update(&hands(t, &empty, &out, dll), true);
        }
        assert_eq!(
            stuck.update(&hands(1.0, &empty, &out, dll), true),
            Status::Failed("the satchels did not go off")
        );
        assert_eq!(stuck.button, None);
    }

    #[test]
    fn a_satchel_goes_off_as_it_comes_by_the_enemy() {
        let dll = DllProfile::default();
        let arsenal = [Armed::new(WeaponId::Satchel, None, Some(2))];
        let out = predicted(1, 2);
        let enemy = Vec3::new(500.0, 0.0, 0.0);
        let mut a = Airburst::new(PlayerKey { slot: 3, userid: 3 }, SimTime(0.0));
        let flying = |x: f32, spared: bool| Burst {
            satchel: Some(Vec3::new(x, 0.0, 40.0)),
            enemy: Some(enemy),
            spared,
        };
        // Far from the enemy yet, then close but before the radio is ready: held.
        for (t, x) in [(0.2, 150.0), (0.4, 420.0)] {
            let Status::Running(r) = a.update(&hands(t, &arsenal, &out, dll), flying(x, true)) else {
                panic!()
            };
            assert_eq!(r.weapon.unwrap().fire, Fire::None);
        }
        // Close, but the bot would be in the blast: held.
        let Status::Running(r) = a.update(&hands(0.6, &arsenal, &out, dll), flying(430.0, false)) else {
            panic!()
        };
        assert_eq!(r.weapon.unwrap().fire, Fire::None);
        let Status::Running(r) = a.update(&hands(0.62, &arsenal, &out, dll), flying(440.0, true)) else {
            panic!()
        };
        assert_eq!(r.weapon.unwrap().fire, Fire::Primary, "set off by the enemy");
        let gone = predicted(2, 2);
        assert_eq!(
            a.update(&hands(0.7, &arsenal, &gone, dll), flying(450.0, true)),
            Status::Done
        );
        assert!(a.burst());
        // Never near the enemy: left lying.
        let mut miss = Airburst::new(PlayerKey { slot: 3, userid: 3 }, SimTime(0.0));
        assert_eq!(
            miss.update(&hands(3.0, &arsenal, &out, dll), flying(200.0, true)),
            Status::Failed("it did not come by the enemy")
        );
        assert!(!miss.burst());
    }
}
