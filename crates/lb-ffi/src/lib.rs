//! C ABI types shared by the C++ adapter and the Rust core (source of truth for `lb_abi.h`).
//!
//! Layout rules: fixed-width integers only, no `bool`, explicit padding, every 8-byte field at an
//! 8-byte offset and every struct size a multiple of 8, so the layout is identical on i386 (where
//! 64-bit members are 4-byte aligned), x86 MSVC and arm64. The adapter reports `sizeof` of every
//! struct in [`LbInitInfo::sizes`]; the core refuses to start on any mismatch.

#![allow(non_camel_case_types)]

use core::ffi::c_void;

/// Bumped on any incompatible change of this file.
pub const LB_ABI_VERSION: u32 = 1;

// ---------------------------------------------------------------------------------------------
// Basic types
// ---------------------------------------------------------------------------------------------

/// Borrowed bytes, not NUL-terminated, valid only for the duration of the call that received them.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbStr {
    pub ptr: *const u8,
    pub len: u32,
}

impl LbStr {
    pub const EMPTY: LbStr = LbStr {
        ptr: core::ptr::null(),
        len: 0,
    };

    pub const fn from_static(s: &'static str) -> LbStr {
        LbStr {
            ptr: s.as_ptr(),
            len: s.len() as u32,
        }
    }

    pub fn from_bytes(b: &[u8]) -> LbStr {
        LbStr {
            ptr: b.as_ptr(),
            len: b.len() as u32,
        }
    }

