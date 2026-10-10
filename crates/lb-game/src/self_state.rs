//! What a bot knows about itself: its own HUD messages and its own entity state.

use lb_core::Vec3;
use lb_core::time::SimTime;

use crate::Known;
use crate::messages::{GameMsg, WeaponInfo};
use crate::weapons::WeaponId;

pub const MAX_AMMO_TYPES: usize = 32;

/// Body state copied from our own entity every frame by the runtime.
#[derive(Clone, Debug, Default)]
pub struct Body {
    pub origin: Vec3,
    pub velocity: Vec3,
    pub v_angle: Vec3,
    pub punchangle: Vec3,
    pub view_ofs: Vec3,
    pub health: f32,
    pub armor: f32,
    pub weapons_mask: u32,
    pub maxspeed: f32,
    pub fov: f32,
    pub flags: u32,
    pub movetype: u8,
    pub waterlevel: u8,
    pub deadflag: u8,
    pub in_duck: bool,
    pub has_longjump: bool,
    pub frags: f32,
    /// Edict index of what the feet stand on (`groundentity`), 0 for the world.
    pub groundentity: u16,
    pub basevelocity: Vec3,
}

pub const FL_ONGROUND: u32 = 1 << 9;
/// Frozen in place: GunGame freezes everyone once a match is won.
pub const FL_FROZEN: u32 = 1 << 12;
pub const FL_DUCKING: u32 = 1 << 14;
/// `basevelocity` is what a push field set this frame, not momentum left from one.
pub const FL_BASEVELOCITY: u32 = 1 << 22;
pub const MOVETYPE_FLY: u8 = 5;
pub const DEAD_NO: u8 = 0;
pub const DEAD_RESPAWNABLE: u8 = 3;

#[derive(Clone, Debug, Default)]
pub struct DamageTaken {
    pub at: SimTime,
    pub health: i32,
    pub armor: i32,
    pub bits: i32,
    pub source: Vec3,
}

/// One weapon as the bot's own client would be told it for prediction.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PredictedWeapon {
    pub clip: i32,
    /// Seconds until the next primary and secondary attack; zero or less: ready.
    pub next_primary: f32,
    pub next_secondary: f32,
    pub reloading: bool,
    /// `m_chargeReady`: satchels out (0 none, 1 out, 2 just set off).
    pub charge_ready: i32,
    /// `m_fInAttack`: gauss charge stage (0 idle, 1 spinning up, 2 charging).
    pub in_attack: i32,
    /// `m_fireState`: the egon beam.
    pub fire_state: i32,
    /// `m_flStartThrow`: server time the grenade pin was pulled, 0 when it is not.
    pub start_throw: f32,
}

/// What the bot's own client would be sent for weapon prediction (`GetWeaponData`, `UpdateClientData`): the
/// honest source of weapon readiness a human's client has too.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Prediction {
    pub at: SimTime,
    pub current: Option<WeaponId>,
    /// Seconds until any attack is allowed (weapon switch and deploy).
    pub next_attack: f32,
    /// Ammo of the active weapon's first type the player carries.
    pub primary_ammo: i32,
    pub weapons: [Option<PredictedWeapon>; 32],
}

#[derive(Clone, Debug)]
pub struct SelfState {
    pub body: Body,
    pub current_weapon: Known<WeaponId>,
    /// Weapon requested by the weapon controller and not yet confirmed by `CurWeapon`.
    pub requested_weapon: Option<(WeaponId, SimTime)>,
    pub clip: [Known<i32>; 32],
    pub ammo: [Known<i32>; MAX_AMMO_TYPES],
    pub hud_health: Known<i32>,
    pub hud_armor: Known<i32>,
    pub fov: Known<i32>,
    pub last_damage: Option<DamageTaken>,
    pub spawned_at: Option<SimTime>,
    pub deaths: i32,
    pub prediction: Option<Prediction>,
}

impl Default for SelfState {
    fn default() -> Self {
        SelfState {
            body: Body::default(),
            current_weapon: Known::Unknown,
            requested_weapon: None,
            clip: [Known::Unknown; 32],
            ammo: [Known::Unknown; MAX_AMMO_TYPES],
            hud_health: Known::Unknown,
            hud_armor: Known::Unknown,
            fov: Known::Unknown,
            last_damage: None,
            spawned_at: None,
            deaths: 0,
            prediction: None,
        }
    }
}

impl SelfState {
    pub fn is_alive(&self) -> bool {
        self.body.deadflag == DEAD_NO && self.body.health > 0.0
    }

