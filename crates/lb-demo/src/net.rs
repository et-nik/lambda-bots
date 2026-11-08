//! Server messages of protocol 48 as a client reads them from a demo.
//!
//! Formats follow the client side of the ReHLDS HLTV proxy (`HLTV/Core/src/Server.cpp`, `World.cpp`) and the
//! server's writers (`engine/sv_main.cpp`). Messages carry no length, so one we cannot read ends the frame.

use std::rc::Rc;

use rustc_hash::FxHashMap;

use crate::Error;
use crate::bits::Bits;
use crate::delta::{Deltas, Desc, Layout, read_field_description};
use crate::file::EventArgs;

const MAX_EDICT_BITS: u32 = 11;
const MAX_EDICTS: usize = 1 << MAX_EDICT_BITS;
const DELTA_OFFSET_BITS: u32 = 6;
const MAX_BASELINE_BITS: u32 = 6;
const ENTITY_BEAM: u32 = 1 << 1;
const RES_CUSTOM: u32 = 1 << 2;
const SND_FL_VOLUME: u32 = 1 << 0;
const SND_FL_ATTENUATION: u32 = 1 << 1;
const SND_FL_LARGE_INDEX: u32 = 1 << 2;
const SND_FL_PITCH: u32 = 1 << 3;
const SND_FL_SENTENCE: u32 = 1 << 4;
const TE_BSPDECAL: u8 = 13;
const TE_TEXTMESSAGE: u8 = 29;
const TE_LINE: u8 = 30;
const USER_MESSAGES: u8 = 64;
const ENTITY: &str = "entity_state_t";
const PLAYER: &str = "entity_state_player_t";
const CUSTOM: &str = "custom_entity_state_t";

/// Lengths of temp entities after the type byte: -1 has its own rule, -2 is not a temp entity. The HLTV proxy's
/// table (ReHLDS `HLTV/Core/src/Server.cpp`), which records as many bytes as it lists; a line is two bytes shorter
/// as the game writes it.
const TE_LENGTH: [i8; 128] = [
    24, 20, 6, 11, 6, 10, 12, 17, 16, 6, 6, 6, 8, -1, 9, 19, //
    -2, 10, 16, 24, 24, 24, 10, 11, 16, 19, -2, 12, 16, -1, 19, 17, //
    -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, //
    -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, //
    -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, //
    -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, -2, //
    -2, -2, -2, 2, 10, 14, 12, 14, 9, 5, 17, 13, 24, 9, 17, 7, //
    10, 19, 19, 12, 7, 7, 9, 16, 18, 6, 10, 13, 7, 1, 18, 15, //
];

/// One entity of a frame: its number and its `entity_state_t` bytes.
#[derive(Clone, Debug)]
pub struct Entity {
    pub number: u16,
    pub custom: bool,
    pub state: Rc<[u8]>,
}

#[derive(Clone, Debug)]
pub struct UserMsgDef {
    pub name: String,
    /// Bytes of the message, -1 when it carries its own length.
    pub size: i32,
}

#[derive(Default)]
pub struct Resources {
    pub sounds: FxHashMap<u16, String>,
    pub models: FxHashMap<u16, String>,
    pub events: FxHashMap<u16, String>,
}

#[derive(Clone, Debug)]
pub struct Sound {
    pub entity: u16,
    pub channel: u8,
    /// The sample, or `!<n>` for a sentence.
    pub sample: String,
    pub volume: f32,
    pub attenuation: f32,
    pub origin: [f32; 3],
    pub pitch: u8,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub index: u16,
    /// The entity that played the event, -1 when unknown.
    pub entity: i32,
    /// Completed as the client does for an entity of the frame: the origin and angles the server left out are the
    /// entity's, a player's ducking is its hull's. A player's angles are its view's (the server sends its model's).
    pub args: EventArgs,
    pub delay: f32,
}

