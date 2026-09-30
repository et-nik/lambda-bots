//! `lb test run`: a map's tests (`maps/<map>/tests.yaml`) one after another with one bot while the others stand
//! still. Before each attempt the bot is given what the test gives (and health), goes to the test's start and stands
//! there a moment; then it tries to get to the goal (`lb_nav::reach`). Every attempt is reported on the console and
//! in the log as it ends; the run is written to `logs/tests/<map>-<time>.json` and compared with the last run of the
//! map's tests.

use std::path::{Path, PathBuf};

use lb_config::map_tests::{Expect, FILE, MapTest, MapTests};
use lb_core::Vec3;
use lb_nav::reach::{Allowed, CHECKS_PER_FRAME, Outcome, Reach, Report};
use serde::Serialize;

use crate::Runtime;
use crate::manager::BotState;
use crate::orders::{Order, OrderKind, give_list, outcome_lines, say};

/// Seconds the items given take to be in hand, and the bot stands at the start before an attempt.
const GIVE_WAIT: f64 = 0.6;
const SETTLE: f64 = 0.5;
/// How near the start the bot gets before an attempt, and how long it has to get there.
const START_RADIUS: f32 = 48.0;
const START_TIMEOUT: f64 = 60.0;
/// Deaths on the way to the start before the attempt is given up.
const START_TRIES: u32 = 2;

pub fn tests_path(install: &Path, map: &str) -> PathBuf {
    install.join("maps").join(map).join(FILE)
}

