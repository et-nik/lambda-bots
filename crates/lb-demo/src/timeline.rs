//! A demo turned into a game timeline: who played, where everyone was and looked at each snapshot, what was shot,
//! heard, blown up and picked up, and who killed whom.

use std::rc::Rc;

use rustc_hash::FxHashMap;

use crate::Error;
use crate::delta::Layout;
use crate::file::{DemoFile, FrameData};
use crate::net::{Decoder, Entity, Event, Msg, Stats};

/// `EF_NODRAW`: the entity is not drawn (an item taken and waiting to come back, a dead player gone).
pub const EF_NODRAW: i32 = 128;

#[derive(Clone, Debug)]
pub struct Player {
    pub slot: u8,
    pub user_id: i32,
    pub name: String,
    /// No `*sid` nor `rate` in the user info, or a `*bot` flag: a fake client.
    pub bot: bool,
    pub hltv: bool,
    /// The `*sid` of the user info, empty when none.
    pub steam_id: String,
    pub first_seen: f32,
    pub last_seen: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    /// Demo time, seconds from the start of playback.
    pub time: f32,
    /// The server's clock in the same message.
    pub server_time: f32,
    pub first_state: u32,
    pub states: u16,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PlayerState {
    pub slot: u8,
    /// Index into [`Recording::players`], `u16::MAX` when the slot had no known player.
    pub who: u16,
    pub origin: [f32; 3],
    /// Pitch, yaw, roll of the player's model (pitch is the view's, scaled by the game).
    pub angles: [f32; 3],
    pub model: u16,
    /// The model index of the weapon in hand (`p_*.mdl`), 0 when none.
    pub weapon: u16,
    pub sequence: i16,
    pub gaitsequence: i16,
    pub effects: i32,
    pub movetype: u8,
    pub solid: u8,
    pub usehull: u8,
    pub spectator: u8,
    pub iuser1: i32,
}

#[derive(Clone, Debug)]
pub struct Shot {
    /// When the event plays: its message's time and the delay the server gave it.
    pub time: f32,
    pub event: String,
    pub entity: i32,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub fparam1: f32,
    pub fparam2: f32,
    pub iparam1: i32,
    pub iparam2: i32,
    pub bparam1: i32,
    pub bparam2: i32,
    pub ducking: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct SoundEvent {
    pub time: f32,
    pub entity: u16,
    pub channel: u8,
    /// Index into [`Recording::sound_names`].
    pub sample: u16,
    pub volume: f32,
    pub origin: [f32; 3],
}

#[derive(Clone, Debug)]
pub struct Kill {
    pub time: f32,
    /// Entity numbers: players are 1..=max_clients, 0 is the world.
    pub killer: u8,
    pub victim: u8,
    pub weapon: String,
}

#[derive(Clone, Copy, Debug)]
pub struct Explosion {
    pub time: f32,
    pub origin: [f32; 3],
    pub scale: u8,
}

/// Where a moving explosive or projectile was at a snapshot.
#[derive(Clone, Copy, Debug)]
pub struct Projectile {
    pub time: f32,
    pub entity: u16,
    pub model: u16,
    pub origin: [f32; 3],
    pub owner: i32,
}

/// An item that came into the recording's entities, changed, or left them. Half-Life sends no hidden entity, so an
/// item taken and waiting to come back leaves them; so does one removed from the game and, in a POV demo, one out of
/// the player's view: the stream does not tell these apart.
#[derive(Clone, Copy, Debug)]
pub struct ItemChange {
    pub time: f32,
    pub entity: u16,
    pub model: u16,
    pub origin: [f32; 3],
    pub visible: bool,
    /// It left the entities.
    pub removed: bool,
}

/// The recording player's own view at a client frame (POV demos).
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub time: f32,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
}

/// The recording player's command at a network frame (POV demos).
#[derive(Clone, Copy, Debug)]
pub struct Command {
    pub time: f32,
    pub msec: u8,
    pub viewangles: [f32; 3],
    pub forwardmove: f32,
    pub sidemove: f32,
    pub buttons: u16,
    pub health: i32,
}

#[derive(Debug, Default)]
pub struct Recording {
    pub map: String,
    pub game_dir: String,
    pub hostname: String,
    /// CRC of the map's BSP as the server computed it, to tell builds of a map apart (`lb_bsp::file::map_crc`); 0 from
    /// a server that computed none (a game of one).
    pub map_crc: u32,
    pub max_clients: u8,
    /// The slot of the recording player in a POV demo.
    pub recorder: Option<u8>,
    pub duration: f32,
    pub players: Vec<Player>,
    pub snapshots: Vec<Snapshot>,
    pub states: Vec<PlayerState>,
    pub shots: Vec<Shot>,
    pub sounds: Vec<SoundEvent>,
    pub sound_names: Vec<String>,
    pub kills: Vec<Kill>,
    pub explosions: Vec<Explosion>,
    pub projectiles: Vec<Projectile>,
    pub items: Vec<ItemChange>,
    pub models: FxHashMap<u16, String>,
    pub views: Vec<View>,
    pub commands: Vec<Command>,
    pub console: Vec<(f32, String)>,
    /// The decoder's counts for this map alone, with the frames of the file that could not be read.
    pub stats: Stats,
    /// Frames that could not be read whole, with the reason (the first few).
    pub errors: Vec<(f32, String)>,
    pub user_messages: FxHashMap<String, u32>,
}

impl Recording {
    pub fn is_hltv(&self) -> bool {
        self.recorder.is_none()
    }

