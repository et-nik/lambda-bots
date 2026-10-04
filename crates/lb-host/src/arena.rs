//! Decoding of the adapter's per-frame input: snapshots and the event arena.

use lb_core::Vec3;
use lb_core::handles::{EntityRef, MapEpoch};
use lb_core::msg::{MsgArg, UserMsg};
use lb_core::time::SimTime;
use lb_ffi::*;
use lb_raw::*;
use smallvec::SmallVec;

use crate::strings::StringTable;

fn v(p: LbVec3) -> Vec3 {
    Vec3::new(p.x, p.y, p.z)
}

fn read<T: Copy>(bytes: &[u8], offset: usize) -> Option<T> {
    let size = core::mem::size_of::<T>();
    if offset.checked_add(size)? > bytes.len() {
        return None;
    }
    // SAFETY: bounds checked above; T is a plain-old-data ABI struct and the read is unaligned.
    Some(unsafe { core::ptr::read_unaligned(bytes.as_ptr().add(offset) as *const T) })
}

fn take(bytes: &[u8], offset: &mut usize, len: usize) -> Option<Vec<u8>> {
    let end = offset.checked_add(len)?;
    let out = bytes.get(*offset..end)?.to_vec();
    *offset = end;
    Some(out)
}

pub fn ent_ref(epoch: MapEpoch, e: LbEntRef) -> EntityRef {
    EntityRef {
        epoch,
        index: e.index,
        serial: e.serial,
    }
}

/// Decodes one frame input. Unknown or malformed records are skipped and counted.
///
/// # Safety
/// All pointers in `input` must be valid for the lengths/counts it declares for the duration of
/// the call (guaranteed by the adapter inside `lb_core_frame_*`).
pub unsafe fn decode_frame(input: &LbFrameInput, strings: &mut StringTable) -> (RawFrame, u32) {
    let h = &input.header;
    let epoch = MapEpoch(h.map_epoch);
    let header = FrameHeader {
        epoch,
        frame_no: h.frame_no,
        sim_time: SimTime(h.sim_time),
        frame_time: h.frame_time,
        mono_ns: h.mono_ns,
        engine_time: h.engine_time,
        pre: h.flags & LB_FRAME_PRE != 0,
        paused: h.flags & LB_FRAME_PAUSED != 0,
        first: h.flags & LB_FRAME_FIRST != 0,
        max_clients: h.max_clients,
        num_edicts: h.num_edicts,
    };
    let clients = if input.clients.is_null() || input.client_count == 0 {
        Vec::new()
    } else {
        // SAFETY: guaranteed by the caller.
        unsafe { core::slice::from_raw_parts(input.clients, input.client_count as usize) }
            .iter()
            .map(decode_client)
            .collect()
    };
    let selves = if input.selves.is_null() || input.self_count == 0 {
        Vec::new()
    } else {
        // SAFETY: guaranteed by the caller.
        unsafe { core::slice::from_raw_parts(input.selves, input.self_count as usize) }
            .iter()
            .map(decode_self)
            .collect()
    };
    let arena = if input.events.data.is_null() || input.events.len == 0 {
        &[][..]
    } else {
        // SAFETY: guaranteed by the caller.
        unsafe { core::slice::from_raw_parts(input.events.data, input.events.len as usize) }
    };
    let (events, malformed) = decode_events(arena, epoch, strings);
    (
        RawFrame {
            header,
            clients,
            selves,
            events,
            dropped_events: input.events.dropped,
        },
        malformed,
    )
}

