//! Per-bot brain: schedule, perception to command pipeline, decision trace.

#![forbid(unsafe_code)]

pub mod arms;
pub mod attention;
pub mod goals;
pub mod mind;
pub mod patrol;
pub mod trail;
pub mod tricks;

use lb_core::Vec3;
use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use lb_knowledge::{
    BeliefParams, Beliefs, ChargerSpot, Chargers, DamageStimulus, EnemyTrack, Explosives, ItemSpot, Items, PlayerKey,
    PublicEvent, Relation, Watch,
};
use lb_motor::{Intents, Motor};
use lb_nav_api::{MapView, NodeId};
use lb_perception::items::{ChargerEntity, ItemEntity};
use lb_perception::projectiles::ProjectileEntity;
use lb_perception::vision::{PERIOD, VisionOutput};
use lb_perception::{Listener, Perception, PerceptionParams, SoundEvent, Subject, Viewer};
use lb_worldq::{Tracer, VisSets};

pub use attention::{Attention, LookReason};
pub use mind::{Body, Character, Mind, WeaponLike};

/// Item spots are looked at on every second look.
const ITEM_LOOK_EVERY: u64 = 2;
/// A lost enemy is looked for as far as it runs in this many seconds more than the bot remembers it.
const SPREAD_BEYOND: f32 = 2.0;
/// Where a lost enemy would come into view is looked for this long after losing it, twice a second.
const EXPECT_FOR: f64 = 8.0;
/// Enemies are expected from places in sight this far off when none is known.
const APPROACH_BAND: [f32; 2] = [350.0, 900.0];
const EXPECT_PERIOD: f64 = 0.5;
/// A place this dangerous (of the map's worst) makes a calm bot glance where the damage there came from.
const DANGER_GLANCE: f32 = 0.25;

/// Where the bot looks from, for what it has in sight.
#[derive(Clone, Copy, Debug)]
pub struct Eyes {
    pub origin: Vec3,
    pub eye: Vec3,
    /// View angles, degrees.
    pub view: Vec3,
    /// Half the horizontal field of view, degrees.
    pub half_fov: f32,
}

/// Everything a bot has perceived and believes, and when its senses run next.
#[derive(Clone, Debug)]
pub struct BotBrain {
    pub perception: Perception,
    pub beliefs: Beliefs,
    /// The map's items and what the bot believes about them; `None` until the map's spots are known.
    pub items: Option<Items>,
    /// The map's wall chargers and whether the bot believes they have anything left.
    pub chargers: Option<Chargers>,
    /// Its own satchels and mines, mines it has seen, projectiles it sees coming.
    pub explosives: Explosives,
    /// Where players spawn on this map (static map knowledge): no mines there.
    pub spawns: Vec<Vec3>,
    /// The map's places on ladders: no mine dropped at their foot, its beam would run up the ladder.
    pub ladders: Vec<Vec3>,
    pub params: PerceptionParams,
    /// Output of the latest vision tick.
    pub last_vision: VisionOutput,
    pub mind: Mind,
    pub motor: Motor,
    pub intents: Intents,
    /// What vigilance looked at on the last frame.
    pub last_attention: Option<Attention>,
    /// When the bot last had each place of the map in sight.
    pub watch: Watch,
    /// Where the bot has been, and where it goes on to with nothing to do where it stands.
    pub patrol: patrol::Patrol,
    /// Where a lost enemy would come into view, worked out twice a second.
    pub expect: Option<(PlayerKey, Vec3)>,
    /// Where an enemy would come into view when none is known: the busiest way into the bot's sight a few hundred
    /// units off, worked out twice a second.
    pub approach: Option<Vec3>,
    /// Where bots at this place were mostly hurt from, when it is a dangerous place (from the map's experience).
    pub danger: Option<Vec3>,
    next_expect: SimTime,
    /// An item spot to look at on every look (waiting for it to come back).
    pub item_focus: Option<usize>,
    /// Damage taken since the last frame, for the moods.
    pub(crate) hurt: bool,
    /// Slot of the player who killed the bot last, from the kill feed.
    pub last_killer: Option<u8>,
    slot: u8,
    item_cursor: usize,
    charger_cursor: usize,
    looks: u64,
    /// Offset of this bot's vision ticks within the period, so bots do not all look on the same frame.
    phase: f64,
    next_vision: Option<SimTime>,
    pub(crate) glance: attention::Glance,
}

