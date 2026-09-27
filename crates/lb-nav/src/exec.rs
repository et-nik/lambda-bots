//! Traversal executors (design v2 §9.1): a special link is carried out in phases with their own deadlines, pressing
//! only what a player would press and checking the outcome. A jump press is not a success until the bot lands at
//! the other end; a use press is not one until the door or lift moves.

use lb_core::math::view_angle_vectors;
use lb_core::{Vec2, Vec3};
use lb_kin::Physics;
use lb_kin::validate::{DROP_OVERRUN, DROP_SLACK, LIFTING, air_steer, hover, run_up_room, swim_jump, takeoff};
use lb_nav_api::NavStep;
use lb_worldq::Tracer;

use crate::graph::{NavNode, NodeFlags};
use crate::known::FailReason;
use crate::spec::{Action, Interaction, MechRef, TraversalSpec};

/// Eye height of a standing player above its origin (`VEC_VIEW`).
pub const EYE_HEIGHT: f32 = 28.0;
/// Steepest a player on the move looks up or down: the tangent of about 12°.
pub const TRAVEL_TILT: f32 = 0.2;

/// Where a player on the move looks toward `p`: that way, tilted up or down no more than `TRAVEL_TILT`.
pub fn travel_look(eye: Vec3, p: Vec3) -> Vec3 {
    let d = p - eye;
    let flat = d.truncate().length();
    eye + d.truncate().extend(d.z.clamp(-flat * TRAVEL_TILT, flat * TRAVEL_TILT))
}
/// A use press is sent once the view is this close to the target, degrees.
const USE_AIM: f32 = 8.0;
/// A mechanism that has not moved this long after it was set off did not react.
const REACTION: f64 = 1.5;
/// Presses before giving up on a button or door.
const PRESSES: u8 = 4;

/// The bot's own state for one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NavInput {
    pub now: f64,
    pub origin: Vec3,
    pub velocity: Vec3,
    /// View angles of the last command.
    pub view: Vec3,
    pub on_ground: bool,
    pub on_ladder: bool,
    pub ducked: bool,
    pub waterlevel: u8,
    /// Brush model under the feet (`groundentity`), 0 for the world or none.
    pub ground_model: u16,
    pub max_speed: f32,
    pub health: f32,
    /// Velocity of the push field the bot is in (`basevelocity` as `trigger_push` sets it); zero outside one.
    pub push: Vec3,
    /// Server gravity (`sv_gravity`); 0 when not known.
    pub gravity: f32,
}

impl NavInput {
    pub fn feet(&self) -> f32 {
        self.origin.z - if self.ducked { 18.0 } else { 36.0 }
    }

    /// Server gravity, the default when not known.
    pub fn gravity(&self) -> f32 {
        if self.gravity > 0.0 {
            self.gravity
        } else {
            Physics::default().gravity
        }
    }

    pub fn eye(&self) -> Vec3 {
        self.origin + Vec3::Z * if self.ducked { 12.0 } else { EYE_HEIGHT }
    }
}

/// Where a moving brush is now.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MoverState {
    /// Offset from where its model was compiled.
    pub offset: Vec3,
    pub velocity: Vec3,
}

impl MoverState {
    pub fn moving(&self) -> bool {
        self.velocity.length() > 1.0
    }

    pub fn at(&self, offset: Vec3) -> bool {
        self.offset.distance(offset) < 4.0 && !self.moving()
    }
}

/// What a trace hit, as far as getting stuck is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitKind {
    World,
    Player,
    /// A door, lift or other moving brush, by model.
    Mover(u16),
    Other,
}

/// The live state of the mechanisms the bot is operating. Executors use it only to carry out their own
/// traversal; planning never reads it.
pub trait MechView {
    fn mover(&self, model: u16) -> Option<MoverState>;
    /// The brush entity still exists (a breakable not broken yet).
    fn exists(&self, model: u16) -> bool;
    /// What `Trace::hit` refers to.
    fn hit_kind(&self, hit: u32) -> HitKind {
        if hit == 0 { HitKind::World } else { HitKind::Other }
    }
}

/// No mechanisms at all.
pub struct NoMechs;

impl MechView for NoMechs {
    fn mover(&self, _model: u16) -> Option<MoverState> {
        None
    }

    fn exists(&self, _model: u16) -> bool {
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecStatus {
    Running,
    /// Waiting for a mechanism: not progress, but not stuck either.
    Waiting,
    Done,
    Failed(FailReason),
}

pub struct ExecCtx<'a> {
    pub input: &'a NavInput,
    pub mech: &'a dyn MechView,
    pub tracer: &'a mut dyn Tracer,
    pub spec: &'a TraversalSpec,
    pub from: &'a NavNode,
    pub to: &'a NavNode,
}

fn flat(v: Vec3) -> Vec2 {
    v.truncate()
}

fn node_feet(n: &NavNode) -> f32 {
    n.origin.z
        - if n.flags.contains(NodeFlags::CROUCH) {
            18.0
        } else {
            36.0
        }
}

/// Standing at `n`: within `radius` across and on its floor.
fn at_node(input: &NavInput, n: &NavNode, radius: f32) -> bool {
    flat(n.origin - input.origin).length() < radius && (input.feet() - node_feet(n)).abs() < 24.0
}

/// Moving toward `target` at `speed` (slowing down the last few units so precise spots are not overshot).
fn toward(input: &NavInput, target: Vec3, speed: f32) -> NavStep {
    let d = flat(target - input.origin);
    let dist = d.length();
    let mut step = NavStep::hold(Vec3::new(target.x, target.y, input.origin.z + EYE_HEIGHT));
    if dist > 1.0 {
        step.move_dir = d / dist;
        step.speed = speed.min((dist * 6.0).max(60.0));
    }
    step
}

/// How far the view is from looking at `p`, degrees.
fn aim_error(input: &NavInput, p: Vec3) -> f32 {
    let (forward, _, _) = view_angle_vectors(input.view);
    let want = (p - input.eye()).normalize_or_zero();
    lb_core::dmath::acos(forward.dot(want).clamp(-1.0, 1.0)).to_degrees()
}

/// View pitch of looking from `eye` at `p`.
fn pitch_to(eye: Vec3, p: Vec3) -> f32 {
    lb_core::math::dir_to_view_angles(p - eye).x
}

/// Looking right at `p` and holding still: used to press buttons.
fn aim_at(input: &NavInput, p: Vec3) -> NavStep {
    let mut step = NavStep::hold(p);
    step.pitch = Some(pitch_to(input.eye(), p));
    step.mandatory = true;
    step
}

fn mover_at(mech: &dyn MechView, m: &MechRef, offset: Vec3) -> bool {
    mech.mover(m.model).is_some_and(|s| s.at(offset))
}

// ---------------------------------------------------------------------------------------------------------------
// Jump
// ---------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JumpPhase {
    Approach,
    RunUp,
    Takeoff,
    Air,
}