fn decode_client(c: &LbClientSnapshot) -> RawClient {
    RawClient {
        slot: c.slot,
        state: match c.state {
            LB_CLIENT_CONNECTING => ClientState::Connecting,
            LB_CLIENT_CONNECTED => ClientState::Connected,
            LB_CLIENT_SPAWNED => ClientState::Spawned,
            _ => ClientState::Free,
        },
        is_fake: c.is_fake != 0,
        is_ours: c.is_ours != 0,
        userid: c.userid,
        origin: v(c.origin),
        velocity: v(c.velocity),
        angles: v(c.angles),
        view_ofs: v(c.view_ofs),
        mins: v(c.mins),
        maxs: v(c.maxs),
        flags: c.flags,
        effects: c.effects,
        movetype: c.movetype,
        solid: c.solid,
        deadflag: c.deadflag,
        waterlevel: c.waterlevel,
        rendermode: c.rendermode,
        renderfx: c.renderfx,
        renderamt: c.renderamt,
        rendercolor: v(c.rendercolor),
        step_left: c.step_left,
        frame: c.frame,
        frags: c.frags,
        sequence: c.sequence,
        gaitsequence: c.gaitsequence,
        weaponmodel: c.weaponmodel_id,
        model: c.model_id,
        ping_ms: c.ping_ms,
    }
}

fn decode_self(s: &LbSelfSnapshot) -> RawSelf {
    RawSelf {
        slot: s.slot,
        bot_gen: s.bot_gen,
        deadflag: s.deadflag,
        movetype: s.movetype,
        waterlevel: s.waterlevel,
        watertype: s.watertype,
        health: s.health,
        armor: s.armor,
        weapons_mask: s.weapons_mask,
        maxspeed: s.maxspeed,
        fov: s.fov,
        duck_time: s.duck_time,
        fall_velocity: s.fall_velocity,
        flags: s.flags,
        origin: v(s.origin),
        velocity: v(s.velocity),
        v_angle: v(s.v_angle),
        angles: v(s.angles),
        punchangle: v(s.punchangle),
        view_ofs: v(s.view_ofs),
        basevelocity: v(s.basevelocity),
        in_duck: s.in_duck != 0,
        has_longjump: s.has_longjump != 0,
        fixangle: s.fixangle,
        groundentity: s.groundentity,
        buttons_applied: s.buttons_applied,
        frags: s.frags,
    }
}

