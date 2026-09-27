//! Recording what the engine answers the core, and giving the same answers again in a replay.
//!
//! `RecordingHost` passes every call on to the real host and keeps what came back. `ReplayHost` hands the kept
//! answers out in the same order; a replayed core that asks something else, or in another order, has left the
//! recording. Calls without an answer (prints, server commands, debug drawing) are not kept: the core makes some of
//! them from log lines, which other threads write at their own pace.

use std::collections::VecDeque;
use std::hash::Hasher;

use lb_core::Vec3;
use lb_ffi::{
    LbBotCommand, LbClientSnapshot, LbEntRef, LbEntitySnapshot, LbEventBatch, LbFrameHeader, LbFrameInput,
    LbMoveFeedback, LbSelfSnapshot, LbWeaponState,
};
use rustc_hash::FxHasher;
use serde::{Deserialize, Serialize};

use crate::host::*;

/// ABI records made of plain numbers: any bytes of the right length are a valid value.
///
/// # Safety
/// Only for `#[repr(C)]` structs whose fields are integers, floats or such structs, padded explicitly.
pub unsafe trait Pod: Copy {}

// SAFETY: repr(C) records of integers and floats with explicit padding fields (the lb-ffi rules).
unsafe impl Pod for LbBotCommand {}
// SAFETY: as above.
unsafe impl Pod for LbMoveFeedback {}
// SAFETY: as above.
unsafe impl Pod for LbEntitySnapshot {}
// SAFETY: as above.
unsafe impl Pod for LbWeaponState {}
// SAFETY: as above.
unsafe impl Pod for LbClientSnapshot {}
// SAFETY: as above.
unsafe impl Pod for LbSelfSnapshot {}
// SAFETY: as above.
unsafe impl Pod for LbFrameHeader {}

pub fn pod_bytes<T: Pod>(items: &[T]) -> Vec<u8> {
    // SAFETY: a `Pod` has no implicit padding, so every byte of the slice is initialized.
    unsafe { core::slice::from_raw_parts(items.as_ptr().cast::<u8>(), core::mem::size_of_val(items)) }.to_vec()
}

/// The records back from `pod_bytes`; a trailing partial record is dropped.
pub fn from_pod<T: Pod>(bytes: &[u8]) -> Vec<T> {
    bytes
        .chunks_exact(core::mem::size_of::<T>())
        // SAFETY: the chunk holds `size_of::<T>()` bytes, and any bytes make a valid `Pod`.
        .map(|c| unsafe { core::ptr::read_unaligned(c.as_ptr().cast::<T>()) })
        .collect()
}

/// What a recording keeps in place of a secret.
pub const REDACTED: &str = "<redacted>";

/// One host call and what it returned.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum HostCall {
    CvarRegister(Vec<Option<CvarHandle>>),
    CvarFind(Option<CvarHandle>),
    CvarFloat(f32),
    CvarString(String),
    /// `query` is a hash of the request, so a replay tracing elsewhere is caught at that trace.
    Trace {
        query: u64,
        result: TraceResult,
    },
    PointContents {
        query: u64,
        contents: i32,
    },
    SetTrackRules(bool),
    SnapshotEntities {
        mask: u32,
        entities: Vec<u8>,
    },
    GetEntity(Option<Vec<u8>>),
    SetCaptureMask(bool),
    ResolveUserMsg(Option<(i32, i32)>),
    CreateBot {
        name: String,
        outcome: CreateBotOutcome,
    },
    KickBot {
        slot: u8,
        ok: bool,
    },
    ClientCommand {
        slot: u8,
        argv: Vec<String>,
        ok: bool,
    },
    /// What the bots were told to do (`LbBotCommand`s) and the engine's feedback.
    RunPlayerMoves {
        cmds: Vec<u8>,
        feedback: Vec<u8>,
        ok: bool,
    },
    PhysicsKey(String),
    ClientInfoKey(String),
    PlayerStats(Option<(i32, i32)>),
    LoadFile(Option<Vec<u8>>),
    WeaponState(Option<Vec<u8>>),
    CompatFacts(CompatFacts),
}

