//! Map mechanisms from the entity lump: doors, lifts and platforms, buttons, triggers, teleports, breakables, and
//! what sets each of them off (`doors.cpp`, `plats.cpp`, `buttons.cpp`, `triggers.cpp`, `func_break.cpp`).
//!
//! Positions are offsets of a brush entity from where its model was compiled: a door is compiled closed and opens
//! to `active`; a door flagged to start open spawns at its travel end and closes when activated.

use lb_core::Vec3;
use lb_core::math::view_angle_vectors;

use crate::entities::Entity;
use crate::world::BspWorld;

pub const SF_DOOR_START_OPEN: i32 = 1;
pub const SF_DOOR_PASSABLE: i32 = 8;
pub const SF_DOOR_NO_AUTO_RETURN: i32 = 32;
pub const SF_DOOR_USE_ONLY: i32 = 256;
pub const SF_BUTTON_DONTMOVE: i32 = 1;
pub const SF_BUTTON_TOGGLE: i32 = 32;
pub const SF_BUTTON_TOUCH_ONLY: i32 = 256;
pub const SF_PLAT_TOGGLE: i32 = 1;
pub const SF_BREAK_TRIGGER_ONLY: i32 = 1;
pub const SF_BREAK_CROWBAR: i32 = 256;
/// `func_breakable` material that cannot be broken.
pub const MATERIAL_UNBREAKABLE_GLASS: i32 = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoverKind {
    Door,
    /// Rotates open; its swing is not modelled offline.
    RotatingDoor,
    /// `func_plat`: a trigger field over it raises it, it comes back down 3 s after reaching the top.
    Plat,
    Button,
    /// Trains, rotating and pendulum brushes: moving geometry nothing here predicts.
    Other,
}

#[derive(Clone, Debug)]
pub struct Mover {
    pub entity: usize,
    pub model: usize,
    pub kind: MoverKind,
    pub classname: String,
    pub targetname: Option<String>,
    pub target: Option<String>,
    pub master: Option<String>,
    pub spawnflags: i32,
    pub movedir: Vec3,
    /// Offset at rest (`m_vecPosition1`: bottom for lifts, closed for doors) and when activated.
    pub rest: Vec3,
    pub active: Vec3,
    pub speed: f32,
    /// Seconds at the activated position before returning; -1 = stays.
    pub wait: f32,
    pub dmg: f32,
    /// Buttons: shootable when above zero.
    pub health: f32,
    /// A player walking into it sets it off.
    pub touch: bool,
    /// The use key sets it off (`FCAP_IMPULSE_USE`).
    pub usable: bool,
}

impl Mover {
    pub fn travel_time(&self) -> f32 {
        (self.active - self.rest).length() / self.speed.max(1.0)
    }

    /// Moves up or down when activated.
    pub fn vertical(&self) -> bool {
        let d = self.active - self.rest;
        d.z.abs() > d.truncate().length()
    }

