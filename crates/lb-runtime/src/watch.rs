//! `lb watch`: what a player does, written to the log to study how people play. Twenty times a second where they
//! are, how they move, where they look and what they hold; four times a second the room around them and where the
//! others are; their shots with what the shot met, their tripmines as they are laid and go, and their satchels and
//! hand grenades as they are thrown, fly (twenty times a second), come to rest and go off.

use lb_core::Vec3;
use lb_core::time::SimTime;
use lb_host::{Host, TraceKind, TraceRequest, TraceResult};
use lb_raw::{ClientState, RawClient, RawPlayback};
use rustc_hash::FxHashMap;

use crate::Runtime;

const EVERY: f64 = 0.05;
const ROOM_EVERY: f64 = 0.25;
/// Directions the room is measured in, at waist height.
const ROOM_DIRS: usize = 8;
const ROOM_REACH: f32 = 1024.0;
const SHOT_REACH: f32 = 4096.0;
const FL_ONGROUND: u32 = 1 << 9;
const FL_DUCKING: u32 = 1 << 14;

pub(crate) struct Watched {
    pub slot: u8,
    pub userid: i32,
    next: SimTime,
    next_room: SimTime,
    /// Its tripmines on the map by entity index: when each was first seen.
    mines: FxHashMap<u16, SimTime>,
    /// Its satchels and hand grenades out by entity index.
    thrown: FxHashMap<u16, Thrown>,
    shots: Vec<Shot>,
}

struct Thrown {
    what: &'static str,
    thrown: SimTime,
    from: Vec3,
    at: Vec3,
    lying: bool,
}

struct Shot {
    event: String,
    origin: Vec3,
    angles: Vec3,
}

impl Watched {
    pub fn new(slot: u8, userid: i32) -> Watched {
        Watched {
            slot,
            userid,
            next: SimTime::ZERO,
            next_room: SimTime::ZERO,
            mines: FxHashMap::default(),
            thrown: FxHashMap::default(),
            shots: Vec::new(),
        }
    }
}

fn view(c: &RawClient) -> Vec3 {
    Vec3::new(-3.0 * c.angles.x, c.angles.y, 0.0)
}

/// How far a line from `start` to `end` gets; past monsters and players unless one is to be ignored (a shot).
fn reach(host: &mut dyn Host, start: Vec3, end: Vec3, ignore: Option<u16>) -> (f32, TraceResult) {
    let tr = host.trace(&TraceRequest {
        start,
        end,
        kind: TraceKind::Line,
        ignore_monsters: ignore.is_none(),
        ignore_glass: false,
        ignore: ignore.map(|index| lb_ffi::LbEntRef {
            index,
            pad: 0,
            serial: 0,
        }),
    });
    (start.distance(end) * tr.fraction, tr)
}

