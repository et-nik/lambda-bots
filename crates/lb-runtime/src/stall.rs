//! Watching for bots that look stuck, for the log and `lb stats`: an enemy close in front and in plain sight the bot
//! has not seen; an enemy it sees and neither shoots nor throws at; standing still for a while. This looks at where
//! the server has every player, which the bots never do: nothing here reaches their decisions.

use lb_core::math::view_angle_vectors;
use lb_core::time::SimTime;
use lb_core::{Vec2, Vec3};
use lb_knowledge::{PlayerKey, TrackState};
use lb_motor::Prio;
use lb_perception::Subject;
use lb_worldq::{HullKind, TraceQuery, Tracer};
use smallvec::SmallVec;

/// Looked at this often.
const PERIOD: f64 = 0.1;
/// An enemy this close, this near the middle of the view (degrees) and in the line of sight of the bot's eye...
const UNSEEN_NEAR: f32 = 500.0;
const UNSEEN_CONE: f32 = 30.0;
/// ... and not seen for this long is a stall.
const UNSEEN_FOR: f64 = 0.6;
/// A target in sight neither shot nor thrown at this long (longer for a slow gun: its cycle and a click's pause,
/// and the reload of a one-round clip).
const IDLE_FOR: f64 = 1.0;
const CLICK_PAUSE: f32 = 0.6;
/// Why the bot held fire is remembered this long.
const WHY_WINDOW: f64 = 5.0;
/// Standing still this long, slower than this.
const STILL_FOR: f64 = 2.0;
const STILL_SPEED: f32 = 20.0;
/// The way a still bot is asked to go is looked along this far with its body, and again a step higher.
const PROBE: f32 = 24.0;
const STEP: f32 = 18.0;
/// Player edicts come first, this many at most.
const MAX_PLAYERS: u32 = 32;

/// Stalls of one kind by cause: how many began, and the seconds they lasted.
#[derive(Clone, Debug, Default)]
pub struct Causes(pub Vec<(String, u32, f64)>);

impl Causes {
    fn begin(&mut self, cause: &str) {
        match self.0.iter_mut().find(|(c, _, _)| c == cause) {
            Some(e) => e.1 += 1,
            None => self.0.push((cause.to_string(), 1, 0.0)),
        }
    }

    fn last(&mut self, cause: &str, secs: f64) {
        if let Some(e) = self.0.iter_mut().find(|(c, _, _)| c == cause) {
            e.2 += secs;
        }
    }

    pub fn add(&mut self, other: &Causes) {
        for (cause, n, secs) in &other.0 {
            match self.0.iter_mut().find(|(c, _, _)| c == cause) {
                Some(e) => {
                    e.1 += n;
                    e.2 += secs;
                }
                None => self.0.push((cause.clone(), *n, *secs)),
            }
        }
    }

    pub fn total(&self) -> (u32, f64) {
        self.0.iter().fold((0, 0.0), |(n, s), (_, k, t)| (n + k, s + t))
    }

