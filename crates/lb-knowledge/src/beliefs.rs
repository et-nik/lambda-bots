//! What a bot believes about other players: tracks of recognized players with honest uncertainty, and hypotheses
//! from sounds, damage and unrecognized glimpses.

use lb_core::Vec3;
use lb_core::math::angle_diff;
use lb_core::time::SimTime;
use lb_game::sounds::SoundKind;
use lb_game::weapons::WeaponId;

use crate::obs::*;
use crate::places::{Spread, Watch};
use lb_core::dmath;
use lb_nav_api::MapView;

/// A track counts as in sight while its last sighting is this recent (vision runs at 20 Hz).
const VISIBLE_AGE: f64 = 0.12;
const RECENTLY_LOST_AGE: f64 = 1.0;
const DROP_AFTER: f64 = 30.0;
/// Velocity from sightings is trusted, and extrapolated, this long after the last one.
const VELOCITY_VALID: f64 = 0.3;
const ALPHA: f32 = 0.5;
const BETA: f32 = 0.2;
/// Uncertainty grows at this share of the maximum speed per second since the last fix.
const SIGMA_GROWTH: f32 = 0.6;
const SIGMA_MAX: f32 = 1500.0;
const SOUND_LIFE: f64 = 5.0;
const DAMAGE_LIFE: f64 = 3.0;
const CUE_LIFE: f64 = 1.0;
const MAX_HYPOTHESES: usize = 12;
/// In a fight: an enemy recognized or damage taken this recently.
const ALERT_FOR: f64 = 3.0;
const PRIMED_FOR: f64 = 2.0;
const PRIMED_ANGLE: f32 = 30.0;
const RANGE_ERROR: f32 = 0.3;
/// Sounds are tied only to tracks this certain (about 3 s after the last fix) and seen within the bot's memory
/// span: an older track would soak up the steps of players the bot has never seen.
const ASSOCIATE_MAX_SIGMA: f32 = 600.0;

