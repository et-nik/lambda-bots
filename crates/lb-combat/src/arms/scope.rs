//! The crossbow's scope, used the way good players use it. Zoomed in multiplayer the crossbow fires a hitscan bolt
//! (120 damage); unzoomed, a slow bolt a moving target steps away from.
//!
//! The bot puts the scope on (the secondary attack), settles the aim for its skill's `scope_settle` (a tenth of a
//! second for experts, over a second for beginners) and fires once the view is on the target; with the view not on
//! it within a second more, the scope comes off. A miss is followed by another shot through the scope, as soon as the crossbow is
//! ready again (0.75 s) and the aim has settled. The scope comes off after the kill, and when the target stays out of
//! sight for a second, comes too close, stops being the target, the view cannot get back onto it, or the clip is
//! empty: at once with a reload when few bolts are left (a reload takes the scope off), otherwise as soon as the game
//! lets the secondary attack toggle it again, a second after it went on.

use lb_core::time::SimTime;
use lb_game::mechanics::{Attack, Trigger, spec};
use lb_game::weapons::WeaponId;
use lb_knowledge::PlayerKey;
use lb_motor::WeaponIntent;

use super::{Hands, Request, Status, hold, press};

/// The scope comes on within this once the game lets the toggle work, or it will not.
const ON_TIMEOUT: f64 = 0.5;
/// The toggle's wait is a second at most: past this the scope will not come on.
const ON_GIVE_UP: f64 = 1.5;
/// After the settle time (and after a miss, once the crossbow is ready again) a shot waits this much longer for the
/// view to come onto the target.
const LATE: f64 = 1.0;
/// A target out of sight this long is given up.
pub const LOST_HOLD: f64 = 1.0;
const CONFIRM: f64 = 0.3;
const OFF_TIMEOUT: f64 = 1.8;
/// With this few bolts left, a reload takes the scope off (and fills the clip).
const RELOAD_AT: i32 = 2;

/// What the scope knows of its target on this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sight {
    /// The view is on the target closely enough for the shot.
    pub on_target: bool,
    /// The target is in sight.
    pub seen: bool,
    /// Why the fight through the scope is over, if it is: the target died (the kill feed said so), stopped being the
    /// target, or came too close for the scope.
    pub done: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    On,
    /// Aiming: for the first shot, or for the next one after a miss.
    Aim {
        since: SimTime,
        follow: bool,
    },
    Fired {
        at: SimTime,
        before: i32,
    },
    Off {
        since: SimTime,
    },
}

#[derive(Clone, Debug)]
pub struct Scope {
    /// Whom the scope is on.
    pub target: PlayerKey,
    phase: Phase,
    started: SimTime,
    /// When the game's weapon data first showed the toggle ready (it may lag the toggle by a frame).
    toggle_ready: Option<SimTime>,
    /// Seconds the aim is settled before a shot.
    settle: f32,
    seen: SimTime,
    /// Shots fired through the scope.
    pub shots: u32,
    /// Why the scope came off.
    pub ended: Option<&'static str>,
}

fn zoomed(h: &Hands<'_>) -> bool {
    h.fov > 0.0 && h.fov < 89.0
}

impl Scope {
    pub fn new(now: SimTime, settle: f32, target: PlayerKey) -> Scope {
        Scope {
            target,
            phase: Phase::On,
            started: now,
            toggle_ready: None,
            settle,
            seen: now,
            shots: 0,
            ended: None,
        }
    }

    fn off(&mut self, now: SimTime, why: &'static str) {
        self.phase = Phase::Off { since: now };
        self.ended = Some(why);
    }

