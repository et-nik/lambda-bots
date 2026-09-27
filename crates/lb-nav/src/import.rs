//! Builds a navigation graph from a yapb graph and the map.
//!
//! yapb node heights are inconsistent, so every node is settled onto the floor, with the map's movers where they
//! rest (a node on a lift lands on its platform). Every link is then classified against the map:
//! - a walk that passes with doors and lifts at rest is a walk, or a drop when it falls on the way;
//! - one that a door blocks and that passes with the door open is a door link, with how the door is opened;
//! - one that a breakable blocks is a breakable link;
//! - one a simulated jump makes is a jump, with the run-up and ducking it needs;
//! - one from inside a teleport trigger to its destination is a teleport;
//! - ladder nodes make ladder links, underwater nodes swim links.
//!
//! Links that fail are kept but not planned through (unless imported links are trusted). Lifts are added from the
//! map: a platform with nodes on it gets links from them to the nodes at its raised position.

use lb_bsp::BspWorld;
use lb_bsp::mech::{Activation, Mechanisms, Mover, MoverKind, TriggerKind};
use lb_bsp::world::BrushKind;
use lb_core::Vec3;
use lb_kin::Physics;
use lb_kin::validate::{plan_jump, simulate_drop, simulate_walk};
use lb_worldq::{HullKind, TraceQuery, Tracer, contents};

use crate::graph::{GraphStats, LinkFlags, LinkKind, NO_SPEC, NavGraph, NavLink, NavNode, NodeFlags, NodeId};
use crate::plan::RUN_SPEED;
use crate::spec::{Action, Anchor, Cost, Interaction, MechRef, Needs, Stance, TraversalSpec};
use crate::validate::{WalkCheck, walk_check};
use crate::yapb::{LINK_JUMP, YapbGraph, node_flags};

/// The use key reaches objects within this distance of the player's origin (`PLAYER_SEARCH_RADIUS`), less a margin.
pub const USE_REACH: f32 = 56.0;
/// Health a bot keeps in reserve when it plans a fall.
const DROP_RESERVE: f32 = 10.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct ImportOptions {
    /// Plan through links that fail the offline check.
    pub trust_imported: bool,
    pub physics: Physics,
}

struct Ctx<'a> {
    world: &'a mut BspWorld,
    mech: &'a Mechanisms,
    phys: Physics,
    nodes: Vec<NavNode>,
    specs: Vec<TraversalSpec>,
}

fn is_water(c: i32) -> bool {
    c <= contents::WATER && c > contents::TRANSLUCENT
}

/// Distance from `p` to the box `mins..maxs` (0 inside).
fn box_distance(p: Vec3, mins: Vec3, maxs: Vec3) -> f32 {
    (p.clamp(mins, maxs) - p).length()
}

fn stance_of(n: &NavNode) -> Stance {
    if n.flags.contains(NodeFlags::LADDER) {
        Stance::Ladder
    } else if n.flags.contains(NodeFlags::WATER) {
        Stance::Swim
    } else if n.flags.contains(NodeFlags::CROUCH) {
        Stance::Crouch
    } else {
        Stance::Stand
    }
}

fn anchor(n: &NavNode, radius: f32) -> Anchor {
    Anchor {
        origin: n.origin,
        radius,
        stance: stance_of(n),
    }
}

/// Where a standing player at the node would be (crouch nodes store the crouched centre).
fn stand_origin(n: &NavNode) -> Vec3 {
    if n.flags.contains(NodeFlags::CROUCH) {
        n.origin + Vec3::Z * 18.0
    } else {
        n.origin
    }
}

/// Centre of the crouching hull at the node's floor.
fn crouch_origin(n: &NavNode) -> Vec3 {
    if n.flags.contains(NodeFlags::CROUCH) {
        n.origin
    } else {
        n.origin - Vec3::Z * 18.0
    }
}

fn mech_ref(m: &Mover) -> MechRef {
    MechRef {
        model: m.model as u16,
        rest: m.rest,
        active: m.active,
        travel: m.travel_time(),
        wait: m.wait,
    }
}

struct Classified {
    kind: LinkKind,
    valid: bool,
    spec: Option<TraversalSpec>,
    dynamic: bool,
}

