//! `lb test motor`: scripted inputs that measure how the engine executes bot commands
//! (run speed, jump apex, command time drift) at a given server fps and command rate.

use lb_core::Vec3;
use lb_core::time::SimTime;

use crate::buttons::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Script {
    Run { secs: f32 },
    Strafe { secs: f32 },
    Jump { count: u32 },
    DuckJump { count: u32 },
    Spin { secs: f32 },
}

impl Script {
    pub fn parse(args: &[&str]) -> Option<Script> {
        let num = |i: usize, d: f32| args.get(i).and_then(|s| s.parse::<f32>().ok()).unwrap_or(d);
        Some(match *args.first()? {
            "run" => Script::Run { secs: num(1, 4.0) },
            "strafe" => Script::Strafe { secs: num(1, 4.0) },
            "jump" => Script::Jump {
                count: num(1, 3.0) as u32,
            },
            "duckjump" => Script::DuckJump {
                count: num(1, 3.0) as u32,
            },
            "spin" => Script::Spin { secs: num(1, 2.0) },
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct MotorTest {
    pub script: Script,
    pub started: SimTime,
    pub start_origin: Vec3,
    pub ground_z: f32,
    pub max_speed2d: f32,
    pub speed_samples: Vec<f32>,
    pub progress: f32,
    pub apexes: Vec<f32>,
    pub cur_apex: f32,
    pub airborne: bool,
    pub jumps_done: u32,
    pub next_jump_at: SimTime,
    pub msec_sent: u64,
    pub commands: u64,
    pub frame_ms_sum: f64,
    pub frames: u64,
    pub yaw: f32,
    pub finished: bool,
}

pub struct MotorOutput {
    pub forward: f32,
    pub side: f32,
    pub buttons: u16,
    pub yaw_delta: f32,
    pub done: bool,
}

impl MotorTest {
    pub fn new(script: Script, now: SimTime, origin: Vec3, yaw: f32) -> MotorTest {
        MotorTest {
            script,
            started: now,
            start_origin: origin,
            ground_z: origin.z,
            max_speed2d: 0.0,
            speed_samples: Vec::new(),
            progress: 0.0,
            apexes: Vec::new(),
            cur_apex: 0.0,
            airborne: false,
            jumps_done: 0,
            next_jump_at: now + 0.5,
            msec_sent: 0,
            commands: 0,
            frame_ms_sum: 0.0,
            frames: 0,
            yaw,
            finished: false,
        }
    }

    /// Produces this frame's inputs and records measurements.
    pub fn step(
        &mut self,
        now: SimTime,
        dt: f32,
        origin: Vec3,
        velocity: Vec3,
        on_ground: bool,
        maxspeed: f32,
    ) -> MotorOutput {
        let elapsed = now.since(self.started) as f32;
        let speed2d = velocity.truncate().length();
        self.max_speed2d = self.max_speed2d.max(speed2d);
        let mut out = MotorOutput {
            forward: 0.0,
            side: 0.0,
            buttons: 0,
            yaw_delta: 0.0,
            done: false,
        };
        match self.script {
            Script::Run { secs } | Script::Strafe { secs } => {
                let speed = if maxspeed > 0.0 { maxspeed } else { 320.0 };
                if matches!(self.script, Script::Run { .. }) {
                    out.forward = speed;
                } else {
                    out.side = speed;
                }
                if (0.3..=0.8).contains(&elapsed) {
                    self.speed_samples.push(speed2d);
                }
                self.progress = (origin - self.start_origin).truncate().length();
                out.done = elapsed >= secs;
            }
            Script::Jump { count } | Script::DuckJump { count } => {
                if on_ground {
                    if self.airborne {
                        self.apexes.push(self.cur_apex);
                        self.airborne = false;
                        self.jumps_done += 1;
                        self.next_jump_at = now + 0.6;
                    }
                    self.ground_z = origin.z;
                    if self.jumps_done < count && now >= self.next_jump_at {
                        out.buttons |= IN_JUMP;
                    }
                } else {
                    if !self.airborne {
                        self.airborne = true;
                        self.cur_apex = 0.0;
                    }
                    self.cur_apex = self.cur_apex.max(origin.z - self.ground_z);
                    if matches!(self.script, Script::DuckJump { .. }) {
                        out.buttons |= IN_DUCK;
                    }
                }
                out.done = self.jumps_done >= count || elapsed > 10.0 + count as f32 * 2.0;
            }
            Script::Spin { secs } => {
                out.yaw_delta = 360.0 / secs * dt;
                out.done = elapsed >= secs;
            }
        }
        out
    }

    /// Frame time the test ran for minus msec sent: bounded by one command quantum when nothing is lost.
    pub fn drift_ms(&self) -> f64 {
        self.frame_ms_sum - self.msec_sent as f64
    }

    /// Median horizontal speed 0.3–0.8 s after the start, when acceleration has finished.
    pub fn steady_speed(&self) -> f32 {
        if self.speed_samples.is_empty() {
            return 0.0;
        }
        let mut v = self.speed_samples.clone();
        v.sort_by(|a, b| a.total_cmp(b));
        v[v.len() / 2]
    }

    pub fn report(&self, bot: &str, now: SimTime, cmd_rate: f32) -> String {
        let elapsed = now.since(self.started);
        let fps = if self.frame_ms_sum > 0.0 {
            self.frames as f64 * 1000.0 / self.frame_ms_sum
        } else {
            0.0
        };
        let avg_msec = if self.commands > 0 {
            self.msec_sent as f64 / self.commands as f64
        } else {
            0.0
        };
        let apex = if self.apexes.is_empty() {
            "-".to_string()
        } else {
            self.apexes
                .iter()
                .map(|a| format!("{a:.1}"))
                .collect::<Vec<_>>()
                .join("/")
        };
        format!(
            "motor test {:?} bot={bot} fps={fps:.0} cmd_rate={cmd_rate} elapsed={elapsed:.2}s max_speed2d={:.1} \
             steady_speed2d={:.1} progress={:.0} apex={apex} cmds={} avg_msec={avg_msec:.2} msec_sent={} \
             frame_ms={:.1} drift_ms={:.2}",
            self.script,
            self.max_speed2d,
            self.steady_speed(),
            self.progress,
            self.commands,
            self.msec_sent,
            self.frame_ms_sum,
            self.drift_ms()
        )
    }
}