    /// Activated once, it only comes back when set off again.
    pub fn toggles(&self) -> bool {
        match self.kind {
            MoverKind::Door | MoverKind::RotatingDoor => {
                self.spawnflags & SF_DOOR_NO_AUTO_RETURN != 0 || self.wait < 0.0
            }
            MoverKind::Plat => self.spawnflags & SF_PLAT_TOGGLE != 0,
            MoverKind::Button => self.spawnflags & SF_BUTTON_TOGGLE != 0 || self.wait < 0.0,
            MoverKind::Other => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerKind {
    Multiple,
    Once,
    Teleport,
    Push,
    Hurt,
    Other,
}

#[derive(Clone, Debug)]
pub struct Trigger {
    pub entity: usize,
    pub model: usize,
    pub kind: TriggerKind,
    pub targetname: Option<String>,
    pub target: Option<String>,
    pub master: Option<String>,
    pub wait: f32,
    pub delay: f32,
    pub spawnflags: i32,
    /// `trigger_push`: the velocity it gives a player in it (speed along its direction).
    pub push: Vec3,
}

/// `trigger_push` flags.
pub const SF_PUSH_ONCE: i32 = 1;
pub const SF_PUSH_START_OFF: i32 = 2;
/// `trigger_multiple`/`trigger_once`: players do not set it off.
pub const SF_TRIGGER_NOCLIENTS: i32 = 2;

#[derive(Clone, Debug)]
pub struct Breakable {
    pub entity: usize,
    pub model: usize,
    pub health: f32,
    pub material: i32,
    pub spawnflags: i32,
    pub targetname: Option<String>,
}

impl Breakable {
    /// Weapons can break it (not trigger-only, not unbreakable glass).
    pub fn breakable(&self) -> bool {
        self.spawnflags & SF_BREAK_TRIGGER_ONLY == 0 && self.material != MATERIAL_UNBREAKABLE_GLASS
    }

    /// One crowbar hit breaks it.
    pub fn crowbar_instant(&self) -> bool {
        self.spawnflags & SF_BREAK_CROWBAR != 0
    }
}

/// What sets off a named entity.
#[derive(Clone, Debug, PartialEq)]
pub enum Activation {
    /// Walking into the trigger (or touch-only button) of model `*model`.
    Touch { model: usize },
    /// Pressing use at the button (or use-only door) of model `*model`.
    Use { model: usize },
    /// Shooting the button of model `*model`.
    Shoot { model: usize },
    /// Fires by itself (`trigger_auto`, timers).
    Auto,
    /// Something the bot cannot operate or that hides state (`multisource`, `env_global`, ...).
    Unsupported { classname: String },
}

/// A way to set off an entity, and the delay until it happens.
#[derive(Clone, Debug, PartialEq)]
pub struct Activator {
    pub how: Activation,
    pub delay: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Mechanisms {
    pub movers: Vec<Mover>,
    pub triggers: Vec<Trigger>,
    pub breakables: Vec<Breakable>,
    /// `(targetname, entity index)` of point entities others can refer to (teleport destinations, relays).
    names: Vec<(String, usize)>,
}

/// Entity variables the engine takes before a `multi_manager` sees its keys; every other key is a target.
const ENTVARS: &[&str] = &[
    "classname",
    "origin",
    "angles",
    "angle",
    "targetname",
    "target",
    "spawnflags",
    "wait",
    "model",
    "netname",
    "message",
    "globalname",
    "rendermode",
    "renderamt",
    "rendercolor",
    "renderfx",
    "speed",
    "health",
];

fn opt(e: &Entity, key: &str) -> Option<String> {
    e.get(key).filter(|v| !v.is_empty()).map(str::to_string)
}

fn float(e: &Entity, key: &str) -> Option<f32> {
    e.get(key)?.trim().parse().ok()
}

/// `SetMovedir`: angle -1 is up, -2 is down, anything else the forward vector of the angles.
fn movedir(e: &Entity) -> Vec3 {
    let angles = e
        .vec3("angles")
        .unwrap_or_else(|| Vec3::new(0.0, float(e, "angle").unwrap_or(0.0), 0.0));
    if angles == Vec3::new(0.0, -1.0, 0.0) {
        Vec3::Z
    } else if angles == Vec3::new(0.0, -2.0, 0.0) {
        -Vec3::Z
    } else {
        view_angle_vectors(angles).0
    }
}

/// Travel of a door or button along `dir`: its size along the direction less 2 and less the lip.
fn travel(dir: Vec3, size: Vec3, lip: f32) -> Vec3 {
    let along = (dir.x * (size.x - 2.0)).abs() + (dir.y * (size.y - 2.0)).abs() + (dir.z * (size.z - 2.0)).abs();
    dir * (along - lip)
}

impl Mechanisms {
    pub fn from_world(world: &BspWorld) -> Mechanisms {
        let mut m = Mechanisms::default();
        for (i, e) in world.entities.iter().enumerate() {
            let class = e.classname();
            if let Some(name) = opt(e, "targetname") {
                m.names.push((name, i));
            }
            let Some(model) = e.brush_model() else { continue };
            let Some(bm) = world.bsp.models.get(model) else {
                continue;
            };
            let size = bm.maxs - bm.mins;
            let spawnflags = e.spawnflags();
            match class {
                "func_door" | "func_door_rotating" | "func_water" => {
                    if e.int("skin").unwrap_or(if class == "func_water" { -3 } else { 0 }) != 0 {
                        continue;
                    }
                    let rotating = class == "func_door_rotating";
                    let dir = if rotating { Vec3::ZERO } else { movedir(e) };
                    let pos2 = if rotating {
                        Vec3::ZERO
                    } else {
                        travel(dir, size, float(e, "lip").unwrap_or(0.0))
                    };
                    let (rest, active) = if spawnflags & SF_DOOR_START_OPEN != 0 {
                        (pos2, Vec3::ZERO)
                    } else {
                        (Vec3::ZERO, pos2)
                    };
                    let targetname = opt(e, "targetname");
                    m.movers.push(Mover {
                        entity: i,
                        model,
                        kind: if rotating {
                            MoverKind::RotatingDoor
                        } else {
                            MoverKind::Door
                        },
                        classname: class.into(),
                        touch: targetname.is_none() && spawnflags & SF_DOOR_USE_ONLY == 0,
                        usable: spawnflags & SF_DOOR_USE_ONLY != 0,
                        targetname,
                        target: opt(e, "target"),
                        master: opt(e, "master"),
                        spawnflags,
                        movedir: dir,
                        rest,
                        active,
                        speed: float(e, "speed").filter(|s| *s != 0.0).unwrap_or(100.0),
                        wait: float(e, "wait").unwrap_or(0.0),
                        dmg: float(e, "dmg").unwrap_or(0.0),
                        health: 0.0,
                    });
                }
                "func_plat" => {
                    let height = float(e, "height").filter(|h| *h != 0.0).unwrap_or(size.z - 8.0);
                    let bottom = Vec3::new(0.0, 0.0, -height);
                    let targetname = opt(e, "targetname");
                    // A targeted plat waits at the top until used; the others rest at the bottom.
                    let (rest, active) = if targetname.is_some() {
                        (Vec3::ZERO, bottom)
                    } else {
                        (bottom, Vec3::ZERO)
                    };
                    m.movers.push(Mover {
                        entity: i,
                        model,
                        kind: MoverKind::Plat,
                        classname: class.into(),
                        touch: spawnflags & SF_PLAT_TOGGLE == 0,
                        usable: false,
                        targetname,
                        target: opt(e, "target"),
                        master: opt(e, "master"),
                        spawnflags,
                        movedir: Vec3::Z,
                        rest,
                        active,
                        speed: float(e, "speed").filter(|s| *s != 0.0).unwrap_or(150.0),
                        wait: 3.0,
                        dmg: 1.0,
                        health: 0.0,
                    });
                }
                "func_button" | "func_rot_button" => {
                    let dir = if class == "func_button" { movedir(e) } else { Vec3::ZERO };
                    let lip = float(e, "lip").filter(|l| *l != 0.0).unwrap_or(4.0);
                    let mut pos2 = travel(dir, size, lip);
                    if spawnflags & SF_BUTTON_DONTMOVE != 0 || pos2.length() < 1.0 {
                        pos2 = Vec3::ZERO;
                    }
                    let health = float(e, "health").unwrap_or(0.0);
                    m.movers.push(Mover {
                        entity: i,
                        model,
                        kind: MoverKind::Button,
                        classname: class.into(),
                        touch: spawnflags & SF_BUTTON_TOUCH_ONLY != 0,
                        usable: spawnflags & SF_BUTTON_TOUCH_ONLY == 0 && health <= 0.0,
                        targetname: opt(e, "targetname"),
                        target: opt(e, "target"),
                        master: opt(e, "master"),
                        spawnflags,
                        movedir: dir,
                        rest: Vec3::ZERO,
                        active: pos2,
                        speed: float(e, "speed").filter(|s| *s != 0.0).unwrap_or(40.0),
                        wait: float(e, "wait").filter(|w| *w != 0.0).unwrap_or(1.0),
                        dmg: 0.0,
                        health,
                    });
                }
                "func_train"
                | "func_tracktrain"
                | "func_rotating"
                | "func_pendulum"
                | "func_platrot"
                | "momentary_door"
                | "momentary_rot_button" => {
                    m.movers.push(Mover {
                        entity: i,
                        model,
                        kind: MoverKind::Other,
                        classname: class.into(),
                        touch: false,
                        usable: false,
                        targetname: opt(e, "targetname"),
                        target: opt(e, "target"),
                        master: opt(e, "master"),
                        spawnflags,
                        movedir: Vec3::ZERO,
                        rest: Vec3::ZERO,
                        active: Vec3::ZERO,
                        speed: float(e, "speed").unwrap_or(0.0),
                        wait: -1.0,
                        dmg: float(e, "dmg").unwrap_or(0.0),
                        health: 0.0,
                    });
                }
                _ if class.starts_with("trigger_") => {
                    let kind = match class {
                        "trigger_multiple" => TriggerKind::Multiple,
                        "trigger_once" => TriggerKind::Once,
                        "trigger_teleport" => TriggerKind::Teleport,
                        "trigger_push" => TriggerKind::Push,
                        "trigger_hurt" => TriggerKind::Hurt,
                        _ => TriggerKind::Other,
                    };
                    m.triggers.push(Trigger {
                        entity: i,
                        model,
                        kind,
                        targetname: opt(e, "targetname"),
                        target: opt(e, "target"),
                        master: opt(e, "master"),
                        wait: float(e, "wait").unwrap_or(if kind == TriggerKind::Multiple { 0.2 } else { 0.0 }),
                        delay: float(e, "delay").unwrap_or(0.0),
                        spawnflags,
                        // `CTriggerPush::Spawn`: no angles push along +x, no speed is 100.
                        push: if kind == TriggerKind::Push {
                            movedir(e) * float(e, "speed").filter(|s| *s != 0.0).unwrap_or(100.0)
                        } else {
                            Vec3::ZERO
                        },
                    });
                }
                "func_breakable" => m.breakables.push(Breakable {
                    entity: i,
                    model,
                    health: float(e, "health").unwrap_or(0.0),
                    material: e.int("material").unwrap_or(0),
                    spawnflags,
                    targetname: opt(e, "targetname"),
                }),
                _ => {}
            }
        }
        m
    }

    pub fn mover(&self, model: usize) -> Option<&Mover> {
        self.movers.iter().find(|m| m.model == model)
    }

    pub fn trigger(&self, model: usize) -> Option<&Trigger> {
        self.triggers.iter().find(|t| t.model == model)
    }

    pub fn breakable(&self, model: usize) -> Option<&Breakable> {
        self.breakables.iter().find(|b| b.model == model)
    }

    /// Entities named `name`.
    pub fn named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = usize> + 'a {
        self.names.iter().filter(move |(n, _)| n == name).map(|(_, i)| *i)
    }

    /// Where a teleport trigger sends a player: the first entity named after its target.
    pub fn teleport_destination(&self, world: &BspWorld, trigger: &Trigger) -> Option<(Vec3, f32)> {
        let target = trigger.target.as_deref()?;
        let e = &world.entities[self.named(target).next()?];
        Some((e.origin(), e.yaw()))
    }

    /// Push fields on when the map starts that keep pushing while a player is in them (a push-once trigger adds its
    /// velocity a single time and is gone): `(model, velocity)`.
    pub fn push_fields(&self) -> Vec<(usize, Vec3)> {
        self.triggers
            .iter()
            .filter(|t| t.kind == TriggerKind::Push && t.spawnflags & (SF_PUSH_START_OFF | SF_PUSH_ONCE) == 0)
            .map(|t| (t.model, t.push))
            .collect()
    }

    /// Puts every mover in its rest position and makes it block.
    pub fn place_at_rest(&self, world: &mut BspWorld) {
        for m in &self.movers {
            if let Some(b) = world.brush_mut(m.model) {
                b.offset = m.rest;
                b.solid = true;
            }
        }
    }

    /// Ways to set off entities named `name`, following relays and `multi_manager`s up to four steps back.
    pub fn activators(&self, world: &BspWorld, name: &str) -> Vec<Activator> {
        let mut out = Vec::new();
        self.collect(world, name, 0.0, 0, &mut out);
        out
    }

    fn collect(&self, world: &BspWorld, name: &str, delay: f32, depth: u32, out: &mut Vec<Activator>) {
        if depth > 4 {
            return;
        }
        for e in &world.entities {
            let class = e.classname();
            let fires = if class == "multi_manager" {
                e.kv.iter().find_map(|(k, v)| {
                    let key = k.split('#').next().unwrap_or(k);
                    (key == name && !ENTVARS.contains(&k.as_str())).then(|| v.trim().parse::<f32>().unwrap_or(0.0))
                })
            } else {
                (e.get("target") == Some(name)).then_some(0.0)
            };
            let Some(extra) = fires else { continue };
            let delay = delay + extra + float(e, "delay").unwrap_or(0.0);
            let model = e.brush_model();
            let how = match (class, model) {
                ("func_button" | "func_rot_button", Some(model)) => {
                    let Some(b) = self.mover(model) else { continue };
                    if b.health > 0.0 {
                        Activation::Shoot { model }
                    } else if b.touch {
                        Activation::Touch { model }
                    } else {
                        Activation::Use { model }
                    }
                }
                ("trigger_multiple" | "trigger_once", Some(model)) if e.spawnflags() & SF_TRIGGER_NOCLIENTS == 0 => {
                    Activation::Touch { model }
                }
                ("func_door" | "func_door_rotating", Some(model)) => match self.mover(model) {
                    Some(d) if d.usable => Activation::Use { model },
                    Some(d) if d.touch => Activation::Touch { model },
                    _ => {
                        if let Some(n) = opt(e, "targetname") {
                            self.collect(world, &n, delay, depth + 1, out);
                        }
                        continue;
                    }
                },
                ("trigger_auto", _) => Activation::Auto,
                ("multi_manager" | "trigger_relay", _) => {
                    if let Some(n) = opt(e, "targetname") {
                        self.collect(world, &n, delay, depth + 1, out);
                    }
                    continue;
                }
                _ => Activation::Unsupported {
                    classname: class.to_string(),
                },
            };
            out.push(Activator { how, delay });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossfire_lifts_buttons_and_doors() {
        let Some(dir) = crate::test_maps_dir() else { return };
        let Ok(bytes) = std::fs::read(dir.join("crossfire.bsp")) else {
            return;
        };
        let world = BspWorld::load(&bytes).unwrap();
        let m = Mechanisms::from_world(&world);
        let lift1 = m.mover(21).expect("lift1 is model *21");
        assert_eq!(lift1.kind, MoverKind::Door);
        assert_eq!(lift1.targetname.as_deref(), Some("lift1"));
        assert!(lift1.vertical() && !lift1.touch && !lift1.usable);
        assert!((lift1.active.z - 158.0).abs() < 0.01, "{:?}", lift1.active);
        assert!((lift1.travel_time() - 0.79).abs() < 0.01);
        let act = m.activators(&world, "lift1");
        assert_eq!(
            act,
            vec![Activator {
                how: Activation::Use { model: 24 },
                delay: 0.0
            }]
        );
        // The bunker door starts open and closes when the airstrike fires it.
        let bunker = m.mover(5).unwrap();
        assert!(bunker.rest.z > 100.0 && bunker.active == Vec3::ZERO, "{bunker:?}");
        // The secret door opens from a touch plate.
        let secret = m.activators(&world, "secret_door");
        assert_eq!(secret[0].how, Activation::Touch { model: 64 });
        // The airstrike: a touch plate through a multi_manager to the shutter, 2 s later.
        let shutter = m.activators(&world, "strike_ready_door");
        assert!(
            shutter
                .iter()
                .any(|a| a.how == Activation::Touch { model: 65 } && (a.delay - 2.0).abs() < 1e-3),
            "{shutter:?}"
        );
    }

    #[test]
    fn no_clients_triggers_are_not_touched() {
        let world = BspWorld {
            bsp: crate::Bsp {
                planes: Vec::new(),
                hull0: Vec::new(),
                nodes: Vec::new(),
                clipnodes: Vec::new(),
                leafs: Vec::new(),
                models: Vec::new(),
                visdata: Vec::new(),
                entities: String::new(),
                textures: Vec::new(),
                fingerprint: ([0; 32], 0),
            },
            entities: crate::parse_entities(
                r#"{ "classname" "trigger_multiple" "model" "*1" "target" "a" }
{ "classname" "trigger_once" "model" "*2" "target" "a" "spawnflags" "2" }
{ "classname" "trigger_multiple" "model" "*3" "target" "a" "spawnflags" "3" }
{ "classname" "trigger_once" "model" "*4" "target" "a" "spawnflags" "1" }"#,
            ),
            brushes: Vec::new(),
            traces: 0,
            pushes: Vec::new(),
        };
        let how: Vec<Activation> = Mechanisms::default()
            .activators(&world, "a")
            .into_iter()
            .map(|a| a.how)
            .collect();
        let unsupported = |c: &str| Activation::Unsupported { classname: c.into() };
        assert_eq!(
            how,
            vec![
                Activation::Touch { model: 1 },
                unsupported("trigger_once"),
                unsupported("trigger_multiple"),
                Activation::Touch { model: 4 },
            ]
        );
    }

    #[test]
    fn push_fields_are_the_continuous_ones_on_at_start() {
        let push = |model, spawnflags| Trigger {
            entity: model,
            model,
            kind: TriggerKind::Push,
            targetname: None,
            target: None,
            master: None,
            wait: 0.0,
            delay: 0.0,
            spawnflags,
            push: Vec3::Z * 100.0,
        };
        let m = Mechanisms {
            triggers: vec![
                push(1, 0),
                push(2, SF_PUSH_ONCE),
                push(3, SF_PUSH_START_OFF),
                push(4, SF_PUSH_ONCE | SF_PUSH_START_OFF),
            ],
            ..Mechanisms::default()
        };
        assert_eq!(m.push_fields(), vec![(1, Vec3::Z * 100.0)]);
    }
}
