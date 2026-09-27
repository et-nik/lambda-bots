//! Player movement as the server runs it for every user command (`SV_RunCmd` → `PM_PlayerMove`): acceleration
//! and friction, stepping up stairs, sliding along walls, gravity, jumps and long jumps, ducking, ladders,
//! swimming and water jumps. Written to behave like the SDK's `pm_shared`, so offline checks and tests move a
//! player the way the server does. Water currents, conveyors and player-to-player collisions are left out.

use lb_core::Vec3;
use lb_core::input::{IN_BACK, IN_DUCK, IN_FORWARD, IN_JUMP, IN_MOVELEFT, IN_MOVERIGHT, IN_USE};
use lb_core::math::view_angle_vectors;
use lb_worldq::{HullKind, Trace, TraceQuery, Tracer, contents};
use smallvec::SmallVec;

use crate::physics::Physics;

pub const VIEW_STAND: f32 = 28.0;
pub const VIEW_DUCK: f32 = 12.0;
/// A duck press finishes crouching after this long on the ground (in the air, at once).
pub const TIME_TO_DUCK: f32 = 0.4;
pub const DUCK_MULTIPLIER: f32 = 0.333;
pub const CLIMB_SPEED: f32 = 200.0;
/// Upward speed of a jump: reaches 45 units under 800 gravity.
pub const JUMP_SPEED: f32 = 268.328_16;
/// Upward speed of a long jump: 56 units.
pub const LONGJUMP_UP: f32 = 299.332_6;
/// Horizontal speed of a long jump along the view.
pub const LONGJUMP_SPEED: f32 = 560.0;
/// Pushed off a ladder by a jump at this speed.
pub const LADDER_JUMP: f32 = 270.0;
const STOP_EPSILON: f32 = 0.1;
const MAX_CLIP_PLANES: usize = 5;
const BUNNYJUMP_MAX_SPEED_FACTOR: f32 = 1.7;
const WATERJUMP_HEIGHT: f32 = 8.0;
/// Commands longer than this are run as two halves.
pub const MAX_CMD_MSEC: u8 = 50;

/// World queries movement needs beyond traces.
pub trait MoveWorld: Tracer {
    /// The ladder the player box (`hull` at `origin`) is in, with the normal of its face pointing out.
    fn ladder(&mut self, origin: Vec3, hull: HullKind) -> Option<Ladder>;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ladder {
    /// Face normal toward the player; `None` when the trace toward the ladder centre did not touch it.
    pub normal: Option<Vec3>,
}

/// The player's movement state between commands (the entity fields `SV_RunCmd` hands to movement and back).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Player {
    pub origin: Vec3,
    pub velocity: Vec3,
    /// Crouched in the small hull (`FL_DUCKING`).
    pub ducked: bool,
    /// Crouching down, not in the small hull yet (`bInDuck`).
    pub ducking: bool,
    /// `flDuckTime`, whole milliseconds: 1000 on a fresh duck press, counting down.
    pub duck_time: f32,
    pub view_z: f32,
    /// What the feet stand on (`Trace::hit` of the ground trace: 0 = world); `None` in the air.
    pub ground: Option<u32>,
    pub waterlevel: u8,
    pub watertype: i32,
    /// On a ladder (movetype fly).
    pub on_ladder: bool,
    /// Buttons of the previous command (`oldbuttons`): a jump needs a fresh press.
    pub oldbuttons: u16,
    /// Downward speed while airborne, still set on the command that lands.
    pub fall_velocity: f32,
    /// Milliseconds of a water jump left (`teleport_time`).
    pub waterjump_time: f32,
    pub waterjump_dir: Vec3,
    /// Has the long jump module (the `slj` physics key).
    pub longjump: bool,
    /// The player's own speed cap (`pev->maxspeed`); 0 = only `sv_maxspeed`.
    pub client_maxspeed: f32,
    pub dead: bool,
}

