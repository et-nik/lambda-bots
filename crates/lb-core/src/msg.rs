//! Engine user messages as captured by the adapter. Interpretation (HLDM semantics) lives in
//! `lb-game`; this is only the transport shape.

use glam::Vec3;
use smallvec::SmallVec;

#[derive(Clone, Debug, PartialEq)]
pub enum MsgArg {
    Byte(i32),
    Char(i32),
    Short(i32),
    Long(i32),
    Angle(f32),
    Coord(f32),
    String(Vec<u8>),
    Entity(i32),
}

impl MsgArg {
    pub fn as_int(&self) -> Option<i32> {
        match *self {
            MsgArg::Byte(v) | MsgArg::Char(v) | MsgArg::Short(v) | MsgArg::Long(v) | MsgArg::Entity(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f32> {
        match *self {
            MsgArg::Angle(v) | MsgArg::Coord(v) => Some(v),
            _ => self.as_int().map(|v| v as f32),
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            MsgArg::String(v) => Some(v),
            _ => None,
        }
    }
}

/// Message destinations (engine `MSG_*`).
pub const MSG_BROADCAST: u8 = 0;
pub const MSG_ONE: u8 = 1;
pub const MSG_ALL: u8 = 2;
pub const MSG_INIT: u8 = 3;
pub const MSG_PVS: u8 = 4;
pub const MSG_PAS: u8 = 5;
pub const MSG_PVS_R: u8 = 6;
pub const MSG_PAS_R: u8 = 7;
pub const MSG_ONE_UNRELIABLE: u8 = 8;
pub const MSG_SPEC: u8 = 9;

#[derive(Clone, Debug)]
pub struct UserMsg {
    pub msg_id: i32,
    pub dest: u8,
    /// Slot of the addressed client for `MSG_ONE*`, 0 otherwise.
    pub target_slot: u8,
    pub origin: Option<Vec3>,
    pub truncated: bool,
    pub from_msg_manager: bool,
    pub args: SmallVec<[MsgArg; 8]>,
}

impl UserMsg {
    pub fn int(&self, i: usize) -> Option<i32> {
        self.args.get(i).and_then(MsgArg::as_int)
    }

    pub fn float(&self, i: usize) -> Option<f32> {
        self.args.get(i).and_then(MsgArg::as_float)
    }

    pub fn string(&self, i: usize) -> Option<&[u8]> {
        self.args.get(i).and_then(MsgArg::as_bytes)
    }
}
