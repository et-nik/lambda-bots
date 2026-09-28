//! What a bot believes about the items placed on the map. Where they are is static map knowledge; whether one is
//! there comes only from looking at its spot or hearing it taken or come back. At map start everything is assumed
//! present. A spot seen empty is expected back within a window: taken at some point after it was last seen, back one
//! respawn time later. An item believed present fades as time passes, faster with more opponents around to take it.
//!
//! Respawn times are the game's (items 30 s, weapons and ammo 20 s) until the bot has timed a few itself: an item seen
//! or heard taken and seen or heard back, both to within a second.

use lb_core::Vec3;
use lb_core::dmath;
use lb_core::time::SimTime;
use lb_game::items::ItemKind;
use lb_game::sounds::SoundKind;

/// Chance per second and per opponent that an unseen item was taken.
const TAKE_RATE: f32 = 0.005;
/// Looks this close together pin when an item went or came back.
const PIN: f64 = 1.0;
/// Timed respawns needed before they replace the game's times, and how much a new one counts.
const SAMPLES: u32 = 2;
const LEARN_RATE: f32 = 0.3;
const RESPAWN_RANGE: [f32; 2] = [5.0, 120.0];

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
    /// When it was taken, when that is known to within a second.
    pub taken_at: Option<SimTime>,
}

/// Items with respawn times of their own: health, armor and the long jump; weapons; ammo.
pub const CLASSES: usize = 3;

pub fn class_of(kind: ItemKind) -> usize {
    match kind {
        ItemKind::Health | ItemKind::Battery | ItemKind::LongJump => 0,
        ItemKind::Weapon(_) => 1,
        ItemKind::Ammo(_) => 2,
    }
}

/// A respawn time timed: its mean and how many timings it rests on.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Learned {
    pub mean: f32,
    pub samples: u32,
}

/// Timings a server keeps at most: new ones keep moving the mean.
const KEEP_SAMPLES: u32 = 20;

impl Learned {
    /// `self` (the server's) with what a bot timed itself (`own`) added in.
    pub fn merge(&mut self, own: &Learned) {
        if own.samples == 0 {
            return;
        }
        let n = self.samples + own.samples;
        self.mean = (self.mean * self.samples as f32 + own.mean * own.samples as f32) / n as f32;
        self.samples = n.min(KEEP_SAMPLES);
    }

    fn add(&mut self, sample: f32) {
        let s = sample.clamp(RESPAWN_RANGE[0], RESPAWN_RANGE[1]);
        self.mean = if self.samples == 0 {
            s
        } else {
            self.mean + LEARN_RATE * (s - self.mean)
        };
        self.samples += 1;
    }
}