#[derive(Clone, Debug)]
pub enum Msg {
    ServerInfo {
        /// The decoder's counts before this message, which opens a map.
        stats_before: Stats,
        /// The models of the map before, which the new map's resource list replaces.
        models_before: FxHashMap<u16, String>,
        /// The map's CRC (the engine's `CRC_MapFile`) as sent: munged for the slot of the client it went to.
        map_crc: u32,
        max_clients: u8,
        player_num: u8,
        game_dir: String,
        hostname: String,
        map: String,
    },
    UserInfo {
        slot: u8,
        user_id: i32,
        info: String,
    },
    Time(f32),
    /// A frame's entities, all of them after a delta was applied.
    Entities(Rc<[Entity]>),
    Event(Event),
    Sound(Sound),
    TempEntity {
        kind: u8,
        data: Vec<u8>,
    },
    User {
        id: u8,
        name: String,
        data: Vec<u8>,
    },
    ClientData(Rc<[u8]>),
    Print(String),
    CenterPrint(String),
    StuffText(String),
    SetView(i16),
    Intermission,
    Disconnect(String),
}

/// What the stream has set up so far: descriptions, resources, user messages, baselines and recent frames.
pub struct Decoder {
    pub deltas: Deltas,
    pub entity_layout: Layout,
    pub client_layout: Layout,
    pub event_layout: Layout,
    pub max_clients: u8,
    pub resources: Resources,
    user_msgs: Vec<Option<UserMsgDef>>,
    baselines: Vec<Option<Rc<[u8]>>>,
    instanced: Vec<Rc<[u8]>>,
    max_instanced: u32,
    /// Recent frames by the low byte of their incoming sequence, with the whole sequence.
    frames: Vec<Option<(i32, Rc<[Entity]>)>>,
    client_frames: Vec<Option<(i32, Rc<[u8]>)>>,
    current: Option<Rc<[Entity]>>,
    current_client: Option<Rc<[u8]>>,
    time: f64,
    pub stats: Stats,
    /// Prints every message read to stderr (debugging a stream).
    pub trace: bool,
    /// The stream is an HLTV proxy's (`svc_hltv` active): its `svc_clientdata` carries nothing.
    pub hltv: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub messages: u64,
    pub frames_failed: u64,
    pub missing_delta_base: u64,
    /// Entity packets whose count disagreed with what was read: misread, so dropped.
    pub count_mismatch: u64,
    pub last_error: Option<String>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            deltas: Deltas::default(),
            entity_layout: Layout::default(),
            client_layout: Layout::default(),
            event_layout: Layout::default(),
            max_clients: 0,
            resources: Resources::default(),
            user_msgs: vec![None; 256],
            baselines: vec![None; MAX_EDICTS],
            instanced: Vec::new(),
            max_instanced: 0,
            frames: vec![None; 256],
            client_frames: vec![None; 256],
            current: None,
            current_client: None,
            time: 0.0,
            stats: Stats::default(),
            trace: false,
            hltv: false,
        }
    }
}

impl Decoder {
    pub fn user_msg(&self, id: u8) -> Option<&UserMsgDef> {
        self.user_msgs[usize::from(id)].as_ref()
    }

    /// Reads one network message; what was read before an error is kept in `out`.
    pub fn message(&mut self, msg: &[u8], incoming_sequence: i32, out: &mut Vec<Msg>) -> Result<(), Error> {
        self.current = None;
        self.current_client = None;
        let mut b = Bits::new(msg);
        let result = self.message_body(&mut b, incoming_sequence, out);
        let slot = (incoming_sequence & 0xFF) as usize;
        if let Some(entities) = self.current.take() {
            self.frames[slot] = Some((incoming_sequence, entities));
        }
        if let Some(client) = self.current_client.take() {
            self.client_frames[slot] = Some((incoming_sequence, client));
        }
        if let Err(e) = &result {
            self.stats.frames_failed += 1;
            self.stats.last_error = Some(e.to_string());
        }
        result
    }

