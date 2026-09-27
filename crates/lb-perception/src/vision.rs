//! Vision: which players a bot sees, and when it recognizes them.
//!
//! A player is a candidate when it is within view range, in the PVS the engine builds around the bot's eye, and
//! some corner of its box (or its center) projects into the view frustum. Up to six body points are then traced for
//! line of sight; other players block, glass does not. The weighted share of visible points, where the player is
//! in the view, how it moves, how far it is and what drew attention to it set the rate at which evidence
//! accumulates. A contact is recognized when the evidence reaches 1; the time that takes at full rate is drawn once
//! per contact. Nothing here consumes randomness before there is evidence, so hidden players change nothing.

use lb_core::Vec3;
use lb_core::math::view_angle_vectors;
use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use lb_game::weapons::WeaponId;
use lb_knowledge::*;
use lb_raw::RawClient;
use lb_worldq::{TraceQuery, Tracer, VisSets};
use smallvec::SmallVec;

use crate::PerceptionParams;

pub const PERIOD: f64 = 0.05;
pub const VIEW_RANGE: f32 = 4096.0;
/// Line-of-sight traces per bot and tick.
pub const TRACE_BUDGET: u32 = 12;
/// Field of view the HL client uses when `fov` is 0 (degrees, 4:3).
pub const DEFAULT_FOV: f32 = 90.0;
/// Screen shape the view is built for; wider screens see more to the sides (Hor+).
pub const DEFAULT_ASPECT: f32 = 16.0 / 9.0;
/// A contact without evidence keeps its evidence and delay this long.
const PENDING_GRACE: f64 = 0.75;
/// A recognized player out of sight this briefly is still followed without recognizing it again.
const SIGHT_HOLD: f64 = 0.1;
const CUE_EVIDENCE: f32 = 0.4;
/// Firing seen: a weapon event of the player this recent.
const SHOT_SEEN_FOR: f64 = 0.2;
/// A player not in contact starts one only if one of the first three body points (chest, head, pelvis) is in
/// sight; the rest is not traced when all three are blocked.
const FIRST_LOOK_POINTS: usize = 3;

const EF_MUZZLEFLASH: u32 = 2;
const EF_NODRAW: u32 = 128;
const FL_ONGROUND: u32 = 1 << 9;
const FL_DUCKING: u32 = 1 << 14;
const MOVETYPE_FLY: u8 = 5;

struct BodyPoint {
    bit: u8,
    weight: f32,
    stand_z: f32,
    crouch_z: f32,
    /// Offset across the line of sight, units.
    lateral: f32,
}

/// Checked in this order (after `checkBodyPartsWithOffsets`); the weights add up to 1.
const BODY: [BodyPoint; 6] = [
    BodyPoint {
        bit: parts::CHEST,
        weight: 0.35,
        stand_z: 8.0,
        crouch_z: 0.0,
        lateral: 0.0,
    },
    BodyPoint {
        bit: parts::HEAD,
        weight: 0.15,
        stand_z: 22.0,
        crouch_z: 10.0,
        lateral: 0.0,
    },
    BodyPoint {
        bit: parts::PELVIS,
        weight: 0.20,
        stand_z: -10.0,
        crouch_z: -12.0,
        lateral: 0.0,
    },
    BodyPoint {
        bit: parts::LEFT,
        weight: 0.10,
        stand_z: 8.0,
        crouch_z: 0.0,
        lateral: 13.0,
    },
    BodyPoint {
        bit: parts::RIGHT,
        weight: 0.10,
        stand_z: 8.0,
        crouch_z: 0.0,
        lateral: -13.0,
    },
    BodyPoint {
        bit: parts::KNEES,
        weight: 0.10,
        stand_z: -28.0,
        crouch_z: -15.0,
        lateral: 0.0,
    },
];

/// The bot doing the looking.
#[derive(Clone, Copy, Debug)]
pub struct Viewer {
    /// Edict index (the client slot); its own body never blocks its view.
    pub index: u16,
    pub eye: Vec3,
    /// View angles as sent with the bot's commands.
    pub angles: Vec3,
    /// `pev->fov`: 0 = default, else a zoomed field of view (4:3 degrees).
    pub fov: f32,
    pub aspect: f32,
    pub head_in_water: bool,
    /// Team index, 0 = no teams: everyone else is an enemy.
    pub team: u8,
}

