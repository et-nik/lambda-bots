//! `Host` implemented through the adapter's `LbHostApi` function table.

use lb_core::Vec3;
use lb_ffi::*;

use crate::host::*;

pub struct FfiHost {
    api: LbHostApi,
}

fn v(p: Vec3) -> LbVec3 {
    LbVec3 { x: p.x, y: p.y, z: p.z }
}

fn from_v(p: LbVec3) -> Vec3 {
    Vec3::new(p.x, p.y, p.z)
}

fn s(text: &str) -> LbStr {
    LbStr::from_bytes(text.as_bytes())
}

fn fixed_str(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

impl FfiHost {
    /// # Safety
    /// `api` must point to a valid host table whose function pointers stay valid while this value
    /// is used, and calls must happen on the engine main thread.
    pub unsafe fn new(api: *const LbHostApi) -> Option<FfiHost> {
        if api.is_null() {
            return None;
        }
        // SAFETY: non-null pointer to a table provided by the adapter.
        let api = unsafe { *api };
        if api.abi_version != LB_ABI_VERSION || api.struct_size as usize != core::mem::size_of::<LbHostApi>() {
            return None;
        }
        Some(FfiHost { api })
    }
}

macro_rules! call {
    ($self:ident . $f:ident ( $($arg:expr),* ) else $default:expr) => {
        match $self.api.$f {
            // SAFETY: the adapter guarantees the pointer is valid on the main thread during core calls.
            Some(f) => unsafe { f($self.api.ctx, $($arg),*) },
            None => $default,
        }
    };
}

impl Host for FfiHost {
    fn server_print(&mut self, text: &str) {
        call!(self.server_print(s(text)) else ())
    }

    fn client_print(&mut self, slot: u8, kind: PrintKind, text: &str) {
        let where_ = match kind {
            PrintKind::Console => LB_PRINT_CONSOLE,
            PrintKind::Center => LB_PRINT_CENTER,
            PrintKind::Chat => LB_PRINT_CHAT,
        };
        call!(self.client_print(slot, where_, s(text)) else ())
    }

    fn server_command(&mut self, command: &str) {
        call!(self.server_command(s(command)) else ())
    }

    fn cvar_register(&mut self, specs: &[CvarSpec]) -> Vec<Option<CvarHandle>> {
        let ffi: Vec<LbCvarSpec> = specs
            .iter()
            .map(|c| LbCvarSpec {
                name: s(&c.name),
                default_value: s(&c.default_value),
                flags: c.flags,
                pad: 0,
            })
            .collect();
        let mut out = vec![0u16; specs.len()];
        let status =
            call!(self.cvar_register(ffi.as_ptr(), ffi.len() as u32, out.as_mut_ptr()) else LB_ERR_UNSUPPORTED);
        if status != LB_OK {
            return vec![None; specs.len()];
        }
        out.into_iter()
            .map(|h| if h == 0 { None } else { Some(CvarHandle(h)) })
            .collect()
    }

    fn cvar_find(&mut self, name: &str) -> Option<CvarHandle> {
        let h = call!(self.cvar_find(s(name)) else 0);
        if h == 0 { None } else { Some(CvarHandle(h)) }
    }

    fn cvar_float(&mut self, handle: CvarHandle) -> f32 {
        call!(self.cvar_get_float(handle.0) else 0.0)
    }

    fn cvar_string(&mut self, handle: CvarHandle) -> String {
        let mut buf = [0u8; 512];
        let n = call!(self.cvar_get_string(handle.0, buf.as_mut_ptr(), buf.len() as u32) else 0) as usize;
        String::from_utf8_lossy(&buf[..n.min(buf.len())]).into_owned()
    }

    fn cvar_set(&mut self, handle: CvarHandle, value: &str) {
        call!(self.cvar_set(handle.0, s(value)) else ())
    }

    fn trace(&mut self, req: &TraceRequest) -> TraceResult {
        let (kind, hull, model) = match req.kind {
            TraceKind::Line => (LB_TRACE_LINE, 0, LbEntRef::default()),
            TraceKind::Hull(h) => (LB_TRACE_HULL, h, LbEntRef::default()),
            TraceKind::Model(m) => (LB_TRACE_MODEL, 0, m),
        };
        let mut flags = 0u16;
        if req.ignore_monsters {
            flags |= LB_TRACE_IGNORE_MONSTERS;
        }
        if req.ignore_glass {
            flags |= LB_TRACE_IGNORE_GLASS;
        }
        let ffi = LbTraceRequest {
            start: v(req.start),
            end: v(req.end),
            kind,
            hull,
            flags,
            ignore: req.ignore.unwrap_or_default(),
            model,
            pad: 0,
        };
        let mut out = LbTraceResult {
            fraction: 1.0,
            end_pos: v(req.end),
            ..Default::default()
        };
        call!(self.trace(&ffi, &mut out) else ());
        TraceResult {
            fraction: out.fraction,
            end_pos: from_v(out.end_pos),
            plane_normal: from_v(out.plane_normal),
            plane_dist: out.plane_dist,
            hit: out.hit,
            hitgroup: out.hitgroup,
            all_solid: out.all_solid != 0,
            start_solid: out.start_solid != 0,
            in_open: out.in_open != 0,
            in_water: out.in_water != 0,
            hit_rendermode: out.hit_rendermode,
            hit_renderfx: out.hit_renderfx,
            hit_renderamt: out.hit_renderamt,
            hit_solid: out.hit_solid,
            hit_classname: out.hit_classname_id,
        }
    }

    fn point_contents(&mut self, point: Vec3) -> i32 {
        call!(self.point_contents(v(point)) else 0)
    }

    fn set_track_rules(&mut self, rules: &[TrackRule]) -> bool {
        let ffi: Vec<LbTrackRule> = rules
            .iter()
            .map(|r| LbTrackRule {
                pattern: s(&r.pattern),
                match_mode: if r.prefix { LB_TRACK_PREFIX } else { LB_TRACK_EXACT },
                kind: r.kind,
                pad: [0; 6],
            })
            .collect();
        call!(self.registry_set_rules(ffi.as_ptr(), ffi.len() as u32) else LB_ERR_UNSUPPORTED) == LB_OK
    }

    fn snapshot_entities(&mut self, kind_mask: u32, out: &mut Vec<EntitySnapshot>) {
        out.clear();
        let cap = 2048usize;
        out.reserve(cap);
        let n = call!(self.snapshot_entities(kind_mask, out.as_mut_ptr(), cap as u32) else 0) as usize;
        // SAFETY: the adapter wrote `n <= cap` initialized snapshots into the reserved buffer.
        unsafe { out.set_len(n.min(cap)) };
    }

    fn get_entity(&mut self, ent: LbEntRef) -> Option<EntitySnapshot> {
        let mut out = core::mem::MaybeUninit::<LbEntitySnapshot>::zeroed();
        let status = call!(self.get_entity(ent, out.as_mut_ptr()) else LB_ERR_UNSUPPORTED);
        // SAFETY: zero-initialized POD, filled by the adapter on success.
        if status == LB_OK {
            Some(unsafe { out.assume_init() })
        } else {
            None
        }
    }

    fn set_capture_mask(&mut self, mask: &[u8; 32]) -> bool {
        call!(self.set_capture_mask(mask.as_ptr()) else LB_ERR_UNSUPPORTED) == LB_OK
    }

    fn resolve_user_msg(&mut self, name: &str) -> Option<(i32, i32)> {
        let mut size = 0i32;
        let id = call!(self.resolve_user_msg(s(name), &mut size) else 0);
        if id > 0 { Some((id, size)) } else { None }
    }

    fn create_bot(&mut self, req: &CreateBotRequest) -> CreateBotOutcome {
        let kvs: Vec<LbKeyValue> = req
            .infokeys
            .iter()
            .map(|(k, val)| LbKeyValue {
                key: s(k),
                value: s(val),
            })
            .collect();
        let ffi = LbCreateBotRequest {
            name: s(&req.name),
            connect_addr: LbStr::from_static("127.0.0.1"),
            infokeys: kvs.as_ptr(),
            infokey_count: kvs.len() as u32,
            flags: 0,
        };
        let mut out = LbCreateBotResult {
            status: LB_ERR_INVALID,
            slot: 0,
            pad: [0; 3],
            userid: 0,
            bot_gen: 0,
            reject_reason: [0; 128],
        };
        let status = call!(self.create_bot(&ffi, &mut out) else LB_ERR_UNSUPPORTED);
        match status {
            LB_OK => CreateBotOutcome::Created {
                slot: out.slot,
                userid: out.userid,
                bot_gen: out.bot_gen,
            },
            LB_ERR_FULL => CreateBotOutcome::ServerFull,
            LB_ERR_REJECTED => CreateBotOutcome::Rejected(fixed_str(&out.reject_reason)),
            other => CreateBotOutcome::Failed(other),
        }
    }

    fn kick_bot(&mut self, slot: u8, bot_gen: u32, reason: &str) -> bool {
        call!(self.kick_bot(slot, bot_gen, s(reason)) else LB_ERR_UNSUPPORTED) == LB_OK
    }

    fn bot_client_command(&mut self, slot: u8, bot_gen: u32, argv: &[&str]) -> bool {
        let mut cmd = LbClientCommand {
            slot,
            argc: 0,
            pad: 0,
            bot_gen,
            argv: [LbStr::EMPTY; LB_MAX_CLIENT_CMD_ARGS],
        };
        for (i, a) in argv.iter().take(LB_MAX_CLIENT_CMD_ARGS).enumerate() {
            cmd.argv[i] = s(a);
            cmd.argc += 1;
        }
        call!(self.bot_client_commands(&cmd, 1) else LB_ERR_UNSUPPORTED) == LB_OK
    }

    fn run_player_moves(&mut self, cmds: &[LbBotCommand], feedback: &mut Vec<LbMoveFeedback>) -> bool {
        feedback.clear();
        feedback.reserve(cmds.len());
        let status = call!(self.run_player_moves(cmds.as_ptr(), cmds.len() as u32, feedback.as_mut_ptr()) else LB_ERR_UNSUPPORTED);
        if status == LB_OK {
            // SAFETY: the adapter fills exactly one feedback record per command.
            unsafe { feedback.set_len(cmds.len()) };
            true
        } else {
            false
        }
    }

    fn physics_key(&mut self, slot: u8, key: &str) -> String {
        let mut buf = [0u8; 256];
        let n = call!(self.get_physics_key(slot, s(key), buf.as_mut_ptr(), buf.len() as u32) else 0) as usize;
        String::from_utf8_lossy(&buf[..n.min(buf.len())]).into_owned()
    }

    fn client_info_key(&mut self, slot: u8, key: &str) -> String {
        let mut buf = [0u8; 256];
        let n = call!(self.get_client_info_key(slot, s(key), buf.as_mut_ptr(), buf.len() as u32) else 0) as usize;
        String::from_utf8_lossy(&buf[..n.min(buf.len())]).into_owned()
    }

    fn player_stats(&mut self, slot: u8) -> Option<(i32, i32)> {
        let (mut ping, mut loss) = (0i32, 0i32);
        let status = call!(self.get_player_stats(slot, &mut ping, &mut loss) else LB_ERR_UNSUPPORTED);
        (status == LB_OK).then_some((ping, loss))
    }

    fn load_file(&mut self, path: &str) -> Option<Vec<u8>> {
        let mut buf = LbOwnedBuffer {
            ptr: core::ptr::null(),
            len: 0,
            pad: 0,
            token: core::ptr::null_mut(),
        };
        let status = call!(self.load_file(s(path), &mut buf) else LB_ERR_UNSUPPORTED);
        if status != LB_OK || buf.ptr.is_null() {
            return None;
        }
        // SAFETY: the adapter returned a buffer of `len` bytes that stays valid until `free_file`.
        let bytes = unsafe { core::slice::from_raw_parts(buf.ptr, buf.len as usize) }.to_vec();
        call!(self.free_file(&mut buf) else ());
        Some(bytes)
    }

    fn send_debug(&mut self, slot: u8, prims: &[DebugPrim]) -> bool {
        let ffi: Vec<LbDebugPrim> = prims
            .iter()
            .map(|p| {
                let (a, b) = p.line.unwrap_or((Vec3::ZERO, Vec3::ZERO));
                LbDebugPrim {
                    kind: if p.line.is_some() { LB_DEBUG_LINE } else { LB_DEBUG_TEXT },
                    r: p.color[0],
                    g: p.color[1],
                    b: p.color[2],
                    width: p.width,
                    life_ds: p.life_ds,
                    brightness: 255,
                    channel: p.channel,
                    a: v(a),
                    b2: v(b),
                    pad: 0,
                    text: p.text.as_deref().map(s).unwrap_or(LbStr::EMPTY),
                }
            })
            .collect();
        call!(self.send_debug(slot, ffi.as_ptr(), ffi.len() as u32) else LB_ERR_UNSUPPORTED) == LB_OK
    }

    fn compat_facts(&mut self) -> CompatFacts {
        let mut out = core::mem::MaybeUninit::<LbCompatFacts>::zeroed();
        let status = call!(self.get_compat_facts(out.as_mut_ptr()) else LB_ERR_UNSUPPORTED);
        if status != LB_OK {
            return CompatFacts::default();
        }
        // SAFETY: zero-initialized POD filled by the adapter.
        let f = unsafe { out.assume_init() };
        CompatFacts {
            engine_kind: f.engine_kind,
            metamod_has_hook_tables: f.metamod_has_hook_tables != 0,
            rehlds_version: (f.rehlds_major != 0 || f.rehlds_minor != 0).then_some((
                f.rehlds_major,
                f.rehlds_minor,
                f.rehlds_build,
            )),
            channels: f.channels,
            engine_version: fixed_str(&f.engine_version),
            metamod_version: fixed_str(&f.metamod_version),
            gamedll_desc: fixed_str(&f.gamedll_desc),
            gamedll_path: fixed_str(&f.gamedll_path),
        }
    }
}

// SAFETY: the host table is only ever called on the engine main thread inside `lb_core_*` calls;
// the plugin keeps it behind a mutex solely to satisfy the static's `Send` bound.
unsafe impl Send for FfiHost {}
