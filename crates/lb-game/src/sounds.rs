//! What a sound means to a listener: classes of game sounds (by sample name) and of client weapon events (by event
//! file), with the volume and attenuation the HL client plays events at (`ev_hldm.cpp`).

use crate::weapons::WeaponId;

/// `ATTN_NORM`: audible up to 1250 units at full volume.
pub const ATTN_NORM: f32 = 0.8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SoundKind {
    /// Footsteps, ladder steps, wading and swimming.
    Step,
    Jump,
    Pain,
    /// A weapon firing (or a gauss charging).
    Shot,
    /// Reloads, deploys, crowbar hits and other weapon noises.
    WeaponNoise,
    Pickup,
    /// An item or weapon materializing (`items/suitchargeok1.wav`).
    ItemRespawn,
    Charger,
    Explosion,
    Mechanism,
    Other,
}

impl SoundKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SoundKind::Step => "step",
            SoundKind::Jump => "jump",
            SoundKind::Pain => "pain",
            SoundKind::Shot => "shot",
            SoundKind::WeaponNoise => "weapon",
            SoundKind::Pickup => "pickup",
            SoundKind::ItemRespawn => "respawn",
            SoundKind::Charger => "charger",
            SoundKind::Explosion => "explosion",
            SoundKind::Mechanism => "mechanism",
            SoundKind::Other => "other",
        }
    }
}

/// A classified sound: what it is and which weapon it hints at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundClass {
    pub kind: SoundKind,
    pub weapon: Option<WeaponId>,
}

impl SoundClass {
    const fn of(kind: SoundKind) -> SoundClass {
        SoundClass { kind, weapon: None }
    }
}

/// Lowercase sample path without the sentence (`!`) and streaming (`*`) markers.
fn normalize(sample: &[u8]) -> String {
    let s = String::from_utf8_lossy(sample).to_ascii_lowercase();
    s.trim_start_matches(['!', '*', '#']).replace('\\', "/")
}

/// Weapons whose shots the game plays as server sounds (with client weapons off, or always for some).
const FIRE_SAMPLES: &[(&str, WeaponId)] = &[
    ("weapons/pl_gun", WeaponId::Glock),
    ("weapons/hks", WeaponId::Mp5),
    ("weapons/glauncher", WeaponId::Mp5),
    ("weapons/sbarrel", WeaponId::Shotgun),
    ("weapons/dbarrel", WeaponId::Shotgun),
    ("weapons/357_shot", WeaponId::Python),
    ("weapons/xbow_fire", WeaponId::Crossbow),
    ("weapons/rocketfire", WeaponId::Rpg),
    ("weapons/gauss2", WeaponId::Gauss),
    ("weapons/egon_", WeaponId::Egon),
    ("agrunt/ag_fire", WeaponId::Hornetgun),
];

/// Class of a sample played by the game (`EmitSound`, `SV_StartSound`).
pub fn classify_sample(sample: &[u8]) -> SoundClass {
    const STEPS: &[&str] = &[
        "player/pl_step",
        "player/pl_metal",
        "player/pl_dirt",
        "player/pl_duct",
        "player/pl_grate",
        "player/pl_tile",
        "player/pl_slosh",
        "player/pl_wade",
        "player/pl_swim",
        "player/pl_ladder",
        "player/pl_snow",
    ];
    let s = normalize(sample);
    if let Some((_, w)) = FIRE_SAMPLES.iter().find(|(p, _)| s.starts_with(p)) {
        return SoundClass {
            kind: SoundKind::Shot,
            weapon: Some(*w),
        };
    }
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| s.starts_with(p));
    let kind = if starts(STEPS) {
        SoundKind::Step
    } else if starts(&["player/pl_jump", "player/plyrjmp"]) {
        SoundKind::Jump
    } else if starts(&[
        "player/pl_pain",
        "player/pl_fallpain",
        "player/pl_die",
        "player/pl_death",
    ]) {
        SoundKind::Pain
    } else if s == "items/suitchargeok1.wav" {
        SoundKind::ItemRespawn
    } else if starts(&["items/medshot", "items/medcharge", "items/suitcharge"]) {
        SoundKind::Charger
    } else if starts(&["items/"]) {
        SoundKind::Pickup
    } else if starts(&["weapons/explode", "weapons/debris"]) {
        SoundKind::Explosion
    } else if starts(&["weapons/"]) {
        SoundKind::WeaponNoise
    } else if starts(&["doors/", "buttons/", "plats/"]) {
        SoundKind::Mechanism
    } else {
        SoundKind::Other
    };
    SoundClass::of(kind)
}