/// Another player as the server has it, with what perception may learn once it is seen.
#[derive(Clone, Copy, Debug)]
pub struct Subject<'a> {
    pub raw: &'a RawClient,
    pub key: PlayerKey,
    /// Team index from the scoreboard, 0 = no teams.
    pub team: u8,
    /// From the weapon model the player shows.
    pub weapon: Option<WeaponId>,
    /// Last weapon event of this player.
    pub shot_at: Option<SimTime>,
}

impl Subject<'_> {
    /// Alive, drawn and in the game.
    pub fn can_be_seen(&self) -> bool {
        self.raw.state == lb_raw::ClientState::Spawned && self.raw.deadflag == 0 && self.raw.effects & EF_NODRAW == 0
    }
}

/// The view frustum built from the actual view angles.
#[derive(Clone, Copy, Debug)]
pub struct Frustum {
    eye: Vec3,
    forward: Vec3,
    right: Vec3,
    up: Vec3,
    tan_h: f32,
    tan_v: f32,
    half_h: f32,
}

impl Frustum {
    /// `fov` is the engine's 4:3 field of view; the horizontal angle widens with `aspect` (Hor+).
    pub fn new(eye: Vec3, angles: Vec3, fov: f32, aspect: f32) -> Frustum {
        let fov = if fov <= 0.0 { DEFAULT_FOV } else { fov.min(170.0) };
        let tan43 = (fov.to_radians() * 0.5).tan();
        let tan_h = tan43 * aspect / (4.0 / 3.0);
        let (forward, right, up) = view_angle_vectors(angles);
        Frustum {
            eye,
            forward,
            right,
            up,
            tan_h,
            tan_v: tan43 * 0.75,
            half_h: tan_h.atan(),
        }
    }

    /// Horizontal field of view in degrees.
    pub fn horizontal_fov(&self) -> f32 {
        self.half_h.to_degrees() * 2.0
    }

    pub fn contains(&self, p: Vec3) -> bool {
        let d = p - self.eye;
        let x = d.dot(self.forward);
        x > 0.0 && d.dot(self.right).abs() <= x * self.tan_h && d.dot(self.up).abs() <= x * self.tan_v
    }

    /// Some corner of the box, or its center, is in view.
    pub fn contains_box(&self, mins: Vec3, maxs: Vec3) -> bool {
        if self.contains((mins + maxs) * 0.5) {
            return true;
        }
        (0..8).any(|i| {
            let c = Vec3::new(
                if i & 1 == 0 { mins.x } else { maxs.x },
                if i & 2 == 0 { mins.y } else { maxs.y },
                if i & 4 == 0 { mins.z } else { maxs.z },
            );
            self.contains(c)
        })
    }

    /// Angle from the view axis to `p` as a share of the horizontal half field of view.
    pub fn eccentricity(&self, p: Vec3) -> f32 {
        let d = (p - self.eye).normalize_or_zero();
        d.dot(self.forward).clamp(-1.0, 1.0).acos() / self.half_h
    }
}

#[derive(Clone, Debug)]
pub struct Contact {
    pub who: PlayerKey,
    /// Accumulated evidence; recognized at 1.
    pub evidence: f32,
    /// Seconds to recognize at full rate, drawn when the contact started.
    pub delay: f32,
    pub first_evidence: SimTime,
    pub last_evidence: SimTime,
    pub recognized: bool,
    /// A recognized contact out of sight since this time.
    pub lost_at: Option<SimTime>,
    /// Started as a quick re-acquisition of a player lost moments ago.
    pub reacquired: bool,
    pub cue_sent: bool,
    pub visibility: f32,
    pub parts: u8,
    pub distance: f32,
}

/// A contact recognized on this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recognition {
    pub who: PlayerKey,
    /// Seconds from the first evidence.
    pub latency: f64,
    pub distance: f32,
    pub reacquired: bool,
}

