//! What differs between game DLLs in how weapons are worked, and which DLL the server runs.
//!
//! - **Satchel buttons.** The classic SDK (and mods built on it) throws a satchel with the secondary attack whether
//!   charges are out or not, and sets the charges off with the primary (with none out, the primary throws too).
//!   Valve's 2023 update and BugfixedHL-Rebased swapped that: the primary throws, the secondary sets them off (and in
//!   the 2023 update does nothing with none out).
//! - **Hand grenade speed.** `(90 − pitch′) × 4`, at most 500, in the classic SDK; `× 6.5`, at most 1000, since
//!   the 2023 update and in BugfixedHL-Rebased.
//!
//! BugfixedHL-Rebased is told by its own cvars. Anything else is taken to throw grenades by the 2023 update, as
//! current mods do (hlsdk-portable among them), and to work satchels the classic way, as yapb did; the config can
//! name the DLL (`game.dll`). `lb selftest` checks it all on a live server. The bots check the satchel buttons as
//! they use them: a press that does the other thing, or nothing, is followed by the other button, and what it showed
//! holds for every bot.

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

    pub fn satchel_buttons(self) -> SatchelButtons {
        match self {
            DllKind::Classic => SatchelButtons::SecondaryThrows,
            DllKind::Bugfixed | DllKind::Valve25 => SatchelButtons::PrimaryThrows,
        }
    }
}

/// Which satchel button throws; the other one sets the charges out off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SatchelButtons {
    /// The classic SDK's way, and the bots' own when the DLL is not known.
    SecondaryThrows,
    /// Valve's 2023 update and BugfixedHL-Rebased.
    PrimaryThrows,
}

/// How the server's DLL works the weapons that differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DllProfile {
    pub kind: DllKind,
    /// Detected rather than named in the config.
    pub detected: bool,
    pub satchel: SatchelButtons,
}

impl Default for DllProfile {
    fn default() -> Self {
        DllProfile::resolve("auto", false)
    }
}

impl DllProfile {
    /// `configured` is the config's `game.dll` (`auto` detects); `bugfixed_cvars`: BugfixedHL's cvars are registered.
    pub fn resolve(configured: &str, bugfixed_cvars: bool) -> DllProfile {
        match DllKind::parse(configured) {
            Some(kind) => DllProfile {
                kind,
                detected: false,
                satchel: kind.satchel_buttons(),
            },
            None if bugfixed_cvars => DllProfile {
                kind: DllKind::Bugfixed,
                detected: true,
                satchel: SatchelButtons::PrimaryThrows,
            },
            None => DllProfile {
                kind: DllKind::Valve25,
                detected: true,
                satchel: SatchelButtons::SecondaryThrows,
            },
        }
    }

    /// Throws a satchel, whether some are out or not.
    pub fn satchel_throw(self) -> Attack {
        match self.satchel {
            SatchelButtons::SecondaryThrows => Attack::Secondary,
            SatchelButtons::PrimaryThrows => Attack::Primary,
        }
    }

    /// Sets off the satchels that are out.
    pub fn satchel_detonate(self) -> Attack {
        self.satchel_throw().other()
    }

    /// Hand grenade throw speed per degree of the throw angle below straight up, and its cap.
    pub fn grenade_speed(self) -> (f32, f32) {
        match self.kind {
            DllKind::Bugfixed | DllKind::Valve25 => (6.5, 1000.0),
            DllKind::Classic => (4.0, 500.0),
        }
    }

    /// The server was seen to set satchels off with `detonate`.
    pub fn set_satchel_detonate(&mut self, detonate: Attack) {
        self.satchel = match detonate {
            Attack::Primary => SatchelButtons::SecondaryThrows,
            Attack::Secondary => SatchelButtons::PrimaryThrows,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_and_buttons() {
        let bhl = DllProfile::resolve("auto", true);
        assert_eq!(bhl.kind, DllKind::Bugfixed);
        assert_eq!(
            (bhl.satchel_throw(), bhl.satchel_detonate()),
            (Attack::Primary, Attack::Secondary)
        );
        let other = DllProfile::resolve("auto", false);
        assert_eq!((other.kind, other.detected), (DllKind::Valve25, true));
        assert_eq!(
            (other.satchel_throw(), other.satchel_detonate()),
            (Attack::Secondary, Attack::Primary),
            "an unknown DLL is taken to work satchels the classic way"
        );
        assert_eq!(other.grenade_speed(), (6.5, 1000.0));
        assert_eq!(DllProfile::default(), other);
        let hl25 = DllProfile::resolve("hl25", false);
        assert_eq!((hl25.detected, hl25.satchel_detonate()), (false, Attack::Secondary));
        let classic = DllProfile::resolve("classic", false);
        assert_eq!((classic.kind, classic.detected), (DllKind::Classic, false));
        assert_eq!(classic.satchel_detonate(), Attack::Primary);
        assert_eq!(classic.grenade_speed(), (4.0, 500.0));
        let mut seen = classic;
        seen.set_satchel_detonate(Attack::Secondary);
        assert_eq!(
            (seen.satchel_throw(), seen.satchel_detonate()),
            (Attack::Primary, Attack::Secondary)
        );
        assert_eq!(
            seen.grenade_speed(),
            (4.0, 500.0),
            "the grenade keeps the classic speed"
        );
        seen.set_satchel_detonate(Attack::Secondary);
        assert_eq!(
            seen.satchel_detonate(),
            Attack::Secondary,
            "setting the same button twice keeps it"
        );
    }
}
