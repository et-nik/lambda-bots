//! Footsteps for engines without the ReHLDS `SV_StartSound` hook. `pm_shared` plays step sounds from the engine
//! side, where Metamod cannot see them, but every step (heard or not) flips the player's `iStepLeft`. A flip
//! becomes a step sound under the multiplayer rule of `PM_PlayStepSound`: `mp_footsteps` on, and the player on a
//! ladder or moving faster than 220 units per second horizontally.

use lb_game::sounds::{ATTN_NORM, SoundClass, SoundKind};
use lb_raw::{ClientState, RawClient};

use crate::hearing::SoundEvent;
use lb_core::time::SimTime;

const MP_STEP_SPEED: f32 = 220.0;
const FL_ONGROUND: u32 = 1 << 9;
const FL_DUCKING: u32 = 1 << 14;
const MOVETYPE_FLY: u8 = 5;

#[derive(Clone, Debug, Default)]
pub struct StepSynth {
    last: Vec<Option<(i32, u8)>>,
}

impl StepSynth {
    pub fn reset(&mut self) {
        self.last.clear();
    }

    /// Appends the steps taken since the previous frame.
    pub fn frame(&mut self, clients: &[RawClient], footsteps: bool, now: SimTime, out: &mut Vec<SoundEvent>) {
        for c in clients {
            let i = c.slot as usize;
            if self.last.len() <= i {
                self.last.resize(i + 1, None);
            }
            if c.state != ClientState::Spawned || c.deadflag != 0 {
                self.last[i] = None;
                continue;
            }
            let previous = self.last[i].replace((c.userid, c.step_left));
            let Some((userid, step)) = previous else { continue };
            if userid != c.userid || step == c.step_left || !footsteps {
                continue;
            }
            let ladder = c.movetype == MOVETYPE_FLY;
            if !ladder && c.velocity.truncate().length() <= MP_STEP_SPEED {
                continue;
            }
            let jump = c.flags & FL_ONGROUND == 0 && c.velocity.z > 0.0;
            let mut volume = if ladder {
                0.35
            } else if jump {
                1.0
            } else if c.waterlevel >= 2 {
                0.65
            } else {
                0.5
            };
            if c.flags & FL_DUCKING != 0 {
                volume *= 0.35;
            }
            out.push(SoundEvent {
                t: now,
                source: Some(c.slot),
                origin: c.origin,
                class: SoundClass {
                    kind: if jump { SoundKind::Jump } else { SoundKind::Step },
                    weapon: None,
                },
                volume,
                attenuation: ATTN_NORM,
                global: false,
            });
        }
    }
}
