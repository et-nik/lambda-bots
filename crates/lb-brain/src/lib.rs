//! Per-bot brain: schedule, perception to command pipeline, decision trace.

#![forbid(unsafe_code)]

pub mod attention;
pub mod mind;

use lb_core::Vec3;
use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use lb_knowledge::{BeliefParams, Beliefs, DamageStimulus, ItemSpot, Items, PublicEvent};
use lb_motor::{Intents, Motor};
use lb_perception::items::ItemEntity;
use lb_perception::vision::{PERIOD, VisionOutput};
use lb_perception::{Listener, Perception, PerceptionParams, SoundEvent, Subject, Viewer};
use lb_worldq::{Tracer, VisSets};

pub use attention::{Attention, LookReason};
pub use mind::{Body, Character, Mind};

/// Item spots are looked at on every second look.
const ITEM_LOOK_EVERY: u64 = 2;

/// Everything a bot has perceived and believes, and when its senses run next.
#[derive(Clone, Debug)]
pub struct BotBrain {
    pub perception: Perception,
    pub beliefs: Beliefs,
    /// The map's items and what the bot believes about them; `None` until the map's spots are known.
    pub items: Option<Items>,
    pub params: PerceptionParams,
    /// Output of the latest vision tick.
    pub last_vision: VisionOutput,
    pub mind: Mind,
    pub motor: Motor,
    pub intents: Intents,
    /// What vigilance looked at on the last frame.
    pub last_attention: Option<Attention>,
    item_cursor: usize,
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
            params,
            last_vision: VisionOutput::default(),
            mind: Mind::default(),
            motor: Motor::default(),
            intents: Intents::default(),
            last_attention: None,
            item_cursor: 0,
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
    }

    pub fn on_death(&mut self) {
        self.perception.reset();
        self.beliefs.on_own_death();
        self.last_vision.clear();
        self.mind.reset();
    }

    /// The map's item spots became known.
    pub fn set_items(&mut self, spots: &[ItemSpot], now: SimTime) {
        self.items = Some(Items::new(spots, now));
        self.item_cursor = 0;
    }

    pub fn on_public(&mut self, e: &PublicEvent) {
        self.beliefs.on_public(e);
        match e {
            PublicEvent::Death { victim, .. } => self.perception.vision.forget(*victim),
        }
    }

    pub fn on_damage(&mut self, d: &DamageStimulus) {
        self.beliefs.on_damage(d);
    }

    pub fn hear(&mut self, sounds: &[SoundEvent], listener: &Listener, vis: &dyn VisSets, rng: &mut Pcg32) {
        for ev in sounds {
            if let Some(s) = self.perception.hearing.hear(ev, listener, vis, &self.params, rng) {
                self.beliefs.on_sound(&s);
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
        for c in &self.last_vision.cues {
            self.beliefs.on_cue(c);
        }
        self.looks += 1;
        true
    }

    /// Looks at item spots on some of the vision ticks; call right after [`BotBrain::see`] returned true.
    pub fn see_items(
        &mut self,
        now: SimTime,
        viewer: &Viewer,
        entities: &[ItemEntity],
        range: f32,
        vis: &dyn VisSets,
        tracer: &mut dyn Tracer,
    ) {
        if !self.looks.is_multiple_of(ITEM_LOOK_EVERY) {
            return;
        }
        if let Some(items) = self.items.as_mut() {
            lb_perception::items::look(now, viewer, items, entities, range, vis, tracer, &mut self.item_cursor);
        }
    }

    pub fn update(&mut self, now: SimTime, params: &BeliefParams) {
        self.beliefs.update(now, params);
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
