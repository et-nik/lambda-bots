//! Where a bot looks when something calls for it: a recognized enemy, the damage compass, an unrecognized glimpse,
//! an enemy lost a moment ago, a shot or a cry of pain nearby, where a lost enemy would come into view, a sound, and,
//! with nothing going on at a place where bots got hurt before, where that came from. Stands in for the vigilance
//! layer and the combat look until the arbiter arrives; the path follower looks along the path otherwise.
//!
//! Something moving where nobody is known to be draws the eyes at once, as it would a player's: that is how the bot
//! gets a glimpse into the middle of its view, where it is recognized soonest. So does a shot or a cry of pain close
//! by, once a second at most. Otherwise a bot on the move does not turn its head at every noise: only at sounds near
//! enough to matter, not already in front of it, and with a few quiet seconds between glances. A sound's height is a
//! guess, so the glance at it stays near level.

use lb_core::Vec3;
use lb_core::math::angle_diff;
use lb_core::time::SimTime;
use lb_game::sounds::SoundKind;
use lb_knowledge::{Beliefs, HypothesisKind, PlayerKey, Relation, TrackState};

use crate::BotBrain;
use lb_core::dmath;

const CHEST: f32 = 8.0;
const DAMAGE_LOOK_FOR: f64 = 1.0;
const GLANCE_FOR: f64 = 0.8;
/// A newer glimpse takes the eyes from a glance at an older one after this long.
const GLIMPSE_KEEP: f64 = 0.25;
/// A shot or a cry of pain this near draws the eyes at once, no more than once in `ALARM_GAP`.
const ALARM_SHOT: f32 = 1000.0;
const ALARM_PAIN: f32 = 600.0;
const ALARM_GAP: f64 = 1.0;
/// A teammate in sight this near a shot or a cry is taken for its source: no alarm.
const FRIEND_NEAR: f32 = 300.0;
/// Quiet time after a glance before a sound or a glimpse draws the eyes again: this, up to twice as long.
const GLANCE_GAP: f64 = 2.5;
const FRESH: f64 = 0.5;
/// A sound this near the view's heading is in front of the bot already, degrees.
const IN_VIEW: f32 = 40.0;
/// Steepest glance up or down at a sound: the tangent of about 15°.
const SOUND_TILT: f32 = 0.27;
/// No enemy seen for this long before a glance where danger comes from.
const DANGER_CALM: f64 = 4.0;