    fn message_body(&mut self, b: &mut Bits<'_>, seq: i32, out: &mut Vec<Msg>) -> Result<(), Error> {
        while !b.at_end() {
            let at = b.bit_pos();
            let cmd = b.read_u8()?;
            self.stats.messages += 1;
            if self.trace {
                eprintln!("  svc {cmd} at bit {at} (byte {})", at / 8);
            }
            if cmd >= USER_MESSAGES {
                self.user_message(b, cmd, out)?;
                continue;
            }
            match cmd {
                0 => return Err(Error::Message("svc_bad".into())),
                1 | 19 | 27 | 28 | 42 => {}
                2 => out.push(Msg::Disconnect(b.read_string()?)),
                3 => self.events(b, out)?,
                4 => b.skip_bytes(4)?,
                5 => out.push(Msg::SetView(b.read_i16()?)),
                6 => self.sound(b, out)?,
                7 => {
                    let t = b.read_f32()?;
                    self.time = f64::from(t);
                    out.push(Msg::Time(t));
                }
                8 => out.push(Msg::Print(b.read_string()?)),
                9 => out.push(Msg::StuffText(b.read_string()?)),
                10 => b.skip_bytes(6)?,
                11 => self.server_info(b, out)?,
                12 => {
                    b.skip_bytes(1)?;
                    b.read_cstring()?;
                }
                13 => {
                    let slot = b.read_u8()?;
                    let user_id = b.read_i32()?;
                    let info = b.read_string()?;
                    b.skip_bytes(16)?;
                    out.push(Msg::UserInfo { slot, user_id, info });
                }
                14 => self.delta_description(b)?,
                15 if self.hltv => {}
                15 => self.client_data(b, seq, out)?,
                16 => b.skip_bytes(2)?,
                17 => {
                    while b.read_bit()? {
                        b.skip_bits(24)?;
                    }
                    b.align();
                }
                18 => b.skip_bytes(11)?,
                20 => spawn_static(b)?,
                21 => self.event_reliable(b, out)?,
                22 => self.baseline(b)?,
                23 => temp_entity(b, self.hltv, out)?,
                24 | 25 => b.skip_bytes(1)?,
                26 => out.push(Msg::CenterPrint(b.read_string()?)),
                29 => b.skip_bytes(14)?,
                30 => out.push(Msg::Intermission),
                31 | 34 | 49 | 56 | 57 => {
                    b.read_cstring()?;
                }
                32 | 35 | 37 | 38 | 47 => b.skip_bytes(2)?,
                33 => {
                    b.read_cstring()?;
                    let n = b.read_u8()?;
                    for _ in 0..n {
                        b.read_cstring()?;
                    }
                }
                36 => {
                    b.skip_bytes(1)?;
                    b.read_cstring()?;
                }
                39 => {
                    let id = b.read_u8()?;
                    let size = b.read_u8()?;
                    let name = crate::bits::latin1(&b.read_bytes(16)?);
                    let name = name.trim_end_matches('\0').to_string();
                    let size = if size == 255 { -1 } else { i32::from(size) };
                    self.user_msgs[usize::from(id)] = Some(UserMsgDef { name, size });
                }
                40 => self.packet_entities(b, out)?,
                41 => {
                    let count = b.read_u16()?;
                    let from = b.read_u8()?;
                    self.packet_entities_delta(b, count, from, seq, out)?;
                }
                43 => self.resource_list(b)?,
                44 => {
                    b.skip_bytes(16 * 4 + 1 + 8 * 4)?;
                    b.read_cstring()?;
                }
                45 => b.skip_bytes(8)?,
                46 => {
                    b.skip_bytes(2)?;
                    b.read_cstring()?;
                    b.skip_bytes(6)?;
                    let flags = u32::from(b.read_u8()?);
                    if flags & RES_CUSTOM != 0 {
                        b.skip_bytes(16)?;
                    }
                }
                48 => b.skip_bytes(4)?,
                50 => match b.read_u8()? {
                    0 => self.hltv = true,
                    1 => b.skip_bytes(10)?,
                    other => return Err(Error::Message(format!("svc_hltv command {other}"))),
                },
                51 => {
                    let n = b.read_u8()?;
                    b.skip_bytes(usize::from(n))?;
                }
                52 => {
                    b.read_cstring()?;
                    b.skip_bytes(1)?;
                }
                53 => {
                    b.skip_bytes(1)?;
                    let n = b.read_u16()?;
                    b.skip_bytes(usize::from(n))?;
                }
                54 => {
                    b.read_cstring()?;
                    b.skip_bytes(1)?;
                }
                55 => b.skip_bytes(4)?,
                58 => {
                    b.skip_bytes(4)?;
                    b.read_cstring()?;
                }
                59 => {
                    if b.read_u8()? != 0 {
                        b.skip_bytes(1)?;
                    }
                }
                other => return Err(Error::Message(format!("unknown message {other}"))),
            }
        }
        Ok(())
    }

