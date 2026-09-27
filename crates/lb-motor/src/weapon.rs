//! The weapon controller: selects weapons with their client command and waits for `CurWeapon` to confirm, keeps
//! the trigger off while switching and for the 0.5 s deploy, and works the trigger: held for automatic weapons,
//! clicked at a cadence for the others.

use lb_core::time::SimTime;
use lb_game::input::{IN_ATTACK, IN_ATTACK2, IN_RELOAD};
use lb_game::mechanics::Trigger;
use lb_game::weapons::WeaponId;
use smallvec::SmallVec;

const CONFIRM_TIMEOUT: f64 = 1.0;
const SELECT_ATTEMPTS: u8 = 3;
const DEPLOY: f64 = 0.5;
const RELOAD_REPEAT: f64 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fire {
    None,
    Primary,
    Secondary,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaponIntent {
    pub select: Option<WeaponId>,
    pub fire: Fire,
    pub trigger: Trigger,
    /// Seconds between clicks for `Trigger::Tap`.
    pub interval: f32,
    pub reload: bool,
}

impl WeaponIntent {
    pub fn hold(select: WeaponId) -> WeaponIntent {
        WeaponIntent {
            select: Some(select),
            fire: Fire::None,
            trigger: Trigger::Hold,
            interval: 0.0,
            reload: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct WeaponController {
    confirmed: Option<WeaponId>,
    pending: Option<(WeaponId, SimTime)>,
    attempts: u8,
    deploy_until: SimTime,
    last_click: Option<SimTime>,
    last_reload: Option<SimTime>,
    /// Last command with the trigger pulled.
    pub fired_at: Option<SimTime>,
}

impl WeaponController {
    pub fn reset(&mut self) {
        *self = WeaponController::default();
    }

    /// Weapon switch in progress or the deploy not finished.
    pub fn busy(&self, now: SimTime) -> bool {
        self.pending.is_some() || now < self.deploy_until
    }

    /// Buttons for this frame; select commands go to `commands`. `current` is the weapon `CurWeapon` confirmed.
    pub fn update(
        &mut self,
        now: SimTime,
        current: Option<WeaponId>,
        intent: Option<&WeaponIntent>,
        commands: &mut SmallVec<[String; 2]>,
    ) -> u16 {
        if current != self.confirmed {
            if current.is_some() {
                self.deploy_until = now + DEPLOY;
            }
            self.confirmed = current;
            if self.pending.is_some_and(|(w, _)| Some(w) == current) {
                self.pending = None;
                self.attempts = 0;
            }
        }
        let Some(intent) = intent else { return 0 };
        if let Some(want) = intent.select
            && Some(want) != current
        {
            let waiting = self
                .pending
                .is_some_and(|(w, t)| w == want && now.since(t) < CONFIRM_TIMEOUT);
            if !waiting {
                if self.pending.is_none_or(|(w, _)| w != want) {
                    self.attempts = 0;
                }
                if self.attempts < SELECT_ATTEMPTS {
                    commands.push(want.classname().to_string());
                    self.pending = Some((want, now));
                    self.attempts += 1;
                }
            }
            return 0;
        }
        self.pending = None;
        if now < self.deploy_until {
            return 0;
        }
        let bit = match intent.fire {
            Fire::None => 0,
            Fire::Primary => IN_ATTACK,
            Fire::Secondary => IN_ATTACK2,
        };
        if bit != 0 {
            let due = self
                .last_click
                .is_none_or(|t| now.since(t) >= f64::from(intent.interval));
            if intent.trigger == Trigger::Hold || due {
                self.last_click = Some(now);
                self.fired_at = Some(now);
                return bit;
            }
            return 0;
        }
        if intent.reload && self.last_reload.is_none_or(|t| now.since(t) >= RELOAD_REPEAT) {
            self.last_reload = Some(now);
            return IN_RELOAD;
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fire(select: WeaponId, trigger: Trigger, interval: f32) -> WeaponIntent {
        WeaponIntent {
            select: Some(select),
            fire: Fire::Primary,
            trigger,
            interval,
            reload: false,
        }
    }

    #[test]
    fn switches_then_waits_for_the_deploy_before_firing() {
        let mut c = WeaponController::default();
        let mut cmds = SmallVec::new();
        let want = fire(WeaponId::Mp5, Trigger::Hold, 0.0);
        assert_eq!(c.update(SimTime(0.0), Some(WeaponId::Glock), Some(&want), &mut cmds), 0);
        assert_eq!(cmds.as_slice(), ["weapon_9mmAR"]);
        assert_eq!(c.update(SimTime(0.2), Some(WeaponId::Glock), Some(&want), &mut cmds), 0);
        assert_eq!(cmds.len(), 1, "no second command while waiting for CurWeapon");
        assert_eq!(
            c.update(SimTime(0.3), Some(WeaponId::Mp5), Some(&want), &mut cmds),
            0,
            "deploying"
        );
        assert_eq!(
            c.update(SimTime(0.81), Some(WeaponId::Mp5), Some(&want), &mut cmds),
            IN_ATTACK
        );
        assert_eq!(
            c.update(SimTime(0.82), Some(WeaponId::Mp5), Some(&want), &mut cmds),
            IN_ATTACK,
            "held"
        );
    }

    #[test]
    fn clicks_keep_their_cadence_and_selects_give_up() {
        let mut c = WeaponController::default();
        let mut cmds = SmallVec::new();
        let tap = fire(WeaponId::Glock, Trigger::Tap, 0.5);
        c.update(SimTime(0.0), Some(WeaponId::Glock), None, &mut cmds);
        let presses: Vec<f64> = (0..200)
            .map(|i| 0.5 + i as f64 * 0.01)
            .filter(|&t| c.update(SimTime(t), Some(WeaponId::Glock), Some(&tap), &mut cmds) != 0)
            .collect();
        assert_eq!(presses.len(), 4, "{presses:?}");
        let want = fire(WeaponId::Rpg, Trigger::Tap, 0.0);
        for i in 0..10 {
            c.update(
                SimTime(3.0 + f64::from(i)),
                Some(WeaponId::Glock),
                Some(&want),
                &mut cmds,
            );
        }
        assert_eq!(cmds.len(), 3, "three attempts at a weapon the game does not switch to");
    }
}