impl Player {
    /// Standing still at `origin` (the centre of the standing hull).
    pub fn standing(origin: Vec3) -> Player {
        Player {
            origin,
            velocity: Vec3::ZERO,
            ducked: false,
            ducking: false,
            duck_time: 0.0,
            view_z: VIEW_STAND,
            ground: None,
            waterlevel: 0,
            watertype: contents::EMPTY,
            on_ladder: false,
            oldbuttons: 0,
            fall_velocity: 0.0,
            waterjump_time: 0.0,
            waterjump_dir: Vec3::ZERO,
            longjump: false,
            client_maxspeed: 0.0,
            dead: false,
        }
    }

    pub fn hull(&self) -> HullKind {
        if self.ducked { HullKind::Crouch } else { HullKind::Stand }
    }

    pub fn on_ground(&self) -> bool {
        self.ground.is_some()
    }

    /// Height of the feet.
    pub fn feet(&self) -> f32 {
        self.origin.z + self.hull().extents().0.z
    }

    pub fn eye(&self) -> Vec3 {
        self.origin + Vec3::Z * self.view_z
    }
}

/// One user command.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cmd {
    /// View angles, pitch positive down.
    pub angles: Vec3,
    pub forward: f32,
    pub side: f32,
    pub up: f32,
    pub buttons: u16,
    pub msec: u8,
}

/// What happened during a command.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MoveEvents {
    pub jumped: bool,
    pub longjumped: bool,
    /// Touched down at this downward speed.
    pub landed: Option<f32>,
    /// Entities the move ran into or stood on (`Trace::hit`, world excluded).
    pub touched: SmallVec<[u32; 4]>,
}

impl MoveEvents {
    fn merge(&mut self, other: MoveEvents) {
        self.jumped |= other.jumped;
        self.longjumped |= other.longjumped;
        self.landed = self.landed.or(other.landed);
        for t in other.touched {
            if !self.touched.contains(&t) {
                self.touched.push(t);
            }
        }
    }
}

/// Runs one command, split in halves above 50 ms as the server does.
pub fn player_move(world: &mut dyn MoveWorld, phys: &Physics, p: &mut Player, cmd: &Cmd) -> MoveEvents {
    if cmd.msec > MAX_CMD_MSEC {
        let half = Cmd {
            msec: cmd.msec / 2,
            ..*cmd
        };
        let mut events = player_move(world, phys, p, &half);
        events.merge(player_move(world, phys, p, &half));
        return events;
    }
    let mut pm = Pm {
        world,
        phys,
        p: *p,
        cmd: *cmd,
        frametime: f32::from(cmd.msec) * 0.001,
        maxspeed: phys.maxspeed,
        forward: Vec3::ZERO,
        right: Vec3::ZERO,
        events: MoveEvents::default(),
    };
    pm.run();
    pm.p.oldbuttons = cmd.buttons;
    pm.p.duck_time = pm.p.duck_time.trunc();
    *p = pm.p;
    pm.events
}

fn is_water(c: i32) -> bool {
    c <= contents::WATER && c > contents::TRANSLUCENT
}

fn clip_velocity(v: Vec3, normal: Vec3, overbounce: f32) -> Vec3 {
    let backoff = v.dot(normal) * overbounce;
    let mut out = v - normal * backoff;
    for c in [&mut out.x, &mut out.y, &mut out.z] {
        if *c > -STOP_EPSILON && *c < STOP_EPSILON {
            *c = 0.0;
        }
    }
    out
}

fn flat_unit(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.y, 0.0).normalize_or_zero()
}

struct Pm<'a> {
    world: &'a mut dyn MoveWorld,
    phys: &'a Physics,
    p: Player,
    cmd: Cmd,
    frametime: f32,
    maxspeed: f32,
    forward: Vec3,
    right: Vec3,
    events: MoveEvents,
}