    fn user_message(&mut self, b: &mut Bits<'_>, id: u8, out: &mut Vec<Msg>) -> Result<(), Error> {
        let Some(def) = self.user_msgs[usize::from(id)].clone() else {
            return Err(Error::Message(format!("unregistered user message {id}")));
        };
        let size = if def.size < 0 {
            usize::from(b.read_u8()?)
        } else {
            def.size as usize
        };
        let data = b.read_bytes(size)?;
        out.push(Msg::User {
            id,
            name: def.name,
            data,
        });
        Ok(())
    }

    fn server_info(&mut self, b: &mut Bits<'_>, out: &mut Vec<Msg>) -> Result<(), Error> {
        let _protocol = b.read_i32()?;
        let _spawn_count = b.read_i32()?;
        let map_crc = b.read_u32()?;
        b.skip_bytes(16)?;
        let max_clients = b.read_u8()?;
        let player_num = b.read_u8()?;
        let _game_type = b.read_u8()?;
        let game_dir = b.read_string()?;
        let hostname = b.read_string()?;
        let map = b.read_string()?;
        b.read_cstring()?;
        if b.read_u8()? != 0 {
            let n = b.read_u8()?;
            b.skip_bytes(usize::from(n) + 16)?;
        }
        let mut stats_before = self.stats.clone();
        stats_before.messages = stats_before.messages.saturating_sub(1);
        let models_before = std::mem::take(&mut self.resources.models);
        let (trace, hltv, stats) = (self.trace, self.hltv, std::mem::take(&mut self.stats));
        *self = Decoder::default();
        self.trace = trace;
        self.hltv = hltv;
        self.stats = stats;
        self.max_clients = max_clients;
        out.push(Msg::ServerInfo {
            stats_before,
            models_before,
            map_crc,
            max_clients,
            player_num,
            game_dir,
            hostname,
            map,
        });
        Ok(())
    }

    fn delta_description(&mut self, b: &mut Bits<'_>) -> Result<(), Error> {
        let name = b.read_string()?;
        let count = b.read_u16()?;
        let mut desc = Desc::default();
        for _ in 0..count {
            desc.fields.push(read_field_description(b)?);
        }
        b.align();
        self.deltas.by_name.insert(name, desc);
        let entity_descs: Vec<&Desc> = [ENTITY, PLAYER, CUSTOM]
            .iter()
            .filter_map(|n| self.deltas.by_name.get(*n))
            .collect();
        self.entity_layout = Layout::from_descs(entity_descs);
        self.client_layout = Layout::from_descs(self.deltas.by_name.get("clientdata_t"));
        self.event_layout = Layout::from_descs(self.deltas.by_name.get("event_t"));
        Ok(())
    }

    fn resource_list(&mut self, b: &mut Bits<'_>) -> Result<(), Error> {
        let total = b.read_bits(12)?;
        for _ in 0..total {
            let kind = b.read_bits(4)?;
            let name = b.read_string()?;
            let index = b.read_bits(12)? as u16;
            let _download_size = b.read_bits(24)?;
            let flags = b.read_bits(3)?;
            if flags & RES_CUSTOM != 0 {
                b.skip_bits(16 * 8)?;
            }
            if b.read_bit()? {
                b.skip_bits(32 * 8)?;
            }
            match kind {
                0 => {
                    self.resources.sounds.insert(index, name);
                }
                2 => {
                    self.resources.models.insert(index, name);
                }
                5 => {
                    self.resources.events.insert(index, name);
                }
                _ => {}
            }
        }
        if b.read_bit()? {
            while b.read_bit()? {
                let short = b.read_bit()?;
                b.skip_bits(if short { 5 } else { 10 })?;
            }
        }
        b.align();
        Ok(())
    }

    fn is_player(&self, number: u32) -> bool {
        number >= 1 && number <= u32::from(self.max_clients)
    }

    fn entity_desc(&self, number: u32, custom: bool) -> Result<&Desc, Error> {
        if custom {
            self.deltas.get(CUSTOM)
        } else if self.is_player(number) {
            self.deltas.get(PLAYER)
        } else {
            self.deltas.get(ENTITY)
        }
    }

    fn zero_state(&self) -> Vec<u8> {
        vec![0u8; self.entity_layout.size.max(1)]
    }