    /// # Safety
    /// `ptr` must be valid for `len` bytes for the lifetime `'a`.
    pub unsafe fn as_bytes<'a>(self) -> &'a [u8] {
        if self.ptr.is_null() || self.len == 0 {
            &[]
        } else {
            // SAFETY: guaranteed by the caller.
            unsafe { core::slice::from_raw_parts(self.ptr, self.len as usize) }
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LbVec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Entity reference validated by the adapter through the edict serial number. `index == 0` is the world.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LbEntRef {
    pub index: u16,
    pub pad: u16,
    pub serial: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbArgs {
    pub argc: u32,
    pub pad: u32,
    pub argv: *const LbStr,
    pub line: LbStr,
}

// ---------------------------------------------------------------------------------------------
// Status codes and enumerations (integer constants, never Rust enums across the boundary)
// ---------------------------------------------------------------------------------------------

pub const LB_OK: i32 = 0;
pub const LB_ERR_INVALID: i32 = -1;
pub const LB_ERR_ABI: i32 = -2;
pub const LB_ERR_STALE: i32 = -3;
pub const LB_ERR_FULL: i32 = -4;
pub const LB_ERR_REJECTED: i32 = -5;
pub const LB_ERR_UNSUPPORTED: i32 = -6;
pub const LB_ERR_NOT_FOUND: i32 = -7;
pub const LB_ERR_BUSY: i32 = -8;

pub const LB_PLATFORM_LINUX: u8 = 1;
pub const LB_PLATFORM_WINDOWS: u8 = 2;
pub const LB_PLATFORM_MACOS: u8 = 3;

pub const LB_ENGINE_HLDS: u8 = 1;
pub const LB_ENGINE_REHLDS: u8 = 2;
pub const LB_ENGINE_XASH: u8 = 3;

pub const LB_SHUTDOWN_DETACH: u32 = 1;
pub const LB_SHUTDOWN_PROCESS_EXIT: u32 = 2;
pub const LB_SHUTDOWN_FATAL: u32 = 3;

/// Frame header flags.
pub const LB_FRAME_PRE: u32 = 1;
pub const LB_FRAME_POST: u32 = 2;
pub const LB_FRAME_PAUSED: u32 = 4;
pub const LB_FRAME_FIRST: u32 = 8;

/// Client slot states.
pub const LB_CLIENT_FREE: u8 = 0;
pub const LB_CLIENT_CONNECTING: u8 = 1;
pub const LB_CLIENT_CONNECTED: u8 = 2;
pub const LB_CLIENT_SPAWNED: u8 = 3;

/// Channel availability bits reported in [`LbCompatFacts::channels`].
pub const LB_CH_SV_STARTSOUND: u32 = 1;
pub const LB_CH_MSGMGR: u32 = 2;
pub const LB_CH_EMITPINGS: u32 = 4;
pub const LB_CH_DROPCLIENT: u32 = 8;
pub const LB_CH_HOSTTIME: u32 = 16;
pub const LB_CH_HOOK_TABLES: u32 = 32;
pub const LB_CH_AMBIENT_SOUND: u32 = 64;
pub const LB_CH_PLAYBACK_EVENT: u32 = 128;

// ---------------------------------------------------------------------------------------------
// Init / map lifecycle
// ---------------------------------------------------------------------------------------------

/// Indices into [`LbInitInfo::sizes`].
pub const LB_SZ_INIT_INFO: usize = 0;
pub const LB_SZ_MAP_INFO: usize = 1;
pub const LB_SZ_FRAME_HEADER: usize = 2;
pub const LB_SZ_FRAME_INPUT: usize = 3;
pub const LB_SZ_CLIENT_SNAPSHOT: usize = 4;
pub const LB_SZ_SELF_SNAPSHOT: usize = 5;
pub const LB_SZ_EVENT_HEADER: usize = 6;
pub const LB_SZ_BOT_COMMAND: usize = 7;
pub const LB_SZ_MOVE_FEEDBACK: usize = 8;
pub const LB_SZ_CLIENT_COMMAND: usize = 9;
pub const LB_SZ_TRACE_REQUEST: usize = 10;
pub const LB_SZ_TRACE_RESULT: usize = 11;
pub const LB_SZ_ENTITY_SNAPSHOT: usize = 12;
pub const LB_SZ_TRACK_RULE: usize = 13;
pub const LB_SZ_CREATE_BOT_REQUEST: usize = 14;
pub const LB_SZ_CREATE_BOT_RESULT: usize = 15;
pub const LB_SZ_DEBUG_PRIM: usize = 16;
pub const LB_SZ_CVAR_SPEC: usize = 17;
pub const LB_SZ_COMPAT_FACTS: usize = 18;
pub const LB_SZ_HOST_API: usize = 19;
pub const LB_SZ_MSG_ARG: usize = 20;
pub const LB_SZ_DISGUISE: usize = 21;
pub const LB_SZ_WEAPON_STATE: usize = 22;
pub const LB_SZ_COUNT: usize = 24;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbInitInfo {
    pub struct_size: u32,
    pub abi_version: u32,
    pub sizes: [u32; LB_SZ_COUNT],
    pub adapter_version: LbStr,
    pub plugin_path: LbStr,
    pub game_dir: LbStr,
    pub install_dir: LbStr,
    pub platform: u8,
    pub pointer_size: u8,
    pub late_load: u8,
    pub pad: [u8; 5],
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbInitResult {
    pub struct_size: u32,
    pub abi_version: u32,
    pub status: i32,
    pub pad: u32,
    /// Points to static memory owned by the core.
    pub core_version: LbStr,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbMapInfo {
    pub struct_size: u32,
    pub map_epoch: u32,
    pub map_name: LbStr,
    pub bsp_path: LbStr,
    pub max_clients: u32,
    pub max_edicts: u32,
    pub worldmap_crc: u32,
    pub has_crc: u8,
    pub late_load: u8,
    pub pad: [u8; 2],
}

// ---------------------------------------------------------------------------------------------
// Frame input
// ---------------------------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbFrameHeader {
    pub struct_size: u32,
    pub map_epoch: u32,
    pub frame_no: u64,
    /// Simulation time in seconds; double precision (the engine's `gpGlobals->time` is a float).
    pub sim_time: f64,
    pub frame_time: f64,
    pub mono_ns: u64,
    pub engine_time: f32,
    pub flags: u32,
    pub max_clients: u32,
    pub num_edicts: u32,
}

/// Raw per-slot client state. Sensor-only: no health, armor, weapons or buttons of other players.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbClientSnapshot {
    pub slot: u8,
    pub state: u8,
    pub is_fake: u8,
    pub is_ours: u8,
    pub userid: i32,
    pub origin: LbVec3,
    pub velocity: LbVec3,
    /// Model angles (what other players see).
    pub angles: LbVec3,
    pub view_ofs: LbVec3,
    pub mins: LbVec3,
    pub maxs: LbVec3,
    pub rendercolor: LbVec3,
    pub flags: u32,
    pub effects: u32,
    pub movetype: u8,
    pub solid: u8,
    pub deadflag: u8,
    pub waterlevel: u8,
    pub rendermode: u8,
    pub renderfx: u8,
    /// `entvars.iStepLeft`, toggled by the player movement code on every footstep.
    pub step_left: u8,
    pub pad0: u8,
    pub renderamt: f32,
    pub frame: f32,
    pub frags: f32,
    pub sequence: i32,
    pub gaitsequence: i32,
    pub weaponmodel_id: u16,
    pub model_id: u16,
    pub ping_ms: u16,
    pub pad1: u16,
}

/// State of one of our bots. Only our own bots are ever described by this struct.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbSelfSnapshot {
    pub slot: u8,
    pub deadflag: u8,
    pub movetype: u8,
    pub waterlevel: u8,
    pub bot_gen: u32,
    pub health: f32,
    pub armor: f32,
    pub weapons_mask: u32,
    pub maxspeed: f32,
    pub fov: f32,
    pub duck_time: f32,
    pub fall_velocity: f32,
    pub flags: u32,
    pub watertype: i32,
    pub origin: LbVec3,
    pub velocity: LbVec3,
    pub v_angle: LbVec3,
    pub angles: LbVec3,
    pub punchangle: LbVec3,
    pub view_ofs: LbVec3,
    pub basevelocity: LbVec3,
    pub in_duck: u8,
    pub has_longjump: u8,
    pub fixangle: u8,
    pub pad0: u8,
    pub groundentity: u16,
    pub buttons_applied: u16,
    pub frags: f32,
    pub pad1: u32,
}

/// A batch of event records captured since the previous frame: `LbEventHeader` + payload, each
/// record padded to a multiple of 8 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbEventBatch {
    pub data: *const u8,
    pub len: u32,
    pub count: u32,
    pub dropped: u32,
    pub first_seq: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbFrameInput {
    pub struct_size: u32,
    pub client_count: u32,
    pub header: LbFrameHeader,
    pub clients: *const LbClientSnapshot,
    pub selves: *const LbSelfSnapshot,
    pub self_count: u32,
    pub pad: u32,
    pub events: LbEventBatch,
}

// ---------------------------------------------------------------------------------------------
// Event arena records
// ---------------------------------------------------------------------------------------------

pub const LB_EV_CLIENT: u16 = 1;
pub const LB_EV_USER_MSG: u16 = 2;
pub const LB_EV_SOUND: u16 = 3;
pub const LB_EV_PLAYBACK: u16 = 4;
pub const LB_EV_ENTITY: u16 = 5;
pub const LB_EV_CLIENT_CMD: u16 = 6;
pub const LB_EV_SERVER_CMD: u16 = 7;
pub const LB_EV_FIXANGLE: u16 = 8;
pub const LB_EV_STRING: u16 = 9;
pub const LB_EV_REG_MSG: u16 = 10;
pub const LB_EV_PRECACHE_EVENT: u16 = 11;
pub const LB_EV_LOG: u16 = 12;
pub const LB_EV_OVERFLOW: u16 = 13;

/// Context in which a record was captured.
pub const LB_CTX_FRAME: u32 = 0;
pub const LB_CTX_BOTCMD: u32 = 1;
pub const LB_CTX_BOTCLCMD: u32 = 2;
pub const LB_CTX_GAMEDLL: u32 = 3;
pub const LB_CTX_CORE: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbEventHeader {
    pub kind: u16,
    /// Total record size including this header, a multiple of 8.
    pub size: u16,
    pub seq: u32,
    /// `LB_CTX_*` in the low byte, slot in the second byte for bot contexts.
    pub ctx: u32,
    pub frame_no_low: u32,
    pub sim_time: f64,
}

pub const LB_CLIENT_EV_CONNECT: u8 = 1;
pub const LB_CLIENT_EV_CONNECT_REJECTED: u8 = 2;
pub const LB_CLIENT_EV_PUT_IN_SERVER: u8 = 3;
pub const LB_CLIENT_EV_DISCONNECT: u8 = 4;
pub const LB_CLIENT_EV_INFO: u8 = 5;

/// Payload of `LB_EV_CLIENT`; followed by `name_len` bytes of name, `model_len` bytes of model,
/// `addr_len` bytes of address (all without terminator).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbEvClient {
    pub what: u8,
    pub slot: u8,
    pub is_ours: u8,
    pub is_fake: u8,
    pub userid: i32,
    pub bot_gen: u32,
    pub topcolor: u8,
    pub bottomcolor: u8,
    pub name_len: u8,
    pub model_len: u8,
    pub addr_len: u8,
    pub pad: [u8; 3],
    pub auth_id_len: u8,
    pub pad2: [u8; 3],
}

pub const LB_MSG_ARG_BYTE: u8 = 1;
pub const LB_MSG_ARG_CHAR: u8 = 2;
pub const LB_MSG_ARG_SHORT: u8 = 3;
pub const LB_MSG_ARG_LONG: u8 = 4;
pub const LB_MSG_ARG_ANGLE: u8 = 5;
pub const LB_MSG_ARG_COORD: u8 = 6;
pub const LB_MSG_ARG_STRING: u8 = 7;
pub const LB_MSG_ARG_ENTITY: u8 = 8;

pub const LB_MSG_FLAG_HAS_ORIGIN: u8 = 1;
pub const LB_MSG_FLAG_TRUNCATED: u8 = 2;
pub const LB_MSG_FLAG_FROM_MSGMGR: u8 = 4;

/// Payload of `LB_EV_USER_MSG`; followed by `argc` × [`LbMsgArg`] and then the string bytes
/// referenced by `LbMsgArg::str_off`/`str_len` (offsets relative to the end of the argument array).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbEvUserMsg {
    pub msg_id: i32,
    pub dest: u8,
    pub target_slot: u8,
    pub flags: u8,
    pub argc: u8,
    pub origin: LbVec3,
    pub strings_len: u16,
    pub pad: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbMsgArg {
    pub tag: u8,
    pub pad: [u8; 3],
    pub ival: i32,
    pub fval: f32,
    pub str_off: u16,
    pub str_len: u16,
}

pub const LB_SOUND_SRC_EMIT: u8 = 1;
pub const LB_SOUND_SRC_AMBIENT: u8 = 2;
pub const LB_SOUND_SRC_REHLDS: u8 = 3;

/// Payload of `LB_EV_SOUND`; followed by `sample_len` bytes of the sample name.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbEvSound {
    pub source: u8,
    pub channel: u8,
    pub sample_len: u16,
    pub entity: LbEntRef,
    pub origin: LbVec3,
    pub volume: f32,
    pub attenuation: f32,
    pub flags: i32,
    pub pitch: i32,
}

/// Payload of `LB_EV_PLAYBACK` (client-predicted events such as weapon fire).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbEvPlayback {
    pub flags: i32,
    pub event_index: u16,
    pub pad: u16,
    pub invoker: LbEntRef,
    pub origin: LbVec3,
    pub angles: LbVec3,
    pub invoker_origin: LbVec3,
    pub delay: f32,
    pub fparam1: f32,
    pub fparam2: f32,
    pub iparam1: i32,
    pub iparam2: i32,
    pub bparam1: i32,
    pub bparam2: i32,
}

