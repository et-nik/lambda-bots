//! The in-game editor (`lb edit ...`): a player walks the map with the graph drawn around them, marks a node, and
//! records overlay changes — links to add or take out, areas bots never go, named places — into
//! `maps/<map>/editor.yaml`. Saving applies them to the graph at once. Only for players with `lb` access, and only
//! with `lb_editor 1`.

use std::path::{Path, PathBuf};

use lb_config::overlay::{LINK_KINDS, OverlayFile, Patch, Place};
use lb_core::Vec3;
use lb_host::DebugPrim;
use lb_nav::{LinkKind, NavGraph, NodeFlags, NodeId};

/// Nodes drawn around the editor, units.
const DRAW_NODES: f32 = 512.0;
/// Links drawn from nodes this close.
const DRAW_LINKS: f32 = 256.0;
/// Seconds between redraws (each drawing lives a little longer).
const REDRAW: f64 = 0.3;
/// Beams one redraw sends (the adapter sends at most 30 a call).
const BEAMS: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show {
    Off,
    Nodes,
    Links,
}

pub struct Editor {
    /// The player editing.
    pub slot: u8,
    pub show: Show,
    pub mark: Option<NodeId>,
    /// `maps/<map>/editor.yaml` as it will be saved.
    pub file: OverlayFile,
    /// The file as it was read when editing started: the web editor saves the same file, and a save over its
    /// changes would lose them.
    loaded: Option<String>,
    /// Changes since the last save, newest last, for undo.
    changes: Vec<Change>,
    next_draw: f64,
}

/// One edit, to undo it.
enum Change {
    Patch,
    Place,
}

pub fn editor_path(install: &Path, map: &str) -> PathBuf {
    install.join("maps").join(map).join("editor.yaml")
}

fn at(p: Vec3) -> [f32; 3] {
    [p.x.round(), p.y.round(), p.z.round()]
}

/// A radius argument: `default` when left out, `None` unless a finite number above 0.
fn radius_arg(arg: Option<&&str>, default: f32) -> Option<f32> {
    match arg {
        None => Some(default),
        Some(r) => r.parse::<f32>().ok().filter(|r| r.is_finite() && *r > 0.0),
    }
}

fn kind_color(kind: LinkKind, valid: bool) -> [u8; 3] {
    if !valid {
        return [255, 0, 0];
    }
    match kind {
        LinkKind::Walk => [200, 200, 200],
        LinkKind::Crouch => [150, 100, 255],
        LinkKind::Jump => [0, 255, 0],
        LinkKind::Drop => [255, 255, 0],
        LinkKind::Ladder => [0, 255, 255],
        LinkKind::Swim => [0, 100, 255],
        LinkKind::Door => [255, 150, 0],
        LinkKind::Lift => [200, 0, 255],
        LinkKind::Teleport => [255, 255, 255],
        LinkKind::Breakable => [150, 75, 0],
        LinkKind::Push => [255, 0, 150],
        LinkKind::LongJump => [128, 255, 0],
        LinkKind::GaussBoost => [255, 128, 128],
    }
}

fn node_color(flags: NodeFlags) -> [u8; 3] {
    if flags.contains(NodeFlags::GOAL) {
        [255, 220, 0]
    } else if flags.contains(NodeFlags::LADDER) {
        [0, 255, 255]
    } else if flags.contains(NodeFlags::WATER) {
        [0, 100, 255]
    } else if flags.contains(NodeFlags::CROUCH) {
        [150, 100, 255]
    } else if flags.contains(NodeFlags::MECHANISM) {
        [255, 150, 0]
    } else {
        [120, 255, 120]
    }
}

fn beam(a: Vec3, b: Vec3, color: [u8; 3], width: u8) -> DebugPrim {
    DebugPrim {
        line: Some((a, b)),
        text: None,
        color,
        width,
        life_ds: 4,
        channel: 0,
    }
}

impl Editor {
    /// Starts editing `map` for the player in `slot`, from the editor file as saved.
    pub fn open(slot: u8, install: &Path, map: &str) -> Result<Editor, String> {
        let path = editor_path(install, map);
        let loaded = std::fs::read_to_string(&path).ok();
        let file = match &loaded {
            Some(text) => OverlayFile::parse(text, &path.display().to_string()).map_err(|e| e.to_string())?,
            None => OverlayFile::new(map),
        };
        Ok(Editor {
            slot,
            show: Show::Links,
            mark: None,
            file,
            loaded,
            changes: Vec::new(),
            next_draw: 0.0,
        })
    }