#[derive(Clone, Debug)]
pub struct JumpExec {
    phase: JumpPhase,
    since: f64,
    /// Run-ups and takeoffs that came to nothing so far.
    attempts: u8,
    /// Room behind the takeoff for a run-up, once measured.
    room: Option<f32>,
}

/// Time an approach after a run-up gets to reach takeoff speed.
const RUN_TIME: f64 = 0.3;

impl JumpExec {
    fn set(&mut self, phase: JumpPhase, now: f64) {
        self.phase = phase;
        self.since = now;
    }

    fn tick(&mut self, c: &mut ExecCtx<'_>, speed: f32, duck: bool) -> (NavStep, ExecStatus) {
        let i = c.input;
        let now = i.now;
        let (a, b) = (c.spec.entry.origin, c.spec.exit.origin);
        let dir = c.spec.dir();
        let rel = flat(i.origin - a);
        let along = rel.dot(dir);
        let lateral = (rel - dir * along).length();
        let v_along = flat(i.velocity).dot(dir);
        let airborne = !i.on_ground && !i.on_ladder && i.waterlevel < 2;
        let look = b + Vec3::Z * EYE_HEIGHT;
        let standing = speed <= 30.0;
        // Run at the speed the jump was checked with: faster overshoots short hops onto ledges and meets walls
        // lower. A standing jump walks up to the takeoff and stops there.
        let takeoff_speed = if standing {
            (flat(a - i.origin).length() * 4.0).clamp(40.0, 150.0)
        } else {
            speed.clamp(100.0, i.max_speed)
        };
        let room = *self.room.get_or_insert_with(|| run_up_room(c.tracer, a, dir, speed));
        let short = room < takeoff::SHORT_RUN;
        let run = |target: Vec3, pace: f32| {
            let mut step = toward(i, target, pace);
            step.speed = pace;
            step.look_at = look;
            step
        };
        for _ in 0..4 {
            match self.phase {
                // Airborne past the takeoff of a jump down or across: it ran off the edge, the flight is on.
                // Airborne before the takeoff, or short of a ledge above, is a step down some stairs or the landing
                // of whatever brought the bot here.
                JumpPhase::Approach | JumpPhase::RunUp
                    if airborne && along > 16.0 && lateral < 32.0 && node_feet(c.to) < node_feet(c.from) + 18.0 =>
                {
                    self.set(JumpPhase::Air, now)
                }
                JumpPhase::Approach => {
                    let front = takeoff::WINDOW.1 + if short { takeoff::SHORT_REACH } else { 0.0 };
                    let in_window = lateral < 16.0 && (takeoff::WINDOW.0..front).contains(&along);
                    let need = speed * if short { takeoff::SPEED_SHORT } else { takeoff::SPEED };
                    // Running the way the jump goes: sideways speed carries the flight off the line.
                    let v_side = (flat(i.velocity) - dir * v_along).length();
                    let speed_ok = if standing {
                        flat(i.velocity).length() < 50.0
                    } else {
                        v_along >= need && v_along <= speed * takeoff::TOO_FAST + 10.0 && v_side < speed * 0.25
                    };
                    if in_window && speed_ok {
                        self.set(JumpPhase::Takeoff, now);
                        continue;
                    }
                    // Too slow at the takeoff, or past it: back for a run-up. Not while still speeding up into
                    // the window, nor right after a run-up (turning around takes a while).
                    let overshot = along > front && lateral < 32.0;
                    let speeding_up = along < 0.0 && v_along > speed * 0.5;
                    let fresh = self.attempts > 0 && now - self.since < RUN_TIME;
                    let slow = in_window && !standing && !speeding_up && !fresh;
                    if overshot || slow {
                        self.attempts += 1;
                        tracing::debug!(
                            "jump: no takeoff (attempt {}): along {along:.0} lateral {lateral:.0} speed {v_along:.0} \
                             of {speed:.0}, run-up room {room:.0}",
                            self.attempts
                        );
                        if self.attempts > 3 {
                            return (run(a, takeoff_speed), ExecStatus::Failed(FailReason::ControllerFailure));
                        }
                        self.set(JumpPhase::RunUp, now);
                        continue;
                    }
                    // Head at the takeoff aimed along the jump, so the bot arrives running the right way.
                    let aim = if along < -48.0 { a } else { a + dir.extend(0.0) * 32.0 };
                    return (run(aim, takeoff_speed), ExecStatus::Running);
                }
                JumpPhase::RunUp => {
                    // Back to the start of the run-up and stop there: the run starts from rest, as validated.
                    let back = a - dir.extend(0.0) * room;
                    let left = flat(back - i.origin).length();
                    let stopped = left < 16.0 && flat(i.velocity).length() < 60.0;
                    if stopped || now - self.since > 1.5 {
                        self.set(JumpPhase::Approach, now);
                        return (run(a, takeoff_speed), ExecStatus::Running);
                    }
                    let pace = if left < 8.0 { 0.0 } else { (left * 5.0).min(i.max_speed) };
                    return (run(back, pace), ExecStatus::Running);
                }
                JumpPhase::Takeoff => {
                    if airborne {
                        self.set(JumpPhase::Air, now);
                        continue;
                    }
                    if now - self.since > 0.3 {
                        self.attempts += 1;
                        self.set(JumpPhase::Approach, now);
                        continue;
                    }
                    let mut step = run(b, takeoff_speed);
                    step.jump = true;
                    step.mandatory = true;
                    return (step, ExecStatus::Running);
                }
                JumpPhase::Air => {
                    // Steer at the landing, but stop pushing once over it.
                    let left = flat(b - i.origin).length();
                    let mut step = run(b, if left < 16.0 { 0.0 } else { i.max_speed });
                    step.mandatory = true;
                    step.duck = duck;
                    if !airborne && now - self.since > 0.1 {
                        if at_node(i, c.to, c.spec.exit.radius + 16.0) {
                            return (step, ExecStatus::Done);
                        }
                        // On the landing's floor and nearer the node than the takeoff (short of it, past it, or a
                        // hop around a corner): walk the rest, as the validator did.
                        let same_floor = (i.feet() - node_feet(c.to)).abs() < 20.0;
                        let nearer = left < flat(b - a).length() - 8.0;
                        if same_floor && nearer && left < 128.0 && now - self.since < 3.0 {
                            let mut s = toward(i, b, i.max_speed);
                            s.look_at = look;
                            return (s, ExecStatus::Running);
                        }
                        if at_node(i, c.from, 40.0) && self.attempts < 3 {
                            // Came down where it took off: the jump did not carry, try again.
                            self.attempts += 1;
                            self.set(JumpPhase::Approach, now);
                            return (run(a, takeoff_speed), ExecStatus::Running);
                        }
                        return (step, ExecStatus::Failed(FailReason::ControllerFailure));
                    }
                    return (step, ExecStatus::Running);
                }
            }
        }
        (run(a, takeoff_speed), ExecStatus::Running)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Drop
// ---------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct DropExec {
    airborne_at: Option<f64>,
}

impl DropExec {
    fn tick(&mut self, c: &mut ExecCtx<'_>, speed: f32) -> (NavStep, ExecStatus) {
        let i = c.input;
        let b = c.spec.exit.origin;
        // Off the ledge heading past the landing, as validated: slowing down at it stops on the ledge above it.
        let beyond = b + (b - c.spec.entry.origin).truncate().normalize_or_zero().extend(0.0) * DROP_OVERRUN;
        let pace = speed.max(150.0).min(i.max_speed);
        let mut step = if self.airborne_at.is_none() && flat(b - i.origin).length() < DROP_OVERRUN {
            let mut s = toward(i, beyond, pace);
            s.speed = pace;
            s
        } else {
            toward(i, b, pace)
        };
        match self.airborne_at {
            None => {
                if i.health <= c.spec.needs.health {
                    return (step, ExecStatus::Failed(FailReason::MissingCapability));
                }
                if !i.on_ground && !i.on_ladder {
                    self.airborne_at = Some(i.now);
                } else if at_node(i, c.to, c.spec.exit.radius) {
                    return (step, ExecStatus::Done);
                }
                (step, ExecStatus::Running)
            }
            Some(at) => {
                step.speed = i.max_speed;
                if i.on_ground && (i.feet() - node_feet(c.from)).abs() < 20.0 {
                    // Down a step, not off the edge yet.
                    self.airborne_at = None;
                    return (step, ExecStatus::Running);
                }
                if (i.on_ground || i.waterlevel >= 2) && i.now - at > 0.05 {
                    if at_node(i, c.to, c.spec.exit.radius + 24.0) {
                        return (step, ExecStatus::Done);
                    }
                    // Landed short or long: walking the rest is the next link's business if it can be walked.
                    if flat(b - i.origin).length() < DROP_SLACK && (i.feet() - node_feet(c.to)).abs() < 20.0 {
                        return (step, ExecStatus::Running);
                    }
                    return (step, ExecStatus::Failed(FailReason::ControllerFailure));
                }
                (step, ExecStatus::Running)
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Ladder
// ---------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct LadderExec {
    boarded: bool,
    left_at: Option<f64>,
    /// Since when the climb up has not risen (against the top of a ladder the ledge is behind or beside).
    stalled_at: Option<f64>,
}

impl LadderExec {
    fn tick(&mut self, c: &mut ExecCtx<'_>, normal: Vec3, mount: Vec3) -> (NavStep, ExecStatus) {
        let i = c.input;
        let target = c.to.origin;
        let to_ladder = c.to.flags.contains(NodeFlags::LADDER);
        let from_ladder = c.from.flags.contains(NodeFlags::LADDER);
        let face = -Vec3::new(normal.x, normal.y, 0.0).normalize_or_zero();
        let mut step = NavStep::hold(i.eye() + face * 64.0);
        step.mandatory = true;
        if i.on_ladder {
            self.boarded = true;
            self.left_at = None;
            let dz = target.z - i.origin.z;
            let off = flat(target - i.origin);
            let across = off - flat(face) * off.dot(flat(face));
            // The ladder node at the top has the ledge under it: it is reached standing there, off the ladder.
            let top = !c.to.flags.contains(NodeFlags::AIRBORNE) && target.z > c.from.origin.z + 32.0;
            if to_ladder && !top && dz.abs() < 12.0 && across.length() < 24.0 {
                return (step, ExecStatus::Done);
            }
            let rising = i.velocity.z > 20.0;
            let stalled = dz > 0.0 && !rising && i.now - *self.stalled_at.get_or_insert(i.now) > 0.3;
            if rising {
                self.stalled_at = None;
            }
            // At the top, or up against it with the ledge behind or beside: step off toward the ledge, as the
            // validator's climb does.
            let level = !to_ladder && (-24.0..8.0).contains(&dz);
            if (!to_ladder || top) && ((dz < 8.0 && target.z > c.from.origin.z - 8.0) || stalled || level) {
                // At the top: step off onto the ledge.
                let mut s = toward(i, target, i.max_speed);
                s.mandatory = true;
                s.pitch = Some(0.0);
                s.look_at = target + Vec3::Z * EYE_HEIGHT;
                return (s, ExecStatus::Running);
            }
            // Up: look up and press forward; down: look at the ladder and press back.
            let (pitch, dir) = if dz > 0.0 { (-45.0, face) } else { (0.0, -face) };
            let mut move_dir = flat(dir).normalize_or_zero();
            // Keep in line with a target on the ladder. A floor node away from the ladder is walked to from its
            // foot or top: edging toward it on the way slides the bot off the side.
            if to_ladder && across.length() > 8.0 {
                move_dir = (move_dir + across.normalize_or_zero() * 0.5).normalize_or_zero();
            }
            step.move_dir = move_dir;
            step.speed = i.max_speed;
            step.pitch = Some(pitch);
            return (step, ExecStatus::Running);
        }
        if !self.boarded && (to_ladder || from_ladder) {
            // From the top the mount point is down in the shaft: step back over the edge facing the ladder.
            if mount.z < i.origin.z - 12.0 {
                let d = flat(mount - i.origin);
                let mut s = NavStep::hold(i.eye() + face * 64.0);
                s.move_dir = if d.length() > 6.0 {
                    d.normalize_or_zero()
                } else {
                    flat(normal).normalize_or_zero()
                };
                s.speed = 150.0;
                s.pitch = Some(30.0);
                s.mandatory = true;
                let left = *self.left_at.get_or_insert(i.now);
                if i.now - left > 6.0 {
                    return (s, ExecStatus::Failed(FailReason::ControllerFailure));
                }
                return (s, ExecStatus::Running);
            }
            // From below or level: walk to the mount point, then into the ladder facing it; the ladder takes over
            // once the bot is in its volume.
            let near = flat(mount - i.origin).length() < 20.0;
            let mut s = if near {
                let mut s = NavStep::hold(i.eye() + face * 64.0);
                s.move_dir = flat(face).normalize_or_zero();
                s.speed = i.max_speed;
                s
            } else {
                toward(i, mount, i.max_speed)
            };
            s.mandatory = near;
            if near {
                s.look_at = i.eye() + face * 64.0;
                s.pitch = Some(if target.z > i.origin.z { -30.0 } else { 0.0 });
            }
            let left = *self.left_at.get_or_insert(i.now);
            if i.now - left > 6.0 {
                return (s, ExecStatus::Failed(FailReason::ControllerFailure));
            }
            return (s, ExecStatus::Running);
        }
        // Off the ladder: done once standing at the exit, else walk there.
        let left = *self.left_at.get_or_insert(i.now);
        if at_node(i, c.to, c.spec.exit.radius + 8.0) {
            return (step, ExecStatus::Done);
        }
        if i.now - left > 3.0 {
            return (step, ExecStatus::Failed(FailReason::ControllerFailure));
        }
        if to_ladder {
            // Fell off: back to the mount point.
            self.boarded = false;
            self.left_at = None;
            return (toward(i, mount, i.max_speed), ExecStatus::Running);
        }
        if i.on_ground {
            return (toward(i, target, i.max_speed), ExecStatus::Running);
        }
        (step, ExecStatus::Running)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Swim
// ---------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct SwimExec;

impl SwimExec {
    fn tick(&mut self, c: &mut ExecCtx<'_>) -> (NavStep, ExecStatus) {
        let i = c.input;
        let target = c.to.origin;
        if (target - i.origin).length() < 40.0 || (i.on_ground && at_node(i, c.to, c.spec.exit.radius)) {
            return (NavStep::hold(target), ExecStatus::Done);
        }
        let mut step = toward(i, target, i.max_speed);
        step.speed = i.max_speed;
        // Swimming goes where the view points, up and down too.
        step.look_at = target;
        step.pitch = Some(pitch_to(i.eye(), target));
        // Rise toward the surface (where the jump also climbs out of the water), or hop over a lip into it; duck
        // through low tunnels.
        let to_land = !c.to.flags.contains(NodeFlags::WATER);
        step.jump = swim_jump(i.origin, i.velocity, i.waterlevel, i.on_ground, target, to_land);
        step.duck = c.from.flags.contains(NodeFlags::CROUCH) || c.to.flags.contains(NodeFlags::CROUCH);
        (step, ExecStatus::Running)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Doors
// ---------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DoorPhase {
    Check,
    GoActivate,
    Activate,
    WaitOpen,
    Pass,
}

#[derive(Clone, Debug)]
pub struct DoorExec {
    phase: DoorPhase,
    since: f64,
    presses: u8,
    pressed_at: Option<f64>,
    pushed_at: Option<f64>,
    /// Through the doorway's middle already.
    via_done: bool,
}

impl DoorExec {
    fn set(&mut self, phase: DoorPhase, now: f64) {
        self.phase = phase;
        self.since = now;
    }

    fn tick(
        &mut self,
        c: &mut ExecCtx<'_>,
        door: MechRef,
        open: Interaction,
        via: Option<Vec3>,
    ) -> (NavStep, ExecStatus) {
        let i = c.input;
        let now = i.now;
        let state = c.mech.mover(door.model);
        let is_open = state.is_some_and(|s| s.at(door.active));
        let moving = state.is_some_and(|s| s.moving());
        let self_opens = open.model() == door.model;
        loop {
            match self.phase {
                DoorPhase::Check => {
                    if is_open {
                        self.set(DoorPhase::Pass, now);
                    } else if moving {
                        self.set(DoorPhase::WaitOpen, now);
                    } else if matches!(open, Interaction::Touch { .. }) && self_opens {
                        // A touch door opens when walked into.
                        self.set(DoorPhase::Pass, now);
                    } else {
                        self.set(DoorPhase::GoActivate, now);
                    }
                }
                DoorPhase::GoActivate => {
                    let spot = open.spot();
                    if flat(spot - i.origin).length() < 16.0 && (i.origin.z - spot.z).abs() < 40.0 {
                        self.set(DoorPhase::Activate, now);
                        continue;
                    }
                    if now - self.since > 8.0 {
                        return (
                            toward(i, spot, i.max_speed),
                            ExecStatus::Failed(FailReason::WaitingForInteraction),
                        );
                    }
                    return (toward(i, spot, i.max_speed), ExecStatus::Running);
                }
                DoorPhase::Activate => match open {
                    Interaction::Use { aim, .. } => {
                        let mut step = aim_at(i, aim);
                        if aim_error(i, aim) < USE_AIM && self.pressed_at.is_none_or(|t| now - t > 0.3) {
                            step.use_key = true;
                            self.presses += 1;
                            self.pressed_at = Some(now);
                            self.set(DoorPhase::WaitOpen, now);
                        } else if now - self.since > 2.0 {
                            return (step, ExecStatus::Failed(FailReason::ControllerFailure));
                        }
                        return (step, ExecStatus::Running);
                    }
                    Interaction::Shoot { aim, .. } => {
                        let mut step = aim_at(i, aim);
                        step.fire_at = Some(aim);
                        if moving || is_open {
                            self.set(DoorPhase::WaitOpen, now);
                        } else if now - self.since > 4.0 {
                            return (step, ExecStatus::Failed(FailReason::WaitingForInteraction));
                        }
                        return (step, ExecStatus::Running);
                    }
                    Interaction::Touch { .. } => {
                        // Standing on the plate is the touch.
                        self.presses += 1;
                        self.pressed_at = Some(now);
                        self.set(DoorPhase::WaitOpen, now);
                    }
                },
                DoorPhase::WaitOpen => {
                    if is_open {
                        self.set(DoorPhase::Pass, now);
                        continue;
                    }
                    let reacted = moving || self.pressed_at.is_none();
                    if !reacted && self.pressed_at.is_some_and(|t| now - t > REACTION) {
                        if self.presses >= PRESSES || self_opens && matches!(open, Interaction::Touch { .. }) {
                            return (
                                NavStep::hold(c.from.origin),
                                ExecStatus::Failed(FailReason::WaitingForInteraction),
                            );
                        }
                        self.set(DoorPhase::GoActivate, now);
                        continue;
                    }
                    if now - self.since > f64::from(door.travel) * 2.0 + 3.0 {
                        return (
                            NavStep::hold(c.from.origin),
                            ExecStatus::Failed(FailReason::WaitingForInteraction),
                        );
                    }
                    // Wait at the entry (back from a remote button), out of the door's way.
                    let mut step = toward(i, c.spec.entry.origin, i.max_speed);
                    step.look_at = c.to.origin + Vec3::Z * EYE_HEIGHT;
                    return (step, ExecStatus::Waiting);
                }
                DoorPhase::Pass => {
                    if at_node(i, c.to, c.spec.exit.radius) {
                        return (toward(i, c.to.origin, i.max_speed), ExecStatus::Done);
                    }
                    let target = match via.filter(|_| !self.via_done) {
                        Some(v) if flat(v - i.origin).length() < 16.0 => {
                            self.via_done = true;
                            c.to.origin
                        }
                        Some(v) => v,
                        None => c.to.origin,
                    };
                    let mut step = toward(i, target, i.max_speed);
                    // A low passage behind the door.
                    step.duck = c.from.flags.contains(NodeFlags::CROUCH) || c.to.flags.contains(NodeFlags::CROUCH);
                    let slow = flat(i.velocity).length() < 20.0;
                    if slow && !is_open {
                        // Pressing into a closed door: a touch door should start opening now.
                        let pushed = *self.pushed_at.get_or_insert(now);
                        if moving {
                            self.set(DoorPhase::WaitOpen, now);
                            self.pushed_at = None;
                        } else if now - pushed > REACTION {
                            if self_opens && matches!(open, Interaction::Touch { .. }) {
                                return (step, ExecStatus::Failed(FailReason::WaitingForInteraction));
                            }
                            self.pushed_at = None;
                            self.set(DoorPhase::GoActivate, now);
                        }
                    } else {
                        self.pushed_at = None;
                    }
                    return (step, ExecStatus::Running);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Lifts
// ---------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LiftPhase {
    WaitRest,
    Board,
    Start,
    Ride,
    Exit,
}

#[derive(Clone, Debug)]
pub struct LiftExec {
    phase: LiftPhase,
    since: f64,
    presses: u8,
    pressed_at: Option<f64>,
}

impl LiftExec {
    fn set(&mut self, phase: LiftPhase, now: f64) {
        self.phase = phase;
        self.since = now;
    }

    fn tick(&mut self, c: &mut ExecCtx<'_>, platform: MechRef, start: Interaction) -> (NavStep, ExecStatus) {
        let i = c.input;
        let now = i.now;
        let state = c.mech.mover(platform.model);
        let at_rest = mover_at(c.mech, &platform, platform.rest);
        let at_top = mover_at(c.mech, &platform, platform.active);
        let moving = state.is_some_and(|s| s.moving());
        let on_it = i.ground_model == platform.model;
        let spot = match start {
            Interaction::Use { spot, .. } | Interaction::Shoot { spot, .. } => spot,
            Interaction::Touch { .. } => c.spec.entry.origin,
        };
        loop {
            match self.phase {
                LiftPhase::WaitRest => {
                    if on_it && !moving {
                        self.set(LiftPhase::Board, now);
                        continue;
                    }
                    if at_rest {
                        self.set(LiftPhase::Board, now);
                        continue;
                    }
                    if now - self.since > f64::from(platform.travel * 2.0 + platform.wait.max(0.0)) + 5.0 {
                        return (
                            NavStep::hold(spot),
                            ExecStatus::Failed(FailReason::WaitingForInteraction),
                        );
                    }
                    // The platform is away: wait off it where the bot stands.
                    return (NavStep::hold(spot + Vec3::Z * EYE_HEIGHT), ExecStatus::Waiting);
                }
                LiftPhase::Board => {
                    if !at_rest && !on_it {
                        self.set(LiftPhase::WaitRest, now);
                        continue;
                    }
                    if on_it && flat(spot - i.origin).length() < 16.0 {
                        self.set(LiftPhase::Start, now);
                        continue;
                    }
                    if now - self.since > 6.0 {
                        return (
                            toward(i, spot, i.max_speed),
                            ExecStatus::Failed(FailReason::ControllerFailure),
                        );
                    }
                    return (toward(i, spot, i.max_speed), ExecStatus::Running);
                }
                LiftPhase::Start => {
                    if moving || !at_rest {
                        self.set(LiftPhase::Ride, now);
                        continue;
                    }
                    match start {
                        Interaction::Use { aim, .. } => {
                            let mut step = aim_at(i, aim);
                            let waited = self.pressed_at.is_none_or(|t| now - t > REACTION);
                            if waited && self.presses >= PRESSES {
                                return (step, ExecStatus::Failed(FailReason::WaitingForInteraction));
                            }
                            if aim_error(i, aim) < USE_AIM && waited {
                                step.use_key = true;
                                self.presses += 1;
                                self.pressed_at = Some(now);
                            } else if now - self.since > 3.0 && self.pressed_at.is_none() {
                                return (step, ExecStatus::Failed(FailReason::ControllerFailure));
                            }
                            return (step, ExecStatus::Waiting);
                        }
                        Interaction::Shoot { aim, .. } => {
                            let mut step = aim_at(i, aim);
                            step.fire_at = Some(aim);
                            if now - self.since > 4.0 {
                                return (step, ExecStatus::Failed(FailReason::WaitingForInteraction));
                            }
                            return (step, ExecStatus::Waiting);
                        }
                        Interaction::Touch { .. } => {
                            if now - self.since > REACTION * 2.0 {
                                return (
                                    NavStep::hold(spot),
                                    ExecStatus::Failed(FailReason::WaitingForInteraction),
                                );
                            }
                            return (NavStep::hold(c.to.origin), ExecStatus::Waiting);
                        }
                    }
                }
                LiftPhase::Ride => {
                    if at_top {
                        self.set(LiftPhase::Exit, now);
                        continue;
                    }
                    if !on_it && !i.on_ground && !moving {
                        return (
                            NavStep::hold(c.to.origin),
                            ExecStatus::Failed(FailReason::ControllerFailure),
                        );
                    }
                    if at_rest && now - self.since > REACTION {
                        // It came back down without us, or never went: start again.
                        self.set(LiftPhase::Board, now);
                        continue;
                    }
                    if now - self.since > f64::from(platform.travel) * 2.0 + 3.0 {
                        return (
                            NavStep::hold(c.to.origin),
                            ExecStatus::Failed(FailReason::WaitingForInteraction),
                        );
                    }
                    let mut step = NavStep::hold(c.to.origin + Vec3::Z * EYE_HEIGHT);
                    if on_it {
                        // Keep to the middle of the platform.
                        let back = flat(spot - i.origin);
                        if back.length() > 24.0 {
                            step.move_dir = back.normalize_or_zero();
                            step.speed = 80.0;
                        }
                    }
                    return (step, ExecStatus::Waiting);
                }
                LiftPhase::Exit => {
                    if at_node(i, c.to, c.spec.exit.radius) {
                        return (toward(i, c.to.origin, i.max_speed), ExecStatus::Done);
                    }
                    let window = f64::from(platform.wait.max(0.0)) + 2.5;
                    if now - self.since > window {
                        return (
                            toward(i, c.to.origin, i.max_speed),
                            ExecStatus::Failed(FailReason::ControllerFailure),
                        );
                    }
                    let mut step = toward(i, c.to.origin, i.max_speed);
                    step.speed = i.max_speed;
                    return (step, ExecStatus::Running);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Teleports and breakables
// ---------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct TeleportExec {
    last: Option<Vec3>,
    teleported: bool,
    at_touch: Option<f64>,
}

impl TeleportExec {
    fn tick(&mut self, c: &mut ExecCtx<'_>, touch: Vec3, dest: Vec3) -> (NavStep, ExecStatus) {
        let i = c.input;
        if let Some(last) = self.last
            && last.distance(i.origin) > 64.0
            && i.origin.distance(dest) < 128.0
        {
            self.teleported = true;
        }
        self.last = Some(i.origin);
        if self.teleported {
            if at_node(i, c.to, c.spec.exit.radius) || i.origin.distance(dest) < 32.0 {
                return (NavStep::hold(c.to.origin), ExecStatus::Done);
            }
            return (toward(i, c.to.origin, i.max_speed), ExecStatus::Running);
        }
        if flat(touch - i.origin).length() < 16.0 {
            let since = *self.at_touch.get_or_insert(i.now);
            if i.now - since > 1.5 {
                return (NavStep::hold(touch), ExecStatus::Failed(FailReason::GeometryInvalid));
            }
        }
        let mut step = toward(i, touch, i.max_speed);
        step.speed = step.speed.max(120.0);
        (step, ExecStatus::Running)
    }
}

#[derive(Clone, Debug, Default)]
pub struct BreakExec;

impl BreakExec {
    fn tick(&mut self, c: &mut ExecCtx<'_>, model: u16, aim: Vec3, crowbar: bool) -> (NavStep, ExecStatus) {
        let i = c.input;
        if !c.mech.exists(model) {
            if at_node(i, c.to, c.spec.exit.radius) {
                return (NavStep::hold(c.to.origin), ExecStatus::Done);
            }
            return (toward(i, c.to.origin, i.max_speed), ExecStatus::Running);
        }
        let reach = if crowbar { 40.0 } else { 0.0 };
        let dist = flat(aim - i.origin).length();
        if crowbar && dist > reach + 16.0 {
            return (toward(i, aim, i.max_speed), ExecStatus::Running);
        }
        let mut step = aim_at(i, aim);
        step.fire_at = Some(aim);
        step.melee = crowbar;
        (step, ExecStatus::Waiting)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Push fields
// ---------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PushPhase {
    /// To the entry, stopping there: the run was checked from rest.
    Approach,
    /// Along the run into the field.
    Run,
    /// Thrown or carried by the field.
    Flight,
    /// Down again: walk or swim the rest.
    Landed,
}

#[derive(Clone, Debug)]
pub struct PushExec {
    phase: PushPhase,
    since: f64,
    jumped: bool,
    runs: u8,
    best: f32,
    best_at: f64,
}

impl PushExec {
    fn set(&mut self, phase: PushPhase, now: f64) {
        self.phase = phase;
        self.since = now;
    }

    /// Mirrors `lb_kin::validate::simulate_push`: the same run, the same jump, the same steering.
    fn tick(
        &mut self,
        c: &mut ExecCtx<'_>,
        dir: Vec2,
        jump_at: Option<f32>,
        hold: Option<Vec2>,
    ) -> (NavStep, ExecStatus) {
        let i = c.input;
        let now = i.now;
        let (a, b) = (c.spec.entry.origin, c.spec.exit.origin);
        let look = b + Vec3::Z * EYE_HEIGHT;
        let airborne = !i.on_ground && !i.on_ladder && i.waterlevel < 2;
        let lifting = i.push.z > LIFTING;
        // The field shows as a flight, as its push, or as more speed along the floor than running gives.
        let carried = airborne || i.push != Vec3::ZERO || flat(i.velocity).length() > i.max_speed * 1.25;
        for _ in 0..4 {
            match self.phase {
                PushPhase::Approach => {
                    let left = flat(a - i.origin).length();
                    if left < 12.0 && flat(i.velocity).length() < 40.0 && i.on_ground {
                        self.jumped = false;
                        self.set(PushPhase::Run, now);
                        continue;
                    }
                    if now - self.since > 6.0 {
                        return (NavStep::hold(look), ExecStatus::Failed(FailReason::ControllerFailure));
                    }
                    let mut step = toward(i, a, i.max_speed);
                    step.look_at = i.eye() + dir.extend(0.0) * 64.0;
                    return (step, ExecStatus::Running);
                }
                PushPhase::Run => {
                    if carried {
                        self.set(PushPhase::Flight, now);
                        continue;
                    }
                    if now - self.since > 3.0 {
                        // The field did not take hold: switched off, or the run missed it.
                        self.runs += 1;
                        if self.runs >= 2 {
                            return (NavStep::hold(look), ExecStatus::Failed(FailReason::GeometryInvalid));
                        }
                        self.set(PushPhase::Approach, now);
                        continue;
                    }
                    let along = flat(i.origin - a).dot(dir);
                    let mut step = NavStep::hold(i.eye() + dir.extend(0.0) * 64.0);
                    step.move_dir = dir;
                    step.speed = i.max_speed;
                    step.mandatory = true;
                    if !self.jumped && i.on_ground && jump_at.is_some_and(|j| along >= j) {
                        step.jump = true;
                        self.jumped = true;
                    }
                    return (step, ExecStatus::Running);
                }
                PushPhase::Flight => {
                    if now - self.since > 12.0 {
                        return (NavStep::hold(look), ExecStatus::Failed(FailReason::ControllerFailure));
                    }
                    if i.waterlevel >= 2 {
                        if (b - i.origin).length() < 24.0 {
                            return (NavStep::hold(look), ExecStatus::Done);
                        }
                        let mut step = toward(i, b, i.max_speed);
                        step.speed = i.max_speed;
                        step.look_at = b;
                        let to_land = !c.to.flags.contains(NodeFlags::WATER);
                        step.jump = swim_jump(i.origin, i.velocity, i.waterlevel, i.on_ground, b, to_land);
                        step.mandatory = true;
                        return (step, ExecStatus::Running);
                    }
                    if let Some(spot) = hold.filter(|_| lifting) {
                        let mut step = NavStep::hold(look);
                        if let Some(d) = hover(i.origin, i.velocity, spot) {
                            step.move_dir = d;
                            step.speed = i.max_speed;
                        }
                        step.mandatory = true;
                        return (step, ExecStatus::Running);
                    }
                    if airborne {
                        let mut step = NavStep::hold(look);
                        if let Some(d) = air_steer(i.origin, i.velocity, b, i.gravity()) {
                            step.move_dir = d;
                            step.speed = i.max_speed;
                        }
                        step.mandatory = true;
                        return (step, ExecStatus::Running);
                    }
                    if !carried {
                        self.best = f32::INFINITY;
                        self.set(PushPhase::Landed, now);
                        continue;
                    }
                    let mut step = toward(i, b, i.max_speed);
                    step.look_at = look;
                    return (step, ExecStatus::Running);
                }
                PushPhase::Landed => {
                    if at_node(i, c.to, c.spec.exit.radius + 8.0) {
                        return (NavStep::hold(look), ExecStatus::Done);
                    }
                    if (airborne && i.velocity.z > 300.0) || flat(i.velocity).length() > i.max_speed * 1.25 {
                        // Thrown again: the landing was the field once more.
                        return (NavStep::hold(look), ExecStatus::Failed(FailReason::ControllerFailure));
                    }
                    let left = (b - i.origin).length();
                    if left < self.best - 1.0 {
                        self.best = left;
                        self.best_at = now;
                    } else if now - self.best_at > 1.5 {
                        return (NavStep::hold(look), ExecStatus::Failed(FailReason::ControllerFailure));
                    }
                    let mut step = toward(i, b, i.max_speed);
                    step.look_at = look;
                    return (step, ExecStatus::Running);
                }
            }
        }
        (NavStep::hold(look), ExecStatus::Running)
    }
}

// ---------------------------------------------------------------------------------------------------------------

/// An executor running one special link.
#[derive(Clone, Debug)]
pub enum Exec {
    Jump(JumpExec),
    Drop(DropExec),
    Ladder(LadderExec),
    Swim(SwimExec),
    Door(DoorExec),
    Lift(LiftExec),
    Teleport(TeleportExec),
    Breakable(BreakExec),
    Push(PushExec),
}

impl Exec {
    pub fn new(spec: &TraversalSpec, now: f64) -> Exec {
        match spec.action {
            Action::Jump { .. } => Exec::Jump(JumpExec {
                phase: JumpPhase::Approach,
                since: now,
                attempts: 0,
                room: None,
            }),
            Action::Drop { .. } => Exec::Drop(DropExec::default()),
            Action::Ladder { .. } => Exec::Ladder(LadderExec::default()),
            Action::Swim => Exec::Swim(SwimExec),
            Action::Door { .. } => Exec::Door(DoorExec {
                phase: DoorPhase::Check,
                since: now,
                presses: 0,
                pressed_at: None,
                pushed_at: None,
                via_done: false,
            }),
            Action::Lift { .. } => Exec::Lift(LiftExec {
                phase: LiftPhase::WaitRest,
                since: now,
                presses: 0,
                pressed_at: None,
            }),
            Action::Teleport { .. } => Exec::Teleport(TeleportExec::default()),
            Action::Breakable { .. } => Exec::Breakable(BreakExec),
            Action::Push { .. } => Exec::Push(PushExec {
                phase: PushPhase::Approach,
                since: now,
                jumped: false,
                runs: 0,
                best: f32::INFINITY,
                best_at: now,
            }),
        }
    }

    pub fn tick(&mut self, c: &mut ExecCtx<'_>) -> (NavStep, ExecStatus) {
        match (self, c.spec.action) {
            (Exec::Jump(e), Action::Jump { speed, duck, .. }) => e.tick(c, speed, duck),
            (Exec::Drop(e), Action::Drop { speed, .. }) => e.tick(c, speed),
            (Exec::Ladder(e), Action::Ladder { normal, mount, .. }) => e.tick(c, normal, mount),
            (Exec::Swim(e), Action::Swim) => e.tick(c),
            (Exec::Door(e), Action::Door { door, open, via }) => e.tick(c, door, open, via),
            (Exec::Lift(e), Action::Lift { platform, start }) => e.tick(c, platform, start),
            (Exec::Teleport(e), Action::Teleport { touch, dest, .. }) => e.tick(c, touch, dest),
            (
                Exec::Breakable(e),
                Action::Breakable {
                    model, aim, crowbar, ..
                },
            ) => e.tick(c, model, aim, crowbar),
            (Exec::Push(e), Action::Push { dir, jump_at, hold, .. }) => e.tick(c, dir, jump_at, hold),
            _ => (
                NavStep::hold(c.to.origin),
                ExecStatus::Failed(FailReason::ControllerFailure),
            ),
        }
    }

    /// Name of the current phase, for diagnostics.
    pub fn phase(&self) -> &'static str {
        match self {
            Exec::Jump(e) => match e.phase {
                JumpPhase::Approach => "jump:approach",
                JumpPhase::RunUp => "jump:run-up",
                JumpPhase::Takeoff => "jump:takeoff",
                JumpPhase::Air => "jump:air",
            },
            Exec::Drop(e) => {
                if e.airborne_at.is_some() {
                    "drop:fall"
                } else {
                    "drop:edge"
                }
            }
            Exec::Ladder(e) => {
                if e.boarded {
                    "ladder:climb"
                } else {
                    "ladder:board"
                }
            }
            Exec::Swim(_) => "swim",
            Exec::Door(e) => match e.phase {
                DoorPhase::Check => "door:check",
                DoorPhase::GoActivate => "door:go-activate",
                DoorPhase::Activate => "door:activate",
                DoorPhase::WaitOpen => "door:wait",
                DoorPhase::Pass => "door:pass",
            },
            Exec::Lift(e) => match e.phase {
                LiftPhase::WaitRest => "lift:wait",
                LiftPhase::Board => "lift:board",
                LiftPhase::Start => "lift:start",
                LiftPhase::Ride => "lift:ride",
                LiftPhase::Exit => "lift:exit",
            },
            Exec::Teleport(_) => "teleport",
            Exec::Breakable(_) => "breakable",
            Exec::Push(e) => match e.phase {
                PushPhase::Approach => "push:approach",
                PushPhase::Run => "push:run",
                PushPhase::Flight => "push:flight",
                PushPhase::Landed => "push:landed",
            },
        }
    }
}
