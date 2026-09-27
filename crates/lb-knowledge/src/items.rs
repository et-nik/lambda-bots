//! What a bot believes about the items placed on the map. Where they are is static map knowledge; whether one is
//! there comes only from looking at its spot. At map start everything is assumed present. A spot seen empty is
//! expected back within a window: taken at some point after it was last seen, back one respawn time later. An
//! item believed present fades as time passes, faster with more opponents around to take it.

use lb_core::Vec3;
use lb_core::time::SimTime;
use lb_game::items::ItemKind;

/// Chance per second and per opponent that an unseen item was taken.
const TAKE_RATE: f32 = 0.005;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemSpot {
    pub kind: ItemKind,
    pub origin: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ItemState {
    /// Seen there, or assumed there at map start.
    Present,
    /// Seen missing; back somewhere in this window.
    Absent { back: (SimTime, SimTime) },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemBelief {
    pub state: ItemState,
    /// Last time the item was seen (or assumed) there.
    pub present_at: SimTime,
    /// Last time the spot was looked at.
    pub checked_at: Option<SimTime>,
}

#[derive(Clone, Debug, Default)]
pub struct Items {
    pub spots: Vec<ItemSpot>,
    pub beliefs: Vec<ItemBelief>,
}

impl Items {
    pub fn new(spots: &[ItemSpot], now: SimTime) -> Items {
        Items {
            spots: spots.to_vec(),
            beliefs: vec![
                ItemBelief {
                    state: ItemState::Present,
                    present_at: now,
                    checked_at: None,
                };
                spots.len()
            ],
        }
    }

    /// The spot was looked at and the item was there or not.
    pub fn observe(&mut self, spot: usize, present: bool, now: SimTime) {
        let Some(b) = self.beliefs.get_mut(spot) else { return };
        let respawn = f64::from(self.spots[spot].kind.respawn());
        b.checked_at = Some(now);
        if present {
            b.state = ItemState::Present;
            b.present_at = now;
            return;
        }
        b.state = match b.state {
            ItemState::Present => ItemState::Absent {
                back: (b.present_at + respawn, now + respawn),
            },
            // Still missing: not back before now; taken again if the window has passed.
            ItemState::Absent { back: (from, to) } if now <= to => ItemState::Absent {
                back: (from.max(now), to),
            },
            ItemState::Absent { .. } => ItemState::Absent {
                back: (now, now + respawn),
            },
        };
    }

    /// Chance the item is there on arrival in `eta` seconds, with `opponents` other players on the server.
    pub fn availability(&self, spot: usize, now: SimTime, eta: f32, opponents: usize) -> f32 {
        let Some(b) = self.beliefs.get(spot) else { return 0.0 };
        let arrive = now + f64::from(eta);
        let fade = |since: SimTime| (-TAKE_RATE * opponents as f32 * arrive.since(since).max(0.0) as f32).exp();
        match b.state {
            ItemState::Present => fade(b.present_at),
            ItemState::Absent { back: (from, to) } => {
                if arrive < from {
                    0.0
                } else if arrive >= to {
                    fade(to)
                } else {
                    (arrive.since(from) / to.since(from).max(1e-3)) as f32
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_game::weapons::WeaponId;

    #[test]
    fn a_taken_item_comes_back_within_its_window() {
        let spots = [ItemSpot {
            kind: ItemKind::Weapon(WeaponId::Shotgun),
            origin: Vec3::ZERO,
        }];
        let mut items = Items::new(&spots, SimTime(0.0));
        assert!(items.availability(0, SimTime(0.0), 2.0, 4) > 0.95);
        items.observe(0, true, SimTime(10.0));
        items.observe(0, false, SimTime(14.0));
        assert_eq!(
            items.beliefs[0].state,
            ItemState::Absent {
                back: (SimTime(30.0), SimTime(34.0))
            }
        );
        assert_eq!(
            items.availability(0, SimTime(15.0), 5.0, 4),
            0.0,
            "arrives before it is back"
        );
        assert!((items.availability(0, SimTime(15.0), 17.0, 4) - 0.5).abs() < 1e-3);
        assert!(items.availability(0, SimTime(40.0), 0.0, 4) > 0.8);
        items.observe(0, false, SimTime(40.0));
        assert_eq!(
            items.beliefs[0].state,
            ItemState::Absent {
                back: (SimTime(40.0), SimTime(60.0))
            },
            "taken again after it came back"
        );
    }
}
