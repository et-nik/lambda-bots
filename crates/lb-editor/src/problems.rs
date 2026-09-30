//! What the page flags in the graph it shows. Errors: a link a change asked for that does not check out (why, from
//! what the check found), a link bots failed in test runs (`lb test`, `lb do`). Links to look at: a trick link that
//! lands in only some of the tries a little off, one put in without the check, a walk that falls off a ledge on the
//! way, a change that does nothing.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use lb_config::overlay::Patch;
use lb_core::Vec3;
use lb_nav::graph::{LinkFlags, LinkKind, NavGraph, NodeId};
use lb_nav::spec::Action;
use lb_navgen::patch::Outcome;
use rustc_hash::FxHashMap;
use serde::Serialize;
use serde_json::Value;

/// A trick link this robust or more is left alone (the jump planner's own "robust enough").
const ROBUST: f32 = 0.8;
/// A place a test run names is taken for the node within this distance of it.
const SAME_NODE: f32 = 24.0;
/// Commands of `lb do` looked at, the last ones.
const ORDERS_READ: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Attention,
}

/// A link, or a change, the page flags.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Problem {
    pub level: Level,
    /// `refused`, `failed`, `missed`, `idle`, `weak`, `trusted` or `fall`.
    pub kind: &'static str,
    /// The link's ends in the graph shown; `None` where no node stands.
    pub from: Option<NodeId>,
    pub to: Option<NodeId>,
    /// Where the ends stand (x, y, z each): a link that is not in the graph is drawn from them.
    pub at: [f32; 6],
    /// The link's kind, or the kind a change asked for.
    pub link: String,
    pub why: String,
    /// The change it comes of: `editor` or `overlay`, and its index there.
    pub patch: Option<(&'static str, usize)>,
}

fn ends(a: Vec3, b: Vec3) -> [f32; 6] {
    [a.x, a.y, a.z, b.x, b.y, b.z]
}

fn node_at(g: &NavGraph, n: NodeId) -> Vec3 {
    g.nodes.get(n as usize).map_or(Vec3::ZERO, |node| node.origin)
}

/// The changes of one file (`editor` or `overlay`) that did not do all they asked: links that do not check out (but
/// those another change puts in after all), and changes that did nothing.
pub fn for_changes(g: &NavGraph, patches: &[Patch], outcomes: &[Outcome], file: &'static str) -> Vec<Problem> {
    let mut out = Vec::new();
    for (i, (p, o)) in patches.iter().zip(outcomes).enumerate() {
        let asked = match p {
            Patch::AddLink { kind, .. } => kind.clone().unwrap_or_else(|| "link".into()),
            _ => String::new(),
        };
        for r in o.refused.iter().filter(|r| g.find_link(r.from, r.to).is_none()) {
            out.push(Problem {
                level: Level::Error,
                kind: "refused",
                from: Some(r.from),
                to: Some(r.to),
                at: ends(node_at(g, r.from), node_at(g, r.to)),
                link: asked.clone(),
                why: r.why.clone(),
                patch: Some((file, i)),
            });
        }
        if o.ok || !o.refused.is_empty() {
            continue;
        }
        let (from, to) = match o.nodes[..] {
            [a, b, ..] => (Some(a), Some(b)),
            [a] => (Some(a), None),
            [] => (None, None),
        };
        let (a, b) = match p {
            Patch::AddLink { from, to, .. } | Patch::RemoveLink { from, to, .. } | Patch::MoveNode { from, to, .. } => {
                (Vec3::from(*from), Vec3::from(*to))
            }
            Patch::AddNode { at, .. } | Patch::Forbid { at, .. } => (Vec3::from(*at), Vec3::from(*at)),
        };
        out.push(Problem {
            // A link taken out that is not there any more is only a change to drop.
            level: if matches!(p, Patch::RemoveLink { .. }) {
                Level::Attention
            } else {
                Level::Error
            },
            kind: "idle",
            from,
            to,
            at: ends(a, b),
            link: patch_name(p).into(),
            why: o.message.clone(),
            patch: Some((file, i)),
        });
    }
    out
}