#[derive(Clone, Debug, Default)]
pub struct VisionOutput {
    pub sightings: Vec<Sighting>,
    pub cues: Vec<AnonymousCue>,
    pub recognitions: Vec<Recognition>,
}

impl VisionOutput {
    pub fn clear(&mut self) {
        self.sightings.clear();
        self.cues.clear();
        self.recognitions.clear();
    }
}

#[derive(Clone, Debug, Default)]
pub struct VisionStats {
    pub ticks: u64,
    pub traces: u64,
    /// Candidates left unchecked because the trace budget ran out.
    pub skipped: u64,
    pub recognitions: u64,
    pub reacquisitions: u64,
    pub latency_sum: f64,
    pub latency_max: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Vision {
    pub contacts: Vec<Contact>,
    pub stats: VisionStats,
    last_tick: Option<SimTime>,
    /// When each player not in contact was last looked at, so a short trace budget rotates over all of them.
    looked_at: SmallVec<[(PlayerKey, SimTime); 32]>,
}

/// Result of looking at one candidate.
enum Look {
    /// Not enough trace budget left to tell.
    Skipped,
    Hidden,
    Seen {
        visibility: f32,
        parts: u8,
    },
}

impl Vision {
    pub fn reset(&mut self) {
        self.contacts.clear();
        self.last_tick = None;
        self.looked_at.clear();
    }

    /// Forgets a player (it died or left).
    pub fn forget(&mut self, slot: u8) {
        self.contacts.retain(|c| c.who.slot != slot);
        self.looked_at.retain(|(k, _)| k.slot != slot);
    }