pub fn decode_events(arena: &[u8], epoch: MapEpoch, strings: &mut StringTable) -> (Vec<StampedEvent>, u32) {
    let hsize = core::mem::size_of::<LbEventHeader>();
    let mut out = Vec::new();
    let mut malformed = 0u32;
    let mut offset = 0usize;
    while offset + hsize <= arena.len() {
        let Some(h) = read::<LbEventHeader>(arena, offset) else {
            break;
        };
        let size = h.size as usize;
        if size < hsize || offset + size > arena.len() {
            malformed += 1;
            break;
        }
        let payload = &arena[offset + hsize..offset + size];
        offset += size;
        let decoded: Option<Option<RawEvent>> = match h.kind {
            LB_EV_CLIENT => decode_client_event(payload).map(Some),
            LB_EV_USER_MSG => decode_user_msg(payload).map(|m| Some(RawEvent::UserMsg(m))),
            LB_EV_SOUND => decode_sound(payload, epoch).map(Some),
            LB_EV_PLAYBACK => read::<LbEvPlayback>(payload, 0).map(|p| {
                Some(RawEvent::Playback(RawPlayback {
                    flags: p.flags,
                    event_index: p.event_index,
                    invoker: ent_ref(epoch, p.invoker),
                    origin: v(p.origin),
                    angles: v(p.angles),
                    invoker_origin: v(p.invoker_origin),
                    fparam1: p.fparam1,
                    fparam2: p.fparam2,
                    iparam1: p.iparam1,
                    iparam2: p.iparam2,
                    bparam1: p.bparam1,
                    bparam2: p.bparam2,
                }))
            }),
            LB_EV_ENTITY => read::<LbEvEntity>(payload, 0).map(|e| {
                Some(RawEvent::Entity(EntityEvent {
                    kind: if e.what == LB_ENTITY_EV_FREE {
                        EntityEventKind::Free
                    } else {
                        EntityEventKind::Spawn
                    },
                    entity: ent_ref(epoch, e.ent),
                    track_kind: e.kind,
                    classname: e.classname_id,
                    origin: v(e.origin),
                }))
            }),
            LB_EV_CLIENT_CMD => decode_command(payload).map(|c| Some(RawEvent::ClientCommand(c))),
            LB_EV_SERVER_CMD => decode_command(payload).map(|c| Some(RawEvent::ServerCommand(c))),
            LB_EV_FIXANGLE => read::<LbEvFixangle>(payload, 0).map(|f| {
                Some(RawEvent::Fixangle {
                    slot: f.slot,
                    bot_gen: f.bot_gen,
                    mode: f.mode,
                    angles: v(f.angles),
                })
            }),
            LB_EV_STRING => decode_named(payload).map(|(id, _, bytes)| {
                strings.set_string(id as u16, bytes);
                None
            }),
            LB_EV_REG_MSG => decode_named(payload).map(|(id, _, bytes)| {
                strings.set_msg_name(id, bytes.clone());
                Some(RawEvent::RegisterMsg { id, name: bytes })
            }),
            LB_EV_PRECACHE_EVENT => decode_named(payload).map(|(id, _, bytes)| {
                strings.set_event_name(id, bytes.clone());
                Some(RawEvent::PrecacheEvent { index: id, name: bytes })
            }),
            LB_EV_LOG => decode_named(payload).map(|(id, _, bytes)| Some(RawEvent::Log { level: id, text: bytes })),
            LB_EV_OVERFLOW => read::<LbEvOverflow>(payload, 0).map(|o| {
                Some(RawEvent::Overflow {
                    dropped_records: o.dropped_records,
                    dropped_bytes: o.dropped_bytes,
                })
            }),
            _ => None,
        };
        match decoded {
            Some(Some(event)) => out.push(StampedEvent {
                seq: h.seq,
                ctx: h.ctx,
                sim_time: SimTime(h.sim_time),
                event,
            }),
            Some(None) => {}
            None => malformed += 1,
        }
    }
    (out, malformed)
}

fn decode_named(payload: &[u8]) -> Option<(i32, u16, Vec<u8>)> {
    let n = read::<LbEvNamed>(payload, 0)?;
    let mut off = core::mem::size_of::<LbEvNamed>();
    let bytes = take(payload, &mut off, n.len as usize)?;
    Some((n.id, n.extra, bytes))
}

fn decode_client_event(payload: &[u8]) -> Option<RawEvent> {
    let c = read::<LbEvClient>(payload, 0)?;
    let mut off = core::mem::size_of::<LbEvClient>();
    let name = take(payload, &mut off, c.name_len as usize)?;
    let model = take(payload, &mut off, c.model_len as usize)?;
    let address = take(payload, &mut off, c.addr_len as usize)?;
    let auth_id = take(payload, &mut off, c.auth_id_len as usize)?;
    let kind = match c.what {
        LB_CLIENT_EV_CONNECT => ClientEventKind::Connect,
        LB_CLIENT_EV_CONNECT_REJECTED => ClientEventKind::ConnectRejected,
        LB_CLIENT_EV_PUT_IN_SERVER => ClientEventKind::PutInServer,
        LB_CLIENT_EV_DISCONNECT => ClientEventKind::Disconnect,
        LB_CLIENT_EV_INFO => ClientEventKind::Info,
        _ => return None,
    };
    Some(RawEvent::Client(ClientEvent {
        kind,
        slot: c.slot,
        userid: c.userid,
        is_ours: c.is_ours != 0,
        is_fake: c.is_fake != 0,
        bot_gen: c.bot_gen,
        name,
        model,
        address,
        auth_id,
        topcolor: c.topcolor,
        bottomcolor: c.bottomcolor,
    }))
}

