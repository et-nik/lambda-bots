//! `lb selftest`: checks on the live server the weapon rules the bots take from the game DLL's profile
//! (`lb_game::dll`): which button throws a satchel and which sets it off, that the crossbow zooms, and how fast a hand
//! grenade leaves the hand. One bot does it, standing still and facing the most open way; it is given the weapons,
//! so the server needs `sv_cheats 1`. The result goes to the console and the log, and a profile the server
//! contradicts is corrected for the session.

use lb_core::Vec3;
use lb_core::time::SimTime;
use lb_game::dll::{DllKind, DllProfile};
use lb_game::entities::ProjectileKind;
use lb_game::input::{IN_ATTACK, IN_ATTACK2};
use lb_game::mechanics::Attack;
use lb_game::self_state::SelfState;
use lb_game::weapons::WeaponId;
use lb_perception::projectiles::ProjectileEntity;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    Give,
    SatchelDraw,
    /// Running on toward the open side, to throw the satchel far.
    SatchelRun,
    SatchelThrow,
    /// Standing still until the satchel is at rest, far enough to be spared its blast.
    SatchelWait,
    /// A second satchel was thrown close by: backing off the way the bot came before trying the other button.
    SatchelBackOff,
    SatchelDetonate { tried_other: bool },
    CrossbowDraw,
    CrossbowZoom,
    CrossbowUnzoom,
    GrenadeDraw,
    GrenadePin,
    GrenadeRelease,
    Done,
}

/// What the test asks of the bot this frame.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub buttons: u16,
    pub forward: f32,
    pub commands: Vec<Vec<String>>,
    /// View angles to hold.
    pub view: Vec3,
    /// Results found on this frame.
    pub lines: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SelfTest {
    step: Step,
    since: SimTime,
    yaw: f32,
    dll: DllProfile,
    satchels: i32,
    pressed: bool,
    seen: Vec<u16>,
    pub lines: Vec<String>,
    /// The profile the server turned out to have; `None` when the test did not get that far.
    pub verdict: Option<DllProfile>,
    pub finished: bool,
    satchel_ok: Option<bool>,
    grenade_speed: Option<f32>,
}

const SETTLE: f64 = 1.3;
/// The run before the throw, and how high the satchel is thrown: it lands some 400 units ahead, out of its blast.
const RUN_UP: f64 = 0.8;
const SATCHEL_PITCH: f32 = -45.0;

fn give(names: &[&str]) -> Vec<Vec<String>> {
    names
        .iter()
        .map(|n| vec!["give".to_string(), (*n).to_string()])
        .collect()
}

fn button(a: Attack) -> u16 {
    match a {
        Attack::Primary => IN_ATTACK,
        Attack::Secondary => IN_ATTACK2,
    }
}

fn name(a: Attack) -> &'static str {
    match a {
        Attack::Primary => "primary",
        Attack::Secondary => "secondary",
    }
}

impl SelfTest {
    /// A test facing `yaw` against the profile `dll`.
    pub fn new(now: SimTime, yaw: f32, dll: DllProfile) -> SelfTest {
        SelfTest {
            step: Step::Give,
            since: now,
            yaw,
            dll,
            satchels: 0,
            pressed: false,
            seen: Vec::new(),
            lines: vec![format!("self-test of the weapon rules, profile {}", dll.kind.as_str())],
            verdict: None,
            finished: false,
            satchel_ok: None,
            grenade_speed: None,
        }
    }

    fn next(&mut self, step: Step, now: SimTime) {
        self.step = step;
        self.since = now;
        self.pressed = false;
    }

    fn fail(&mut self, what: &str, now: SimTime) {
        self.lines.push(format!(
            "  {what}: no answer from the game (is sv_cheats 1?); test stopped"
        ));
        self.next(Step::Done, now);
    }