    pub fn update(&mut self, h: &Hands<'_>, sight: Sight) -> Status {
        let now = h.now;
        let w = WeaponId::Crossbow;
        let clip = h.predicted(w).map_or(0, |p| p.clip);
        if sight.seen {
            self.seen = now;
        }
        match self.phase {
            Phase::On => {
                if zoomed(h) {
                    self.phase = Phase::Aim {
                        since: now,
                        follow: false,
                    };
                    return self.update(h, sight);
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
            Phase::Aim { since, follow } => {
                let held = now.since(since);
                let settle = f64::from(self.settle);
                // After a shot the crossbow is ready again in its cycle; the time to get back on the target counts
                // from then.
                let ready_in = if follow { f64::from(spec(w).cycle) } else { 0.0 };
                let over = if let Some(why) = sight.done {
                    Some(why)
                } else if clip <= 0 {
                    Some("the clip is empty")
                } else if now.since(self.seen) > LOST_HOLD {
                    Some("out of sight")
                } else if held >= settle.max(ready_in) + LATE {
                    Some("not on the target")
                } else {
                    None
                };
                if let Some(why) = over {
                    self.off(now, why);
                    return self.update(h, sight);
                }
                let ready = h.ready(w) && h.predicted(w).is_none_or(|p| p.next_primary <= 0.0);
                if held >= settle && sight.on_target && ready {
                    self.phase = Phase::Fired { at: now, before: clip };
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
                if clip < before {
                    self.shots += 1;
                    // A miss: stay zoomed for the next shot; the kill shows in the kill feed and ends it.
                    self.phase = Phase::Aim {
                        since: now,
                        follow: true,
                    };
                    return Status::Running(Request {
                        weapon: Some(hold(w)),
                        ..Request::default()
                    });
                }
                if now.since(at) > CONFIRM {
                    self.off(now, "the shot did not go");
                    return self.update(h, sight);
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

    /// The game's crossbow: the secondary attack toggles the scope with a second's wait, the primary fires a bolt
    /// every 0.75 s; the target dies to the `kill_on`-th zoomed shot.
    struct Game {
        fov: f32,
        clip: i32,
        next_secondary: f64,
        next_primary: f64,
        shots: Vec<(f64, bool)>,
        kill_on: usize,
    }

    impl Game {
        fn frame(&mut self, t: f64, w: Option<WeaponIntent>) {
            let Some(w) = w else { return };
            match w.fire {
                Fire::Secondary if t >= self.next_secondary => {
                    self.fov = if self.fov == 0.0 { 20.0 } else { 0.0 };
                    self.next_secondary = t + 1.0;
                }
                Fire::Primary if self.clip > 0 && t >= self.next_primary => {
                    self.clip -= 1;
                    self.next_primary = t + 0.75;
                    self.shots.push((t, self.fov == 20.0));
                }
                _ => {}
            }
            if w.reload && self.fov != 0.0 {
                self.fov = 0.0;
            }
        }

        fn killed(&self) -> bool {
            self.shots.len() >= self.kill_on
        }
    }

    struct Run {
        settle: f32,
        clip: i32,
        /// When the game lets the toggle work again.
        toggle_at: f64,
        kill_on: usize,
        on_target_from: f64,
        /// The target is out of sight from then on.
        hidden_from: f64,
    }

    impl Default for Run {
        fn default() -> Run {
            Run {
                settle: 0.12,
                clip: 5,
                toggle_at: 0.0,
                kill_on: 1,
                on_target_from: 0.0,
                hidden_from: f64::INFINITY,
            }
        }
    }

    impl Run {
        fn go(self) -> (Game, f64, Status) {
            let mut g = Game {
                fov: 0.0,
                clip: self.clip,
                next_secondary: self.toggle_at,
                next_primary: 0.0,
                shots: Vec::new(),
                kill_on: self.kill_on,
            };
            let mut s = Scope::new(SimTime(0.0), self.settle, PlayerKey { slot: 2, userid: 2 });
            let arsenal = [Armed::new(WeaponId::Crossbow, Some(self.clip), Some(10))];
            let mut t = 0.0;
            let mut status = Status::Done;
            while t < 6.0 {
                let mut prediction = Prediction {
                    current: Some(WeaponId::Crossbow),
                    primary_ammo: 10,
                    ..Prediction::default()
                };
                prediction.weapons[WeaponId::Crossbow as usize] = Some(PredictedWeapon {
                    clip: g.clip,
                    next_secondary: (g.next_secondary - t) as f32,
                    next_primary: (g.next_primary - t) as f32,
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
                let seen = t < self.hidden_from;
                let sight = Sight {
                    on_target: seen && t >= self.on_target_from,
                    seen,
                    done: g.killed().then_some("the target died"),
                };
                status = s.update(&h, sight);
                match status {
                    Status::Running(r) => g.frame(t, r.weapon),
                    _ => break,
                }
                t += 0.01;
            }
            (g, t, status)
        }
    }

    #[test]
    fn scope_on_shot_scope_off_after_the_kill() {
        let (g, t, status) = Run::default().go();
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
    fn a_miss_is_followed_by_another_shot_through_the_scope() {
        let (g, t, status) = Run {
            kill_on: 3,
            ..Run::default()
        }
        .go();
        assert_eq!(status, Status::Done);
        assert_eq!(g.shots.len(), 3, "{:?}", g.shots);
        assert!(
            g.shots.iter().all(|(_, zoomed)| *zoomed),
            "all through the scope: {:?}",
            g.shots
        );
        assert!(
            g.shots.windows(2).all(|p| p[1].0 - p[0].0 < 0.9),
            "as soon as the crossbow is ready: {:?}",
            g.shots
        );
        assert!(t < g.shots[2].0 + 0.2, "off right after the kill: {t}");
        assert_eq!(g.fov, 0.0);
    }

    #[test]
    fn a_reload_takes_the_scope_off_when_the_clip_runs_low_or_out() {
        let (g, t, _) = Run {
            clip: 2,
            ..Run::default()
        }
        .go();
        assert_eq!(g.shots.len(), 1);
        assert!(t < 0.4, "off by the reload right after the kill: {t}");
        assert_eq!(g.fov, 0.0);
        // Two bolts and a target that takes three: both go through the scope, then the reload.
        let (g, _, status) = Run {
            clip: 2,
            kill_on: 3,
            ..Run::default()
        }
        .go();
        assert_eq!(status, Status::Done);
        assert_eq!(g.shots.len(), 2);
        assert_eq!(g.fov, 0.0);
    }

    #[test]
    fn a_target_gone_from_sight_ends_the_zoom() {
        let (g, t, status) = Run {
            kill_on: 9,
            hidden_from: 0.3,
            ..Run::default()
        }
        .go();
        assert_eq!(status, Status::Done);
        assert_eq!(g.shots.len(), 1);
        assert!((1.2..1.6).contains(&t), "a second's wait for it to show again: {t}");
        assert_eq!(g.fov, 0.0);
    }

    #[test]
    fn a_toggle_still_waiting_delays_the_scope_without_failing() {
        let (g, _, status) = Run {
            toggle_at: 0.7,
            ..Run::default()
        }
        .go();
        assert_eq!(status, Status::Done);
        assert_eq!(g.shots.len(), 1);
        let (at, zoomed) = g.shots[0];
        assert!(zoomed && (0.8..0.95).contains(&at), "shot once the scope came on: {at}");
        let (g, _, status) = Run {
            toggle_at: 2.5,
            ..Run::default()
        }
        .go();
        assert!(g.shots.is_empty());
        assert_eq!(status, Status::Failed("the scope did not come on"));
    }

    #[test]
    fn no_shot_without_the_view_on_the_target() {
        let (g, _, status) = Run {
            on_target_from: 9.0,
            ..Run::default()
        }
        .go();
        assert!(g.shots.is_empty());
        assert_eq!(status, Status::Done);
        assert_eq!(g.fov, 0.0, "the scope still comes off");
    }
}