    fn baseline(&mut self, b: &mut Bits<'_>) -> Result<(), Error> {
        let zero = self.zero_state();
        while b.peek_bits(16)? != 0xFFFF {
            let index = b.read_bits(MAX_EDICT_BITS)?;
            let kind = b.read_bits(2)?;
            let custom = kind & ENTITY_BEAM != 0;
            let mut state = self.zero_state();
            self.entity_desc(index, custom)?
                .decode(b, &zero, &mut state, self.time)?;
            self.baselines[index as usize] = Some(state.into());
        }
        b.skip_bits(16)?;
        self.max_instanced = b.read_bits(6)?;
        self.instanced.clear();
        for _ in 0..self.max_instanced {
            let mut state = self.zero_state();
            self.deltas.get(ENTITY)?.decode(b, &zero, &mut state, self.time)?;
            self.instanced.push(state.into());
        }
        b.align();
        Ok(())
    }

    /// The header of an entity in a packet: its number, removal, custom and baseline flags.
    fn delta_header(&self, b: &mut Bits<'_>, numbase: &mut u32, full: bool) -> Result<EntityHeader, Error> {
        let mut h = EntityHeader::default();
        let consecutive = if full {
            b.read_bit()?
        } else {
            h.remove = b.read_bit()?;
            false
        };
        h.number = if consecutive {
            *numbase + 1
        } else if b.read_bit()? {
            b.read_bits(MAX_EDICT_BITS)?
        } else {
            *numbase + b.read_bits(DELTA_OFFSET_BITS)?
        };
        *numbase = h.number;
        if !h.remove {
            h.custom = b.read_bit()?;
            if self.max_instanced > 0 && b.read_bit()? {
                h.instanced = Some(b.read_bits(MAX_BASELINE_BITS)? as usize);
            }
            if full && h.instanced.is_none() && b.read_bit()? {
                h.offset = b.read_bits(MAX_BASELINE_BITS)? as usize;
            }
        }
        Ok(h)
    }

    fn base_for(&self, h: &EntityHeader, built: &[Entity]) -> Result<Rc<[u8]>, Error> {
        if let Some(i) = h.instanced {
            return self
                .instanced
                .get(i)
                .cloned()
                .ok_or_else(|| Error::Message(format!("instanced baseline {i} missing")));
        }
        if h.offset > 0 {
            return built
                .len()
                .checked_sub(h.offset)
                .map(|i| built[i].state.clone())
                .ok_or_else(|| Error::Message("baseline offset past the frame".into()));
        }
        Ok(self.baselines[h.number as usize % MAX_EDICTS]
            .clone()
            .unwrap_or_else(|| self.zero_state().into()))
    }

    fn decode_entity(&self, b: &mut Bits<'_>, h: &EntityHeader, base: &[u8]) -> Result<Entity, Error> {
        let mut state = self.zero_state();
        if base.len() < state.len() {
            let mut grown = self.zero_state();
            grown[..base.len()].copy_from_slice(base);
            self.entity_desc(h.number, h.custom)?
                .decode(b, &grown, &mut state, self.time)?;
        } else {
            self.entity_desc(h.number, h.custom)?
                .decode(b, base, &mut state, self.time)?;
        }
        Ok(Entity {
            number: h.number as u16,
            custom: h.custom,
            state: state.into(),
        })
    }

    fn packet_entities(&mut self, b: &mut Bits<'_>, out: &mut Vec<Msg>) -> Result<(), Error> {
        let count = b.read_u16()?;
        let mut built: Vec<Entity> = Vec::with_capacity(usize::from(count));
        let mut numbase = 0;
        let mut last = None;
        while b.peek_bits(16)? != 0 {
            let h = self.delta_header(b, &mut numbase, true)?;
            if h.remove {
                return Err(Error::Message("removal in a full entity packet".into()));
            }
            in_order(h.number, &mut last)?;
            let base = self.base_for(&h, &built)?;
            let e = self.decode_entity(b, &h, &base)?;
            built.push(e);
        }
        b.skip_bits(16)?;
        b.align();
        self.check_count(built.len(), count)?;
        let entities: Rc<[Entity]> = built.into();
        self.current = Some(entities.clone());
        out.push(Msg::Entities(entities));
        Ok(())
    }