impl HostCall {
    pub fn name(&self) -> &'static str {
        match self {
            HostCall::CvarRegister(_) => "cvar_register",
            HostCall::CvarFind(_) => "cvar_find",
            HostCall::CvarFloat(_) => "cvar_float",
            HostCall::CvarString(_) => "cvar_string",
            HostCall::Trace { .. } => "trace",
            HostCall::PointContents { .. } => "point_contents",
            HostCall::SetTrackRules(_) => "set_track_rules",
            HostCall::SnapshotEntities { .. } => "snapshot_entities",
            HostCall::GetEntity(_) => "get_entity",
            HostCall::SetCaptureMask(_) => "set_capture_mask",
            HostCall::ResolveUserMsg(_) => "resolve_user_msg",
            HostCall::CreateBot { .. } => "create_bot",
            HostCall::KickBot { .. } => "kick_bot",
            HostCall::ClientCommand { .. } => "bot_client_command",
            HostCall::RunPlayerMoves { .. } => "run_player_moves",
            HostCall::PhysicsKey(_) => "physics_key",
            HostCall::ClientInfoKey(_) => "client_info_key",
            HostCall::PlayerStats(_) => "player_stats",
            HostCall::LoadFile(_) => "load_file",
            HostCall::WeaponState(_) => "weapon_state",
            HostCall::CompatFacts(_) => "compat_facts",
        }
    }
}

fn hash_vec(h: &mut FxHasher, v: Vec3) {
    for c in v.to_array() {
        h.write_u32(c.to_bits());
    }
}

fn hash_ent(h: &mut FxHasher, e: LbEntRef) {
    h.write_u16(e.index);
    h.write_u32(e.serial);
}

fn trace_key(req: &TraceRequest) -> u64 {
    let mut h = FxHasher::default();
    hash_vec(&mut h, req.start);
    hash_vec(&mut h, req.end);
    match req.kind {
        TraceKind::Line => h.write_u8(0),
        TraceKind::Hull(n) => h.write_u16(0x100 | u16::from(n)),
        TraceKind::Model(e) => {
            h.write_u8(2);
            hash_ent(&mut h, e);
        }
    }
    h.write_u8(u8::from(req.ignore_monsters) | u8::from(req.ignore_glass) << 1);
    if let Some(e) = req.ignore {
        hash_ent(&mut h, e);
    }
    h.finish()
}

fn point_key(p: Vec3) -> u64 {
    let mut h = FxHasher::default();
    hash_vec(&mut h, p);
    h.finish()
}

/// Passes calls to the real host, keeping the answers in `calls` while recording. The list is the caller's, so
/// the calls made before a panic are not lost with the host.
pub struct RecordingHost<'a> {
    inner: &'a mut dyn Host,
    calls: Option<&'a mut Vec<HostCall>>,
    secret: Option<CvarHandle>,
}

impl<'a> RecordingHost<'a> {
    pub fn new(inner: &'a mut dyn Host, calls: Option<&'a mut Vec<HostCall>>) -> RecordingHost<'a> {
        RecordingHost {
            inner,
            calls,
            secret: None,
        }
    }

    /// Keeps [`REDACTED`] in place of this cvar's value, unless it is empty.
    pub fn hiding(mut self, secret: Option<CvarHandle>) -> RecordingHost<'a> {
        self.secret = secret;
        self
    }

    fn keep(&mut self, call: impl FnOnce() -> HostCall) {
        if let Some(calls) = self.calls.as_mut() {
            calls.push(call());
        }
    }
}

