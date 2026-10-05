//! The chat worker: a thread that asks the model for the bots' lines one request at a time, so the game never
//! waits on the network. It reads the key, keeps the memory of players and the day's token count, and answers every
//! request, with a line or with why there is none.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lb_chat::memory::Memory;
use lb_chat::prompt::{self, Known, Rendered};
use lb_chat::{Aliases, ChatRequest, Failure, MapSummary, Outcome, Reply, sanitize};
use lb_config::main_config::{ChatConfig, ProviderKind};
use lb_llm::{Client, ErrorClass, Stop};

use super::backend::{ChatBackend, Job};
use super::store::{self, Notes, Paths, Usage};
use crate::logging;

/// Requests waiting for the worker; more are answered at once as failed.
const QUEUE: usize = 32;
/// A request older than this when its turn comes is not worth sending.
const STALE: Duration = Duration::from_secs(20);
/// Waits after failures in a row: 5 s, 10 s, 20 s, … up to a minute.
const BACKOFF_FIRST: f64 = 5.0;
const BACKOFF_MAX: f64 = 60.0;
/// The token count is written every this many requests (and when the worker stops).
const USAGE_SAVE_EVERY: u64 = 10;

enum Msg {
    Job(Job, Instant),
    Shutdown,
}

pub struct WorkerBackend {
    jobs: SyncSender<Msg>,
    replies: Receiver<Reply>,
    /// Replies made here: requests the queue had no room for.
    refused: Vec<Reply>,
    done: Receiver<()>,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<String>>,
    stopped: bool,
}

impl WorkerBackend {
    pub fn start(config: ChatConfig, install: &Path) -> std::io::Result<WorkerBackend> {
        let (jobs, rx) = mpsc::sync_channel(QUEUE);
        let (tx, replies) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new("starting".to_string()));
        let paths = Paths::new(install);
        {
            let (stop, status) = (stop.clone(), status.clone());
            std::thread::Builder::new().name("lb-chat".into()).spawn(move || {
                let mut worker = Worker::new(config, paths, status);
                while let Ok(msg) = rx.recv() {
                    match msg {
                        Msg::Job(job, queued) => {
                            if let Some(reply) = worker.handle(job, queued, stop.load(Ordering::SeqCst)) {
                                let _ = tx.send(reply);
                            }
                        }
                        Msg::Shutdown => break,
                    }
                }
                worker.save();
                worker.set_status("stopped".into());
                let _ = done_tx.send(());
            })?;
        }
        Ok(WorkerBackend {
            jobs,
            replies,
            refused: Vec::new(),
            done,
            stop,
            status,
            stopped: false,
        })
    }
}

impl ChatBackend for WorkerBackend {
    fn send(&mut self, job: Job) {
        if self.stopped {
            return;
        }
        let id = match &job {
            Job::Ask(req) => Some(req.id),
            _ => None,
        };
        match self.jobs.try_send(Msg::Job(job, Instant::now())) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                if let Some(id) = id {
                    self.refused.push(Reply {
                        id,
                        outcome: Outcome::Failed(Failure::Backoff),
                    });
                }
            }
        }
    }

    fn poll(&mut self) -> Vec<Reply> {
        let mut out = std::mem::take(&mut self.refused);
        out.extend(self.replies.try_iter());
        out
    }

    fn status(&self) -> String {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn shutdown(&mut self, wait: Duration) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.stop.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + wait;
        // The queue may be full: it empties quickly once requests are refused.
        loop {
            match self.jobs.try_send(Msg::Shutdown) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => break,
                Err(TrySendError::Full(_)) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                Err(TrySendError::Full(_)) => return,
            }
        }
        match self
            .done
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {}
            Err(RecvTimeoutError::Timeout) => tracing::warn!("chat worker still busy after {wait:?}"),
        }
    }

    fn live(&self) -> bool {
        !self.stopped
    }
}