fn patch_name(p: &Patch) -> &'static str {
    match p {
        Patch::Forbid { .. } => "forbid",
        Patch::AddLink { .. } => "add link",
        Patch::RemoveLink { .. } => "remove link",
        Patch::AddNode { .. } => "add node",
        Patch::MoveNode { .. } => "move node",
    }
}

/// Trick links that land in only some of the tries a little off, links put in without the check, and walks that
/// fall off a ledge on the way (`falls`: the links that do, and how far).
pub fn for_links(g: &NavGraph, falls: &[(NodeId, NodeId, f32)]) -> Vec<Problem> {
    let mut out = Vec::new();
    for n in 0..g.len() as NodeId {
        for l in g.links(n) {
            let at = ends(node_at(g, n), node_at(g, l.to));
            let link = l.kind.as_str().to_string();
            if l.flags.contains(LinkFlags::TRUSTED) {
                out.push(Problem {
                    level: Level::Attention,
                    kind: "trusted",
                    from: Some(n),
                    to: Some(l.to),
                    at,
                    link,
                    why: "put in without the check (trusted): bots try it as it is".into(),
                    patch: None,
                });
                continue;
            }
            let robustness = match g.spec(l).map(|s| s.action) {
                Some(Action::Jump { robustness, .. })
                | Some(Action::LongJump { robustness })
                | Some(Action::GaussBoost { robustness, .. }) => robustness,
                _ => 1.0,
            };
            if l.valid() && robustness < ROBUST {
                out.push(Problem {
                    level: Level::Attention,
                    kind: "weak",
                    from: Some(n),
                    to: Some(l.to),
                    at,
                    link,
                    why: format!(
                        "lands in {:.0}% of the tries a little off (the takeoff, the speed or the aim)",
                        robustness * 100.0
                    ),
                    patch: None,
                });
            }
        }
    }
    for &(a, b, h) in falls {
        let kind = g.find_link(a, b).map_or(LinkKind::Walk, |l| l.kind);
        out.push(Problem {
            level: Level::Attention,
            kind: "fall",
            from: Some(a),
            to: Some(b),
            at: ends(node_at(g, a), node_at(g, b)),
            link: kind.as_str().into(),
            why: format!(
                "walking it falls {h:.0} u off a ledge on the way; where it lands is not checked as a drop's landing is"
            ),
            patch: None,
        });
    }
    out
}

/// A link failure or a trick that missed, as a run reported it: where the ends stood.
#[derive(Clone, Debug)]
struct Seen {
    from_at: Vec3,
    to_at: Vec3,
    link: String,
    /// What happened, and in which run.
    what: String,
    missed: bool,
}

/// What the runs of the map's tests reported: the last test run and the last commands.
fn reported(install: &Path, map: &str) -> Vec<Seen> {
    let dir = install.join("logs").join("tests");
    let prefix = format!("{map}-");
    let mut runs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "json")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_prefix(&prefix))
                    .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        })
        .collect();
    runs.sort();
    let mut seen = Vec::new();
    if let Some(v) = runs.last().and_then(|p| read_json(p)) {
        let when = stamp(v.get("started").and_then(Value::as_str).unwrap_or(""));
        for t in v.get("tests").and_then(Value::as_array).into_iter().flatten() {
            let id = t.get("id").and_then(Value::as_str).unwrap_or("?");
            let attempts = t
                .get("attempts")
                .and_then(Value::as_array)
                .map_or(&[][..], |a| a.as_slice());
            for (i, a) in attempts.iter().enumerate() {
                let source = format!("lb test {id} [{}/{}], {when}", i + 1, attempts.len());
                let missed = a.get("outcome").and_then(Value::as_str) == Some("missed");
                attempt(a.get("report"), missed, &source, &mut seen);
            }
        }
    }
    let orders = std::fs::read_to_string(dir.join(format!("{map}-orders.jsonl"))).unwrap_or_default();
    let lines: Vec<&str> = orders.lines().collect();
    for line in &lines[lines.len().saturating_sub(ORDERS_READ)..] {
        let Ok(e) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let source = format!(
            "lb do {} go {}, {}",
            e.get("bot").and_then(Value::as_str).unwrap_or("?"),
            e.get("what").and_then(Value::as_str).unwrap_or("?"),
            stamp(e.get("at").and_then(Value::as_str).unwrap_or(""))
        );
        let missed = e.pointer("/outcome/outcome").and_then(Value::as_str) == Some("missed");
        attempt(e.get("report"), missed, &source, &mut seen);
    }
    seen
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