    fn packet_entities_delta(
        &mut self,
        b: &mut Bits<'_>,
        count: u16,
        from: u8,
        seq: i32,
        out: &mut Vec<Msg>,
    ) -> Result<(), Error> {
        let want = base_sequence(seq, from);
        let Some(old) = self.frames[usize::from(from)]
            .as_ref()
            .filter(|(s, _)| *s == want)
            .map(|(_, e)| e.clone())
        else {
            self.stats.missing_delta_base += 1;
            return Err(Error::Message(format!("delta base {from} unknown (sequence {seq})")));
        };
        let mut built: Vec<Entity> = Vec::with_capacity(old.len() + 8);
        let mut oldi = 0;
        let mut numbase = 0;
        let mut last = None;
        while b.peek_bits(16)? != 0 {
            let h = self.delta_header(b, &mut numbase, false)?;
            in_order(h.number, &mut last)?;
            let mut oldnum = old.get(oldi).map_or(u32::MAX, |e| u32::from(e.number));
            while oldnum < h.number {
                built.push(old[oldi].clone());
                oldi += 1;
                oldnum = old.get(oldi).map_or(u32::MAX, |e| u32::from(e.number));
            }
            if h.number == oldnum {
                if !h.remove {
                    let e = self.decode_entity(b, &h, &old[oldi].state)?;
                    built.push(e);
                }
                oldi += 1;
                continue;
            }
            if h.remove {
                continue;
            }
            let base = self.base_for(&h, &built)?;
            let e = self.decode_entity(b, &h, &base)?;
            built.push(e);
        }
        b.skip_bits(16)?;
        b.align();
        built.extend(old.iter().skip(oldi).cloned());
        self.check_count(built.len(), count)?;
        let entities: Rc<[Entity]> = built.into();
        self.current = Some(entities.clone());
        out.push(Msg::Entities(entities));
        Ok(())
    }

    /// An entity packet that read another number of entities than it said it had is misread: none of it is kept.
    fn check_count(&mut self, read: usize, count: u16) -> Result<(), Error> {
        if read == usize::from(count) {
            return Ok(());
        }
        self.stats.count_mismatch += 1;
        Err(Error::Message(format!("{read} entities read of {count}")))
    }

    fn client_data(&mut self, b: &mut Bits<'_>, seq: i32, out: &mut Vec<Msg>) -> Result<(), Error> {
        let size = self.client_layout.size.max(1);
        let base: Rc<[u8]> = if b.read_bit()? {
            let from = b.read_u8()?;
            let want = base_sequence(seq, from);
            let Some(base) = self.client_frames[usize::from(from)]
                .as_ref()
                .filter(|(s, _)| *s == want)
                .map(|(_, c)| c.clone())
            else {
                self.stats.missing_delta_base += 1;
                return Err(Error::Message(format!(
                    "client data delta base {from} unknown (sequence {seq})"
                )));
            };
            base
        } else {
            vec![0u8; size].into()
        };
        let mut state = vec![0u8; size];
        self.deltas
            .get("clientdata_t")?
            .decode(b, &base, &mut state, self.time)?;
        while b.read_bit()? {
            b.skip_bits(6)?;
            self.deltas.get("weapon_data_t")?.skip(b)?;
        }
        b.align();
        let state: Rc<[u8]> = state.into();
        self.current_client = Some(state.clone());
        out.push(Msg::ClientData(state));
        Ok(())
    }

    fn event_args(&self, b: &mut Bits<'_>) -> Result<EventArgs, Error> {
        let desc = self.deltas.get("event_t")?;
        let size = desc.size().max(72);
        let zero = vec![0u8; size];
        let mut state = vec![0u8; size];
        desc.decode(b, &zero, &mut state, self.time)?;
        let l = &self.event_layout;
        let f = |name: &str| l.slot(name).map_or(0.0, |s| s.read_f32(&state));
        let i = |name: &str| l.slot(name).map_or(0, |s| s.read_i64(&state) as i32);
        Ok(EventArgs {
            flags: i("flags"),
            entindex: i("entindex"),
            origin: [f("origin[0]"), f("origin[1]"), f("origin[2]")],
            angles: [f("angles[0]"), f("angles[1]"), f("angles[2]")],
            velocity: [f("velocity[0]"), f("velocity[1]"), f("velocity[2]")],
            ducking: i("ducking"),
            fparam1: f("fparam1"),
            fparam2: f("fparam2"),
            iparam1: i("iparam1"),
            iparam2: i("iparam2"),
            bparam1: i("bparam1"),
            bparam2: i("bparam2"),
        })
    }

