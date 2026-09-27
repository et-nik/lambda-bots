//! What differs between game DLLs in how weapons are worked, and which DLL the server runs.
//!
//! - **Satchel buttons.** With no charge out, the primary attack throws one everywhere. With charges out, the
//!   classic SDK (and mods built on it, such as hlsdk-portable) sets them off with the primary attack and throws
//!   another with the secondary; Valve's 2023 update and BugfixedHL-Rebased swapped that: the secondary sets them
//!   off, the primary throws.
//! - **Hand grenade speed.** `(90 − pitch′) × 4`, at most 500, in the classic SDK; `× 6.5`, at most 1000, since
//!   the 2023 update and in BugfixedHL-Rebased.
//!
//! BugfixedHL-Rebased is told by its own cvars. The 2023 update has no such mark, so anything else is taken for the
//! classic SDK unless the config names the DLL (`game.dll`); `lb selftest` checks the guess on a live server.

use crate::mechanics::Attack;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DllKind {
    /// BugfixedHL-Rebased.
    Bugfixed,
    /// Valve's 2023 update of Half-Life (HL25).
    Valve25,
    /// The classic SDK and the mods built on it.
    Classic,
}

impl DllKind {
    pub const ALL: [DllKind; 3] = [DllKind::Bugfixed, DllKind::Valve25, DllKind::Classic];

    pub fn as_str(self) -> &'static str {
        match self {
            DllKind::Bugfixed => "bugfixed",
            DllKind::Valve25 => "hl25",
            DllKind::Classic => "classic",
        }
    }

    pub fn parse(s: &str) -> Option<DllKind> {
        DllKind::ALL
            .into_iter()
            .find(|k| k.as_str().eq_ignore_ascii_case(s.trim()))
    }
}

/// How the server's DLL works the weapons that differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DllProfile {
    pub kind: DllKind,
    /// Detected rather than named in the config.
    pub detected: bool,
    /// The satchel buttons were seen to work the other way round than `kind` has them.
    pub satchel_swapped: bool,
}

impl Default for DllProfile {
    fn default() -> Self {
        DllProfile {
            kind: DllKind::Bugfixed,
            detected: true,
            satchel_swapped: false,
        }
    }
}

impl DllProfile {
    /// `configured` is the config's `game.dll` (`auto` detects); `bugfixed_cvars`: BugfixedHL's cvars are registered.
    pub fn resolve(configured: &str, bugfixed_cvars: bool) -> DllProfile {
        match DllKind::parse(configured) {
            Some(kind) => DllProfile {
                kind,
                detected: false,
                satchel_swapped: false,
            },
            None => DllProfile {
                kind: if bugfixed_cvars {
                    DllKind::Bugfixed
                } else {
                    DllKind::Classic
                },
                detected: true,
                satchel_swapped: false,
            },
        }
    }

    /// Throws a satchel while none of the bot's is out.
    pub fn satchel_throw(self) -> Attack {
        Attack::Primary
    }

    /// Throws another satchel while some are out.
    pub fn satchel_throw_more(self) -> Attack {
        other(self.satchel_detonate())
    }

    /// Sets off the satchels that are out.
    pub fn satchel_detonate(self) -> Attack {
        let secondary = matches!(self.kind, DllKind::Bugfixed | DllKind::Valve25) != self.satchel_swapped;
        if secondary { Attack::Secondary } else { Attack::Primary }
    }

    /// Hand grenade throw speed per degree of the throw angle below straight up, and its cap.
    pub fn grenade_speed(self) -> (f32, f32) {
        match self.kind {
            DllKind::Bugfixed | DllKind::Valve25 => (6.5, 1000.0),
            DllKind::Classic => (4.0, 500.0),
        }
    }

    /// The satchel buttons turned out the other way round (a detonation press threw a satchel).
    pub fn swap_satchel_buttons(&mut self) {
        self.satchel_swapped = !self.satchel_swapped;
    }
}

fn other(a: Attack) -> Attack {
    match a {
        Attack::Primary => Attack::Secondary,
        Attack::Secondary => Attack::Primary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_and_buttons() {
        let bhl = DllProfile::resolve("auto", true);
        assert_eq!(bhl.kind, DllKind::Bugfixed);
        assert_eq!(bhl.satchel_detonate(), Attack::Secondary);
        assert_eq!(bhl.satchel_throw_more(), Attack::Primary);
        let other = DllProfile::resolve("auto", false);
        assert_eq!(other.kind, DllKind::Classic);
        assert_eq!(other.satchel_detonate(), Attack::Primary);
        assert_eq!(other.grenade_speed(), (4.0, 500.0));
        let named = DllProfile::resolve("HL25", false);
        assert_eq!((named.kind, named.detected), (DllKind::Valve25, false));
        let mut p = other;
        p.swap_satchel_buttons();
        assert_eq!(p.satchel_detonate(), Attack::Secondary);
        assert_eq!(p.satchel_throw_more(), Attack::Primary);
        assert_eq!(p.grenade_speed(), (4.0, 500.0), "the grenade keeps the classic speed");
    }
}