fn point(v: Option<&Value>) -> Option<Vec3> {
    let a = v?.as_array()?;
    Some(Vec3::new(
        a.first()?.as_f64()? as f32,
        a.get(1)?.as_f64()? as f32,
        a.get(2)?.as_f64()? as f32,
    ))
}

/// The links one attempt failed, and its trick when it missed.
fn attempt(report: Option<&Value>, missed: bool, source: &str, out: &mut Vec<Seen>) {
    let Some(r) = report else {
        return;
    };
    for f in r.get("failures").and_then(Value::as_array).into_iter().flatten() {
        let (Some(from_at), Some(to_at)) = (point(f.get("from_at")), point(f.get("to_at"))) else {
            continue;
        };
        out.push(Seen {
            from_at,
            to_at,
            link: f.get("kind").and_then(Value::as_str).unwrap_or("?").into(),
            what: format!(
                "failed: {} ({source})",
                f.get("reason").and_then(Value::as_str).unwrap_or("?")
            ),
            missed: false,
        });
    }
    if !missed {
        return;
    }
    let (Some(trick), Some(spot)) = (r.get("trick"), point(r.get("spot"))) else {
        return;
    };
    let Some(takeoff) = point(trick.get("takeoff")) else {
        return;
    };
    let off = point(r.get("landing")).map_or(0.0, |l| (l - spot).truncate().length());
    out.push(Seen {
        from_at: takeoff,
        to_at: spot,
        link: trick.get("trick").and_then(Value::as_str).unwrap_or("trick").into(),
        what: format!("missed: came down {off:.0} u from the spot ({source})"),
        missed: true,
    });
}

/// `YYYYMMDD-HHMMSS` (UTC) the way people read it.
fn stamp(s: &str) -> String {
    let d: Vec<char> = s.chars().collect();
    if d.len() != 15 {
        return s.to_string();
    }
    let at = |r: std::ops::Range<usize>| d[r].iter().collect::<String>();
    format!(
        "{}-{}-{} {}:{} UTC",
        at(0..4),
        at(4..6),
        at(6..8),
        at(9..11),
        at(11..13)
    )
}

/// Files read last per map, with when they changed and what they came to.
type Cached = (Vec<(PathBuf, Option<SystemTime>)>, Vec<Seen>);

fn cache() -> &'static Mutex<FxHashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<FxHashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// The files of the map's test runs and commands, with when they changed.
fn stamps(install: &Path, map: &str) -> Vec<(PathBuf, Option<SystemTime>)> {
    let dir = install.join("logs").join("tests");
    let mut files: Vec<(PathBuf, Option<SystemTime>)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&format!("{map}-")))
        })
        .map(|p| {
            let t = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
            (p, t)
        })
        .collect();
    files.sort();
    files
}

/// Links bots failed and tricks that missed in the map's last test run and its last commands, on the graph shown:
/// one problem a link, telling how often and the last time.
pub fn from_tests(install: &Path, map: &str, g: &NavGraph) -> Vec<Problem> {
    let now = stamps(install, map);
    let seen = {
        let mut c = cache().lock().expect("no panics while locked");
        match c.get(map) {
            Some((files, seen)) if *files == now => seen.clone(),
            _ => {
                let seen = reported(install, map);
                c.insert(map.to_string(), (now, seen.clone()));
                seen
            }
        }
    };
    let near = |p: Vec3| g.nearest(p, SAME_NODE, 1).first().map(|(n, _)| *n);
    let mut by_link: Vec<(Problem, usize)> = Vec::new();
    for s in &seen {
        let (from, to) = (near(s.from_at), near(s.to_at));
        match by_link
            .iter_mut()
            .find(|(p, _)| p.from == from && p.to == to && p.link == s.link && (p.kind == "missed") == s.missed)
        {
            Some((p, count)) => {
                *count += 1;
                p.why = format!("{} times, the last {}", *count, s.what);
            }
            None => by_link.push((
                Problem {
                    level: Level::Error,
                    kind: if s.missed { "missed" } else { "failed" },
                    from,
                    to,
                    at: ends(s.from_at, s.to_at),
                    link: s.link.clone(),
                    why: s.what.clone(),
                    patch: None,
                },
                1,
            )),
        }
    }
    by_link.into_iter().map(|(p, _)| p).collect()
}