impl Host for RecordingHost<'_> {
    fn server_print(&mut self, text: &str) {
        self.inner.server_print(text);
    }

    fn client_print(&mut self, slot: u8, kind: PrintKind, text: &str) {
        self.inner.client_print(slot, kind, text);
    }

    fn server_command(&mut self, command: &str) {
        self.inner.server_command(command);
    }

    fn cvar_register(&mut self, specs: &[CvarSpec]) -> Vec<Option<CvarHandle>> {
        let r = self.inner.cvar_register(specs);
        self.keep(|| HostCall::CvarRegister(r.clone()));
        r
    }

    fn cvar_find(&mut self, name: &str) -> Option<CvarHandle> {
        let r = self.inner.cvar_find(name);
        self.keep(|| HostCall::CvarFind(r));
        r
    }

    fn cvar_float(&mut self, handle: CvarHandle) -> f32 {
        let r = self.inner.cvar_float(handle);
        self.keep(|| HostCall::CvarFloat(r));
        r
    }

    fn cvar_string(&mut self, handle: CvarHandle) -> String {
        let r = self.inner.cvar_string(handle);
        let hidden = self.secret == Some(handle) && !r.is_empty();
        self.keep(|| HostCall::CvarString(if hidden { REDACTED.into() } else { r.clone() }));
        r
    }

    fn cvar_set(&mut self, handle: CvarHandle, value: &str) {
        self.inner.cvar_set(handle, value);
    }

    fn trace(&mut self, req: &TraceRequest) -> TraceResult {
        let result = self.inner.trace(req);
        self.keep(|| HostCall::Trace {
            query: trace_key(req),
            result,
        });
        result
    }

    fn point_contents(&mut self, point: Vec3) -> i32 {
        let contents = self.inner.point_contents(point);
        self.keep(|| HostCall::PointContents {
            query: point_key(point),
            contents,
        });
        contents
    }

    fn set_track_rules(&mut self, rules: &[TrackRule]) -> bool {
        let r = self.inner.set_track_rules(rules);
        self.keep(|| HostCall::SetTrackRules(r));
        r
    }

    fn snapshot_entities(&mut self, kind_mask: u32, out: &mut Vec<EntitySnapshot>) {
        self.inner.snapshot_entities(kind_mask, out);
        self.keep(|| HostCall::SnapshotEntities {
            mask: kind_mask,
            entities: pod_bytes(out),
        });
    }

    fn get_entity(&mut self, ent: LbEntRef) -> Option<EntitySnapshot> {
        let r = self.inner.get_entity(ent);
        self.keep(|| HostCall::GetEntity(r.as_ref().map(|e| pod_bytes(std::slice::from_ref(e)))));
        r
    }

    fn set_capture_mask(&mut self, mask: &[u8; 32]) -> bool {
        let r = self.inner.set_capture_mask(mask);
        self.keep(|| HostCall::SetCaptureMask(r));
        r
    }

    fn resolve_user_msg(&mut self, name: &str) -> Option<(i32, i32)> {
        let r = self.inner.resolve_user_msg(name);
        self.keep(|| HostCall::ResolveUserMsg(r));
        r
    }

    fn create_bot(&mut self, req: &CreateBotRequest) -> CreateBotOutcome {
        let outcome = self.inner.create_bot(req);
        self.keep(|| HostCall::CreateBot {
            name: req.name.clone(),
            outcome: outcome.clone(),
        });
        outcome
    }

    fn kick_bot(&mut self, slot: u8, bot_gen: u32, reason: &str) -> bool {
        let ok = self.inner.kick_bot(slot, bot_gen, reason);
        self.keep(|| HostCall::KickBot { slot, ok });
        ok
    }

    fn bot_client_command(&mut self, slot: u8, bot_gen: u32, argv: &[&str]) -> bool {
        let ok = self.inner.bot_client_command(slot, bot_gen, argv);
        self.keep(|| HostCall::ClientCommand {
            slot,
            argv: argv.iter().map(|a| a.to_string()).collect(),
            ok,
        });
        ok
    }

    fn run_player_moves(&mut self, cmds: &[LbBotCommand], feedback: &mut Vec<LbMoveFeedback>) -> bool {
        let ok = self.inner.run_player_moves(cmds, feedback);
        self.keep(|| HostCall::RunPlayerMoves {
            cmds: pod_bytes(cmds),
            feedback: pod_bytes(feedback),
            ok,
        });
        ok
    }

    fn physics_key(&mut self, slot: u8, key: &str) -> String {
        let r = self.inner.physics_key(slot, key);
        self.keep(|| HostCall::PhysicsKey(r.clone()));
        r
    }

    fn client_info_key(&mut self, slot: u8, key: &str) -> String {
        let r = self.inner.client_info_key(slot, key);
        self.keep(|| HostCall::ClientInfoKey(r.clone()));
        r
    }

    fn player_stats(&mut self, slot: u8) -> Option<(i32, i32)> {
        let r = self.inner.player_stats(slot);
        self.keep(|| HostCall::PlayerStats(r));
        r
    }

    fn load_file(&mut self, path: &str) -> Option<Vec<u8>> {
        let r = self.inner.load_file(path);
        self.keep(|| HostCall::LoadFile(r.clone()));
        r
    }

    fn weapon_state(&mut self, slot: u8) -> Option<LbWeaponState> {
        let r = self.inner.weapon_state(slot);
        self.keep(|| HostCall::WeaponState(r.as_ref().map(|w| pod_bytes(std::slice::from_ref(w)))));
        r
    }

    fn send_debug(&mut self, slot: u8, prims: &[DebugPrim]) -> bool {
        self.inner.send_debug(slot, prims)
    }

    fn compat_facts(&mut self) -> CompatFacts {
        let r = self.inner.compat_facts();
        self.keep(|| HostCall::CompatFacts(r.clone()));
        r
    }
}