#[derive(Clone, Debug, Default)]
pub struct Items {
    pub spots: Vec<ItemSpot>,
    pub beliefs: Vec<ItemBelief>,
    /// Respawn times as the bot knows them: timed on this server before, then by itself.
    pub learned: [Learned; CLASSES],
    /// What the bot timed itself, for the server to keep.
    pub own: [Learned; CLASSES],
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
                    taken_at: None,
                };
                spots.len()
            ],
            learned: [Learned::default(); CLASSES],
            own: [Learned::default(); CLASSES],
        }
    }

    /// Respawn times timed before (on this server, in earlier games) to start from.
    pub fn with_learned(mut self, learned: [Learned; CLASSES]) -> Items {
        self.learned = learned;
        self
    }

    /// Seconds before a taken item of this kind comes back, as far as the bot knows.
    pub fn respawn(&self, kind: ItemKind) -> f32 {
        let l = self.learned[class_of(kind)];
        if l.samples >= SAMPLES { l.mean } else { kind.respawn() }
    }

    fn came_back(&mut self, spot: usize, at: SimTime) {
        if let Some(taken) = self.beliefs[spot].taken_at {
            let class = class_of(self.spots[spot].kind);
            self.learned[class].add(at.since(taken) as f32);
            self.own[class].add(at.since(taken) as f32);
        }
    }

    /// The spot was looked at and the item was there or not.
    pub fn observe(&mut self, spot: usize, present: bool, now: SimTime) {
        let Some(&b) = self.beliefs.get(spot) else { return };
        let respawn = f64::from(self.respawn(self.spots[spot].kind));
        let last_look = b.checked_at.filter(|t| now.since(*t) <= PIN);
        let b = &mut self.beliefs[spot];
        b.checked_at = Some(now);
        if present {
            if matches!(b.state, ItemState::Absent { .. })
                && let Some(t) = last_look
            {
                self.came_back(spot, t + now.since(t) / 2.0);
            }
            let b = &mut self.beliefs[spot];
            b.state = ItemState::Present;
            b.present_at = now;
            b.taken_at = None;
            return;
        }
        b.state = match b.state {
            ItemState::Present => {
                b.taken_at = last_look.map(|t| t + now.since(t) / 2.0);
                ItemState::Absent {
                    back: (b.present_at + respawn, now + respawn),
                }
            }
            // Still missing: not back before now; taken again if the window has passed.
            ItemState::Absent { back: (from, to) } if now <= to => ItemState::Absent {
                back: (from.max(now), to),
            },
            // Past its window: late if the spot was watched all along, else maybe back and taken again.
            ItemState::Absent { .. } => {
                if last_look.is_none() {
                    b.taken_at = None;
                }
                ItemState::Absent {
                    back: (now, now + respawn),
                }
            }
        };
    }

    /// A pickup or a respawn was heard around `pos` (within `reach`): it is the item of the only spot there.
    pub fn heard(&mut self, kind: SoundKind, pos: Vec3, reach: f32, now: SimTime) {
        let mut near = self
            .spots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.origin.distance(pos) <= reach)
            .map(|(i, _)| i);
        let (Some(spot), None) = (near.next(), near.next()) else {
            return;
        };
        let respawn = f64::from(self.respawn(self.spots[spot].kind));
        match (kind, self.beliefs[spot].state) {
            (SoundKind::Pickup, ItemState::Present) => {
                let b = &mut self.beliefs[spot];
                b.state = ItemState::Absent {
                    back: (now + respawn, now + respawn),
                };
                b.taken_at = Some(now);
            }
            (SoundKind::ItemRespawn, ItemState::Absent { .. }) => {
                self.came_back(spot, now);
                let b = &mut self.beliefs[spot];
                b.state = ItemState::Present;
                b.present_at = now;
                b.taken_at = None;
            }
            (SoundKind::ItemRespawn, ItemState::Present) => self.beliefs[spot].present_at = now,
            _ => {}
        }
    }

    /// Chance the item is there on arrival in `eta` seconds, with `opponents` other players on the server.
    pub fn availability(&self, spot: usize, now: SimTime, eta: f32, opponents: usize) -> f32 {
        let Some(b) = self.beliefs.get(spot) else { return 0.0 };
        let arrive = now + f64::from(eta);
        let fade = |since: SimTime| dmath::exp(-TAKE_RATE * opponents as f32 * arrive.since(since).max(0.0) as f32);
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

    #[test]
    fn respawns_heard_and_watched_are_timed_and_learned() {
        let spots = [
            ItemSpot {
                kind: ItemKind::Battery,
                origin: Vec3::ZERO,
            },
            ItemSpot {
                kind: ItemKind::Health,
                origin: Vec3::new(500.0, 0.0, 0.0),
            },
        ];
        let mut items = Items::new(&spots, SimTime(0.0));
        // Heard taken at 10, heard back at 55: a server with 45 s items.
        items.heard(SoundKind::Pickup, Vec3::new(40.0, 0.0, 0.0), 128.0, SimTime(10.0));
        assert_eq!(
            items.beliefs[0].state,
            ItemState::Absent {
                back: (SimTime(40.0), SimTime(40.0))
            }
        );
        items.heard(SoundKind::ItemRespawn, Vec3::new(-30.0, 0.0, 0.0), 128.0, SimTime(55.0));
        assert_eq!(items.beliefs[0].state, ItemState::Present);
        assert_eq!(items.respawn(ItemKind::Battery), 30.0, "one timing is not enough");
        // Watched taken between 60 and 60.5, watched all along until back between 105 and 105.5.
        items.observe(0, true, SimTime(60.0));
        for i in 1..=90 {
            items.observe(0, false, SimTime(60.0 + f64::from(i) * 0.5));
        }
        items.observe(0, true, SimTime(105.5));
        assert!(
            (items.respawn(ItemKind::Health) - 45.0).abs() < 0.5,
            "items share their time"
        );
        assert_eq!(items.respawn(ItemKind::Weapon(WeaponId::Mp5)), 20.0);
        // Two spots within reach: the sound is not tied to either.
        items.heard(SoundKind::Pickup, Vec3::new(250.0, 0.0, 0.0), 300.0, SimTime(110.0));
        assert_eq!(items.beliefs[1].state, ItemState::Present);
    }
}