    fn events(&mut self, b: &mut Bits<'_>, out: &mut Vec<Msg>) -> Result<(), Error> {
        let count = b.read_bits(5)?;
        for _ in 0..count {
            let index = b.read_bits(10)? as u16;
            let mut packet_index = None;
            let mut args = EventArgs::default();
            if b.read_bit()? {
                packet_index = Some(b.read_bits(11)? as usize);
                if b.read_bit()? {
                    args = self.event_args(b)?;
                }
            }
            let delay = if b.read_bit()? {
                b.read_bits(16)? as f32 / 100.0
            } else {
                0.0
            };
            let entity = match (packet_index, &self.current) {
                (Some(p), Some(ents)) if p < ents.len() => {
                    self.complete_args(&ents[p], &mut args);
                    i32::from(ents[p].number)
                }
                (Some(_), _) if args.entindex > 0 => {
                    // The server sent the invoker's own angles. The client divides a player's pitch by -3 here (Xash
                    // `CL_ParseEvent`), which gives no view pitch: it is turned as for an entity of the frame.
                    view_angles(&mut args.angles, self.is_player(args.entindex as u32));
                    args.entindex
                }
                _ if args.entindex > 0 => args.entindex,
                _ => -1,
            };
            out.push(Msg::Event(Event {
                index,
                entity,
                args,
                delay,
            }));
        }
        b.align();
        Ok(())
    }

    /// What the server leaves out of an event whose invoker is in the frame, filled in as the client does (Xash
    /// `CL_ParseEvent`): the entity's origin and angles where the event has none, a player's ducking from its hull.
    fn complete_args(&self, e: &Entity, args: &mut EventArgs) {
        let l = &self.entity_layout;
        let f = |name: &str| l.slot(name).map_or(0.0, |s| s.read_f32(&e.state));
        let player = self.is_player(u32::from(e.number));
        args.entindex = i32::from(e.number);
        if args.origin == [0.0; 3] {
            args.origin = [f("origin[0]"), f("origin[1]"), f("origin[2]")];
        }
        if args.angles == [0.0; 3] {
            args.angles = [f("angles[0]"), f("angles[1]"), f("angles[2]")];
        }
        if player {
            args.ducking = i32::from(l.slot("usehull").is_some_and(|s| s.read_i64(&e.state) == 1));
        }
        view_angles(&mut args.angles, player);
    }

    fn event_reliable(&mut self, b: &mut Bits<'_>, out: &mut Vec<Msg>) -> Result<(), Error> {
        let index = b.read_bits(10)? as u16;
        let args = self.event_args(b)?;
        let delay = if b.read_bit()? {
            b.read_bits(16)? as f32 / 100.0
        } else {
            0.0
        };
        b.align();
        out.push(Msg::Event(Event {
            index,
            entity: if args.entindex > 0 { args.entindex } else { -1 },
            args,
            delay,
        }));
        Ok(())
    }

    fn sound(&mut self, b: &mut Bits<'_>, out: &mut Vec<Msg>) -> Result<(), Error> {
        let flags = b.read_bits(9)?;
        let volume = if flags & SND_FL_VOLUME != 0 {
            b.read_bits(8)? as f32 / 255.0
        } else {
            1.0
        };
        let attenuation = if flags & SND_FL_ATTENUATION != 0 {
            b.read_bits(8)? as f32 / 64.0
        } else {
            0.8
        };
        let channel = b.read_bits(3)? as u8;
        let entity = b.read_bits(MAX_EDICT_BITS)? as u16;
        let index = b.read_bits(if flags & SND_FL_LARGE_INDEX != 0 { 16 } else { 8 })? as u16;
        let origin = b.read_bit_vec3_coord()?;
        let pitch = if flags & SND_FL_PITCH != 0 {
            b.read_bits(8)? as u8
        } else {
            100
        };
        b.align();
        let sample = if flags & SND_FL_SENTENCE != 0 {
            format!("!{index}")
        } else {
            self.resources
                .sounds
                .get(&index)
                .cloned()
                .unwrap_or_else(|| format!("#{index}"))
        };
        out.push(Msg::Sound(Sound {
            entity,
            channel,
            sample,
            volume,
            attenuation,
            origin,
            pitch,
        }));
        Ok(())
    }
}