/// Answers host calls from a recording, checking that the core asks what it asked when recorded.
#[derive(Default)]
pub struct ReplayHost {
    calls: VecDeque<HostCall>,
    /// Why the replay stopped following the recording: the core asked something the recording does not have.
    /// Every later call gets a default answer.
    pub broken: Option<String>,
    /// Decisions (bot commands, client commands, bots added or kicked) that came out different.
    pub diffs: Vec<String>,
    /// Bot commands compared with the recorded ones.
    pub commands_checked: u64,
    /// Console output of the core, kept when `Some`.
    pub console: Option<Vec<String>>,
}

macro_rules! answer {
    ($self:ident, $what:literal, $pat:pat => $value:expr, else $default:expr) => {
        match $self.next($what) {
            Some($pat) => $value,
            other => {
                $self.unexpected($what, other);
                $default
            }
        }
    };
}

impl ReplayHost {
    pub fn new(calls: Vec<HostCall>) -> ReplayHost {
        ReplayHost {
            calls: calls.into(),
            ..ReplayHost::default()
        }
    }

    /// The answers for the next step of the recording.
    pub fn load(&mut self, calls: Vec<HostCall>) {
        self.calls = calls.into();
    }

    /// Recorded calls the core has not made (yet).
    pub fn left(&self) -> impl Iterator<Item = &HostCall> {
        self.calls.iter()
    }

    fn next(&mut self, what: &'static str) -> Option<HostCall> {
        if self.broken.is_some() {
            return None;
        }
        let call = self.calls.pop_front();
        if call.is_none() {
            self.broken = Some(format!(
                "the core called {what}, the recording has no more calls in this step"
            ));
        }
        call
    }

    fn unexpected(&mut self, what: &'static str, got: Option<HostCall>) {
        if let Some(call) = got {
            self.broken = Some(format!(
                "the core called {what}, the recording has {} here",
                call.name()
            ));
        }
    }

    fn check(&mut self, what: &'static str, same: bool, detail: impl FnOnce() -> String) {
        if !same && self.broken.is_none() {
            self.broken = Some(format!("{what}: {}", detail()));
        }
    }
}

impl Host for ReplayHost {
    fn server_print(&mut self, text: &str) {
        if let Some(console) = self.console.as_mut() {
            console.push(text.trim_end().to_string());
        }
    }

    fn client_print(&mut self, _slot: u8, _kind: PrintKind, _text: &str) {}

    fn server_command(&mut self, _command: &str) {}

    fn cvar_register(&mut self, specs: &[CvarSpec]) -> Vec<Option<CvarHandle>> {
        answer!(self, "cvar_register", HostCall::CvarRegister(h) => h, else vec![None; specs.len()])
    }

    fn cvar_find(&mut self, _name: &str) -> Option<CvarHandle> {
        answer!(self, "cvar_find", HostCall::CvarFind(h) => h, else None)
    }

    fn cvar_float(&mut self, _handle: CvarHandle) -> f32 {
        answer!(self, "cvar_float", HostCall::CvarFloat(v) => v, else 0.0)
    }

    fn cvar_string(&mut self, _handle: CvarHandle) -> String {
        answer!(self, "cvar_string", HostCall::CvarString(v) => v, else String::new())
    }

    fn cvar_set(&mut self, _handle: CvarHandle, _value: &str) {}

    fn trace(&mut self, req: &TraceRequest) -> TraceResult {
        let (query, result) = answer!(self, "trace", HostCall::Trace { query, result } => (query, result),
            else return TraceResult::default());
        self.check("trace", query == trace_key(req), || {
            format!("a trace from {} to {} the recording does not have", req.start, req.end)
        });
        result
    }

    fn point_contents(&mut self, point: Vec3) -> i32 {
        let (query, contents) = answer!(self, "point_contents",
            HostCall::PointContents { query, contents } => (query, contents), else return 0);
        self.check("point_contents", query == point_key(point), || {
            format!("contents at {point} the recording does not have")
        });
        contents
    }

    fn set_track_rules(&mut self, _rules: &[TrackRule]) -> bool {
        answer!(self, "set_track_rules", HostCall::SetTrackRules(ok) => ok, else false)
    }