    pub fn unsaved(&self) -> usize {
        self.changes.len()
    }

    fn patch(&mut self, p: Patch) {
        self.file.nav.patches.push(p);
        self.changes.push(Change::Patch);
    }

    /// Runs `lb edit <args>` for the editor standing at `origin`; returns what to print.
    pub fn command(&mut self, args: &[&str], graph: Option<&NavGraph>, origin: Vec3) -> Vec<String> {
        let nearest = graph.and_then(|g| g.nearest(origin, 256.0, 1).first().map(|(n, _)| *n));
        match args {
            ["show", what] => {
                self.show = match *what {
                    "nodes" => Show::Nodes,
                    "links" => Show::Links,
                    "off" => Show::Off,
                    _ => return vec!["lb edit show nodes|links|off".into()],
                };
                vec![format!("showing {what}")]
            }
            ["mark"] => match nearest {
                Some(n) => {
                    self.mark = Some(n);
                    vec![format!("marked node {n}")]
                }
                None => vec!["no node near".into()],
            },
            ["link", opts @ ..] | ["unlink", opts @ ..] => {
                let (Some(g), Some(a), Some(b)) = (graph, self.mark, nearest) else {
                    return vec!["mark a node first (lb edit mark), then stand at the other end".into()];
                };
                if a == b {
                    return vec!["the other end is the marked node itself".into()];
                }
                let (from, to) = (at(g.node(a).origin), at(g.node(b).origin));
                let both = opts.contains(&"both");
                if args[0] == "link" {
                    let kinds: Vec<&str> = opts.iter().copied().filter(|o| *o != "both" && *o != "trust").collect();
                    if kinds.len() > 1 || kinds.iter().any(|k| !LINK_KINDS.contains(k)) {
                        return vec![format!(
                            "lb edit link [kind] [both] [trust]: kind is one of {}",
                            LINK_KINDS.join(", ")
                        )];
                    }
                    let kind = kinds.first().map(|k| k.to_string());
                    self.patch(Patch::AddLink {
                        from,
                        to,
                        kind,
                        both,
                        trust: opts.contains(&"trust"),
                        note: String::new(),
                    });
                    vec![format!(
                        "link {a} -> {b}{} added; `lb edit save` checks and applies it",
                        if both { " and back" } else { "" }
                    )]
                } else {
                    self.patch(Patch::RemoveLink {
                        from,
                        to,
                        both,
                        note: String::new(),
                    });
                    vec![format!(
                        "link {a} -> {b}{} taken out on save",
                        if both { " and back" } else { "" }
                    )]
                }
            }
            ["forbid", rest @ ..] => {
                let Some(radius) = radius_arg(rest.first(), 48.0) else {
                    return vec!["lb edit forbid [radius]: the radius is a number of units above 0".into()];
                };
                self.patch(Patch::Forbid {
                    at: at(origin),
                    radius,
                    note: String::new(),
                });
                vec![format!("bots will not plan through here ({radius:.0} u) after saving")]
            }
            ["place", name, rest @ ..] => {
                let Some(radius) = radius_arg(rest.first(), 128.0) else {
                    return vec![
                        "lb edit place <name> [radius] [tags...]: the radius is a number of units above 0".into(),
                    ];
                };
                self.file.places.retain(|p| p.name != *name);
                self.file.places.push(Place {
                    name: name.to_string(),
                    at: at(origin),
                    radius,
                    tags: rest.iter().skip(1).map(|t| t.to_string()).collect(),
                });
                self.changes.push(Change::Place);
                vec![format!("place {name} here ({radius:.0} u)")]
            }
            ["undo"] => match self.changes.pop() {
                Some(Change::Patch) => {
                    self.file.nav.patches.pop();
                    vec!["last patch undone".into()]
                }
                Some(Change::Place) => {
                    self.file.places.pop();
                    vec!["last place undone".into()]
                }
                None => vec!["nothing to undo since the last save".into()],
            },
            ["info"] => {
                let (Some(g), Some(n)) = (graph, nearest) else {
                    return vec!["no node near".into()];
                };
                let node = g.node(n);
                let mut out = vec![format!(
                    "node {n} at {:.0} {:.0} {:.0}, {:?}, radius {:.0}",
                    node.origin.x, node.origin.y, node.origin.z, node.flags, node.radius
                )];
                for l in g.links(n) {
                    let spec = g.spec(l).map(|s| format!(" {}", s.action.name())).unwrap_or_default();
                    out.push(format!(
                        "  -> {} {}{spec} {:.1} s{}",
                        l.to,
                        l.kind.as_str(),
                        l.cost,
                        if l.valid() { "" } else { " (off)" }
                    ));
                }
                out
            }
            _ => vec![
                "lb edit on|off | show nodes|links|off | mark | link [kind] [both] [trust] | unlink [both] | \
                 forbid [radius] | place <name> [radius] [tags...] | info | undo | save"
                    .into(),
            ],
        }
    }