fn decode_user_msg(payload: &[u8]) -> Option<UserMsg> {
    let m = read::<LbEvUserMsg>(payload, 0)?;
    let args_off = core::mem::size_of::<LbEvUserMsg>();
    let arg_size = core::mem::size_of::<LbMsgArg>();
    let strings_off = args_off + m.argc as usize * arg_size;
    let strings = payload.get(strings_off..strings_off + m.strings_len as usize)?;
    let mut args: SmallVec<[MsgArg; 8]> = SmallVec::new();
    for i in 0..m.argc as usize {
        let a = read::<LbMsgArg>(payload, args_off + i * arg_size)?;
        args.push(match a.tag {
            LB_MSG_ARG_BYTE => MsgArg::Byte(a.ival),
            LB_MSG_ARG_CHAR => MsgArg::Char(a.ival),
            LB_MSG_ARG_SHORT => MsgArg::Short(a.ival),
            LB_MSG_ARG_LONG => MsgArg::Long(a.ival),
            LB_MSG_ARG_ANGLE => MsgArg::Angle(a.fval),
            LB_MSG_ARG_COORD => MsgArg::Coord(a.fval),
            LB_MSG_ARG_ENTITY => MsgArg::Entity(a.ival),
            LB_MSG_ARG_STRING => {
                let start = a.str_off as usize;
                MsgArg::String(strings.get(start..start + a.str_len as usize)?.to_vec())
            }
            _ => return None,
        });
    }
    Some(UserMsg {
        msg_id: m.msg_id,
        dest: m.dest,
        target_slot: m.target_slot,
        origin: (m.flags & LB_MSG_FLAG_HAS_ORIGIN != 0).then(|| v(m.origin)),
        truncated: m.flags & LB_MSG_FLAG_TRUNCATED != 0,
        from_msg_manager: m.flags & LB_MSG_FLAG_FROM_MSGMGR != 0,
        args,
    })
}

fn decode_sound(payload: &[u8], epoch: MapEpoch) -> Option<RawEvent> {
    let s = read::<LbEvSound>(payload, 0)?;
    let mut off = core::mem::size_of::<LbEvSound>();
    let sample = take(payload, &mut off, s.sample_len as usize)?;
    Some(RawEvent::Sound(RawSound {
        source: match s.source {
            LB_SOUND_SRC_AMBIENT => SoundSource::Ambient,
            LB_SOUND_SRC_REHLDS => SoundSource::Rehlds,
            _ => SoundSource::Emit,
        },
        entity: ent_ref(epoch, s.entity),
        channel: s.channel,
        sample,
        origin: v(s.origin),
        volume: s.volume,
        attenuation: s.attenuation,
        flags: s.flags,
        pitch: s.pitch,
    }))
}

fn decode_command(payload: &[u8]) -> Option<CommandEvent> {
    let c = read::<LbEvCommand>(payload, 0)?;
    let mut off = core::mem::size_of::<LbEvCommand>();
    let mut argv = Vec::with_capacity(c.argc as usize);
    for _ in 0..c.argc {
        let len: u16 = read(payload, off)?;
        off += 2;
        argv.push(take(payload, &mut off, len as usize)?);
    }
    let line = take(payload, &mut off, c.line_len as usize)?;
    Some(CommandEvent {
        slot: c.slot,
        userid: c.userid,
        argv,
        line,
    })
}

/// Encoder used by tests and test hosts to build arenas exactly like the adapter does.
pub mod encode {
    use super::*;

    pub struct ArenaWriter {
        pub bytes: Vec<u8>,
        pub count: u32,
        seq: u32,
    }

    impl Default for ArenaWriter {
        fn default() -> Self {
            ArenaWriter {
                bytes: Vec::new(),
                count: 0,
                seq: 1,
            }
        }
    }

    fn pod_bytes<T: Copy>(v: &T) -> &[u8] {
        // SAFETY: T is a plain-old-data ABI struct; reading its bytes is always valid.
        unsafe { core::slice::from_raw_parts(v as *const T as *const u8, core::mem::size_of::<T>()) }
    }

