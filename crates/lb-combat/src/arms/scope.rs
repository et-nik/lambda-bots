//! The crossbow's scope, used the way good players use it: snapped on for the shot and off again. Zoomed in
//! multiplayer the crossbow fires a hitscan bolt (120 damage); unzoomed, a slow bolt a moving target steps away from.
//!
//! The bot puts the scope on (the secondary attack), settles the aim for its skill's `scope_settle` (a tenth of a
//! second for experts, over a second for beginners) and fires once the view is on the target; a target it cannot
//! settle on in time gets no shot. Then the scope comes off: at once with a reload when few bolts are left in the clip
//! (a reload takes the scope off), otherwise as soon as the game lets the secondary attack toggle it again, a second
//! after it went on. The view is 20° wide only for that second.

use lb_core::time::SimTime;
use lb_game::mechanics::{Attack, Trigger};
use lb_game::weapons::WeaponId;
use lb_motor::WeaponIntent;

use super::{Hands, Request, Status, hold, press};

/// The scope comes on within this once the game lets the toggle work, or it will not.
const ON_TIMEOUT: f64 = 0.5;
/// The toggle's wait is a second at most: past this the scope will not come on.
const ON_GIVE_UP: f64 = 1.5;
/// After the settle time the shot waits this much longer for the view to come onto the target.
const LATE: f64 = 0.4;
const CONFIRM: f64 = 0.3;
const OFF_TIMEOUT: f64 = 1.8;
/// With this few bolts left after the shot, a reload takes the scope off (and fills the clip).
const RELOAD_AT: i32 = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    On,
    Aim { since: SimTime },
    Fired { at: SimTime, before: i32 },
    Off { since: SimTime },
}

#[derive(Clone, Debug)]
pub struct Scope {
    phase: Phase,
    started: SimTime,
    /// When the game's weapon data first showed the toggle ready (it may lag the toggle by a frame).
    toggle_ready: Option<SimTime>,
    /// Seconds the aim is settled before the shot.
    settle: f32,
    /// A shot went off.
    pub fired: bool,
}

fn zoomed(h: &Hands<'_>) -> bool {
    h.fov > 0.0 && h.fov < 89.0
}

impl Scope {
    pub fn new(now: SimTime, settle: f32) -> Scope {
        Scope {
            phase: Phase::On,
            started: now,
            toggle_ready: None,
            settle,
            fired: false,
        }
    }

