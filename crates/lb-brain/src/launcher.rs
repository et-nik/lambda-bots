//! The RPG as a strong player works it; the game's rocket is in [`lb_combat::rocket`].
//!
//! - **Aim:** low on the near side of the target's box, where it will be when the rocket gets there. A guided
//!   rocket's lead shrinks as it flies, so the laser spot comes onto the target as the rocket does (a spot led all the
//!   way slides off a strafing target onto whatever lies behind it); an unguided one's view makes up for its path,
//!   which runs above and to the right of the view's line.
//! - **Laser:** off at targets closer than the skill's `rpg_unguided`, on further. A switch is a tap of the secondary
//!   attack while the launcher is ready, the shot held for it; the game tells the state in the weapon data
//!   (`vuser2`), and a game that never shows a switch is left as it is.
//! - **Quick reload:** with the laser off (or no rocket of the bot's on the way) the empty launcher is reloaded the
//!   moment the rocket is out, a rocket every 2 s rather than the game's 3.5; with the laser on, the game will not
//!   reload while it guides.
//! - **Pressed:** an enemy that stays within the rocket's least distance (longer than the bot's patience; at once
//!   when the bot is hurt or the enemy very close) gets a rocket anyway, as close as its blast leaves the bot some
//!   health. A bot about to die under fire fires at anyone beyond the crowbar's reach (not in GunGame, where a
//!   suicide costs).

use lb_combat::rocket::{self, Height};
use lb_core::Vec3;
use lb_core::rng::Pcg32;
use lb_game::mechanics::Trigger;
use lb_game::self_state::Prediction;
use lb_game::weapons::WeaponId;
use lb_knowledge::{EnemyTrack, TrackState};
use lb_motor::{Fire, WeaponIntent};
use lb_worldq::{TraceQuery, Tracer};
use smallvec::SmallVec;

use crate::mind::{Body, Character, Mind, rocket_min};

/// Around the skill's unguided distance the laser stays as it is within this share of it.
const LASER_HYSTERESIS: f32 = 0.1;
/// A laser switch shows in the weapon data by then (the game switches it every 0.2 s at most).
const LASER_SETTLE: f64 = 0.3;
/// After this many switches in a row the game never showed, the laser is left as it is for the life: an even number,
/// so a game that does not tell is left with the laser as it was.
const LASER_UNSEEN: u8 = 2;
/// A rocket that meets something this close to the point it goes for goes off there; one that would meet something
/// further from it than this is not fired. The rocket's way is looked along this far past the point.
const ON_AIM: f32 = 40.0;
const OFF_AIM: f32 = 120.0;
const PAST_AIM: f32 = 64.0;
/// With the floor in front of the target out of the rocket's way, it is aimed at the target's middle for this long.
const HIGH_FOR: f64 = 0.5;
/// A rocket leads the target by its mean velocity over this many seconds (a strafe there and back), and by what it does
/// now beyond that as if it went on for this long at most: a strafe turns back within some 0.3 s, and the lead with
/// it.
const RUN_WINDOW: f64 = 0.8;
const STRAFE_KEEPS: f32 = 0.2;
/// The bot's own run toward the target carries it on this long after a shot on the ground (it stops, or backs off);
/// in the air it flies on the whole of the rocket's way.
const OWN_RUN: f32 = 0.15;
/// While the game reloads, the bot counts on it for this much longer at least.
const RELOAD_STEP: f32 = 0.05;
/// A pressed rocket leaves the bot this much health.
const TRADE_SPARE: f32 = 15.0;
/// With this much health or less, under fire, the bot fires at anyone further than `POINT_BLANK`.
const DOOMED: f32 = 35.0;
const POINT_BLANK: f32 = 64.0;
/// Seconds an enemy may stay within the rocket's least distance before the bot fires anyway: bold, careful.
const PATIENCE: [f32; 2] = [0.25, 1.0];
/// An enemy closer than this presses the bot at once.
const PRESSED_AT: f32 = 160.0;