    fn snapshot_entities(&mut self, kind_mask: u32, out: &mut Vec<EntitySnapshot>) {
        out.clear();
        let (mask, entities) = answer!(self, "snapshot_entities",
            HostCall::SnapshotEntities { mask, entities } => (mask, entities), else return);
        self.check("snapshot_entities", mask == kind_mask, || {
            format!("kinds {kind_mask:#x} instead of the recorded {mask:#x}")
        });
        out.extend(from_pod::<EntitySnapshot>(&entities));
    }

    fn get_entity(&mut self, _ent: LbEntRef) -> Option<EntitySnapshot> {
        answer!(self, "get_entity",
            HostCall::GetEntity(e) => e.and_then(|b| from_pod::<EntitySnapshot>(&b).into_iter().next()), else None)
    }

    fn set_capture_mask(&mut self, _mask: &[u8; 32]) -> bool {
        answer!(self, "set_capture_mask", HostCall::SetCaptureMask(ok) => ok, else false)
    }

    fn resolve_user_msg(&mut self, _name: &str) -> Option<(i32, i32)> {
        answer!(self, "resolve_user_msg", HostCall::ResolveUserMsg(r) => r, else None)
    }

    fn create_bot(&mut self, req: &CreateBotRequest) -> CreateBotOutcome {
        let (name, outcome) = answer!(self, "create_bot", HostCall::CreateBot { name, outcome } => (name, outcome),
            else return CreateBotOutcome::Failed(-1));
        if name != req.name {
            self.diffs.push(format!("added bot {} instead of {name}", req.name));
        }
        outcome
    }

    fn kick_bot(&mut self, slot: u8, _bot_gen: u32, _reason: &str) -> bool {
        let (recorded, ok) = answer!(self, "kick_bot", HostCall::KickBot { slot, ok } => (slot, ok), else return false);
        if recorded != slot {
            self.diffs.push(format!("kicked slot {slot} instead of {recorded}"));
        }
        ok
    }

    fn bot_client_command(&mut self, slot: u8, _bot_gen: u32, argv: &[&str]) -> bool {
        let (recorded_slot, recorded, ok) = answer!(self, "bot_client_command",
            HostCall::ClientCommand { slot, argv, ok } => (slot, argv, ok), else return false);
        if recorded_slot != slot || recorded.iter().map(String::as_str).ne(argv.iter().copied()) {
            self.diffs.push(format!(
                "slot {slot}: client command `{}` instead of `{}` (slot {recorded_slot})",
                argv.join(" "),
                recorded.join(" ")
            ));
        }
        ok
    }

    fn run_player_moves(&mut self, cmds: &[LbBotCommand], feedback: &mut Vec<LbMoveFeedback>) -> bool {
        feedback.clear();
        let (recorded, fb, ok) = answer!(self, "run_player_moves",
            HostCall::RunPlayerMoves { cmds, feedback, ok } => (cmds, feedback, ok), else return false);
        self.commands_checked += cmds.len() as u64;
        if pod_bytes(cmds) != recorded {
            self.diffs.push(describe_commands(&from_pod(&recorded), cmds));
        }
        feedback.extend(from_pod::<LbMoveFeedback>(&fb));
        ok
    }

    fn physics_key(&mut self, _slot: u8, _key: &str) -> String {
        answer!(self, "physics_key", HostCall::PhysicsKey(v) => v, else String::new())
    }

    fn client_info_key(&mut self, _slot: u8, _key: &str) -> String {
        answer!(self, "client_info_key", HostCall::ClientInfoKey(v) => v, else String::new())
    }

    fn player_stats(&mut self, _slot: u8) -> Option<(i32, i32)> {
        answer!(self, "player_stats", HostCall::PlayerStats(v) => v, else None)
    }

    fn load_file(&mut self, _path: &str) -> Option<Vec<u8>> {
        answer!(self, "load_file", HostCall::LoadFile(v) => v, else None)
    }

    fn weapon_state(&mut self, _slot: u8) -> Option<LbWeaponState> {
        answer!(self, "weapon_state",
            HostCall::WeaponState(w) => w.and_then(|b| from_pod::<LbWeaponState>(&b).into_iter().next()), else None)
    }

    fn send_debug(&mut self, _slot: u8, _prims: &[DebugPrim]) -> bool {
        true
    }

