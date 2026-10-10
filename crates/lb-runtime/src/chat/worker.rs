//! The chat worker: a thread that answers the bots' requests, so the game never waits on the network. A moment of the
//! game mostly gets a ready phrase at once; the rest go to the model one request at a time, its line through the
//! filter. It reads the key and `config/chat/`, keeps the memory of players and the day's token count, and answers
//! every request, with a line or with why there is none.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lb_chat::memory::{Memory, PlayerMemory};
use lb_chat::phrases::{self, Offer, Ring};
use lb_chat::prompt::{self, Context, Known, Rendered};
use lb_chat::{Aliases, ChatRequest, Event, Failure, MapSummary, Outcome, Reply, Who, profanity, sanitize};
use lb_config::chat_phrases::Moment;
use lb_config::main_config::{ChatConfig, ProviderKind};
use lb_core::rng::{Pcg32, splitmix64};
use lb_llm::{Client, Completion, ErrorClass, LlmError, Stop};

use super::backend::{ChatBackend, Job};
use super::store::{self, Library, Paths, Usage};
use super::transcript::Transcript;
use crate::logging;

/// Requests waiting for the worker; more are answered at once as failed.
const QUEUE: usize = 32;
/// A request older than this when its turn comes is not worth sending.
const STALE: Duration = Duration::from_secs(20);
/// A moment whose request failed gets a phrase instead while it is this fresh.
const PHRASE_FRESH: Duration = Duration::from_secs(10);
/// Waits after failures in a row: 5 s, 10 s, 20 s, … up to a minute.
const BACKOFF_FIRST: f64 = 5.0;
const BACKOFF_MAX: f64 = 60.0;
/// Failures in a row after which the provider answering again goes to the console.
const FAILURES_TOLD: u32 = 3;
/// Out of API credit: one try this often until a request goes through.
const UNPAID_WAIT: Duration = Duration::from_secs(600);
/// A map's notes request waits for this long without jobs, so the answers after a map change go first.
const NOTES_IDLE: Duration = Duration::from_secs(15);
/// The token count is written every this many requests (and when the worker stops).
const USAGE_SAVE_EVERY: u64 = 10;
/// The random stream of the phrases: the model's share, the pick, the manner.
const PHRASE_STREAM: u64 = 0x7068_7261_7365;

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
        WorkerBackend::spawn(config, install, |_| {})
    }

    /// Starts the worker thread; `tune` sets the worker up before its first job (tests shorten its waits).
    fn spawn(config: ChatConfig, install: &Path, tune: fn(&mut Worker)) -> std::io::Result<WorkerBackend> {
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
                tune(&mut worker);
                worker.run(&rx, &tx, &stop);
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

/// Unix seconds as the time of day: `09:13 UTC`.
fn utc(unix: u64) -> String {
    format!("{:02}:{:02} UTC", unix / 3600 % 24, unix / 60 % 60)
}

/// This run's seed: request ids repeat from run to run, the phrases' chances need not.
fn process_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    splitmix64(unix_now() ^ (u64::from(nanos) << 32) ^ u64::from(std::process::id()))
}

/// A line for the log and the server console, which shows no info lines.
fn tell(line: String) {
    tracing::info!("{line}");
    logging::console_line(format!("[lambdabots] {line}"));
}

struct Worker {
    config: ChatConfig,
    paths: Paths,
    client: Option<Client>,
    /// Why the provider cannot be used until the settings change.
    disabled: Option<String>,
    backoff_until: Option<Instant>,
    /// Requests in a row that found the provider busy or out of reach.
    failures: u32,
    /// Out of API credit: the provider's latest word on it, and since when (unix seconds).
    out_of_credit: Option<(String, u64)>,
    memory: Memory,
    memory_dirty: bool,
    /// What the admin wrote in `config/chat/`.
    library: Library,
    usage: Usage,
    usage_dirty: u64,
    status: Arc<Mutex<String>>,
    /// `chat.transcript`: requests and answers written down.
    transcript: Option<Transcript>,
    /// The phrases picked lately, by every bot.
    ring: Ring,
    /// Mixed into each request's id for the phrases' chances ([`process_seed`]).
    seed: u64,
    /// Requests for the model, oldest first, and when they came: they go one at a time.
    waiting: VecDeque<(Box<ChatRequest>, Instant)>,
    /// The map whose notes request waits for a quiet moment.
    notes_due: Option<Box<MapSummary>>,
    /// Maps whose notes request was still waiting when the next map ended: it goes after the requests waiting.
    overdue: VecDeque<Box<MapSummary>>,
    /// [`NOTES_IDLE`] and [`UNPAID_WAIT`]; shorter in tests.
    notes_idle: Duration,
    unpaid_wait: Duration,
}

impl Worker {
    fn new(config: ChatConfig, paths: Paths, status: Arc<Mutex<String>>) -> Worker {
        let mut w = Worker {
            memory: store::load_memory(&paths.memory),
            library: Library::load(&paths.chat),
            usage: store::load_usage(&paths.usage),
            config,
            paths,
            client: None,
            disabled: None,
            backoff_until: None,
            failures: 0,
            out_of_credit: None,
            memory_dirty: false,
            usage_dirty: 0,
            status,
            transcript: None,
            ring: Ring::default(),
            seed: process_seed(),
            waiting: VecDeque::new(),
            notes_due: None,
            overdue: VecDeque::new(),
            notes_idle: NOTES_IDLE,
            unpaid_wait: UNPAID_WAIT,
        };
        w.sync_transcript();
        tracing::info!(
            "chat worker: {} players remembered; {}",
            w.memory.players.len(),
            w.library.summary().join(" · ")
        );
        w.refresh_status();
        w
    }

    fn set_status(&self, s: String) {
        if let Ok(mut status) = self.status.lock() {
            *status = s;
        }
    }

    fn refresh_status(&self) {
        let p = &self.config.provider;
        let today = self.usage.on(unix_now() / 86_400);
        let limit = match self.config.limits.tokens_per_day {
            0 => String::new(),
            n => format!(" of {n}"),
        };
        let at = self
            .client
            .as_ref()
            .map_or_else(|| p.base_url.clone(), |c| c.url().to_string());
        let mut library = self.library.summary().join(" · ");
        match self.library.problems.len() {
            0 => {}
            1 => library.push_str(" · 1 problem, see the log"),
            n => library.push_str(&format!(" · {n} problems, see the log")),
        }
        self.set_status(format!(
            "{}; {} at {}; {today}{limit} tokens today; {} players remembered\nconfig/chat: {library}",
            self.state(),
            p.model,
            if at.is_empty() {
                "the provider's default address".into()
            } else {
                at
            },
            self.memory.players.len()
        ));
    }