    impl ArenaWriter {
        pub fn record(&mut self, kind: u16, ctx: u32, sim_time: f64, parts: &[&[u8]]) {
            let body: usize = parts.iter().map(|p| p.len()).sum();
            let hsize = core::mem::size_of::<LbEventHeader>();
            let size = (hsize + body).div_ceil(8) * 8;
            let header = LbEventHeader {
                kind,
                size: size as u16,
                seq: self.seq,
                ctx,
                frame_no_low: 0,
                sim_time,
            };
            self.seq += 1;
            self.count += 1;
            self.bytes.extend_from_slice(pod_bytes(&header));
            for p in parts {
                self.bytes.extend_from_slice(p);
            }
            self.bytes.resize(self.bytes.len() + (size - hsize - body), 0);
        }

        pub fn named(&mut self, kind: u16, id: i32, text: &[u8]) {
            let n = LbEvNamed {
                id,
                len: text.len() as u16,
                extra: 0,
            };
            self.record(kind, LB_CTX_FRAME, 0.0, &[pod_bytes(&n), text]);
        }

        pub fn user_msg(&mut self, msg_id: i32, dest: u8, target_slot: u8, args: &[MsgArg]) {
            let mut strings = Vec::new();
            let mut ffi_args = Vec::new();
            for a in args {
                let mut arg = LbMsgArg {
                    tag: 0,
                    pad: [0; 3],
                    ival: 0,
                    fval: 0.0,
                    str_off: 0,
                    str_len: 0,
                };
                match a {
                    MsgArg::Byte(x) => (arg.tag, arg.ival) = (LB_MSG_ARG_BYTE, *x),
                    MsgArg::Char(x) => (arg.tag, arg.ival) = (LB_MSG_ARG_CHAR, *x),
                    MsgArg::Short(x) => (arg.tag, arg.ival) = (LB_MSG_ARG_SHORT, *x),
                    MsgArg::Long(x) => (arg.tag, arg.ival) = (LB_MSG_ARG_LONG, *x),
                    MsgArg::Entity(x) => (arg.tag, arg.ival) = (LB_MSG_ARG_ENTITY, *x),
                    MsgArg::Angle(x) => (arg.tag, arg.fval) = (LB_MSG_ARG_ANGLE, *x),
                    MsgArg::Coord(x) => (arg.tag, arg.fval) = (LB_MSG_ARG_COORD, *x),
                    MsgArg::String(s) => {
                        arg.tag = LB_MSG_ARG_STRING;
                        arg.str_off = strings.len() as u16;
                        arg.str_len = s.len() as u16;
                        strings.extend_from_slice(s);
                    }
                }
                ffi_args.push(arg);
            }
            let m = LbEvUserMsg {
                msg_id,
                dest,
                target_slot,
                flags: 0,
                argc: ffi_args.len() as u8,
                origin: LbVec3::default(),
                strings_len: strings.len() as u16,
                pad: 0,
            };
            let mut args_bytes = Vec::new();
            for a in &ffi_args {
                args_bytes.extend_from_slice(pod_bytes(a));
            }
            self.record(
                LB_EV_USER_MSG,
                LB_CTX_FRAME,
                0.0,
                &[pod_bytes(&m), &args_bytes, &strings],
            );
        }

        pub fn client(&mut self, what: u8, slot: u8, userid: i32, is_ours: bool, name: &[u8]) {
            self.client_gen(what, slot, userid, is_ours, 0, name);
        }