    fn compat_facts(&mut self) -> CompatFacts {
        answer!(self, "compat_facts", HostCall::CompatFacts(f) => f, else CompatFacts::default())
    }
}

/// How replayed bot commands differ from the recorded ones.
pub fn describe_commands(recorded: &[LbBotCommand], replayed: &[LbBotCommand]) -> String {
    let mut out = Vec::new();
    if recorded.len() != replayed.len() {
        out.push(format!("{} commands instead of {}", replayed.len(), recorded.len()));
    }
    let bits = |v: lb_ffi::LbVec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
    for (a, b) in recorded.iter().zip(replayed) {
        let mut d = Vec::new();
        if a.slot != b.slot || a.bot_gen != b.bot_gen {
            d.push(format!(
                "bot {}/{} instead of {}/{}",
                b.slot, b.bot_gen, a.slot, a.bot_gen
            ));
        }
        if a.buttons != b.buttons {
            d.push(format!("buttons {:#06x} instead of {:#06x}", b.buttons, a.buttons));
        }
        if bits(a.view_angles) != bits(b.view_angles) {
            let (va, vb) = (a.view_angles, b.view_angles);
            d.push(format!(
                "view ({} {} {}) instead of ({} {} {})",
                vb.x, vb.y, vb.z, va.x, va.y, va.z
            ));
        }
        let moves = |c: &LbBotCommand| [c.forwardmove.to_bits(), c.sidemove.to_bits(), c.upmove.to_bits()];
        if moves(a) != moves(b) {
            d.push(format!(
                "move ({} {} {}) instead of ({} {} {})",
                b.forwardmove, b.sidemove, b.upmove, a.forwardmove, a.sidemove, a.upmove
            ));
        }
        if (a.msec, a.impulse, a.flags, a.random_seed) != (b.msec, b.impulse, b.flags, b.random_seed) {
            d.push(format!(
                "msec/impulse/flags/seed {}/{}/{}/{} instead of {}/{}/{}/{}",
                b.msec, b.impulse, b.flags, b.random_seed, a.msec, a.impulse, a.flags, a.random_seed
            ));
        }
        if !d.is_empty() {
            out.push(format!("slot {}: {}", a.slot, d.join(", ")));
        }
    }
    out.join("; ")
}

/// A frame input copied out of the adapter's buffers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameRec {
    header: Vec<u8>,
    clients: Vec<u8>,
    selves: Vec<u8>,
    events: Vec<u8>,
    event_count: u32,
    dropped: u32,
    first_seq: u32,
}

impl FrameRec {
    /// # Safety
    /// The pointers in `input` must be valid for the counts and lengths it declares, as inside `lb_core_frame_pre`.
    pub unsafe fn capture(input: &LbFrameInput) -> FrameRec {
        // SAFETY: guaranteed by the caller.
        let slice = |ptr: *const u8, len: usize| unsafe {
            if ptr.is_null() || len == 0 {
                Vec::new()
            } else {
                core::slice::from_raw_parts(ptr, len).to_vec()
            }
        };
        FrameRec {
            header: pod_bytes(std::slice::from_ref(&input.header)),
            clients: slice(
                input.clients.cast(),
                input.client_count as usize * core::mem::size_of::<LbClientSnapshot>(),
            ),
            selves: slice(
                input.selves.cast(),
                input.self_count as usize * core::mem::size_of::<LbSelfSnapshot>(),
            ),
            events: slice(input.events.data, input.events.len as usize),
            event_count: input.events.count,
            dropped: input.events.dropped,
            first_seq: input.events.first_seq,
        }
    }

    /// A frame input put together by a test or a simulated engine.
    pub fn from_parts(
        header: LbFrameHeader,
        clients: &[LbClientSnapshot],
        selves: &[LbSelfSnapshot],
        events: &[u8],
    ) -> FrameRec {
        FrameRec {
            header: pod_bytes(std::slice::from_ref(&header)),
            clients: pod_bytes(clients),
            selves: pod_bytes(selves),
            events: events.to_vec(),
            event_count: 0,
            dropped: 0,
            first_seq: 0,
        }
    }

    pub fn header(&self) -> Option<LbFrameHeader> {
        from_pod::<LbFrameHeader>(&self.header).into_iter().next()
    }