/// Angles into -180..180 (the client's `COM_NormalizeAngles`), and a player's model pitch, its view's divided by -3,
/// back into the view's.
fn view_angles(angles: &mut [f32; 3], player: bool) {
    for a in angles.iter_mut() {
        if *a > 180.0 {
            *a -= 360.0;
        } else if *a < -180.0 {
            *a += 360.0;
        }
    }
    if player {
        angles[0] *= -3.0;
    }
}

/// The sequence of the frame a delta names by the low byte of its sequence: the latest one up to `seq`.
fn base_sequence(seq: i32, from: u8) -> i32 {
    seq.wrapping_sub(i32::from((seq as u8).wrapping_sub(from)))
}

/// Entities of a packet come in rising numbers below the edict limit; anything else is a misread packet.
fn in_order(number: u32, last: &mut Option<u32>) -> Result<(), Error> {
    if number as usize >= MAX_EDICTS || last.is_some_and(|l| number <= l) {
        return Err(Error::Message(format!("entity {number} after {last:?}")));
    }
    *last = Some(number);
    Ok(())
}

#[derive(Default)]
struct EntityHeader {
    number: u32,
    remove: bool,
    custom: bool,
    instanced: Option<usize>,
    offset: usize,
}

fn temp_entity(b: &mut Bits<'_>, hltv: bool, out: &mut Vec<Msg>) -> Result<(), Error> {
    let kind = b.read_u8()?;
    let len = if kind == TE_LINE && !hltv {
        17
    } else {
        TE_LENGTH.get(usize::from(kind)).copied().unwrap_or(-2)
    };
    let data = match len {
        -2 => return Err(Error::Message(format!("temp entity {kind}"))),
        -1 if kind == TE_BSPDECAL => {
            let mut d = b.read_bytes(8)?;
            let ent = b.read_bytes(2)?;
            let has_model = ent != [0, 0];
            d.extend(ent);
            if has_model {
                d.extend(b.read_bytes(2)?);
            }
            d
        }
        -1 if kind == TE_TEXTMESSAGE => {
            let mut d = b.read_bytes(6)?;
            let effect = d[5];
            d.extend(b.read_bytes(if effect == 2 { 16 } else { 14 })?);
            d.extend(b.read_cstring()?);
            d
        }
        -1 => return Err(Error::Message(format!("temp entity {kind} of unknown length"))),
        n => b.read_bytes(n as usize)?,
    };
    out.push(Msg::TempEntity { kind, data });
    Ok(())
}

/// `svc_spawnstatic` as the engine's `PF_makestatic` writes it (game DLLs hardly use it).
fn spawn_static(b: &mut Bits<'_>) -> Result<(), Error> {
    b.skip_bytes(2 + 1 + 1 + 2 + 1)?;
    b.skip_bytes(3 * 3)?;
    if b.read_u8()? != 0 {
        b.skip_bytes(1 + 3 + 1)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delta_base_is_the_latest_frame_with_that_low_byte() {
        assert_eq!(base_sequence(0x102, 0xff), 0xff);
        assert_eq!(base_sequence(0x102, 0x01), 0x101);
        assert_eq!(
            base_sequence(0x102, 0x02),
            0x102,
            "no earlier frame: the base is missing"
        );
    }

    #[test]
    fn packet_entities_rise_below_the_edict_limit() {
        let mut last = None;
        assert!(in_order(1, &mut last).is_ok());
        assert!(in_order(5, &mut last).is_ok());
        assert!(in_order(5, &mut last).is_err());
        assert!(in_order(3, &mut None).is_ok());
        assert!(in_order(MAX_EDICTS as u32, &mut None).is_err());
    }

    #[test]
    fn a_players_model_pitch_turns_into_its_view_pitch() {
        let mut a = [350.0, 270.0, 0.0];
        view_angles(&mut a, true);
        assert_eq!(a, [30.0, -90.0, 0.0], "a model pitched 10 up is a view 30 down");
        let mut a = [350.0, 270.0, 0.0];
        view_angles(&mut a, false);
        assert_eq!(a, [-10.0, -90.0, 0.0]);
    }
}