pub const LB_ENTITY_EV_SPAWN: u8 = 1;
pub const LB_ENTITY_EV_FREE: u8 = 2;

/// Payload of `LB_EV_ENTITY`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbEvEntity {
    pub what: u8,
    pub kind: u8,
    pub classname_id: u16,
    pub ent: LbEntRef,
    pub origin: LbVec3,
    pub pad: [u32; 2],
}

/// Payload of `LB_EV_CLIENT_CMD` / `LB_EV_SERVER_CMD`; followed by `argc` × (u16 length + bytes),
/// then `line_len` bytes of the full argument line.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbEvCommand {
    pub slot: u8,
    pub argc: u8,
    pub line_len: u16,
    pub userid: i32,
}

/// Payload of `LB_EV_FIXANGLE`: the adapter acknowledged a forced view angle for one of our bots.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbEvFixangle {
    pub slot: u8,
    pub mode: u8,
    pub pad: u16,
    pub bot_gen: u32,
    pub angles: LbVec3,
    pub pad2: u32,
}

/// Payload of `LB_EV_STRING` / `LB_EV_REG_MSG` / `LB_EV_PRECACHE_EVENT` / `LB_EV_LOG`; followed by
/// `len` bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbEvNamed {
    /// String id, user message id, event index or log level depending on the record kind.
    pub id: i32,
    pub len: u16,
    pub extra: u16,
}