    fn looked_at(&self, key: PlayerKey) -> f64 {
        self.looked_at
            .iter()
            .find(|(k, _)| *k == key)
            .map_or(f64::NEG_INFINITY, |(_, t)| t.0)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        now: SimTime,
        viewer: &Viewer,
        subjects: &[Subject<'_>],
        beliefs: &Beliefs,
        vis: &dyn VisSets,
        tracer: &mut dyn Tracer,
        params: &PerceptionParams,
        rng: &mut Pcg32,
        out: &mut VisionOutput,
    ) {
        let dt = self.last_tick.map_or(PERIOD, |t| now.since(t)).clamp(0.0, 0.1) as f32;
        self.last_tick = Some(now);
        self.stats.ticks += 1;
        let frustum = Frustum::new(viewer.eye, viewer.angles, viewer.fov, viewer.aspect);
        // Players in contact first (recognized, then noticed); the rest least recently looked at first, then by
        // distance, so a short budget still reaches everyone within a few ticks.
        let mut order: SmallVec<[(u8, f64, f32, usize); 32]> = subjects
            .iter()
            .enumerate()
            .filter(|(_, s)| s.raw.slot as u16 != viewer.index && s.can_be_seen())
            .map(|(i, s)| {
                let (rank, since) = match self.contacts.iter().find(|c| c.who == s.key) {
                    Some(c) if c.recognized => (0, 0.0),
                    Some(_) => (1, 0.0),
                    None => (2, self.looked_at(s.key)),
                };
                (rank, since, s.raw.origin.distance(viewer.eye), i)
            })
            .collect();
        order.sort_by(|a, b| {
            (a.0, a.1, a.2)
                .partial_cmp(&(b.0, b.1, b.2))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut budget = TRACE_BUDGET;
        let mut touched: SmallVec<[PlayerKey; 32]> = SmallVec::new();
        for &(rank, _, distance, i) in &order {
            let s = &subjects[i];
            let look = self.look(viewer, &frustum, s, distance, rank, vis, tracer, &mut budget);
            if rank == 2 && !matches!(look, Look::Skipped) {
                match self.looked_at.iter_mut().find(|(k, _)| *k == s.key) {
                    Some(entry) => entry.1 = now,
                    None => self.looked_at.push((s.key, now)),
                }
            }
            match look {
                Look::Skipped => {
                    self.stats.skipped += 1;
                    touched.push(s.key);
                }
                Look::Hidden => {}
                Look::Seen { visibility, parts } => {
                    touched.push(s.key);
                    self.evidence(
                        now, dt, viewer, &frustum, s, distance, visibility, parts, beliefs, params, rng, out,
                    );
                }
            }
        }
        self.contacts.retain_mut(|c| {
            if touched.contains(&c.who) {
                return true;
            }
            if c.recognized {
                let lost = *c.lost_at.get_or_insert(now);
                return now.since(lost) <= SIGHT_HOLD;
            }
            c.evidence = (c.evidence - dt / (2.0 * c.delay)).max(0.0);
            now.since(c.last_evidence) <= PENDING_GRACE
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn look(
        &mut self,
        viewer: &Viewer,
        frustum: &Frustum,
        s: &Subject<'_>,
        distance: f32,
        rank: u8,
        vis: &dyn VisSets,
        tracer: &mut dyn Tracer,
        budget: &mut u32,
    ) -> Look {
        let raw = s.raw;
        let (mins, maxs) = (raw.origin + raw.mins, raw.origin + raw.maxs);
        if distance > VIEW_RANGE || !vis.box_in_pvs(viewer.eye, mins, maxs) || !frustum.contains_box(mins, maxs) {
            return Look::Hidden;
        }
        let crouched = raw.flags & FL_DUCKING != 0;
        let flat = (raw.origin - viewer.eye).truncate().normalize_or_zero();
        let across = Vec3::new(-flat.y, flat.x, 0.0);
        let (mut visibility, mut parts, mut hits) = (0.0f32, 0u8, 0);
        for (n, p) in BODY.iter().enumerate() {
            if rank == 0 && hits >= 2 || rank == 2 && n == FIRST_LOOK_POINTS && hits == 0 {
                break;
            }
            if *budget == 0 {
                if hits == 0 {
                    return Look::Skipped;
                }
                break;
            }
            *budget -= 1;
            self.stats.traces += 1;
            let z = if crouched { p.crouch_z } else { p.stand_z };
            let point = raw.origin + Vec3::Z * z + across * p.lateral;
            let tr = tracer.trace(&TraceQuery::sight(viewer.eye, point, viewer.index));
            if tr.fraction >= 1.0 || tr.hit == Some(u32::from(raw.slot)) {
                visibility += p.weight;
                parts |= p.bit;
                hits += 1;
            }
        }
        if hits == 0 {
            return Look::Hidden;
        }
        if viewer.head_in_water != (raw.waterlevel >= 2) {
            visibility *= 0.5;
        }
        Look::Seen { visibility, parts }
    }

    #[allow(clippy::too_many_arguments)]
    fn evidence(
        &mut self,
        now: SimTime,
        dt: f32,
        viewer: &Viewer,
        frustum: &Frustum,
        s: &Subject<'_>,
        distance: f32,
        visibility: f32,
        parts: u8,
        beliefs: &Beliefs,
        params: &PerceptionParams,
        rng: &mut Pcg32,
        out: &mut VisionOutput,
    ) {
        let raw = s.raw;
        let index = match self.contacts.iter().position(|c| c.who == s.key) {
            Some(i) => i,
            None => {
                let reacquired = beliefs.track(s.key).is_some_and(|t| {
                    now.since(t.last_seen) <= f64::from(params.reacquire_grace)
                        && t.pos.distance(raw.origin) <= 3.0 * t.sigma + 64.0
                });
                let delay = if reacquired {
                    params.reacquire_delay
                } else {
                    rng.range_f32(params.recognition_delay[0], params.recognition_delay[1])
                };
                self.contacts.push(Contact {
                    who: s.key,
                    evidence: 0.0,
                    delay: delay.max(0.01),
                    first_evidence: now,
                    last_evidence: now,
                    recognized: false,
                    lost_at: None,
                    reacquired,
                    cue_sent: false,
                    visibility,
                    parts,
                    distance,
                });
                self.contacts.len() - 1
            }
        };
        let c = &mut self.contacts[index];
        c.visibility = visibility;
        c.parts = parts;
        c.distance = distance;
        c.last_evidence = now;
        let firing = raw.effects & EF_MUZZLEFLASH != 0 || s.shot_at.is_some_and(|t| now.since(t) <= SHOT_SEEN_FOR);
        let first = !c.recognized;
        if first {
            let chest = raw.origin + Vec3::Z * 8.0;
            let rate = visibility
                * fov_gain(frustum.eccentricity(chest), params.peripheral_gain)
                * motion_gain(raw)
                * range_gain(distance)
                * cue_gain(firing, bearing(viewer.eye, raw.origin), beliefs, now);
            c.evidence += rate * dt / c.delay;
            if c.evidence < 1.0 {
                if c.evidence >= CUE_EVIDENCE && !c.cue_sent {
                    c.cue_sent = true;
                    let dir = (raw.origin - viewer.eye).normalize_or_zero();
                    let range = RangeBin::of(distance);
                    out.cues.push(AnonymousCue {
                        t: now,
                        dir,
                        range,
                        pos: viewer.eye + dir * range.distance(),
                    });
                }
                return;
            }
            c.recognized = true;
            let latency = now.since(c.first_evidence);
            out.recognitions.push(Recognition {
                who: s.key,
                latency,
                distance,
                reacquired: c.reacquired,
            });
            self.stats.recognitions += 1;
            if c.reacquired {
                self.stats.reacquisitions += 1;
            } else {
                self.stats.latency_sum += latency;
                self.stats.latency_max = self.stats.latency_max.max(latency);
            }
        }
        let c = &mut self.contacts[index];
        c.lost_at = None;
        let sigma = (0.002 * distance).max(0.5);
        let noise = Vec3::new(rng.normal(), rng.normal(), rng.normal()) * sigma;
        let relation = if viewer.team != 0 && s.team == viewer.team {
            Relation::Friend
        } else {
            Relation::Enemy
        };
        out.sightings.push(Sighting {
            who: s.key,
            relation,
            t: now,
            pos: raw.origin + noise,
            sigma,
            distance,
            visibility,
            parts,
            stance: if raw.flags & FL_DUCKING != 0 {
                Stance::Crouched
            } else {
                Stance::Standing
            },
            on_ground: raw.flags & FL_ONGROUND != 0,
            on_ladder: raw.movetype == MOVETYPE_FLY,
            in_water: raw.waterlevel >= 2,
            facing: lb_core::math::normalize_angle(raw.angles.y + rng.normal() * 5.0),
            weapon: s.weapon,
            firing,
            render: RenderCue {
                mode: raw.rendermode,
                fx: raw.renderfx,
                amount: raw.renderamt,
            },
            first,
            noticed_at: self.contacts[index].first_evidence,
        });
    }
}

/// World yaw from `from` toward `to`, degrees.
pub fn bearing(from: Vec3, to: Vec3) -> f32 {
    let d = to - from;
    d.y.atan2(d.x).to_degrees()
}

/// Center of the view counts fully, the middle band 0.7, the edge the bot's peripheral gain.
pub fn fov_gain(eccentricity: f32, peripheral: f32) -> f32 {
    if eccentricity <= 0.35 {
        1.0
    } else if eccentricity <= 0.8 {
        0.7
    } else {
        peripheral
    }
}

/// Moving players are noticed sooner (`isEnemyNoticeable` as rates).
pub fn motion_gain(raw: &RawClient) -> f32 {
    let speed = raw.velocity.length();
    if speed > 150.0 {
        1.0
    } else if speed >= 30.0 {
        0.8
    } else if raw.flags & FL_DUCKING != 0 {
        0.3
    } else {
        0.5
    }
}

pub fn range_gain(distance: f32) -> f32 {
    if distance < 300.0 {
        1.0
    } else if distance < 1000.0 {
        1.0 - 0.4 * (distance - 300.0) / 700.0
    } else if distance < 2500.0 {
        0.6 - 0.25 * (distance - 1000.0) / 1500.0
    } else {
        0.35
    }
}

/// A seen muzzle flash doubles the rate, a sound or the damage compass pointing there adds half, a fight a third.
pub fn cue_gain(firing: bool, bearing: f32, beliefs: &Beliefs, now: SimTime) -> f32 {
    let mut g = 1.0;
    if firing {
        g *= 2.0;
    }
    if beliefs.primed(bearing, now) {
        g *= 1.5;
    }
    if beliefs.alert(now) {
        g *= 1.3;
    }
    g
}