/// Each frame, after the players' snapshots are in.
pub(crate) fn tick(rt: &mut Runtime, host: &mut dyn Host) {
    let now = rt.now;
    let mut watched = std::mem::take(&mut rt.watched);
    watched.retain(|w| {
        let alive = rt.clients.get(w.slot).is_some_and(|c| c.userid == w.userid);
        if !alive {
            tracing::info!("watch #{}: left the server", w.userid);
        }
        alive
    });
    for w in &mut watched {
        let Some(c) = rt.clients_now.iter().find(|c| c.slot == w.slot).cloned() else {
            continue;
        };
        let eye = c.origin + c.view_ofs;
        for s in std::mem::take(&mut w.shots) {
            // Most weapon events carry no angles (the glock's does not): the shot goes where the player looks.
            let aim = if s.angles == Vec3::ZERO { view(&c) } else { s.angles };
            let (d, tr) = reach(
                host,
                eye,
                eye + lb_core::math::view_angle_vectors(aim).0 * SHOT_REACH,
                Some(u16::from(w.slot)),
            );
            let hit = if tr.hit.index == 0 {
                "the world".to_string()
            } else if let Some(slot) = rt.player_slot(tr.hit.index) {
                format!("player #{}", rt.clients.get(slot).map_or(0, |c| c.userid))
            } else {
                format!("{} #{}", rt.strings.string_lossy(tr.hit_classname), tr.hit.index)
            };
            tracing::info!(
                "watch #{} shot {} from {:.0} {:.0} {:.0} aimed {:.1} {:.1}: met {hit} {d:.0} units off at {:.0} {:.0} {:.0}",
                w.userid,
                s.event,
                s.origin.x,
                s.origin.y,
                s.origin.z,
                aim.x,
                aim.y,
                tr.end_pos.x,
                tr.end_pos.y,
                tr.end_pos.z
            );
        }
        if now >= w.next || now + 1.0 < w.next {
            w.next = now + EVERY;
            let v = view(&c);
            let weapon = rt
                .weapon_models
                .get(&c.weaponmodel)
                .copied()
                .flatten()
                .map_or_else(|| rt.strings.string_lossy(c.weaponmodel), |w| w.classname().to_string());
            tracing::info!(
                "watch #{} at {:.0} {:.0} {:.0} vel {:.0} {:.0} {:.0} speed {:.0} view {:.1} {:.1} {}{}{}water {} holds {weapon}{}",
                w.userid,
                c.origin.x,
                c.origin.y,
                c.origin.z,
                c.velocity.x,
                c.velocity.y,
                c.velocity.z,
                c.velocity.truncate().length(),
                v.x,
                v.y,
                if c.flags & FL_ONGROUND != 0 { "ground " } else { "air " },
                if c.flags & FL_DUCKING != 0 { "ducked " } else { "" },
                if c.state == ClientState::Spawned {
                    ""
                } else {
                    "unspawned "
                },
                c.waterlevel,
                if c.deadflag != 0 { " dead" } else { "" }
            );
        }
        if now >= w.next_room || now + 1.0 < w.next_room {
            w.next_room = now + ROOM_EVERY;
            let mut room = String::new();
            for i in 0..ROOM_DIRS {
                let (sin, cos) = lb_core::dmath::sin_cos((i as f32 * 360.0 / ROOM_DIRS as f32).to_radians());
                let dir = Vec3::new(cos, sin, 0.0);
                let (d, _) = reach(host, c.origin, c.origin + dir * ROOM_REACH, None);
                room.push_str(&format!(" {d:.0}"));
            }
            let others: Vec<String> = rt
                .clients_now
                .iter()
                .filter(|o| o.slot != w.slot && o.state == ClientState::Spawned && o.deadflag == 0)
                .map(|o| format!("#{} {:.0} {:.0} {:.0}", o.userid, o.origin.x, o.origin.y, o.origin.z))
                .collect();
            tracing::info!(
                "watch #{} room (0 45 .. 315 degrees){room}; others: {}",
                w.userid,
                others.join(", ")
            );
        }
    }
    rt.watched = watched;
}

/// A weapon event: the watched player's shots are traced on the next frame.
pub(crate) fn on_playback(rt: &mut Runtime, p: &RawPlayback) {
    let Some(slot) = rt.player_slot(p.invoker.index) else {
        return;
    };
    let Some(w) = rt.watched.iter_mut().find(|w| w.slot == slot) else {
        return;
    };
    let event = rt
        .strings
        .event_name(i32::from(p.event_index))
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default();
    let origin = if p.origin == Vec3::ZERO {
        p.invoker_origin
    } else {
        p.origin
    };
    w.shots.push(Shot {
        event,
        origin,
        angles: p.angles,
    });
}

/// After the projectiles are refreshed: the watched players' tripmines laid and gone.
pub(crate) fn mines(rt: &mut Runtime, laid: &[(u16, u16, Vec3, Vec3, bool)]) {
    let now = rt.now;
    for w in &mut rt.watched {
        let slot = u16::from(w.slot);
        for &(index, owner, origin, angles, armed) in laid {
            if owner == slot && !w.mines.contains_key(&index) {
                w.mines.insert(index, now);
                tracing::info!(
                    "watch #{} mine #{index} laid at {:.0} {:.0} {:.0} facing {:.0} {:.0}{}",
                    w.userid,
                    origin.x,
                    origin.y,
                    origin.z,
                    angles.x,
                    angles.y,
                    if armed { " (armed)" } else { "" }
                );
            }
        }
        let userid = w.userid;
        w.mines.retain(|index, at| {
            let here = laid.iter().any(|l| l.0 == *index && l.1 == slot);
            if !here {
                tracing::info!("watch #{userid} mine #{index} gone after {:.1} s", now.since(*at));
            }
            here
        });
    }
}