/// Errors first, then by what they are and where.
pub fn sort(problems: &mut [Problem]) {
    let order = |k: &str| match k {
        "refused" => 0,
        "failed" | "missed" => 1,
        "idle" => 2,
        "fall" => 3,
        "weak" => 4,
        _ => 5,
    };
    problems.sort_by(|a, b| {
        a.level
            .cmp(&b.level)
            .then(order(a.kind).cmp(&order(b.kind)))
            .then(a.from.cmp(&b.from))
            .then(a.to.cmp(&b.to))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_nav::graph::{GraphStats, NO_SPEC, NavLink, NavNode, NodeFlags};
    use lb_nav::spec::{Anchor, Cost, Needs, Stance, TraversalSpec};
    use lb_navgen::patch::Refusal;

    /// Nodes 0, 1, 2 in a row: 0 → 1 a jump that lands half the time, 1 → 2 a trusted long jump, 2 → 0 a walk.
    fn graph() -> NavGraph {
        let node = |x: f32| NavNode {
            origin: Vec3::new(x, 0.0, 36.0),
            flags: NodeFlags::empty(),
            radius: 16.0,
            support: 0,
            first_link: 0,
            link_count: 0,
        };
        let spec = |action| TraversalSpec {
            entry: Anchor {
                origin: Vec3::ZERO,
                radius: 24.0,
                stance: Stance::Stand,
            },
            exit: Anchor {
                origin: Vec3::ZERO,
                radius: 32.0,
                stance: Stance::Stand,
            },
            action,
            needs: Needs::default(),
            deadline: 5.0,
            cost: Cost::default(),
        };
        let link = |to, kind, flags, spec| NavLink {
            to,
            kind,
            length: 100.0,
            flags,
            cost: 1.0,
            spec,
        };
        let out = vec![
            vec![link(1, LinkKind::Jump, LinkFlags::VALID, 0)],
            vec![link(2, LinkKind::LongJump, LinkFlags::VALID | LinkFlags::TRUSTED, 1)],
            vec![link(0, LinkKind::Walk, LinkFlags::VALID, NO_SPEC)],
        ];
        let specs = vec![
            spec(Action::Jump {
                speed: 200.0,
                duck: false,
                robustness: 0.5,
            }),
            spec(Action::LongJump { robustness: 0.5 }),
        ];
        NavGraph::from_parts(
            vec![node(0.0), node(100.0), node(200.0)],
            out,
            specs,
            "test",
            GraphStats::default(),
        )
    }

    #[test]
    fn weak_trusted_and_falling_links_are_to_look_at() {
        let g = graph();
        let found = for_links(&g, &[(2, 0, 64.0)]);
        let of = |kind: &str| {
            found
                .iter()
                .find(|p| p.kind == kind)
                .unwrap_or_else(|| panic!("{kind}: {found:?}"))
        };
        assert_eq!((of("weak").from, of("weak").to), (Some(0), Some(1)));
        assert!(of("weak").why.contains("50%"));
        assert_eq!(of("trusted").from, Some(1), "a trusted link is not also weak");
        assert_eq!(found.iter().filter(|p| p.from == Some(1)).count(), 1);
        assert!(of("fall").why.contains("64 u") && of("fall").level == Level::Attention);
    }

    #[test]
    fn a_change_that_did_not_all_it_asked_is_named() {
        let g = graph();
        let patches = vec![
            Patch::AddLink {
                from: [0.0, 0.0, 36.0],
                to: [200.0, 0.0, 36.0],
                kind: Some("longjump".into()),
                both: true,
                trust: false,
                note: String::new(),
            },
            Patch::RemoveLink {
                from: [0.0, 0.0, 36.0],
                to: [200.0, 0.0, 36.0],
                both: false,
                note: String::new(),
            },
        ];
        let outcomes = vec![
            Outcome {
                ok: true,
                message: "2 -> 0 longjump put in; 0 -> 2 does not check out".into(),
                nodes: vec![0, 2],
                links: vec![(2, 0)],
                refused: vec![Refusal {
                    from: 0,
                    to: 2,
                    why: "the long jump comes down 95 u past the landing".into(),
                }],
            },
            Outcome {
                ok: false,
                message: "there is no link 0 -> 2".into(),
                nodes: vec![0, 2],
                ..Outcome::default()
            },
        ];
        let found = for_changes(&g, &patches, &outcomes, "editor");
        assert_eq!(found.len(), 2, "{found:?}");
        let mut later = outcomes.clone();
        later[0].refused[0] = Refusal {
            from: 0,
            to: 1,
            why: "put in by another change after all".into(),
        };
        assert!(
            for_changes(&g, &patches, &later, "editor")
                .iter()
                .all(|p| p.kind != "refused"),
            "a link another change puts in is no problem"
        );
        let refused = &found[0];
        assert_eq!(
            (refused.kind, refused.level, refused.from, refused.to),
            ("refused", Level::Error, Some(0), Some(2))
        );
        assert_eq!(
            (refused.link.as_str(), refused.patch),
            ("longjump", Some(("editor", 0)))
        );
        assert_eq!(refused.at, [0.0, 0.0, 36.0, 200.0, 0.0, 36.0]);
        assert_eq!((found[1].kind, found[1].level), ("idle", Level::Attention));
    }

    #[test]
    fn links_failed_in_runs_are_found_by_where_their_ends_stand() {
        let g = graph();
        let install = std::env::temp_dir().join(format!("lb-problems-{}", std::process::id()));
        let dir = install.join("logs").join("tests");
        std::fs::create_dir_all(&dir).unwrap();
        let failure = serde_json::json!({
            "t": 1.5, "from": 7, "to": 8, "from_at": [2, 0, 36], "to_at": [98, 0, 36],
            "kind": "jump", "reason": "controller"
        });
        let run = serde_json::json!({
            "started": "20260930-205134",
            "tests": [{ "id": "ledge", "attempts": [
                { "passed": false, "outcome": "missed", "report": {
                    "spot": [200, 0, 36], "landing": [140, 0, 36], "failures": [failure],
                    "trick": { "trick": "gauss_boost", "takeoff": [0, 0, 36] } } },
                { "passed": true, "outcome": "arrived", "report": { "failures": [failure] } }
            ]}]
        });
        std::fs::write(dir.join("m-20260930-205134.json"), run.to_string()).unwrap();
        let order = serde_json::json!({
            "at": "20260930-210000", "bot": "rolo", "what": "0 0 36",
            "outcome": { "outcome": "stuck", "why": "…" }, "report": { "failures": [failure] }
        });
        std::fs::write(dir.join("m-orders.jsonl"), format!("{order}\n")).unwrap();
        let found = from_tests(&install, "m", &g);
        std::fs::remove_dir_all(&install).unwrap();
        let failed = found.iter().find(|p| p.kind == "failed").expect("the jump that failed");
        assert_eq!((failed.from, failed.to, failed.level), (Some(0), Some(1), Level::Error));
        assert!(
            failed
                .why
                .starts_with("3 times, the last failed: controller (lb do rolo go 0 0 36"),
            "{}",
            failed.why
        );
        let missed = found
            .iter()
            .find(|p| p.kind == "missed")
            .expect("the boost that missed");
        assert_eq!((missed.from, missed.to), (Some(0), Some(2)));
        assert!(
            missed.why.contains("60 u from the spot (lb test ledge [1/2]"),
            "{}",
            missed.why
        );
    }

    #[test]
    fn run_stamps_read_as_dates() {
        assert_eq!(stamp("20260930-205134"), "2026-09-30 20:51 UTC");
        assert_eq!(stamp("soon"), "soon");
    }
}