/// A sound of `kind` about `distance` away is worth a glance.
fn worth_a_look(kind: SoundKind, distance: f32) -> bool {
    let reach = match kind {
        SoundKind::Shot => 1500.0,
        SoundKind::Pain => 1000.0,
        SoundKind::Step | SoundKind::Jump | SoundKind::Pickup => 700.0,
        SoundKind::WeaponNoise => 500.0,
        _ => 0.0,
    };
    distance <= reach
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LookReason {
    Enemy(PlayerKey),
    Damage,
    Lost(PlayerKey),
    /// Where a lost enemy would come into view.
    Expect(PlayerKey),
    /// Where bots at this place were mostly hurt from.
    Danger,
    Glimpse,
    Sound(SoundKind),
}

impl LookReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            LookReason::Enemy(_) => "enemy",
            LookReason::Damage => "damage",
            LookReason::Lost(_) => "lost",
            LookReason::Expect(_) => "expect",
            LookReason::Danger => "danger",
            LookReason::Glimpse => "glimpse",
            LookReason::Sound(_) => "sound",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Attention {
    pub point: Vec3,
    pub reason: LookReason,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Glance {
    /// The glance under way and when it ends.
    current: Option<(Attention, SimTime)>,
    quiet_until: SimTime,
    /// The last glimpse (a hypothesis id) looked at.
    glimpsed: Option<u32>,
    alarm_until: SimTime,
}

impl Glance {
    fn start(&mut self, a: Attention, now: SimTime) {
        // The gap varies from glance to glance without a random stream: by the time of the glance.
        let jitter = (now.secs() * 7.31).fract();
        self.current = Some((a, now + GLANCE_FOR));
        self.quiet_until = now + GLANCE_FOR + GLANCE_GAP * (1.0 + jitter);
    }
}

fn toward(eye: Vec3, bearing: f32) -> Vec3 {
    let (s, c) = dmath::sin_cos(bearing.to_radians());
    eye + Vec3::new(c, s, 0.0) * 300.0
}

/// A look at `p` from `eye`, tilted no more than `SOUND_TILT`.
fn level(eye: Vec3, p: Vec3) -> Vec3 {
    let d = p - eye;
    let flat = d.truncate().length();
    eye + d.truncate().extend(d.z.clamp(-flat * SOUND_TILT, flat * SOUND_TILT))
}

pub(crate) fn pick(brain: &mut BotBrain, now: SimTime, eye: Vec3) -> Option<Attention> {
    let yaw = brain.motor.view.y;
    let b = &brain.beliefs;
    let nearest = |state: TrackState| {
        b.enemies()
            .filter(move |t| t.state == state)
            .min_by(|x, y| x.pos.distance(eye).total_cmp(&y.pos.distance(eye)))
    };
    if let Some(t) = nearest(TrackState::Visible) {
        brain.glance.current = None;
        return Some(Attention {
            point: t.pos + Vec3::Z * CHEST,
            reason: LookReason::Enemy(t.who),
        });
    }
    if let Some(d) = b.last_damage.filter(|d| now.since(d.t) <= DAMAGE_LOOK_FOR)
        && let Some(bearing) = d.bearing
    {
        return Some(Attention {
            point: toward(eye, bearing),
            reason: LookReason::Damage,
        });
    }
    // A glimpse not looked at yet draws the eyes at once; a glance at an older one gives way after a moment.
    let glancing = brain.glance.current.filter(|(_, until)| now < *until);
    let at_glimpse = glancing.filter(|(a, _)| a.reason == LookReason::Glimpse);
    let fresh = b
        .hypotheses
        .iter()
        .filter(|h| h.kind == HypothesisKind::Cue && now.since(h.t) <= FRESH && Some(h.id) != brain.glance.glimpsed)
        .max_by(|x, y| x.t.0.total_cmp(&y.t.0))
        .and_then(|h| Some((h.id, h.pos?)));
    if let Some((id, point)) = fresh
        && at_glimpse.is_none_or(|(_, until)| now >= until + (GLIMPSE_KEEP - GLANCE_FOR))
    {
        let a = Attention {
            point,
            reason: LookReason::Glimpse,
        };
        brain.glance.glimpsed = Some(id);
        brain.glance.start(a, now);
        return Some(a);
    }
    if let Some((a, _)) = at_glimpse {
        return Some(a);
    }
    if let Some(t) = nearest(TrackState::RecentlyLost) {
        return Some(Attention {
            point: t.pos + Vec3::Z * CHEST,
            reason: LookReason::Lost(t.who),
        });
    }
    if let Some((a, _)) = glancing {
        return Some(a);
    }
    brain.glance.current = None;
    if now >= brain.glance.alarm_until
        && let Some(a) = alarm(b, now, eye, yaw)
    {
        brain.glance.alarm_until = now + ALARM_GAP;
        brain.glance.start(a, now);
        return Some(a);
    }
    if let Some((who, point)) = brain.expect {
        return Some(Attention {
            point,
            reason: LookReason::Expect(who),
        });
    }
    if now < brain.glance.quiet_until {
        return None;
    }
    let sound = || {
        b.hypotheses
            .iter()
            .filter(|h| now.since(h.t) <= FRESH)
            .filter_map(|h| match h.kind {
                HypothesisKind::Sound(k) => Some((h, k, h.pos.unwrap_or_else(|| toward(eye, h.bearing)))),
                _ => None,
            })
            .filter(|&(h, k, p)| worth_a_look(k, p.distance(eye)) && angle_diff(h.bearing, yaw).abs() > IN_VIEW)
            .max_by(|x, y| x.0.strength.total_cmp(&y.0.strength))
            .map(|(_, k, p)| Attention {
                point: level(eye, p),
                reason: LookReason::Sound(k),
            })
    };
    // With nothing going on, a look where the damage taken at this place came from before.
    let danger = || {
        let calm = now.since(brain.mind.last_enemy_seen()) >= DANGER_CALM;
        brain
            .danger
            .filter(|p| calm && angle_diff(dmath::atan2(p.y - eye.y, p.x - eye.x).to_degrees(), yaw).abs() > IN_VIEW)
            .map(|p| Attention {
                point: level(eye, p),
                reason: LookReason::Danger,
            })
    };
    let a = sound().or_else(danger)?;
    brain.glance.start(a, now);
    Some(a)
}

/// A fresh shot or cry of pain near enough to matter, not in front of the bot already and not a teammate's in sight:
/// worth a look at once.
fn alarm(b: &Beliefs, now: SimTime, eye: Vec3, yaw: f32) -> Option<Attention> {
    let friend_near = |p: Vec3| {
        b.tracks.iter().any(|t| {
            t.relation == Relation::Friend
                && matches!(t.state, TrackState::Visible | TrackState::RecentlyLost)
                && t.pos.distance(p) <= FRIEND_NEAR
        })
    };
    b.hypotheses
        .iter()
        .filter(|h| now.since(h.t) <= FRESH)
        .filter_map(|h| {
            let (kind, reach) = match h.kind {
                HypothesisKind::Sound(SoundKind::Shot) => (SoundKind::Shot, ALARM_SHOT),
                HypothesisKind::Sound(SoundKind::Pain) => (SoundKind::Pain, ALARM_PAIN),
                _ => return None,
            };
            let p = h.pos.unwrap_or_else(|| toward(eye, h.bearing));
            (p.distance(eye) <= reach && angle_diff(h.bearing, yaw).abs() > IN_VIEW && !friend_near(p))
                .then_some((h.strength, kind, p))
        })
        .max_by(|x, y| x.0.total_cmp(&y.0))
        .map(|(_, kind, p)| Attention {
            point: level(eye, p),
            reason: LookReason::Sound(kind),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_config::skill::Presets;
    use lb_knowledge::{AnonymousCue, RangeBin, RenderCue, Sighting, SoundStimulus, Stance, parts};
    use lb_perception::PerceptionParams;

    const EYE: Vec3 = Vec3::new(0.0, 0.0, 28.0);
    const LATER: PlayerKey = PlayerKey { slot: 3, userid: 103 };

    fn brain() -> BotBrain {
        BotBrain::new(1, PerceptionParams::from_skill(&Presets::default().at(50)))
    }

    fn cue(b: &mut BotBrain, t: f64, yaw: f32) {
        let (sin, cos) = dmath::sin_cos(yaw.to_radians());
        let dir = Vec3::new(cos, sin, 0.0);
        b.beliefs.on_cue(&AnonymousCue {
            t: SimTime(t),
            dir,
            range: RangeBin::Near,
            pos: EYE + dir * RangeBin::Near.distance(),
        });
    }

    fn sound(b: &mut BotBrain, t: f64, kind: SoundKind, at: Vec3) {
        let d = at - EYE;
        b.beliefs.on_sound(&SoundStimulus {
            t: SimTime(t),
            kind,
            weapon: None,
            pos: at,
            bearing: dmath::atan2(d.y, d.x).to_degrees(),
            bearing_sigma: 10.0,
            range: d.length(),
            gain: 0.5,
        });
    }

    fn teammate(b: &mut BotBrain, t: f64, at: Vec3) {
        b.beliefs.on_sighting(&Sighting {
            who: PlayerKey { slot: 4, userid: 104 },
            relation: Relation::Friend,
            t: SimTime(t),
            pos: at,
            sigma: 1.0,
            distance: at.distance(EYE),
            visibility: 1.0,
            parts: parts::CHEST,
            stance: Stance::Standing,
            on_ground: true,
            on_ladder: false,
            in_water: false,
            facing: 0.0,
            weapon: None,
            firing: true,
            render: RenderCue::default(),
            first: true,
            noticed_at: SimTime(t),
        });
    }

    fn reason(b: &mut BotBrain, t: f64) -> Option<LookReason> {
        pick(b, SimTime(t), EYE).map(|a| a.reason)
    }

    #[test]
    fn a_glimpse_draws_the_eyes_at_once_and_once() {
        let mut b = brain();
        sound(&mut b, 0.9, SoundKind::Step, Vec3::new(-400.0, 0.0, 0.0));
        assert_eq!(reason(&mut b, 1.0), Some(LookReason::Sound(SoundKind::Step)));
        // Where a lost enemy would come back is watched, and the quiet gap after the glance is on.
        b.expect = Some((LATER, Vec3::new(0.0, 500.0, 28.0)));
        cue(&mut b, 1.1, 90.0);
        assert_eq!(
            reason(&mut b, 1.1),
            Some(LookReason::Glimpse),
            "the glimpse beats the glance under way, the gap and the expected enemy"
        );
        assert_eq!(reason(&mut b, 1.3), Some(LookReason::Glimpse), "the glance at it holds");
        assert_eq!(
            reason(&mut b, 2.0),
            Some(LookReason::Expect(LATER)),
            "a glimpse looked at is not looked at again"
        );
    }

    #[test]
    fn a_newer_glimpse_takes_over_after_a_moment() {
        let mut b = brain();
        cue(&mut b, 1.0, 60.0);
        let first = pick(&mut b, SimTime(1.0), EYE).unwrap();
        cue(&mut b, 1.1, -60.0);
        assert_eq!(pick(&mut b, SimTime(1.1), EYE).unwrap(), first, "too soon to turn away");
        let second = pick(&mut b, SimTime(1.3), EYE).unwrap();
        assert_eq!(second.reason, LookReason::Glimpse);
        assert!(second.point.y < 0.0, "the newer one: {second:?}");
    }

    #[test]
    fn shots_close_by_are_looked_at_once_a_second() {
        let mut b = brain();
        // A glance at steps just started: its quiet gap would hold ordinary sounds back for seconds.
        sound(&mut b, 0.95, SoundKind::Step, Vec3::new(0.0, -300.0, 0.0));
        assert_eq!(reason(&mut b, 1.0), Some(LookReason::Sound(SoundKind::Step)));
        sound(&mut b, 1.9, SoundKind::Shot, Vec3::new(-800.0, 0.0, 0.0));
        assert_eq!(reason(&mut b, 1.9), Some(LookReason::Sound(SoundKind::Shot)));
        sound(&mut b, 2.8, SoundKind::Shot, Vec3::new(0.0, 700.0, 0.0));
        assert_eq!(
            reason(&mut b, 2.8),
            None,
            "the glance at the first ended, the next alarm waits a second"
        );
        assert_eq!(reason(&mut b, 2.95), Some(LookReason::Sound(SoundKind::Shot)));
        let mut far = brain();
        sound(&mut far, 1.0, SoundKind::Shot, Vec3::new(-1400.0, 0.0, 0.0));
        far.glance.quiet_until = SimTime(5.0);
        assert_eq!(reason(&mut far, 1.0), None, "a shot far off waits for the quiet gap");
    }

    #[test]
    fn a_teammate_firing_raises_no_alarm() {
        let mut b = brain();
        b.glance.quiet_until = SimTime(5.0);
        teammate(&mut b, 1.0, Vec3::new(-600.0, 0.0, 0.0));
        sound(&mut b, 1.0, SoundKind::Shot, Vec3::new(-650.0, 50.0, 0.0));
        assert_eq!(reason(&mut b, 1.0), None);
        sound(&mut b, 1.0, SoundKind::Shot, Vec3::new(0.0, 700.0, 0.0));
        assert_eq!(reason(&mut b, 1.0), Some(LookReason::Sound(SoundKind::Shot)));
    }
}