    /// Decodes the recorded frame the way the live one was decoded.
    pub fn decode(&self, strings: &mut crate::strings::StringTable) -> Option<(lb_raw::RawFrame, u32)> {
        // SAFETY: `with_input` points the input at buffers this record owns, as long as it declares.
        self.with_input(|input| unsafe { crate::arena::decode_frame(input, strings) })
    }

    /// Calls `f` with an input pointing into this record, laid out as the adapter passes it.
    pub fn with_input<R>(&self, f: impl FnOnce(&LbFrameInput) -> R) -> Option<R> {
        let header = self.header()?;
        let clients = from_pod::<LbClientSnapshot>(&self.clients);
        let selves = from_pod::<LbSelfSnapshot>(&self.selves);
        let input = LbFrameInput {
            struct_size: core::mem::size_of::<LbFrameInput>() as u32,
            client_count: clients.len() as u32,
            header,
            clients: clients.as_ptr(),
            selves: selves.as_ptr(),
            self_count: selves.len() as u32,
            pad: 0,
            events: LbEventBatch {
                data: self.events.as_ptr(),
                len: self.events.len() as u32,
                count: self.event_count,
                dropped: self.dropped,
                first_seq: self.first_seq,
            },
        };
        Some(f(&input))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Answers derived from the question, so a replay can be checked against it.
    struct Echo;

    impl Host for Echo {
        fn server_print(&mut self, _text: &str) {}
        fn client_print(&mut self, _slot: u8, _kind: PrintKind, _text: &str) {}
        fn server_command(&mut self, _command: &str) {}
        fn cvar_register(&mut self, specs: &[CvarSpec]) -> Vec<Option<CvarHandle>> {
            (0..specs.len() as u16).map(|i| Some(CvarHandle(i))).collect()
        }
        fn cvar_find(&mut self, _name: &str) -> Option<CvarHandle> {
            None
        }
        fn cvar_float(&mut self, handle: CvarHandle) -> f32 {
            f32::from(handle.0)
        }
        fn cvar_string(&mut self, handle: CvarHandle) -> String {
            handle.0.to_string()
        }
        fn cvar_set(&mut self, _handle: CvarHandle, _value: &str) {}
        fn trace(&mut self, req: &TraceRequest) -> TraceResult {
            TraceResult {
                fraction: 0.5,
                end_pos: (req.start + req.end) * 0.5,
                ..TraceResult::default()
            }
        }
        fn point_contents(&mut self, point: Vec3) -> i32 {
            -(point.z as i32)
        }
        fn set_track_rules(&mut self, _rules: &[TrackRule]) -> bool {
            true
        }
        fn snapshot_entities(&mut self, _kind_mask: u32, out: &mut Vec<EntitySnapshot>) {
            out.clear();
        }
        fn get_entity(&mut self, _ent: LbEntRef) -> Option<EntitySnapshot> {
            None
        }
        fn set_capture_mask(&mut self, _mask: &[u8; 32]) -> bool {
            true
        }
        fn resolve_user_msg(&mut self, _name: &str) -> Option<(i32, i32)> {
            Some((77, -1))
        }
        fn create_bot(&mut self, _req: &CreateBotRequest) -> CreateBotOutcome {
            CreateBotOutcome::ServerFull
        }
        fn kick_bot(&mut self, _slot: u8, _bot_gen: u32, _reason: &str) -> bool {
            true
        }
        fn bot_client_command(&mut self, _slot: u8, _bot_gen: u32, _argv: &[&str]) -> bool {
            true
        }
        fn run_player_moves(&mut self, cmds: &[LbBotCommand], feedback: &mut Vec<LbMoveFeedback>) -> bool {
            feedback.clear();
            for c in cmds {
                // SAFETY: all-zero bytes are a valid `LbMoveFeedback`.
                let mut fb: LbMoveFeedback = unsafe { core::mem::zeroed() };
                fb.slot = c.slot;
                fb.health = 100.0;
                feedback.push(fb);
            }
            true
        }
        fn physics_key(&mut self, _slot: u8, _key: &str) -> String {
            "1".into()
        }
        fn client_info_key(&mut self, _slot: u8, _key: &str) -> String {
            String::new()
        }
        fn player_stats(&mut self, _slot: u8) -> Option<(i32, i32)> {
            None
        }
        fn load_file(&mut self, _path: &str) -> Option<Vec<u8>> {
            None
        }
        fn send_debug(&mut self, _slot: u8, _prims: &[DebugPrim]) -> bool {
            true
        }
        fn compat_facts(&mut self) -> CompatFacts {
            CompatFacts::default()
        }
    }

    fn command(slot: u8, yaw: f32) -> LbBotCommand {
        // SAFETY: all-zero bytes are a valid `LbBotCommand`.
        let mut c: LbBotCommand = unsafe { core::mem::zeroed() };
        c.slot = slot;
        c.view_angles.y = yaw;
        c.msec = 10;
        c
    }

    fn line(z: f32) -> TraceRequest {
        TraceRequest {
            start: Vec3::ZERO,
            end: Vec3::new(0.0, 0.0, z),
            kind: TraceKind::Line,
            ignore_monsters: true,
            ignore_glass: false,
            ignore: None,
        }
    }

    /// The same questions as when recorded.
    fn session(host: &mut dyn Host, yaw: f32) -> (f32, i32, bool) {
        let tr = host.trace(&line(64.0));
        let contents = host.point_contents(Vec3::new(0.0, 0.0, 3.0));
        let mut fb = Vec::new();
        let ok = host.run_player_moves(&[command(1, yaw), command(2, 0.0)], &mut fb);
        (tr.end_pos.z, contents, ok && fb.len() == 2 && fb[1].slot == 2)
    }

    #[test]
    fn replay_gives_the_recorded_answers() {
        let (mut echo, mut calls) = (Echo, Vec::new());
        let live = session(&mut RecordingHost::new(&mut echo, Some(&mut calls)), 90.0);
        assert_eq!(calls.len(), 3);
        let mut replay = ReplayHost::new(roundtrip(&calls));
        assert_eq!(session(&mut replay, 90.0), live);
        assert!(
            replay.broken.is_none() && replay.diffs.is_empty(),
            "{:?} {:?}",
            replay.broken,
            replay.diffs
        );
        assert_eq!(replay.commands_checked, 2);
    }

    #[test]
    fn replay_reports_other_decisions_and_other_questions() {
        let (mut echo, mut calls) = (Echo, Vec::new());
        session(&mut RecordingHost::new(&mut echo, Some(&mut calls)), 90.0);

        let mut replay = ReplayHost::new(calls.clone());
        session(&mut replay, 91.0);
        assert!(replay.broken.is_none());
        assert_eq!(replay.diffs.len(), 1);
        assert!(replay.diffs[0].starts_with("slot 1: view"), "{}", replay.diffs[0]);

        let mut replay = ReplayHost::new(calls);
        replay.trace(&line(32.0));
        assert!(replay.broken.as_deref().is_some_and(|b| b.starts_with("trace:")));
        replay.point_contents(Vec3::ZERO);
        assert!(
            replay.broken.as_deref().is_some_and(|b| b.starts_with("trace:")),
            "the first break is kept"
        );
    }

    #[test]
    fn a_frame_input_survives_the_copy() {
        // SAFETY: all-zero bytes are valid for these records.
        let (mut header, mut client): (LbFrameHeader, LbClientSnapshot) = unsafe { core::mem::zeroed() };
        header.frame_no = 42;
        client.slot = 3;
        client.origin.x = 12.5;
        let events = [7u8; 24];
        let input = LbFrameInput {
            struct_size: 0,
            client_count: 1,
            header,
            clients: &client,
            selves: core::ptr::null(),
            self_count: 0,
            pad: 0,
            events: LbEventBatch {
                data: events.as_ptr(),
                len: events.len() as u32,
                count: 1,
                dropped: 2,
                first_seq: 3,
            },
        };
        // SAFETY: the pointers above are valid for the declared counts.
        let rec = unsafe { FrameRec::capture(&input) };
        let seen = rec
            .with_input(|i| {
                // SAFETY: `with_input` points the input at owned, correctly typed buffers.
                let c = unsafe { &*i.clients };
                // SAFETY: as above.
                let e = unsafe { core::slice::from_raw_parts(i.events.data, i.events.len as usize) };
                (
                    i.header.frame_no,
                    i.client_count,
                    c.slot,
                    c.origin.x,
                    e.to_vec(),
                    i.events.dropped,
                )
            })
            .unwrap();
        assert_eq!(seen, (42, 1, 3, 12.5, events.to_vec(), 2));
    }

    fn roundtrip(calls: &[HostCall]) -> Vec<HostCall> {
        let bytes = postcard::to_allocvec(calls).unwrap();
        postcard::from_bytes(&bytes).unwrap()
    }
}
