//! Weapon protocols: the presses and looks a weapon needs beyond pulling the trigger at a target in sight.
//!
//! - [`gauss`]: charging the gauss and letting it go when the view is on the target, or dumping a charge that has
//!   been held too long.
//! - [`boost`]: a gauss boost, the charge let go looking back and down as the bot jumps.
//! - [`throw`]: hand grenades (pin, cook, aim, release), satchels (one, a pile, or from a jump) and snarks (one, or
//!   all of them at an enemy close by).
//! - [`mine`]: placing a tripmine on a wall.
//! - [`launcher`]: the MP5's grenade on a lobbed arc.
//! - [`scope`]: the crossbow's scope snapped on for a shot and off again.
//! - [`detonate`]: setting off the bot's satchels (lying, or flying by an enemy), or shooting a tripmine.
//!
//! A protocol reads the bot's own state ([`Hands`]) every frame and asks for a weapon, a look and a movement
//! ([`Request`]); the brain offers them at `Prio::Protocol`, above combat, so a charge or a pulled pin is never
//! interrupted by a shot. Phases move on what the game reports back (the weapon in hand, the prediction data, the
//! ammo count), never on what was asked.

pub mod boost;
pub mod detonate;
pub mod gauss;
pub mod launcher;
pub mod mine;
pub mod scope;
pub mod throw;

use lb_core::Vec3;
use lb_core::math::angle_diff;
use lb_core::time::SimTime;
use lb_game::dll::DllProfile;
use lb_game::mechanics::{Attack, Trigger};
use lb_game::self_state::{PredictedWeapon, Prediction};
use lb_game::weapons::WeaponId;
use lb_motor::{Fire, LookIntent, MoveIntent, WeaponIntent};

use crate::policy::Armed;

/// The bot's own state a protocol reads.
#[derive(Clone, Copy, Debug)]
pub struct Hands<'a> {
    pub now: SimTime,
    pub eye: Vec3,
    pub origin: Vec3,
    pub velocity: Vec3,
    /// View angles now.
    pub view: Vec3,
    pub on_ground: bool,
    pub on_ladder: bool,
    pub waterlevel: u8,
    /// Field of view the game set: 0 is the default, less a zoomed scope.
    pub fov: f32,
    /// Weapon `CurWeapon` confirmed.
    pub weapon: Option<WeaponId>,
    pub arsenal: &'a [Armed],
    pub prediction: Option<&'a Prediction>,
    pub dll: DllProfile,
    /// `sv_gravity`.
    pub gravity: f32,
}

impl Hands<'_> {
    pub fn armed(&self, w: WeaponId) -> Option<&Armed> {
        self.arsenal.iter().find(|a| a.id == w)
    }

    /// Ammo of `w` in reserve (throwables, clip-less weapons); 0 when not carried or not known. For the weapon in
    /// hand the prediction data tells it first.
    pub fn reserve(&self, w: WeaponId) -> i32 {
        match self.prediction {
            Some(p) if p.current == Some(w) && self.weapon == Some(w) => p.primary_ammo,
            _ => self.armed(w).and_then(|a| a.reserve).unwrap_or(0),
        }
    }

    pub fn predicted(&self, w: WeaponId) -> Option<PredictedWeapon> {
        self.prediction?.weapons.get(w as usize).copied().flatten()
    }

    /// `w` is in hand and past its deploy.
    pub fn ready(&self, w: WeaponId) -> bool {
        self.weapon == Some(w)
            && self
                .prediction
                .is_none_or(|p| p.current == Some(w) && p.next_attack <= 0.0)
    }
}

/// What a protocol asks of the arbiter this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Request {
    pub weapon: Option<WeaponIntent>,
    pub look: Option<LookIntent>,
    pub movement: Option<MoveIntent>,
    /// Jump (a fresh press; held, it stays one jump).
    pub jump: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    Running(Request),
    Done,
    /// Gave up; the reason is for the decision trace.
    Failed(&'static str),
}

pub(crate) fn fire_of(a: Attack) -> Fire {
    match a {
        Attack::Primary => Fire::Primary,
        Attack::Secondary => Fire::Secondary,
    }
}

/// The game takes `attack` of the weapon now (its clock for that button has run out), as far as its prediction data
/// shows; taken when there is none.
pub(crate) fn takes(p: Option<PredictedWeapon>, attack: Attack) -> bool {
    p.is_none_or(|p| match attack {
        Attack::Primary => p.next_primary <= 0.0,
        Attack::Secondary => p.next_secondary <= 0.0,
    })
}

/// Holds `w` in hand without firing.
pub(crate) fn hold(w: WeaponId) -> WeaponIntent {
    WeaponIntent::hold(w)
}

/// Works `attack` of `w` with `trigger`; a tap presses once per `interval`.
pub(crate) fn press(w: WeaponId, attack: Attack, trigger: Trigger, interval: f32) -> WeaponIntent {
    WeaponIntent {
        select: Some(w),
        fire: fire_of(attack),
        trigger,
        interval,
        reload: false,
    }
}

/// The view is within `degrees` of `want` on both axes.
pub fn settled(view: Vec3, want: Vec3, degrees: f32) -> bool {
    angle_diff(want.y, view.y).abs() <= degrees && (want.x - view.x).abs() <= degrees
}

/// Standing still.
pub fn stop() -> MoveIntent {
    MoveIntent {
        dir: lb_core::Vec2::ZERO,
        speed: 0.0,
    }
}