    /// One frame of the test: `projectiles` are the entities in the world now.
    pub fn step(&mut self, now: SimTime, s: &SelfState, projectiles: &[ProjectileEntity], slot: u8) -> Frame {
        let mut f = Frame {
            view: Vec3::new(0.0, self.yaw, 0.0),
            ..Frame::default()
        };
        let waited = now.since(self.since);
        let current = s.current_weapon.get();
        let predicted = |w: WeaponId| s.predicted(w);
        let count = |w: WeaponId| {
            s.prediction
                .map_or(0, |p| if p.current == Some(w) { p.primary_ammo } else { 0 })
        };
        match self.step {
            Step::Give => {
                if !self.pressed {
                    f.commands = give(&[
                        "item_healthkit",
                        "item_healthkit",
                        "item_battery",
                        "weapon_satchel",
                        "weapon_satchel",
                        "weapon_crossbow",
                        "weapon_handgrenade",
                    ]);
                    self.pressed = true;
                }
                if waited >= 1.0 {
                    self.next(Step::SatchelDraw, now);
                }
            }
            Step::SatchelDraw => {
                if current != Some(WeaponId::Satchel) {
                    if !self.pressed {
                        f.commands.push(vec![WeaponId::Satchel.classname().into()]);
                        self.pressed = true;
                    }
                    if waited > 3.0 {
                        self.fail("drawing the satchel", now);
                    }
                } else if waited >= SETTLE {
                    self.satchels = count(WeaponId::Satchel);
                    self.next(Step::SatchelRun, now);
                }
            }
            Step::SatchelRun => {
                f.forward = 250.0;
                f.view.x = SATCHEL_PITCH;
                if waited >= RUN_UP {
                    self.next(Step::SatchelThrow, now);
                }
            }
            Step::SatchelThrow => {
                f.view.x = SATCHEL_PITCH;
                if !self.pressed {
                    f.forward = 250.0;
                    f.buttons = button(self.dll.satchel_throw());
                    self.pressed = true;
                } else if waited >= 1.0 {
                    let out = predicted(WeaponId::Satchel).is_some_and(|p| p.charge_ready == 1);
                    self.lines.push(format!(
                        "  satchel: the {} attack {}",
                        name(self.dll.satchel_throw()),
                        if out {
                            "throws one (as expected)"
                        } else {
                            "threw nothing"
                        }
                    ));
                    if !out {
                        self.fail("throwing a satchel", now);
                    } else {
                        self.satchels = count(WeaponId::Satchel);
                        self.next(Step::SatchelWait, now);
                    }
                }
            }
            Step::SatchelWait => {
                if waited >= 1.0 {
                    self.next(Step::SatchelDetonate { tried_other: false }, now);
                }
            }
            Step::SatchelBackOff => {
                f.forward = -250.0;
                if waited >= RUN_UP {
                    self.next(Step::SatchelDetonate { tried_other: true }, now);
                }
            }
            Step::SatchelDetonate { tried_other } => {
                let detonate = if tried_other {
                    other(self.dll.satchel_detonate())
                } else {
                    self.dll.satchel_detonate()
                };
                if !self.pressed {
                    f.buttons = button(detonate);
                    self.pressed = true;
                } else if waited >= 1.0 {
                    let state = predicted(WeaponId::Satchel).map_or(0, |p| p.charge_ready);
                    let thrown = count(WeaponId::Satchel) < self.satchels;
                    if state != 1 && !thrown {
                        self.lines
                            .push(format!("  satchel: the {} attack sets the charges off", name(detonate)));
                        self.satchel_ok = Some(!tried_other);
                        self.next(Step::CrossbowDraw, now);
                    } else if thrown && !tried_other {
                        self.lines.push(format!(
                            "  satchel: the {} attack threw another satchel instead of setting them off",
                            name(detonate)
                        ));
                        self.satchels = count(WeaponId::Satchel);
                        self.next(Step::SatchelBackOff, now);
                    } else {
                        self.lines.push("  satchel: neither attack sets the charges off".into());
                        self.next(Step::CrossbowDraw, now);
                    }
                }
            }
            Step::CrossbowDraw => {
                if current != Some(WeaponId::Crossbow) {
                    if !self.pressed {
                        f.commands.push(vec![WeaponId::Crossbow.classname().into()]);
                        self.pressed = true;
                    }
                    if waited > 3.0 {
                        self.fail("drawing the crossbow", now);
                    }
                } else if waited >= SETTLE {
                    self.next(Step::CrossbowZoom, now);
                }
            }
            Step::CrossbowZoom | Step::CrossbowUnzoom => {
                let zoom = self.step == Step::CrossbowZoom;
                if !self.pressed {
                    f.buttons = IN_ATTACK2;
                    self.pressed = true;
                } else if waited >= 1.2 {
                    let fov = s.body.fov;
                    if zoom {
                        self.lines.push(format!(
                            "  crossbow: the secondary attack {}",
                            if (fov - 20.0).abs() < 1.0 {
                                "zooms to 20°".to_string()
                            } else {
                                format!("left the view at {fov}°")
                            }
                        ));
                        self.next(Step::CrossbowUnzoom, now);
                    } else {
                        self.next(Step::GrenadeDraw, now);
                    }
                }
            }
            Step::GrenadeDraw => {
                if current != Some(WeaponId::HandGrenade) {
                    if !self.pressed {
                        f.commands.push(vec![WeaponId::HandGrenade.classname().into()]);
                        self.pressed = true;
                    }
                    if waited > 3.0 {
                        self.fail("drawing the grenade", now);
                    }
                } else if waited >= SETTLE {
                    self.seen = projectiles
                        .iter()
                        .filter(|p| p.kind == ProjectileKind::Grenade)
                        .map(|p| p.index)
                        .collect();
                    self.next(Step::GrenadePin, now);
                }
            }
            Step::GrenadePin => {
                f.buttons = IN_ATTACK;
                if waited >= 0.7 {
                    self.next(Step::GrenadeRelease, now);
                }
            }
            Step::GrenadeRelease => {
                let new = projectiles.iter().find(|p| {
                    p.kind == ProjectileKind::Grenade && p.owner == u16::from(slot) && !self.seen.contains(&p.index)
                });
                if let Some(g) = new {
                    let speed = g.velocity.length();
                    self.grenade_speed = Some(speed);
                    let (k, _) = self.dll.grenade_speed();
                    // Thrown level: the view pitch 0 becomes 10° up, (90 + 10) × k.
                    self.lines.push(format!(
                        "  grenade: thrown level it left at {speed:.0} units/s (the profile expects {:.0})",
                        100.0 * k
                    ));
                    self.next(Step::Done, now);
                } else if waited > 1.5 {
                    self.fail("throwing the grenade", now);
                }
            }
            Step::Done => {
                if !self.finished {
                    self.finish();
                }
            }
        }
        f.lines = std::mem::take(&mut self.lines);
        f
    }