impl Pm<'_> {
    /// A player trace: one that starts in solid does not move at all (`PM_PlayerTrace`).
    fn trace(&mut self, start: Vec3, end: Vec3) -> Trace {
        let mut tr = self.world.trace(&TraceQuery::hull(start, end, self.p.hull()));
        if tr.all_solid {
            tr.start_solid = true;
        }
        if tr.start_solid {
            tr.fraction = 0.0;
            tr.end = start;
        }
        tr
    }

    /// An entity the move ran into or stands on gets touched (`PM_AddToTouched`).
    fn touch(&mut self, tr: &Trace) {
        if let Some(hit) = tr.hit.filter(|h| *h != 0)
            && !self.events.touched.contains(&hit)
        {
            self.events.touched.push(hit);
        }
    }

    fn run(&mut self) {
        self.check_parameters();
        if self.p.duck_time > 0.0 {
            self.p.duck_time = (self.p.duck_time - f32::from(self.cmd.msec)).max(0.0);
        }
        let (forward, right, _) = view_angle_vectors(self.cmd.angles);
        self.forward = forward;
        self.right = right;

        self.categorize();
        if !self.p.on_ground() {
            self.p.fall_velocity = -self.p.velocity.z;
        }
        let ladder = if self.p.dead {
            None
        } else {
            self.world.ladder(self.p.origin, self.p.hull())
        };
        self.duck();
        if let Some(l) = ladder {
            self.ladder_move(l);
        } else {
            self.p.on_ladder = false;
        }

        if self.p.on_ladder {
            self.check_water();
            if self.cmd.buttons & IN_JUMP == 0 {
                self.p.oldbuttons &= !IN_JUMP;
            }
            self.fly_move();
            return;
        }

        if self.p.waterlevel <= 1 {
            self.add_correct_gravity();
        }
        if self.p.waterjump_time > 0.0 {
            self.water_jump();
            self.fly_move();
            self.check_water();
            return;
        }
        if self.p.waterlevel >= 2 {
            if self.p.waterlevel == 2 {
                self.check_water_jump();
            }
            if self.p.velocity.z < 0.0 && self.p.waterjump_time > 0.0 {
                self.p.waterjump_time = 0.0;
            }
            if self.cmd.buttons & IN_JUMP != 0 {
                self.jump();
            } else {
                self.p.oldbuttons &= !IN_JUMP;
            }
            self.water_move();
            self.categorize();
            return;
        }
        if self.cmd.buttons & IN_JUMP != 0 {
            if ladder.is_none() {
                self.jump();
            }
        } else {
            self.p.oldbuttons &= !IN_JUMP;
        }
        if self.p.on_ground() {
            self.p.velocity.z = 0.0;
            self.friction();
        }
        self.check_velocity();
        if self.p.on_ground() {
            self.walk_move();
        } else {
            self.air_move();
        }
        self.categorize();
        self.check_velocity();
        if self.p.waterlevel <= 1 {
            self.fixup_gravity();
        }
        if self.p.on_ground() {
            self.p.velocity.z = 0.0;
        }
        self.check_falling();
    }

    fn check_parameters(&mut self) {
        let c = &mut self.cmd;
        let spd = (c.forward * c.forward + c.side * c.side + c.up * c.up).sqrt();
        if self.p.client_maxspeed != 0.0 {
            self.maxspeed = self.maxspeed.min(self.p.client_maxspeed);
        }
        if self.phys.use_slowdown && self.p.on_ground() && c.buttons & IN_USE != 0 {
            self.maxspeed /= 3.0;
        }
        if spd != 0.0 && spd > self.maxspeed {
            let ratio = self.maxspeed / spd;
            c.forward *= ratio;
            c.side *= ratio;
            c.up *= ratio;
        }
        if self.p.dead {
            c.forward = 0.0;
            c.side = 0.0;
            c.up = 0.0;
        }
    }

    fn check_velocity(&mut self) {
        let max = self.phys.maxvelocity;
        let v = &mut self.p.velocity;
        for c in [&mut v.x, &mut v.y, &mut v.z] {
            if c.is_nan() {
                *c = 0.0;
            }
            *c = c.clamp(-max, max);
        }
    }

    fn add_correct_gravity(&mut self) {
        if self.p.waterjump_time > 0.0 {
            return;
        }
        self.p.velocity.z -= self.phys.gravity * 0.5 * self.frametime;
        self.check_velocity();
    }

    fn fixup_gravity(&mut self) {
        if self.p.waterjump_time > 0.0 {
            return;
        }
        self.p.velocity.z -= self.phys.gravity * self.frametime * 0.5;
        self.check_velocity();
    }

    /// Water level from the contents at the feet, the waist and the eyes.
    fn check_water(&mut self) -> bool {
        let (mins, maxs) = self.p.hull().extents();
        let origin = self.p.origin;
        let mut point = Vec3::new(
            origin.x + (mins.x + maxs.x) * 0.5,
            origin.y + (mins.y + maxs.y) * 0.5,
            origin.z + mins.z + 1.0,
        );
        self.p.waterlevel = 0;
        self.p.watertype = contents::EMPTY;
        let c = self.world.point_contents(point);
        if is_water(c) {
            self.p.watertype = c;
            self.p.waterlevel = 1;
            point.z = origin.z + (mins.z + maxs.z) * 0.5;
            if is_water(self.world.point_contents(point)) {
                self.p.waterlevel = 2;
                point.z = origin.z + self.p.view_z;
                if is_water(self.world.point_contents(point)) {
                    self.p.waterlevel = 3;
                }
            }
        }
        self.p.waterlevel > 1
    }

    /// Water level and ground: on ground when a floor (normal z ≥ 0.7) is within 2 units below and the player is
    /// not shooting up.
    fn categorize(&mut self) {
        self.check_water();
        let origin = self.p.origin;
        if self.p.velocity.z > 180.0 {
            self.p.ground = None;
            return;
        }
        let tr = self.trace(origin, origin - Vec3::Z * 2.0);
        self.p.ground = if tr.normal.z < 0.7 {
            None
        } else {
            Some(tr.hit.unwrap_or(0))
        };
        if self.p.ground.is_some() {
            self.p.waterjump_time = 0.0;
            if self.p.waterlevel < 2 && !tr.start_solid && !tr.all_solid {
                self.p.origin = tr.end;
            }
            self.touch(&tr);
        }
    }

    fn fits(&mut self, origin: Vec3) -> bool {
        !self.trace(origin, origin).start_solid
    }

    fn unduck(&mut self) {
        let offset = HullKind::Crouch.extents().0 - HullKind::Stand.extents().0;
        let mut new_origin = self.p.origin;
        if self.p.on_ground() {
            new_origin += offset;
        }
        if !self.fits(new_origin) {
            return;
        }
        self.p.ducked = false;
        if !self.fits(new_origin) {
            self.p.ducked = true;
            return;
        }
        self.p.ducking = false;
        self.p.view_z = VIEW_STAND;
        self.p.duck_time = 0.0;
        self.p.origin = new_origin;
        self.categorize();
    }

    fn duck(&mut self) {
        let changed = self.p.oldbuttons ^ self.cmd.buttons;
        let pressed = changed & self.cmd.buttons;
        if self.cmd.buttons & IN_DUCK != 0 {
            self.p.oldbuttons |= IN_DUCK;
        } else {
            self.p.oldbuttons &= !IN_DUCK;
        }
        if self.p.dead {
            if self.p.ducked {
                self.unduck();
            }
            return;
        }
        if self.p.ducked {
            self.cmd.forward *= DUCK_MULTIPLIER;
            self.cmd.side *= DUCK_MULTIPLIER;
            self.cmd.up *= DUCK_MULTIPLIER;
        }
        if self.cmd.buttons & IN_DUCK == 0 && !self.p.ducking && !self.p.ducked {
            return;
        }
        if self.cmd.buttons & IN_DUCK == 0 {
            self.unduck();
            return;
        }
        if pressed & IN_DUCK != 0 && !self.p.ducked {
            self.p.duck_time = 1000.0;
            self.p.ducking = true;
        }
        if !self.p.ducking {
            return;
        }
        if self.p.duck_time / 1000.0 <= 1.0 - TIME_TO_DUCK || !self.p.on_ground() {
            self.p.ducked = true;
            self.p.ducking = false;
            self.p.view_z = VIEW_DUCK;
            if self.p.on_ground() {
                // The small hull's centre is 18 units lower: the feet stay on the floor.
                self.p.origin -= HullKind::Crouch.extents().0 - HullKind::Stand.extents().0;
                self.fix_crouch_stuck();
                self.categorize();
            }
        } else {
            let t = (1.0 - self.p.duck_time / 1000.0).max(0.0) / TIME_TO_DUCK;
            let s = 3.0 * t * t - 2.0 * t * t * t;
            let more = HullKind::Crouch.extents().0.z - HullKind::Stand.extents().0.z;
            self.p.view_z = (VIEW_DUCK - more) * s + VIEW_STAND * (1.0 - s);
        }
    }

    fn fix_crouch_stuck(&mut self) {
        if self.fits(self.p.origin) {
            return;
        }
        let start = self.p.origin;
        for _ in 0..36 {
            self.p.origin.z += 1.0;
            if self.fits(self.p.origin) {
                return;
            }
        }
        self.p.origin = start;
    }

    fn ladder_move(&mut self, ladder: Ladder) {
        self.p.on_ladder = true;
        let feet = self.p.origin + Vec3::Z * (self.p.hull().extents().0.z - 1.0);
        let on_floor = self.world.point_contents(feet) == contents::SOLID;
        let Some(normal) = ladder.normal else { return };
        let mut speed = CLIMB_SPEED.min(self.maxspeed);
        if self.p.ducked {
            speed *= DUCK_MULTIPLIER;
        }
        let b = self.cmd.buttons;
        let mut forward = 0.0;
        let mut right = 0.0;
        if b & IN_BACK != 0 {
            forward -= speed;
        }
        if b & IN_FORWARD != 0 {
            forward += speed;
        }
        if b & IN_MOVELEFT != 0 {
            right -= speed;
        }
        if b & IN_MOVERIGHT != 0 {
            right += speed;
        }
        if b & IN_JUMP != 0 {
            self.p.on_ladder = false;
            self.p.velocity = normal * LADDER_JUMP;
            return;
        }
        if forward == 0.0 && right == 0.0 {
            self.p.velocity = Vec3::ZERO;
            return;
        }
        let velocity = self.forward * forward + self.right * right;
        let perp = Vec3::Z.cross(normal).normalize_or_zero();
        let into = velocity.dot(normal);
        let lateral = velocity - normal * into;
        // Moving into the face turns into climbing along it.
        self.p.velocity = lateral - normal.cross(perp) * into;
        if on_floor && into > 0.0 {
            self.p.velocity += normal * CLIMB_SPEED;
        }
    }

    fn water_jump(&mut self) {
        self.p.waterjump_time = self.p.waterjump_time.min(10000.0);
        if self.p.waterjump_time <= 0.0 {
            return;
        }
        self.p.waterjump_time -= f32::from(self.cmd.msec);
        if self.p.waterjump_time < 0.0 || self.p.waterlevel == 0 {
            self.p.waterjump_time = 0.0;
        }
        self.p.velocity.x = self.p.waterjump_dir.x;
        self.p.velocity.y = self.p.waterjump_dir.y;
    }

    fn check_water_jump(&mut self) {
        if self.p.waterjump_time > 0.0 || self.p.velocity.z < -180.0 {
            return;
        }
        let flat_vel = flat_unit(self.p.velocity);
        let flat_fwd = flat_unit(self.forward);
        if flat_vel != Vec3::ZERO && flat_vel.dot(flat_fwd) < 0.0 {
            return;
        }
        let mut start = self.p.origin + Vec3::Z * WATERJUMP_HEIGHT;
        let end = start + flat_fwd * 24.0;
        let hull = self.p.hull();
        let tr = self.world.trace(&TraceQuery::line(start, end));
        if tr.fraction < 1.0 && tr.normal.z.abs() < 0.1 {
            start.z += hull.extents().1.z - WATERJUMP_HEIGHT;
            let end = start + flat_fwd * 24.0;
            self.p.waterjump_dir = -tr.normal * 50.0;
            let tr = self.world.trace(&TraceQuery::line(start, end));
            if tr.fraction == 1.0 {
                self.p.waterjump_time = 2000.0;
                self.p.velocity.z = 225.0;
                self.p.oldbuttons |= IN_JUMP;
            }
        }
    }

    fn prevent_mega_bunny_jumping(&mut self) {
        let max = BUNNYJUMP_MAX_SPEED_FACTOR * self.maxspeed;
        if max <= 0.0 {
            return;
        }
        let spd = self.p.velocity.length();
        if spd > max {
            self.p.velocity *= (max / spd) * 0.65;
        }
    }

    fn jump(&mut self) {
        if self.p.dead {
            self.p.oldbuttons |= IN_JUMP;
            return;
        }
        if self.p.waterjump_time > 0.0 {
            self.p.waterjump_time = (self.p.waterjump_time - f32::from(self.cmd.msec)).max(0.0);
            return;
        }
        if self.p.waterlevel >= 2 {
            self.p.ground = None;
            self.p.velocity.z = match self.p.watertype {
                contents::WATER => 100.0,
                contents::SLIME => 80.0,
                _ => 50.0,
            };
            return;
        }
        if !self.p.on_ground() {
            self.p.oldbuttons |= IN_JUMP;
            return;
        }
        if self.p.oldbuttons & IN_JUMP != 0 {
            return;
        }
        self.p.ground = None;
        if self.phys.bunnyhop_cap {
            self.prevent_mega_bunny_jumping();
        }
        let crouched = self.p.ducking || self.p.ducked;
        if crouched
            && self.p.longjump
            && self.cmd.buttons & IN_DUCK != 0
            && self.p.duck_time > 0.0
            && self.p.velocity.length() > 50.0
        {
            self.p.velocity.x = self.forward.x * LONGJUMP_SPEED;
            self.p.velocity.y = self.forward.y * LONGJUMP_SPEED;
            self.p.velocity.z = LONGJUMP_UP;
            self.events.longjumped = true;
        } else {
            self.p.velocity.z = JUMP_SPEED;
        }
        self.events.jumped = true;
        self.fixup_gravity();
        self.p.oldbuttons |= IN_JUMP;
    }

    fn friction(&mut self) {
        if self.p.waterjump_time > 0.0 {
            return;
        }
        let v = self.p.velocity;
        let speed = v.length();
        if speed < 0.1 {
            return;
        }
        let mut drop = 0.0;
        if self.p.on_ground() {
            // Edge friction: no floor within 34 units under the point 16 units ahead.
            let feet = self.p.origin.z + self.p.hull().extents().0.z;
            let start = Vec3::new(
                self.p.origin.x + v.x / speed * 16.0,
                self.p.origin.y + v.y / speed * 16.0,
                feet,
            );
            let tr = self.trace(start, start - Vec3::Z * 34.0);
            let friction = if tr.fraction == 1.0 {
                self.phys.friction * self.phys.edgefriction
            } else {
                self.phys.friction
            };
            let control = speed.max(self.phys.stopspeed);
            drop += control * friction * self.frametime;
        }
        let newspeed = (speed - drop).max(0.0) / speed;
        self.p.velocity = v * newspeed;
    }

    fn accelerate(&mut self, wishdir: Vec3, wishspeed: f32, accel: f32) {
        if self.p.dead || self.p.waterjump_time > 0.0 {
            return;
        }
        let add = wishspeed - self.p.velocity.dot(wishdir);
        if add <= 0.0 {
            return;
        }
        let amount = (accel * self.frametime * wishspeed).min(add);
        self.p.velocity += wishdir * amount;
    }

    fn air_accelerate(&mut self, wishdir: Vec3, wishspeed: f32, accel: f32) {
        if self.p.dead || self.p.waterjump_time > 0.0 {
            return;
        }
        let add = wishspeed.min(30.0) - self.p.velocity.dot(wishdir);
        if add <= 0.0 {
            return;
        }
        let amount = (accel * wishspeed * self.frametime).min(add);
        self.p.velocity += wishdir * amount;
    }

    /// Horizontal wish direction and speed from the command, capped at maxspeed.
    fn wish(&self) -> (Vec3, f32) {
        let f = flat_unit(self.forward);
        let r = flat_unit(self.right);
        let wishvel = Vec3::new(
            f.x * self.cmd.forward + r.x * self.cmd.side,
            f.y * self.cmd.forward + r.y * self.cmd.side,
            0.0,
        );
        let speed = wishvel.length();
        let dir = if speed > 0.0 { wishvel / speed } else { Vec3::ZERO };
        (dir, speed.min(self.maxspeed))
    }

    fn walk_move(&mut self) {
        let (wishdir, wishspeed) = self.wish();
        self.p.velocity.z = 0.0;
        self.accelerate(wishdir, wishspeed, self.phys.accelerate);
        self.p.velocity.z = 0.0;
        if self.p.velocity.length() < 1.0 {
            self.p.velocity = Vec3::ZERO;
            return;
        }
        let origin = self.p.origin;
        let dest = Vec3::new(
            origin.x + self.p.velocity.x * self.frametime,
            origin.y + self.p.velocity.y * self.frametime,
            origin.z,
        );
        let tr = self.trace(origin, dest);
        if tr.fraction == 1.0 {
            self.p.origin = tr.end;
            return;
        }
        if self.p.waterjump_time > 0.0 {
            return;
        }
        let original = self.p.origin;
        let original_vel = self.p.velocity;
        self.fly_move();
        let down = self.p.origin;
        let down_vel = self.p.velocity;

        // Again from one step up, then back down onto the step.
        self.p.origin = original;
        self.p.velocity = original_vel;
        let tr = self.trace(original, original + Vec3::Z * self.phys.stepsize);
        if !tr.start_solid && !tr.all_solid {
            self.p.origin = tr.end;
        }
        self.fly_move();
        let here = self.p.origin;
        let tr = self.trace(here, here - Vec3::Z * self.phys.stepsize);
        if tr.normal.z < 0.7 {
            self.p.origin = down;
            self.p.velocity = down_vel;
            return;
        }
        if !tr.start_solid && !tr.all_solid {
            self.p.origin = tr.end;
        }
        let up = self.p.origin;
        let flat = |p: Vec3| (p.x - original.x).powi(2) + (p.y - original.y).powi(2);
        if flat(down) > flat(up) {
            self.p.origin = down;
            self.p.velocity = down_vel;
        } else {
            self.p.velocity.z = down_vel.z;
        }
    }

    fn air_move(&mut self) {
        let (wishdir, wishspeed) = self.wish();
        self.air_accelerate(wishdir, wishspeed, self.phys.airaccelerate);
        self.fly_move();
    }

    fn water_move(&mut self) {
        let mut wishvel = self.forward * self.cmd.forward + self.right * self.cmd.side;
        if self.cmd.forward == 0.0 && self.cmd.side == 0.0 && self.cmd.up == 0.0 {
            wishvel.z -= 60.0;
        } else {
            wishvel.z += self.cmd.up;
        }
        let mut wishspeed = wishvel.length();
        if wishspeed > self.maxspeed {
            wishvel *= self.maxspeed / wishspeed;
            wishspeed = self.maxspeed;
        }
        wishspeed *= 0.8;
        let speed = self.p.velocity.length();
        let newspeed = if speed > 0.0 {
            let n = (speed - self.frametime * speed * self.phys.friction).max(0.0);
            self.p.velocity *= n / speed;
            n
        } else {
            0.0
        };
        if wishspeed < 0.1 {
            return;
        }
        let add = wishspeed - newspeed;
        if add > 0.0 {
            let dir = wishvel.normalize_or_zero();
            let amount = (self.phys.accelerate * wishspeed * self.frametime).min(add);
            self.p.velocity += dir * amount;
        }
        // Assume a stair or slope: press down from a step above the destination.
        let dest = self.p.origin + self.p.velocity * self.frametime;
        let start = dest + Vec3::Z * (self.phys.stepsize + 1.0);
        let tr = self.trace(start, dest);
        if !tr.start_solid && !tr.all_solid {
            self.p.origin = tr.end;
            return;
        }
        self.fly_move();
    }

    /// Slides along up to four planes it hits during the frame.
    fn fly_move(&mut self) {
        let mut planes: [Vec3; MAX_CLIP_PLANES] = [Vec3::ZERO; MAX_CLIP_PLANES];
        let mut numplanes = 0;
        let mut original_vel = self.p.velocity;
        let primal_vel = self.p.velocity;
        let mut all_fraction = 0.0;
        let mut time_left = self.frametime;
        for _ in 0..4 {
            if self.p.velocity == Vec3::ZERO {
                break;
            }
            let end = self.p.origin + self.p.velocity * time_left;
            let tr = self.trace(self.p.origin, end);
            all_fraction += tr.fraction;
            if tr.all_solid {
                self.p.velocity = Vec3::ZERO;
                return;
            }
            if tr.fraction > 0.0 {
                self.p.origin = tr.end;
                original_vel = self.p.velocity;
                numplanes = 0;
            }
            if tr.fraction == 1.0 {
                break;
            }
            self.touch(&tr);
            time_left -= time_left * tr.fraction;
            if numplanes >= MAX_CLIP_PLANES {
                self.p.velocity = Vec3::ZERO;
                break;
            }
            planes[numplanes] = tr.normal;
            numplanes += 1;
            if numplanes == 1 && !self.p.on_ladder && !self.p.on_ground() {
                // Player friction is always 1, so walls and floors both reflect with an overbounce of 1.
                let v = clip_velocity(original_vel, planes[0], 1.0);
                self.p.velocity = v;
                original_vel = v;
            } else {
                let mut i = 0;
                while i < numplanes {
                    self.p.velocity = clip_velocity(original_vel, planes[i], 1.0);
                    let against = (0..numplanes).any(|j| j != i && self.p.velocity.dot(planes[j]) < 0.0);
                    if !against {
                        break;
                    }
                    i += 1;
                }
                if i == numplanes {
                    if numplanes != 2 {
                        self.p.velocity = Vec3::ZERO;
                        break;
                    }
                    // Along the crease of two planes.
                    let dir = planes[0].cross(planes[1]);
                    self.p.velocity = dir * dir.dot(self.p.velocity);
                }
                if self.p.velocity.dot(primal_vel) <= 0.0 {
                    self.p.velocity = Vec3::ZERO;
                    break;
                }
            }
        }
        if all_fraction == 0.0 {
            self.p.velocity = Vec3::ZERO;
        }
    }

    fn check_falling(&mut self) {
        if self.p.on_ground() {
            if self.p.fall_velocity > 0.0 && !self.p.dead {
                self.events.landed = Some(self.p.fall_velocity);
            }
            self.p.fall_velocity = 0.0;
        }
    }
}
