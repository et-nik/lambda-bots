//! Recording a map's session and replaying it.
//!
//! A recording starts with a map (`lb record start`, then a map change or `restart`): the runtime carries little
//! from one map to the next, so a snapshot of that (`Carried`), the files the runtime reads and the map's
//! navigation files are enough to rebuild it. Every call the adapter then makes into the core is kept as a step:
//! what came in (the frame input, the command), every answer the host gave while the core ran, and what the runtime
//! took from outside the engine (the navigation loader finishing, telemetry commands). A replay runs the steps
//! through a fresh runtime whose host answers from the recording, and compares what the bots decide with what they
//! decided.
//!
//! File: `LBREC\0\r\n`, the format version (u32 LE), then blocks: the compressed length (u32 LE) and an lz4 block of
//! postcard-encoded `Record`s. `Start` comes first, `Step`s follow, `End` closes the recording; one cut short by a
//! crash replays up to where it stops.

use std::collections::VecDeque;
use std::fs::File;
use std::hash::Hasher;
use std::io::{BufReader, BufWriter, ErrorKind, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;

use lb_config::MainConfig;
use lb_core::handles::MapEpoch;
use lb_core::rng::Pcg32;
use lb_host::Host;
use lb_host::record::{FrameRec, HostCall, REDACTED, ReplayHost};
use lb_host::strings::StringTable;
use lb_styles::PersonaSource;
use rustc_hash::FxHasher;
use serde::{Deserialize, Serialize};

use crate::cvars::{Cv, Cvars};
use crate::roster::RosterFilter;
use crate::{CORE_VERSION, InitData, Runtime, commands, nav, panic_message};

pub const MAGIC: &[u8; 8] = b"LBREC\0\r\n";
pub const FORMAT: u32 = 4;
/// Uncompressed bytes gathered before a block goes to the writer thread.
const BLOCK: usize = 1 << 20;
/// A recording stops by itself past this much uncompressed data.
const MAX_BYTES: u64 = 4 << 30;
/// The largest block a reader accepts, packed or unpacked: far above `BLOCK` and a `Start` with a map's files.
const MAX_BLOCK: usize = 256 << 20;
/// The largest record the writer takes: after the records before it in its block, and packed (which may grow it by a
/// tenth), it still fits `MAX_BLOCK`.
const MAX_RECORD: usize = MAX_BLOCK / 8 * 7 - BLOCK;
const _: () = assert!(lz4_flex::block::get_maximum_output_size(BLOCK + MAX_RECORD) + 4 <= MAX_BLOCK);

/// Something the runtime took from outside the engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Outside {
    /// The navigation loader was polled: what it had loaded (`nav::summary`), `None` while it was still working.
    NavPoll(Option<String>),
    /// Commands accepted from the telemetry command channel.
    Channel(Vec<String>),
    /// The chat worker's replies.
    Chat(Vec<lb_chat::Reply>),
}

#[derive(Debug, Default)]
pub enum OutsideMode {
    #[default]
    Live,
    Record(Vec<Outside>),
    Replay(Script),
}

/// The outside inputs of one replayed step, in the order they were taken.
#[derive(Debug, Default)]
pub struct Script {
    events: VecDeque<Outside>,
    /// The runtime wanted an input the recording does not have at this point.
    pub broken: Option<String>,
}

impl Script {
    pub fn new(events: Vec<Outside>) -> Script {
        Script {
            events: events.into(),
            broken: None,
        }
    }

    pub fn left(&self) -> usize {
        self.events.len()
    }

    fn next(&mut self) -> Option<Outside> {
        if self.broken.is_some() {
            None
        } else {
            self.events.pop_front()
        }
    }

    /// The commands the channel delivered at this point, if any.
    fn channel(&mut self) -> Vec<String> {
        if let Some(Outside::Channel(_)) = self.events.front()
            && let Some(Outside::Channel(lines)) = self.events.pop_front()
        {
            return lines;
        }
        Vec::new()
    }

    /// The chat replies that came in at this point, if any.
    fn chat(&mut self) -> Vec<lb_chat::Reply> {
        if let Some(Outside::Chat(_)) = self.events.front()
            && let Some(Outside::Chat(replies)) = self.events.pop_front()
        {
            return replies;
        }
        Vec::new()
    }