impl Classified {
    fn plain(kind: LinkKind, valid: bool) -> Classified {
        Classified {
            kind,
            valid,
            spec: None,
            dynamic: false,
        }
    }
}

impl Ctx<'_> {
    /// Settles a yapb node: returns the resting origin, extra flags and the mover it stands on.
    fn settle(&mut self, origin: Vec3, mut flags: NodeFlags) -> (Vec3, NodeFlags, u16) {
        if flags.contains(NodeFlags::LADDER) {
            return (origin, flags, 0);
        }
        if is_water(self.world.point_contents(origin)) {
            return (origin, flags | NodeFlags::WATER, 0);
        }
        let try_hull = |w: &mut BspWorld, start: Vec3, hull: HullKind| {
            let tr = w.trace(&TraceQuery::hull(start + Vec3::Z * 2.0, start - Vec3::Z * 96.0, hull));
            (!tr.start_solid && !tr.all_solid && tr.fraction < 1.0 && tr.normal.z >= 0.7).then_some((tr.end, tr.hit))
        };
        let crouch = flags.contains(NodeFlags::CROUCH);
        let found = if crouch {
            try_hull(self.world, origin, HullKind::Crouch)
        } else {
            try_hull(self.world, origin, HullKind::Stand).or_else(|| {
                let r = try_hull(self.world, origin - Vec3::Z * 18.0, HullKind::Crouch);
                if r.is_some() {
                    flags |= NodeFlags::CROUCH;
                }
                r
            })
        };
        let Some((pos, hit)) = found else {
            return (origin, flags | NodeFlags::AIRBORNE, 0);
        };
        let support = hit
            .filter(|m| *m != 0)
            .and_then(|m| self.mech.mover(m as usize))
            .map_or(0, |m| m.model as u16);
        if support != 0 {
            flags |= NodeFlags::ON_MOVER;
        }
        (pos, flags, support)
    }

    fn walk(&mut self, a: &NavNode, b: &NavNode) -> Option<(LinkKind, WalkCheck)> {
        let crouch = a.flags.contains(NodeFlags::CROUCH) || b.flags.contains(NodeFlags::CROUCH);
        if !crouch {
            let r = walk_check(self.world, a.origin, b.origin, HullKind::Stand);
            if matches!(r, WalkCheck::Ok | WalkCheck::Drop(_)) {
                return Some((LinkKind::Walk, r));
            }
        }
        let r = walk_check(self.world, crouch_origin(a), crouch_origin(b), HullKind::Crouch);
        matches!(r, WalkCheck::Ok | WalkCheck::Drop(_)).then_some((LinkKind::Crouch, r))
    }

    /// Walking at the node gets there, sliding along what is in the way (the straight check does not slide).
    fn slides(&mut self, a: &NavNode, b: &NavNode) -> bool {
        if a.flags.contains(NodeFlags::CROUCH) || b.flags.contains(NodeFlags::CROUCH) {
            return false;
        }
        let v = simulate_walk(self.world, &self.phys, stand_origin(a), stand_origin(b), false);
        v.ok && v.flight < 0.1
    }

    fn spec(&mut self, spec: TraversalSpec) -> Option<TraversalSpec> {
        Some(spec)
    }

    /// A walk that falls on the way: simulate walking off and landing.
    fn drop_link(&mut self, a: &NavNode, b: &NavNode) -> Option<Classified> {
        let v = simulate_drop(self.world, &self.phys, stand_origin(a), stand_origin(b), 200.0);
        if !v.ok {
            return None;
        }
        let damage = if v.in_water {
            0.0
        } else {
            self.phys.fall_damage(v.impact)
        };
        let spec = TraversalSpec {
            entry: anchor(a, 24.0),
            exit: anchor(b, 32.0),
            action: Action::Drop { speed: 200.0, damage },
            needs: Needs {
                health: damage + DROP_RESERVE,
                longjump: false,
            },
            deadline: 3.0 + v.flight,
            cost: Cost {
                time: a.origin.distance(b.origin) / RUN_SPEED + v.flight,
                wait: 0.0,
                damage,
            },
        };
        Some(Classified {
            kind: LinkKind::Drop,
            valid: true,
            spec: self.spec(spec),
            dynamic: false,
        })
    }

    fn jump_link(&mut self, a: &NavNode, b: &NavNode) -> Option<Classified> {
        let plan = plan_jump(self.world, &self.phys, stand_origin(a), stand_origin(b))?;
        let damage = self.phys.fall_damage(plan.impact);
        let spec = TraversalSpec {
            entry: anchor(a, 24.0),
            exit: anchor(b, 32.0),
            action: Action::Jump {
                speed: plan.speed,
                duck: plan.duck,
                robustness: plan.robustness,
            },
            needs: Needs {
                health: damage + if damage > 0.0 { DROP_RESERVE } else { 0.0 },
                longjump: false,
            },
            deadline: 5.0 + plan.flight,
            cost: Cost {
                time: a.origin.distance(b.origin) / (RUN_SPEED * 0.8) + (1.0 - plan.robustness),
                wait: 0.0,
                damage,
            },
        };
        Some(Classified {
            kind: LinkKind::Jump,
            valid: true,
            spec: self.spec(spec),
            dynamic: false,
        })
    }

    /// Where a player gets on the ladder next to a ladder node, and the face normal there: a spot in front of the
    /// ladder brush where the standing hull fits and touches the ladder, on the side nearest to `toward`.
    fn ladder_mount(&mut self, near: Vec3, toward: Vec3) -> Option<(Vec3, Vec3)> {
        let b = self
            .world
            .brushes
            .iter()
            .filter(|b| b.kind == BrushKind::Volume(contents::LADDER))
            .map(|b| (box_distance(near, b.abs_mins(), b.abs_maxs()), b))
            .filter(|(d, _)| *d < 48.0)
            .min_by(|x, y| x.0.total_cmp(&y.0))?
            .1;
        let (mins, maxs) = (b.abs_mins(), b.abs_maxs());
        let mut best: Option<(f32, Vec3, Vec3)> = None;
        for (axis, side) in [(0usize, 1.0f32), (0, -1.0), (1, 1.0), (1, -1.0)] {
            let other = 1 - axis;
            let mut p = near;
            p[axis] = if side > 0.0 {
                maxs[axis] + 14.0
            } else {
                mins[axis] - 14.0
            };
            let (lo, hi) = (mins[other] + 8.0, maxs[other] - 8.0);
            p[other] = if lo <= hi {
                near[other].clamp(lo, hi)
            } else {
                (mins[other] + maxs[other]) * 0.5
            };
            p.z = near.z.clamp(mins.z + 20.0, maxs.z + 16.0);
            let fits = !self.world.trace(&TraceQuery::hull(p, p, HullKind::Stand)).start_solid;
            if !fits {
                continue;
            }
            let Some(normal) = lb_kin::MoveWorld::ladder(self.world, p, HullKind::Stand).and_then(|l| l.normal) else {
                continue;
            };
            let d = p.distance(toward);
            if best.is_none_or(|(bd, _, _)| d < bd) {
                best = Some((d, normal, p));
            }
        }
        best.map(|(_, n, p)| (n, p))
    }

    fn ladder_link(&mut self, a: &NavNode, b: &NavNode) -> Classified {
        let (at, toward) = if a.flags.contains(NodeFlags::LADDER) {
            (
                a.origin,
                if b.flags.contains(NodeFlags::LADDER) {
                    a.origin
                } else {
                    b.origin
                },
            )
        } else {
            (b.origin, a.origin)
        };
        let Some((normal, mount)) = self.ladder_mount(at, toward) else {
            return Classified::plain(LinkKind::Ladder, false);
        };
        let spec = TraversalSpec {
            entry: anchor(a, 24.0),
            exit: anchor(b, 24.0),
            action: Action::Ladder {
                normal,
                up: b.origin.z > a.origin.z,
                mount,
            },
            needs: Needs::default(),
            deadline: 4.0 + a.origin.distance(b.origin) / 100.0,
            cost: Cost {
                time: a.origin.distance(b.origin) / (RUN_SPEED * LinkKind::Ladder.speed_factor()),
                wait: 0.0,
                damage: 0.0,
            },
        };
        Classified {
            kind: LinkKind::Ladder,
            valid: true,
            spec: self.spec(spec),
            dynamic: false,
        }
    }

    /// The node nearest to `near` a player can operate `how` from.
    fn interaction(&mut self, how: &Activation, near: Vec3) -> Option<Interaction> {
        let model = match *how {
            Activation::Use { model } | Activation::Touch { model } | Activation::Shoot { model } => model,
            _ => return None,
        };
        let brush = self
            .world
            .brush(model)
            .or_else(|| self.world.brushes.iter().find(|b| b.model == model).or(None));
        let (mins, maxs) = match brush {
            Some(b) => (b.abs_mins(), b.abs_maxs()),
            None => {
                let m = self.world.bsp.models.get(model)?;
                (m.mins, m.maxs)
            }
        };
        let center = (mins + maxs) * 0.5;
        let candidates: Vec<(f32, Vec3)> = self
            .nodes
            .iter()
            .filter(|n| {
                !n.flags
                    .intersects(NodeFlags::LADDER | NodeFlags::AIRBORNE | NodeFlags::WATER)
            })
            .map(|n| (n.origin.distance(near), n.origin))
            .collect();
        let pick = |ok: &mut dyn FnMut(Vec3) -> bool| {
            candidates
                .iter()
                .filter(|(_, o)| ok(*o))
                .min_by(|x, y| x.0.total_cmp(&y.0))
                .map(|(_, o)| *o)
        };
        match *how {
            Activation::Use { .. } => {
                let spot = pick(&mut |o| box_distance(o, mins, maxs) < USE_REACH)?;
                Some(Interaction::Use {
                    model: model as u16,
                    spot,
                    aim: center,
                })
            }
            Activation::Touch { .. } => {
                let world = &*self.world;
                let spot = pick(&mut |o| world.hull_overlaps(model, Vec3::ZERO, o, HullKind::Stand))?;
                Some(Interaction::Touch {
                    model: model as u16,
                    spot,
                })
            }
            Activation::Shoot { .. } => {
                let world = &mut *self.world;
                let spot = pick(&mut |o| {
                    let eye = o + Vec3::Z * 28.0;
                    eye.distance(center) < 1500.0 && {
                        let tr = world.trace(&TraceQuery::line(eye, center));
                        tr.fraction > 0.97 || tr.hit == Some(model as u32)
                    }
                })?;
                Some(Interaction::Shoot {
                    model: model as u16,
                    spot,
                    aim: center,
                })
            }
            _ => None,
        }
    }

    /// How a door (or a lift) is set off: by itself (touch, use), or by what targets it.
    fn opener(&mut self, m: &Mover, near: Vec3) -> Option<Interaction> {
        let brush = self.world.brush(m.model)?;
        let (mins, maxs) = (brush.abs_mins(), brush.abs_maxs());
        if m.usable {
            return Some(Interaction::Use {
                model: m.model as u16,
                spot: near,
                aim: (mins + maxs) * 0.5,
            });
        }
        if m.touch {
            return Some(Interaction::Touch {
                model: m.model as u16,
                spot: near,
            });
        }
        let name = m.targetname.clone()?;
        let activators = self.mech.activators(self.world, &name);
        activators
            .iter()
            .filter(|a| a.delay < 5.0)
            .find_map(|a| self.interaction(&a.how, near))
    }

    /// A walk a door blocks: opens the door offline and walks again.
    fn door_link(&mut self, a: &NavNode, b: &NavNode, model: usize) -> Option<Classified> {
        let door = self.mech.mover(model)?.clone();
        if !matches!(door.kind, MoverKind::Door | MoverKind::RotatingDoor) {
            return None;
        }
        let saved = {
            let brush = self.world.brush_mut(model)?;
            let saved = (brush.offset, brush.solid);
            if door.kind == MoverKind::RotatingDoor {
                brush.solid = false;
            } else {
                brush.offset = door.active;
            }
            saved
        };
        let walk = self.walk(a, b);
        if let Some(brush) = self.world.brush_mut(model) {
            (brush.offset, brush.solid) = saved;
        }
        let (kind_through, _) = walk?;
        let open = self.opener(&door, a.origin);
        let Some(open) = open else {
            // Nothing opens it: a wall for now.
            return Some(Classified::plain(LinkKind::Door, false));
        };
        let door_ref = mech_ref(&door);
        let travel_time = a.origin.distance(b.origin) / (RUN_SPEED * kind_through.speed_factor());
        let detour = open.spot().distance(a.origin) / RUN_SPEED * 2.0;
        let spec = TraversalSpec {
            entry: anchor(a, 24.0),
            exit: anchor(b, 32.0),
            action: Action::Door { door: door_ref, open },
            needs: Needs::default(),
            deadline: 6.0 + detour + door_ref.travel * 2.0,
            cost: Cost {
                time: travel_time + detour,
                wait: door_ref.travel,
                damage: 0.0,
            },
        };
        Some(Classified {
            kind: LinkKind::Door,
            valid: true,
            spec: self.spec(spec),
            dynamic: true,
        })
    }

    fn breakable_link(&mut self, a: &NavNode, b: &NavNode, model: usize) -> Option<Classified> {
        let br = self.mech.breakable(model)?.clone();
        if !br.breakable() {
            return None;
        }
        let saved = {
            let brush = self.world.brush_mut(model)?;
            let s = brush.solid;
            brush.solid = false;
            s
        };
        let walk = self.walk(a, b);
        let center = self
            .world
            .brush(model)
            .map(|b| (b.abs_mins() + b.abs_maxs()) * 0.5)
            .unwrap_or_default();
        if let Some(brush) = self.world.brush_mut(model) {
            brush.solid = saved;
        }
        let (kind, _) = walk?;
        let spec = TraversalSpec {
            entry: anchor(a, 24.0),
            exit: anchor(b, 32.0),
            action: Action::Breakable {
                model: model as u16,
                aim: center,
                health: br.health,
                crowbar: br.crowbar_instant(),
            },
            needs: Needs::default(),
            deadline: 12.0,
            cost: Cost {
                time: a.origin.distance(b.origin) / (RUN_SPEED * kind.speed_factor()),
                wait: 1.0 + br.health / 50.0,
                damage: 0.0,
            },
        };
        Some(Classified {
            kind: LinkKind::Breakable,
            valid: true,
            spec: self.spec(spec),
            dynamic: true,
        })
    }

    fn teleport_link(&mut self, a: &NavNode, b: &NavNode) -> Option<Classified> {
        let mech = self.mech;
        let t = mech.triggers.iter().find(|t| {
            t.kind == TriggerKind::Teleport && self.world.hull_overlaps(t.model, Vec3::ZERO, a.origin, HullKind::Stand)
        })?;
        let (dest, _) = mech.teleport_destination(self.world, t)?;
        let arrive = dest + Vec3::Z * 37.0;
        if arrive.distance(b.origin) > 160.0 {
            return None;
        }
        let spec = TraversalSpec {
            entry: anchor(a, 32.0),
            exit: anchor(b, 48.0),
            action: Action::Teleport {
                trigger: t.model as u16,
                touch: a.origin,
                dest: arrive,
            },
            needs: Needs::default(),
            deadline: 4.0,
            cost: Cost {
                time: 0.2 + arrive.distance(b.origin) / RUN_SPEED,
                wait: 0.0,
                damage: 0.0,
            },
        };
        Some(Classified {
            kind: LinkKind::Teleport,
            valid: true,
            spec: self.spec(spec),
            dynamic: false,
        })
    }

    /// The solid brush entity a straight move from `a` to `b` runs into, if any.
    fn blocker(&mut self, a: &NavNode, b: &NavNode) -> Option<usize> {
        let (from, to, hull) = if a.flags.contains(NodeFlags::CROUCH) || b.flags.contains(NodeFlags::CROUCH) {
            (crouch_origin(a), crouch_origin(b), HullKind::Crouch)
        } else {
            (a.origin, b.origin, HullKind::Stand)
        };
        let lift = Vec3::Z * 2.0;
        let tr = self.world.trace(&TraceQuery::hull(from + lift, to + lift, hull));
        tr.hit.filter(|h| *h != 0 && tr.fraction < 1.0).map(|h| h as usize)
    }

    fn classify(&mut self, ai: usize, bi: usize, link_flags: u16) -> Classified {
        let (a, b) = (self.nodes[ai], self.nodes[bi]);
        if a.flags.contains(NodeFlags::LADDER) || b.flags.contains(NodeFlags::LADDER) {
            // A ladder node standing at the top or the foot of a ladder is also a floor node: a link to the same
            // floor is walked.
            let floor = |n: &NavNode| !n.flags.contains(NodeFlags::AIRBORNE);
            if floor(&a)
                && floor(&b)
                && (a.origin.z - b.origin.z).abs() < 18.0
                && walk_check(self.world, a.origin, b.origin, HullKind::Stand) == WalkCheck::Ok
            {
                return Classified::plain(LinkKind::Walk, true);
            }
            return self.ladder_link(&a, &b);
        }
        if a.flags.contains(NodeFlags::WATER) || b.flags.contains(NodeFlags::WATER) {
            let spec = TraversalSpec {
                entry: anchor(&a, 32.0),
                exit: anchor(&b, 32.0),
                action: Action::Swim,
                needs: Needs::default(),
                deadline: 6.0 + a.origin.distance(b.origin) / 100.0,
                cost: Cost {
                    time: a.origin.distance(b.origin) / (RUN_SPEED * LinkKind::Swim.speed_factor()),
                    wait: 0.0,
                    damage: 0.0,
                },
            };
            return Classified {
                kind: LinkKind::Swim,
                valid: true,
                spec: Some(spec),
                dynamic: false,
            };
        }
        if a.flags.contains(NodeFlags::AIRBORNE) || b.flags.contains(NodeFlags::AIRBORNE) {
            return Classified::plain(LinkKind::Walk, true);
        }
        let dynamic = a.flags.contains(NodeFlags::ON_MOVER) || b.flags.contains(NodeFlags::ON_MOVER);
        let walk = Classified {
            kind: LinkKind::Walk,
            valid: true,
            spec: None,
            dynamic,
        };
        if link_flags & LINK_JUMP != 0 {
            // yapb also marks hops around corners: where walking gets there, walk.
            if self.slides(&a, &b) {
                return walk;
            }
            return self
                .jump_link(&a, &b)
                .unwrap_or_else(|| Classified::plain(LinkKind::Jump, false));
        }
        if let Some((kind, r)) = self.walk(&a, &b) {
            if let WalkCheck::Drop(h) = r
                && h > 20.0
                && let Some(mut c) = self.drop_link(&a, &b)
            {
                c.dynamic = dynamic;
                return c;
            }
            return Classified {
                kind,
                valid: true,
                spec: None,
                dynamic,
            };
        }
        if self.slides(&a, &b) {
            return walk;
        }
        if let Some(model) = self.blocker(&a, &b) {
            if let Some(c) = self.door_link(&a, &b, model) {
                return c;
            }
            if let Some(c) = self.breakable_link(&a, &b, model) {
                return c;
            }
        }
        if let Some(c) = self.teleport_link(&a, &b) {
            return c;
        }
        if let Some(c) = self.jump_link(&a, &b) {
            return c;
        }
        let crouch = a.flags.contains(NodeFlags::CROUCH) || b.flags.contains(NodeFlags::CROUCH);
        Classified::plain(if crouch { LinkKind::Crouch } else { LinkKind::Walk }, false)
    }

    /// Lift links: from nodes on a platform at rest to nodes next to where it stops when activated.
    fn lift_links(&mut self, out: &mut [Vec<NavLink>]) -> usize {
        let mut added = 0;
        let movers: Vec<Mover> = self
            .mech
            .movers
            .iter()
            .filter(|m| matches!(m.kind, MoverKind::Door | MoverKind::Plat) && m.vertical())
            .cloned()
            .collect();
        for m in movers {
            let rise = (m.active - m.rest).z;
            if rise <= 0.0 {
                continue;
            }
            let board: Vec<NodeId> = (0..self.nodes.len() as NodeId)
                .filter(|&i| self.nodes[i as usize].support == m.model as u16)
                .collect();
            let Some(brush) = self.world.brush(m.model) else {
                continue;
            };
            let (mins, maxs) = (brush.abs_mins(), brush.abs_maxs());
            let top = maxs.z + rise;
            if board.is_empty() {
                continue;
            }
            // Where to start it from: an interaction reachable standing on the platform.
            let near = self.nodes[board[0] as usize].origin;
            let start = if m.kind == MoverKind::Plat && m.touch {
                Some(Interaction::Touch {
                    model: m.model as u16,
                    spot: near,
                })
            } else {
                let board_origins: Vec<Vec3> = board.iter().map(|&i| self.nodes[i as usize].origin).collect();
                board_origins.iter().find_map(|&o| {
                    let i = self.opener(&m, o)?;
                    let reach = matches!(i, Interaction::Use { .. } | Interaction::Touch { .. })
                        && board_origins.iter().any(|b| b.distance(i.spot()) < 1.0);
                    reach.then_some(i)
                })
            };
            let Some(start) = start else { continue };
            let mut exits: Vec<(f32, NodeId)> = (0..self.nodes.len() as NodeId)
                .filter(|&i| {
                    let n = &self.nodes[i as usize];
                    n.support != m.model as u16
                        && !n
                            .flags
                            .intersects(NodeFlags::LADDER | NodeFlags::AIRBORNE | NodeFlags::WATER)
                        && (n.origin.z - 36.0 - top).abs() <= 20.0
                        && box_distance(
                            n.origin,
                            Vec3::new(mins.x, mins.y, top),
                            Vec3::new(maxs.x, maxs.y, top + 72.0),
                        ) < 160.0
                })
                .map(|i| {
                    let d = box_distance(
                        self.nodes[i as usize].origin,
                        mins,
                        Vec3::new(maxs.x, maxs.y, top + 72.0),
                    );
                    (d, i)
                })
                .collect();
            exits.sort_by(|x, y| x.0.total_cmp(&y.0));
            exits.truncate(4);
            let saved = self.world.brush(m.model).map(|b| b.offset);
            if let Some(b) = self.world.brush_mut(m.model) {
                b.offset = m.active;
            }
            let platform = mech_ref(&m);
            for &from in &board {
                let a = self.nodes[from as usize];
                let raised = NavNode {
                    origin: a.origin + Vec3::Z * rise,
                    ..a
                };
                for &(_, to) in &exits {
                    let b = self.nodes[to as usize];
                    if self.walk(&raised, &b).is_none() {
                        continue;
                    }
                    let spec = TraversalSpec {
                        entry: anchor(&a, 24.0),
                        exit: anchor(&b, 32.0),
                        action: Action::Lift { platform, start },
                        needs: Needs::default(),
                        deadline: 10.0 + platform.travel * 2.0 + platform.wait.max(0.0),
                        cost: Cost {
                            time: platform.travel + raised.origin.distance(b.origin) / RUN_SPEED + 1.0,
                            wait: platform.travel + platform.wait.max(0.0),
                            damage: 0.0,
                        },
                    };
                    self.specs.push(spec);
                    let cost = spec.cost.time + spec.cost.wait;
                    out[from as usize].push(NavLink {
                        to,
                        kind: LinkKind::Lift,
                        length: a.origin.distance(b.origin),
                        flags: LinkFlags::VALID | LinkFlags::MECHANISM | LinkFlags::DYNAMIC,
                        cost,
                        spec: (self.specs.len() - 1) as u32,
                    });
                    added += 1;
                }
            }
            if let (Some(offset), Some(b)) = (saved, self.world.brush_mut(m.model)) {
                b.offset = offset;
            }
        }
        added
    }
}