/// A client weapon event as heard by other players.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EventSound {
    pub class: SoundClass,
    pub volume: f32,
    pub attenuation: f32,
}

/// The sound other players hear for a precached event (`events/glock1.sc`); `None` for silent or unknown events.
pub fn classify_event(name: &[u8]) -> Option<EventSound> {
    let s = normalize(name);
    let stem = s.rsplit('/').next()?.strip_suffix(".sc")?;
    use SoundKind::*;
    use WeaponId as W;
    let (kind, weapon, volume) = match stem {
        "glock1" | "glock2" => (Shot, W::Glock, 0.96),
        "shotgun1" | "shotgun2" => (Shot, W::Shotgun, 0.97),
        "mp5" => (Shot, W::Mp5, 1.0),
        "mp52" => (Shot, W::Mp5, 1.0),
        "python" => (Shot, W::Python, 0.85),
        "gauss" => (Shot, W::Gauss, 0.8),
        "gaussspin" => (Shot, W::Gauss, 1.0),
        "egon_fire" => (Shot, W::Egon, 0.98),
        "egon_stop" => (WeaponNoise, W::Egon, 0.98),
        "crossbow1" | "crossbow2" => (Shot, W::Crossbow, 1.0),
        "rpg" => (Shot, W::Rpg, 0.9),
        "firehornet" => (Shot, W::Hornetgun, 1.0),
        "crowbar" => (WeaponNoise, W::Crowbar, 1.0),
        _ => return None,
    };
    Some(EventSound {
        class: SoundClass {
            kind,
            weapon: Some(weapon),
        },
        volume,
        attenuation: ATTN_NORM,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples() {
        assert_eq!(classify_sample(b"player/pl_step3.wav").kind, SoundKind::Step);
        assert_eq!(classify_sample(b"player/pl_ladder1.wav").kind, SoundKind::Step);
        assert_eq!(classify_sample(b"Player/PL_Jump2.wav").kind, SoundKind::Jump);
        assert_eq!(classify_sample(b"items/suitchargeok1.wav").kind, SoundKind::ItemRespawn);
        assert_eq!(classify_sample(b"items/gunpickup2.wav").kind, SoundKind::Pickup);
        assert_eq!(classify_sample(b"weapons/reload3.wav").kind, SoundKind::WeaponNoise);
        let shot = classify_sample(b"weapons/pl_gun3.wav");
        assert_eq!((shot.kind, shot.weapon), (SoundKind::Shot, Some(WeaponId::Glock)));
        assert_eq!(classify_sample(b"!HG_ALERT").kind, SoundKind::Other);
    }

    #[test]
    fn events() {
        let glock = classify_event(b"events/glock1.sc").unwrap();
        assert_eq!(glock.class.kind, SoundKind::Shot);
        assert_eq!(glock.class.weapon, Some(WeaponId::Glock));
        assert_eq!(glock.attenuation, ATTN_NORM);
        assert_eq!(
            classify_event(b"events/mp52.sc").unwrap().class.weapon,
            Some(WeaponId::Mp5)
        );
        assert_eq!(classify_event(b"events/train.sc"), None);
        assert_eq!(
            classify_event(b"events/tripfire.sc"),
            None,
            "the deploy sound comes from the mine entity"
        );
    }
}
