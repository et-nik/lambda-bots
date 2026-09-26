//! Raw sensor data captured from the engine; visible only to perception and runtime.
//!
//! These types describe the world as the server knows it (positions of every player, every
//! entity). Decision code never sees them directly: perception turns them into observations that
//! respect what a human player could perceive.

#![forbid(unsafe_code)]

use lb_core::Vec3;
use lb_core::handles::{EntityRef, MapEpoch};
use lb_core::msg::UserMsg;
use lb_core::time::SimTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientState {
    Free,
    Connecting,
    Connected,
    Spawned,
}

#[derive(Clone, Debug)]
pub struct RawClient {
    pub slot: u8,
    pub state: ClientState,
    pub is_fake: bool,
    pub is_ours: bool,
    pub userid: i32,
    pub origin: Vec3,
    pub velocity: Vec3,
    pub angles: Vec3,
    pub view_ofs: Vec3,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub flags: u32,
    pub effects: u32,
    pub movetype: u8,
    pub solid: u8,
    pub deadflag: u8,
    pub waterlevel: u8,
    pub rendermode: u8,
    pub renderfx: u8,
    pub renderamt: f32,
    pub rendercolor: Vec3,
    pub step_left: u8,
    pub frame: f32,
    pub frags: f32,
    pub sequence: i32,
    pub gaitsequence: i32,
    pub weaponmodel: u16,
    pub model: u16,
    pub ping_ms: u16,
}

/// State of one of our own bots (the bot legitimately knows all of it).
#[derive(Clone, Debug, Default)]
pub struct RawSelf {
    pub slot: u8,
    pub bot_gen: u32,
    pub deadflag: u8,
    pub movetype: u8,
    pub waterlevel: u8,
    pub watertype: i32,
    pub health: f32,
    pub armor: f32,
    pub weapons_mask: u32,
    pub maxspeed: f32,
    pub fov: f32,
    pub duck_time: f32,
    pub fall_velocity: f32,
    pub flags: u32,
    pub origin: Vec3,
    pub velocity: Vec3,
    pub v_angle: Vec3,
    pub angles: Vec3,
    pub punchangle: Vec3,
    pub view_ofs: Vec3,
    pub basevelocity: Vec3,
    pub in_duck: bool,
    pub has_longjump: bool,
    pub fixangle: u8,
    pub groundentity: u16,
    pub buttons_applied: u16,
    pub frags: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientEventKind {
    Connect,
    ConnectRejected,
    PutInServer,
    Disconnect,
    Info,
}

#[derive(Clone, Debug)]
pub struct ClientEvent {
    pub kind: ClientEventKind,
    pub slot: u8,
    pub userid: i32,
    pub is_ours: bool,
    pub is_fake: bool,
    pub bot_gen: u32,
    pub name: Vec<u8>,
    pub model: Vec<u8>,
    pub address: Vec<u8>,
    pub auth_id: Vec<u8>,
    pub topcolor: u8,
    pub bottomcolor: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundSource {
    Emit,
    Ambient,
    Rehlds,
}

#[derive(Clone, Debug)]
pub struct RawSound {
    pub source: SoundSource,
    pub entity: EntityRef,
    pub channel: u8,
    pub sample: Vec<u8>,
    pub origin: Vec3,
    pub volume: f32,
    pub attenuation: f32,
    pub flags: i32,
    pub pitch: i32,
}

#[derive(Clone, Debug)]
pub struct RawPlayback {
    pub flags: i32,
    pub event_index: u16,
    pub invoker: EntityRef,
    pub origin: Vec3,
    pub angles: Vec3,
    pub invoker_origin: Vec3,
    pub fparam1: f32,
    pub fparam2: f32,
    pub iparam1: i32,
    pub iparam2: i32,
    pub bparam1: i32,
    pub bparam2: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityEventKind {
    Spawn,
    Free,
}

#[derive(Clone, Debug)]
pub struct EntityEvent {
    pub kind: EntityEventKind,
    pub entity: EntityRef,
    pub track_kind: u8,
    pub classname: u16,
    pub origin: Vec3,
}

#[derive(Clone, Debug)]
pub struct CommandEvent {
    pub slot: u8,
    pub userid: i32,
    pub argv: Vec<Vec<u8>>,
    pub line: Vec<u8>,
}

#[derive(Clone, Debug)]
pub enum RawEvent {
    Client(ClientEvent),
    UserMsg(UserMsg),
    Sound(RawSound),
    Playback(RawPlayback),
    Entity(EntityEvent),
    ClientCommand(CommandEvent),
    ServerCommand(CommandEvent),
    Fixangle {
        slot: u8,
        bot_gen: u32,
        mode: u8,
        angles: Vec3,
    },
    RegisterMsg {
        id: i32,
        name: Vec<u8>,
    },
    PrecacheEvent {
        index: i32,
        name: Vec<u8>,
    },
    Log {
        level: i32,
        text: Vec<u8>,
    },
    Overflow {
        dropped_records: u32,
        dropped_bytes: u32,
    },
}

#[derive(Clone, Debug)]
pub struct StampedEvent {
    pub seq: u32,
    pub ctx: u32,
    pub sim_time: SimTime,
    pub event: RawEvent,
}

#[derive(Clone, Copy, Debug)]
pub struct FrameHeader {
    pub epoch: MapEpoch,
    pub frame_no: u64,
    pub sim_time: SimTime,
    pub frame_time: f64,
    pub mono_ns: u64,
    pub engine_time: f32,
    pub pre: bool,
    pub paused: bool,
    pub first: bool,
    pub max_clients: u32,
    pub num_edicts: u32,
}

#[derive(Clone, Debug)]
pub struct RawFrame {
    pub header: FrameHeader,
    pub clients: Vec<RawClient>,
    pub selves: Vec<RawSelf>,
    pub events: Vec<StampedEvent>,
    pub dropped_events: u32,
}