    /// Writes `maps/<map>/editor.yaml`, unless someone else saved it since editing started.
    pub fn save(&mut self, install: &Path) -> Result<PathBuf, String> {
        let path = editor_path(install, &self.file.map);
        if std::fs::read_to_string(&path).ok() != self.loaded {
            return Err(format!(
                "{} changed since `lb edit on` (saved from the web editor?): not saved; `lb edit off`, `lb edit on` \
                 and make the {} changes again",
                path.display(),
                self.changes.len()
            ));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = self.file.to_yaml().map_err(|e| e.to_string())?;
        let tmp = path.with_extension("yaml.tmp");
        std::fs::write(&tmp, &text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
        // The map editor's graph was made of the file as it was.
        lb_navgen::mapload::remove_edited(install, &self.file.map);
        self.loaded = Some(text);
        self.changes.clear();
        Ok(path)
    }

    /// What to draw for the editor standing at `eye` now, if a redraw is due.
    pub fn draw(&mut self, now: f64, graph: &NavGraph, eye: Vec3) -> Option<Vec<DebugPrim>> {
        if self.show == Show::Off || now < self.next_draw {
            return None;
        }
        self.next_draw = now + REDRAW;
        let mut near = graph.nearest(eye, DRAW_NODES, 64);
        near.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut out = Vec::new();
        if let Some(m) = self.mark {
            let o = graph.node(m).origin;
            out.push(beam(o - Vec3::Z * 36.0, o + Vec3::Z * 72.0, [255, 0, 0], 20));
        }
        if self.show == Show::Links {
            for &(n, d) in near.iter().filter(|(_, d)| *d <= DRAW_LINKS) {
                let a = graph.node(n).origin;
                for l in graph.links(n) {
                    if out.len() >= BEAMS * 2 / 3 {
                        break;
                    }
                    let b = graph.node(l.to).origin;
                    out.push(beam(a, b + Vec3::Z * 4.0, kind_color(l.kind, l.valid()), 8));
                }
                let _ = d;
            }
        }
        for &(n, _) in &near {
            if out.len() >= BEAMS {
                break;
            }
            let node = graph.node(n);
            let o = node.origin;
            out.push(beam(o - Vec3::Z * 36.0, o + Vec3::Z * 16.0, node_color(node.flags), 12));
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_nav::graph::{GraphStats, LinkFlags, NO_SPEC, NavLink, NavNode};

    fn line() -> NavGraph {
        let nodes: Vec<NavNode> = (0..3)
            .map(|i| NavNode {
                origin: Vec3::new(i as f32 * 100.0, 0.0, 36.0),
                flags: NodeFlags::empty(),
                radius: 16.0,
                support: 0,
                first_link: 0,
                link_count: 0,
            })
            .collect();
        let walk = |to| NavLink {
            to,
            kind: LinkKind::Walk,
            length: 100.0,
            flags: LinkFlags::VALID,
            cost: 0.3,
            spec: NO_SPEC,
        };
        let out = vec![vec![walk(1)], vec![walk(0), walk(2)], vec![walk(1)]];
        NavGraph::from_parts(nodes, out, Vec::new(), "test", GraphStats::default())
    }

    #[test]
    fn edits_are_recorded_undone_and_saved() {
        let dir = std::env::temp_dir().join(format!("lb-editor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let g = line();
        let mut ed = Editor::open(1, &dir, "crossfire").unwrap();
        assert!(ed.command(&["link"], Some(&g), Vec3::ZERO)[0].contains("mark a node"));
        ed.command(&["mark"], Some(&g), Vec3::new(0.0, 0.0, 36.0));
        ed.command(&["link", "jump", "both"], Some(&g), Vec3::new(200.0, 0.0, 36.0));
        ed.command(&["forbid", "64"], Some(&g), Vec3::new(100.0, 0.0, 36.0));
        ed.command(
            &["place", "bridge", "96", "choke"],
            Some(&g),
            Vec3::new(100.0, 0.0, 36.0),
        );
        assert_eq!(ed.unsaved(), 3);
        ed.command(&["undo"], Some(&g), Vec3::ZERO);
        assert!(ed.file.places.is_empty());
        // The map editor's graph was made of the file before: it goes with the save.
        let edited = lb_navgen::mapload::overlay_path(&dir, "crossfire", lb_navgen::mapload::EDITED);
        std::fs::create_dir_all(edited.parent().unwrap()).unwrap();
        std::fs::write(&edited, b"stale").unwrap();
        let path = ed.save(&dir).unwrap();
        assert!(!edited.exists());
        let again = Editor::open(1, &dir, "crossfire").unwrap();
        assert_eq!(again.file.nav.patches.len(), 2);
        assert!(matches!(
            &again.file.nav.patches[0],
            Patch::AddLink { kind: Some(k), both: true, .. } if k == "jump"
        ));
        assert!(path.ends_with("maps/crossfire/editor.yaml"));
        let drawn = ed.draw(1.0, &g, Vec3::new(0.0, 0.0, 64.0)).unwrap();
        assert!(!drawn.is_empty() && drawn.len() <= BEAMS);
        assert!(ed.draw(1.1, &g, Vec3::ZERO).is_none(), "not due yet");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_saved_by_another_editor_is_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("lb-editor-conflict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let g = line();
        let here = Vec3::new(100.0, 0.0, 36.0);
        let mut ed = Editor::open(1, &dir, "crossfire").unwrap();
        ed.command(&["forbid"], Some(&g), here);
        let mut web = OverlayFile::new("crossfire");
        web.places.push(Place {
            name: "roof".into(),
            at: [0.0, 0.0, 0.0],
            radius: 64.0,
            tags: Vec::new(),
        });
        let path = editor_path(&dir, "crossfire");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, web.to_yaml().unwrap()).unwrap();
        assert!(ed.save(&dir).unwrap_err().contains("changed since"));
        let mut ed = Editor::open(1, &dir, "crossfire").unwrap();
        ed.command(&["forbid"], Some(&g), here);
        ed.save(&dir).unwrap();
        let saved = Editor::open(1, &dir, "crossfire").unwrap().file;
        assert_eq!((saved.places.len(), saved.nav.patches.len()), (1, 1));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bad_radii_and_kinds_are_not_recorded() {
        let dir = std::env::temp_dir().join(format!("lb-editor-bad-{}", std::process::id()));
        let g = line();
        let mut ed = Editor::open(1, &dir, "crossfire").unwrap();
        let here = Vec3::new(100.0, 0.0, 36.0);
        for r in ["0", "-5", "nan", "inf", "wide"] {
            assert!(ed.command(&["forbid", r], Some(&g), here)[0].contains("above 0"), "{r}");
            assert!(
                ed.command(&["place", "bridge", r], Some(&g), here)[0].contains("above 0"),
                "{r}"
            );
        }
        ed.command(&["mark"], Some(&g), Vec3::new(0.0, 0.0, 36.0));
        let far = Vec3::new(200.0, 0.0, 36.0);
        assert!(ed.command(&["link", "jmup"], Some(&g), far)[0].contains("kind is one of"));
        assert!(ed.command(&["link", "jump", "drop"], Some(&g), far)[0].contains("kind is one of"));
        assert_eq!(ed.unsaved(), 0);
        assert!(ed.file.nav.patches.is_empty() && ed.file.places.is_empty());
        ed.command(&["link", "both", "drop"], Some(&g), far);
        ed.command(&["forbid"], Some(&g), here);
        assert_eq!(ed.unsaved(), 2);
    }
}