/// What the game tells of the launcher in hand.
fn in_hand(body: &Body) -> Option<&Prediction> {
    body.prediction
        .as_ref()
        .filter(|p| body.weapon == Some(WeaponId::Rpg) && p.current == Some(WeaponId::Rpg))
}

/// The launcher in hand has a rocket in it, as the game tells (a frame ahead of the HUD's messages).
pub(crate) fn loaded(body: &Body) -> Option<bool> {
    in_hand(body)
        .and_then(|p| p.weapons[WeaponId::Rpg as usize])
        .map(|w| w.clip > 0)
}

/// The laser guides the rockets now: as the game tells, on (as at spawn) for a game that does not.
pub(crate) fn laser_on(m: &Mind, body: &Body) -> bool {
    m.arms.laser_unseen >= LASER_UNSEEN || in_hand(body).is_none_or(|p| p.rpg_laser)
}

/// The laser guides a rocket of the bot's on its way: the game does not reload meanwhile.
pub(crate) fn guiding(body: &Body) -> bool {
    in_hand(body).is_some_and(|p| p.rpg_laser && p.rpg_rockets > 0)
}

/// The laser is wanted against a target `distance` away, `on` now.
pub(crate) fn wants_laser(ch: &Character, distance: f32, on: bool) -> bool {
    let Some(limit) = ch.skill.rpg_unguided else {
        return true;
    };
    let share = if on {
        1.0 - LASER_HYSTERESIS
    } else {
        1.0 + LASER_HYSTERESIS
    };
    distance >= limit * share
}

/// What the laser asks of a shot about to go.
pub(crate) enum Laser {
    /// As wanted, or left as it is: fire.
    Ready,
    /// Switched on this frame.
    Switch(WeaponIntent),
    /// To be switched once the launcher is ready, or shown switched.
    Wait,
}

/// The laser of the loaded launcher in hand, to be `want`.
pub(crate) fn laser(m: &mut Mind, body: &Body, want: bool) -> Laser {
    let now = body.now;
    let Some(p) = in_hand(body) else {
        return Laser::Ready;
    };
    if let Some((at, before)) = m.arms.laser_flip {
        if p.rpg_laser != before {
            m.arms.laser_unseen = 0;
        } else if now.since(at) < LASER_SETTLE {
            return Laser::Wait;
        } else {
            m.arms.laser_unseen = m.arms.laser_unseen.saturating_add(1);
        }
        m.arms.laser_flip = None;
    }
    if m.arms.laser_unseen >= LASER_UNSEEN || p.rpg_laser == want || (p.rpg_laser && p.rpg_rockets > 0) {
        return Laser::Ready;
    }
    let ready = p.weapons[WeaponId::Rpg as usize].is_some_and(|w| !w.reloading && w.next_secondary <= 0.0);
    if !ready || p.next_attack > 0.0 {
        return Laser::Wait;
    }
    m.arms.laser_flip = Some((now, p.rpg_laser));
    Laser::Switch(WeaponIntent {
        select: Some(WeaponId::Rpg),
        fire: Fire::Toggle,
        trigger: Trigger::Tap,
        interval: 0.0,
        reload: false,
    })
}

/// The empty launcher in hand, reloaded the moment the game lets it be: at once after an unguided rocket. The bot
/// reckons with the reload once the game shows it ([`follow_reload`]).
pub(crate) fn quick_reload(m: &Mind, body: &Body) -> Option<WeaponIntent> {
    if m.choice.is_some_and(|c| c.weapon() != WeaponId::Rpg) {
        return None;
    }
    let p = in_hand(body)?;
    let w = p.weapons[WeaponId::Rpg as usize]?;
    if w.clip > 0 || w.reloading || p.primary_ammo <= 0 || p.next_attack > 0.0 || (p.rpg_laser && p.rpg_rockets > 0) {
        return None;
    }
    Some(WeaponIntent {
        reload: true,
        ..WeaponIntent::hold(WeaponId::Rpg)
    })
}