    pub fn line(&self) -> String {
        let mut rows = self.0.clone();
        rows.sort_by(|a, b| b.2.total_cmp(&a.2));
        rows.iter()
            .map(|(c, n, s)| format!("{c} ×{n} {s:.1} s"))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// What the watch counted.
#[derive(Clone, Debug, Default)]
pub struct Stalls {
    pub unseen: Causes,
    pub idle: Causes,
    pub still: Causes,
}

impl Stalls {
    pub fn add(&mut self, o: &Stalls) {
        self.unseen.add(&o.unseen);
        self.idle.add(&o.idle);
        self.still.add(&o.still);
    }
}

/// A stall under way: since when, and its cause once it lasted long enough to count.
#[derive(Clone, Debug)]
struct Episode {
    since: SimTime,
    cause: Option<String>,
    /// Why the bot held fire on each look so far, and for how long.
    why: SmallVec<[(&'static str, f64); 4]>,
}

impl Episode {
    fn new(since: SimTime) -> Episode {
        Episode {
            since,
            cause: None,
            why: SmallVec::new(),
        }
    }

    fn note(&mut self, why: &'static str) {
        match self.why.iter_mut().find(|(w, _)| *w == why) {
            Some(e) => e.1 += PERIOD,
            None => self.why.push((why, PERIOD)),
        }
    }

    /// The reason held most of the time.
    fn main_why(&self) -> &'static str {
        self.why
            .iter()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map_or("no reason noted", |(w, _)| w)
    }
}

#[derive(Clone, Debug, Default)]
pub struct StallWatch {
    next: SimTime,
    unseen: SmallVec<[(PlayerKey, Episode); 4]>,
    idle: Option<(PlayerKey, Episode)>,
    still: Option<Episode>,
    fired_at: SimTime,
    /// The target in sight, since when.
    target: Option<(PlayerKey, SimTime)>,
    /// Why the bot held fire on each look of the last `WHY_WINDOW` seconds with a target in sight.
    whys: SmallVec<[(SimTime, &'static str); 64]>,
    pub counts: Stalls,
}

/// The bot as the watch looks at it.
pub struct Sample<'a> {
    pub now: SimTime,
    pub name: &'a str,
    pub key: PlayerKey,
    pub team: u8,
    pub origin: Vec3,
    pub ducked: bool,
    pub eye: Vec3,
    pub view: Vec3,
    /// The view's field of view, degrees; zero for the default.
    pub fov: f32,
    pub speed: f32,
    pub weapon: Option<lb_game::weapons::WeaponId>,
    /// The attack buttons went out since the last look.
    pub fired: bool,
    pub brain: &'a lb_brain::BotBrain,
    /// What navigation does now.
    pub nav_phase: &'static str,
}

impl StallWatch {
    /// A new life: nothing under way.
    pub fn reset(&mut self) {
        let counts = std::mem::take(&mut self.counts);
        *self = StallWatch {
            counts,
            ..StallWatch::default()
        };
    }

    pub fn tick(&mut self, s: &Sample<'_>, others: &[Subject<'_>], tracer: &mut dyn Tracer) {
        if s.fired {
            self.fired_at = s.now;
        }
        if s.now < self.next {
            return;
        }
        self.next = s.now + PERIOD;
        self.unseen_enemies(s, others, tracer);
        self.idle_target(s, others);
        self.standing(s, tracer);
    }

    fn unseen_enemies(&mut self, s: &Sample<'_>, others: &[Subject<'_>], tracer: &mut dyn Tracer) {
        let (forward, _, _) = view_angle_vectors(s.view);
        // A zoomed view sees only what is in the scope.
        let zoomed = s.fov > 0.0 && s.fov < 89.0;
        let cone = if zoomed {
            UNSEEN_CONE.min(0.5 * s.fov)
        } else {
            UNSEEN_CONE
        };
        let cone = lb_core::dmath::cos(cone.to_radians());
        let mut in_front: SmallVec<[(PlayerKey, f32, f32, &Subject<'_>); 4]> = SmallVec::new();
        for o in others {
            if o.key == s.key || !o.can_be_seen() || (s.team != 0 && o.team == s.team) {
                continue;
            }
            let chest = o.raw.origin + Vec3::Z * 8.0;
            let to = chest - s.eye;
            let d = to.length();
            let dot = forward.dot(to / d.max(1.0));
            if d > UNSEEN_NEAR || dot < cone {
                continue;
            }
            let seen = s
                .brain
                .beliefs
                .track(o.key)
                .is_some_and(|t| t.state == TrackState::Visible);
            if seen {
                continue;
            }
            let mut q = TraceQuery::line(s.eye, chest);
            q.ignore_glass = true;
            if tracer.trace(&q).fraction < 1.0 {
                continue;
            }
            // Another player in between hides it, from the bots as from anyone.
            let sight = tracer.trace(&TraceQuery::sight(s.eye, chest, u16::from(s.key.slot)));
            if sight
                .hit
                .is_some_and(|h| (1..=MAX_PLAYERS).contains(&h) && h != u32::from(o.key.slot))
            {
                continue;
            }
            in_front.push((o.key, d, lb_core::dmath::acos(dot.clamp(-1.0, 1.0)).to_degrees(), o));
        }
        // Episodes of enemies no longer in front unseen end.
        let ended: SmallVec<[(PlayerKey, Episode); 4]> = self
            .unseen
            .iter()
            .filter(|(k, _)| !in_front.iter().any(|(o, _, _, _)| o == k))
            .cloned()
            .collect();
        for (k, e) in ended {
            if let Some(cause) = &e.cause {
                self.counts.unseen.last(cause, s.now.since(e.since));
            }
            self.unseen.retain(|(o, _)| *o != k);
        }
        for (key, d, off, o) in in_front {
            let e = match self.unseen.iter_mut().find(|(k, _)| *k == key) {
                Some((_, e)) => e,
                None => {
                    self.unseen.push((key, Episode::new(s.now)));
                    &mut self.unseen.last_mut().expect("just pushed").1
                }
            };
            if e.cause.is_some() || s.now.since(e.since) < UNSEEN_FOR {
                continue;
            }
            let contact = s.brain.perception.vision.contacts.iter().find(|c| c.who == key);
            let (cause, detail) = match contact {
                Some(c) if !c.recognized => (
                    "recognizing",
                    format!(
                        "evidence {:.2} in {:.1} s (recognized at 1 in {:.2} s at full rate)",
                        c.evidence,
                        s.now.since(c.first_evidence),
                        c.delay
                    ),
                ),
                Some(_) => ("recognized, not believed in sight", String::new()),
                None => ("no contact", String::new()),
            };
            let (speed, ducked) = (o.raw.velocity.length(), o.raw.flags & (1 << 14) != 0);
            tracing::info!(
                "stall: {} does not see an enemy {d:.0} units away, {off:.0}° off its view, in plain sight for {:.1} \
                 s: {cause} {detail}; the enemy moves at {speed:.0}{}",
                s.name,
                s.now.since(e.since),
                if ducked { ", ducked" } else { "" }
            );
            self.counts.unseen.begin(cause);
            e.cause = Some(cause.to_string());
        }
    }

    fn idle_target(&mut self, s: &Sample<'_>, others: &[Subject<'_>]) {
        let m = &s.brain.mind;
        let target = m.target.and_then(|k| {
            s.brain
                .beliefs
                .track(k)
                .filter(|t| t.state == TrackState::Visible)
                .map(|t| (k, t.pos))
        });
        match (target, self.target) {
            (Some((k, _)), Some((held, _))) if k == held => {}
            (Some((k, _)), _) => self.target = Some((k, s.now)),
            (None, _) => self.target = None,
        }
        // A one-round weapon between its shots reloads; a slow one cycles, a clicked one pauses.
        let quiet = s.weapon.map_or(IDLE_FOR, |w| {
            let spec = lb_game::mechanics::spec(w);
            let reload = if spec.clip == 1 { spec.reload } else { 0.0 };
            let click = if spec.trigger == lb_game::mechanics::Trigger::Tap {
                CLICK_PAUSE
            } else {
                0.0
            };
            IDLE_FOR.max(f64::from(spec.cycle + click + reload) + 0.3)
        });
        if target.is_some() {
            let controller = &s.brain.motor.weapon;
            // The fight's reason first; with none, whoever has the weapon channel, and why the trigger stayed off.
            let why = match (m.hold_fire, s.brain.intents.weapon) {
                (Some("its weapon is not out yet"), _) if controller.refused(s.now).is_some() => {
                    "the game would not draw the weapon"
                }
                (Some(why), _) => why,
                (None, Some((Prio::Protocol, _))) => "a weapon protocol has the weapon",
                (None, Some((prio, _))) if prio > Prio::Protocol => "a traversal has the weapon",
                (None, Some(_)) => controller.quiet.unwrap_or("fired"),
                (None, None) => "nothing asks for the weapon",
            };
            self.whys.retain(|(t, _)| s.now.since(*t) <= WHY_WINDOW);
            self.whys.push((s.now, if s.fired { "fired" } else { why }));
        }
        let busy = m.arms.active.as_ref().map(|a| a.name());
        let engaging = s.now.since(self.fired_at) < quiet || busy.is_some_and(|b| b != "scope");
        let Some((key, pos)) = target.filter(|_| !engaging) else {
            if let Some((_, e)) = self.idle.take()
                && let Some(cause) = &e.cause
            {
                self.counts.idle.last(cause, s.now.since(e.since));
            }
            return;
        };
        match &mut self.idle {
            Some((k, _)) if *k == key => {}
            _ => {
                if let Some((_, e)) = self.idle.take()
                    && let Some(cause) = &e.cause
                {
                    self.counts.idle.last(cause, s.now.since(e.since));
                }
                let sighted = self.target.map_or(s.now, |(_, t)| t);
                self.idle = Some((key, Episode::new(self.fired_at.max(sighted))));
            }
        }
        let Some((_, e)) = self.idle.as_mut() else { return };
        if e.cause.is_some() || s.now.since(e.since) < quiet {
            return;
        }
        let since = e.since;
        for &(_, why) in self.whys.iter().filter(|(t, w)| *t >= since && *w != "fired") {
            e.note(why);
        }
        let cause = e.main_why();
        let weapon = s.weapon.map_or("none", |w| w.classname());
        let chosen = m.choice.map_or("none", |c| c.weapon().classname());
        let really = others
            .iter()
            .find(|o| o.key == key)
            .is_some_and(|o| o.raw.origin.distance(pos) < 64.0);
        let whys: Vec<String> = e.why.iter().map(|(w, t)| format!("{w} {t:.1} s")).collect();
        tracing::info!(
            "stall: {} does not fight an enemy in sight {:.0} units away for {:.1} s: {}; {weapon} in hand, {chosen} \
             chosen, goal {}, navigation {}{}",
            s.name,
            pos.distance(s.eye),
            s.now.since(e.since),
            whys.join(", "),
            m.goal.map_or("none", |g| g.kind.as_str()),
            s.nav_phase,
            if really {
                ""
            } else {
                "; the enemy is not where it is believed"
            }
        );
        self.counts.idle.begin(cause);
        e.cause = Some(cause.to_string());
    }

    fn standing(&mut self, s: &Sample<'_>, tracer: &mut dyn Tracer) {
        if s.speed >= STILL_SPEED {
            if let Some(e) = self.still.take()
                && let Some(cause) = &e.cause
            {
                self.counts.still.last(cause, s.now.since(e.since));
            }
            return;
        }
        let e = self.still.get_or_insert(Episode::new(s.now));
        if e.cause.is_some() || s.now.since(e.since) < STILL_FOR {
            return;
        }
        let m = &s.brain.mind;
        let intents = &s.brain.intents;
        let mover = intents.movement.map_or("nobody".to_string(), |(p, mv)| {
            let way = if mv.speed > 0.0 && mv.dir != Vec2::ZERO {
                blocked_way(s, mv.dir, tracer)
            } else {
                String::new()
            };
            format!("{p:?} at {:.0}{way}", mv.speed)
        });
        let cause = match (&m.arms.active, m.goal) {
            (Some(a), _) => format!("protocol {}", a.name()),
            (None, Some(g)) => format!("goal {}", g.kind.as_str()),
            (None, None) => "no goal".to_string(),
        };
        let fight = m
            .target
            .and_then(|k| s.brain.beliefs.track(k))
            .map_or("no target".to_string(), |t| {
                format!("target {:?} {:.0} units away", t.state, t.pos.distance(s.eye))
            });
        tracing::info!(
            "stall: {} stands still for {:.1} s: {cause}, the movement asked by {mover}, navigation {}, {fight}, task \
             {}",
            s.name,
            s.now.since(e.since),
            s.nav_phase,
            m.task.as_ref().map_or("none", |t| t.name())
        );
        self.counts.still.begin(&cause);
        e.cause = Some(cause);
    }
}

/// What stops a still bot going along `dir` with its body: a player, a wall or a box too high to step onto.
fn blocked_way(s: &Sample<'_>, dir: Vec2, tracer: &mut dyn Tracer) -> String {
    let hull = if s.ducked { HullKind::Crouch } else { HullKind::Stand };
    let ahead = (dir.normalize() * PROBE).extend(0.0);
    let probe = |tracer: &mut dyn Tracer, from: Vec3| {
        let mut q = TraceQuery::hull(from, from + ahead, hull);
        q.ignore_monsters = false;
        q.ignore = Some(u16::from(s.key.slot));
        tracer.trace(&q)
    };
    let low = probe(tracer, s.origin);
    if !low.blocked() {
        return ", the way open".to_string();
    }
    let what = match low.hit {
        Some(n) if (1..=MAX_PLAYERS).contains(&n) => "a player",
        Some(0) => "the world",
        Some(_) => "a brush entity",
        None => "something",
    };
    if !probe(tracer, s.origin + Vec3::Z * STEP).blocked() {
        return format!(", {what} a step high {:.0} units ahead", low.fraction * PROBE);
    }
    format!(", {what} in the way {:.0} units ahead", low.fraction * PROBE)
}