    pub fn model_name(&self, index: u16) -> &str {
        self.models.get(&index).map_or("", String::as_str)
    }

    pub fn states_of(&self, snap: &Snapshot) -> &[PlayerState] {
        let a = snap.first_state as usize;
        &self.states[a..a + usize::from(snap.states)]
    }
}

/// Field slots of `entity_state_t` the timeline reads, resolved once the descriptions are known.
struct Fields {
    origin: [Option<crate::delta::Slot>; 3],
    angles: [Option<crate::delta::Slot>; 3],
    modelindex: Option<crate::delta::Slot>,
    weaponmodel: Option<crate::delta::Slot>,
    sequence: Option<crate::delta::Slot>,
    gaitsequence: Option<crate::delta::Slot>,
    effects: Option<crate::delta::Slot>,
    movetype: Option<crate::delta::Slot>,
    solid: Option<crate::delta::Slot>,
    usehull: Option<crate::delta::Slot>,
    spectator: Option<crate::delta::Slot>,
    iuser1: Option<crate::delta::Slot>,
    owner: Option<crate::delta::Slot>,
}

impl Fields {
    fn from_layout(l: &Layout) -> Self {
        Self {
            origin: [l.slot("origin[0]"), l.slot("origin[1]"), l.slot("origin[2]")],
            angles: [l.slot("angles[0]"), l.slot("angles[1]"), l.slot("angles[2]")],
            modelindex: l.slot("modelindex"),
            weaponmodel: l.slot("weaponmodel"),
            sequence: l.slot("sequence"),
            gaitsequence: l.slot("gaitsequence"),
            effects: l.slot("effects"),
            movetype: l.slot("movetype"),
            solid: l.slot("solid"),
            usehull: l.slot("usehull"),
            spectator: l.slot("spectator"),
            iuser1: l.slot("iuser1"),
            owner: l.slot("owner"),
        }
    }
}

fn f(slot: Option<crate::delta::Slot>, s: &[u8]) -> f32 {
    slot.map_or(0.0, |x| x.read_f32(s))
}

fn i(slot: Option<crate::delta::Slot>, s: &[u8]) -> i64 {
    slot.map_or(0, |x| x.read_i64(s))
}

/// Models of things thrown, fired or laid that fly or wait to go off.
fn is_projectile(model: &str) -> bool {
    matches!(
        model,
        "models/grenade.mdl"
            | "models/w_satchel.mdl"
            | "models/v_tripmine.mdl"
            | "models/rpgrocket.mdl"
            | "models/crossbow_bolt.mdl"
            | "models/w_squeak.mdl"
            | "models/hornet.mdl"
    )
}

/// World models of weapons, ammo and items lying on the map (dropped weapon boxes too).
fn is_item(model: &str) -> bool {
    model.starts_with("models/w_") && model != "models/w_satchel.mdl" && model != "models/w_squeak.mdl"
}

fn info_value<'a>(info: &'a str, key: &str) -> Option<&'a str> {
    let mut parts = info.trim_start_matches('\\').split('\\');
    while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
        if k == key {
            return Some(v);
        }
    }
    None
}

