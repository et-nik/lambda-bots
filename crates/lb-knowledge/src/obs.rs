//! Observations: what perception hands to beliefs. Everything here is something a human player could see, hear or
//! read on the HUD; none of it comes from raw server state.

use lb_core::Vec3;
use lb_core::time::SimTime;
use lb_game::sounds::SoundKind;
use lb_game::weapons::WeaponId;

/// The sense behind a belief. `Oracle` marks knowledge taken from server state and must never reach beliefs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sensor {
    Vision,
    Hearing,
    Damage,
    Public,
    Oracle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Provenance {
    pub sensor: Sensor,
    pub t: SimTime,
}

impl Provenance {
    pub fn new(sensor: Sensor, t: SimTime) -> Provenance {
        debug_assert_ne!(sensor, Sensor::Oracle, "beliefs must not come from server state");
        Provenance { sensor, t }
    }
}

/// A player as the scoreboard names it: slot and user id (the user id changes when the slot is reused).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlayerKey {
    pub slot: u8,
    pub userid: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    Enemy,
    Friend,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stance {
    Standing,
    Crouched,
}

/// Body points checked for line of sight (bits of [`Sighting::parts`]).
pub mod parts {
    pub const CHEST: u8 = 1;
    pub const HEAD: u8 = 2;
    pub const PELVIS: u8 = 4;
    pub const LEFT: u8 = 8;
    pub const RIGHT: u8 = 16;
    pub const KNEES: u8 = 32;
}

/// How a player is drawn; a glow or transparency can mark spawn protection.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RenderCue {
    pub mode: u8,
    pub fx: u8,
    pub amount: f32,
}

/// A recognized player in view on this vision tick.
#[derive(Clone, Debug, PartialEq)]
pub struct Sighting {
    pub who: PlayerKey,
    pub relation: Relation,
    pub t: SimTime,
    /// Observed origin, with the observation error applied.
    pub pos: Vec3,
    /// 1σ of `pos` in units.
    pub sigma: f32,
    pub distance: f32,
    /// Weighted share of the body in sight, 0..1.
    pub visibility: f32,
    pub parts: u8,
    pub stance: Stance,
    pub on_ground: bool,
    pub on_ladder: bool,
    pub in_water: bool,
    /// Facing yaw with observation error, degrees.
    pub facing: f32,
    pub weapon: Option<WeaponId>,
    /// A muzzle flash or a weapon event of this player was seen.
    pub firing: bool,
    pub render: RenderCue,
    /// Recognized on this tick rather than seen again.
    pub first: bool,
    /// When the first evidence of this contact was seen (before recognition).
    pub noticed_at: SimTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeBin {
    Near,
    Mid,
    Far,
}

impl RangeBin {
    pub fn of(distance: f32) -> RangeBin {
        if distance < 500.0 {
            RangeBin::Near
        } else if distance < 1500.0 {
            RangeBin::Mid
        } else {
            RangeBin::Far
        }
    }

    /// A typical distance for the bin, for turning a cue into a point to look at.
    pub fn distance(self) -> f32 {
        match self {
            RangeBin::Near => 300.0,
            RangeBin::Mid => 900.0,
            RangeBin::Far => 2000.0,
        }
    }
}

/// Something noticed but not yet recognized: a direction and a rough range, no identity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnonymousCue {
    pub t: SimTime,
    /// Unit direction from the eye.
    pub dir: Vec3,
    pub range: RangeBin,
    /// A point to look at: the eye moved along `dir` by the bin's typical distance.
    pub pos: Vec3,
}

/// A heard sound: where it seemed to come from, never who made it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundStimulus {
    pub t: SimTime,
    pub kind: SoundKind,
    pub weapon: Option<WeaponId>,
    /// Estimated source position (bearing, elevation and range with localization error).
    pub pos: Vec3,
    /// Estimated world yaw of the source, degrees.
    pub bearing: f32,
    /// 1σ of the bearing, degrees.
    pub bearing_sigma: f32,
    pub range: f32,
    /// Gain at the ear, 0..1.
    pub gain: f32,
}

/// Damage taken, as far as the HUD damage compass tells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DamageStimulus {
    pub t: SimTime,
    /// World yaw the damage seems to come from; `None` when the compass lights every side or none.
    pub bearing: Option<f32>,
    /// Health plus armor lost.
    pub amount: i32,
    pub bits: i32,
}

/// Information every player gets.
#[derive(Clone, Debug, PartialEq)]
pub enum PublicEvent {
    /// The kill feed; `killer` is `None` for the world and for suicides.
    Death {
        t: SimTime,
        killer: Option<u8>,
        victim: u8,
        weapon: String,
    },
}
