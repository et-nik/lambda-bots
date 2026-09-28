//! Moods that come and go (yapb's emotions, with its over-clamp fixed): a fight in sight and kills make a bot bolder,
//! getting hurt makes a healthy bot bolder still and a hurt one warier; with nothing going on both drift back to the
//! personality's own. A mood never strays more than `SWAY` from the personality.

use lb_core::time::SimTime;

const STEP: f64 = 0.5;
/// An enemy seen this recently keeps the fight going.
const FIGHT: f64 = 1.0;
/// With no enemy for this long the moods settle.
const SETTLE_AFTER: f64 = 5.0;
const FIGHT_RISE: f32 = 0.02;
const SETTLE: f32 = 0.05;
const KILL: f32 = 0.1;
const HURT_BOLD: f32 = 0.05;
const HURT_WARY: f32 = 0.05;
/// Health above which being hurt makes a bot bolder rather than warier.
const HURT_HEALTH: f32 = 60.0;
const SWAY: f32 = 0.3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emotions {
    pub aggression: f32,
    pub fear: f32,
    base: (f32, f32),
    next: SimTime,
}

impl Default for Emotions {
    fn default() -> Self {
        Emotions::new(0.5, 0.5)
    }
}

impl Emotions {
    /// The personality's own aggression and fear.
    pub fn base(&self) -> (f32, f32) {
        self.base
    }

    pub fn new(aggression: f32, fear: f32) -> Emotions {
        Emotions {
            aggression,
            fear,
            base: (aggression, fear),
            next: SimTime::ZERO,
        }
    }

    /// Back to the personality's own (a new life).
    pub fn settle(&mut self) {
        (self.aggression, self.fear) = self.base;
    }

    fn clamp(&mut self) {
        let keep = |v: f32, base: f32| v.clamp((base - SWAY).max(0.0), (base + SWAY).min(1.0));
        self.aggression = keep(self.aggression, self.base.0);
        self.fear = keep(self.fear, self.base.1);
    }

    /// Every frame; the moods move twice a second.
    pub fn update(&mut self, now: SimTime, enemy_seen_at: Option<SimTime>) {
        if now < self.next {
            return;
        }
        self.next = now + STEP;
        let since = enemy_seen_at.map_or(f64::INFINITY, |t| now.since(t));
        if since <= FIGHT {
            self.aggression += FIGHT_RISE;
        } else if since >= SETTLE_AFTER {
            let toward = |v: f32, base: f32| v + (base - v).clamp(-SETTLE, SETTLE);
            self.aggression = toward(self.aggression, self.base.0);
            self.fear = toward(self.fear, self.base.1);
        }
        self.clamp();
    }

    pub fn on_kill(&mut self) {
        self.aggression += KILL;
        self.clamp();
    }

    /// Hurt by an enemy, `health` left.
    pub fn on_hurt(&mut self, health: f32) {
        if health > HURT_HEALTH {
            self.aggression += HURT_BOLD;
        } else {
            self.fear += HURT_WARY;
        }
        self.clamp();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fights_embolden_hurts_scare_and_calm_settles() {
        let mut e = Emotions::new(0.5, 0.5);
        for i in 0..40 {
            let t = SimTime(f64::from(i) * 0.5);
            e.update(t, Some(t));
        }
        assert!(
            (e.aggression - 0.8).abs() < 1e-5,
            "no bolder than the personality allows: {}",
            e.aggression
        );
        e.on_hurt(30.0);
        e.on_hurt(30.0);
        assert!((e.fear - 0.6).abs() < 1e-5);
        e.on_kill();
        assert!((e.aggression - 0.8).abs() < 1e-5);
        for i in 0..40 {
            e.update(SimTime(40.0 + f64::from(i) * 0.5), Some(SimTime(19.5)));
        }
        assert_eq!((e.aggression, e.fear), (0.5, 0.5), "back to itself after a calm spell");
        let mut rusher = Emotions::new(0.95, 0.1);
        rusher.on_kill();
        assert_eq!(rusher.aggression, 1.0, "clamped to 1");
    }
}