#[derive(Default)]
struct Builder {
    rec: Recording,
    slot_player: FxHashMap<u8, u16>,
    server_time: f32,
    items_seen: FxHashMap<u16, (u16, bool)>,
    sound_index: FxHashMap<String, u16>,
    /// A server info opened this map.
    started: bool,
    /// The decoder's counts when this map began.
    stats_base: Stats,
    /// Frames of the file that could not be read.
    file_frames_failed: u64,
    first_time: Option<f32>,
}

impl Builder {
    fn for_map(map: &str, game_dir: &str, stats_base: &Stats) -> Self {
        let mut b = Builder::default();
        b.rec.map = map.trim_start_matches("maps/").trim_end_matches(".bsp").to_string();
        b.rec.game_dir = game_dir.to_string();
        b.stats_base = stats_base.clone();
        b
    }

    /// The recording with the decoder's counts and models as they were when the map ended.
    fn finish(mut self, stats: &Stats, models: &FxHashMap<u16, String>, saw_view: bool, last_time: f32) -> Recording {
        if !saw_view {
            self.rec.recorder = None;
            self.rec.views.clear();
            self.rec.commands.clear();
        }
        let first = self.first_time.unwrap_or(0.0);
        let last = self.rec.snapshots.last().map_or(last_time, |s| s.time);
        self.rec.duration = (last - first).max(0.0);
        self.rec.stats = stats_since(stats, &self.stats_base);
        self.rec.stats.frames_failed += self.file_frames_failed;
        if self.rec.models.is_empty() {
            self.rec.models = models.clone();
        }
        self.rec
    }

    fn user_info(&mut self, time: f32, slot: u8, user_id: i32, info: &str) {
        if info.is_empty() {
            self.slot_player.remove(&slot);
            return;
        }
        let name = info_value(info, "name").unwrap_or("").to_string();
        let has = |k: &str| info_value(info, k).is_some();
        let bot = info_value(info, "*bot") == Some("1") || (!has("*sid") && !has("rate") && !has("*hltv"));
        let hltv = has("*hltv");
        let steam_id = info_value(info, "*sid").unwrap_or("").to_string();
        if let Some(&idx) = self.slot_player.get(&slot) {
            let p = &mut self.rec.players[usize::from(idx)];
            if p.user_id == user_id {
                (p.name, p.bot, p.hltv, p.steam_id) = (name, bot, hltv, steam_id);
                return;
            }
        }
        let player = Player {
            slot,
            user_id,
            name,
            bot,
            hltv,
            steam_id,
            first_seen: time,
            last_seen: time,
        };
        self.rec.players.push(player);
        self.slot_player.insert(slot, (self.rec.players.len() - 1) as u16);
    }