    fn fail(&mut self, why: String) {
        self.broken.get_or_insert(why);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RecordRequest {
    /// Record the next map, for `seconds` of it or until it ends.
    Start {
        seconds: Option<f64>,
    },
    Stop,
}

/// What the runtime carries from one map to the next: what a recording has to start from.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Carried {
    pub config: MainConfig,
    pub cvars: Cvars,
    pub strings: StringTable,
    pub carry_over: Vec<String>,
    pub requested: Vec<String>,
    pub roster_warned: bool,
    pub rng: Pcg32,
    pub master_seed: u64,
    pub safe_mode: Option<String>,
    pub dev: bool,
    pub freeze: bool,
    pub game_mode_forced: i32,
    pub chat: lb_chat::Carry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Root {
    Install,
    Game,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Start {
    pub core_version: String,
    pub abi_version: u32,
    pub map: String,
    /// When the recording began (Unix seconds), for people.
    pub unix_time: u64,
    pub init: InitData,
    /// What the host answered while the runtime was created.
    pub init_calls: Vec<HostCall>,
    pub carried: Carried,
    /// The files the runtime reads (config, profiles, names) and the map's navigation files, by root.
    pub files: Vec<(Root, String, Vec<u8>)>,
    /// What the runtime made of its files; a replay reading different ones warns.
    pub fingerprint: u64,
}

/// A call of the adapter into the core.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Entry {
    MapStart {
        map: String,
        max_clients: u32,
        epoch: u32,
        late_load: bool,
    },
    MapEnd,
    FramePre(FrameRec),
    FramePost {
        mono_ns: u64,
    },
    /// `lb ...` from the server console, the whole argument list.
    Command(Vec<String>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    pub entry: Entry,
    pub calls: Vec<HostCall>,
    pub outside: Vec<Outside>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct End {
    pub reason: String,
    pub steps: u64,
    pub frames: u64,
    pub seconds: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Record {
    Start(Box<Start>),
    Step(Step),
    End(End),
}

impl Runtime {
    /// What the runtime carries, the telemetry secret and the chat key replaced by [`REDACTED`]: a replay opens no
    /// sockets.
    pub fn carried(&self) -> Carried {
        let mut config = self.config.clone();
        if !config.telemetry.secret.is_empty() {
            config.telemetry.secret = REDACTED.into();
        }
        config.chat.redact();
        let mut cvars = self.cvars.clone();
        cvars.redact(Cv::TelemetrySecret, REDACTED);
        Carried {
            config,
            cvars,
            strings: self.strings.clone(),
            carry_over: self.carry_over.clone(),
            requested: self.requested.clone(),
            roster_warned: self.roster_warned,
            rng: self.rng.clone(),
            master_seed: self.master_seed,
            safe_mode: self.safe_mode.clone(),
            dev: self.dev,
            freeze: self.freeze,
            game_mode_forced: self.game_mode_forced,
            chat: self.chat.carry(),
        }
    }

    pub fn restore(&mut self, c: Carried) {
        self.filter = RosterFilter::from_config(&c.config.roster);
        self.config = c.config;
        self.cvars = c.cvars;
        self.strings = c.strings;
        self.carry_over = c.carry_over;
        self.requested = c.requested;
        self.roster_warned = c.roster_warned;
        self.rng = c.rng;
        self.master_seed = c.master_seed;
        self.safe_mode = c.safe_mode;
        self.dev = c.dev;
        self.freeze = c.freeze;
        self.game_mode_forced = c.game_mode_forced;
        self.chat.restore(c.chat);
    }

    /// A hash of what the runtime made of its files (personalities, skill table, styles, names).
    pub fn fingerprint(&self) -> u64 {
        let mut h = FxHasher::default();
        for p in self.roster.iter() {
            let mut p = (**p).clone();
            p.source = PersonaSource::Unsaved;
            h.write(format!("{p:?}").as_bytes());
        }
        h.write(format!("{:?}{:?}{:?}", self.presets, self.styles, self.names.names()).as_bytes());
        h.finish()
    }

    /// The navigation loader's result once it is done; a replay takes it at the frame the recording did.
    pub(crate) fn poll_nav_loader(&mut self) -> Option<Result<nav::LoadedMap, String>> {
        let loader = self.nav_loader.as_ref()?;
        match &mut self.outside {
            OutsideMode::Live => loader.poll(),
            OutsideMode::Record(log) => {
                let result = loader.poll();
                log.push(Outside::NavPoll(result.as_ref().map(nav::summary)));
                result
            }
            OutsideMode::Replay(script) => match script.next() {
                Some(Outside::NavPoll(None)) => None,
                Some(Outside::NavPoll(Some(recorded))) => {
                    let result = loader.wait();
                    let replayed = nav::summary(&result);
                    if replayed != recorded {
                        script.fail(format!("the map loaded as `{replayed}`, recorded as `{recorded}`"));
                    }
                    Some(result)
                }
                other => {
                    script.fail(format!(
                        "the runtime polled the navigation loader, the recording has {other:?}"
                    ));
                    None
                }
            },
        }
    }

    /// The chat worker's replies, taken once at the start of every `frame_post`; a replay takes them from the
    /// recording.
    pub(crate) fn chat_replies(&mut self) -> Vec<lb_chat::Reply> {
        if let OutsideMode::Replay(script) = &mut self.outside {
            return script.chat();
        }
        let replies = self.chat.backend.poll();
        if let OutsideMode::Record(log) = &mut self.outside
            && !replies.is_empty()
        {
            log.push(Outside::Chat(replies.clone()));
        }
        replies
    }

    /// Commands from the telemetry command channel.
    pub(crate) fn channel_commands(&mut self) -> Vec<String> {
        if let OutsideMode::Replay(script) = &mut self.outside {
            return script.channel();
        }
        let lines = self.read_command_channel();
        if let OutsideMode::Record(log) = &mut self.outside
            && !lines.is_empty()
        {
            log.push(Outside::Channel(lines.clone()));
        }
        lines
    }
}

/// Runs one recorded call of the adapter against the runtime.
pub fn run_step(rt: &mut Runtime, host: &mut dyn Host, entry: &Entry) {
    match entry {
        Entry::MapStart {
            map,
            max_clients,
            epoch,
            late_load,
        } => rt.map_start(host, map, *max_clients, MapEpoch(*epoch), *late_load),
        Entry::MapEnd => rt.map_end(host),
        Entry::FramePre(frame) => {
            if let Some((frame, malformed)) = frame.decode(&mut rt.strings) {
                rt.frame_pre(host, frame, malformed);
            }
        }
        Entry::FramePost { mono_ns } => rt.frame_post(host, *mono_ns),
        Entry::Command(argv) => {
            let args: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
            for line in commands::execute(rt, host, &args) {
                host.server_print(&format!("{line}\n"));
            }
        }
    }
}

/// Compresses and writes blocks of records on a thread of its own.
struct Writer {
    block: Vec<u8>,
    tx: Option<Sender<Vec<u8>>>,
    thread: Option<JoinHandle<Result<u64, String>>>,
    /// Uncompressed bytes written so far.
    bytes: u64,
}

impl Writer {
    fn create(path: &Path) -> Result<Writer, String> {
        let io = |e: std::io::Error| format!("{}: {e}", path.display());
        let mut file = BufWriter::new(File::create(path).map_err(io)?);
        file.write_all(MAGIC).map_err(io)?;
        file.write_all(&FORMAT.to_le_bytes()).map_err(io)?;
        let (tx, rx) = channel::<Vec<u8>>();
        let name = path.display().to_string();
        let thread = std::thread::Builder::new()
            .name("lb-record".into())
            .spawn(move || {
                let mut written = (MAGIC.len() + 4) as u64;
                for block in rx {
                    let packed = lz4_flex::compress_prepend_size(&block);
                    file.write_all(&(packed.len() as u32).to_le_bytes())
                        .and_then(|()| file.write_all(&packed))
                        .map_err(|e| format!("{name}: {e}"))?;
                    written += 4 + packed.len() as u64;
                }
                file.flush().map_err(|e| format!("{name}: {e}"))?;
                Ok(written)
            })
            .map_err(|e| e.to_string())?;
        Ok(Writer {
            block: Vec::with_capacity(BLOCK + BLOCK / 4),
            tx: Some(tx),
            thread: Some(thread),
            bytes: 0,
        })
    }

    fn write(&mut self, record: &Record) -> Result<(), String> {
        let before = self.block.len();
        self.block = postcard::to_extend(record, std::mem::take(&mut self.block)).map_err(|e| e.to_string())?;
        let size = self.block.len() - before;
        if size > MAX_RECORD {
            self.block.truncate(before);
            return Err(format!(
                "a record of {} MiB, more than a replay reads ({} MiB)",
                size >> 20,
                MAX_RECORD >> 20
            ));
        }
        self.bytes += size as u64;
        if self.block.len() >= BLOCK {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        if self.block.is_empty() {
            return Ok(());
        }
        let block = std::mem::replace(&mut self.block, Vec::with_capacity(BLOCK + BLOCK / 4));
        self.tx
            .as_ref()
            .and_then(|tx| tx.send(block).ok())
            .ok_or_else(|| "the recording writer stopped".to_string())
    }

    /// Writes what is left and waits for the file to be complete; returns its size.
    fn close(mut self) -> Result<u64, String> {
        let flushed = self.flush();
        drop(self.tx.take());
        let written = match self.thread.take() {
            Some(t) => t.join().unwrap_or_else(|_| Err("the recording writer panicked".into())),
            None => Ok(0),
        };
        flushed.and(written)
    }
}

struct Active {
    path: PathBuf,
    map: String,
    writer: Writer,
    seconds: Option<f64>,
    /// Sim time of the first and the latest frame.
    began: Option<f64>,
    now: f64,
    steps: u64,
    frames: u64,
}

enum State {
    Off,
    Armed { seconds: Option<f64> },
    On(Box<Active>),
}

/// Writes recordings: waits for a map to start one, then keeps every step until stopped.
pub struct Recorder {
    init_calls: Vec<HostCall>,
    /// Host calls of the step in progress; `RecordingHost` writes them here.
    calls: Vec<HostCall>,
    pending: Option<Entry>,
    state: State,
}

impl Recorder {
    /// `init_calls`: what the host answered while the runtime was created.
    pub fn new(init_calls: Vec<HostCall>) -> Recorder {
        Recorder {
            init_calls,
            calls: Vec::new(),
            pending: None,
            state: State::Off,
        }
    }

    pub fn on(&self) -> bool {
        matches!(self.state, State::On(_))
    }

    /// Starts the recording asked for, as a map is about to start.
    pub fn map_starting(&mut self, rt: &mut Runtime, map: &str) {
        let State::Armed { seconds } = self.state else { return };
        self.state = match open(rt, map, &self.init_calls, seconds) {
            Ok(active) => {
                tracing::info!("recording {map} to {}", active.path.display());
                rt.outside = OutsideMode::Record(Vec::new());
                State::On(Box::new(active))
            }
            Err(e) => {
                tracing::warn!("cannot record {map}: {e}");
                State::Off
            }
        };
        rt.record_status = self.status();
    }

    /// Begins a step, the adapter's call `entry`; returns where the host calls go while recording.
    pub fn begin(&mut self, entry: impl FnOnce() -> Entry) -> Option<&mut Vec<HostCall>> {
        if !self.on() {
            return None;
        }
        self.calls.clear();
        self.pending = Some(entry());
        Some(&mut self.calls)
    }

    /// Ends the step begun last: writes it, then handles `lb record` and the recording's limits.
    pub fn end(&mut self, rt: &mut Runtime) {
        let mut stop = None;
        if let (Some(entry), State::On(active)) = (self.pending.take(), &mut self.state) {
            let after_frame = matches!(entry, Entry::FramePost { .. });
            if let Entry::FramePre(frame) = &entry
                && let Some(h) = frame.header()
            {
                active.frames += 1;
                active.now = h.sim_time;
                active.began.get_or_insert(h.sim_time);
            }
            let outside = match &mut rt.outside {
                OutsideMode::Record(log) => std::mem::take(log),
                _ => Vec::new(),
            };
            let step = Step {
                entry,
                calls: std::mem::take(&mut self.calls),
                outside,
            };
            active.steps += 1;
            if let Err(e) = active.writer.write(&Record::Step(step)) {
                stop = Some(format!("write failed: {e}"));
            } else if active.writer.bytes > MAX_BYTES {
                stop = Some(format!("{} GiB recorded", MAX_BYTES >> 30));
            } else if after_frame
                && let (Some(limit), Some(began)) = (active.seconds, active.began)
                && active.now - began >= limit
            {
                stop = Some(format!("{limit} s recorded"));
            }
        }
        if let Some(reason) = stop {
            self.finish(rt, &reason);
        }
        if let Some(request) = rt.record_request.take() {
            match request {
                RecordRequest::Start { seconds } => {
                    if !self.on() {
                        self.state = State::Armed { seconds };
                    }
                }
                RecordRequest::Stop => match self.state {
                    State::On(_) => self.finish(rt, "stopped"),
                    _ => self.state = State::Off,
                },
            }
        }
        rt.record_status = self.status();
    }

    /// The core panicked in the step begun last: keeps what it did up to the panic and closes the recording, so a
    /// replay runs into the same panic.
    pub fn crashed(&mut self, rt: &mut Runtime, why: &str) {
        if !self.on() {
            return;
        }
        if let (Some(entry), State::On(active)) = (self.pending.take(), &mut self.state) {
            let outside = match &mut rt.outside {
                OutsideMode::Record(log) => std::mem::take(log),
                _ => Vec::new(),
            };
            let step = Step {
                entry,
                calls: std::mem::take(&mut self.calls),
                outside,
            };
            active.steps += 1;
            let _ = active.writer.write(&Record::Step(step));
        }
        self.finish(rt, &format!("core panic: {why}"));
    }

    /// Closes the recording in progress; one waiting for its map stays armed.
    pub fn finish(&mut self, rt: &mut Runtime, reason: &str) {
        self.pending = None;
        self.calls.clear();
        if !self.on() {
            return;
        }
        if let State::On(active) = std::mem::replace(&mut self.state, State::Off) {
            let Active {
                path,
                mut writer,
                began,
                now,
                steps,
                frames,
                ..
            } = *active;
            let seconds = began.map_or(0.0, |b| now - b);
            let end = End {
                reason: reason.to_string(),
                steps,
                frames,
                seconds,
            };
            let ended = writer.write(&Record::End(end));
            match ended.and(writer.close()) {
                Ok(size) => tracing::info!(
                    "recording saved to {} ({reason}): {frames} frames, {seconds:.0} s, {:.1} MB",
                    path.display(),
                    size as f64 / 1e6
                ),
                Err(e) => tracing::warn!("recording {} may be incomplete: {e}", path.display()),
            }
            rt.outside = OutsideMode::Live;
        }
        rt.record_status = self.status();
    }

    pub fn status(&self) -> String {
        match &self.state {
            State::Off => "not recording".into(),
            State::Armed { seconds } => format!(
                "recording starts with the next map (changelevel or restart){}",
                seconds.map(|s| format!(", for {s} s")).unwrap_or_default()
            ),
            State::On(a) => format!(
                "recording {} to {}: {} frames, {:.0} s, {:.1} MB uncompressed{}",
                a.map,
                a.path.display(),
                a.frames,
                a.began.map_or(0.0, |b| a.now - b),
                a.writer.bytes as f64 / 1e6,
                a.seconds.map(|s| format!(", stops at {s} s")).unwrap_or_default()
            ),
        }
    }
}

fn open(rt: &Runtime, map: &str, init_calls: &[HostCall], seconds: Option<f64>) -> Result<Active, String> {
    let dir = rt.init.install_dir.join("records");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{map}-{}.lbrec", crate::roster::stamp()));
    let mut writer = Writer::create(&path)?;
    let start = Start {
        core_version: CORE_VERSION.to_string(),
        abi_version: lb_ffi::LB_ABI_VERSION,
        map: map.to_string(),
        unix_time: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        init: rt.init.clone(),
        init_calls: init_calls.to_vec(),
        carried: rt.carried(),
        files: gather_files(&rt.init, map),
        fingerprint: rt.fingerprint(),
    };
    writer.write(&Record::Start(Box::new(start)))?;
    Ok(Active {
        path,
        map: map.to_string(),
        writer,
        seconds,
        began: None,
        now: 0.0,
        steps: 0,
        frames: 0,
    })
}

/// The files a replay needs: what the runtime reads from the install directory and the map's navigation files.
fn gather_files(init: &InitData, map: &str) -> Vec<(Root, String, Vec<u8>)> {
    let mut files = Vec::new();
    let install = &init.install_dir;
    let config = install.join("config");
    let main_config = config.join("lambdabots.yaml");
    // A link to the main config elsewhere (under `profiles/`, say) is the main config too.
    let main_real = std::fs::canonicalize(&main_config).ok();
    let is_main = |path: &Path| {
        path == main_config
            || main_real
                .as_ref()
                .is_some_and(|m| std::fs::canonicalize(path).is_ok_and(|p| p == *m))
    };
    let mut add = |root: Root, rel: String, path: &Path| match std::fs::read(path) {
        Ok(bytes) if is_main(path) => match scrub_config(&bytes) {
            Some(bytes) => files.push((root, rel, bytes)),
            None => tracing::warn!("recording without {}: it does not parse", path.display()),
        },
        Ok(bytes) => files.push((root, rel, bytes)),
        Err(e) => tracing::warn!("recording without {}: {e}", path.display()),
    };
    // Only what the runtime reads from `config/`: a copy of the main config there would keep the secret.
    let mut paths = vec![main_config.clone(), config.join("difficulty.yaml")];
    paths.extend(lb_config::check::yaml_files(&config.join("styles")));
    for dir in ["profiles", "names"] {
        walk(&install.join(dir), &mut paths);
    }
    for path in paths {
        if path.is_file()
            && let Ok(rel) = path.strip_prefix(install)
        {
            add(Root::Install, rel.to_string_lossy().into_owned(), &path);
        }
    }
    let generated = install.join("data").join("profiles.yaml");
    if generated.is_file() {
        add(Root::Install, "data/profiles.yaml".into(), &generated);
    }
    let experience = crate::learned::experience_file(install, map);
    if experience.is_file() {
        add(Root::Install, format!("data/experience/{map}.json"), &experience);
    }
    let respawns = crate::learned::respawn_file(install);
    if respawns.is_file() {
        add(Root::Install, "data/learned/respawn.json".into(), &respawns);
    }
    let map_files = nav::map_files(&init.game_dir, install, map);
    if let Some(bsp) = &map_files.bsp {
        add(Root::Game, format!("maps/{map}.bsp"), bsp);
    }
    if let Some(graph) = &map_files.graph {
        let (root, rel) = match graph.strip_prefix(install) {
            Ok(rel) => (Root::Install, rel.to_string_lossy().into_owned()),
            Err(_) => (Root::Game, format!("addons/yapb/data/graph/{map}.graph")),
        };
        add(root, rel, graph);
    }
    for name in nav::OVERLAYS {
        let overlay = install.join("maps").join(map).join(name);
        if overlay.is_file() {
            add(Root::Install, format!("maps/{map}/{name}"), &overlay);
        }
    }
    files
}

/// The main config with the telemetry secret and the chat key replaced by [`REDACTED`]; `None` when it does not
/// parse, as the runtime then does not read it either.
fn scrub_config(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut config = MainConfig::parse(std::str::from_utf8(bytes).ok()?, "").ok()?;
    if !config.telemetry.secret.is_empty() {
        config.telemetry.secret = REDACTED.into();
    }
    config.chat.redact();
    lb_config::yaml::to_string(&config).ok().map(String::into_bytes)
}

/// Whether a recorded path (a file's, a map's name) stays under its root: plain names only, no `..`, `.`, root or
/// drive.
fn stays_inside(rel: &str) -> bool {
    let mut parts = Path::new(rel).components().peekable();
    parts.peek().is_some() && parts.all(|c| matches!(c, Component::Normal(_)))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Reads the records of a recording in order.
pub struct Reader {
    file: BufReader<File>,
    block: Vec<u8>,
    pos: usize,
}

impl Reader {
    pub fn open(path: &Path) -> Result<Reader, String> {
        let mut file = BufReader::new(File::open(path).map_err(|e| format!("{}: {e}", path.display()))?);
        let mut head = [0u8; 12];
        file.read_exact(&mut head)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if &head[..8] != MAGIC {
            return Err(format!("{} is not a lambdabots recording", path.display()));
        }
        let format = u32::from_le_bytes([head[8], head[9], head[10], head[11]]);
        if format != FORMAT {
            return Err(format!(
                "{}: recording format {format}, this build reads {FORMAT}",
                path.display()
            ));
        }
        Ok(Reader {
            file,
            block: Vec::new(),
            pos: 0,
        })
    }

    /// The next record; `None` where the file ends (a recording cut short ends without `End`).
    pub fn next_record(&mut self) -> Result<Option<Record>, String> {
        while self.pos >= self.block.len() {
            let mut len = [0u8; 4];
            match self.file.read_exact(&mut len) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(None),
                Err(e) => return Err(e.to_string()),
            }
            let len = u32::from_le_bytes(len) as usize;
            if len > MAX_BLOCK {
                return Err(format!("damaged recording: a block of {len} bytes"));
            }
            let mut packed = vec![0u8; len];
            if self.file.read_exact(&mut packed).is_err() {
                return Ok(None);
            }
            let (size, _) = lz4_flex::block::uncompressed_size(&packed).map_err(|e| e.to_string())?;
            if size > MAX_BLOCK {
                return Err(format!("damaged recording: a block of {size} bytes unpacked"));
            }
            self.block = lz4_flex::decompress_size_prepended(&packed).map_err(|e| e.to_string())?;
            self.pos = 0;
        }
        let (record, rest) = postcard::take_from_bytes::<Record>(&self.block[self.pos..]).map_err(|e| e.to_string())?;
        self.pos = self.block.len() - rest.len();
        Ok(Some(record))
    }
}

#[derive(Clone, Debug, Default)]
pub struct ReplayOptions {
    /// Where to make a new directory for the recorded files, touching nothing else there; the system's temporary
    /// one by default.
    pub dir: Option<PathBuf>,
    /// Keep the unpacked files (and the replay's logs) afterwards.
    pub keep_dir: bool,
    /// How many differing decisions to list.
    pub max_diffs: usize,
}

#[derive(Debug, Default)]
pub struct ReplayReport {
    pub map: String,
    pub recorded_by: String,
    pub steps: u64,
    pub frames: u64,
    /// Bot commands compared with the recorded ones.
    pub commands: u64,
    pub seconds: f64,
    /// The first differing decisions, with the frame they came in.
    pub diffs: Vec<(u64, String)>,
    pub diff_count: u64,
    /// Where the replay stopped following the recording, and at which frame.
    pub broken: Option<(u64, String)>,
    /// The recording ended with a core panic and the replay ran into it too.
    pub panic_reproduced: Option<String>,
    pub end: Option<End>,
    pub warnings: Vec<String>,
    pub dir: PathBuf,
}

impl ReplayReport {
    pub fn matches(&self) -> bool {
        self.broken.is_none() && self.diff_count == 0
    }

    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!(
            "{}: {} steps, {} frames, {:.1} s of play, {} bot commands compared (recorded by core {})",
            self.map, self.steps, self.frames, self.seconds, self.commands, self.recorded_by
        )];
        out.extend(self.warnings.iter().map(|w| format!("warning: {w}")));
        match &self.end {
            Some(end) => out.push(format!(
                "recording ended: {} ({} steps, {} frames)",
                end.reason, end.steps, end.frames
            )),
            None => out.push("the recording has no end record: it was cut short".into()),
        }
        if let Some(p) = &self.panic_reproduced {
            out.push(format!("the recorded core panic happened again: {p}"));
        }
        for (frame, d) in &self.diffs {
            out.push(format!("frame {frame}: {d}"));
        }
        if self.diff_count > self.diffs.len() as u64 {
            out.push(format!("... {} more", self.diff_count - self.diffs.len() as u64));
        }
        if let Some((frame, why)) = &self.broken {
            out.push(format!("diverged at frame {frame}: {why}"));
        }
        out.push(if self.matches() {
            "replay matches the recording: every bot command is the same".into()
        } else {
            format!("replay differs: {} decisions differ", self.diff_count)
        });
        out
    }
}

/// A new, empty directory under `base` for one replay.
fn fresh_dir(base: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(base).map_err(|e| format!("{}: {e}", base.display()))?;
    let mut n = 0u32;
    loop {
        let dir = base.join(format!("lb-replay-{}-{n}", std::process::id()));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == ErrorKind::AlreadyExists && n < 1000 => n += 1,
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        }
    }
}

/// Removes a replay's directory when the replay returns, whichever way, unless it is to be kept.
struct Unpacked<'a> {
    dir: &'a Path,
    keep: bool,
}

impl Drop for Unpacked<'_> {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(self.dir);
        }
    }
}

