//! How the bots of a recording looked while running: how often steeply up or down, how fast the view turned, how
//! often it snapped round, and how far it strayed from the way they ran; for all running, and for running out of a
//! fight (no shot for `CALM` seconds). Ladders and water are left out, where the view steers the move.
//!
//! Usage: cargo run --release -p lb-runtime --example look_stats -- <recording.lbrec>...

use std::path::Path;

use lb_core::math::angle_diff;
use lb_ffi::{LbBotCommand, LbMoveFeedback};
use lb_host::record::{HostCall, from_pod};
use lb_runtime::record::{Entry, Reader, Record};

/// Running: faster than this across.
const RUNNING: f32 = 100.0;
const STEEP: f32 = 20.0;
/// A turn of more than `SNAP_ANGLE` within `SNAP_WINDOW` seconds is a snap.
const SNAP_WINDOW: f64 = 0.25;
const SNAP_ANGLE: f32 = 45.0;
const MOVETYPE_FLY: u8 = 5;
const IN_ATTACK: u16 = 1;
/// Seconds after the last shot that still count as fighting.
const CALM: f64 = 2.0;

#[derive(Default)]
struct Bot {
    last_yaw: Option<f32>,
    window: Option<(f64, f32)>,
    fired_at: f64,
}

#[derive(Default)]
struct Totals {
    running: f64,
    steep: f64,
    turned: f64,
    snaps: u32,
    astray: f64,
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: look_stats <recording.lbrec>...");
        std::process::exit(2);
    }
    for file in files {
        match stats(Path::new(&file)) {
            Ok([all, calm]) => {
                println!("{file}:");
                for (what, t) in [("running", all), ("running out of a fight", calm)] {
                    println!(
                        "  {what} {:.0} s: looking steeply {:.1}% of it, view turning {:.0} deg/s, {:.1} snaps a \
                         minute, {:.0} deg off the way run on average",
                        t.running,
                        t.steep * 100.0 / t.running.max(1e-9),
                        t.turned / t.running.max(1e-9),
                        f64::from(t.snaps) * 60.0 / t.running.max(1e-9),
                        t.astray / t.running.max(1e-9)
                    );
                }
            }
            Err(e) => eprintln!("{file}: {e}"),
        }
    }
}

impl Totals {
    fn add(&mut self, dt: f64, pitch: f32, turn: Option<f32>, astray: f32, snap: bool) {
        self.running += dt;
        if pitch.abs() > STEEP {
            self.steep += dt;
        }
        self.turned += f64::from(turn.unwrap_or(0.0));
        self.astray += f64::from(astray) * dt;
        self.snaps += u32::from(snap);
    }
}

fn stats(path: &Path) -> Result<[Totals; 2], String> {
    let mut reader = Reader::open(path)?;
    let mut bots: Vec<Bot> = (0..64).map(|_| Bot::default()).collect();
    let (mut all, mut calm) = (Totals::default(), Totals::default());
    let mut now = 0.0f64;
    while let Some(record) = reader.next_record()? {
        let Record::Step(step) = record else { continue };
        if let Entry::FramePre(frame) = &step.entry
            && let Some(h) = frame.header()
        {
            now = h.sim_time;
        }
        for call in &step.calls {
            let HostCall::RunPlayerMoves { cmds, feedback, .. } = call else {
                continue;
            };
            let cmds: Vec<LbBotCommand> = from_pod(cmds);
            let feedback: Vec<LbMoveFeedback> = from_pod(feedback);
            for cmd in &cmds {
                let Some(fb) = feedback.iter().find(|f| f.slot == cmd.slot) else {
                    continue;
                };
                let bot = &mut bots[cmd.slot as usize % 64];
                if cmd.buttons & IN_ATTACK != 0 {
                    bot.fired_at = now;
                }
                let (pitch, yaw) = (cmd.view_angles.x, cmd.view_angles.y);
                let v = (fb.velocity.x, fb.velocity.y);
                let speed = (v.0 * v.0 + v.1 * v.1).sqrt();
                let running = fb.deadflag == 0 && speed > RUNNING && fb.movetype != MOVETYPE_FLY && fb.waterlevel < 2;
                if !running {
                    bot.last_yaw = None;
                    bot.window = None;
                    continue;
                }
                let dt = f64::from(cmd.msec) / 1000.0;
                let turn = bot.last_yaw.map(|last| angle_diff(yaw, last).abs());
                bot.last_yaw = Some(yaw);
                let heading = lb_core::dmath::atan2(v.1, v.0).to_degrees();
                let astray = angle_diff(yaw, heading).abs();
                let mut snap = false;
                match bot.window {
                    Some((since, from)) if now - since >= SNAP_WINDOW => {
                        snap = angle_diff(yaw, from).abs() > SNAP_ANGLE;
                        bot.window = Some((now, yaw));
                    }
                    Some(_) => {}
                    None => bot.window = Some((now, yaw)),
                }
                all.add(dt, pitch, turn, astray, snap);
                if now - bot.fired_at > CALM {
                    calm.add(dt, pitch, turn, astray, snap);
                }
            }
        }
    }
    Ok([all, calm])
}