        /// A client event of one of our bots of generation `bot_gen` (or anyone's, with `is_ours` false).
        pub fn client_gen(&mut self, what: u8, slot: u8, userid: i32, is_ours: bool, bot_gen: u32, name: &[u8]) {
            let c = LbEvClient {
                what,
                slot,
                is_ours: is_ours as u8,
                is_fake: is_ours as u8,
                userid,
                bot_gen,
                topcolor: 0,
                bottomcolor: 0,
                name_len: name.len() as u8,
                model_len: 0,
                addr_len: 0,
                pad: [0; 3],
                auth_id_len: 0,
                pad2: [0; 3],
            };
            self.record(LB_EV_CLIENT, LB_CTX_FRAME, 0.0, &[pod_bytes(&c), name]);
        }

        /// A client's command as the adapter's `ClientCommand` hook passes it: the words and the arguments' line.
        pub fn command(&mut self, slot: u8, userid: i32, argv: &[&[u8]], line: &[u8]) {
            let c = LbEvCommand {
                slot,
                argc: argv.len() as u8,
                line_len: line.len() as u16,
                userid,
            };
            let mut words = Vec::new();
            for a in argv {
                words.extend_from_slice(&(a.len() as u16).to_ne_bytes());
                words.extend_from_slice(a);
            }
            self.record(LB_EV_CLIENT_CMD, LB_CTX_FRAME, 0.0, &[pod_bytes(&c), &words, line]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::encode::ArenaWriter;
    use super::*;

    #[test]
    fn roundtrip_client_commands() {
        let mut w = ArenaWriter::default();
        w.command(7, 3, &[b"say", "привет всем".as_bytes()], "\"привет всем\"".as_bytes());
        let (events, malformed) = decode_events(&w.bytes, MapEpoch(1), &mut StringTable::default());
        assert_eq!(malformed, 0);
        let RawEvent::ClientCommand(c) = &events[0].event else {
            panic!("{events:?}");
        };
        assert_eq!((c.slot, c.userid), (7, 3));
        assert_eq!(c.argv, vec![b"say".to_vec(), "привет всем".as_bytes().to_vec()]);
        assert_eq!(c.line, "\"привет всем\"".as_bytes());
    }

    #[test]
    fn roundtrip_user_message_and_names() {
        let mut w = ArenaWriter::default();
        w.named(LB_EV_REG_MSG, 71, b"CurWeapon");
        w.user_msg(
            71,
            lb_core::msg::MSG_ONE,
            3,
            &[MsgArg::Byte(1), MsgArg::Byte(2), MsgArg::Byte(17)],
        );
        w.user_msg(
            80,
            lb_core::msg::MSG_ALL,
            0,
            &[MsgArg::Byte(1), MsgArg::String(b"9mmhandgun".to_vec())],
        );
        w.client(LB_CLIENT_EV_DISCONNECT, 5, 12, true, b"Gordon");
        let mut strings = StringTable::default();
        let (events, malformed) = decode_events(&w.bytes, MapEpoch(2), &mut strings);
        assert_eq!(malformed, 0);
        assert_eq!(events.len(), 4);
        assert_eq!(strings.msg_name(71), Some(&b"CurWeapon"[..]));
        match &events[1].event {
            RawEvent::UserMsg(m) => {
                assert_eq!(m.msg_id, 71);
                assert_eq!(m.target_slot, 3);
                assert_eq!(m.int(2), Some(17));
            }
            other => panic!("unexpected {other:?}"),
        }
        match &events[2].event {
            RawEvent::UserMsg(m) => assert_eq!(m.string(1), Some(&b"9mmhandgun"[..])),
            other => panic!("unexpected {other:?}"),
        }
        match &events[3].event {
            RawEvent::Client(c) => {
                assert_eq!(c.kind, ClientEventKind::Disconnect);
                assert_eq!(c.name, b"Gordon");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn truncated_arena_is_reported() {
        let mut w = ArenaWriter::default();
        w.named(LB_EV_LOG, 1, b"hello");
        let cut = &w.bytes[..w.bytes.len() - 8];
        let mut strings = StringTable::default();
        let (events, malformed) = decode_events(cut, MapEpoch(1), &mut strings);
        assert!(events.is_empty());
        assert_eq!(malformed, 1);
    }
}