/// Payload of `LB_EV_OVERFLOW`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbEvOverflow {
    pub dropped_records: u32,
    pub dropped_bytes: u32,
}

// ---------------------------------------------------------------------------------------------
// Commands, feedback, traces, entities
// ---------------------------------------------------------------------------------------------

pub const LB_CMD_SET_SEED: u8 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbBotCommand {
    pub slot: u8,
    pub flags: u8,
    pub buttons: u16,
    pub bot_gen: u32,
    pub view_angles: LbVec3,
    pub forwardmove: f32,
    pub sidemove: f32,
    pub upmove: f32,
    pub impulse: u8,
    pub msec: u8,
    pub pad: u16,
    pub random_seed: u32,
}

pub const LB_MOVE_OK: u8 = 0;
pub const LB_MOVE_STALE: u8 = 1;
pub const LB_MOVE_NOT_OURS: u8 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbMoveFeedback {
    pub slot: u8,
    pub status: u8,
    pub deadflag: u8,
    pub waterlevel: u8,
    pub flags: u32,
    pub frame_no: u64,
    pub origin: LbVec3,
    pub velocity: LbVec3,
    pub v_angle: LbVec3,
    pub health: f32,
    pub movetype: u8,
    pub pad: [u8; 3],
    pub pad2: u32,
}