/// Replays a recording through a fresh runtime; `console` gets the core's console output.
pub fn replay(path: &Path, opts: &ReplayOptions, console: &mut dyn FnMut(&str)) -> Result<ReplayReport, String> {
    let mut reader = Reader::open(path)?;
    let Some(Record::Start(start)) = reader.next_record()? else {
        return Err(format!("{} does not begin with a start record", path.display()));
    };
    if start.abi_version != lb_ffi::LB_ABI_VERSION {
        return Err(format!(
            "recorded with ABI {}, this build has ABI {}",
            start.abi_version,
            lb_ffi::LB_ABI_VERSION
        ));
    }
    if let Some((_, rel, _)) = start.files.iter().find(|(_, rel, _)| !stays_inside(rel)) {
        return Err(format!(
            "{}: the recorded file {rel:?} is outside its directory",
            path.display()
        ));
    }
    let dir = fresh_dir(&opts.dir.clone().unwrap_or_else(std::env::temp_dir))?;
    // Made before the runtime, so it goes after it: the runtime lets go of its files first.
    let _unpacked = Unpacked {
        dir: &dir,
        keep: opts.keep_dir,
    };
    for (root, rel, bytes) in &start.files {
        let path = dir.join(match root {
            Root::Install => "install",
            Root::Game => "game",
        });
        let path = path.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let mut report = ReplayReport {
        map: start.map.clone(),
        recorded_by: start.core_version.clone(),
        dir: dir.clone(),
        ..ReplayReport::default()
    };
    if start.core_version != CORE_VERSION {
        report.warnings.push(format!(
            "replayed by core {CORE_VERSION}: expect differences wherever the code changed"
        ));
    }
    let init = InitData {
        install_dir: dir.join("install"),
        game_dir: dir.join("game"),
        sandbox: true,
        ..start.init.clone()
    };
    let mut host = ReplayHost::new(start.init_calls.clone());
    host.console = Some(Vec::new());
    let mut rt = Runtime::new(&mut host, init);
    if let Some(b) = host.broken.take() {
        return Err(format!("the runtime started differently: {b}"));
    }
    let fingerprint = start.fingerprint;
    rt.restore(start.carried);
    if rt.fingerprint() != fingerprint {
        report.warnings.push(
            "the config, profile or name files differ from what the server had loaded: decisions may differ".into(),
        );
    }
    let mut frame_no = 0;
    let mut first_time = None;
    while let Some(record) = reader.next_record()? {
        let step = match record {
            Record::Start(_) => return Err("a second start record".into()),
            Record::End(end) => {
                report.end = Some(end);
                break;
            }
            Record::Step(step) => step,
        };
        if let Entry::MapStart { map, .. } = &step.entry
            && !stays_inside(map)
        {
            report.broken = Some((frame_no, format!("the recorded map {map:?} is outside its directory")));
            break;
        }
        report.steps += 1;
        if let Entry::FramePre(frame) = &step.entry
            && let Some(h) = frame.header()
        {
            frame_no = h.frame_no;
            report.frames += 1;
            report.seconds = h.sim_time - *first_time.get_or_insert(h.sim_time);
        }
        host.load(step.calls);
        rt.outside = OutsideMode::Replay(Script::new(step.outside));
        let ran = catch_unwind(AssertUnwindSafe(|| run_step(&mut rt, &mut host, &step.entry)));
        for line in host.console.as_mut().map(std::mem::take).unwrap_or_default() {
            console(&line);
        }
        for d in host.diffs.drain(..) {
            report.diff_count += 1;
            if report.diffs.len() < opts.max_diffs {
                report.diffs.push((frame_no, d));
            }
        }
        if let Err(payload) = ran {
            let msg = panic_message(&payload);
            match reader.next_record()? {
                Some(Record::End(end)) if end.reason.starts_with("core panic") => {
                    report.panic_reproduced = Some(msg);
                    report.end = Some(end);
                }
                _ => report.broken = Some((frame_no, format!("the core panicked: {msg}"))),
            }
            break;
        }
        let script = match std::mem::take(&mut rt.outside) {
            OutsideMode::Replay(script) => script,
            _ => Script::default(),
        };
        let broken = host
            .broken
            .take()
            .or_else(|| {
                host.left().next().map(|c| {
                    format!(
                        "the core made fewer host calls than recorded (next recorded: {}, {} left)",
                        c.name(),
                        host.left().count()
                    )
                })
            })
            .or(script.broken)
            .or_else(|| (!script.events.is_empty()).then(|| format!("unused outside inputs {:?}", script.events)));
        if let Some(why) = broken {
            report.broken = Some((frame_no, why));
            break;
        }
    }
    report.commands = host.commands_checked;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use lb_ffi::*;
    use lb_host::arena::encode::ArenaWriter;
    use lb_host::record::RecordingHost;
    use lb_host::*;

    use super::*;

    /// A server with a flat, empty world: bots join, stand where they spawned and see the other player.
    #[derive(Default)]
    struct SimServer {
        cvars: Vec<String>,
        joined: Vec<(u8, i32, String)>,
        announced: usize,
        /// The bots stand on the ground.
        ground: bool,
        /// The human in slot 7 is out of everyone's sight.
        hide_human: bool,
        /// The human joins the server (events) at this frame.
        human_joins: Option<u64>,
        /// The human's chat lines, by frame.
        says: Vec<(u64, String)>,
        /// The human's team chat lines, by frame.
        team_says: Vec<(u64, String)>,
        /// Client commands the bots sent.
        sent: Vec<Vec<String>>,
    }

    impl Host for SimServer {
        fn server_print(&mut self, _text: &str) {}
        fn client_print(&mut self, _slot: u8, _kind: PrintKind, _text: &str) {}
        fn server_command(&mut self, _command: &str) {}
        fn cvar_register(&mut self, specs: &[CvarSpec]) -> Vec<Option<CvarHandle>> {
            let first = self.cvars.len() as u16;
            self.cvars.extend(specs.iter().map(|s| s.default_value.clone()));
            (0..specs.len() as u16).map(|i| Some(CvarHandle(first + i))).collect()
        }
        fn cvar_find(&mut self, _name: &str) -> Option<CvarHandle> {
            None
        }
        fn cvar_float(&mut self, h: CvarHandle) -> f32 {
            self.cvar_string(h).parse().unwrap_or(0.0)
        }
        fn cvar_string(&mut self, h: CvarHandle) -> String {
            self.cvars.get(h.0 as usize).cloned().unwrap_or_default()
        }
        fn cvar_set(&mut self, h: CvarHandle, value: &str) {
            if let Some(v) = self.cvars.get_mut(h.0 as usize) {
                *v = value.to_string();
            }
        }
        fn trace(&mut self, req: &TraceRequest) -> TraceResult {
            let floor = req.end.z < 0.0 && req.start.z >= 0.0;
            let fraction = if floor {
                req.start.z / (req.start.z - req.end.z)
            } else {
                1.0
            };
            TraceResult {
                fraction,
                end_pos: req.start + (req.end - req.start) * fraction,
                plane_normal: if floor { lb_core::Vec3::Z } else { lb_core::Vec3::ZERO },
                in_open: true,
                ..TraceResult::default()
            }
        }
        fn point_contents(&mut self, p: lb_core::Vec3) -> i32 {
            if p.z < 0.0 { -2 } else { -1 }
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
            None
        }
        fn create_bot(&mut self, req: &CreateBotRequest) -> CreateBotOutcome {
            if self.joined.len() >= 2 {
                return CreateBotOutcome::ServerFull;
            }
            let slot = self.joined.len() as u8 + 1;
            let userid = 10 + i32::from(slot);
            self.joined.push((slot, userid, req.name.clone()));
            CreateBotOutcome::Created {
                slot,
                userid,
                bot_gen: 1,
            }
        }
        fn kick_bot(&mut self, _slot: u8, _bot_gen: u32, _reason: &str) -> bool {
            true
        }
        fn bot_client_command(&mut self, _slot: u8, _bot_gen: u32, argv: &[&str]) -> bool {
            self.sent.push(argv.iter().map(|a| a.to_string()).collect());
            true
        }
        fn run_player_moves(&mut self, cmds: &[LbBotCommand], feedback: &mut Vec<LbMoveFeedback>) -> bool {
            feedback.clear();
            for c in cmds {
                let mut fb = lb_host::record::from_pod::<LbMoveFeedback>(&[0; size_of::<LbMoveFeedback>()])[0];
                fb.slot = c.slot;
                fb.status = LB_MOVE_OK;
                fb.health = 100.0;
                fb.v_angle = c.view_angles;
                feedback.push(fb);
            }
            true
        }
        fn physics_key(&mut self, _slot: u8, _key: &str) -> String {
            String::new()
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

    fn zeroed<T: lb_host::record::Pod>() -> T {
        lb_host::record::from_pod::<T>(&vec![0; size_of::<T>()])[0]
    }

    impl SimServer {
        fn frame(&mut self, n: u64) -> Entry {
            let mut header: LbFrameHeader = zeroed();
            header.frame_no = n;
            header.sim_time = 1.0 + n as f64 * 0.01;
            header.frame_time = 0.01;
            header.max_clients = 8;
            header.flags = LB_FRAME_PRE;
            let mut arena = ArenaWriter::default();
            for (slot, userid, name) in &self.joined[self.announced..] {
                arena.client_gen(LB_CLIENT_EV_CONNECT, *slot, *userid, true, 1, name.as_bytes());
                arena.client_gen(LB_CLIENT_EV_PUT_IN_SERVER, *slot, *userid, true, 1, name.as_bytes());
            }
            self.announced = self.joined.len();
            if self.human_joins == Some(n) {
                arena.client(LB_CLIENT_EV_CONNECT, 7, 3, false, b"Gordon");
                arena.client(LB_CLIENT_EV_PUT_IN_SERVER, 7, 3, false, b"Gordon");
            }
            for (_, text) in self.says.iter().filter(|(at, _)| *at == n) {
                arena.command(7, 3, &[b"say", text.as_bytes()], text.as_bytes());
            }
            for (_, text) in self.team_says.iter().filter(|(at, _)| *at == n) {
                arena.command(7, 3, &[b"say_team", text.as_bytes()], text.as_bytes());
            }
            let mut clients = Vec::new();
            let mut selves = Vec::new();
            for (slot, userid, _) in &self.joined {
                let mut c: LbClientSnapshot = zeroed();
                (c.slot, c.state, c.is_fake, c.is_ours, c.userid) = (*slot, LB_CLIENT_SPAWNED, 1, 1, *userid);
                c.origin.x = f32::from(*slot) * 200.0;
                c.origin.z = 36.0;
                clients.push(c);
                let mut s: LbSelfSnapshot = zeroed();
                (s.slot, s.bot_gen, s.health, s.maxspeed) = (*slot, 1, 100.0, 270.0);
                if self.ground {
                    s.flags = lb_game::self_state::FL_ONGROUND;
                }
                s.origin = c.origin;
                s.view_ofs.z = 28.0;
                selves.push(s);
            }
            let mut human: LbClientSnapshot = zeroed();
            (human.slot, human.state, human.userid) = (7, LB_CLIENT_SPAWNED, 3);
            human.origin = LbVec3 {
                x: 500.0 + lb_core::dmath::sin(n as f32 * 0.05) * 300.0,
                y: 150.0,
                z: 36.0,
            };
            if !self.hide_human {
                clients.push(human);
            }
            Entry::FramePre(FrameRec::from_parts(header, &clients, &selves, &arena.bytes))
        }
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_recorded_session_replays_to_the_same_commands() {
        let root = dir("record-test");
        let init = InitData {
            adapter_version: "test".into(),
            plugin_path: root.join("plugin"),
            game_dir: root.join("game"),
            install_dir: root.join("install"),
            platform: 0,
            late_load: false,
            sandbox: true,
        };
        std::fs::create_dir_all(root.join("install/config")).unwrap();
        std::fs::write(
            root.join("install/config/lambdabots.yaml"),
            "schema: lambdabots/main@1\ntelemetry:\n  secret: hunter2-secret\n",
        )
        .unwrap();
        let mut server = SimServer::default();
        let mut init_calls = Vec::new();
        let mut rt = Runtime::new(&mut RecordingHost::new(&mut server, Some(&mut init_calls)), init);
        let mut rec = Recorder::new(init_calls);
        let step = |rt: &mut Runtime, rec: &mut Recorder, server: &mut SimServer, entry: Entry| {
            let secret = rt.cvars.handle_of(Cv::TelemetrySecret);
            let mut host = RecordingHost::new(server, rec.begin(|| entry.clone())).hiding(secret);
            run_step(rt, &mut host, &entry);
            rec.end(rt);
        };
        step(
            &mut rt,
            &mut rec,
            &mut server,
            Entry::Command(vec!["lb".into(), "record".into(), "start".into()]),
        );
        assert!(rt.record_status.starts_with("recording starts"), "{}", rt.record_status);
        step(&mut rt, &mut rec, &mut server, Entry::MapEnd);
        rec.finish(&mut rt, "the map ended");
        assert!(
            rt.record_status.starts_with("recording starts"),
            "still armed: {}",
            rt.record_status
        );
        let map = Entry::MapStart {
            map: "flatland".into(),
            max_clients: 8,
            epoch: 1,
            late_load: false,
        };
        rec.map_starting(&mut rt, "flatland");
        assert!(rec.on());
        step(&mut rt, &mut rec, &mut server, map);
        // Bots join 5 s into the map; at 7 s they are made quicker.
        for n in 0..1200 {
            if n == 600 {
                let h = rt.cvars.handle_of(Cv::TelemetrySecret).unwrap();
                server.cvars[h.0 as usize] = "hunter2-rotated".into();
            }
            if n == 700 {
                let h = rt.cvars.handle_of(Cv::Reflex).unwrap();
                server.cvars[h.0 as usize] = "1.5".into();
            }
            let frame = server.frame(n);
            step(&mut rt, &mut rec, &mut server, frame);
            step(
                &mut rt,
                &mut rec,
                &mut server,
                Entry::FramePost {
                    mono_ns: n * 10_000_000,
                },
            );
            if n == 900 {
                step(
                    &mut rt,
                    &mut rec,
                    &mut server,
                    Entry::Command(vec!["lb".into(), "list".into()]),
                );
            }
        }
        assert_eq!(rt.bots.len(), 2, "two bots joined");
        for b in &rt.bots {
            let base = b.persona.skill_params(&rt.presets);
            assert_eq!(b.character.skill, base.with_reflex(1.5), "{}", b.persona.name);
        }
        rec.finish(&mut rt, "test over");
        let file = std::fs::read_dir(root.join("install/records"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let mut reader = Reader::open(&file).unwrap();
        while let Some(record) = reader.next_record().unwrap() {
            let bytes = postcard::to_allocvec(&record).unwrap();
            assert!(
                !bytes.windows(8).any(|w| w == b"hunter2-"),
                "a telemetry secret is in the recording: {record:?}"
            );
        }

        let opts = ReplayOptions {
            dir: Some(root.join("replay")),
            max_diffs: 5,
            ..ReplayOptions::default()
        };
        std::fs::create_dir_all(root.join("replay")).unwrap();
        std::fs::write(root.join("replay/mine"), "").unwrap();
        let report = replay(&file, &opts, &mut |_| {}).unwrap();
        assert!(report.matches(), "{}", report.lines().join("\n"));
        assert!(report.dir.starts_with(root.join("replay")) && !report.dir.exists());
        assert!(root.join("replay/mine").exists(), "the given directory keeps its files");
        assert_eq!(
            (report.steps, report.frames),
            (2402, 1200),
            "{}",
            report.lines().join("\n")
        );
        assert!(report.commands > 1000, "{}", report.lines().join("\n"));
        assert_eq!(report.end.as_ref().map(|e| e.reason.as_str()), Some("test over"));

        // The same recording with another master seed: the bots decide differently, and the replay says so.
        let tampered = root.join("tampered.lbrec");
        let mut reader = Reader::open(&file).unwrap();
        let mut writer = Writer::create(&tampered).unwrap();
        while let Some(mut record) = reader.next_record().unwrap() {
            if let Record::Start(start) = &mut record {
                start.carried.master_seed ^= 1;
            }
            writer.write(&record).unwrap();
        }
        writer.close().unwrap();
        let report = replay(&tampered, &opts, &mut |_| {}).unwrap();
        assert!(
            !report.matches() && report.diff_count > 0,
            "{}",
            report.lines().join("\n")
        );
        assert!(report.diffs[0].1.contains("seed"), "{:?}", report.diffs[0]);

        let escaping = root.join("escaping.lbrec");
        let mut reader = Reader::open(&file).unwrap();
        let mut writer = Writer::create(&escaping).unwrap();
        while let Some(mut record) = reader.next_record().unwrap() {
            if let Record::Start(start) = &mut record {
                start.files.push((Root::Install, "../../escaped".into(), Vec::new()));
            }
            writer.write(&record).unwrap();
        }
        writer.close().unwrap();
        let err = replay(&escaping, &opts, &mut |_| {}).unwrap_err();
        assert!(err.contains("outside"), "{err}");
        assert!(!root.join("replay/escaped").exists());

        let escaping = root.join("escaping-map.lbrec");
        let mut reader = Reader::open(&file).unwrap();
        let mut writer = Writer::create(&escaping).unwrap();
        while let Some(mut record) = reader.next_record().unwrap() {
            if let Record::Step(Step {
                entry: Entry::MapStart { map, .. },
                ..
            }) = &mut record
            {
                *map = "x/../../../escaped".into();
            }
            writer.write(&record).unwrap();
        }
        writer.close().unwrap();
        let report = replay(&escaping, &opts, &mut |_| {}).unwrap();
        let broken = report.broken.as_ref().map_or("", |(_, why)| why.as_str());
        assert!(broken.contains("outside"), "{}", report.lines().join("\n"));

        // A replay that fails after unpacking takes its directory away too, unless it is kept.
        let twice = root.join("twice.lbrec");
        let mut reader = Reader::open(&file).unwrap();
        let mut writer = Writer::create(&twice).unwrap();
        let start = reader.next_record().unwrap().unwrap();
        writer.write(&start).unwrap();
        writer.write(&start).unwrap();
        writer.close().unwrap();
        let left = || -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(root.join("replay"))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };
        let err = replay(&twice, &opts, &mut |_| {}).unwrap_err();
        assert!(err.contains("second start"), "{err}");
        assert_eq!(left(), ["mine"]);
        let keep = ReplayOptions {
            keep_dir: true,
            ..opts.clone()
        };
        replay(&twice, &keep, &mut |_| {}).unwrap_err();
        assert_eq!(left().len(), 2, "{:?}", left());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_main_config_is_scrubbed_under_any_name_and_overlays_are_kept() {
        let root = dir("record-files");
        let install = root.join("install");
        for d in ["config", "profiles", "maps/flatland"] {
            std::fs::create_dir_all(install.join(d)).unwrap();
        }
        std::fs::write(
            install.join("config/lambdabots.yaml"),
            "schema: lambdabots/main@1\ntelemetry:\n  secret: hunter2-secret\n",
        )
        .unwrap();
        std::fs::write(install.join("maps/flatland/overlay.yaml"), "overlay").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            install.join("config/lambdabots.yaml"),
            install.join("profiles/alias.yaml"),
        )
        .unwrap();
        let init = InitData {
            adapter_version: "test".into(),
            plugin_path: root.join("plugin"),
            game_dir: root.join("game"),
            install_dir: install,
            platform: 0,
            late_load: false,
            sandbox: true,
        };
        let files = gather_files(&init, "flatland");
        let rels: Vec<&str> = files.iter().map(|(_, rel, _)| rel.as_str()).collect();
        assert!(rels.contains(&"maps/flatland/overlay.yaml"), "{rels:?}");
        assert!(!rels.contains(&"maps/flatland/editor.yaml"), "{rels:?}");
        #[cfg(unix)]
        assert!(rels.contains(&"profiles/alias.yaml"), "{rels:?}");
        for (_, rel, bytes) in &files {
            assert!(!bytes.windows(8).any(|w| w == b"hunter2-"), "the secret is in {rel}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A step of the adapter, kept while the recorder is on.
    fn step(rt: &mut Runtime, rec: &mut Recorder, server: &mut SimServer, entry: Entry) {
        let mut host = RecordingHost::new(server, rec.begin(|| entry.clone()));
        run_step(rt, &mut host, &entry);
        rec.end(rt);
    }

    /// `lb ...` from the server console.
    fn command(rt: &mut Runtime, rec: &mut Recorder, server: &mut SimServer, argv: &[&str]) {
        step(
            rt,
            rec,
            server,
            Entry::Command(argv.iter().map(|a| a.to_string()).collect()),
        );
    }

    /// Frames `range` of the map; `after` sees each one done.
    fn frames(
        rt: &mut Runtime,
        rec: &mut Recorder,
        server: &mut SimServer,
        range: std::ops::Range<u64>,
        mut after: impl FnMut(&mut Runtime, &mut Recorder, &mut SimServer, u64),
    ) {
        for n in range {
            let frame = server.frame(n);
            step(rt, rec, server, frame);
            step(
                rt,
                rec,
                server,
                Entry::FramePost {
                    mono_ns: n * 10_000_000,
                },
            );
            after(rt, rec, server, n);
        }
    }

    /// A server where one bot joins 5 s into each map and Gordon (slot 7, `userid` 3) at frame 20; chat on, the
    /// worker `fake`.
    fn chat_server(root: &Path, fake: crate::chat::FakeBackend) -> (Runtime, Recorder, SimServer) {
        std::fs::create_dir_all(root.join("install/config")).unwrap();
        std::fs::write(
            root.join("install/config/lambdabots.yaml"),
            "schema: lambdabots/main@1\nquota:\n  count: 1\n  mode: normal\nengine:\n  master_seed: 7\nchat:\n  enabled: \
             true\n  provider:\n    api_key: sk-hunter2-chat\n",
        )
        .unwrap();
        let init = InitData {
            adapter_version: "test".into(),
            plugin_path: root.join("plugin"),
            game_dir: root.join("game"),
            install_dir: root.join("install"),
            platform: 0,
            late_load: false,
            sandbox: true,
        };
        let mut server = SimServer {
            ground: true,
            hide_human: true,
            human_joins: Some(20),
            ..SimServer::default()
        };
        let mut init_calls = Vec::new();
        let mut rt = Runtime::new(&mut RecordingHost::new(&mut server, Some(&mut init_calls)), init);
        rt.chat.backend = Box::new(fake);
        (rt, Recorder::new(init_calls), server)
    }

    /// The map `flatland` starts as `epoch` on an empty server; with `record`, the recording starts with it.
    fn new_map(rt: &mut Runtime, rec: &mut Recorder, server: &mut SimServer, epoch: u32, record: bool) {
        if record {
            command(rt, rec, server, &["lb", "record", "start"]);
            rec.map_starting(rt, "flatland");
            assert!(rec.on());
        }
        server.joined.clear();
        server.announced = 0;
        server.says.clear();
        server.team_says.clear();
        let map = Entry::MapStart {
            map: "flatland".into(),
            max_clients: 8,
            epoch,
            late_load: false,
        };
        step(rt, rec, server, map);
    }

    /// The name the bot plays under.
    fn netname(rt: &Runtime) -> String {
        rt.clients
            .get(rt.bots[0].id.slot)
            .map(|c| c.name.clone())
            .unwrap_or_default()
    }

    /// Whether a bot said `text`.
    fn said(server: &SimServer, text: &str) -> bool {
        server.sent.iter().any(|a| a[0] == "say" && a[1] == text)
    }

    /// The recording made under `root`.
    fn recording(root: &Path) -> PathBuf {
        std::fs::read_dir(root.join("install/records"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
    }

    /// The recording made under `root` replays to the same commands, and writes no chat log; with the worker's
    /// replies saying another line, the replay tells.
    fn replays_and_tells_another_line(root: &Path) {
        let file = recording(root);
        let opts = ReplayOptions {
            dir: Some(root.join("replay")),
            keep_dir: true,
            max_diffs: 5,
        };
        let report = replay(&file, &opts, &mut |_| {}).unwrap();
        assert!(report.matches(), "{}", report.lines().join("\n"));
        let mut files = Vec::new();
        walk(&report.dir, &mut files);
        assert!(!files.is_empty());
        assert!(
            files.iter().all(|f| !f
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("chatlog"))),
            "a replay writes no chat log: {files:?}"
        );
        let tampered = root.join("tampered-chat.lbrec");
        let mut reader = Reader::open(&file).unwrap();
        let mut writer = Writer::create(&tampered).unwrap();
        let mut replies = 0;
        while let Some(mut record) = reader.next_record().unwrap() {
            if let Record::Step(step) = &mut record {
                for o in &mut step.outside {
                    if let Outside::Chat(chat) = o {
                        for r in chat.iter_mut() {
                            replies += 1;
                            r.outcome = lb_chat::Outcome::Line("ну и ладно".into());
                        }
                    }
                }
            }
            writer.write(&record).unwrap();
        }
        writer.close().unwrap();
        assert!(replies >= 1);
        let report = replay(&tampered, &opts, &mut |_| {}).unwrap();
        assert!(!report.matches(), "{}", report.lines().join("\n"));
    }

    #[test]
    fn chat_lines_replay_to_the_same_commands() {
        use lb_chat::{Outcome, Trigger};

        let root = dir("record-chat");
        let fake = crate::chat::FakeBackend::answering(&[Outcome::Line("ну привет".into())]);
        let sent = fake.sent.clone();
        let (mut rt, mut rec, mut server) = chat_server(&root, fake);
        rt.chat.chatlog = Some(crate::chat::ChatLog::new(root.join("chatlog")));
        new_map(&mut rt, &mut rec, &mut server, 1, true);
        let (mut named, mut stood) = (String::new(), 0);
        frames(&mut rt, &mut rec, &mut server, 0..1800, |rt, _, server, n| {
            if n == 800 {
                named = format!("{}, привет", netname(rt));
                server.says.push((850, named.clone()));
                server.team_says.push((860, "го на рельсы".into()));
                server.says.push((870, "/login secret".into()));
            }
            stood += usize::from(rt.bots.first().is_some_and(|b| b.chat.typing_in_the_open()));
        });
        let (bot, persona) = (netname(&rt), rt.bots[0].persona.name.clone());
        assert_eq!(rt.clients.humans(false), 1, "Gordon is on the server");
        assert!(said(&server, "ну привет"), "{:?}", server.sent);
        assert!(stood > 100, "the bot stood still while typing ({stood} frames)");
        assert!(
            sent.lock()
                .unwrap()
                .asked
                .iter()
                .any(|r| matches!(&r.trigger, Trigger::Addressed { text, .. } if *text == named)),
            "Gordon named the bot"
        );
        let chat: Vec<(bool, &str)> = rt
            .chat
            .journal
            .entries()
            .filter_map(|e| match &e.event {
                lb_chat::Event::Chat { from, text, .. } => Some((from.bot, text.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(
            chat,
            [(false, named.as_str()), (true, "ну привет")],
            "the bot's line is in the journal; a plugin's command and a team line nobody else sees are not"
        );
        step(&mut rt, &mut rec, &mut server, Entry::MapEnd);
        {
            let sent = sent.lock().unwrap();
            assert_eq!(sent.summaries.len(), 1, "the map's summary went to the memory");
            let gordon = &sent.summaries[0].players[0];
            assert_eq!(gordon.key, "name:gordon");
            let talk: Vec<(&str, bool, &str)> = gordon
                .talk
                .iter()
                .map(|(_, bot, mine, text)| (bot.as_str(), *mine, text.as_str()))
                .collect();
            assert_eq!(
                talk,
                [
                    (persona.as_str(), false, named.as_str()),
                    (persona.as_str(), true, "ну привет")
                ],
                "the talk went to the memory"
            );
        }
        assert!(
            rt.chat.social.unsaved(3, lb_core::time::SimTime::ZERO).is_empty(),
            "and is not sent again"
        );
        rec.finish(&mut rt, "test over");

        let mut logs: Vec<PathBuf> = std::fs::read_dir(root.join("chatlog"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        logs.sort();
        let text: String = logs.iter().map(|f| std::fs::read_to_string(f).unwrap()).collect();
        let headers = text.lines().filter(|l| *l == "---- flatland ----").count();
        assert!(
            text.starts_with("---- flatland ----\n") && headers == logs.len(),
            "{text}"
        );
        let lines: Vec<&str> = text
            .lines()
            .filter(|l| !l.starts_with("----"))
            .map(|l| l.get(9..).unwrap_or_default())
            .collect();
        assert_eq!(
            lines,
            [
                "+ Gordon".to_string(),
                format!("  Gordon: {named}"),
                "  Gordon (team): го на рельсы".to_string(),
                "  Gordon: /login ***".to_string(),
                format!("» {bot}: ну привет")
            ],
            "{text}"
        );

        let mut reader = Reader::open(&recording(&root)).unwrap();
        while let Some(record) = reader.next_record().unwrap() {
            let bytes = postcard::to_allocvec(&record).unwrap();
            assert!(
                !bytes.windows(8).any(|w| w == b"hunter2-"),
                "the chat key is in the recording"
            );
        }
        replays_and_tells_another_line(&root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn phrases_replay_to_the_same_commands() {
        use lb_chat::{Outcome, Trigger};

        let root = dir("record-phrase");
        let fake = crate::chat::FakeBackend::answering(&[Outcome::Phrase("гг, все молодцы".into())]);
        let sent = fake.sent.clone();
        let (mut rt, mut rec, mut server) = chat_server(&root, fake);
        new_map(&mut rt, &mut rec, &mut server, 1, true);
        frames(&mut rt, &mut rec, &mut server, 0..1500, |rt, rec, server, n| {
            if n == 900 {
                let name = rt.bots[0].persona.name.clone();
                command(rt, rec, server, &["lb", "chat", "event", &name, "win"]);
            }
        });
        assert!(said(&server, "гг, все молодцы"), "{:?}", server.sent);
        let asked = &sent.lock().unwrap().asked;
        assert!(
            matches!(asked[..], [ref r] if matches!(r.trigger, Trigger::MatchEnd { won: true, .. })),
            "{asked:?}"
        );
        let n = rt.chat.counts;
        assert_eq!((n.asked, n.phrases, n.lines, n.said), (1, 1, 0, 1));
        rec.finish(&mut rt, "test over");
        replays_and_tells_another_line(&root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_talk_carries_over_the_map_change_and_replays() {
        use lb_chat::{Outcome, Trigger};

        let root = dir("record-talk");
        let fake = crate::chat::FakeBackend::answering(&[
            Outcome::Line("привет, Гордон".into()),
            Outcome::Line("а то".into()),
        ]);
        let sent = fake.sent.clone();
        let (mut rt, mut rec, mut server) = chat_server(&root, fake);
        new_map(&mut rt, &mut rec, &mut server, 1, false);
        let mut named = String::new();
        frames(&mut rt, &mut rec, &mut server, 0..1500, |rt, _, server, n| {
            if n == 800 {
                named = format!("{}, привет", netname(rt));
                server.says.push((850, named.clone()));
            }
        });
        assert!(said(&server, "привет, Гордон"), "{:?}", server.sent);
        let persona = rt.bots[0].persona.name.clone();
        step(&mut rt, &mut rec, &mut server, Entry::MapEnd);

        new_map(&mut rt, &mut rec, &mut server, 2, true);
        frames(&mut rt, &mut rec, &mut server, 0..1500, |_, _, server, n| {
            if n == 800 {
                server.says.push((850, "ты свою уже приготовил?".into()));
            }
        });
        assert_eq!(rt.bots[0].persona.name, persona, "the bot is back on the next map");
        {
            let sent = sent.lock().unwrap();
            let asked = sent
                .asked
                .iter()
                .find(|r| matches!(&r.trigger, Trigger::Continued { text, .. } if text == "ты свою уже приготовил?"))
                .unwrap_or_else(|| panic!("no Continued: {:#?}", sent.asked));
            assert_eq!(asked.bot.persona, persona);
            let joined =
                |r: &lb_chat::ChatRequest| r.events.iter().any(|e| matches!(e.event, lb_chat::Event::Join { .. }));
            assert!(joined(&sent.asked[0]), "Gordon joined on the first map");
            assert!(!joined(asked), "and is only back on the second: {:?}", asked.events);
            let before: Vec<(bool, &str)> = asked
                .talk
                .iter()
                .filter(|s| s.age > asked.scene.elapsed)
                .map(|s| (s.mine, s.text.as_str()))
                .collect();
            assert_eq!(
                before,
                [(false, named.as_str()), (true, "привет, Гордон")],
                "the talk of the map before"
            );
        }
        assert!(said(&server, "а то"), "{:?}", server.sent);
        rec.finish(&mut rt, "test over");
        replays_and_tells_another_line(&root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_busy_bot_answers_once_its_line_is_out() {
        use lb_chat::Outcome;

        let root = dir("record-busy");
        let fake = crate::chat::FakeBackend::answering(&[Outcome::Line("ну привет".into())]);
        let (mut rt, mut rec, mut server) = chat_server(&root, fake);
        new_map(&mut rt, &mut rec, &mut server, 1, true);
        let typed = "ну всё, пошёл я на рельсы, кто со мной";
        frames(&mut rt, &mut rec, &mut server, 0..3500, |rt, rec, server, n| {
            if n == 700 {
                let name = rt.bots[0].persona.name.clone();
                command(rt, rec, server, &["lb", "chat", "say", &name, typed]);
                server.says.push((800, format!("{}, привет", netname(rt))));
            }
        });
        let said: Vec<&str> = server
            .sent
            .iter()
            .filter(|a| a[0] == "say")
            .map(|a| a[1].as_str())
            .collect();
        assert_eq!(said, [typed, "ну привет"], "the answer waited for the line being typed");
        rec.finish(&mut rt, "test over");
        replays_and_tells_another_line(&root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_config_reload_has_the_worker_read_its_files_again() {
        let root = dir("record-reload");
        let fake = crate::chat::FakeBackend::answering(&[]);
        let sent = fake.sent.clone();
        let (mut rt, mut rec, mut server) = chat_server(&root, fake);
        rt.chat.chatlog = Some(crate::chat::ChatLog::new(root.join("chatlog")));
        new_map(&mut rt, &mut rec, &mut server, 1, false);
        frames(&mut rt, &mut rec, &mut server, 0..100, |_, _, _, _| {});
        command(&mut rt, &mut rec, &mut server, &["lb", "config", "reload"]);
        assert_eq!(sent.lock().unwrap().reloads, 1);
        server.says.push((150, "кто тут".into()));
        frames(&mut rt, &mut rec, &mut server, 100..200, |_, _, _, _| {});
        let mut logs: Vec<PathBuf> = std::fs::read_dir(root.join("chatlog"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        logs.sort();
        let text: String = logs.iter().map(|f| std::fs::read_to_string(f).unwrap()).collect();
        let lines: Vec<&str> = text
            .lines()
            .map(|l| {
                if l.starts_with("----") {
                    l
                } else {
                    l.get(9..).unwrap_or(l)
                }
            })
            .collect();
        assert_eq!(
            lines,
            [
                "---- flatland ----",
                "+ Gordon",
                "---- flatland ----",
                "  Gordon: кто тут"
            ],
            "the file let go, the next line opens it under the map's header"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_recording_of_another_format_is_refused() {
        let root = dir("record-format");
        let path = root.join("old.lbrec");
        std::fs::write(&path, [MAGIC.as_slice(), &3u32.to_le_bytes()].concat()).unwrap();
        let Err(err) = Reader::open(&path) else {
            panic!("a recording of format 3 opens");
        };
        assert!(err.contains("recording format 3, this build reads 4"), "{err}");
        let err = replay(&path, &ReplayOptions::default(), &mut |_| {}).unwrap_err();
        assert!(err.contains("recording format 3"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recorded_files_stay_inside_the_replay_directory() {
        for rel in [
            "config/lambdabots.yaml",
            "maps/de..x.bsp",
            "addons/yapb/data/graph/a.graph",
        ] {
            assert!(stays_inside(rel), "{rel}");
        }
        for rel in ["", ".", "./config/a", "/etc/passwd", "../x", "config/../../x"] {
            assert!(!stays_inside(rel), "{rel}");
        }
    }

    #[test]
    fn oversized_blocks_are_refused() {
        let root = dir("record-oversized");
        let path = root.join("bad.lbrec");
        let head = [MAGIC.as_slice(), &FORMAT.to_le_bytes()].concat();
        std::fs::write(&path, [head.as_slice(), &u32::MAX.to_le_bytes()].concat()).unwrap();
        assert!(Reader::open(&path).unwrap().next_record().is_err());
        let packed = [u32::MAX.to_le_bytes().as_slice(), &[0x10, b'a', 0, 0]].concat();
        std::fs::write(
            &path,
            [head.as_slice(), &(packed.len() as u32).to_le_bytes(), &packed].concat(),
        )
        .unwrap();
        let err = Reader::open(&path).unwrap().next_record().unwrap_err();
        assert!(err.contains("unpacked"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_recorded_config_keeps_no_secret() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/config/lambdabots.yaml");
        let text = std::fs::read_to_string(path)
            .unwrap()
            .replace("secret: \"\"", "secret: hunter2");
        let original = MainConfig::parse(&text, "t").unwrap();
        assert_eq!(original.telemetry.secret, "hunter2");
        let scrubbed = String::from_utf8(scrub_config(text.as_bytes()).unwrap()).unwrap();
        assert!(!scrubbed.contains("hunter2"), "{scrubbed}");
        let mut back = MainConfig::parse(&scrubbed, "t").unwrap();
        assert_eq!(back.telemetry.secret, REDACTED);
        back.telemetry.secret = original.telemetry.secret.clone();
        assert_eq!(back, original);
        assert!(scrub_config(b"schema: lambdabots/main@1\nquota: [").is_none());
    }
}