    pub fn on_ground(&self) -> bool {
        self.body.flags & FL_ONGROUND != 0
    }

    pub fn on_ladder(&self) -> bool {
        self.body.movetype == MOVETYPE_FLY
    }

    pub fn owns(&self, w: WeaponId) -> bool {
        self.body.weapons_mask & w.bit() != 0
    }

    /// Weapon `w` can fire now, as the prediction data says; `None` without that data.
    pub fn ready(&self, w: WeaponId) -> Option<bool> {
        let p = self.prediction.as_ref()?;
        let weapon = p.weapons.get(w as usize).copied().flatten()?;
        Some(p.next_attack <= 0.0 && weapon.next_primary <= 0.0 && !weapon.reloading)
    }

    /// What the prediction data says of weapon `w`.
    pub fn predicted(&self, w: WeaponId) -> Option<PredictedWeapon> {
        self.prediction.as_ref()?.weapons.get(w as usize).copied().flatten()
    }

    /// Resets per-life state on spawn (weapons, clips and ammo come fresh from the game).
    pub fn on_spawn(&mut self, now: SimTime) {
        self.current_weapon = Known::Unknown;
        self.requested_weapon = None;
        self.clip = [Known::Unknown; 32];
        self.ammo = [Known::Unknown; MAX_AMMO_TYPES];
        self.last_damage = None;
        self.spawned_at = Some(now);
        self.prediction = None;
    }

    /// Applies a message addressed to this bot.
    pub fn apply(&mut self, msg: &GameMsg, now: SimTime) {
        match *msg {
            GameMsg::CurWeapon { active, id, clip } => {
                if let Some(w) = WeaponId::from_id(id) {
                    if active {
                        self.current_weapon = Known::Value(w);
                        if matches!(self.requested_weapon, Some((r, _)) if r == w) {
                            self.requested_weapon = None;
                        }
                    }
                    self.clip[id as usize] = Known::Value(clip);
                }
            }
            GameMsg::AmmoX { index, total } => {
                if let Some(slot) = self.ammo.get_mut(index as usize) {
                    *slot = Known::Value(total);
                }
            }
            GameMsg::AmmoPickup { index, delta } => {
                if let Some(Known::Value(v)) = self.ammo.get_mut(index as usize) {
                    *v += delta;
                }
            }
            GameMsg::Health(h) => self.hud_health = Known::Value(h),
            GameMsg::Battery(a) => self.hud_armor = Known::Value(a),
            GameMsg::SetFov(f) => self.fov = Known::Value(f),
            GameMsg::Damage {
                armor,
                health,
                bits,
                source,
            } => {
                self.last_damage = Some(DamageTaken {
                    at: now,
                    health,
                    armor,
                    bits,
                    source,
                });
            }
            _ => {}
        }
    }
}

/// Weapon registry built from `WeaponList` (ammo indices and maxima per weapon).
#[derive(Clone, Debug, Default)]
pub struct WeaponRegistry {
    pub entries: Vec<WeaponInfo>,
}

impl WeaponRegistry {
    pub fn insert(&mut self, info: WeaponInfo) {
        match self.entries.iter_mut().find(|e| e.id == info.id) {
            Some(e) => *e = info,
            None => self.entries.push(info),
        }
    }

    pub fn get(&self, w: WeaponId) -> Option<&WeaponInfo> {
        self.entries.iter().find(|e| e.id == w as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ammo_pickup_is_a_delta_and_needs_a_known_total() {
        let mut s = SelfState::default();
        s.apply(&GameMsg::AmmoPickup { index: 1, delta: 17 }, SimTime(1.0));
        assert_eq!(s.ammo[1], Known::Unknown);
        s.apply(&GameMsg::AmmoX { index: 1, total: 68 }, SimTime(1.0));
        s.apply(&GameMsg::AmmoPickup { index: 1, delta: 17 }, SimTime(2.0));
        assert_eq!(s.ammo[1], Known::Value(85));
    }

    #[test]
    fn cur_weapon_confirms_request() {
        let mut s = SelfState {
            requested_weapon: Some((WeaponId::Glock, SimTime(0.0))),
            ..Default::default()
        };
        s.apply(
            &GameMsg::CurWeapon {
                active: true,
                id: 2,
                clip: 17,
            },
            SimTime(0.1),
        );
        assert_eq!(s.current_weapon, Known::Value(WeaponId::Glock));
        assert!(s.requested_weapon.is_none());
        assert_eq!(s.clip[2], Known::Value(17));
    }
}