/// The bot's reload of the launcher in hand lasts as long as the game's: its own reckoning ran out a moment early and,
/// the HUD's clip still empty, asked for another reload's time with a rocket in the clip.
pub(crate) fn follow_reload(m: &mut Mind, body: &Body) {
    if let Some(w) = in_hand(body).and_then(|p| p.weapons[WeaponId::Rpg as usize]) {
        m.follow_reload(body.now, w.reloading, RELOAD_STEP);
    }
}

/// The bot's speed along the line from its eye to `to`, which the game gives its rocket.
pub(crate) fn along(body: &Body, to: Vec3) -> f32 {
    body.velocity.dot((to - body.eye).normalize_or_zero())
}

/// The launcher is ready to fire now, as the game tells: the shot asked for leaves the launcher on this frame.
pub(crate) fn ready(body: &Body) -> Option<bool> {
    in_hand(body).map(|p| {
        p.next_attack <= 0.0
            && p.weapons[WeaponId::Rpg as usize].is_some_and(|w| w.clip > 0 && !w.reloading && w.next_primary <= 0.0)
    })
}

/// How much closer than the rocket's least distance the target and the bot come while the rocket flies `distance`.
fn closing(body: &Body, t: &EnemyTrack, distance: f32) -> f32 {
    let flight = rocket::flight(distance, along(body, t.pos));
    let to = (t.pos - body.origin).truncate().normalize_or_zero();
    let theirs = if t.velocity_known(body.now) {
        (-t.vel.truncate()).dot(to).max(0.0)
    } else {
        0.0
    };
    let mine = body.velocity.truncate().dot(to).max(0.0);
    let carried = if body.on_ground { flight.min(OWN_RUN) } else { flight };
    theirs * flight + mine * carried
}

/// Rockets are not fired at `t`, `distance` away, closer than this: [`rocket_min`] where the rocket will meet it.
pub(crate) fn rocket_from(body: &Body, t: &EnemyTrack, distance: f32) -> f32 {
    rocket_min(body) + closing(body, t, distance)
}

/// Every combat tick: when the target in sight came within the rocket's least distance, kept while it stays there.
pub(crate) fn watch_close(m: &mut Mind, body: &Body, t: Option<&EnemyTrack>, distance: f32) {
    m.rocket_close = match t {
        Some(t) if t.state == TrackState::Visible && distance < rocket_from(body, t, distance) => {
            Some(m.rocket_close.filter(|(k, _)| *k == t.who).unwrap_or((t.who, body.now)))
        }
        _ => None,
    };
}

/// The target presses the bot: it has stayed within the rocket's least distance longer than the bot's patience, or
/// hurts it, or is very close.
pub(crate) fn pressed(m: &Mind, body: &Body, ch: &Character, t: &EnemyTrack, distance: f32) -> bool {
    let Some((who, since)) = m.rocket_close else {
        return false;
    };
    let patience = PATIENCE[1] + (PATIENCE[0] - PATIENCE[1]) * ch.aggression.clamp(0.0, 1.0);
    who == t.who && (m.under_fire(body.now) || distance < PRESSED_AT || body.now.since(since) >= f64::from(patience))
}

/// Rockets are fired at `t`, `distance` away, no closer than this now: [`rocket_from`], and pressed as close as the
/// blast leaves the bot [`TRADE_SPARE`] health (about to die, beyond the crowbar's reach).
pub(crate) fn least(m: &Mind, body: &Body, ch: &Character, t: &EnemyTrack, distance: f32) -> f32 {
    let safe = rocket_from(body, t, distance);
    if !pressed(m, body, ch, t, distance) {
        return safe;
    }
    let doomed = body.health <= DOOMED && m.under_fire(body.now) && body.gungame.is_none();
    let floor = if doomed {
        POINT_BLANK
    } else {
        let damage = body.damages.primary(WeaponId::Rpg);
        rocket::trade_min(body.health, body.armor, damage, TRADE_SPARE)
    };
    (floor + closing(body, t, distance)).min(safe)
}