    /// `on_target`: the view is on the target closely enough for the shot.
    pub fn update(&mut self, h: &Hands<'_>, on_target: bool) -> Status {
        let now = h.now;
        let w = WeaponId::Crossbow;
        let clip = h.predicted(w).map_or(0, |p| p.clip);
        match self.phase {
            Phase::On => {
                if zoomed(h) {
                    self.phase = Phase::Aim { since: now };
                    return self.update(h, on_target);
                }
                if h.predicted(w).is_none_or(|p| p.next_secondary <= 0.0) {
                    self.toggle_ready.get_or_insert(now);
                } else {
                    self.toggle_ready = None;
                }
                let waited = self.toggle_ready.map_or(0.0, |t| now.since(t));
                if waited > ON_TIMEOUT || now.since(self.started) > ON_GIVE_UP || h.weapon != Some(w) {
                    return Status::Failed("the scope did not come on");
                }
                Status::Running(Request {
                    weapon: Some(press(w, Attack::Secondary, Trigger::Hold, 0.0)),
                    ..Request::default()
                })
            }
            Phase::Aim { since } => {
                let held = now.since(since);
                if clip <= 0 || held >= f64::from(self.settle) + LATE {
                    self.phase = Phase::Off { since: now };
                    return self.update(h, on_target);
                }
                if held >= f64::from(self.settle) && on_target && h.ready(w) {
                    self.phase = Phase::Fired { at: now, before: clip };
                    self.fired = true;
                    return Status::Running(Request {
                        weapon: Some(press(w, Attack::Primary, Trigger::Hold, 0.0)),
                        ..Request::default()
                    });
                }
                Status::Running(Request {
                    weapon: Some(hold(w)),
                    ..Request::default()
                })
            }
            Phase::Fired { at, before } => {
                if clip < before || now.since(at) > CONFIRM {
                    self.phase = Phase::Off { since: now };
                    return Status::Running(Request {
                        weapon: Some(hold(w)),
                        ..Request::default()
                    });
                }
                Status::Running(Request {
                    weapon: Some(press(w, Attack::Primary, Trigger::Hold, 0.0)),
                    ..Request::default()
                })
            }
            Phase::Off { since } => {
                if !zoomed(h) || now.since(since) > OFF_TIMEOUT {
                    return Status::Done;
                }
                let secondary_ready = h.predicted(w).is_none_or(|p| p.next_secondary <= 0.0);
                let weapon = if clip <= RELOAD_AT && h.reserve(w) > 0 {
                    WeaponIntent {
                        reload: true,
                        ..hold(w)
                    }
                } else if secondary_ready {
                    press(w, Attack::Secondary, Trigger::Hold, 0.0)
                } else {
                    hold(w)
                };
                Status::Running(Request {
                    weapon: Some(weapon),
                    ..Request::default()
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Armed;
    use lb_core::Vec3;
    use lb_game::dll::DllProfile;
    use lb_game::self_state::{PredictedWeapon, Prediction};
    use lb_motor::Fire;

    /// The game's crossbow: the secondary attack toggles the scope with a second's wait, the primary fires a bolt.
    struct Game {
        fov: f32,
        clip: i32,
        next_secondary: f64,
        shots: Vec<(f64, bool)>,
    }

    impl Game {
        fn frame(&mut self, t: f64, w: Option<WeaponIntent>) {
            let Some(w) = w else { return };
            match w.fire {
                Fire::Secondary if t >= self.next_secondary => {
                    self.fov = if self.fov == 0.0 { 20.0 } else { 0.0 };
                    self.next_secondary = t + 1.0;
                }
                Fire::Primary if self.clip > 0 => {
                    self.clip -= 1;
                    self.shots.push((t, self.fov == 20.0));
                }
                _ => {}
            }
            if w.reload && self.fov != 0.0 {
                self.fov = 0.0;
            }
        }
    }

    fn run(settle: f32, on_target_from: f64, clip: i32) -> (Game, f64, Status) {
        run_after(settle, on_target_from, clip, 0.0)
    }

    /// `toggle_at`: when the game lets the toggle work again.
    fn run_after(settle: f32, on_target_from: f64, clip: i32, toggle_at: f64) -> (Game, f64, Status) {
        let mut g = Game {
            fov: 0.0,
            clip,
            next_secondary: toggle_at,
            shots: Vec::new(),
        };
        let mut s = Scope::new(SimTime(0.0), settle);
        let arsenal = [Armed::new(WeaponId::Crossbow, Some(clip), Some(10))];
        let mut t = 0.0;
        let mut status = Status::Done;
        while t < 3.0 {
            let mut prediction = Prediction {
                current: Some(WeaponId::Crossbow),
                primary_ammo: 10,
                ..Prediction::default()
            };
            prediction.weapons[WeaponId::Crossbow as usize] = Some(PredictedWeapon {
                clip: g.clip,
                next_secondary: (g.next_secondary - t) as f32,
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
                fov: g.fov,
                weapon: Some(WeaponId::Crossbow),
                arsenal: &arsenal,
                prediction: Some(&prediction),
                dll: DllProfile::default(),
                gravity: 800.0,
            };
            status = s.update(&h, t >= on_target_from);
            match status {
                Status::Running(r) => g.frame(t, r.weapon),
                _ => break,
            }
            t += 0.01;
        }
        (g, t, status)
    }

    #[test]
    fn scope_on_shot_scope_off_within_a_second_or_so() {
        let (g, t, status) = run(0.12, 0.0, 5);
        assert_eq!(status, Status::Done);
        assert_eq!(g.shots.len(), 1);
        let (at, zoomed) = g.shots[0];
        assert!(
            zoomed && (0.1..0.25).contains(&at),
            "a zoomed shot right after the scope came on: {at}"
        );
        assert!(t <= 1.1, "the scope came off when the game allowed: {t}");
        assert_eq!(g.fov, 0.0);
    }

    #[test]
    fn a_reload_takes_the_scope_off_at_once_when_the_clip_runs_low() {
        let (g, t, _) = run(0.12, 0.0, 2);
        assert_eq!(g.shots.len(), 1);
        assert!(t < 0.4, "off by the reload right after the shot: {t}");
        assert_eq!(g.fov, 0.0);
    }

    #[test]
    fn a_toggle_still_waiting_delays_the_scope_without_failing() {
        let (g, _, status) = run_after(0.12, 0.0, 5, 0.7);
        assert_eq!(status, Status::Done);
        assert_eq!(g.shots.len(), 1);
        let (at, zoomed) = g.shots[0];
        assert!(zoomed && (0.8..0.95).contains(&at), "shot once the scope came on: {at}");
        let (g, _, status) = run_after(0.12, 0.0, 5, 2.5);
        assert!(g.shots.is_empty());
        assert_eq!(status, Status::Failed("the scope did not come on"));
    }

    #[test]
    fn no_shot_without_the_view_on_the_target() {
        let (g, _, status) = run(0.12, 9.0, 5);
        assert!(g.shots.is_empty());
        assert_eq!(status, Status::Done);
        assert_eq!(g.fov, 0.0, "the scope still comes off");
    }
}