    fn finish(&mut self) {
        self.finished = true;
        let classic_satchel = match self.satchel_ok {
            Some(true) => Some(self.dll.satchel_detonate() == Attack::Primary),
            Some(false) => Some(self.dll.satchel_detonate() == Attack::Secondary),
            None => None,
        };
        let classic_grenade = self.grenade_speed.map(|v| (v - 400.0).abs() < (v - 650.0).abs());
        let kind = match (classic_satchel, classic_grenade) {
            (Some(true), _) | (None, Some(true)) => Some(DllKind::Classic),
            (Some(false), Some(false)) | (None, Some(false)) => Some(match self.dll.kind {
                DllKind::Classic => DllKind::Bugfixed,
                k => k,
            }),
            (Some(false), None) => Some(match self.dll.kind {
                DllKind::Classic => DllKind::Bugfixed,
                k => k,
            }),
            (Some(false), Some(true)) | (None, None) => None,
        };
        match kind {
            Some(k) if k == self.dll.kind => {
                self.lines
                    .push(format!("  verdict: the {} profile is right", k.as_str()));
                self.verdict = Some(self.dll);
            }
            Some(k) => {
                self.lines.push(format!(
                    "  verdict: the server plays by the {} rules, not {}: set game.dll: {} in config/lambdabots.yaml",
                    k.as_str(),
                    self.dll.kind.as_str(),
                    k.as_str()
                ));
                self.verdict = Some(DllProfile {
                    kind: k,
                    detected: false,
                    satchel_swapped: false,
                });
            }
            None => self
                .lines
                .push("  verdict: the results do not fit a known profile".into()),
        }
    }
}

fn other(a: Attack) -> Attack {
    match a {
        Attack::Primary => Attack::Secondary,
        Attack::Secondary => Attack::Primary,
    }
}
