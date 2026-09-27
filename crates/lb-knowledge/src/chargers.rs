//! What a bot believes about the map's wall chargers. Where they are is static map knowledge; whether one has
//! anything left shows only on its face (a spent charger's display goes dark) or when using it gives nothing. A spent
//! charger is full again 60 s (health) or 30 s (suit) later in multiplayer.

use lb_core::Vec3;
use lb_core::time::SimTime;

pub const HEALTH_RECHARGE: f64 = 60.0;
pub const SUIT_RECHARGE: f64 = 30.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChargerSpot {
    /// A suit charger rather than a health one.
    pub suit: bool,
    /// Brush model of the charger (`*N`).
    pub model: u16,
    pub center: Vec3,
    /// Standing origin in reach and sight of it.
    pub spot: Vec3,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChargerBelief {
    /// Spent until then.
    pub empty_until: Option<SimTime>,
    pub checked_at: Option<SimTime>,
}

#[derive(Clone, Debug, Default)]
pub struct Chargers {
    pub spots: Vec<ChargerSpot>,
    pub beliefs: Vec<ChargerBelief>,
}

impl Chargers {
    pub fn new(spots: &[ChargerSpot]) -> Chargers {
        Chargers {
            spots: spots.to_vec(),
            beliefs: vec![ChargerBelief::default(); spots.len()],
        }
    }

    fn recharge(&self, i: usize) -> f64 {
        if self.spots[i].suit {
            SUIT_RECHARGE
        } else {
            HEALTH_RECHARGE
        }
    }

    /// The charger's face was seen lit (`empty` false) or dark.
    pub fn observe(&mut self, i: usize, empty: bool, now: SimTime) {
        if i >= self.beliefs.len() {
            return;
        }
        let recharge = self.recharge(i);
        let b = &mut self.beliefs[i];
        b.checked_at = Some(now);
        if !empty {
            b.empty_until = None;
        } else if b.empty_until.is_none_or(|t| t <= now) {
            // Spent at some point before now: back within the recharge time.
            b.empty_until = Some(now + recharge);
        }
    }

    /// Using it gave nothing: spent now.
    pub fn drained(&mut self, i: usize, now: SimTime) {
        if i < self.beliefs.len() {
            let until = now + self.recharge(i);
            self.beliefs[i].empty_until = Some(until);
        }
    }

    /// Believed to have something left at `at`.
    pub fn available(&self, i: usize, at: SimTime) -> bool {
        self.beliefs
            .get(i)
            .is_some_and(|b| b.empty_until.is_none_or(|t| at >= t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spent_chargers_come_back() {
        let spots = [ChargerSpot {
            suit: true,
            model: 12,
            center: Vec3::ZERO,
            spot: Vec3::X * 40.0,
        }];
        let mut c = Chargers::new(&spots);
        assert!(c.available(0, SimTime(0.0)));
        c.observe(0, true, SimTime(10.0));
        assert!(!c.available(0, SimTime(20.0)));
        assert!(c.available(0, SimTime(40.0)), "a suit charger is back in 30 s");
        c.observe(0, false, SimTime(15.0));
        assert!(c.available(0, SimTime(15.0)), "seen lit again");
        c.drained(0, SimTime(50.0));
        assert!(!c.available(0, SimTime(60.0)));
    }
}