impl BotBrain {
    pub fn new(slot: u8, params: PerceptionParams) -> BotBrain {
        BotBrain {
            perception: Perception::default(),
            beliefs: Beliefs::default(),
            items: None,
            chargers: None,
            explosives: Explosives::default(),
            spawns: Vec::new(),
            ladders: Vec::new(),
            params,
            last_vision: VisionOutput::default(),
            mind: Mind::default(),
            motor: Motor::default(),
            intents: Intents::default(),
            last_attention: None,
            watch: Watch::default(),
            patrol: patrol::Patrol::default(),
            expect: None,
            approach: None,
            danger: None,
            next_expect: SimTime::ZERO,
            item_focus: None,
            hurt: false,
            last_killer: None,
            slot,
            item_cursor: 0,
            charger_cursor: 0,
            looks: 0,
            phase: f64::from(slot % 32) / 32.0 * PERIOD,
            next_vision: None,
            glance: attention::Glance::default(),
        }
    }

    pub fn on_spawn(&mut self) {
        self.perception.reset();
        self.next_vision = None;
        self.glance = attention::Glance::default();
        self.mind.reset();
        self.motor.reset();
        self.patrol.on_spawn();
    }

    pub fn on_death(&mut self) {
        self.perception.reset();
        self.beliefs.on_own_death();
        self.explosives.on_own_death();
        self.last_vision.clear();
        self.mind.reset();
    }

    /// The map's item spots became known; `learned`: respawn times the server's bots timed before.
    pub fn set_items(&mut self, spots: &[ItemSpot], now: SimTime, learned: [lb_knowledge::Learned; 3]) {
        self.items = Some(Items::new(spots, now).with_learned(learned));
        self.item_cursor = 0;
    }

    /// The map's wall chargers became known.
    pub fn set_chargers(&mut self, spots: &[ChargerSpot]) {
        self.chargers = Some(Chargers::new(spots));
        self.charger_cursor = 0;
    }

    /// The navigation graph was replaced: where lost enemies may be, the places watched and the places been to were
    /// over its nodes.
    pub fn on_new_graph(&mut self) {
        self.beliefs.on_new_graph();
        self.watch = Watch::default();
        self.patrol.reset();
    }

    pub fn on_public(&mut self, e: &PublicEvent) {
        self.beliefs.on_public(e);
        match e {
            PublicEvent::Death { victim, killer, .. } => {
                self.perception.vision.forget(*victim);
                if self.mind.target.is_some_and(|k| k.slot == *victim) {
                    self.mind.wake_combat();
                }
                if *killer == Some(self.slot) {
                    self.mind.mood.on_kill();
                }
                if *victim == self.slot && killer.is_some_and(|k| k != self.slot) {
                    self.last_killer = *killer;
                }
            }
        }
    }

    pub fn on_damage(&mut self, d: &DamageStimulus) {
        self.beliefs.on_damage(d);
        self.hurt = true;
    }

    pub fn hear(&mut self, sounds: &[SoundEvent], listener: &Listener, vis: &dyn VisSets, rng: &mut Pcg32) {
        for ev in sounds {
            if let Some(s) = self.perception.hearing.hear(ev, listener, vis, &self.params, rng) {
                // A grenade bouncing is where it will go off, whoever threw it; it tells nothing of where a player is.
                if s.kind == lb_game::sounds::SoundKind::Bounce {
                    self.explosives.heard_bounce(s.pos, s.sigma(), s.t);
                    continue;
                }
                self.beliefs.on_sound(&s);
                if matches!(
                    s.kind,
                    lb_game::sounds::SoundKind::Pickup | lb_game::sounds::SoundKind::ItemRespawn
                ) && let Some(items) = self.items.as_mut()
                {
                    // Where an item sound came from is known to about a third of its distance.
                    items.heard(s.kind, s.pos, (0.35 * s.range).max(96.0), s.t);
                }
            }
        }
    }