    /// Whether requests go to the provider, and why not.
    fn state(&self) -> String {
        let now = Instant::now();
        let waiting = self.backoff_until.filter(|&until| until > now);
        if let Some(why) = &self.disabled {
            return format!("disabled: {why}");
        }
        if let Some((message, since)) = &self.out_of_credit {
            let next = match waiting {
                Some(until) => {
                    let left = until.saturating_duration_since(now).as_secs_f64().round() as u64;
                    format!("next try at {} (`lb chat reload` tries now)", utc(unix_now() + left))
                }
                None => "the next request tries again".into(),
            };
            return format!("out of API credit since {}, {next}: {message}", utc(*since));
        }
        if self.over_budget() {
            return format!(
                "the day's {} tokens are spent, requests wait for 00:00 UTC",
                self.config.limits.tokens_per_day
            );
        }
        match waiting {
            Some(until) => format!(
                "waiting {:.0} s after failures",
                until.saturating_duration_since(now).as_secs_f64()
            ),
            None => "ready".into(),
        }
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

    /// Why a request cannot go now, if it cannot: settings refused; out of API credit until the next try; the day's
    /// tokens spent; a wait after failures.
    fn blocked(&self) -> Option<Failure> {
        let waiting = self.backoff_until.is_some_and(|t| t > Instant::now());
        if self.disabled.is_some() {
            Some(Failure::Disabled)
        } else if waiting && self.out_of_credit.is_some() {
            Some(Failure::Billing)
        } else if self.over_budget() {
            Some(Failure::Budget)
        } else if waiting {
            Some(Failure::Backoff)
        } else {
            None
        }
    }

    fn sync_transcript(&mut self) {
        if !self.config.transcript {
            self.transcript = None;
        } else if self.transcript.is_none() {
            self.transcript = Some(Transcript::new(self.paths.install.join("logs")));
        }
    }

    /// Sends a prompt, counting tokens and handling failures. `title` and `outcome` (what came of an answer) go to
    /// the transcript with the exchange.
    fn complete(
        &mut self,
        rendered: Rendered,
        title: &str,
        outcome: impl FnOnce(&Completion) -> String,
    ) -> Result<Completion, Failure> {
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
        let (result, wire) = client.exchange(&prompt);
        if let Some(t) = self.transcript.as_mut() {
            let what = match &result {
                Ok(done) => outcome(done),
                Err(e) => format!("failed: {e}"),
            };
            t.write(title, &wire, &what);
        }
        match result {
            Ok(done) => {
                self.answered(&done);
                Ok(done)
            }
            Err(e) => Err(self.failed(&e)),
        }
    }

    /// A request went through: the provider is back if it was not, and the tokens count.
    fn answered(&mut self, done: &Completion) {
        if let Some((_, since)) = self.out_of_credit.take() {
            let minutes = unix_now().saturating_sub(since) / 60;
            tell(format!(
                "chat: the provider answers again after {} h {} min out of API credit",
                minutes / 60,
                minutes % 60
            ));
        } else if self.failures >= FAILURES_TOLD {
            tell(format!(
                "chat: the provider answers again after {} failed requests",
                self.failures
            ));
        }
        self.failures = 0;
        self.backoff_until = None;
        let (day, limit) = (unix_now() / 86_400, self.config.limits.tokens_per_day);
        let before = self.usage.on(day);
        self.usage.add(day, done.input_tokens + done.output_tokens);
        if limit > 0 && before < limit && self.usage.on(day) >= limit {
            tracing::warn!("chat: the day's {limit} tokens are spent; requests wait for 00:00 UTC");
        }
        self.usage_dirty += 1;
        if self.usage_dirty >= USAGE_SAVE_EVERY {
            self.save_usage();
        }
    }

    /// What a failed request means for the next ones: refused settings stop them; out of API credit, one tries every
    /// [`UNPAID_WAIT`]; a busy provider is waited for, longer after each failure in a row.
    fn failed(&mut self, e: &LlmError) -> Failure {
        match e.class() {
            ErrorClass::Fatal => {
                tracing::warn!("chat disabled until `lb chat reload` or `lb config reload`: {e}");
                self.disabled = Some(e.to_string());
                Failure::Disabled
            }
            ErrorClass::Billing => {
                self.failures = 0;
                self.backoff_until = Some(Instant::now() + self.unpaid_wait);
                match self.out_of_credit.as_mut() {
                    Some((message, _)) => {
                        tracing::info!("chat: still out of API credit: {e}");
                        *message = e.to_string();
                    }
                    None => {
                        tracing::error!(
                            "chat: out of API credit; one try every {} min until the provider answers, `lb chat \
                             reload` tries now: {e}",
                            self.unpaid_wait.as_secs() / 60
                        );
                        self.out_of_credit = Some((e.to_string(), unix_now()));
                    }
                }
                Failure::Billing
            }
            ErrorClass::Backoff(asked) => {
                self.failures += 1;
                let ours = (BACKOFF_FIRST * 2f64.powi(self.failures.min(8) as i32 - 1)).min(BACKOFF_MAX);
                let wait = asked.map_or(ours, |d| d.as_secs_f64().max(1.0));
                if self.failures == 1 {
                    tracing::warn!("chat request failed, waiting {wait:.0} s: {e}");
                } else {
                    tracing::info!(
                        "chat request failed, {} in a row, waiting {wait:.0} s: {e}",
                        self.failures
                    );
                }
                self.backoff_until = Some(Instant::now() + Duration::from_secs_f64(wait));
                Failure::Backoff
            }
            ErrorClass::Drop => {
                tracing::warn!("chat request dropped: {e}");
                Failure::Error
            }
        }
    }

    /// Takes the jobs as they come until the backend shuts down. Every job that came is taken in before the next
    /// request goes to the network ([`Worker::take`], [`Worker::next`]), so a phrase waits for one request at most. A
    /// map's notes request goes once no job came for [`NOTES_IDLE`]; none while the backend stops.
    fn run(&mut self, rx: &Receiver<Msg>, tx: &Sender<Reply>, stop: &AtomicBool) {
        let mut send = |reply| {
            let _ = tx.send(reply);
        };
        loop {
            let first = if self.busy() {
                match rx.try_recv() {
                    Ok(msg) => Some(msg),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => return,
                }
            } else {
                let next = match self.notes_due {
                    Some(_) => rx.recv_timeout(self.notes_idle),
                    None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                };
                match next {
                    Ok(msg) => Some(msg),
                    Err(RecvTimeoutError::Timeout) => {
                        if let Some(summary) = self.notes_due.take()
                            && !stop.load(Ordering::SeqCst)
                        {
                            self.ask_notes(&summary);
                            self.refresh_status();
                        }
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            };
            for msg in first.into_iter().chain(rx.try_iter()) {
                match msg {
                    Msg::Job(job, queued) => self.take(job, queued, stop.load(Ordering::SeqCst), &mut send),
                    Msg::Shutdown => {
                        for (req, _) in std::mem::take(&mut self.waiting) {
                            send(Reply {
                                id: req.id,
                                outcome: Outcome::Failed(Failure::Backoff),
                            });
                        }
                        return;
                    }
                }
            }
            self.next(stop.load(Ordering::SeqCst), &mut send);
        }
    }

    /// Requests or notes wait for the network.
    fn busy(&self) -> bool {
        !self.waiting.is_empty() || !self.overdue.is_empty()
    }

    /// A job that came: done at once when it needs no network (a phrase, no line while the provider cannot be asked,
    /// settings, a map's end, a command); a request for the model waits for its turn.
    fn take(&mut self, job: Job, queued: Instant, stopping: bool, send: &mut impl FnMut(Reply)) {
        match job {
            Job::Ask(req) => match self.ask(&req, queued, stopping, false) {
                Some(outcome) => send(Reply { id: req.id, outcome }),
                None => self.waiting.push_back((req, queued)),
            },
            Job::MapEnd(summary) => self.map_end(summary),
            Job::Configure(config) => self.configure(*config),
            Job::Preview(req) => self.preview(&req),
            Job::Memory { query, forget } => self.memory_command(&query, forget),
            Job::Reload => self.reload(),
        }
        self.refresh_status();
    }

    /// The next job for the network: the oldest request for the model, else an overdue notes request (none while the
    /// worker stops). False when nothing waits.
    fn next(&mut self, stopping: bool, send: &mut impl FnMut(Reply)) -> bool {
        if let Some((req, queued)) = self.waiting.pop_front() {
            let outcome = self
                .ask(&req, queued, stopping, true)
                .unwrap_or(Outcome::Failed(Failure::Error));
            send(Reply { id: req.id, outcome });
        } else if let Some(summary) = self.overdue.pop_front() {
            if !stopping {
                self.ask_notes(&summary);
            }
        } else {
            return false;
        }
        self.refresh_status();
        true
    }

    /// New settings; another provider starts afresh.
    fn configure(&mut self, config: ChatConfig) {
        if self.config.provider != config.provider {
            self.start_over();
        }
        self.config = config;
        self.sync_transcript();
    }

    /// The next request goes to the provider whatever it answered last: a new client, no refusal, no wait.
    fn start_over(&mut self) {
        self.client = None;
        self.disabled = None;
        self.backoff_until = None;
        self.failures = 0;
        self.out_of_credit = None;
    }

    /// `lb chat reload` and `lb config reload`: `config/chat/` read again, the provider tried at the next request.
    fn reload(&mut self) {
        self.library.reload(&self.paths.chat);
        self.start_over();
        let summary = self.library.summary();
        tracing::info!("chat: config/chat/ read again: {}", summary.join(" · "));
        logging::console_line(
            "[lambdabots] chat: config/chat/ read again, the next request goes to the provider:".into(),
        );
        for line in summary {
            logging::console_line(format!("  {line}"));
        }
    }

    /// `lb chat prompt`: the prompt a request makes, on the server console.
    fn preview(&self, req: &ChatRequest) {
        let r = self.render(req);
        for (title, text) in [("system", &r.system_static), ("bot", &r.system), ("request", &r.user)] {
            logging::console_line(format!("[lambdabots] chat prompt, {title}:"));
            for line in text.lines() {
                logging::console_line(format!("  {line}"));
            }
        }
    }

    /// The answer to a request: a phrase for a moment while the model is out of reach or when it is not the model's
    /// turn ([`Worker::models_turn`]); no line for the rest while the model is out of reach; else the model's line,
    /// or a phrase in its stead when the request fails while the moment is fresh ([`PHRASE_FRESH`]). `None`: the model
    /// is to be asked, which `network` alone allows.
    fn ask(&mut self, req: &ChatRequest, queued: Instant, stopping: bool, network: bool) -> Option<Outcome> {
        if stopping {
            return Some(Outcome::Failed(Failure::Backoff));
        }
        let aliases = self.aliases(req);
        let offer = self.offer(req, &aliases);
        let mut rng = Pcg32::new(self.seed ^ req.id, PHRASE_STREAM);
        let blocked = self.blocked();
        if let Some(offer) = &offer
            && (blocked.is_some() || !self.models_turn(req, offer, &mut rng))
        {
            return Some(self.phrase(req, offer, &mut rng, blocked));
        }
        if let Some(why) = blocked {
            return Some(Outcome::Failed(why));
        }
        if !network {
            return None;
        }
        if queued.elapsed() > STALE {
            return Some(Outcome::Failed(Failure::Backoff));
        }
        if offer.is_none()
            && self.config.phrases.enabled
            && let Some(moment) = phrases::key(&req.trigger, req.bot.userid)
        {
            let language = phrases::language(&self.library.phrases, req, &self.remembered(req));
            tracing::debug!("chat {}: no phrase for {} ({language})", req.bot.name, moment.key());
        }
        Some(match (self.ask_model(req, &aliases), &offer) {
            (Ok(outcome), _) => outcome,
            (Err(why), Some(offer)) if queued.elapsed() < PHRASE_FRESH => self.phrase(req, offer, &mut rng, Some(why)),
            (Err(why), _) => Outcome::Failed(why),
        })
    }

    /// What `phrases.yaml` offers a request's moment; nothing with `chat.phrases.enabled` off.
    fn offer(&self, req: &ChatRequest, aliases: &Aliases) -> Option<Offer> {
        if !self.config.phrases.enabled {
            return None;
        }
        Offer::of(&self.library.phrases, req, aliases, &self.remembered(req))
    }

    /// Whether the model meets a moment a phrase could: a greeting of a player the bots know, any other moment one
    /// time in `chat.phrases.ai_share`.
    fn models_turn(&self, req: &ChatRequest, offer: &Offer, rng: &mut Pcg32) -> bool {
        match offer.moment {
            Moment::Greet => prompt::partner(req).is_some_and(|who| self.knows(req, who)),
            _ => rng.next_f32() < self.config.phrases.ai_share,
        }
    }

    /// Whether the bots know a player: the memory has them by their key or by a name they used, or `players.yaml` has
    /// an entry for them (by nickname, or with aliases alone, too).
    fn knows(&self, req: &ChatRequest, who: &Who) -> bool {
        let key = req
            .scene
            .players
            .iter()
            .find(|p| p.name == who.name)
            .and_then(|p| p.key.as_deref())
            .unwrap_or("");
        self.memory.players.contains_key(key)
            || self.memory.find(&who.name).is_some()
            || self.library.players.has(key, &who.name)
    }

    /// A phrase of the offer, picked past the ring; `model`: why the model does not meet the moment, for the log.
    fn phrase(&mut self, req: &ChatRequest, offer: &Offer, rng: &mut Pcg32, model: Option<Failure>) -> Outcome {
        let Some(text) = offer.pick(&mut self.ring, &req.bot, rng) else {
            return Outcome::Failed(model.unwrap_or(Failure::Error));
        };
        tracing::info!(
            "chat {}: {} -> phrase {}: {text}{}",
            req.bot.name,
            req.trigger.summary(),
            offer.moment.key(),
            model.map(|why| format!(", the model {why:?}")).unwrap_or_default()
        );
        Outcome::Phrase(text)
    }

    /// The model's line for a request, through the filter ([`verdict`]); silence when it has none.
    fn ask_model(&mut self, req: &ChatRequest, aliases: &Aliases) -> Result<Outcome, Failure> {
        let started = Instant::now();
        let rendered = self.render(req);
        tracing::debug!(
            "chat prompt for {}:\n{}\n{}",
            req.bot.name,
            rendered.system,
            rendered.user
        );
        let names = names(req, aliases);
        let title = format!("{} · {}", req.bot.name, req.trigger.summary());
        let done = self.complete(rendered, &title, |done| match verdict(req, aliases, &names, done) {
            Ok(line) => format!("line: {line}"),
            Err(why) => format!("no line: {why}"),
        })?;
        let line = verdict(req, aliases, &names, &done);
        tracing::info!(
            "chat {}: {} -> {} ({} ms, {} + {} tokens)",
            req.bot.name,
            req.trigger.summary(),
            match &line {
                Ok(line) => line.clone(),
                Err(why) => format!("no line: {why}"),
            },
            started.elapsed().as_millis(),
            done.input_tokens,
            done.output_tokens
        );
        Ok(line.map_or(Outcome::Skip, Outcome::Line))
    }

    /// The prompt of a request, the same for the model and for `lb chat prompt`: with what the admin wrote of the
    /// server, the bot and the map, and the bot's talks with the player the memory keeps.
    fn render(&self, req: &ChatRequest) -> Rendered {
        let known = self.known(req);
        let aliases = self.aliases(req);
        let map = self.library.map(&req.scene.map);
        let talks = self
            .partner_memory(req)
            .and_then(|m| m.talks.get(&req.bot.persona))
            .map_or(&[][..], Vec::as_slice);
        let ctx = Context {
            server: &self.config.server,
            server_text: self.library.server(),
            bot: self.library.bot(&req.bot.persona, &req.bot.name),
            map: &map,
            talks,
        };
        prompt::render(req, &known, &aliases, &self.memory.maps, &ctx, unix_now())
    }

    /// What the bots know of the players a request shows, and of the player it is for when they are gone.
    fn known<'a>(&'a self, req: &'a ChatRequest) -> Vec<Known<'a>> {
        let mut known: Vec<Known<'a>> = req
            .scene
            .players
            .iter()
            .filter(|p| !p.me)
            .filter_map(|p| {
                let key = p.key.as_deref()?;
                let note = self.library.players.get(key, &p.name);
                let memory = self.memory.players.get(key);
                (note.is_some() || memory.is_some()).then_some(Known {
                    name: &p.name,
                    note,
                    memory,
                })
            })
            .collect();
        if let Some(who) = prompt::partner(req)
            && !req.scene.players.iter().any(|p| p.name == who.name)
        {
            let found = self.memory.find(&who.name);
            let note = self
                .library
                .players
                .get(found.map_or("", |(key, _)| key.as_str()), &who.name);
            let memory = found.map(|(_, m)| m);
            if note.is_some() || memory.is_some() {
                known.push(Known {
                    name: &who.name,
                    note,
                    memory,
                });
            }
        }
        known
    }

    /// The memory of the player a request is for ([`prompt::partner`]): by their key when they are on the
    /// scoreboard, else by name.
    fn partner_memory(&self, req: &ChatRequest) -> Option<&PlayerMemory> {
        let who = prompt::partner(req)?;
        match req.scene.players.iter().find(|p| p.name == who.name) {
            Some(p) => self.memory.players.get(p.key.as_deref()?),
            None => self.memory.find(&who.name).map(|(_, m)| m),
        }
    }

    /// The lines the memory keeps of the player a request is for: those they wrote, and theirs in their talks with
    /// the bot.
    fn remembered(&self, req: &ChatRequest) -> Vec<&str> {
        let Some(m) = self.partner_memory(req) else {
            return Vec::new();
        };
        let talked = m
            .talks
            .get(&req.bot.persona)
            .into_iter()
            .flatten()
            .filter(|(_, mine, _)| !mine)
            .map(|(_, _, text)| text.as_str());
        m.lines.iter().map(|(_, line)| line.as_str()).chain(talked).collect()
    }

    /// The aliases of the players a request shows or speaks of.
    fn aliases(&self, req: &ChatRequest) -> Aliases {
        let notes = &self.library.players;
        let mut aliases = Aliases::default();
        for p in &req.scene.players {
            aliases.insert(&p.name, notes.aliases(p.key.as_deref().unwrap_or_default(), &p.name));
        }
        let people = req
            .events
            .iter()
            .chain(&req.chat)
            .flat_map(|r| r.event.people())
            .chain(req.trigger.people())
            .filter(|w| !req.scene.players.iter().any(|p| p.name == w.name));
        for who in people {
            aliases.insert(&who.name, notes.aliases("", &who.name));
        }
        aliases
    }

    /// A map ended: the memory takes it in and is saved at once; its notes request waits for a quiet moment
    /// ([`NOTES_IDLE`]). The one still waiting from the map before is due now ([`Worker::overdue`]).
    fn map_end(&mut self, s: Box<MapSummary>) {
        self.overdue.extend(self.notes_due.take());
        if !self.config.memory.enabled || s.players.is_empty() {
            return;
        }
        let now = unix_now();
        self.memory.merge(&s, now);
        self.memory.prune(now, self.config.memory.forget_after_days);
        self.memory_dirty = true;
        self.save();
        if self.config.memory.ai_notes {
            self.notes_due = Some(s);
        }
    }

    /// Asks the model for notes on a map's players, on top of those it wrote before.
    fn ask_notes(&mut self, s: &MapSummary) {
        if !self.config.memory.enabled || !self.config.memory.ai_notes || self.blocked().is_some() {
            return;
        }
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
        let Some(rendered) = prompt::render_notes(s, &previous, &self.summary_aliases(s)) else {
            return;
        };
        let Ok(done) = self.complete(rendered, &format!("notes after {}", s.map), |done| {
            format!("notes on {} players", prompt::parse_notes(&done.text).len())
        }) else {
            return;
        };
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
        self.memory_dirty = true;
        self.save();
    }

    fn summary_aliases(&self, s: &MapSummary) -> Aliases {
        let mut aliases = Aliases::default();
        for p in &s.players {
            aliases.insert(&p.name, self.library.players.aliases(&p.key, &p.name));
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
                    let talks: Vec<String> = m
                        .talks
                        .iter()
                        .map(|(bot, lines)| format!("{bot} {}", lines.len()))
                        .collect();
                    let name = m.names.first().map_or("", String::as_str);
                    format!(
                        "chat: {key} ({}): {} maps, {}/{} kills/deaths, {} wins, weapons {:?}; vs bots: {}; notes: {}; \
                         lines: {:?}; talks (lines): {}; admin note: {}; alias: {}",
                        m.names.join(", "),
                        m.maps,
                        m.kills,
                        m.deaths,
                        m.wins,
                        m.favourite_weapons(),
                        duels.join(", "),
                        if m.notes.is_empty() { "-" } else { &m.notes },
                        lines,
                        if talks.is_empty() {
                            "-".to_string()
                        } else {
                            talks.join(", ")
                        },
                        self.library.players.get(key, name).unwrap_or("-"),
                        match self.library.players.aliases(key, name) {
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

/// The names a request's line may hold, which the filter leaves out: the scoreboard's, the trigger's, every alias.
fn names(req: &ChatRequest, aliases: &Aliases) -> Vec<String> {
    req.scene
        .players
        .iter()
        .map(|p| p.name.clone())
        .chain(req.trigger.people().into_iter().map(|w| w.name.clone()))
        .chain(aliases.words())
        .collect()
}

/// Whether the player a request is for ([`prompt::partner`]) spoke of cheats: in the line it answers, or in their
/// lines it shows.
fn raised(req: &ChatRequest, names: &[String]) -> bool {
    let Some(who) = prompt::partner(req) else {
        return false;
    };
    let line = req
        .trigger
        .line()
        .filter(|(from, _)| from.userid == who.userid)
        .map(|(_, text)| text);
    let chat = req.chat.iter().filter_map(|r| match &r.event {
        Event::Chat { from, text, .. } if from.userid == who.userid => Some(text.as_str()),
        _ => None,
    });
    let talk = req.talk.iter().filter(|s| !s.mine).map(|s| s.text.as_str());
    line.into_iter()
        .chain(chat)
        .chain(talk)
        .any(|text| profanity::cheating(text, names))
}

/// What of the model's answer goes into the chat, in this order: nothing when it refused or kept quiet; the line
/// cleaned up, players called by their aliases; nothing when the line holds a slur (whatever the bot's `profanity`),
/// swearing from a bot without `profanity`, or talk of cheats the player did not start ([`raised`]). `names`: those
/// the line may hold ([`names`]). `Err`: why there is no line, a dropped one with it.
fn verdict(req: &ChatRequest, aliases: &Aliases, names: &[String], done: &Completion) -> Result<String, String> {
    if done.stop == Stop::Refusal {
        return Err("the model refused".into());
    }
    let line = aliases.apply(&sanitize::clean_reply(&done.text, &req.bot.name).ok_or("the model keeps quiet")?);
    let dropped = if profanity::slur(&line, names) {
        "slur"
    } else if !req.bot.profanity && profanity::has(&line, names) {
        "swearing"
    } else if profanity::cheating(&line, names) && !raised(req, names) {
        "cheating"
    } else {
        return Ok(line);
    };
    Err(format!("dropped ({dropped}): {line}"))
}

#[cfg(test)]
mod tests {
    use lb_chat::request::{BotCard, PlayerCard, PlayerMap, Recent, Scene};
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

    /// The bot of [`request`].
    fn kleiner() -> Who {
        Who {
            bot: true,
            ..who(2, "Kleiner")
        }
    }

    fn request(id: u64) -> ChatRequest {
        ChatRequest {
            id,
            bot: BotCard {
                name: "Kleiner".into(),
                persona: "Kleiner".into(),
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
            chat: Vec::new(),
            own: Vec::new(),
            talk: Vec::new(),
            language: "en".into(),
            max_chars: 60,
            team: false,
        }
    }

    fn event(id: u64, trigger: Trigger) -> ChatRequest {
        ChatRequest { trigger, ..request(id) }
    }

    /// The bot won the match: a moment every phrase of fills.
    fn win(id: u64) -> ChatRequest {
        event(
            id,
            Trigger::MatchEnd {
                winner: Some(kleiner()),
                won: true,
            },
        )
    }

    fn ask(req: ChatRequest) -> Job {
        Job::Ask(Box::new(req))
    }

    fn answer(text: &str) -> Canned {
        let body = serde_json::json!({
            "choices": [{"message": {"content": text}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 5},
        });
        Canned::json(200, &body.to_string())
    }

    /// Moonshot's refusal once the money ran out (as in lb-llm's tests), the account and key ids replaced.
    const UNPAID: &str = "Your account org-test <ak-test> is suspended due to insufficient balance, please recharge \
                          your account or check your plan and billing details";

    fn unpaid() -> Canned {
        let body = serde_json::json!({"error": {"message": UNPAID, "type": "exceeded_current_quota_error"}});
        Canned::json(429, &body.to_string())
    }

    /// A worker run by hand, its chances the same each time.
    fn worker(root: &Path, config: ChatConfig) -> Worker {
        let mut w = Worker::new(config, Paths::new(root), Arc::default());
        w.seed = 7;
        w
    }

    /// The worker takes `jobs` in, which came at those times, then does what waits for the network, as its thread
    /// would with no more jobs coming: the replies, in the order they went.
    fn wake_at(w: &mut Worker, jobs: Vec<(Job, Instant)>, stopping: bool) -> Vec<Reply> {
        let mut out = Vec::new();
        let mut send = |reply| out.push(reply);
        for (job, queued) in jobs {
            w.take(job, queued, stopping, &mut send);
        }
        while w.next(stopping, &mut send) {}
        out
    }

    /// [`wake_at`] with `jobs` that came now.
    fn wake(w: &mut Worker, jobs: Vec<Job>) -> Vec<Reply> {
        let now = Instant::now();
        wake_at(w, jobs.into_iter().map(|job| (job, now)).collect(), false)
    }

    fn status(w: &Worker) -> String {
        w.status.lock().unwrap().clone()
    }

    fn of(replies: &[Reply], id: u64) -> &Outcome {
        &replies.iter().find(|r| r.id == id).unwrap().outcome
    }

    fn phrase(outcome: &Outcome) -> bool {
        matches!(outcome, Outcome::Phrase(text) if !text.is_empty())
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
        let mut w = WorkerBackend::spawn(config(&server.url()), &root, |w| {
            w.notes_idle = Duration::from_millis(50)
        })
        .unwrap();
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
        assert!(
            w.status().contains("players.yaml: 1 player · server.yaml: not found"),
            "{}",
            w.status()
        );
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
        assert_eq!(server.requests().len(), 3, "the notes, once nothing came for a while");
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
    fn a_nickname_entry_is_for_players_without_a_steamid() {
        let root = dir("nickname");
        std::fs::create_dir_all(root.join("config/chat")).unwrap();
        std::fs::write(
            root.join("config/chat/players.yaml"),
            "schema: lambdabots/chat-players@1\nplayers:\n  - id: Gordon\n    alias: Гордон\n  - id: \"[B] Barney\"\n    \
             by_name: true\n    alias: Барни\n",
        )
        .unwrap();
        let w = worker(&root, config("http://127.0.0.1:9"));
        let mut req = request(1);
        assert_eq!(
            w.aliases(&req).of("Gordon"),
            None,
            "Gordon has a SteamID, and writes the trigger"
        );
        req.scene.players[0].key = Some("name:gordon".into());
        assert_eq!(w.aliases(&req).of("Gordon"), Some("Гордон"));
        req.scene.players.push(PlayerCard {
            name: "[B] Barney".into(),
            key: Some("STEAM_0:0:9".into()),
            ..req.scene.players[0].clone()
        });
        assert_eq!(
            w.aliases(&req).of("[B] Barney"),
            Some("Барни"),
            "by_name: whatever the SteamID"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_transcript_holds_requests_and_answers() {
        let server = MockServer::start(vec![
            answer("hey"),
            Canned::json(500, r#"{"error":{"message":"overloaded"}}"#),
        ]);
        let root = dir("transcript");
        let mut c = config(&server.url());
        c.provider.api_key = lb_config::main_config::Secret("sk-hunter2".into());
        c.transcript = true;
        let mut w = WorkerBackend::start(c, &root).unwrap();
        w.send(Job::Ask(Box::new(request(1))));
        w.send(Job::Ask(Box::new(request(2))));
        assert_eq!(replies(&mut w, 2).len(), 2);
        w.shutdown(Duration::from_secs(5));
        let text = std::fs::read_to_string(super::super::transcript::today(&root.join("logs"))).unwrap();
        assert!(
            text.contains("UTC · Kleiner · Gordon to the bot: kleiner, hi"),
            "{text}"
        );
        assert!(
            text.contains(&format!(">>> POST {}/v1/chat/completions", server.url())),
            "{text}"
        );
        assert!(
            text.contains("- content: |-") && text.contains("role: system"),
            "{text}"
        );
        assert!(text.contains("<<< 200 in") && text.contains("=== line: hey"), "{text}");
        assert!(
            text.contains("<<< 500 in") && text.contains("=== failed: HTTP 500: overloaded"),
            "{text}"
        );
        assert!(!text.contains("hunter2"), "no key in the transcript");
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
        assert!(
            w.status()
                .starts_with("the day's 100 tokens are spent, requests wait for 00:00 UTC; "),
            "{}",
            w.status()
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

    #[test]
    fn out_of_api_credit_one_try_waits_for_the_next() {
        let server = MockServer::start(vec![unpaid(), unpaid(), answer("back again")]);
        let root = dir("credit");
        let mut w = worker(&root, config(&server.url()));
        w.unpaid_wait = Duration::from_millis(400);
        w.failures = 2;
        let got = wake(&mut w, vec![ask(request(1)), ask(request(2))]);
        assert_eq!(*of(&got, 1), Outcome::Failed(Failure::Billing));
        assert_eq!(*of(&got, 2), Outcome::Failed(Failure::Billing));
        assert_eq!(server.requests().len(), 1, "the second waits, unsent");
        let (message, since) = w.out_of_credit.clone().unwrap();
        assert_eq!(message, format!("HTTP 429: {UNPAID}"));
        assert_eq!(w.failures, 0);
        let s = status(&w);
        assert!(
            s.starts_with(&format!("out of API credit since {}, next try at ", utc(since))),
            "{s}"
        );
        assert!(
            s.contains(&format!(" UTC (`lb chat reload` tries now): HTTP 429: {UNPAID}; ")),
            "{s}"
        );
        std::thread::sleep(Duration::from_millis(450));
        w.refresh_status();
        assert!(
            status(&w).starts_with(&format!(
                "out of API credit since {}, the next request tries again: HTTP 429: Your account",
                utc(since)
            )),
            "{}",
            status(&w)
        );
        let got = wake(&mut w, vec![ask(request(3))]);
        assert_eq!(
            *of(&got, 3),
            Outcome::Failed(Failure::Billing),
            "the try after the wait"
        );
        assert_eq!(server.requests().len(), 2);
        assert_eq!(
            w.out_of_credit.as_ref().map(|o| o.1),
            Some(since),
            "still since the first"
        );
        std::thread::sleep(Duration::from_millis(450));
        let got = wake(&mut w, vec![ask(request(4))]);
        assert_eq!(*of(&got, 4), Outcome::Line("back again".into()));
        assert!(w.out_of_credit.is_none() && w.backoff_until.is_none());
        assert!(status(&w).starts_with("ready; "), "{}", status(&w));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_reload_or_another_provider_tries_at_once() {
        // Anthropic's answer to an account without credit (lb-llm's tests, from its docs and bug reports).
        let body = serde_json::json!({"type": "error", "error": {
            "type": "invalid_request_error",
            "message": "Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing to \
                        upgrade or purchase credits."
        }});
        let server = MockServer::start(vec![Canned::json(400, &body.to_string())]);
        let root = dir("credit-reset");
        let mut c = ChatConfig {
            enabled: true,
            ..ChatConfig::default()
        };
        c.provider.base_url = server.url();
        c.provider.api_key_env = String::new();
        c.provider.timeout = 2.0;
        let mut w = worker(&root, c.clone());
        assert!(
            status(&w).ends_with("maps.yaml: not found · phrases: built-in"),
            "{}",
            status(&w)
        );
        assert_eq!(
            wake(&mut w, vec![ask(request(1))])[0].outcome,
            Outcome::Failed(Failure::Billing)
        );
        assert!(
            status(&w).contains("HTTP 400: Your credit balance is too low"),
            "{}",
            status(&w)
        );
        wake(&mut w, vec![Job::Reload]);
        assert!(w.out_of_credit.is_none() && w.backoff_until.is_none());
        assert!(status(&w).starts_with("ready; "), "{}", status(&w));
        assert_eq!(
            wake(&mut w, vec![ask(request(2))])[0].outcome,
            Outcome::Failed(Failure::Billing)
        );
        assert_eq!(server.requests().len(), 2, "the reload let the next request go at once");
        let mut same = c.clone();
        same.phrases.ai_share = 0.5;
        wake(&mut w, vec![Job::Configure(Box::new(same))]);
        assert!(w.out_of_credit.is_some(), "the same provider: still out of credit");
        w.failures = 2;
        let mut other = c;
        other.provider.model = "claude-other".into();
        wake(&mut w, vec![Job::Configure(Box::new(other))]);
        assert!(w.out_of_credit.is_none() && w.backoff_until.is_none() && w.failures == 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn moments_get_phrases_without_a_key() {
        let root = dir("phrases-nokey");
        let mut c = ChatConfig {
            enabled: true,
            ..ChatConfig::default()
        };
        c.provider.api_key_env = "LB_TEST_SURELY_UNSET_KEY".into();
        c.phrases.ai_share = 1.0;
        let mut w = worker(&root, c);
        let got = wake(&mut w, vec![ask(win(1)), ask(request(2)), ask(win(3))]);
        assert!(phrase(of(&got, 1)), "the model's turn failed: a phrase instead");
        assert_eq!(*of(&got, 2), Outcome::Failed(Failure::Disabled));
        assert!(phrase(of(&got, 3)), "{:?}", of(&got, 3));
        assert!(w.disabled.is_some());
        let got = wake(&mut w, vec![ask(request(4)), ask(win(5))]);
        assert_eq!(got.iter().map(|r| r.id).collect::<Vec<_>>(), [4, 5], "both at once");
        assert!(phrase(of(&got, 5)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn moments_get_phrases_out_of_api_credit() {
        let server = MockServer::start(vec![unpaid()]);
        let root = dir("phrases-credit");
        let mut c = config(&server.url());
        c.phrases.ai_share = 1.0;
        let mut w = worker(&root, c);
        let got = wake(&mut w, vec![ask(win(1)), ask(win(2))]);
        assert!(phrase(of(&got, 1)) && phrase(of(&got, 2)), "{got:?}");
        assert_ne!(of(&got, 1), of(&got, 2), "the ring keeps them apart");
        assert_eq!(server.requests().len(), 1);
        let since = w.out_of_credit.as_ref().unwrap().1;
        let got = wake(&mut w, vec![ask(win(3)), ask(request(4))]);
        assert!(phrase(of(&got, 3)));
        assert_eq!(*of(&got, 4), Outcome::Failed(Failure::Billing));
        assert_eq!(server.requests().len(), 1);
        assert_eq!(
            (w.out_of_credit.as_ref().map(|o| o.1), w.failures),
            (Some(since), 0),
            "phrases change nothing of it"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_moment_whose_request_failed_gets_a_phrase_while_fresh() {
        let server = MockServer::start(vec![Canned::json(500, r#"{"error":{"message":"overloaded"}}"#)]);
        let root = dir("phrases-failed");
        let mut c = config(&server.url());
        c.phrases.ai_share = 1.0;
        let mut w = worker(&root, c.clone());
        let got = wake(&mut w, vec![ask(win(1))]);
        assert!(phrase(of(&got, 1)), "{got:?}");
        assert_eq!(w.failures, 1);
        let mut w = worker(&root, c);
        let older = Instant::now()
            .checked_sub(PHRASE_FRESH + Duration::from_secs(1))
            .unwrap();
        let got = wake_at(&mut w, vec![(ask(win(2)), older)], false);
        assert_eq!(
            *of(&got, 2),
            Outcome::Failed(Failure::Backoff),
            "too late for the moment"
        );
        assert_eq!(server.requests().len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn phrases_never_wait_behind_the_model() {
        let server = MockServer::start(vec![answer("hi").delay(Duration::from_millis(300))]);
        let root = dir("fast");
        let mut c = config(&server.url());
        c.phrases.ai_share = 0.0;
        let mut w = worker(&root, c);
        let now = Instant::now();
        let long_ago = now.checked_sub(STALE + Duration::from_secs(10)).unwrap();
        let got = wake_at(
            &mut w,
            vec![
                (ask(request(1)), now),
                (ask(win(2)), now),
                (ask(request(3)), long_ago),
                (ask(win(4)), long_ago),
            ],
            false,
        );
        assert_eq!(
            got.iter().map(|r| r.id).collect::<Vec<_>>(),
            [2, 4, 1, 3],
            "phrases at once, then the model's requests in order"
        );
        assert!(phrase(of(&got, 2)) && phrase(of(&got, 4)), "a phrase is never stale");
        assert_eq!(*of(&got, 1), Outcome::Line("hi".into()));
        assert_eq!(*of(&got, 3), Outcome::Failed(Failure::Backoff));
        assert_eq!(server.requests().len(), 1, "a stale request is not sent");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_phrase_waits_for_one_model_request_at_most() {
        let server = MockServer::start(vec![answer("hi").delay(Duration::from_millis(300))]);
        let root = dir("between");
        let mut c = config(&server.url());
        c.phrases.ai_share = 0.0;
        let mut w = WorkerBackend::start(c, &root).unwrap();
        let on_the_wire = |n: usize| {
            let until = Instant::now() + Duration::from_secs(10);
            while server.requests().len() < n && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        w.send(ask(request(1)));
        on_the_wire(1);
        w.send(ask(request(2)));
        w.send(ask(request(3)));
        on_the_wire(2);
        w.send(ask(win(4)));
        let ids: Vec<u64> = replies(&mut w, 4).iter().map(|r| r.id).collect();
        assert_eq!(
            ids,
            [1, 2, 4, 3],
            "the phrase goes between the model's requests, not after them all"
        );
        w.shutdown(Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn players_the_bots_know_are_greeted_by_the_model() {
        let server = MockServer::start(vec![answer("hey Gordon, long time")]);
        let root = dir("greet");
        let mut c = config(&server.url());
        c.phrases.ai_share = 0.0;
        let mut w = worker(&root, c);
        let joined = |id| event(id, Trigger::Joined { who: who(1, "Gordon") });
        let got = wake(&mut w, vec![ask(joined(1))]);
        assert!(phrase(of(&got, 1)), "a stranger gets a phrase: {got:?}");
        assert_eq!(server.requests().len(), 0);
        let model = Outcome::Line("hey Gordon, long time".into());
        w.memory.players.insert("STEAM_0:1:42".into(), PlayerMemory::default());
        assert_eq!(wake(&mut w, vec![ask(joined(2))])[0].outcome, model, "by the key");
        w.memory.players.clear();
        let pending = PlayerMemory {
            names: vec!["Gordon".into()],
            ..Default::default()
        };
        w.memory.players.insert("name:gordon".into(), pending);
        assert_eq!(
            wake(&mut w, vec![ask(joined(3))])[0].outcome,
            model,
            "by a name they used"
        );
        w.memory.players.clear();
        std::fs::create_dir_all(root.join("config/chat")).unwrap();
        std::fs::write(
            root.join("config/chat/players.yaml"),
            "schema: lambdabots/chat-players@1\nplayers:\n  - id: gordon\n    by_name: true\n    alias: Гордон\n",
        )
        .unwrap();
        wake(&mut w, vec![Job::Reload]);
        let model = Outcome::Line("hey Гордон, long time".into());
        assert_eq!(
            wake(&mut w, vec![ask(joined(4))])[0].outcome,
            model,
            "players.yaml, aliases alone"
        );
        let hello = event(
            5,
            Trigger::Greeted {
                from: who(1, "Gordon"),
                text: "hi all".into(),
            },
        );
        assert_eq!(wake(&mut w, vec![ask(hello)])[0].outcome, model);
        let stranger = event(
            6,
            Trigger::Greeted {
                from: who(3, "Barney"),
                text: "hi all".into(),
            },
        );
        assert!(phrase(&wake(&mut w, vec![ask(stranger)])[0].outcome));
        assert_eq!(server.requests().len(), 4);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn said(text: &str) -> Completion {
        Completion {
            text: text.into(),
            stop: Stop::End,
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    fn chat(from: Who, text: &str) -> Recent {
        Recent {
            age: 20.0,
            event: Event::Chat {
                from,
                text: text.into(),
                team: false,
            },
        }
    }

    #[test]
    fn the_filter_drops_what_a_bot_may_not_say() {
        let mut aliases = Aliases::default();
        aliases.insert("Gordon", &["Гордон"]);
        let say = |req: &ChatRequest, text: &str| verdict(req, &aliases, &names(req, &aliases), &said(text));
        let req = request(1);
        assert_eq!(say(&req, "Kleiner: \"hey Gordon\""), Ok("hey Гордон".into()));
        assert_eq!(say(&req, "-"), Err("the model keeps quiet".into()));
        let refused = Completion {
            stop: Stop::Refusal,
            ..said("hey")
        };
        assert_eq!(verdict(&req, &aliases, &[], &refused), Err("the model refused".into()));
        let mut rude = request(1);
        rude.bot.profanity = true;
        assert_eq!(say(&req, "ну ты и пидор"), Err("dropped (slur): ну ты и пидор".into()));
        assert_eq!(
            say(&rude, "ну ты и пидор"),
            Err("dropped (slur): ну ты и пидор".into()),
            "whatever the bot's profanity"
        );
        assert_eq!(
            say(&req, "блять, опять"),
            Err("dropped (swearing): блять, опять".into())
        );
        assert_eq!(say(&rude, "блять, опять"), Ok("блять, опять".into()));
        let mut named = request(1);
        named.scene.players[0].name = "FUCK_YOU_player".into();
        assert_eq!(
            say(&named, "FUCK_YOU_player, gg"),
            Ok("FUCK_YOU_player, gg".into()),
            "a nickname is no swearing"
        );
        assert_eq!(say(&req, "да ты читер"), Err("dropped (cheating): да ты читер".into()));
        let mut asked = request(1);
        asked.trigger = Trigger::Addressed {
            from: who(1, "Gordon"),
            text: "y teb9 4it est?".into(),
        };
        assert_eq!(
            say(&asked, "ага, чит называется скилл"),
            Ok("ага, чит называется скилл".into()),
            "the player spoke of cheats first"
        );
        let mut earlier = request(1);
        earlier.chat = vec![chat(who(1, "Gordon"), "kleiner wh?")];
        assert!(say(&earlier, "какой ещё wh").is_ok(), "in a line the request shows");
        earlier.chat = vec![chat(who(3, "Barney"), "kleiner wh?")];
        assert!(say(&earlier, "какой ещё wh").is_err(), "another player's line");
    }

    #[test]
    fn a_dropped_line_is_silence_in_the_transcript() {
        let server = MockServer::start(vec![answer("блять, опять ты")]);
        let root = dir("dropped");
        let mut c = config(&server.url());
        c.transcript = true;
        let mut w = worker(&root, c);
        assert_eq!(wake(&mut w, vec![ask(request(1))])[0].outcome, Outcome::Skip);
        let text = std::fs::read_to_string(super::super::transcript::today(&root.join("logs"))).unwrap();
        assert!(
            text.contains("=== no line: dropped (swearing): блять, опять ты"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_prompt_holds_what_the_admin_wrote_and_the_talks_kept() {
        let root = dir("context");
        let chat = root.join("config/chat");
        std::fs::create_dir_all(&chat).unwrap();
        let write = |name: &str, text: &str| std::fs::write(chat.join(name), text).unwrap();
        write(
            "server.yaml",
            "schema: lambdabots/chat-server@1\ncontext: GunGame all night\n",
        );
        write(
            "bots.yaml",
            "schema: lambdabots/chat-bots@1\nbots:\n  - name: Kleiner\n    context: a scientist of Black Mesa\n  - \
             name: \"[B] Kleiner\"\n    context: the other one\n",
        );
        write(
            "maps.yaml",
            "schema: lambdabots/chat-maps@1\nmaps:\n  - map: crossfire\n    note: the rails by the bridge\n",
        );
        let mut w = worker(&root, config("http://127.0.0.1:9"));
        let two_days_ago = unix_now() - 2 * 86_400;
        let gordon = PlayerMemory {
            names: vec!["Gordon".into()],
            talks: BTreeMap::from([(
                "Kleiner".to_string(),
                vec![
                    (two_days_ago, false, "where are the rails?".to_string()),
                    (two_days_ago + 5, true, "follow me, Gordon".to_string()),
                ],
            )]),
            ..Default::default()
        };
        w.memory.players.insert("STEAM_0:1:42".into(), gordon);
        let mut req = request(1);
        req.bot.name = "[B] Kleiner".into();
        let r = w.render(&req);
        assert!(r.system_static.contains("GunGame all night"), "{}", r.system_static);
        assert!(
            r.system.contains("a scientist of Black Mesa") && !r.system.contains("the other one"),
            "by persona first: {}",
            r.system
        );
        assert!(r.user.contains("the rails by the bridge"), "{}", r.user);
        assert!(
            r.user.contains("where are the rails?") && r.user.contains("follow me, Gordon"),
            "{}",
            r.user
        );
        let mut other = request(1);
        other.bot.persona = "Eli".into();
        other.bot.name = "[B] Kleiner".into();
        let r = w.render(&other);
        assert!(r.system.contains("the other one"), "else by nickname: {}", r.system);
        assert!(!r.user.contains("where are the rails?"), "talks go by the persona");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_map_is_kept_at_once_and_its_notes_wait_for_a_quiet_moment() {
        let server = MockServer::start(vec![
            answer("hi there"),
            answer("{\"STEAM_0:1:42\": \"plays the crossbow\"}"),
        ]);
        let root = dir("notes");
        let mut c = config(&server.url());
        c.phrases.ai_share = 0.0;
        let mut w = worker(&root, c);
        let summary = |map: &str| {
            Box::new(MapSummary {
                map: map.into(),
                language: "en".into(),
                minutes: 10,
                players: vec![PlayerMap {
                    key: "STEAM_0:1:42".into(),
                    name: "Gordon".into(),
                    vs_bots: vec![("Kleiner".into(), 3, 1)],
                    kills: 3,
                    deaths: 1,
                    ..Default::default()
                }],
                ..Default::default()
            })
        };
        let kept = || store::load_memory(&root.join("data/chat/memory.json"));
        wake(&mut w, vec![Job::MapEnd(summary("crossfire"))]);
        assert_eq!(server.requests().len(), 0, "the notes wait");
        assert_eq!(kept().players["STEAM_0:1:42"].maps, 1, "the map is kept at once");
        let got = wake(
            &mut w,
            vec![Job::MapEnd(summary("stalkyard")), ask(win(1)), ask(request(2))],
        );
        assert!(phrase(of(&got, 1)), "{got:?}");
        assert_eq!(*of(&got, 2), Outcome::Line("hi there".into()));
        let asked = server.requests();
        assert_eq!(asked.len(), 2, "due by the next map's end");
        let notes = |body: &str| body.contains("Update the note") && body.contains("Map crossfire, 10 min");
        assert!(!notes(&asked[0].body) && notes(&asked[1].body), "after the answers");
        assert_eq!(kept().players["STEAM_0:1:42"].notes, "plays the crossbow");
        assert_eq!(w.notes_due.as_ref().map(|s| s.map.as_str()), Some("stalkyard"));
        wake_at(&mut w, vec![(Job::MapEnd(summary("bounce")), Instant::now())], true);
        assert_eq!(server.requests().len(), 2, "no notes once the worker stops");
        assert!(w.overdue.is_empty());
        assert_eq!(kept().players["STEAM_0:1:42"].maps, 3, "the map is kept all the same");
        let _ = std::fs::remove_dir_all(&root);
    }
}