    fn entities(&mut self, time: f32, layout: &Layout, ents: &Rc<[Entity]>) {
        let fl = &Fields::from_layout(layout);
        let first = self.rec.states.len() as u32;
        let maxc = u16::from(self.rec.max_clients);
        let mut present: FxHashMap<u16, ()> = FxHashMap::default();
        for e in ents.iter() {
            let s = &e.state[..];
            if e.number >= 1 && e.number <= maxc {
                let slot = (e.number - 1) as u8;
                let who = self.slot_player.get(&slot).copied().unwrap_or(u16::MAX);
                if let Some(p) = self.rec.players.get_mut(usize::from(who)) {
                    p.last_seen = time;
                }
                self.rec.states.push(PlayerState {
                    slot,
                    who,
                    origin: [f(fl.origin[0], s), f(fl.origin[1], s), f(fl.origin[2], s)],
                    angles: [f(fl.angles[0], s), f(fl.angles[1], s), f(fl.angles[2], s)],
                    model: i(fl.modelindex, s) as u16,
                    weapon: i(fl.weaponmodel, s) as u16,
                    sequence: i(fl.sequence, s) as i16,
                    gaitsequence: i(fl.gaitsequence, s) as i16,
                    effects: i(fl.effects, s) as i32,
                    movetype: i(fl.movetype, s) as u8,
                    solid: i(fl.solid, s) as u8,
                    usehull: i(fl.usehull, s) as u8,
                    spectator: i(fl.spectator, s) as u8,
                    iuser1: i(fl.iuser1, s) as i32,
                });
                continue;
            }
            if e.custom {
                continue;
            }
            let model = i(fl.modelindex, s) as u16;
            let name = self.rec.models.get(&model).map_or("", String::as_str);
            let origin = [f(fl.origin[0], s), f(fl.origin[1], s), f(fl.origin[2], s)];
            if is_projectile(name) {
                self.rec.projectiles.push(Projectile {
                    time,
                    entity: e.number,
                    model,
                    origin,
                    owner: i(fl.owner, s) as i32,
                });
            } else if is_item(name) {
                present.insert(e.number, ());
                let visible = i(fl.effects, s) as i32 & EF_NODRAW == 0;
                let changed = self.items_seen.get(&e.number) != Some(&(model, visible));
                if changed {
                    self.items_seen.insert(e.number, (model, visible));
                    self.rec.items.push(ItemChange {
                        time,
                        entity: e.number,
                        model,
                        origin,
                        visible,
                        removed: false,
                    });
                }
            }
        }
        let gone: Vec<(u16, u16)> = self
            .items_seen
            .iter()
            .filter(|(n, _)| !present.contains_key(n))
            .map(|(&n, &(m, _))| (n, m))
            .collect();
        for (n, m) in gone {
            self.items_seen.remove(&n);
            self.rec.items.push(ItemChange {
                time,
                entity: n,
                model: m,
                origin: [0.0; 3],
                visible: false,
                removed: true,
            });
        }
        let count = self.rec.states.len() as u32 - first;
        self.rec.snapshots.push(Snapshot {
            time,
            server_time: self.server_time,
            first_state: first,
            states: count as u16,
        });
    }

    fn sound_id(&mut self, sample: &str) -> u16 {
        if let Some(&id) = self.sound_index.get(sample) {
            return id;
        }
        let id = self.rec.sound_names.len() as u16;
        self.rec.sound_names.push(sample.to_string());
        self.sound_index.insert(sample.to_string(), id);
        id
    }
}

/// Reads a whole demo, one [`Recording`] per map it went through. Frames that cannot be read are skipped and
/// counted in [`Recording::stats`].
pub fn read(bytes: &[u8]) -> Result<Vec<Recording>, Error> {
    let file = DemoFile::open(bytes)?;
    let mut dec = Decoder::default();
    dec.trace = std::env::var_os("LB_DEMO_TRACE").is_some();
    let mut done: Vec<Recording> = Vec::new();
    let mut b = Builder::default();
    b.rec.map = file.header.map.clone();
    b.rec.game_dir = file.header.game_dir.clone();
    let mut msgs = Vec::new();
    let mut last_time = 0.0f32;
    let mut saw_view = false;
    for segment in &file.segments {
        for frame in file.frames(segment) {
            let frame = match frame {
                Ok(fr) => fr,
                Err(e) => {
                    b.file_frames_failed += 1;
                    b.rec
                        .errors
                        .push((last_time, format!("segment {}: {e}", segment.description)));
                    break;
                }
            };
            let time = frame.time;
            if segment.kind != 0 {
                last_time = last_time.max(time);
            }
            match frame.data {
                FrameData::Net {
                    view,
                    cmd,
                    incoming_sequence,
                    msg,
                } => {
                    msgs.clear();
                    let failed = dec.message(msg, incoming_sequence, &mut msgs).err();
                    if segment.kind != 0 && view.maxclients > 0 {
                        saw_view = true;
                        b.rec.commands.push(Command {
                            time,
                            msec: cmd.msec,
                            viewangles: cmd.viewangles,
                            forwardmove: cmd.forwardmove,
                            sidemove: cmd.sidemove,
                            buttons: cmd.buttons,
                            health: view.health,
                        });
                    }
                    for m in msgs.drain(..) {
                        if let Msg::ServerInfo {
                            map,
                            stats_before,
                            models_before,
                            ..
                        } = &m
                            && b.started
                        {
                            let next = Builder::for_map(map, &b.rec.game_dir, stats_before);
                            let finished = std::mem::replace(&mut b, next);
                            done.push(finished.finish(stats_before, models_before, saw_view, last_time));
                        }
                        apply(&mut b, &dec, time, m);
                    }
                    if let Some(e) = failed
                        && b.rec.errors.len() < 20
                    {
                        b.rec.errors.push((time, e.to_string()));
                    }
                }
                FrameData::ClientData { origin, viewangles, .. } => {
                    b.rec.views.push(View {
                        time,
                        origin,
                        angles: viewangles,
                    });
                }
                FrameData::ConsoleCommand(c) => b.rec.console.push((time, c)),
                // An event the recording client played itself, its own weapon's: the server sends it no event for
                // its own shots (`FEV_NOTHOST`), the client predicts them.
                FrameData::Event { index, delay, args, .. } if segment.kind != 0 => {
                    let entity = if args.entindex > 0 { args.entindex } else { -1 };
                    let event = Event {
                        index: index as u16,
                        entity,
                        args,
                        delay,
                    };
                    apply(&mut b, &dec, time, Msg::Event(event));
                }
                _ => {}
            }
        }
    }
    done.push(b.finish(&dec.stats, &dec.resources.models, saw_view, last_time));
    Ok(done)
}