    /// Runs vision when this bot's tick is due; returns whether it ran.
    #[allow(clippy::too_many_arguments)]
    pub fn see(
        &mut self,
        now: SimTime,
        viewer: &Viewer,
        subjects: &[Subject<'_>],
        vis: &dyn VisSets,
        tracer: &mut dyn Tracer,
        rng: &mut Pcg32,
    ) -> bool {
        let due = *self.next_vision.get_or_insert(now + self.phase);
        if now < due {
            return false;
        }
        let next = due + PERIOD;
        self.next_vision = Some(if next <= now { now + PERIOD } else { next });
        self.last_vision.clear();
        self.perception.vision.tick(
            now,
            viewer,
            subjects,
            &self.beliefs,
            vis,
            tracer,
            &self.params,
            rng,
            &mut self.last_vision,
        );
        for s in &self.last_vision.sightings {
            self.beliefs.on_sighting(s);
        }
        // An enemy just recognized is aimed at on this very frame.
        if self
            .last_vision
            .sightings
            .iter()
            .any(|s| s.first && s.relation == Relation::Enemy)
        {
            self.mind.wake_combat();
        }
        for c in &self.last_vision.cues {
            self.beliefs.on_cue(c);
        }
        self.looks += 1;
        true
    }

    /// Looks at item spots and chargers on some of the vision ticks; call right after [`BotBrain::see`] returned
    /// true.
    #[allow(clippy::too_many_arguments)]
    pub fn see_items(
        &mut self,
        now: SimTime,
        viewer: &Viewer,
        entities: &[ItemEntity],
        chargers: &[ChargerEntity],
        range: f32,
        vis: &dyn VisSets,
        tracer: &mut dyn Tracer,
    ) {
        if !self.looks.is_multiple_of(ITEM_LOOK_EVERY) {
            return;
        }
        if let Some(items) = self.items.as_mut() {
            lb_perception::items::look(
                now,
                viewer,
                items,
                entities,
                range,
                vis,
                tracer,
                &mut self.item_cursor,
                self.item_focus,
            );
        }
        if let Some(c) = self.chargers.as_mut() {
            let cursor = &mut self.charger_cursor;
            lb_perception::items::look_chargers(now, viewer, c, chargers, range, vis, tracer, cursor);
        }
    }

    /// Looks at projectiles and mines on the vision tick; call right after [`BotBrain::see`] returned true.
    pub fn see_projectiles(
        &mut self,
        now: SimTime,
        viewer: &Viewer,
        entities: &[ProjectileEntity],
        vis: &dyn VisSets,
        tracer: &mut dyn Tracer,
    ) {
        let (mut seen, mut missing) = (Vec::new(), Vec::new());
        lb_perception::projectiles::look(
            now,
            viewer,
            entities,
            &self.explosives,
            vis,
            tracer,
            &mut seen,
            &mut missing,
        );
        for s in &seen {
            self.explosives.on_sighting(s);
        }
        for p in missing {
            self.explosives.mine_missing(p);
        }
    }

    /// An explosion was seen or heard at `pos`.
    pub fn on_explosion(&mut self, pos: Vec3) {
        self.explosives.on_explosion(pos);
    }

    /// Ages beliefs to `now`; with the map known, notes the places in sight and spreads lost enemies over the
    /// places they may be at.
    pub fn update(&mut self, now: SimTime, params: &BeliefParams, map: Option<&dyn MapView>, eyes: Option<&Eyes>) {
        self.beliefs.update(now, params);
        let Some(map) = map else { return };
        if let Some(e) = eyes {
            self.watch.update(now, e.origin, e.eye, e.view, e.half_fov, map);
            self.patrol.mark(now, e.origin, map);
        }
        let watch = eyes.map(|_| &self.watch);
        self.beliefs
            .spread(now, map, watch, params.track_forget + SPREAD_BEYOND);
        if now >= self.next_expect {
            self.next_expect = now + EXPECT_PERIOD;
            self.expect = self.watch.node.and_then(|at| self.reappear(now, at, map));
            self.approach = self.watch.node.and_then(|at| approach(at, map));
            self.danger = self.watch.node.and_then(|at| {
                let from = map.danger_from(at).filter(|_| map.danger(at) >= DANGER_GLANCE)?;
                map.visible(at, from).then(|| map.node_origin(from) + Vec3::Z * 28.0)
            });
        }
    }

    /// Where the nearest lost enemy would come into view: next to its likeliest places out of sight, the places in
    /// sight that lead to them.
    fn reappear(&self, now: SimTime, at: NodeId, map: &dyn MapView) -> Option<(PlayerKey, Vec3)> {
        let me = map.node_origin(at);
        let t = self
            .beliefs
            .enemies()
            .filter(|t| t.state != lb_knowledge::TrackState::Visible && now.since(t.last_seen) <= EXPECT_FOR)
            .filter(|t| t.spread.is_some())
            .min_by(|a, b| a.pos.distance(me).total_cmp(&b.pos.distance(me)))?;
        let spread = t.spread.as_deref()?;
        let mut score: smallvec::SmallVec<[(NodeId, f32); 16]> = smallvec::SmallVec::new();
        for (n, p) in spread.likely(16) {
            if n == at || map.visible(at, n) {
                continue;
            }
            map.for_each_link(n, &mut |m, _| {
                if m != at && map.visible(at, m) {
                    match score.iter_mut().find(|s| s.0 == m) {
                        Some(s) => s.1 += p,
                        None => score.push((m, p)),
                    }
                }
            });
        }
        let (m, _) = score
            .into_iter()
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))?;
        Some((t.who, map.node_origin(m) + Vec3::Z * 8.0))
    }

    /// Where damage just taken came from, as far as the bot can tell: see [`damage_dealer`].
    pub fn damage_source(&self, d: &DamageStimulus, eye: Vec3) -> Option<Vec3> {
        damage_dealer(&self.beliefs, d, eye).map(|t| t.pos)
    }

    /// Seconds since the bot last saw or heard an enemy (long when it never did).
    pub fn calm_for(&self, now: SimTime) -> f64 {
        self.beliefs
            .enemies()
            .map(|t| now.since(t.last_seen))
            .fold(f64::INFINITY, f64::min)
            .min(now.since(self.mind.last_enemy_seen()))
    }

    /// Where the bot's attention goes now, if anywhere but its path.
    pub fn attention(&mut self, now: SimTime, eye: Vec3) -> Option<Attention> {
        attention::pick(self, now, eye)
    }
}