/// The map's tests: none when it has no file.
pub fn read_tests(install: &Path, map: &str) -> Result<MapTests, String> {
    let path = tests_path(install, map);
    match std::fs::read_to_string(&path) {
        Ok(text) => MapTests::parse(&text, &path.display().to_string()).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(MapTests::new(map)),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub fn write_tests(install: &Path, map: &str, tests: &MapTests) -> Result<PathBuf, String> {
    let path = tests_path(install, map);
    write_file(&path, &tests.to_yaml())?;
    Ok(path)
}

fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn point(p: [f32; 3]) -> Vec3 {
    Vec3::new(p[0], p[1], p[2])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Give,
    ToStart,
    Settle,
    Attempt,
}

/// One attempt of a test, as it went.
#[derive(Clone, Debug, Serialize)]
pub struct AttemptResult {
    pub passed: bool,
    #[serde(flatten)]
    pub outcome: Outcome,
    pub report: Report,
}

#[derive(Clone, Debug, Serialize)]
pub struct TestResult {
    pub id: String,
    pub expect: &'static str,
    pub attempts: Vec<AttemptResult>,
}

impl TestResult {
    pub fn passed(&self) -> usize {
        self.attempts.iter().filter(|a| a.passed).count()
    }
}

pub struct TestRun {
    pub map: String,
    /// The bot running the tests.
    pub userid: i32,
    pub name: String,
    tests: Vec<MapTest>,
    index: usize,
    attempt: u32,
    phase: Phase,
    /// The phase's order is out.
    issued: bool,
    /// Tries to get to the start this attempt.
    to_start: u32,
    /// Attempts of every test, overriding the tests' own.
    repeat: Option<u32>,
    pub results: Vec<TestResult>,
    /// When the run started (UTC), for the report's name.
    pub stamp: String,
    /// The player who started the run, told how it goes too.
    pub reply_to: Option<u8>,
}

impl TestRun {
    pub fn new(map: &str, userid: i32, name: &str, tests: Vec<MapTest>, repeat: Option<u32>) -> TestRun {
        TestRun {
            map: map.to_string(),
            userid,
            name: name.to_string(),
            results: tests
                .iter()
                .map(|t| TestResult {
                    id: t.id.clone(),
                    expect: t.expect.as_str(),
                    attempts: Vec::new(),
                })
                .collect(),
            tests,
            index: 0,
            attempt: 0,
            phase: Phase::Give,
            issued: false,
            to_start: 0,
            repeat,
            stamp: crate::roster::stamp(),
            reply_to: None,
        }
    }

    fn test(&self) -> &MapTest {
        &self.tests[self.index]
    }

    fn attempts(&self) -> u32 {
        self.repeat.unwrap_or(self.test().repeat).max(1)
    }

    fn set(&mut self, phase: Phase) {
        self.phase = phase;
        self.issued = false;
    }

    /// The attempt is over: the next one, or the next test's first. False when the run is done.
    fn next(&mut self) -> bool {
        self.attempt += 1;
        self.to_start = 0;
        if self.attempt >= self.attempts() {
            self.attempt = 0;
            self.index += 1;
        }
        self.set(Phase::Give);
        self.index < self.tests.len()
    }

    /// What the run is doing, for `lb test`.
    pub fn status(&self) -> String {
        match self.tests.get(self.index) {
            Some(t) => format!(
                "{} runs {} ({} of {}), attempt {} of {}: {:?}",
                self.name,
                t.id,
                self.index + 1,
                self.tests.len(),
                self.attempt + 1,
                self.attempts(),
                self.phase
            ),
            None => format!("{}: done", self.name),
        }
    }

    fn record(&mut self, reach: &Reach, outcome: Outcome) -> Vec<String> {
        let expect = self.test().expect;
        let passed = match expect {
            Expect::Arrive => outcome.arrived(),
            Expect::NoWay => matches!(outcome, Outcome::NoWay { .. }),
        };
        let mut lines = outcome_lines(reach);
        if let Some(first) = lines.first_mut() {
            *first = format!(
                "lb test {} [{}/{}]: {}{first}",
                self.test().id,
                self.attempt + 1,
                self.attempts(),
                if passed { "" } else { "FAILED: " }
            );
        }
        self.results[self.index].attempts.push(AttemptResult {
            passed,
            outcome,
            report: reach.report().clone(),
        });
        lines
    }

    /// An attempt that could not be made: the bot never got to the start.
    fn no_start(&mut self, why: String) -> Vec<String> {
        let line = format!(
            "lb test {} [{}/{}]: FAILED: the start was not reached: {why}",
            self.test().id,
            self.attempt + 1,
            self.attempts()
        );
        self.results[self.index].attempts.push(AttemptResult {
            passed: false,
            outcome: Outcome::Stuck {
                why: format!("the start was not reached: {why}"),
            },
            report: Report::default(),
        });
        vec![line]
    }
}

/// A test's attempt on its way: getting to `spot` for the run.
fn go(spot: Vec3, radius: f32, allowed: Allowed, timeout: f64, now: f64, what: &str) -> Order {
    let mut reach = Reach::new(spot, radius, allowed, timeout, now);
    reach.checks = CHECKS_PER_FRAME;
    Order::go(reach, what.to_string(), true)
}

impl Runtime {
    /// Moves the test run on: the bot's next order when its last one is done.
    pub(crate) fn tick_test_run(&mut self, host: &mut dyn lb_host::Host) {
        let now = self.now;
        let Some(run) = self.test_run.as_mut() else {
            return;
        };
        let Some(bot) = self.bots.iter_mut().find(|b| b.userid == run.userid) else {
            self.end_test_run(host, "the bot left");
            return;
        };
        if bot.state != BotState::Alive {
            return;
        }
        let done = bot.order.as_ref().is_none_or(|o| o.done(now));
        if run.issued && !done {
            return;
        }
        let finished = |bot: &mut crate::manager::Bot| match bot.order.take().map(|o| o.kind) {
            Some(OrderKind::Go(reach)) => reach.outcome().cloned().map(|o| (reach, o)),
            _ => None,
        };
        let mut lines = Vec::new();
        let mut over = false;
        match run.phase {
            Phase::Give if !run.issued => {
                let mut names = run.test().give.clone();
                names.push("health".into());
                match give_list(&names) {
                    Ok(items) => {
                        for item in items {
                            bot.pending_client_cmds.push(vec!["give".into(), item]);
                        }
                    }
                    Err(e) => lines.push(format!("lb test {}: {e}", run.test().id)),
                }
                bot.order = Some(Order::hold(now + GIVE_WAIT));
                run.issued = true;
            }
            Phase::Give => {
                bot.order = None;
                run.set(if run.test().start.is_some() {
                    Phase::ToStart
                } else {
                    Phase::Settle
                });
            }
            Phase::ToStart if !run.issued => {
                let start = point(run.test().start.unwrap_or(run.test().goal));
                bot.nav.clear();
                bot.order = Some(go(
                    start,
                    START_RADIUS,
                    Allowed::default(),
                    START_TIMEOUT,
                    now.secs(),
                    "to the start",
                ));
                run.issued = true;
            }
            Phase::ToStart => match finished(bot) {
                Some((_, o)) if o.arrived() => run.set(Phase::Settle),
                Some((_, Outcome::Died)) if run.to_start + 1 < START_TRIES => {
                    run.to_start += 1;
                    run.set(Phase::Give);
                }
                other => {
                    let why = other.map_or("called off".into(), |(_, o)| o.describe());
                    lines.extend(run.no_start(why));
                    over = !run.next();
                }
            },
            Phase::Settle if !run.issued => {
                bot.order = Some(Order::hold(now + SETTLE));
                run.issued = true;
            }
            Phase::Settle => {
                bot.order = None;
                run.set(Phase::Attempt);
            }
            Phase::Attempt if !run.issued => {
                let t = run.test();
                let allowed = Allowed::parse(&t.tricks).unwrap_or_default();
                let order = go(
                    point(t.goal),
                    t.radius,
                    allowed,
                    f64::from(t.timeout),
                    now.secs(),
                    &t.id,
                );
                bot.nav.clear();
                bot.order = Some(order);
                run.issued = true;
            }
            Phase::Attempt => {
                if let Some((reach, outcome)) = finished(bot) {
                    lines.extend(run.record(&reach, outcome));
                }
                over = !run.next();
            }
        }
        say(host, run.reply_to, &lines);
        if over {
            self.end_test_run(host, "");
        }
    }

    /// Ends the test run (`why` when it stops before its end): the bot goes back to its behavior, the summary goes to
    /// the console and the log, the report to `logs/tests/`.
    pub(crate) fn end_test_run(&mut self, host: &mut dyn lb_host::Host, why: &str) {
        let Some(run) = self.test_run.take() else {
            return;
        };
        if let Some(bot) = self.bots.iter_mut().find(|b| b.userid == run.userid) {
            bot.order = None;
            bot.nav.clear();
        }
        let dir = self.init.install_dir.join("logs").join("tests");
        let before = last_report(&dir, &run.map);
        let mut lines = vec![format!(
            "lb test: {} tests on {}{}",
            run.results.iter().filter(|r| !r.attempts.is_empty()).count(),
            run.map,
            if why.is_empty() {
                String::new()
            } else {
                format!(", stopped: {why}")
            }
        )];
        for r in run.results.iter().filter(|r| !r.attempts.is_empty()) {
            let was = before
                .as_ref()
                .and_then(|b| b.iter().find(|(id, ..)| *id == r.id))
                .map(|(_, passed, tried)| format!(" (was {passed}/{tried})"))
                .unwrap_or_default();
            lines.push(format!("  {}: {}/{} passed{was}", r.id, r.passed(), r.attempts.len()));
        }
        let report = serde_json::json!({
            "map": run.map,
            "bot": run.name,
            "core": crate::CORE_VERSION,
            "started": run.stamp,
            "ended": crate::roster::stamp(),
            "stopped": (!why.is_empty()).then_some(why),
            "tests": run.results,
        });
        let path = dir.join(format!("{}-{}.json", run.map, run.stamp));
        match serde_json::to_string_pretty(&report)
            .map_err(|e| e.to_string())
            .and_then(|text| write_file(&path, &text))
        {
            Ok(()) => lines.push(format!("  report: {}", path.display())),
            Err(e) => lines.push(format!("  report not written: {e}")),
        }
        say(host, run.reply_to, &lines);
        self.test_summary = lines;
    }
}

const USAGE: &str = "usage: lb test [list] | lb test add <id> <spot> [from <spot>] [give <item>...] \
                     [tricks <trick>...] [radius R] [timeout T] [repeat N] [expect arrive|no_way] [note <text>] | \
                     lb test remove <id> | lb test run [<id>...|all] [bot <name|#userid>] [repeat N] | lb test stop | \
                     lb test results | lb test motor ...";
/// Words that start the options of `lb test add`.
const ADD_KEYS: &[&str] = &[
    "from", "give", "tricks", "radius", "timeout", "repeat", "expect", "note",
];

/// One line about a test.
pub fn describe(t: &MapTest) -> String {
    let p = |v: [f32; 3]| format!("{:.0} {:.0} {:.0}", v[0], v[1], v[2]);
    let mut s = format!("{}: ", t.id);
    if let Some(start) = t.start {
        s.push_str(&format!("from {} ", p(start)));
    }
    s.push_str(&format!("to {}", p(t.goal)));
    if !t.give.is_empty() {
        s.push_str(&format!(", give {}", t.give.join(" ")));
    }
    s.push_str(&format!(", tricks {}", t.tricks.join(" ")));
    if t.repeat > 1 {
        s.push_str(&format!(", {} times", t.repeat));
    }
    if t.expect == Expect::NoWay {
        s.push_str(", expect no way");
    }
    if let Some(n) = &t.note {
        s.push_str(&format!(" ({n})"));
    }
    s
}

/// `lb test ...` (but `lb test motor`).
pub(crate) fn command(rt: &mut Runtime, host: &mut dyn lb_host::Host, args: &[&str]) -> Vec<String> {
    let Some(map) = rt.map.as_ref().map(|m| m.name.clone()) else {
        return vec!["no map".into()];
    };
    let install = rt.init.install_dir.clone();
    match args.first().copied() {
        None | Some("list") => {
            let file = match read_tests(&install, &map) {
                Ok(f) => f,
                Err(e) => return vec![e],
            };
            let path = tests_path(&install, &map);
            let mut out = if file.tests.is_empty() {
                vec![format!(
                    "{map} has no tests ({}); lb test add <id> <spot> adds one",
                    path.display()
                )]
            } else {
                let mut out = vec![format!("{} tests of {map} ({}):", file.tests.len(), path.display())];
                out.extend(file.tests.iter().map(|t| format!("  {}", describe(t))));
                out
            };
            if let Some(run) = &rt.test_run {
                out.push(run.status());
            }
            out
        }
        Some("add") => add(rt, host, &install, &map, &args[1..]).unwrap_or_else(|e| vec![e]),
        Some("remove") => {
            let [id] = &args[1..] else {
                return vec!["usage: lb test remove <id>".into()];
            };
            let mut file = match read_tests(&install, &map) {
                Ok(f) => f,
                Err(e) => return vec![e],
            };
            let before = file.tests.len();
            file.tests.retain(|t| t.id != *id);
            if file.tests.len() == before {
                return vec![format!("no test `{id}`")];
            }
            match write_tests(&install, &map, &file) {
                Ok(path) => vec![format!("test {id} removed from {}", path.display())],
                Err(e) => vec![e],
            }
        }
        Some("run") => run(rt, host, &install, &map, &args[1..]).unwrap_or_else(|e| vec![e]),
        Some("stop") => {
            if rt.test_run.is_none() {
                return vec!["no test run".into()];
            }
            rt.end_test_run(host, "lb test stop");
            Vec::new()
        }
        Some("results") => {
            if rt.test_summary.is_empty() {
                vec!["no test run yet".into()]
            } else {
                rt.test_summary.clone()
            }
        }
        _ => vec![USAGE.into()],
    }
}

/// `lb test add`: a test into the map's file (replacing one with the same id). Typed in the game without `from`,
/// the test starts where the player stands.
fn add(
    rt: &mut Runtime,
    host: &mut dyn lb_host::Host,
    install: &Path,
    map: &str,
    args: &[&str],
) -> Result<Vec<String>, String> {
    use crate::orders::{Target, spot_of};
    let [id, rest @ ..] = args else {
        return Err(USAGE.into());
    };
    if ADD_KEYS.contains(id) || id.contains(char::is_whitespace) {
        return Err(format!("`{id}` is not an id: {USAGE}"));
    }
    let (goal, used) = Target::parse(rest)?;
    let mut test = MapTest::new(id, spot_of(rt, host, &goal)?.to_array());
    let mut start = None;
    let mut i = used;
    while i < rest.len() {
        let key = rest[i];
        let words: Vec<&str> = rest[i + 1..]
            .iter()
            .copied()
            .take_while(|w| !ADD_KEYS.contains(w))
            .collect();
        let number = |what: &str| -> Result<f32, String> {
            words
                .first()
                .and_then(|w| w.parse::<f32>().ok())
                .filter(|v| *v > 0.0)
                .ok_or_else(|| format!("{what} <number above 0>"))
        };
        match key {
            "from" => {
                let (t, n) = Target::parse(&rest[i + 1..])?;
                start = Some(spot_of(rt, host, &t)?);
                i += 1 + n;
                continue;
            }
            "give" => {
                give_list(&words)?;
                test.give = words.iter().map(|w| w.to_string()).collect();
            }
            "tricks" => {
                Allowed::parse(&words)?;
                test.tricks = words.iter().map(|w| w.to_string()).collect();
            }
            "radius" => test.radius = number("radius")?,
            "timeout" => test.timeout = number("timeout")?,
            "repeat" => test.repeat = number("repeat")? as u32,
            "expect" => {
                test.expect = match words.first().copied() {
                    Some("arrive") => Expect::Arrive,
                    Some("no_way" | "noway") => Expect::NoWay,
                    _ => return Err("expect arrive|no_way".into()),
                }
            }
            "note" => {
                test.note = Some(rest[i + 1..].join(" "));
                break;
            }
            other => return Err(format!("unknown option `{other}`: {USAGE}")),
        }
        i += 1 + words.len();
    }
    if start.is_none() && rt.command_slot.is_some() {
        start = spot_of(rt, host, &Target::Me).ok();
    }
    test.start = start.map(|s| s.to_array());
    let mut file = read_tests(install, map)?;
    let replaced = match file.tests.iter_mut().find(|t| t.id == test.id) {
        Some(t) => {
            *t = test.clone();
            true
        }
        None => {
            file.tests.push(test.clone());
            false
        }
    };
    let path = write_tests(install, map, &file)?;
    Ok(vec![format!(
        "test {} in {}: {}",
        if replaced { "replaced" } else { "added" },
        path.display(),
        describe(&test)
    )])
}

/// `lb test run`: the tests named (all of them by default) with one bot, the named one or the first alive.
fn run(
    rt: &mut Runtime,
    host: &mut dyn lb_host::Host,
    install: &Path,
    map: &str,
    args: &[&str],
) -> Result<Vec<String>, String> {
    if rt.test_run.is_some() {
        return Err("a test run is under way: lb test stop".into());
    }
    if rt.graph.is_none() || rt.search_world.is_none() {
        return Err(format!("no navigation yet: {}", rt.nav_status));
    }
    let file = read_tests(install, map)?;
    let (mut ids, mut who, mut repeat) = (Vec::new(), None, None);
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "bot" => {
                who = Some(args.get(i + 1).ok_or("bot <name|#userid>")?.to_string());
                i += 2;
            }
            "repeat" => {
                let n = args.get(i + 1).and_then(|n| n.parse::<u32>().ok()).filter(|n| *n > 0);
                repeat = Some(n.ok_or("repeat <attempts>")?);
                i += 2;
            }
            "all" => i += 1,
            id => {
                ids.push(id);
                i += 1;
            }
        }
    }
    let tests: Vec<MapTest> = if ids.is_empty() {
        file.tests.clone()
    } else {
        ids.iter()
            .map(|id| {
                file.get(id)
                    .cloned()
                    .ok_or(format!("no test `{id}`; lb test lists them"))
            })
            .collect::<Result<_, _>>()?
    };
    if tests.is_empty() {
        return Err(format!("{map} has no tests; lb test add <id> <spot> adds one"));
    }
    let bot = match &who {
        Some(w) => crate::orders::live_bots(rt, w).first().copied(),
        None => rt.bots.iter().position(|b| b.state == BotState::Alive),
    }
    .ok_or("no live bot to run the tests: lb add")?;
    for b in &mut rt.bots {
        if b.order.as_ref().is_some_and(|o| !o.test) {
            b.order = None;
        }
    }
    let b = &rt.bots[bot];
    let attempts: u32 = tests.iter().map(|t| repeat.unwrap_or(t.repeat)).sum();
    let mut out = vec![format!(
        "{} runs {} tests of {map} ({attempts} attempts) while the others stand still; every attempt is reported as it \
         ends; lb test stop ends the run",
        b.persona.name,
        tests.len()
    )];
    let gives = tests.iter().any(|t| !t.give.is_empty());
    let mut run = TestRun::new(map, b.userid, &b.persona.name, tests, repeat);
    run.reply_to = rt.command_slot;
    rt.test_run = Some(run);
    if gives && !crate::orders::cheats(rt, host) {
        out.push(crate::orders::CHEATS_OFF.into());
    }
    Ok(out)
}

