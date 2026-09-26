//! Decoders for HLDM user messages. Tags are validated; a mismatch is counted and the message
//! ignored, which tolerates BHL/AG variants.

use lb_core::Vec3;
use lb_core::msg::UserMsg;

#[derive(Clone, Debug, PartialEq)]
pub enum GameMsg {
    WeaponList(WeaponInfo),
    CurWeapon {
        active: bool,
        id: i32,
        clip: i32,
    },
    AmmoX {
        index: u8,
        total: i32,
    },
    AmmoPickup {
        index: u8,
        delta: i32,
    },
    WeapPickup {
        id: i32,
    },
    ItemPickup {
        classname: String,
    },
    Health(i32),
    Battery(i32),
    Damage {
        armor: i32,
        health: i32,
        bits: i32,
        source: Vec3,
    },
    DeathMsg {
        killer: u8,
        victim: u8,
        weapon: String,
    },
    ScoreInfo {
        slot: u8,
        frags: i32,
        deaths: i32,
        team: i32,
    },
    TeamInfo {
        slot: u8,
        team: String,
    },
    GameMode {
        teamplay: bool,
    },
    ResetHud,
    InitHud,
    SetFov(i32),
    ScreenFade {
        duration: i32,
        hold: i32,
        flags: i32,
        color: [u8; 4],
    },
    TextMsg {
        dest: i32,
        text: String,
    },
    SayText {
        sender: u8,
        text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeaponInfo {
    pub name: String,
    pub ammo1: i32,
    pub max_ammo1: i32,
    pub ammo2: i32,
    pub max_ammo2: i32,
    pub slot: i32,
    pub position: i32,
    pub id: i32,
    pub flags: i32,
}

fn text(m: &UserMsg, i: usize) -> Option<String> {
    m.string(i).map(|b| String::from_utf8_lossy(b).into_owned())
}

/// Decodes a message given its registered name.
pub fn decode(name: &[u8], m: &UserMsg) -> Option<GameMsg> {
    Some(match name {
        b"WeaponList" => GameMsg::WeaponList(WeaponInfo {
            name: text(m, 0)?,
            ammo1: m.int(1)?,
            max_ammo1: m.int(2)?,
            ammo2: m.int(3)?,
            max_ammo2: m.int(4)?,
            slot: m.int(5)?,
            position: m.int(6)?,
            id: m.int(7)?,
            flags: m.int(8)?,
        }),
        b"CurWeapon" => GameMsg::CurWeapon {
            active: m.int(0)? != 0,
            id: m.int(1)?,
            clip: signed_byte(m.int(2)?),
        },
        b"AmmoX" => GameMsg::AmmoX {
            index: m.int(0)? as u8,
            total: m.int(1)?,
        },
        b"AmmoPickup" => GameMsg::AmmoPickup {
            index: m.int(0)? as u8,
            delta: m.int(1)?,
        },
        b"WeapPickup" => GameMsg::WeapPickup { id: m.int(0)? },
        b"ItemPickup" => GameMsg::ItemPickup { classname: text(m, 0)? },
        b"Health" => GameMsg::Health(m.int(0)?),
        b"Battery" => GameMsg::Battery(m.int(0)?),
        b"Damage" => GameMsg::Damage {
            armor: m.int(0)?,
            health: m.int(1)?,
            bits: m.int(2)?,
            source: Vec3::new(m.float(3)?, m.float(4)?, m.float(5)?),
        },
        b"DeathMsg" => GameMsg::DeathMsg {
            killer: m.int(0)? as u8,
            victim: m.int(1)? as u8,
            weapon: text(m, 2).unwrap_or_default(),
        },
        b"ScoreInfo" => GameMsg::ScoreInfo {
            slot: m.int(0)? as u8,
            frags: m.int(1)? as i16 as i32,
            deaths: m.int(2)? as i16 as i32,
            team: m.int(4).unwrap_or(0),
        },
        b"TeamInfo" => GameMsg::TeamInfo {
            slot: m.int(0)? as u8,
            team: text(m, 1)?,
        },
        b"GameMode" => GameMsg::GameMode {
            teamplay: m.int(0)? != 0,
        },
        b"ResetHUD" => GameMsg::ResetHud,
        b"InitHUD" => GameMsg::InitHud,
        b"SetFOV" => GameMsg::SetFov(m.int(0)?),
        b"ScreenFade" => GameMsg::ScreenFade {
            duration: m.int(0)?,
            hold: m.int(1)?,
            flags: m.int(2)?,
            color: [m.int(3)? as u8, m.int(4)? as u8, m.int(5)? as u8, m.int(6)? as u8],
        },
        b"TextMsg" => GameMsg::TextMsg {
            dest: m.int(0)?,
            text: text(m, 1)?,
        },
        b"SayText" => GameMsg::SayText {
            sender: m.int(0)? as u8,
            text: text(m, 1)?,
        },
        _ => return None,
    })
}

/// Messages the core wants the adapter to capture (by name; ids are resolved per map).
pub const WANTED: &[&str] = &[
    "WeaponList",
    "CurWeapon",
    "AmmoX",
    "AmmoPickup",
    "WeapPickup",
    "ItemPickup",
    "Health",
    "Battery",
    "Damage",
    "DeathMsg",
    "ScoreInfo",
    "TeamInfo",
    "GameMode",
    "ResetHUD",
    "InitHUD",
    "SetFOV",
    "ScreenFade",
    "TextMsg",
    "SayText",
];

fn signed_byte(v: i32) -> i32 {
    if v > 127 { v - 256 } else { v }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_core::msg::{MSG_ONE, MsgArg};
    use smallvec::smallvec;

    fn msg(args: Vec<MsgArg>) -> UserMsg {
        UserMsg {
            msg_id: 1,
            dest: MSG_ONE,
            target_slot: 1,
            origin: None,
            truncated: false,
            from_msg_manager: false,
            args: args.into(),
        }
    }

    #[test]
    fn decodes_cur_weapon_with_no_clip() {
        let m = msg(vec![MsgArg::Byte(1), MsgArg::Byte(1), MsgArg::Byte(255)]);
        assert_eq!(
            decode(b"CurWeapon", &m),
            Some(GameMsg::CurWeapon {
                active: true,
                id: 1,
                clip: -1
            })
        );
    }

    #[test]
    fn decodes_damage_and_rejects_bad_tags() {
        let m = msg(vec![
            MsgArg::Byte(4),
            MsgArg::Byte(12),
            MsgArg::Long(2),
            MsgArg::Coord(10.0),
            MsgArg::Coord(20.0),
            MsgArg::Coord(30.0),
        ]);
        assert!(matches!(
            decode(b"Damage", &m),
            Some(GameMsg::Damage { health: 12, .. })
        ));
        let bad = UserMsg {
            args: smallvec![MsgArg::String(b"x".to_vec())],
            ..msg(vec![])
        };
        assert_eq!(decode(b"Health", &bad), None);
    }

    #[test]
    fn score_info_negative_frags() {
        let m = msg(vec![
            MsgArg::Byte(3),
            MsgArg::Short(-2i16 as u16 as i32),
            MsgArg::Short(5),
            MsgArg::Short(0),
            MsgArg::Short(0),
        ]);
        assert_eq!(
            decode(b"ScoreInfo", &m),
            Some(GameMsg::ScoreInfo {
                slot: 3,
                frags: -2,
                deaths: 5,
                team: 0
            })
        );
    }
}
