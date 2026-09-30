//! Traversal contracts of special links (design v2 §9.1): where a link starts and ends, what the bot does on the
//! way, what must hold before it commits, how long it may take and what it costs. Executors (`exec`) carry them
//! out; the importer fills them from checks against the map.

use lb_core::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stance {
    Stand,
    Crouch,
    Ladder,
    Swim,
}

/// A place a traversal starts or ends at.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    /// Player origin (hull centre) there.
    pub origin: Vec3,
    /// How close counts as there, units.
    pub radius: f32,
    pub stance: Stance,
}

/// A brush entity the traversal depends on, with its positions (offsets from where its model was compiled).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MechRef {
    /// Brush model `*model`.
    pub model: u16,
    pub rest: Vec3,
    pub active: Vec3,
    /// Seconds to move between the two.
    pub travel: f32,
    /// Seconds it stays activated before it returns; negative = until set off again.
    pub wait: f32,
}

/// How a mechanism is set off.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Interaction {
    /// Walking into it (touch doors) or standing in its field (platforms, touch plates).
    Touch { model: u16, spot: Vec3 },
    /// The use key at `spot` (player origin), looking at `aim`.
    Use { model: u16, spot: Vec3, aim: Vec3 },
    /// Shooting it at `aim` from `spot`.
    Shoot { model: u16, spot: Vec3, aim: Vec3 },
}

impl Interaction {
    pub fn model(&self) -> u16 {
        match *self {
            Interaction::Touch { model, .. } | Interaction::Use { model, .. } | Interaction::Shoot { model, .. } => {
                model
            }
        }
    }

    pub fn spot(&self) -> Vec3 {
        match *self {
            Interaction::Touch { spot, .. } | Interaction::Use { spot, .. } | Interaction::Shoot { spot, .. } => spot,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// Run at the landing and jump from the entry; the least run-up speed, ducking in the air for high ledges.
    Jump {
        speed: f32,
        duck: bool,
        robustness: f32,
    },
    /// Walk off the edge at `speed`; `damage` is what the landing costs.
    Drop {
        speed: f32,
        damage: f32,
    },
    /// Climb; `normal` points out of the ladder face, `mount` is where a player stands on it at the entry height.
    Ladder {
        normal: Vec3,
        up: bool,
        mount: Vec3,
    },
    Swim,
    /// Pass a door that `open` sets off (the door itself for touch and use doors), through `via` when the way is
    /// not straight (the middle of a doorway the leaf slides across).
    Door {
        door: MechRef,
        open: Interaction,
        via: Option<Vec3>,
    },
    /// Ride a platform from the entry (on the platform at rest) to the exit (at the platform's other end).
    Lift {
        platform: MechRef,
        start: Interaction,
    },
    /// Walk into the trigger at `touch`; the player appears at `dest`.
    Teleport {
        trigger: u16,
        touch: Vec3,
        dest: Vec3,
    },
    /// Break the brush at `aim` first.
    Breakable {
        model: u16,
        aim: Vec3,
        health: f32,
        crowbar: bool,
    },
    /// From rest at the entry run along `dir` into the push field `trigger` (jumping `jump_at` units along the run),
    /// keep over `hold` while it lifts, then steer the flight onto the exit (`lb_kin::validate::simulate_push`).
    Push {
        trigger: u16,
        dir: Vec2,
        jump_at: Option<f32>,
        hold: Option<Vec2>,
    },
    /// Run through the entry and long jump from it looking at the exit, holding duck and steering onto the exit
    /// in the air.
    LongJump {
        robustness: f32,
    },
    /// Stop at the entry, look back from the exit `pitch` degrees down, charge the gauss for a recoil of `push`
    /// units/s, jump and let the charge go; the recoil throws the bot toward the exit, steering onto it in the air.
    GaussBoost {
        pitch: f32,
        robustness: f32,
        push: f32,
    },
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Action::Jump { .. } => "jump",
            Action::Drop { .. } => "drop",
            Action::Ladder { .. } => "ladder",
            Action::Swim => "swim",
            Action::Door { .. } => "door",
            Action::Lift { .. } => "lift",
            Action::Teleport { .. } => "teleport",
            Action::Breakable { .. } => "breakable",
            Action::Push { .. } => "push",
            Action::LongJump { .. } => "longjump",
            Action::GaussBoost { .. } => "gauss_boost",
        }
    }

    /// The brush entity the traversal waits on or acts upon.
    pub fn mechanism(&self) -> Option<u16> {
        match *self {
            Action::Door { door, .. } => Some(door.model),
            Action::Lift { platform, .. } => Some(platform.model),
            Action::Teleport { trigger, .. } => Some(trigger),
            Action::Breakable { model, .. } => Some(model),
            Action::Push { trigger, .. } => Some(trigger),
            _ => None,
        }
    }
}

/// What must hold before committing to the traversal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Needs {
    /// Health above this (falls).
    pub health: f32,
    pub longjump: bool,
    /// A gauss and a full charge's uranium.
    pub gauss: bool,
}

/// Expected cost, seconds and points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    /// Moving through it.
    pub time: f32,
    /// Waiting for a mechanism, on average.
    pub wait: f32,
    /// Health lost.
    pub damage: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TraversalSpec {
    pub entry: Anchor,
    pub exit: Anchor,
    pub action: Action,
    pub needs: Needs,
    /// The whole traversal must be done within this many seconds of starting it.
    pub deadline: f32,
    pub cost: Cost,
}

impl TraversalSpec {
    /// Horizontal direction from entry to exit.
    pub fn dir(&self) -> Vec2 {
        (self.exit.origin - self.entry.origin).truncate().normalize_or_zero()
    }

    /// The fastest a bot should arrive at the entry: faster, it runs past a jump's takeoff or off a drop's edge
    /// before the traversal can slow it down. `None` when any speed will do.
    pub fn entry_speed(&self) -> Option<f32> {
        match self.action {
            Action::Jump { speed, .. } => Some(speed.max(120.0)),
            Action::Drop { speed, .. } => Some(speed),
            Action::Ladder { .. } => Some(150.0),
            // The run into the field starts from rest; a boost stands still at its takeoff.
            Action::Push { .. } | Action::GaussBoost { .. } => Some(100.0),
            _ => None,
        }
    }
}