/// Every test of the map's last run on record: its id, attempts passed and made.
fn last_report(dir: &Path, map: &str) -> Option<Vec<(String, usize, usize)>> {
    let prefix = format!("{map}-");
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "json")
                && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    n.strip_prefix(&prefix)
                        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
                })
        })
        .collect();
    files.sort();
    let text = std::fs::read_to_string(files.last()?).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let tests = v.get("tests")?.as_array()?;
    Some(
        tests
            .iter()
            .filter_map(|t| {
                let id = t.get("id")?.as_str()?.to_string();
                let attempts = t.get("attempts")?.as_array()?;
                let passed = attempts
                    .iter()
                    .filter(|a| a.get("passed").and_then(|p| p.as_bool()) == Some(true))
                    .count();
                Some((id, passed, attempts.len()))
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_report_of_the_map_is_the_one_compared_with() {
        let dir = std::env::temp_dir().join(format!("lb-testrun-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let report = |passed: &[bool]| {
            let attempts: Vec<_> = passed.iter().map(|p| serde_json::json!({ "passed": p })).collect();
            serde_json::json!({ "tests": [{ "id": "ledge", "attempts": attempts }] }).to_string()
        };
        std::fs::write(dir.join("dm_snow-20260930-100000.json"), report(&[false, false])).unwrap();
        std::fs::write(dir.join("dm_snow-20260930-110000.json"), report(&[true, false, true])).unwrap();
        std::fs::write(dir.join("dm_snow2-20260930-120000.json"), report(&[false])).unwrap();
        let last = last_report(&dir, "dm_snow").unwrap();
        assert_eq!(last, vec![("ledge".to_string(), 2, 3)]);
        assert!(last_report(&dir, "crossfire").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