impl Drop for WorkerBackend {
    fn drop(&mut self) {
        if !self.stopped {
            self.stop.store(true, Ordering::SeqCst);
            let _ = self.jobs.try_send(Msg::Shutdown);
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

struct Worker {
    config: ChatConfig,
    paths: Paths,
    client: Option<Client>,
    /// Why the provider cannot be used until the settings change.
    disabled: Option<String>,
    backoff_until: Option<Instant>,
    failures: u32,
    memory: Memory,
    memory_dirty: bool,
    notes: Notes,
    usage: Usage,
    usage_dirty: u64,
    status: Arc<Mutex<String>>,
}

impl Worker {
    fn new(config: ChatConfig, paths: Paths, status: Arc<Mutex<String>>) -> Worker {
        let mut w = Worker {
            memory: store::load_memory(&paths.memory),
            notes: Notes::load(&paths.players),
            usage: store::load_usage(&paths.usage),
            config,
            paths,
            client: None,
            disabled: None,
            backoff_until: None,
            failures: 0,
            memory_dirty: false,
            usage_dirty: 0,
            status,
        };
        tracing::info!(
            "chat worker: {} players remembered, {} notes from {}",
            w.memory.players.len(),
            w.notes.len(),
            w.paths.players.display()
        );
        w.refresh_status();
        w
    }

    fn set_status(&self, s: String) {
        if let Ok(mut status) = self.status.lock() {
            *status = s;
        }
    }

    fn refresh_status(&mut self) {
        let p = &self.config.provider;
        let today = self.usage.on(unix_now() / 86_400);
        let limit = match self.config.limits.tokens_per_day {
            0 => String::new(),
            n => format!(" of {n}"),
        };
        let state = match (&self.disabled, self.backoff_until) {
            (Some(why), _) => format!("disabled: {why}"),
            (None, Some(until)) if until > Instant::now() => {
                format!(
                    "waiting {:.0} s after failures",
                    until.saturating_duration_since(Instant::now()).as_secs_f64()
                )
            }
            _ => "ready".into(),
        };
        let at = self
            .client
            .as_ref()
            .map_or_else(|| p.base_url.clone(), |c| c.url().to_string());
        self.set_status(format!(
            "{state}; {} at {}; {today}{limit} tokens today; {} players remembered",
            p.model,
            if at.is_empty() {
                "the provider's default address".into()
            } else {
                at
            },
            self.memory.players.len()
        ));
    }

    /// The key: the config's own, else the file's, else the environment variable's; it must fit an HTTP header.
    fn key(&self) -> Result<Option<String>, String> {
        let key = self.key_text()?;
        if key.as_deref().is_some_and(|k| !k.bytes().all(|b| b.is_ascii_graphic())) {
            return Err("the API key has characters an HTTP header cannot carry".into());
        }
        Ok(key)
    }

    fn key_text(&self) -> Result<Option<String>, String> {
        let p = &self.config.provider;
        if !p.api_key.is_empty() {
            return Ok(Some(p.api_key.0.trim().to_string()));
        }
        if !p.api_key_file.trim().is_empty() {
            let path = self.paths.resolve(&p.api_key_file);
            let key = std::fs::read_to_string(&path).map_err(|e| format!("api_key_file {}: {e}", path.display()))?;
            return Ok(Some(key.trim().to_string()).filter(|k| !k.is_empty()));
        }
        let var = p.api_key_env.trim();
        Ok((!var.is_empty())
            .then(|| std::env::var(var).ok())
            .flatten()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty()))
    }

    fn client(&mut self) -> Result<&Client, String> {
        if self.client.is_none() {
            let p = &self.config.provider;
            let key = self.key()?;
            if key.is_none() && p.kind == ProviderKind::Anthropic && p.base_url.trim().is_empty() {
                return Err(format!(
                    "no API key: set chat.provider.api_key, api_key_file, or the {} variable",
                    if p.api_key_env.trim().is_empty() {
                        "api_key_env"
                    } else {
                        p.api_key_env.trim()
                    }
                ));
            }
            let extra_body = (!p.extra_body.trim().is_empty())
                .then(|| serde_json::from_str(&p.extra_body).map_err(|e| format!("extra_body: {e}")))
                .transpose()?;
            let settings = lb_llm::Settings {
                kind: match p.kind {
                    ProviderKind::Anthropic => lb_llm::Kind::Anthropic,
                    ProviderKind::Openai => lb_llm::Kind::OpenAi,
                },
                base_url: p.base_url.trim().to_string(),
                model: p.model.trim().to_string(),
                key,
                headers: p.headers.iter().map(|(k, v)| (k.clone(), v.0.clone())).collect(),
                max_tokens: p.max_tokens,
                temperature: p.temperature,
                extra_body,
                timeout: Duration::from_secs_f32(p.timeout),
                ca_file: (!p.ca_file.trim().is_empty()).then(|| self.paths.resolve(&p.ca_file)),
            };
            let client = Client::new(settings).map_err(|e| e.to_string())?;
            tracing::info!("chat: {} at {}", p.model, client.url());
            self.client = Some(client);
        }
        self.client.as_ref().ok_or_else(|| "no client".to_string())
    }

    fn over_budget(&self) -> bool {
        let limit = self.config.limits.tokens_per_day;
        limit > 0 && self.usage.on(unix_now() / 86_400) >= limit
    }

    /// Why a request cannot go now, if it cannot.
    fn blocked(&self) -> Option<Failure> {
        if self.disabled.is_some() {
            Some(Failure::Disabled)
        } else if self.over_budget() {
            Some(Failure::Budget)
        } else if self.backoff_until.is_some_and(|t| t > Instant::now()) {
            Some(Failure::Backoff)
        } else {
            None
        }
    }

    /// Sends a prompt, counting tokens and handling failures; `None` after a failure, which says why.
    fn complete(&mut self, rendered: Rendered) -> Result<lb_llm::Completion, Failure> {
        let client = match self.client() {
            Ok(c) => c,
            Err(why) => {
                tracing::warn!("chat disabled: {why}");
                self.disabled = Some(why);
                return Err(Failure::Disabled);
            }
        };
        let prompt = lb_llm::Prompt {
            system_static: rendered.system_static,
            system: rendered.system,
            user: rendered.user,
            max_tokens: rendered.max_tokens,
        };
        match client.complete(&prompt) {
            Ok(done) => {
                self.failures = 0;
                self.backoff_until = None;
                self.usage
                    .add(unix_now() / 86_400, done.input_tokens + done.output_tokens);
                self.usage_dirty += 1;
                if self.usage_dirty >= USAGE_SAVE_EVERY {
                    self.save_usage();
                }
                Ok(done)
            }
            Err(e) => match e.class() {
                ErrorClass::Fatal => {
                    tracing::warn!("chat disabled until `lb chat reload` or `lb config reload`: {e}");
                    self.disabled = Some(e.to_string());
                    Err(Failure::Disabled)
                }
                ErrorClass::Backoff(asked) => {
                    self.failures += 1;
                    let ours = (BACKOFF_FIRST * 2f64.powi(self.failures.min(8) as i32 - 1)).min(BACKOFF_MAX);
                    let wait = asked.map_or(ours, |d| d.as_secs_f64().max(1.0));
                    tracing::warn!("chat request failed, waiting {wait:.0} s: {e}");
                    self.backoff_until = Some(Instant::now() + Duration::from_secs_f64(wait));
                    Err(Failure::Backoff)
                }
                ErrorClass::Drop => {
                    tracing::warn!("chat request dropped: {e}");
                    Err(Failure::Error)
                }
            },
        }
    }

    fn handle(&mut self, job: Job, queued: Instant, stopping: bool) -> Option<Reply> {
        let reply = match job {
            Job::Ask(req) => {
                let outcome = if stopping || queued.elapsed() > STALE {
                    Outcome::Failed(Failure::Backoff)
                } else {
                    self.ask(&req)
                };
                Some(Reply { id: req.id, outcome })
            }
            Job::MapEnd(summary) => {
                self.map_end(&summary, !stopping);
                None
            }
            Job::Configure(config) => {
                if self.config.provider != config.provider {
                    self.client = None;
                    self.disabled = None;
                    self.backoff_until = None;
                }
                self.config = *config;
                None
            }
            Job::Preview(req) => {
                let known = self.known(&req);
                let aliases = self.aliases(&req);
                let r = prompt::render(
                    &req,
                    &known,
                    &aliases,
                    &self.memory.maps,
                    &self.config.server,
                    unix_now(),
                );
                for (title, text) in [("system", &r.system_static), ("bot", &r.system), ("request", &r.user)] {
                    logging::console_line(format!("[lambdabots] chat prompt, {title}:"));
                    for line in text.lines() {
                        logging::console_line(format!("  {line}"));
                    }
                }
                None
            }
            Job::Memory { query, forget } => {
                self.memory_command(&query, forget);
                None
            }
            Job::Reload => {
                self.notes = Notes::load(&self.paths.players);
                self.client = None;
                self.disabled = None;
                self.backoff_until = None;
                self.failures = 0;
                logging::console_line(format!(
                    "[lambdabots] chat: {} notes from {}, provider settings read again",
                    self.notes.len(),
                    self.paths.players.display()
                ));
                None
            }
        };
        self.refresh_status();
        reply
    }

    /// What the bots know of the players a request shows: the one it answers first.
    fn known<'a>(&'a self, req: &'a ChatRequest) -> Vec<Known<'a>> {
        let first = req.trigger.about().map(|w| w.name.as_str());
        let mut players: Vec<_> = req.scene.players.iter().filter(|p| !p.me && p.key.is_some()).collect();
        players.sort_by_key(|p| Some(p.name.as_str()) != first);
        players
            .into_iter()
            .filter_map(|p| {
                let key = p.key.as_deref()?;
                let note = self.notes.get(key, &p.name);
                let memory = self.memory.players.get(key);
                (note.is_some() || memory.is_some()).then_some(Known {
                    name: &p.name,
                    note,
                    memory,
                })
            })
            .collect()
    }

    /// The aliases of the players a request shows or speaks of.
    fn aliases(&self, req: &ChatRequest) -> Aliases {
        let mut aliases = Aliases::default();
        for p in &req.scene.players {
            aliases.insert(
                &p.name,
                self.notes.aliases(p.key.as_deref().unwrap_or_default(), &p.name),
            );
        }
        let people = req
            .events
            .iter()
            .flat_map(|r| r.event.people())
            .chain(req.trigger.about());
        for who in people {
            aliases.insert(&who.name, self.notes.aliases("", &who.name));
        }
        aliases
    }

    fn ask(&mut self, req: &ChatRequest) -> Outcome {
        if let Some(why) = self.blocked() {
            return Outcome::Failed(why);
        }
        let started = Instant::now();
        let aliases = self.aliases(req);
        let rendered = {
            let known = self.known(req);
            prompt::render(
                req,
                &known,
                &aliases,
                &self.memory.maps,
                &self.config.server,
                unix_now(),
            )
        };
        tracing::debug!(
            "chat prompt for {}:\n{}\n{}",
            req.bot.name,
            rendered.system,
            rendered.user
        );
        match self.complete(rendered) {
            Ok(done) => {
                let line = (done.stop != Stop::Refusal)
                    .then(|| sanitize::clean_reply(&done.text, &req.bot.name))
                    .flatten()
                    .map(|line| aliases.apply(&line));
                tracing::info!(
                    "chat {}: {:?} -> {} ({} ms, {} + {} tokens{})",
                    req.bot.name,
                    req.trigger,
                    line.as_deref().unwrap_or("(nothing)"),
                    started.elapsed().as_millis(),
                    done.input_tokens,
                    done.output_tokens,
                    if done.stop == Stop::Refusal { ", refused" } else { "" }
                );
                line.map_or(Outcome::Skip, Outcome::Line)
            }
            Err(failure) => Outcome::Failed(failure),
        }
    }

    /// Takes a map into the memory; `notes`: there is time to ask the model for notes on its players.
    fn map_end(&mut self, s: &MapSummary, notes: bool) {
        if !self.config.memory.enabled || s.players.is_empty() {
            return;
        }
        let now = unix_now();
        let previous: BTreeMap<String, String> = s
            .players
            .iter()
            .filter_map(|p| {
                self.memory
                    .players
                    .get(&p.key)
                    .map(|m| (p.key.clone(), m.notes.clone()))
            })
            .collect();
        self.memory.merge(s, now);
        self.memory.prune(now, self.config.memory.forget_after_days);
        self.memory_dirty = true;
        if notes
            && self.config.memory.ai_notes
            && self.blocked().is_none()
            && let Some(rendered) = prompt::render_notes(s, &previous, &self.summary_aliases(s))
            && let Ok(done) = self.complete(rendered)
        {
            let mut notes = prompt::parse_notes(&done.text);
            notes.retain(|key, _| s.players.iter().any(|p| &p.key == key));
            tracing::info!(
                "chat: notes on {} players after {} ({} + {} tokens)",
                notes.len(),
                s.map,
                done.input_tokens,
                done.output_tokens
            );
            self.memory.apply_notes(&notes);
        }
        self.save();
    }

    fn summary_aliases(&self, s: &MapSummary) -> Aliases {
        let mut aliases = Aliases::default();
        for p in &s.players {
            aliases.insert(&p.name, self.notes.aliases(&p.key, &p.name));
        }
        aliases
    }

    fn memory_command(&mut self, query: &str, forget: bool) {
        let out = if forget {
            match self.memory.forget(query) {
                Some(key) => {
                    self.memory_dirty = true;
                    self.save();
                    format!("chat: forgot {key}")
                }
                None => format!("chat: nobody called `{query}` is remembered"),
            }
        } else {
            match self.memory.find(query) {
                Some((key, m)) => {
                    let duels: Vec<String> = m.vs_bots.iter().map(|(bot, [a, b])| format!("{bot} {a}:{b}")).collect();
                    let lines: Vec<&str> = m.lines.iter().map(|(_, l)| l.as_str()).collect();
                    format!(
                        "chat: {key} ({}): {} maps, {}/{} kills/deaths, {} wins, weapons {:?}; vs bots: {}; notes: {}; \
                         lines: {:?}; admin note: {}; alias: {}",
                        m.names.join(", "),
                        m.maps,
                        m.kills,
                        m.deaths,
                        m.wins,
                        m.favourite_weapons(),
                        duels.join(", "),
                        if m.notes.is_empty() { "-" } else { &m.notes },
                        lines,
                        self.notes
                            .get(key, m.names.first().map_or("", String::as_str))
                            .unwrap_or("-"),
                        match self.notes.aliases(key, m.names.first().map_or("", String::as_str)) {
                            [] => "-".to_string(),
                            all => all.join(", "),
                        }
                    )
                }
                None => format!("chat: nobody called `{query}` is remembered"),
            }
        };
        logging::console_line(format!("[lambdabots] {out}"));
    }

    fn save_usage(&mut self) {
        if let Ok(text) = serde_json::to_string(&self.usage)
            && let Err(e) = store::write_atomic(&self.paths.usage, &text)
        {
            tracing::warn!("chat: {}: {e}", self.paths.usage.display());
        }
        self.usage_dirty = 0;
    }

    fn save(&mut self) {
        if self.usage_dirty > 0 {
            self.save_usage();
        }
        if self.memory_dirty {
            if let Err(e) = store::write_atomic(&self.paths.memory, &self.memory.to_json()) {
                tracing::warn!("chat: {}: {e}", self.paths.memory.display());
            }
            self.memory_dirty = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use lb_chat::request::{BotCard, PlayerCard, PlayerMap, Scene};
    use lb_chat::{Trigger, Who};
    use lb_config::main_config::ChatConfig;
    use lb_llm::testing::{Canned, MockServer};

    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lb-chat-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn config(url: &str) -> ChatConfig {
        let mut c = ChatConfig {
            enabled: true,
            ..ChatConfig::default()
        };
        c.provider.kind = ProviderKind::Openai;
        c.provider.base_url = format!("{url}/v1");
        c.provider.api_key_env = String::new();
        c.provider.timeout = 2.0;
        c
    }

    fn who(slot: u8, name: &str) -> Who {
        Who {
            slot,
            userid: i32::from(slot),
            name: name.into(),
            bot: false,
        }
    }

    fn request(id: u64) -> ChatRequest {
        ChatRequest {
            id,
            bot: BotCard {
                name: "Kleiner".into(),
                userid: 2,
                skill: 50,
                style: "balanced".into(),
                favourite_weapons: Vec::new(),
                profanity: false,
                manner_text: None,
                manner: 0,
                about: None,
                boldness: 0.0,
                alive: true,
                frags: 0,
                deaths: 0,
                level: None,
            },
            trigger: Trigger::Addressed {
                from: who(1, "Gordon"),
                text: "kleiner, hi".into(),
            },
            scene: Scene {
                map: "crossfire".into(),
                gungame: false,
                teamplay: false,
                elapsed: 30.0,
                players: vec![PlayerCard {
                    name: "Gordon".into(),
                    key: Some("STEAM_0:1:42".into()),
                    frags: 1,
                    deaths: 0,
                    level: None,
                    me: false,
                    duel: (0, 1),
                }],
                leader: None,
            },
            events: Vec::new(),
            language: "en".into(),
            max_chars: 60,
            team: false,
        }
    }

    fn answer(text: &str) -> Canned {
        let body = serde_json::json!({
            "choices": [{"message": {"content": text}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 5},
        });
        Canned::json(200, &body.to_string())
    }

    /// Waits for the replies to `n` requests.
    fn replies(w: &mut WorkerBackend, n: usize) -> Vec<Reply> {
        let mut out = Vec::new();
        let until = Instant::now() + Duration::from_secs(10);
        while out.len() < n && Instant::now() < until {
            out.extend(w.poll());
            std::thread::sleep(Duration::from_millis(5));
        }
        out
    }

    #[test]
    fn answers_skips_and_remembers() {
        let server = MockServer::start(vec![answer("Kleiner: \"hey Gordon\""), answer("-"), answer("{}")]);
        let root = dir("answers");
        std::fs::create_dir_all(root.join("config/chat")).unwrap();
        std::fs::write(
            root.join("config/chat/players.yaml"),
            "schema: lambdabots/chat-players@1\nplayers:\n  - id: STEAM_0:1:42\n    name: Gordon\n    note: the boss\n",
        )
        .unwrap();
        let mut w = WorkerBackend::start(config(&server.url()), &root).unwrap();
        w.send(Job::Ask(Box::new(request(1))));
        w.send(Job::Ask(Box::new(request(2))));
        let got = replies(&mut w, 2);
        assert_eq!(
            got[0],
            Reply {
                id: 1,
                outcome: Outcome::Line("hey Gordon".into())
            }
        );
        assert_eq!(
            got[1],
            Reply {
                id: 2,
                outcome: Outcome::Skip
            }
        );
        let asked = &server.requests()[0];
        assert!(
            asked.body.contains("the boss"),
            "the admin's note goes into the prompt: {}",
            asked.body
        );
        assert!(w.status().contains("ready"), "{}", w.status());
        let summary = MapSummary {
            map: "crossfire".into(),
            language: "en".into(),
            minutes: 10,
            players: vec![PlayerMap {
                key: "STEAM_0:1:42".into(),
                name: "Gordon".into(),
                vs_bots: vec![("Kleiner".into(), 3, 1)],
                kills: 3,
                deaths: 1,
                lines: vec![(10.0, "kleiner, hi".into())],
                ..Default::default()
            }],
            ..Default::default()
        };
        w.send(Job::MapEnd(Box::new(summary)));
        let until = Instant::now() + Duration::from_secs(10);
        while server.requests().len() < 3 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        w.shutdown(Duration::from_secs(5));
        let memory = store::load_memory(&root.join("data/chat/memory.json"));
        assert_eq!(memory.players["STEAM_0:1:42"].vs_bots["Kleiner"], [3, 1]);
        let usage = store::load_usage(&root.join("data/chat/usage.json"));
        assert_eq!(usage.tokens, 315, "two answers and the notes request");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn players_are_called_by_their_aliases() {
        let server = MockServer::start(vec![answer("Gordon, hi! where is gordon?")]);
        let root = dir("aliases");
        std::fs::create_dir_all(root.join("config/chat")).unwrap();
        std::fs::write(
            root.join("config/chat/players.yaml"),
            "schema: lambdabots/chat-players@1\nplayers:\n  - id: STEAM_0:1:42\n    name: Gordon Freeman\n    alias: [Гордон, Фримен]\n",
        )
        .unwrap();
        let mut w = WorkerBackend::start(config(&server.url()), &root).unwrap();
        w.send(Job::Ask(Box::new(request(1))));
        let got = replies(&mut w, 1);
        assert_eq!(got[0].outcome, Outcome::Line("Гордон, hi! where is Гордон?".into()));
        let asked = &server.requests()[0];
        assert!(
            asked.body.contains("Гордон or Фримен (nickname Gordon)"),
            "{}",
            asked.body
        );
        assert!(asked.body.contains("Гордон writes to you"), "{}", asked.body);
        w.shutdown(Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bad_key_stops_requests_and_busy_providers_wait() {
        let server = MockServer::start(vec![
            Canned::json(429, r#"{"error":{"message":"slow down"}}"#).header("retry-after", "30"),
            Canned::json(401, r#"{"error":{"message":"bad key"}}"#),
        ]);
        let root = dir("failures");
        let mut w = WorkerBackend::start(config(&server.url()), &root).unwrap();
        w.send(Job::Ask(Box::new(request(1))));
        w.send(Job::Ask(Box::new(request(2))));
        let got = replies(&mut w, 2);
        assert_eq!(got[0].outcome, Outcome::Failed(Failure::Backoff));
        assert_eq!(
            got[1].outcome,
            Outcome::Failed(Failure::Backoff),
            "waiting: not even sent"
        );
        assert_eq!(server.requests().len(), 1);
        w.send(Job::Reload);
        w.send(Job::Ask(Box::new(request(3))));
        w.send(Job::Ask(Box::new(request(4))));
        let got = replies(&mut w, 2);
        assert_eq!(got[0].outcome, Outcome::Failed(Failure::Disabled));
        assert_eq!(got[1].outcome, Outcome::Failed(Failure::Disabled));
        assert_eq!(server.requests().len(), 2, "a refused key is not tried again");
        assert!(w.status().contains("bad key"), "{}", w.status());
        w.shutdown(Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_day_budget_holds() {
        let server = MockServer::start(vec![answer("gg")]);
        let root = dir("budget");
        let mut c = config(&server.url());
        c.limits.tokens_per_day = 100;
        let mut w = WorkerBackend::start(c, &root).unwrap();
        for id in 1..=3 {
            w.send(Job::Ask(Box::new(request(id))));
        }
        let got = replies(&mut w, 3);
        let outcomes: Vec<&Outcome> = got.iter().map(|r| &r.outcome).collect();
        assert_eq!(
            outcomes,
            [
                &Outcome::Line("gg".into()),
                &Outcome::Failed(Failure::Budget),
                &Outcome::Failed(Failure::Budget)
            ]
        );
        w.shutdown(Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_key_for_the_anthropic_api_disables_chat() {
        let root = dir("nokey");
        let mut c = ChatConfig::default();
        c.provider.api_key_env = "LB_TEST_SURELY_UNSET_KEY".into();
        let mut w = WorkerBackend::start(c, &root).unwrap();
        w.send(Job::Ask(Box::new(request(1))));
        assert_eq!(replies(&mut w, 1)[0].outcome, Outcome::Failed(Failure::Disabled));
        assert!(w.status().contains("no API key"), "{}", w.status());
        w.shutdown(Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn shutdown_waits_for_a_slow_request_at_most_as_asked() {
        let server = MockServer::start(vec![answer("late").delay(Duration::from_millis(1500))]);
        let root = dir("slow");
        let mut w = WorkerBackend::start(config(&server.url()), &root).unwrap();
        w.send(Job::Ask(Box::new(request(1))));
        std::thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        w.shutdown(Duration::from_millis(300));
        assert!(
            started.elapsed() < Duration::from_millis(900),
            "{:?}",
            started.elapsed()
        );
        let other = MockServer::start(vec![answer("late").delay(Duration::from_millis(1500))]);
        let mut again = WorkerBackend::start(config(&other.url()), &root).unwrap();
        again.send(Job::Ask(Box::new(request(2))));
        std::thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        again.shutdown(Duration::from_secs(10));
        assert!(
            started.elapsed() < Duration::from_secs(6),
            "it stops once its request is done"
        );
        assert_eq!(
            again.poll(),
            vec![Reply {
                id: 2,
                outcome: Outcome::Line("late".into())
            }]
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