/// Where the view goes for a rocket at `t`, `guided` by the laser or not ([`Mind::aim`] goes by what the bot saw of
/// it, the aim error added).
pub(crate) fn aim(
    m: &mut Mind,
    t: &EnemyTrack,
    body: &Body,
    ch: &Character,
    guided: bool,
    tracer: &mut dyn Tracer,
    rng: &mut Pcg32,
) -> Option<Vec3> {
    let now = body.now;
    let skill = ch.aim();
    let seen = m.aim.seen(now, body.eye, &skill, rng)?;
    // Led along the ground (the track's vertical speed lags a landing): on its run as it has gone of late, on what it
    // does now only for a while, as a strafe turns back.
    let vel = Vec3::new(seen.vel.x, seen.vel.y, 0.0);
    let mean = m
        .aim
        .mean_velocity(now, skill.latency, RUN_WINDOW)
        .unwrap_or(Vec3::ZERO);
    let led = |t: f32| mean * t + (vel - mean) * (STRAFE_KEEPS * (1.0 - lb_core::dmath::exp(-t / STRAFE_KEEPS)));
    let lead = match m.arms.rocket_flying(now).filter(|r| r.guided && r.target == t.who) {
        Some(r) => (r.arrives.since(now) as f32).max(0.0),
        None => {
            let speed = along(body, seen.center);
            let first = rocket::flight(seen.center.distance(body.eye), speed);
            rocket::flight((seen.center + led(first)).distance(body.eye), speed)
        }
    };
    let ahead = seen.center + led(lead);
    let half = if seen.crouched { 18.0 } else { 36.0 };
    let height = if now < m.arms.rocket_high_until {
        Height::Middle
    } else {
        Height::Floor
    };
    let point = rocket::aim_point(body.eye, seen.center, ahead, half, seen.on_ground, height, tracer) + seen.error;
    Some(if guided {
        point
    } else {
        rocket::unguided_view(body.eye, point, along(body, point))
    })
}

/// Why a rocket fired now along the view, on `look`, would do no good: its blast would reach the bot, which may be no
/// closer to it than `need` (a center's distance, as [`least`]), or it would meet something well before or past `aim`,
/// the point it goes for. The rocket goes where the view is, which the trigger lets be some degrees off the aim: past
/// a corner it would clip. It leaves under the eye and rises over the view's line before it turns (a door's lintel
/// right in front catches it); then a guided one flies to the laser spot, an unguided one on along its path.
pub(crate) fn way(
    m: &mut Mind,
    body: &Body,
    look: Vec3,
    aim: Vec3,
    guided: bool,
    need: f32,
    tracer: &mut dyn Tracer,
) -> Option<&'static str> {
    let eye = body.eye;
    let speed = along(body, look);
    let reach = look.distance(eye);
    let point = if guided {
        look
    } else {
        rocket::unguided_path(eye, look, reach, speed)
    };
    let aimed = if guided {
        aim
    } else {
        rocket::unguided_path(eye, aim, aim.distance(eye), speed)
    };
    let mut stops: SmallVec<[Vec3; 5]> = rocket::rise(eye, look, speed)
        .into_iter()
        .filter(|(_, d)| *d < reach)
        .map(|(p, _)| p)
        .collect();
    let last = stops.last().copied().unwrap_or(eye);
    stops.push(point);
    stops.push(point + (point - last).normalize_or_zero() * PAST_AIM);
    let hit = stops.windows(2).find_map(|w| {
        let t = tracer.trace(&TraceQuery::line(w[0], w[1]));
        if t.start_solid {
            Some((w[0], Vec3::ZERO))
        } else {
            (t.fraction < 1.0).then(|| (w[0] + (w[1] - w[0]) * t.fraction, t.normal))
        }
    });
    let (hit, normal) = hit?;
    if hit.distance(point).min(hit.distance(aimed)) <= ON_AIM {
        return None;
    }
    let pull = rocket::pull_out(body.damages.primary(WeaponId::Rpg));
    if (hit + normal * pull).distance(body.origin) < need - rocket::HALF_WIDTH {
        return Some("the blast would reach it");
    }
    if hit.distance(aimed) > OFF_AIM {
        if !guided {
            m.arms.rocket_high_until = body.now + HIGH_FOR;
        }
        return Some("the rocket would hit something on the way");
    }
    None
}