/// Who dealt damage just taken, as far as the bot can tell: an enemy in sight that fired lately, in the direction the
/// damage compass shows.
pub fn damage_dealer<'a>(beliefs: &'a Beliefs, d: &DamageStimulus, eye: Vec3) -> Option<&'a EnemyTrack> {
    let bearing = d.bearing?;
    beliefs
        .enemies()
        .filter(|t| d.t.since(t.last_seen) <= 0.5)
        .filter(|t| t.traits.fired_at.is_some_and(|f| d.t.since(f) <= 1.5))
        .map(|t| {
            let to = t.pos - eye;
            let off = lb_core::math::angle_diff(lb_core::dmath::atan2(to.y, to.x).to_degrees(), bearing).abs();
            (t, off)
        })
        .filter(|(_, off)| *off <= 45.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(t, _)| t)
}

/// Where an enemy would come into view from `at` when none is known: the busiest place in sight within
/// `APPROACH_BAND`, one next to places out of sight (a doorway, a corner) counting more.
fn approach(at: NodeId, map: &dyn MapView) -> Option<Vec3> {
    let me = map.node_origin(at);
    let mut best: Option<(NodeId, f32)> = None;
    map.for_each_visible(at, &mut |n| {
        let d = map.node_origin(n).distance(me);
        if !(APPROACH_BAND[0]..=APPROACH_BAND[1]).contains(&d) || map.transit(n) {
            return;
        }
        let mut edge = false;
        map.for_each_link(n, &mut |m, _| edge |= m != at && !map.visible(at, m));
        let score = map.flow(n) * if edge { 1.0 } else { 0.3 };
        if score > 0.0 && best.is_none_or(|(b, s)| score > s || (score == s && n < b)) {
            best = Some((n, score));
        }
    });
    best.map(|(n, _)| map.node_origin(n))
}