/// What the decoder counted since `base`, an earlier copy of its counts.
fn stats_since(now: &Stats, base: &Stats) -> Stats {
    let frames_failed = now.frames_failed.saturating_sub(base.frames_failed);
    Stats {
        messages: now.messages.saturating_sub(base.messages),
        frames_failed,
        missing_delta_base: now.missing_delta_base.saturating_sub(base.missing_delta_base),
        count_mismatch: now.count_mismatch.saturating_sub(base.count_mismatch),
        last_error: now.last_error.clone().filter(|_| frames_failed > 0),
    }
}

/// The map CRC the server computed, from the one it sent the client in slot `slot`: munged for that slot
/// (`COM_Munge3` with `(-1 - slot) & 0xff`).
fn unmunge_crc(sent: u32, slot: u8) -> u32 {
    const TABLE: [u8; 4] = [0x20, 0x07, 0x13, 0x61];
    let seq = u32::from(!slot);
    let mut bytes = (sent ^ seq).to_le_bytes();
    for (j, b) in bytes.iter_mut().enumerate() {
        *b ^= 0xa5 | (j << j) as u8 | j as u8 | TABLE[j];
    }
    u32::from_le_bytes(bytes).swap_bytes() ^ !seq
}

fn apply(b: &mut Builder, dec: &Decoder, time: f32, m: Msg) {
    match m {
        Msg::ServerInfo {
            map_crc,
            max_clients,
            player_num,
            hostname,
            map,
            ..
        } => {
            b.started = true;
            b.rec.map_crc = unmunge_crc(map_crc, player_num);
            b.rec.max_clients = max_clients;
            b.rec.hostname = hostname;
            b.rec.recorder = Some(player_num);
            let name = map.trim_start_matches("maps/").trim_end_matches(".bsp");
            if !name.is_empty() {
                b.rec.map = name.to_string();
            }
        }
        Msg::UserInfo { slot, user_id, info } => b.user_info(time, slot, user_id, &info),
        Msg::Time(t) => b.server_time = t,
        Msg::Entities(ents) => {
            if b.rec.models.is_empty() {
                b.rec.models = dec.resources.models.clone();
            }
            if b.first_time.is_none() {
                b.first_time = Some(time);
            }
            b.entities(time, &dec.entity_layout, &ents);
        }
        Msg::Event(e) => {
            let event = dec
                .resources
                .events
                .get(&e.index)
                .cloned()
                .unwrap_or_else(|| format!("#{}", e.index));
            b.rec.shots.push(Shot {
                time: time + e.delay,
                event,
                entity: e.entity,
                origin: e.args.origin,
                angles: e.args.angles,
                fparam1: e.args.fparam1,
                fparam2: e.args.fparam2,
                iparam1: e.args.iparam1,
                iparam2: e.args.iparam2,
                bparam1: e.args.bparam1,
                bparam2: e.args.bparam2,
                ducking: e.args.ducking,
            });
        }
        Msg::Sound(s) => {
            let sample = b.sound_id(&s.sample);
            b.rec.sounds.push(SoundEvent {
                time,
                entity: s.entity,
                channel: s.channel,
                sample,
                volume: s.volume,
                origin: s.origin,
            });
        }
        Msg::TempEntity { kind: 3, data } if data.len() >= 9 => {
            let c = |o: usize| f32::from(i16::from_le_bytes([data[o], data[o + 1]])) / 8.0;
            b.rec.explosions.push(Explosion {
                time,
                origin: [c(0), c(2), c(4)],
                scale: data[8],
            });
        }
        Msg::User { name, data, .. } => {
            *b.rec.user_messages.entry(name.clone()).or_default() += 1;
            if name == "DeathMsg" && data.len() >= 3 {
                let end = data[2..].iter().position(|&c| c == 0).map_or(data.len(), |p| p + 2);
                b.rec.kills.push(Kill {
                    time,
                    killer: data[0],
                    victim: data[1],
                    weapon: crate::bits::latin1(&data[2..end]),
                });
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::{DEMO_PROTOCOL, ENTRY_SIZE, HEADER_SIZE};

    /// A demo of one playback segment holding `frames` (each: type, time, number, data).
    fn demo(frames: &[(u8, f32, &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        for (n, &(kind, time, data)) in frames.iter().enumerate() {
            body.push(kind);
            body.extend_from_slice(&time.to_le_bytes());
            body.extend_from_slice(&(n as i32).to_le_bytes());
            body.extend_from_slice(data);
        }
        let start = HEADER_SIZE + 4 + ENTRY_SIZE;
        let mut b = vec![0u8; HEADER_SIZE];
        b[..8].copy_from_slice(b"HLDEMO\0\0");
        b[8..12].copy_from_slice(&DEMO_PROTOCOL.to_le_bytes());
        b[12..16].copy_from_slice(&48i32.to_le_bytes());
        b[16..23].copy_from_slice(b"dm_test");
        b[540..544].copy_from_slice(&(HEADER_SIZE as i32).to_le_bytes());
        b.extend_from_slice(&1i32.to_le_bytes());
        let mut e = vec![0u8; ENTRY_SIZE];
        e[..4].copy_from_slice(&1i32.to_le_bytes());
        e[4..12].copy_from_slice(b"Playback");
        e[84..88].copy_from_slice(&(start as i32).to_le_bytes());
        e[88..92].copy_from_slice(&(body.len() as i32).to_le_bytes());
        b.extend_from_slice(&e);
        b.extend_from_slice(&body);
        b
    }

    #[test]
    fn the_recording_players_own_shots_come_from_the_events_its_client_played() {
        let mut event = Vec::new();
        event.extend_from_slice(&0i32.to_le_bytes());
        event.extend_from_slice(&7i32.to_le_bytes());
        event.extend_from_slice(&0.1f32.to_le_bytes());
        let mut args = [0u8; 72];
        args[4..8].copy_from_slice(&1i32.to_le_bytes());
        args[8..12].copy_from_slice(&64.0f32.to_le_bytes());
        args[68..72].copy_from_slice(&1i32.to_le_bytes());
        event.extend_from_slice(&args);
        let recordings = read(&demo(&[(6, 12.5, &event), (5, 13.0, &[])])).unwrap();
        let shots = &recordings[0].shots;
        assert_eq!(shots.len(), 1);
        let s = &shots[0];
        assert_eq!((s.event.as_str(), s.entity), ("#7", 1));
        assert!((s.time - 12.6).abs() < 1e-4, "{}", s.time);
        assert_eq!((s.origin[0], s.bparam1, s.bparam2), (64.0, 0, 1));
    }

    #[test]
    fn the_map_crc_comes_out_the_same_whatever_slot_it_was_sent_to() {
        // levels.bsp, sent to slots 5 and 3.
        assert_eq!(unmunge_crc(0x59d9_cbf9, 5), 0x5993_99a3);
        assert_eq!(unmunge_crc(0x5fd9_cbff, 3), 0x5993_99a3);
    }
}