#[derive(Clone, Copy, Debug)]
pub struct BeliefParams {
    /// Seconds a lost player stays in memory after the last position fix.
    pub track_forget: f32,
    /// Server maximum speed, for the growth of position uncertainty.
    pub maxspeed: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackState {
    Visible,
    RecentlyLost,
    Predicted,
    Stale,
}

impl TrackState {
    pub fn as_str(self) -> &'static str {
        match self {
            TrackState::Visible => "visible",
            TrackState::RecentlyLost => "lost",
            TrackState::Predicted => "predicted",
            TrackState::Stale => "stale",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObservedTraits {
    pub weapon: Option<WeaponId>,
    pub stance: Stance,
    pub on_ground: bool,
    pub on_ladder: bool,
    pub in_water: bool,
    pub facing: f32,
    pub fired_at: Option<SimTime>,
    pub render: RenderCue,
}

#[derive(Clone, Debug)]
pub struct EnemyTrack {
    pub who: PlayerKey,
    pub relation: Relation,
    pub state: TrackState,
    /// Where the player is believed to be now, and the 1σ of that belief.
    pub pos: Vec3,
    pub sigma: f32,
    /// Estimated velocity; meaningful while `velocity_known`.
    pub vel: Vec3,
    pub last_seen: SimTime,
    /// When this contact was recognized, and when its first evidence was seen.
    pub recognized_at: SimTime,
    pub noticed_at: SimTime,
    pub last_heard: Option<SimTime>,
    /// Distance, visibility and visible parts at the last sighting.
    pub distance: f32,
    pub visibility: f32,
    pub parts: u8,
    pub traits: ObservedTraits,
    pub prov: Provenance,
    /// Where it may be while out of sight, over the map's places.
    pub spread: Option<Box<Spread>>,
    fix_pos: Vec3,
    fix_sigma: f32,
    fix_t: SimTime,
}

impl EnemyTrack {
    fn new(s: &Sighting) -> EnemyTrack {
        EnemyTrack {
            who: s.who,
            relation: s.relation,
            state: TrackState::Visible,
            pos: s.pos,
            sigma: s.sigma,
            vel: Vec3::ZERO,
            last_seen: s.t,
            recognized_at: s.t,
            noticed_at: s.noticed_at,
            last_heard: None,
            distance: s.distance,
            visibility: s.visibility,
            parts: s.parts,
            traits: traits_of(s, None),
            prov: Provenance::new(Sensor::Vision, s.t),
            spread: None,
            fix_pos: s.pos,
            fix_sigma: s.sigma,
            fix_t: s.t,
        }
    }

    /// When the player was last placed, by sight or sound.
    pub fn fixed_at(&self) -> SimTime {
        self.fix_t
    }

    pub fn velocity_known(&self, now: SimTime) -> bool {
        now.since(self.last_seen) <= VELOCITY_VALID && self.fix_t == self.last_seen
    }

    pub fn age(&self, now: SimTime) -> f64 {
        now.since(self.last_seen)
    }

    fn observe(&mut self, s: &Sighting) {
        let dt = s.t.since(self.last_seen) as f32;
        if !s.first && self.fix_t == self.last_seen && dt > 0.0 && f64::from(dt) <= VELOCITY_VALID {
            let predicted = self.fix_pos + self.vel * dt;
            let residual = s.pos - predicted;
            self.fix_pos = predicted + residual * ALPHA;
            self.vel += residual * (BETA / dt);
        } else {
            self.fix_pos = s.pos;
            self.vel = Vec3::ZERO;
        }
        if s.first {
            self.recognized_at = s.t;
            self.noticed_at = s.noticed_at;
        }
        self.fix_sigma = s.sigma;
        self.fix_t = s.t;
        self.last_seen = s.t;
        self.relation = s.relation;
        self.distance = s.distance;
        self.visibility = s.visibility;
        self.parts = s.parts;
        self.traits = traits_of(s, self.traits.fired_at);
        self.pos = self.fix_pos;
        self.sigma = self.fix_sigma;
        self.state = TrackState::Visible;
        self.prov = Provenance::new(Sensor::Vision, s.t);
    }

    /// Combines a position heard for this player with the current belief (both treated as isotropic Gaussians).
    fn fuse(&mut self, pos: Vec3, sigma: f32, t: SimTime) {
        let (a, b) = (self.sigma.max(1.0).powi(2), sigma.max(1.0).powi(2));
        let var = a * b / (a + b);
        self.fix_pos = (self.pos * b + pos * a) / (a + b);
        self.fix_sigma = var.sqrt();
        self.fix_t = t;
        self.vel = Vec3::ZERO;
        self.pos = self.fix_pos;
        self.sigma = self.fix_sigma;
        self.last_heard = Some(t);
        self.prov = Provenance::new(Sensor::Hearing, t);
    }

    fn age_to(&mut self, now: SimTime, p: &BeliefParams) -> bool {
        let since_fix = now.since(self.fix_t).max(0.0);
        self.pos = self.fix_pos + self.vel * since_fix.min(VELOCITY_VALID) as f32;
        self.sigma = (self.fix_sigma + SIGMA_GROWTH * p.maxspeed * since_fix as f32).min(SIGMA_MAX);
        let seen = now.since(self.last_seen);
        self.state = if seen <= VISIBLE_AGE {
            TrackState::Visible
        } else if seen <= RECENTLY_LOST_AGE {
            TrackState::RecentlyLost
        } else if since_fix <= f64::from(p.track_forget) {
            TrackState::Predicted
        } else {
            TrackState::Stale
        };
        since_fix <= DROP_AFTER
    }
}

fn traits_of(s: &Sighting, fired_at: Option<SimTime>) -> ObservedTraits {
    ObservedTraits {
        weapon: s.weapon,
        stance: s.stance,
        on_ground: s.on_ground,
        on_ladder: s.on_ladder,
        in_water: s.in_water,
        facing: s.facing,
        fired_at: if s.firing { Some(s.t) } else { fired_at },
        render: s.render,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HypothesisKind {
    Sound(SoundKind),
    Damage,
    Cue,
}

/// Something suggests a player is somewhere, without saying who.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hypothesis {
    /// Numbers hypotheses in the order they came, for goals to refer to one.
    pub id: u32,
    pub kind: HypothesisKind,
    pub t: SimTime,
    /// Estimated position; damage gives only a bearing.
    pub pos: Option<Vec3>,
    /// World yaw toward the source, degrees.
    pub bearing: f32,
    /// 1σ of the bearing, degrees.
    pub bearing_sigma: f32,
    pub weapon: Option<WeaponId>,
    /// 0..1: gain of a sound, share of health lost, evidence of a cue.
    pub strength: f32,
    /// Tied to a track (the only one that fits it).
    pub track: Option<PlayerKey>,
}

#[derive(Clone, Debug)]
pub struct Beliefs {
    pub tracks: Vec<EnemyTrack>,
    pub hypotheses: Vec<Hypothesis>,
    pub last_damage: Option<DamageStimulus>,
    alert_until: SimTime,
    /// `track_forget` of the last update: how long after a sighting sounds may still be tied to a track.
    memory: f64,
    next_hypothesis: u32,
}

impl Default for Beliefs {
    fn default() -> Self {
        Beliefs {
            tracks: Vec::new(),
            hypotheses: Vec::new(),
            last_damage: None,
            alert_until: SimTime::ZERO,
            memory: 8.0,
            next_hypothesis: 1,
        }
    }
}

impl Beliefs {
    pub fn track(&self, who: PlayerKey) -> Option<&EnemyTrack> {
        self.tracks.iter().find(|t| t.who == who)
    }

    pub fn track_by_slot(&self, slot: u8) -> Option<&EnemyTrack> {
        self.tracks.iter().find(|t| t.who.slot == slot)
    }

    pub fn on_sighting(&mut self, s: &Sighting) {
        if s.relation == Relation::Enemy {
            self.alert_until = self.alert_until.max(s.t + ALERT_FOR);
        }
        // A new user id in the slot is a different player.
        self.tracks.retain(|t| t.who.slot != s.who.slot || t.who == s.who);
        match self.tracks.iter_mut().find(|t| t.who == s.who) {
            Some(t) => t.observe(s),
            None => self.tracks.push(EnemyTrack::new(s)),
        }
    }

    pub fn on_cue(&mut self, c: &AnonymousCue) {
        let bearing = dmath::atan2(c.dir.y, c.dir.x).to_degrees();
        self.push_hypothesis(Hypothesis {
            id: 0,
            kind: HypothesisKind::Cue,
            t: c.t,
            pos: Some(c.pos),
            bearing,
            bearing_sigma: 5.0,
            weapon: None,
            strength: 0.4,
            track: None,
        });
    }

    pub fn on_sound(&mut self, s: &SoundStimulus) {
        let lateral = s.range * dmath::tan(s.bearing_sigma.to_radians());
        let sigma = (lateral * lateral + (RANGE_ERROR * s.range).powi(2)).sqrt();
        let memory = self.memory;
        let fits = |t: &EnemyTrack| {
            t.relation == Relation::Enemy
                && t.state != TrackState::Visible
                && t.sigma <= ASSOCIATE_MAX_SIGMA
                && s.t.since(t.last_seen) <= memory
                && t.pos.distance(s.pos) <= 2.0 * (t.sigma * t.sigma + sigma * sigma).sqrt()
                && !matches!((s.weapon, t.traits.weapon), (Some(a), Some(b)) if a != b)
        };
        let mut fitting = self.tracks.iter().enumerate().filter(|(_, t)| fits(t)).map(|(i, _)| i);
        let only = match (fitting.next(), fitting.next()) {
            (Some(i), None) => Some(i),
            _ => None,
        };
        let mut track = None;
        if let Some(i) = only {
            let t = &mut self.tracks[i];
            t.fuse(s.pos, sigma, s.t);
            if s.kind == SoundKind::Shot {
                t.traits.fired_at = Some(s.t);
            }
            track = Some(t.who);
        }
        self.push_hypothesis(Hypothesis {
            id: 0,
            kind: HypothesisKind::Sound(s.kind),
            t: s.t,
            pos: Some(s.pos),
            bearing: s.bearing,
            bearing_sigma: s.bearing_sigma,
            weapon: s.weapon,
            strength: s.gain,
            track,
        });
    }

    pub fn on_damage(&mut self, d: &DamageStimulus) {
        self.alert_until = self.alert_until.max(d.t + ALERT_FOR);
        self.last_damage = Some(*d);
        if let Some(bearing) = d.bearing {
            self.push_hypothesis(Hypothesis {
                id: 0,
                kind: HypothesisKind::Damage,
                t: d.t,
                pos: None,
                bearing,
                bearing_sigma: 15.0,
                weapon: None,
                strength: (d.amount as f32 / 100.0).clamp(0.05, 1.0),
                track: None,
            });
        }
    }

    pub fn on_public(&mut self, e: &PublicEvent) {
        match e {
            PublicEvent::Death { victim, .. } => self.tracks.retain(|t| t.who.slot != *victim),
        }
    }

    /// Forgets the bot's own fights when it dies; tracks stay and age.
    pub fn on_own_death(&mut self) {
        self.hypotheses.clear();
        self.last_damage = None;
        self.alert_until = SimTime::ZERO;
    }

    pub fn hypothesis(&self, id: u32) -> Option<&Hypothesis> {
        self.hypotheses.iter().find(|h| h.id == id)
    }

    fn push_hypothesis(&mut self, mut h: Hypothesis) {
        h.id = self.next_hypothesis;
        self.next_hypothesis = self.next_hypothesis.wrapping_add(1).max(1);
        if self.hypotheses.len() >= MAX_HYPOTHESES {
            let weakest = self
                .hypotheses
                .iter()
                .enumerate()
                .min_by(|a, b| (a.1.strength, a.1.t.0).partial_cmp(&(b.1.strength, b.1.t.0)).unwrap())
                .map(|(i, _)| i);
            if let Some(i) = weakest {
                self.hypotheses.swap_remove(i);
            }
        }
        self.hypotheses.push(h);
    }

    /// Ages tracks and hypotheses to `now`.
    pub fn update(&mut self, now: SimTime, p: &BeliefParams) {
        self.memory = f64::from(p.track_forget);
        self.tracks.retain_mut(|t| t.age_to(now, p));
        self.hypotheses.retain(|h| {
            let life = match h.kind {
                HypothesisKind::Sound(_) => SOUND_LIFE,
                HypothesisKind::Damage => DAMAGE_LIFE,
                HypothesisKind::Cue => CUE_LIFE,
            };
            now.since(h.t) <= life
        });
    }

    /// Spreads every enemy out of sight over the places it may be at by now, starting over from each new fix (a
    /// sighting or a sound tied to it); `horizon`: how many seconds' run from the fix it is looked for.
    pub fn spread(&mut self, now: SimTime, map: &dyn MapView, watch: Option<&Watch>, horizon: f32) {
        for t in self.tracks.iter_mut().filter(|t| t.relation == Relation::Enemy) {
            // In sight, or forgotten (nobody looks for it any more).
            if matches!(t.state, TrackState::Visible | TrackState::Stale) {
                t.spread = None;
                continue;
            }
            match t.spread.as_mut() {
                // Steps heard every few tenths of a second each move the fix: twice a second is enough.
                Some(s) if s.fresh(now) => {}
                Some(s) if s.lost_at == t.fix_t => s.update(now, map, watch),
                _ => {
                    let vel = (t.fix_t == t.last_seen && t.vel != Vec3::ZERO).then_some(t.vel);
                    t.spread = Spread::new(map, t.fix_pos, vel, t.fix_t, horizon, now, watch).map(Box::new);
                }
            }
        }
    }

    /// In a fight: an enemy was recognized or damage taken within the last 3 s.
    pub fn alert(&self, now: SimTime) -> bool {
        now < self.alert_until
    }

    /// A sound or the damage compass pointed within 30° of `bearing` in the last 2 s.
    pub fn primed(&self, bearing: f32, now: SimTime) -> bool {
        self.hypotheses.iter().any(|h| {
            matches!(h.kind, HypothesisKind::Sound(_) | HypothesisKind::Damage)
                && now.since(h.t) <= PRIMED_FOR
                && angle_diff(bearing, h.bearing).abs() <= PRIMED_ANGLE
        })
    }

    pub fn enemies(&self) -> impl Iterator<Item = &EnemyTrack> {
        self.tracks.iter().filter(|t| t.relation == Relation::Enemy)
    }

    /// Enemies in sight now.
    pub fn visible_enemies(&self) -> impl Iterator<Item = &EnemyTrack> {
        self.enemies().filter(|t| t.state == TrackState::Visible)
    }

    /// Enemies believed within `radius` of `p`: in sight, or lost at most 0.5 s ago with a tight estimate.
    pub fn enemies_near(&self, p: Vec3, radius: f32, now: SimTime) -> usize {
        self.enemies()
            .filter(|t| t.state == TrackState::Visible || (t.age(now) <= 0.5 && t.sigma < 60.0))
            .filter(|t| t.pos.distance(p) <= radius)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: BeliefParams = BeliefParams {
        track_forget: 8.0,
        maxspeed: 300.0,
    };

    fn sighting(slot: u8, t: f64, pos: Vec3, first: bool) -> Sighting {
        Sighting {
            who: PlayerKey {
                slot,
                userid: 10 + slot as i32,
            },
            relation: Relation::Enemy,
            t: SimTime(t),
            pos,
            sigma: 2.0,
            distance: 500.0,
            visibility: 1.0,
            parts: parts::CHEST | parts::HEAD,
            stance: Stance::Standing,
            on_ground: true,
            on_ladder: false,
            in_water: false,
            facing: 0.0,
            weapon: Some(WeaponId::Glock),
            firing: false,
            render: RenderCue::default(),
            first,
            noticed_at: SimTime(t),
        }
    }

    #[test]
    fn tracks_follow_sightings_and_fade() {
        let mut b = Beliefs::default();
        for i in 0..10 {
            let t = i as f64 * 0.05;
            b.on_sighting(&sighting(3, t, Vec3::new(300.0 * t as f32, 0.0, 0.0), i == 0));
            b.update(SimTime(t), &P);
        }
        let tr = b.track_by_slot(3).unwrap();
        assert_eq!(tr.state, TrackState::Visible);
        assert!((tr.vel.x - 300.0).abs() < 80.0, "velocity converges: {:?}", tr.vel);
        let last = tr.pos;
        b.update(SimTime(0.45 + 0.2), &P);
        let tr = b.track_by_slot(3).unwrap();
        assert_eq!(tr.state, TrackState::RecentlyLost);
        assert!(tr.pos.x > last.x, "extrapolated along the velocity");
        b.update(SimTime(0.45 + 3.0), &P);
        let tr = b.track_by_slot(3).unwrap();
        assert_eq!(tr.state, TrackState::Predicted);
        assert!(tr.sigma > 500.0, "uncertainty grows: {}", tr.sigma);
        b.update(SimTime(0.45 + 9.0), &P);
        assert_eq!(b.track_by_slot(3).unwrap().state, TrackState::Stale);
        b.update(SimTime(0.45 + 31.0), &P);
        assert!(b.track_by_slot(3).is_none(), "dropped after 30 s");
    }

    #[test]
    fn a_sound_joins_the_only_track_that_fits() {
        let mut b = Beliefs::default();
        b.on_sighting(&sighting(3, 0.0, Vec3::new(1000.0, 0.0, 0.0), true));
        b.on_sighting(&sighting(4, 0.0, Vec3::new(-1000.0, 0.0, 0.0), true));
        b.update(SimTime(2.0), &P);
        let heard = SoundStimulus {
            t: SimTime(2.0),
            kind: SoundKind::Shot,
            weapon: Some(WeaponId::Glock),
            pos: Vec3::new(1100.0, 100.0, 0.0),
            bearing: 5.0,
            bearing_sigma: 20.0,
            range: 1100.0,
            gain: 0.2,
        };
        b.on_sound(&heard);
        let tr = b.track_by_slot(3).unwrap();
        assert_eq!(tr.last_heard, Some(SimTime(2.0)));
        assert!(tr.sigma < 360.0, "the fix narrows the estimate: {}", tr.sigma);
        assert_eq!(b.hypotheses.last().unwrap().track, Some(tr.who));
        assert!(b.track_by_slot(4).unwrap().last_heard.is_none());
        assert!(b.primed(5.0, SimTime(2.5)) && !b.primed(90.0, SimTime(2.5)));
        // Long lost: too uncertain to claim the sound.
        b.update(SimTime(9.0), &P);
        let late = SoundStimulus {
            t: SimTime(9.0),
            pos: b.track_by_slot(3).unwrap().pos,
            ..heard
        };
        b.on_sound(&late);
        assert_eq!(b.track_by_slot(3).unwrap().last_heard, Some(SimTime(2.0)));
    }

    #[test]
    fn deaths_close_tracks_and_new_userids_replace_them() {
        let mut b = Beliefs::default();
        b.on_sighting(&sighting(3, 0.0, Vec3::ZERO, true));
        let mut other = sighting(3, 1.0, Vec3::ZERO, true);
        other.who.userid = 99;
        b.on_sighting(&other);
        assert_eq!(b.tracks.len(), 1);
        assert_eq!(b.tracks[0].who.userid, 99);
        b.on_public(&PublicEvent::Death {
            t: SimTime(2.0),
            killer: Some(1),
            victim: 3,
            weapon: "9mmhandgun".into(),
        });
        assert!(b.tracks.is_empty());
    }
}
