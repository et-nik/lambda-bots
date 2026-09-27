//! Intents, channel arbiter, look/locomotion/stance/weapon controllers, command encoder.
//!
//! Behavior asks for what it wants through intents on four channels (look, movement, stance, weapon), each with a
//! priority; the highest priority on a channel wins and the first request wins a tie. The motor then turns the
//! winners into one user command: the look controller moves the view, movement is projected on the new yaw,
//! stance and weapon controllers press their buttons with correct edges, and the encoder adds direction buttons.

#![forbid(unsafe_code)]

pub mod look;
pub mod weapon;

use lb_core::math::{dir_to_view_angles, world_vel_to_move};
use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3};
use lb_game::input::{IN_DUCK, IN_JUMP, direction_buttons};
use lb_game::weapons::WeaponId;
use smallvec::SmallVec;

pub use look::{LookController, LookGoal, LookParams};
pub use weapon::{Fire, WeaponController, WeaponIntent};

/// Channel priorities (design §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Prio {
    /// Glances and looking around.
    Optional = 20,
    /// The current goal.
    Goal = 50,
    /// Fighting and reacting to threats.
    Threat = 70,
    /// Weapon protocols that must finish.
    Protocol = 85,
    /// A traversal nav cannot interrupt (jumps, ladders).
    Traversal = 90,
    Lifecycle = 100,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LookIntent {
    /// Look at a point; `engaged` marks aiming at an enemy.
    Point { at: Vec3, engaged: bool },
    /// Hold view angles (ladders).
    Angles(Vec3),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveIntent {
    /// World direction on the ground plane (need not be normalized; zero = stop).
    pub dir: Vec2,
    pub speed: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StanceIntent {
    pub jump: bool,
    pub duck: bool,
}

/// This frame's requests, one winner per channel.
#[derive(Clone, Debug, Default)]
pub struct Intents {
    pub look: Option<(Prio, LookIntent)>,
    pub movement: Option<(Prio, MoveIntent)>,
    pub stance: Option<(Prio, StanceIntent)>,
    pub weapon: Option<(Prio, WeaponIntent)>,
}

fn offer<T>(slot: &mut Option<(Prio, T)>, prio: Prio, intent: T) {
    if slot.as_ref().is_none_or(|(held, _)| prio > *held) {
        *slot = Some((prio, intent));
    }
}

impl Intents {
    pub fn clear(&mut self) {
        *self = Intents::default();
    }

    pub fn look(&mut self, prio: Prio, intent: LookIntent) {
        offer(&mut self.look, prio, intent);
    }

    pub fn movement(&mut self, prio: Prio, intent: MoveIntent) {
        offer(&mut self.movement, prio, intent);
    }

    pub fn stance(&mut self, prio: Prio, intent: StanceIntent) {
        offer(&mut self.stance, prio, intent);
    }

    pub fn weapon(&mut self, prio: Prio, intent: WeaponIntent) {
        offer(&mut self.weapon, prio, intent);
    }
}

/// The bot's own state the motor needs.
#[derive(Clone, Copy, Debug)]
pub struct MotorInput {
    pub now: SimTime,
    pub dt: f32,
    pub eye: Vec3,
    pub velocity: Vec3,
    pub maxspeed: f32,
    pub on_ladder: bool,
    /// Weapon confirmed by `CurWeapon`.
    pub weapon: Option<WeaponId>,
}

/// One user command's worth of output.
#[derive(Clone, Debug, Default)]
pub struct MotorOut {
    pub angles: Vec3,
    pub forward: f32,
    pub side: f32,
    pub buttons: u16,
    pub commands: SmallVec<[String; 2]>,
}

#[derive(Clone, Debug, Default)]
pub struct Motor {
    /// The view the bot sends with its commands; also what its eyes see.
    pub view: Vec3,
    pub look: LookController,
    pub weapon: WeaponController,
    /// Buttons of the last command that actually went out.
    last_sent: u16,
}

impl Motor {
    pub fn reset(&mut self) {
        self.look.reset();
        self.weapon.reset();
        self.last_sent = 0;
    }

    /// The engine turned the view (spawn, teleport): take it as it is.
    pub fn set_view(&mut self, angles: Vec3) {
        self.view = Vec3::new(angles.x, angles.y, 0.0);
        self.look.reset();
    }

    /// Feedback from the command driver: the buttons that were sent.
    pub fn sent(&mut self, buttons: u16) {
        self.last_sent = buttons;
    }

    pub fn run(&mut self, intents: &Intents, input: &MotorInput, params: &LookParams, rng: &mut Pcg32) -> MotorOut {
        let mut out = MotorOut::default();
        if let Some((_, look)) = intents.look {
            let goal = match look {
                LookIntent::Point { at, engaged } => LookGoal {
                    angles: dir_to_view_angles(at - input.eye),
                    engaged,
                },
                LookIntent::Angles(angles) => LookGoal { angles, engaged: false },
            };
            let moving = input.velocity.length() > 1.0;
            self.look.update(
                &mut self.view,
                &goal,
                input.dt,
                params,
                input.now,
                moving,
                self.weapon.fired_at,
                rng,
            );
        }
        out.angles = self.view;
        if let Some((_, m)) = intents.movement {
            let dir = m.dir.normalize_or_zero();
            let speed = m.speed.clamp(0.0, input.maxspeed.max(1.0));
            let (forward, side) = world_vel_to_move(dir.extend(0.0) * speed, self.view.y);
            out.forward = forward;
            out.side = side;
        }
        if let Some((_, s)) = intents.stance {
            // A jump needs a fresh press, and on a ladder it would let go.
            if s.jump && self.last_sent & IN_JUMP == 0 && !input.on_ladder {
                out.buttons |= IN_JUMP;
            }
            if s.duck {
                out.buttons |= IN_DUCK;
            }
        }
        let weapon = intents.weapon.as_ref().map(|(_, w)| w);
        out.buttons |= self.weapon.update(input.now, input.weapon, weapon, &mut out.commands);
        out.buttons |= direction_buttons(out.forward, out.side);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_config::skill::AimModel;

    #[test]
    fn higher_priority_wins_and_ties_keep_the_first() {
        let mut i = Intents::default();
        let a = LookIntent::Angles(Vec3::X);
        let b = LookIntent::Angles(Vec3::Y);
        i.look(Prio::Goal, a);
        i.look(Prio::Goal, b);
        assert_eq!(i.look, Some((Prio::Goal, a)));
        i.look(Prio::Threat, b);
        i.look(Prio::Optional, a);
        assert_eq!(i.look, Some((Prio::Threat, b)));
    }

    #[test]
    fn movement_follows_the_new_view_and_jumps_need_a_fresh_press() {
        let mut m = Motor::default();
        let mut intents = Intents::default();
        intents.look(Prio::Goal, LookIntent::Angles(Vec3::new(0.0, 90.0, 0.0)));
        intents.movement(
            Prio::Goal,
            MoveIntent {
                dir: Vec2::Y,
                speed: 300.0,
            },
        );
        intents.stance(
            Prio::Goal,
            StanceIntent {
                jump: true,
                duck: false,
            },
        );
        let input = MotorInput {
            now: SimTime(0.0),
            dt: 1.0,
            eye: Vec3::ZERO,
            velocity: Vec3::ZERO,
            maxspeed: 300.0,
            on_ladder: false,
            weapon: None,
        };
        let params = LookParams {
            model: AimModel::Spring,
            turn_speed: 900.0,
            skill: 50,
        };
        let mut rng = Pcg32::new(1, 1);
        let turn = MotorInput { dt: 0.1, ..input };
        for _ in 0..10 {
            m.run(
                &Intents {
                    stance: None,
                    ..intents.clone()
                },
                &turn,
                &params,
                &mut rng,
            );
        }
        let out = m.run(&intents, &input, &params, &mut rng);
        assert!((out.angles.y - 90.0).abs() < 1.0, "{}", out.angles);
        assert!((out.forward - 300.0).abs() < 5.0 && out.side.abs() < 5.0, "{out:?}");
        assert_ne!(out.buttons & IN_JUMP, 0);
        m.sent(out.buttons);
        let again = m.run(&intents, &input, &params, &mut rng);
        assert_eq!(again.buttons & IN_JUMP, 0, "held jump is not a new press");
    }
}
