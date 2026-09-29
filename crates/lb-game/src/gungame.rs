//! GunGame as a player sees it. The plugin writes every player's level to the scoreboard as frags (level × 100, set on
//! each level change; kills on the level add to it between), so the scoreboard tells everyone's level, and its first
//! line (most frags, then fewest deaths) is the leader. What a level gave the bot is what it carries: one gun, or
//! a throwable, or tripmines with a glock that only sets them off, or on the last level the crowbar. The order of
//! the levels is not needed: a player on the last level shows the crowbar in its hands.

use crate::weapons::{WeaponId, weapons_in_mask};

/// Player slots are 1-based; index 0 is unused.
pub const SLOTS: usize = 33;

/// What a bot's level gave it to fight with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kit {
    /// One gun.
    Gun(WeaponId),
    /// Hand grenades, snarks or satchels.
    Throwable(WeaponId),
    /// Tripmines, with a glock that hurts nobody on this level and only sets the mines off.
    Mines,
    /// The crowbar alone: the last level, or the warmup.
    Crowbar,
    /// Not what a level gives: weapons from elsewhere, or none for the moment between two levels.
    Other,
}

impl Kit {
    /// The level a bot carrying `weapons` (`entvars.weapons`) is on, as yapb tells it (`isGunGameMeleeLevel`,
    /// `isGunGameTripmineLevel`, `refreshForcedWeapon`).
    pub fn of(weapons: u32) -> Kit {
        let owned = || weapons_in_mask(weapons);
        if owned().any(|w| w == WeaponId::Tripmine) && !owned().any(WeaponId::is_primary) {
            return Kit::Mines;
        }
        let mut guns = owned().filter(|w| !w.is_melee() && !w.is_throwable());
        if let Some(gun) = guns.next() {
            return if guns.next().is_none() {
                Kit::Gun(gun)
            } else {
                Kit::Other
            };
        }
        let mut throwables = owned().filter(|w| w.is_throwable());
        match (throwables.next(), throwables.next()) {
            (Some(t), None) => Kit::Throwable(t),
            (Some(_), Some(_)) => Kit::Other,
            (None, _) if owned().any(WeaponId::is_melee) => Kit::Crowbar,
            (None, _) => Kit::Other,
        }
    }

    /// Weapons the kit fights with, as a mask of weapon bits (all of them for `Other`).
    pub fn weapons(self) -> u32 {
        match self {
            Kit::Gun(w) | Kit::Throwable(w) => w.bit(),
            Kit::Mines => WeaponId::Tripmine.bit() | WeaponId::Glock.bit(),
            Kit::Crowbar => WeaponId::Crowbar.bit(),
            Kit::Other => u32::MAX,
        }
    }

    /// The weapon to hold when nothing can be fired: the one the level gave.
    pub fn main(self) -> Option<WeaponId> {
        match self {
            Kit::Gun(w) | Kit::Throwable(w) => Some(w),
            Kit::Mines => Some(WeaponId::Tripmine),
            Kit::Crowbar => Some(WeaponId::Crowbar),
            Kit::Other => None,
        }
    }

    /// Whether `w` hurts other players: the plugin blocks damage by anything but the level's weapons, and the glock
    /// of the tripmine level is not one of them.
    pub fn hurts_with(self, w: WeaponId) -> bool {
        self.weapons() & w.bit() != 0 && !(self == Kit::Mines && w == WeaponId::Glock)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kit::Gun(_) => "gun",
            Kit::Throwable(_) => "throwable",
            Kit::Mines => "mines",
            Kit::Crowbar => "crowbar",
            Kit::Other => "other",
        }
    }
}

/// The scoreboard's view of a match: every player's level and who leads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Board {
    /// Level by slot; `None` for a slot nobody plays in.
    pub levels: [Option<i16>; SLOTS],
    /// The scoreboard's first line.
    pub leader: Option<u8>,
    /// A suicide costs a kill, and a level below the first kill of one (`gg_descore`, FFA only).
    pub descore: bool,
}

impl Board {
    /// The board of `players`, each `(slot, frags, deaths)`, with `per_level` frags a level.
    pub fn new(players: impl IntoIterator<Item = (u8, i32, i32)>, per_level: i32, descore: bool) -> Board {
        let mut levels = [None; SLOTS];
        let mut first: Option<(u8, i32, i32)> = None;
        for (slot, frags, deaths) in players {
            let Some(level) = levels.get_mut(slot as usize) else {
                continue;
            };
            *level = Some(frags.div_euclid(per_level.max(1)).clamp(0, i32::from(i16::MAX)) as i16);
            // As the scoreboard sorts: frags down, then deaths up, then by slot.
            let ahead = first.is_none_or(|(s, f, d)| (frags, -deaths, -i32::from(slot)) > (f, -d, -i32::from(s)));
            if ahead {
                first = Some((slot, frags, deaths));
            }
        }
        Board {
            levels,
            leader: first.map(|(slot, ..)| slot),
            descore,
        }
    }

    pub fn level(&self, slot: u8) -> Option<i32> {
        self.levels.get(slot as usize).copied().flatten().map(i32::from)
    }

    /// The highest level anyone is on.
    pub fn top(&self) -> i32 {
        self.levels.iter().flatten().copied().max().map_or(0, i32::from)
    }
}