pub fn import_yapb(
    g: &YapbGraph,
    world: &mut BspWorld,
    mech: &Mechanisms,
    opts: &ImportOptions,
    source: &str,
) -> NavGraph {
    let started = std::time::Instant::now();
    let traces_before = world.traces;
    mech.place_at_rest(world);
    let mut stats = GraphStats {
        nodes: g.nodes.len(),
        ..Default::default()
    };
    let mut ctx = Ctx {
        world,
        mech,
        phys: opts.physics,
        nodes: Vec::with_capacity(g.nodes.len()),
        specs: Vec::new(),
    };
    for n in &g.nodes {
        let mut flags = NodeFlags::empty();
        for (bit, flag) in [
            (node_flags::CROUCH, NodeFlags::CROUCH),
            (node_flags::LADDER, NodeFlags::LADDER),
            (node_flags::GOAL, NodeFlags::GOAL),
            (node_flags::CAMP, NodeFlags::CAMP),
            (node_flags::SNIPER, NodeFlags::SNIPER),
            (node_flags::BUTTON | node_flags::LIFT, NodeFlags::MECHANISM),
        ] {
            if n.flags & bit != 0 {
                flags |= flag;
            }
        }
        let (origin, flags, support) = ctx.settle(n.origin, flags);
        if flags.contains(NodeFlags::AIRBORNE) {
            stats.unsettled += 1;
        }
        ctx.nodes.push(NavNode {
            origin,
            flags,
            radius: n.radius.clamp(0.0, 128.0),
            support,
            first_link: 0,
            link_count: 0,
        });
    }
    let mut out: Vec<Vec<NavLink>> = vec![Vec::new(); ctx.nodes.len()];
    for (i, n) in g.nodes.iter().enumerate() {
        for l in &n.links {
            let to = l.index as usize;
            if to >= ctx.nodes.len() || to == i {
                continue;
            }
            let c = ctx.classify(i, to, l.flags);
            let (a, b) = (ctx.nodes[i], ctx.nodes[to]);
            let mut flags = LinkFlags::IMPORTED;
            if c.valid {
                flags |= LinkFlags::VALID;
            } else if opts.trust_imported {
                flags |= LinkFlags::VALID | LinkFlags::TRUSTED;
            }
            if c.dynamic {
                flags |= LinkFlags::DYNAMIC;
            }
            let length = a.origin.distance(b.origin);
            let (cost, spec) = match c.spec {
                Some(s) => {
                    ctx.specs.push(s);
                    (s.cost.time + s.cost.wait, (ctx.specs.len() - 1) as u32)
                }
                None => (length / (RUN_SPEED * c.kind.speed_factor()), NO_SPEC),
            };
            out[i].push(NavLink {
                to: to as NodeId,
                kind: c.kind,
                length,
                flags,
                cost: cost.max(length / RUN_SPEED),
                spec,
            });
        }
    }
    stats.added = ctx.lift_links(&mut out);
    let mut probes = Vec::new();
    for (i, list) in out.iter().enumerate() {
        for l in list.iter().filter(|l| l.valid()) {
            let Some(spec) = ctx.specs.get(l.spec as usize) else {
                continue;
            };
            let (a, b) = (ctx.nodes[i], ctx.nodes[l.to as usize]);
            probes.extend(crate::probe::probes_for(
                ctx.world,
                mech,
                (i as NodeId, &a),
                (l.to, &b),
                l.kind,
                spec,
            ));
        }
    }
    stats.traces = ctx.world.traces - traces_before;
    stats.millis = started.elapsed().as_millis();
    let Ctx { nodes, specs, .. } = ctx;
    let mut graph = NavGraph::from_parts(nodes, out, specs, source, stats);
    graph.probes = probes;
    graph
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossfire_imports_with_mechanisms() {
        let Some(maps) = lb_bsp::test_maps_dir() else { return };
        let (Ok(bsp), Ok(graph)) = (
            std::fs::read(maps.join("crossfire.bsp")),
            std::fs::read(maps.join("../addons/yapb/data/graph/crossfire.graph")),
        ) else {
            return;
        };
        let mut world = lb_bsp::BspWorld::load(&bsp).unwrap();
        let mech = Mechanisms::from_world(&world);
        let yapb = crate::yapb::parse(&graph).unwrap();
        let g = import_yapb(&yapb, &mut world, &mech, &ImportOptions::default(), "crossfire.graph");
        let s = &g.stats;
        eprintln!(
            "crossfire: {} nodes, {} links, {} invalid, {} added; {}; {} unsettled, {} traces, {} ms",
            s.nodes,
            s.links,
            s.invalid,
            s.added,
            s.kinds(),
            s.unsettled,
            s.traces,
            s.millis
        );
        assert_eq!(s.nodes, 1598);
        assert!(s.invalid * 20 < s.links, "more than 5% of links rejected: {s:?}");
        let reachable = crate::plan::plan(&g, 0, (g.len() - 1) as NodeId, &|_, _| 0.0);
        assert!(reachable.is_some(), "first and last node are connected");
        // Every tower lift carries bots up from its platform.
        for model in [21u16, 30, 32, 34] {
            assert!(
                g.links.iter().any(|l| l.kind == LinkKind::Lift
                    && matches!(g.spec(l).map(|s| s.action), Some(Action::Lift { platform, .. }) if platform.model == model)),
                "no lift link for *{model}"
            );
        }
        let lift = g.links.iter().find(|l| l.kind == LinkKind::Lift).unwrap();
        let Some(Action::Lift { start, .. }) = g.spec(lift).map(|s| s.action) else {
            unreachable!()
        };
        assert!(matches!(start, Interaction::Use { .. }), "{start:?}");
    }
}
