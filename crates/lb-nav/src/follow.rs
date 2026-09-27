//! Path following: turns a planned node path into movement, look, jump and duck requests every frame. Cheap: no
//! traces; the caller supplies the bot's own state.

use lb_core::{Vec2, Vec3};

use crate::graph::{LinkKind, NavGraph, NodeFlags, NodeId};

/// Eye height of a standing player above its origin (`VEC_VIEW`).
pub const EYE_HEIGHT: f32 = 28.0;
const PROGRESS_WINDOW: f64 = 0.8;
const PROGRESS_MIN: f32 = 8.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct FollowInput {
    pub origin: Vec3,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub on_ladder: bool,
    pub now: f64,
    pub max_speed: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FollowStatus {
    Moving,
    Arrived,
    /// No progress after the recovery moves; plan again, avoiding `(from, to)`.
    Stuck {
        from: NodeId,
        to: NodeId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FollowOutput {
    /// Horizontal direction to move in, world space; zero = stand still.
    pub move_dir: Vec2,
    pub speed: f32,
    pub look_at: Vec3,
    /// Pitch to hold on ladders (positive looks down, as in view angles).
    pub pitch: Option<f32>,
    pub jump: bool,
    pub duck: bool,
    /// On a jump or ladder link: its look and stance must not be overridden.
    pub mandatory: bool,
    pub status: FollowStatus,
}

#[derive(Clone, Debug)]
pub struct PathFollower {
    path: Vec<NodeId>,
    next: usize,
    best: f32,
    last_progress: f64,
    recovery: u8,
    recovery_until: f64,
    strafe_sign: f32,
    jumped_at: f64,
}

impl PathFollower {
    /// `path[0]` is the node the bot starts at.
    pub fn new(path: Vec<NodeId>, now: f64) -> PathFollower {
        PathFollower {
            next: usize::from(path.len() > 1),
            path,
            best: f32::INFINITY,
            last_progress: now,
            recovery: 0,
            recovery_until: 0.0,
            strafe_sign: 1.0,
            jumped_at: f64::NEG_INFINITY,
        }
    }

    pub fn goal(&self) -> NodeId {
        *self.path.last().expect("paths are not empty")
    }

    pub fn target(&self) -> Option<NodeId> {
        self.path.get(self.next).copied()
    }

    pub fn remaining(&self) -> &[NodeId] {
        &self.path[self.next.min(self.path.len())..]
    }

    fn kind(&self, g: &NavGraph) -> LinkKind {
        if self.next == 0 {
            return LinkKind::Walk;
        }
        g.find_link(self.path[self.next - 1], self.path[self.next])
            .map(|l| l.kind)
            .unwrap_or(LinkKind::Walk)
    }

    fn reached(&self, g: &NavGraph, s: &FollowInput, kind: LinkKind) -> bool {
        let node = g.node(self.path[self.next]);
        let to = node.origin - s.origin;
        let flat = to.truncate().length();
        let last = self.next + 1 == self.path.len();
        match kind {
            LinkKind::Ladder => flat < 24.0 && to.z.abs() < 24.0,
            LinkKind::Jump => s.on_ground && flat < 32.0 && s.origin.z > node.origin.z - 20.0,
            _ => {
                let radius = if last { 24.0 } else { node.radius.max(24.0) };
                if flat < radius && to.z.abs() < 40.0 {
                    return true;
                }
                // Passed the node: the plane through it, across the segment we came along.
                if !last && self.next > 0 && to.z.abs() < 40.0 {
                    let from = g.node(self.path[self.next - 1]).origin;
                    let seg = (node.origin - from).truncate();
                    if seg.length() > 1.0 && seg.dot(-to.truncate()) > 0.0 && flat < 64.0 {
                        return true;
                    }
                }
                false
            }
        }
    }

    pub fn tick(&mut self, g: &NavGraph, s: &FollowInput) -> FollowOutput {
        let mut out = FollowOutput {
            move_dir: Vec2::ZERO,
            speed: 0.0,
            look_at: s.origin + Vec3::Z * EYE_HEIGHT,
            pitch: None,
            jump: false,
            duck: false,
            mandatory: false,
            status: FollowStatus::Moving,
        };
        while self.next < self.path.len() && self.reached(g, s, self.kind(g)) {
            self.next += 1;
            self.best = f32::INFINITY;
            self.last_progress = s.now;
            self.recovery = 0;
        }
        let Some(target) = self.target() else {
            out.status = FollowStatus::Arrived;
            return out;
        };
        let kind = self.kind(g);
        let node = g.node(target);
        let to = node.origin - s.origin;
        let flat = to.truncate().length();
        out.move_dir = to.truncate().normalize_or_zero();
        out.speed = s.max_speed;
        out.look_at = node.origin + Vec3::Z * EYE_HEIGHT;
        // Look further along a straight walk so turns start early.
        if matches!(kind, LinkKind::Walk | LinkKind::Crouch)
            && let Some(&after) = self.path.get(self.next + 1)
            && flat < 96.0
        {
            out.look_at = g.node(after).origin + Vec3::Z * EYE_HEIGHT;
        }
        out.duck = kind == LinkKind::Crouch || node.flags.contains(NodeFlags::CROUCH);
        out.mandatory = kind == LinkKind::Jump || (kind == LinkKind::Ladder && s.on_ladder);
        match kind {
            LinkKind::Jump => {
                let from = g.node(self.path[self.next - 1]).origin;
                let near_start = (s.origin - from).truncate().length() < 48.0;
                if s.on_ground && near_start && s.now - self.jumped_at > 0.6 {
                    out.jump = true;
                    self.jumped_at = s.now;
                }
                out.duck = !s.on_ground;
            }
            LinkKind::Ladder => {
                if s.on_ladder {
                    out.pitch = Some(if to.z > 8.0 {
                        -60.0
                    } else if to.z < -8.0 {
                        60.0
                    } else {
                        0.0
                    });
                }
            }
            LinkKind::Walk | LinkKind::Crouch | LinkKind::Drop => {}
        }
        self.recover(s, flat, kind, &mut out);
        out
    }

    /// Progress watchdog: jump, then strafe, then give up so the caller plans again.
    fn recover(&mut self, s: &FollowInput, flat: f32, kind: LinkKind, out: &mut FollowOutput) {
        if flat < self.best - PROGRESS_MIN {
            self.best = flat;
            self.last_progress = s.now;
            return;
        }
        if s.now < self.recovery_until {
            if self.recovery == 2 {
                out.move_dir = Vec2::new(-out.move_dir.y, out.move_dir.x) * self.strafe_sign;
            }
            return;
        }
        if s.now - self.last_progress < PROGRESS_WINDOW || kind == LinkKind::Ladder && s.on_ladder {
            return;
        }
        self.recovery += 1;
        self.last_progress = s.now;
        match self.recovery {
            1 if s.on_ground => {
                out.jump = true;
                out.duck = false;
                self.recovery_until = s.now + 0.4;
            }
            1 | 2 => {
                self.recovery = 2;
                self.strafe_sign = if (s.now * 7.0).fract() < 0.5 { 1.0 } else { -1.0 };
                self.recovery_until = s.now + 0.5;
            }
            _ => {
                let from = if self.next > 0 {
                    self.path[self.next - 1]
                } else {
                    self.path[0]
                };
                out.status = FollowStatus::Stuck {
                    from,
                    to: self.path[self.next.min(self.path.len() - 1)],
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{NavLink, NavNode};

    fn line_graph(kinds: &[LinkKind]) -> NavGraph {
        let n = kinds.len() + 1;
        let mut nodes: Vec<NavNode> = (0..n)
            .map(|i| NavNode {
                origin: Vec3::new(i as f32 * 100.0, 0.0, 0.0),
                flags: NodeFlags::empty(),
                radius: 16.0,
                first_link: i as u32,
                link_count: 0,
            })
            .collect();
        let mut links = Vec::new();
        for (i, kind) in kinds.iter().enumerate() {
            nodes[i].link_count = 1;
            links.push(NavLink {
                to: i as u32 + 1,
                kind: *kind,
                length: 100.0,
                valid: true,
            });
        }
        nodes[n - 1].first_link = links.len() as u32;
        NavGraph {
            nodes,
            links,
            ..Default::default()
        }
    }

    fn input(x: f32, now: f64) -> FollowInput {
        FollowInput {
            origin: Vec3::new(x, 0.0, 0.0),
            on_ground: true,
            now,
            max_speed: 300.0,
            ..Default::default()
        }
    }

    #[test]
    fn walks_node_to_node_and_arrives() {
        let g = line_graph(&[LinkKind::Walk, LinkKind::Walk]);
        let mut f = PathFollower::new(vec![0, 1, 2], 0.0);
        let out = f.tick(&g, &input(0.0, 0.0));
        assert_eq!(out.move_dir, Vec2::X);
        assert_eq!(f.target(), Some(1));
        f.tick(&g, &input(90.0, 0.3));
        assert_eq!(f.target(), Some(2), "within the radius of node 1");
        assert_eq!(f.tick(&g, &input(195.0, 0.6)).status, FollowStatus::Arrived);
    }

    #[test]
    fn jump_links_press_once_near_the_start() {
        let g = line_graph(&[LinkKind::Jump]);
        let mut f = PathFollower::new(vec![0, 1], 0.0);
        assert!(f.tick(&g, &input(10.0, 0.0)).jump);
        assert!(!f.tick(&g, &input(12.0, 0.1)).jump, "no second press right after");
        let mut air = input(40.0, 0.2);
        air.on_ground = false;
        let out = f.tick(&g, &air);
        assert!(out.duck && !out.jump, "crouch in the air");
    }

    #[test]
    fn no_progress_escalates_to_stuck() {
        let g = line_graph(&[LinkKind::Walk]);
        let mut f = PathFollower::new(vec![0, 1], 0.0);
        let mut statuses = Vec::new();
        let mut jumped = false;
        for i in 0..60 {
            let out = f.tick(&g, &input(0.0, i as f64 * 0.1));
            jumped |= out.jump;
            statuses.push(out.status);
        }
        assert!(jumped, "first recovery is a jump");
        assert!(statuses.contains(&FollowStatus::Stuck { from: 0, to: 1 }));
    }
}