pub const LB_MAX_CLIENT_CMD_ARGS: usize = 8;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbClientCommand {
    pub slot: u8,
    pub argc: u8,
    pub pad: u16,
    pub bot_gen: u32,
    pub argv: [LbStr; LB_MAX_CLIENT_CMD_ARGS],
}

pub const LB_TRACE_LINE: u8 = 1;
pub const LB_TRACE_HULL: u8 = 2;
pub const LB_TRACE_MODEL: u8 = 3;

pub const LB_TRACE_IGNORE_MONSTERS: u16 = 1;
pub const LB_TRACE_IGNORE_GLASS: u16 = 2;
pub const LB_TRACE_MISSILE: u16 = 4;

/// Hull numbers as used by the engine: 0 point, 1 standing player, 2 large, 3 crouching player.
pub const LB_HULL_POINT: u8 = 0;
pub const LB_HULL_HUMAN: u8 = 1;
pub const LB_HULL_LARGE: u8 = 2;
pub const LB_HULL_HEAD: u8 = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbTraceRequest {
    pub start: LbVec3,
    pub end: LbVec3,
    pub kind: u8,
    pub hull: u8,
    pub flags: u16,
    pub ignore: LbEntRef,
    pub model: LbEntRef,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct LbTraceResult {
    pub fraction: f32,
    pub end_pos: LbVec3,
    pub plane_normal: LbVec3,
    pub plane_dist: f32,
    pub hit: LbEntRef,
    pub hitgroup: i32,
    pub all_solid: u8,
    pub start_solid: u8,
    pub in_open: u8,
    pub in_water: u8,
    pub hit_rendermode: u8,
    pub hit_renderfx: u8,
    pub hit_solid: u8,
    pub pad: u8,
    pub hit_renderamt: f32,
    pub hit_classname_id: u16,
    pub pad2: u16,
    pub pad3: u32,
}

pub const LB_TRACK_EXACT: u8 = 1;
pub const LB_TRACK_PREFIX: u8 = 2;

/// Entity kinds assigned by the track rules (values chosen by the core).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbTrackRule {
    pub pattern: LbStr,
    pub match_mode: u8,
    pub kind: u8,
    pub pad: [u8; 6],
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbEntitySnapshot {
    pub ent: LbEntRef,
    pub owner: LbEntRef,
    pub classname_id: u16,
    pub model_id: u16,
    pub kind: u8,
    pub solid: u8,
    pub movetype: u8,
    pub rendermode: u8,
    pub origin: LbVec3,
    pub angles: LbVec3,
    pub velocity: LbVec3,
    pub avelocity: LbVec3,
    pub absmin: LbVec3,
    pub absmax: LbVec3,
    pub rendercolor: LbVec3,
    pub effects: u32,
    pub spawnflags: u32,
    pub frame: f32,
    pub renderamt: f32,
    pub renderfx: u8,
    pub deadflag: u8,
    pub pad: [u8; 10],
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbKeyValue {
    pub key: LbStr,
    pub value: LbStr,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbCreateBotRequest {
    pub name: LbStr,
    pub connect_addr: LbStr,
    pub infokeys: *const LbKeyValue,
    pub infokey_count: u32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbCreateBotResult {
    pub status: i32,
    pub slot: u8,
    pub pad: [u8; 3],
    pub userid: i32,
    pub bot_gen: u32,
    pub reject_reason: [u8; 128],
}

pub const LB_DISGUISE_PING: u8 = 1;
pub const LB_DISGUISE_AUTHID: u8 = 2;
pub const LB_DISGUISE_SESSION: u8 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbDisguise {
    pub slot: u8,
    pub flags: u8,
    pub ping: u16,
    pub bot_gen: u32,
    pub loss: u8,
    pub pad: [u8; 3],
    pub session_seconds: f32,
    pub authid: [u8; 40],
}

pub const LB_DEBUG_LINE: u8 = 1;
pub const LB_DEBUG_TEXT: u8 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbDebugPrim {
    pub kind: u8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub width: u8,
    pub life_ds: u8,
    pub brightness: u8,
    pub channel: u8,
    pub a: LbVec3,
    pub b2: LbVec3,
    pub pad: u32,
    pub text: LbStr,
}

pub const LB_CVAR_PROTECTED: u32 = 1;
pub const LB_CVAR_SERVER: u32 = 2;
pub const LB_CVAR_READONLY: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbCvarSpec {
    pub name: LbStr,
    pub default_value: LbStr,
    pub flags: u32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbOwnedBuffer {
    pub ptr: *const u8,
    pub len: u32,
    pub pad: u32,
    pub token: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LbCompatFacts {
    pub engine_kind: u8,
    pub metamod_has_hook_tables: u8,
    pub rehlds_major: u8,
    pub rehlds_minor: u8,
    pub rehlds_build: i32,
    pub channels: u32,
    pub pad: u32,
    pub engine_version: [u8; 64],
    pub metamod_version: [u8; 32],
    pub gamedll_desc: [u8; 64],
    pub gamedll_path: [u8; 256],
}

/// Weapon slots in [`LbWeaponState::weapons`] (weapon ids 0..31).
pub const LB_MAX_WEAPONS: usize = 32;

/// One weapon as the bot's own client would be told it for prediction (`weapon_data_t` from the game's
/// `GetWeaponData`). Times are seconds from now.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LbWeaponData {
    /// Weapon id (`WEAPON_*`); 0 = not carried.
    pub id: i32,
    pub clip: i32,
    /// Until the next primary and secondary attack; zero or less: ready.
    pub next_primary: f32,
    pub next_secondary: f32,
    pub idle: f32,
    pub in_reload: i32,
    pub in_special_reload: i32,
    /// `m_chargeReady`, `m_fInAttack`, `m_fireState` (grenade pin, gauss charge, egon beam).
    pub iuser1: i32,
    pub iuser2: i32,
    pub iuser3: i32,
    /// `pev->fuser1`, `m_flStartThrow`, `m_flReleaseThrow`.
    pub fuser1: f32,
    pub fuser2: f32,
    pub fuser3: f32,
    pub pad: u32,
}

/// Filled by `get_weapon_data`: the bot's weapons and what `UpdateClientData` tells its client.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LbWeaponState {
    /// Active weapon id (`clientdata.m_iId`); 0 = none.
    pub current: i32,
    /// Until any attack is allowed (`m_flNextAttack`: weapon switches, deploys).
    pub next_attack: f32,
    /// `m_flNextAmmoBurn`, `m_flAmmoStartCharge` (egon, gauss charging).
    pub next_ammo_burn: f32,
    pub ammo_start_charge: f32,
    /// The active weapon's ammo types and what the player carries of them.
    pub primary_type: i32,
    pub primary_ammo: i32,
    pub secondary_type: i32,
    pub secondary_ammo: i32,
    pub weapons: [LbWeaponData; LB_MAX_WEAPONS],
}

pub const LB_PRINT_CONSOLE: u8 = 1;
pub const LB_PRINT_CENTER: u8 = 2;
pub const LB_PRINT_CHAT: u8 = 3;

// ---------------------------------------------------------------------------------------------
// Host API: C++ -> Rust callbacks. Main thread only, valid only during an `lb_core_*` call.
// ---------------------------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LbHostApi {
    pub struct_size: u32,
    pub abi_version: u32,
    pub ctx: *mut c_void,
    pub server_print: Option<unsafe extern "C" fn(ctx: *mut c_void, text: LbStr)>,
    pub client_print: Option<unsafe extern "C" fn(ctx: *mut c_void, slot: u8, where_: u8, text: LbStr)>,
    /// Queued; executed by the engine after the current frame, never immediately.
    pub server_command: Option<unsafe extern "C" fn(ctx: *mut c_void, text: LbStr)>,
    pub cvar_register: Option<
        unsafe extern "C" fn(ctx: *mut c_void, specs: *const LbCvarSpec, count: u32, out_handles: *mut u16) -> i32,
    >,
    pub cvar_find: Option<unsafe extern "C" fn(ctx: *mut c_void, name: LbStr) -> u16>,
    pub cvar_get_float: Option<unsafe extern "C" fn(ctx: *mut c_void, handle: u16) -> f32>,
    pub cvar_get_string: Option<unsafe extern "C" fn(ctx: *mut c_void, handle: u16, buf: *mut u8, cap: u32) -> u32>,
    pub cvar_set: Option<unsafe extern "C" fn(ctx: *mut c_void, handle: u16, value: LbStr)>,
    pub trace: Option<unsafe extern "C" fn(ctx: *mut c_void, req: *const LbTraceRequest, out: *mut LbTraceResult)>,
    pub trace_batch: Option<
        unsafe extern "C" fn(ctx: *mut c_void, reqs: *const LbTraceRequest, out: *mut LbTraceResult, count: u32) -> u32,
    >,
    pub point_contents: Option<unsafe extern "C" fn(ctx: *mut c_void, point: LbVec3) -> i32>,
    pub registry_set_rules:
        Option<unsafe extern "C" fn(ctx: *mut c_void, rules: *const LbTrackRule, count: u32) -> i32>,
    pub snapshot_entities:
        Option<unsafe extern "C" fn(ctx: *mut c_void, kind_mask: u32, out: *mut LbEntitySnapshot, cap: u32) -> u32>,
    pub get_entity: Option<unsafe extern "C" fn(ctx: *mut c_void, ent: LbEntRef, out: *mut LbEntitySnapshot) -> i32>,
    pub set_capture_mask: Option<unsafe extern "C" fn(ctx: *mut c_void, mask: *const u8) -> i32>,
    pub resolve_user_msg: Option<unsafe extern "C" fn(ctx: *mut c_void, name: LbStr, size: *mut i32) -> i32>,
    pub create_bot: Option<
        unsafe extern "C" fn(ctx: *mut c_void, req: *const LbCreateBotRequest, out: *mut LbCreateBotResult) -> i32,
    >,
    pub kick_bot: Option<unsafe extern "C" fn(ctx: *mut c_void, slot: u8, bot_gen: u32, reason: LbStr) -> i32>,
    pub bot_client_commands:
        Option<unsafe extern "C" fn(ctx: *mut c_void, cmds: *const LbClientCommand, count: u32) -> i32>,
    pub run_player_moves: Option<
        unsafe extern "C" fn(
            ctx: *mut c_void,
            cmds: *const LbBotCommand,
            count: u32,
            feedback: *mut LbMoveFeedback,
        ) -> i32,
    >,
    pub get_physics_key:
        Option<unsafe extern "C" fn(ctx: *mut c_void, slot: u8, key: LbStr, buf: *mut u8, cap: u32) -> u32>,
    pub get_client_info_key:
        Option<unsafe extern "C" fn(ctx: *mut c_void, slot: u8, key: LbStr, buf: *mut u8, cap: u32) -> u32>,
    pub get_weapon_data: Option<unsafe extern "C" fn(ctx: *mut c_void, slot: u8, out: *mut c_void, cap: u32) -> i32>,
    pub set_bot_disguise: Option<unsafe extern "C" fn(ctx: *mut c_void, d: *const LbDisguise) -> i32>,
    pub get_player_stats:
        Option<unsafe extern "C" fn(ctx: *mut c_void, slot: u8, ping: *mut i32, loss: *mut i32) -> i32>,
    pub load_file: Option<unsafe extern "C" fn(ctx: *mut c_void, path: LbStr, out: *mut LbOwnedBuffer) -> i32>,
    pub free_file: Option<unsafe extern "C" fn(ctx: *mut c_void, buf: *mut LbOwnedBuffer)>,
    pub send_debug:
        Option<unsafe extern "C" fn(ctx: *mut c_void, slot: u8, prims: *const LbDebugPrim, count: u32) -> i32>,
    pub get_compat_facts: Option<unsafe extern "C" fn(ctx: *mut c_void, out: *mut LbCompatFacts) -> i32>,
}

/// Sizes of all ABI structs as seen by Rust, indexed by the `LB_SZ_*` constants.
pub fn abi_sizes() -> [u32; LB_SZ_COUNT] {
    use core::mem::size_of;
    let mut s = [0u32; LB_SZ_COUNT];
    s[LB_SZ_INIT_INFO] = size_of::<LbInitInfo>() as u32;
    s[LB_SZ_MAP_INFO] = size_of::<LbMapInfo>() as u32;
    s[LB_SZ_FRAME_HEADER] = size_of::<LbFrameHeader>() as u32;
    s[LB_SZ_FRAME_INPUT] = size_of::<LbFrameInput>() as u32;
    s[LB_SZ_CLIENT_SNAPSHOT] = size_of::<LbClientSnapshot>() as u32;
    s[LB_SZ_SELF_SNAPSHOT] = size_of::<LbSelfSnapshot>() as u32;
    s[LB_SZ_EVENT_HEADER] = size_of::<LbEventHeader>() as u32;
    s[LB_SZ_BOT_COMMAND] = size_of::<LbBotCommand>() as u32;
    s[LB_SZ_MOVE_FEEDBACK] = size_of::<LbMoveFeedback>() as u32;
    s[LB_SZ_CLIENT_COMMAND] = size_of::<LbClientCommand>() as u32;
    s[LB_SZ_TRACE_REQUEST] = size_of::<LbTraceRequest>() as u32;
    s[LB_SZ_TRACE_RESULT] = size_of::<LbTraceResult>() as u32;
    s[LB_SZ_ENTITY_SNAPSHOT] = size_of::<LbEntitySnapshot>() as u32;
    s[LB_SZ_TRACK_RULE] = size_of::<LbTrackRule>() as u32;
    s[LB_SZ_CREATE_BOT_REQUEST] = size_of::<LbCreateBotRequest>() as u32;
    s[LB_SZ_CREATE_BOT_RESULT] = size_of::<LbCreateBotResult>() as u32;
    s[LB_SZ_DEBUG_PRIM] = size_of::<LbDebugPrim>() as u32;
    s[LB_SZ_CVAR_SPEC] = size_of::<LbCvarSpec>() as u32;
    s[LB_SZ_COMPAT_FACTS] = size_of::<LbCompatFacts>() as u32;
    s[LB_SZ_HOST_API] = size_of::<LbHostApi>() as u32;
    s[LB_SZ_MSG_ARG] = size_of::<LbMsgArg>() as u32;
    s[LB_SZ_DISGUISE] = size_of::<LbDisguise>() as u32;
    s[LB_SZ_WEAPON_STATE] = size_of::<LbWeaponState>() as u32;
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, size_of};

    /// Structs whose size must not depend on pointer width or 64-bit member alignment.
    #[test]
    fn pointer_free_structs_are_8_byte_multiples() {
        for (name, size) in [
            ("LbFrameHeader", size_of::<LbFrameHeader>()),
            ("LbClientSnapshot", size_of::<LbClientSnapshot>()),
            ("LbSelfSnapshot", size_of::<LbSelfSnapshot>()),
            ("LbEventHeader", size_of::<LbEventHeader>()),
            ("LbBotCommand", size_of::<LbBotCommand>()),
            ("LbMoveFeedback", size_of::<LbMoveFeedback>()),
            ("LbTraceRequest", size_of::<LbTraceRequest>()),
            ("LbTraceResult", size_of::<LbTraceResult>()),
            ("LbEntitySnapshot", size_of::<LbEntitySnapshot>()),
            ("LbMsgArg", size_of::<LbMsgArg>()),
            ("LbEvUserMsg", size_of::<LbEvUserMsg>()),
            ("LbEvSound", size_of::<LbEvSound>()),
            ("LbEvPlayback", size_of::<LbEvPlayback>()),
            ("LbEvEntity", size_of::<LbEvEntity>()),
            ("LbEvClient", size_of::<LbEvClient>()),
            ("LbEvCommand", size_of::<LbEvCommand>()),
            ("LbEvFixangle", size_of::<LbEvFixangle>()),
            ("LbEvNamed", size_of::<LbEvNamed>()),
            ("LbCompatFacts", size_of::<LbCompatFacts>()),
            ("LbDisguise", size_of::<LbDisguise>()),
            ("LbWeaponData", size_of::<LbWeaponData>()),
            ("LbWeaponState", size_of::<LbWeaponState>()),
        ] {
            assert_eq!(size % 8, 0, "{name} size {size} is not a multiple of 8");
        }
    }

    #[test]
    fn fixed_sizes() {
        assert_eq!(size_of::<LbEventHeader>(), 24);
        assert_eq!(size_of::<LbFrameHeader>(), 56);
        assert_eq!(size_of::<LbBotCommand>(), 40);
        assert_eq!(size_of::<LbMsgArg>(), 16);
        assert_eq!(size_of::<LbTraceResult>(), 64);
        assert_eq!(size_of::<LbEntitySnapshot>(), 136);
        assert_eq!(size_of::<LbMoveFeedback>(), 64);
        assert_eq!(size_of::<LbEvEntity>(), 32);
        assert_eq!(size_of::<LbClientSnapshot>(), 136);
        assert_eq!(size_of::<LbSelfSnapshot>(), 144);
        assert!(align_of::<LbEventHeader>() <= 8);
    }
}