/// What one bot knows of the match: the board, its own level and what that level gave it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GunGame {
    pub board: Board,
    pub slot: u8,
    pub level: i32,
    pub kit: Kit,
    /// Kills do not count yet: the bot on the first level with only the crowbar, the plugin's warmup kit (it stays so
    /// while only bots are on the server).
    pub warmup: bool,
}

impl GunGame {
    /// The match as the bot in `slot`, carrying `weapons`, sees it.
    pub fn new(board: &Board, slot: u8, weapons: u32) -> GunGame {
        let level = board.level(slot).unwrap_or(0);
        let kit = Kit::of(weapons);
        GunGame {
            board: *board,
            slot,
            level,
            kit,
            warmup: kit == Kit::Crowbar && level == 0,
        }
    }

    pub fn leads(&self) -> bool {
        self.board.leader == Some(self.slot)
    }

    /// Levels the bot is behind the leader.
    pub fn behind(&self) -> i32 {
        (self.board.top() - self.level).max(0)
    }

    /// The player in `slot`, seen with `weapon` in hand, is on the last level: one kill from winning.
    pub fn on_last_level(&self, slot: u8, weapon: Option<WeaponId>) -> bool {
        !self.warmup && weapon == Some(WeaponId::Crowbar) && self.board.level(slot).is_some_and(|l| l > 0)
    }

    /// A suicide would cost the bot a kill.
    pub fn descore(&self) -> bool {
        self.board.descore && !self.warmup
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(ws: &[WeaponId]) -> u32 {
        ws.iter().fold(1 << crate::weapons::WEAPON_SUIT_BIT, |m, w| m | w.bit())
    }

    #[test]
    fn a_level_is_told_by_what_it_gave() {
        use WeaponId::*;
        assert_eq!(Kit::of(mask(&[Rpg])), Kit::Gun(Rpg));
        assert_eq!(
            Kit::of(mask(&[Crowbar, Mp5])),
            Kit::Gun(Mp5),
            "a crowbar next to one gun"
        );
        assert_eq!(Kit::of(mask(&[Tripmine, Glock])), Kit::Mines);
        assert_eq!(Kit::of(mask(&[Tripmine])), Kit::Mines);
        assert_eq!(Kit::of(mask(&[HandGrenade])), Kit::Throwable(HandGrenade));
        assert_eq!(Kit::of(mask(&[Snark])), Kit::Throwable(Snark));
        assert_eq!(Kit::of(mask(&[Crowbar])), Kit::Crowbar);
        assert_eq!(Kit::of(mask(&[Crowbar, Glock])), Kit::Gun(Glock), "the HLDM spawn kit");
        assert_eq!(Kit::of(mask(&[Crowbar, Glock, Shotgun])), Kit::Other);
        assert_eq!(
            Kit::of(mask(&[Tripmine, Mp5])),
            Kit::Gun(Mp5),
            "mines next to a primary gun are no mine level"
        );
        assert_eq!(Kit::of(mask(&[])), Kit::Other, "stripped for a level change");
    }

    #[test]
    fn only_the_level_weapons_hurt() {
        use WeaponId::*;
        assert!(Kit::Gun(Rpg).hurts_with(Rpg));
        assert!(!Kit::Gun(Rpg).hurts_with(Crowbar));
        assert!(Kit::Mines.hurts_with(Tripmine));
        assert!(!Kit::Mines.hurts_with(Glock));
        assert_eq!(
            Kit::Mines.weapons() & Glock.bit(),
            Glock.bit(),
            "the glock is still used, on mines"
        );
        assert!(Kit::Other.hurts_with(Crowbar));
    }

    #[test]
    fn the_leader_is_the_scoreboards_first_line() {
        let b = Board::new([(1, 203, 4), (2, 350, 9), (3, 350, 2), (4, -1, 0)], 100, true);
        assert_eq!(b.leader, Some(3), "most frags, then fewest deaths");
        assert_eq!(b.level(2), Some(3));
        assert_eq!(
            b.level(4),
            Some(0),
            "a suicide on the first level leaves the frags below zero"
        );
        assert_eq!(b.level(5), None);
        assert_eq!(b.top(), 3);
        let tie = Board::new([(5, 100, 1), (2, 100, 1)], 100, true);
        assert_eq!(tie.leader, Some(2), "a full tie goes to the lower slot");
    }

    #[test]
    fn the_warmup_and_the_last_level() {
        use WeaponId::*;
        let b = Board::new([(1, 0, 0), (2, 0, 0)], 100, true);
        let warm = GunGame::new(&b, 1, mask(&[Crowbar]));
        assert!(warm.warmup && !warm.descore());
        assert!(
            !warm.on_last_level(2, Some(Crowbar)),
            "in the warmup everyone has the crowbar"
        );
        let b = Board::new([(1, 1101, 3), (2, 400, 5)], 100, true);
        let me = GunGame::new(&b, 2, mask(&[Rpg]));
        assert!(!me.warmup && me.descore() && !me.leads());
        assert_eq!(me.behind(), 7);
        assert!(me.on_last_level(1, Some(Crowbar)));
        assert!(!me.on_last_level(1, Some(Rpg)));
        let last = GunGame::new(&b, 1, mask(&[Crowbar]));
        assert!(
            !last.warmup && last.leads(),
            "the crowbar past the first level is the last level"
        );
    }
}