/// After the projectiles are refreshed: the watched players' satchels and hand grenades (what, index, owner, origin,
/// velocity) thrown, come to rest and gone, how far the player and the nearest other player were from each as it was
/// thrown and as it went. One gone off is seen where it burst (`bursts`: index, origin; the entity stays a moment,
/// hidden, there); else where it was last seen.
pub(crate) fn thrown(rt: &mut Runtime, out: &[(&'static str, u16, u16, Vec3, Vec3)], bursts: &[(u16, Vec3)]) {
    let now = rt.now;
    let clients = &rt.clients_now;
    for w in &mut rt.watched {
        let Some(me) = clients.iter().find(|c| c.slot == w.slot) else {
            continue;
        };
        let (slot, userid) = (u16::from(w.slot), w.userid);
        let nearest_other = |at: Vec3| {
            clients
                .iter()
                .filter(|c| u16::from(c.slot) != slot && c.state == ClientState::Spawned && c.deadflag == 0)
                .map(|c| (c.origin.distance(at), c.userid))
                .fold((f32::INFINITY, 0), |a, b| if b.0 < a.0 { b } else { a })
        };
        for &(what, index, _, origin, velocity) in out.iter().filter(|s| s.2 == slot) {
            let Some(s) = w.thrown.get_mut(&index) else {
                let v = view(me);
                let (d, other) = nearest_other(me.origin);
                tracing::info!(
                    "watch #{userid} {what} #{index} thrown: at {:.0} {:.0} {:.0} vel {:.0} {:.0} {:.0}; the player at {:.0} {:.0} {:.0} vel {:.0} {:.0} {:.0} view {:.1} {:.1}{}; the nearest other #{other} {d:.0} off",
                    origin.x,
                    origin.y,
                    origin.z,
                    velocity.x,
                    velocity.y,
                    velocity.z,
                    me.origin.x,
                    me.origin.y,
                    me.origin.z,
                    me.velocity.x,
                    me.velocity.y,
                    me.velocity.z,
                    v.x,
                    v.y,
                    if me.flags & FL_ONGROUND != 0 { " ground" } else { " air" }
                );
                w.thrown.insert(
                    index,
                    Thrown {
                        what,
                        thrown: now,
                        from: origin,
                        at: origin,
                        lying: false,
                    },
                );
                continue;
            };
            s.at = origin;
            tracing::info!(
                "watch #{userid} {what} #{index} path {:.2} at {:.1} {:.1} {:.1} vel {:.0} {:.0} {:.0}",
                now.since(s.thrown),
                origin.x,
                origin.y,
                origin.z,
                velocity.x,
                velocity.y,
                velocity.z
            );
            if !s.lying && velocity.length() < 1.0 {
                s.lying = true;
                tracing::info!(
                    "watch #{userid} {what} #{index} lies at {:.0} {:.0} {:.0} after {:.1} s, {:.0} units from where it was first seen, the player {:.0} off",
                    origin.x,
                    origin.y,
                    origin.z,
                    now.since(s.thrown),
                    origin.distance(s.from),
                    origin.distance(me.origin)
                );
            }
        }
        w.thrown.retain(|index, s| {
            let here = out.iter().any(|o| o.1 == *index && o.2 == slot);
            if !here {
                let burst = bursts.iter().find(|b| b.0 == *index).map(|b| b.1);
                let at = burst.unwrap_or(s.at);
                let (d, other) = nearest_other(at);
                tracing::info!(
                    "watch #{userid} {} #{index} {} after {:.2} s at {:.0} {:.0} {:.0}, {:.0} units from where it was first seen: the player {:.0} units off, the nearest other #{other} {d:.0}",
                    s.what,
                    if burst.is_some() { "burst" } else { "gone" },
                    now.since(s.thrown),
                    at.x,
                    at.y,
                    at.z,
                    at.distance(s.from),
                    at.distance(me.origin)
                );
            }
            here
        });
    }
}
