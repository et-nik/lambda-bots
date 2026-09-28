//! Tricks: what the bot tells navigation it may do on the way, and the tricks it decides on itself.
//!
//! - **On the way** ([`BotBrain::nav_tricks`], every frame): the long jump links open with the module, when the
//!   server allows long jumps. Long jumps along straight stretches of the way as often as the style likes them (a
//!   roll every 8–12 s). The gauss boost links for skills with tricks and styles that gauss-jump, with a gauss, 40
//!   uranium and 60 health, no enemy seen for two seconds.
//! - **A long jump at an enemy** ([`BotBrain::attack_leap`]): in a fight, with the module, at an enemy in sight
//!   300–900 units away no more than 64 below or 40 above, the will to close in (health × aggression) of 20 at
//!   least, the view on the enemy (within 18° across, no more than 15° up or down) and moving: every half second,
//!   as likely as the style likes, when the flight followed through the server's traces comes down safely and
//!   nearer the enemy, and no snark is about the bot, the enemy or the landing. Then 0.9–1.4 s before the next. The
//!   aim and the shots go on in the air.
//! - **A gauss jump on the way** ([`BotBrain::gauss_leap`]): with the gauss in hand, 30 uranium and 60 health, no
//!   enemy about, on the way somewhere more than 1400 units or 12 nodes off: every 10–18 s, as likely as the style
//!   likes, navigation looks for a boost that lands further along the way (`NavService::gauss_leap`). Not far
//!   enough, it looks again in 4–6 s.
//! - The weapons' part of a boost (charge, turn, jump, let go) is the `GaussBoost` protocol, started when
//!   navigation stops at a boost's takeoff and asks for it.

use lb_core::Vec3;
use lb_core::math::view_angle_vectors;
use lb_core::rng::BotRng;
use lb_core::time::SimTime;
use lb_decision::GoalKind;
use lb_game::entities::ProjectileKind;
use lb_game::weapons::WeaponId;
use lb_nav_api::{NavService, Tricks};

use crate::BotBrain;
use crate::arms::Active;
use crate::mind::{Body, Character};

/// Long jumps along the way are rolled for this often, seconds.
const RUNWAY_ROLL: [f32; 2] = [8.0, 12.0];
/// Paths take gauss boost links with this much uranium (a full charge takes 16, the rest is for the fight after);
/// a boost starts on a full charge's, and goes on as its charge eats it.
const BOOST_URANIUM: i32 = 40;
const BOOST_CHARGE_CELLS: i32 = 16;
const BOOST_HEALTH: f32 = 60.0;
/// No enemy seen this long: calm enough to stop and charge for a boost.
const BOOST_CALM: f64 = 2.0;
/// A long jump at an enemy: how far (horizontally), how far below and above, the least will, how close the view
/// must be to it (cosine across, degrees up or down), how fast the bot must be moving.
const LEAP_BAND: [f32; 2] = [300.0, 900.0];
const LEAP_DZ: [f32; 2] = [-64.0, 40.0];
const LEAP_WILL: f32 = 20.0;
const LEAP_FACING: f32 = 0.95;
const LEAP_PITCH: f32 = 15.0;
const LEAP_SPEED: f32 = 60.0;
/// Looked at this often; after a leap, the next no sooner than this.
const LEAP_CHECK: f64 = 0.5;
const LEAP_REST: [f32; 2] = [0.9, 1.4];
/// The long jump keys are pressed this long: the motor lets go of duck for a command first when it is held.
const LEAP_PRESS: f64 = 0.15;
/// No leap with a snark seen this recently this close to the bot, the enemy or the landing.
const LEAP_SNARKS: f32 = 300.0;
const LEAP_SNARKS_SEEN: f64 = 1.0;
/// A gauss jump on the way: the uranium and the health it needs, how far off the destination must be, and how
/// often it is rolled for (or looked at again when the destination is near).
const GAUSS_URANIUM: i32 = 30;
const GAUSS_FAR: f32 = 1400.0;
const GAUSS_FAR_NODES: usize = 12;
const GAUSS_ROLL: [f32; 2] = [10.0, 18.0];
const GAUSS_NEAR_AGAIN: [f32; 2] = [4.0, 6.0];

/// What the tricks came to, for `lb brain` and the stand statistics.
#[derive(Clone, Debug, Default)]
pub struct TrickStats {
    /// Long jumps taken at an enemy.
    pub leaps: u32,
    /// Gauss jumps on the way navigation found a boost for, and gauss boosts started (links and those).
    pub gauss_jumps: u32,
    pub boosts: u32,
    /// Boosts whose charge went, and boosts given up (with why).
    pub boosts_fired: u32,
    pub boost_failures: Vec<(&'static str, u32)>,
}

impl TrickStats {
    pub(crate) fn boost_failed(&mut self, why: &'static str) {
        match self.boost_failures.iter_mut().find(|(w, _)| *w == why) {
            Some(f) => f.1 += 1,
            None => self.boost_failures.push((why, 1)),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TrickState {
    /// What navigation was told it may do on the last frame, and the uranium then.
    pub told: Tricks,
    pub uranium: i32,
    /// Why the last look for a gauss jump on the way came to nothing.
    pub gauss_why: &'static str,
    /// Long jumps along the way are taken (the style's roll) until then.
    runway: bool,
    runway_until: SimTime,
    next_leap: SimTime,
    /// A long jump at an enemy is being pressed until then.
    leap_until: SimTime,
    next_gauss: SimTime,
    pub stats: TrickStats,
}

impl TrickState {
    /// A new life: timers start over, statistics stay.
    pub fn reset(&mut self) {
        let stats = std::mem::take(&mut self.stats);
        *self = TrickState {
            stats,
            ..TrickState::default()
        };
    }
}

impl BotBrain {
    /// What navigation may do on the way this frame.
    pub(crate) fn nav_tricks(&mut self, body: &Body, ch: &Character, rng: &mut BotRng) -> Tricks {
        let now = body.now;
        let t = &mut self.mind.tricks;
        if now >= t.runway_until {
            t.runway = rng.decision.next_f32() < ch.tricks.longjump;
            t.runway_until = now + f64::from(rng.decision.range_f32(RUNWAY_ROLL[0], RUNWAY_ROLL[1]));
        }
        let longjump = body.has_longjump && body.tricks.longjump;
        let uranium = self.hands(body).reserve(WeaponId::Gauss);
        let calm = self.beliefs.visible_enemies().next().is_none()
            && (self.mind.last_enemy_seen() == SimTime::ZERO || now.since(self.mind.last_enemy_seen()) > BOOST_CALM);
        let boosting = matches!(self.mind.arms.active, Some(Active::GaussBoost(_)));
        let boost_now = ch.skill.tricks
            && body.allows(WeaponId::Gauss)
            && (boosting || uranium >= BOOST_CHARGE_CELLS)
            && body.health >= BOOST_HEALTH
            && body.waterlevel < 2
            && calm;
        let told = Tricks {
            longjump,
            runway: longjump && self.mind.tricks.runway,
            gauss_boost: body.tricks.gauss_boost && ch.tricks.gauss_jump > 0.0 && uranium >= BOOST_URANIUM,
            boost_now,
            gauss_damage: body.damages.gauss_charged,
            selfgauss: body.selfgauss == 1,
        };
        self.mind.tricks.told = told;
        self.mind.tricks.uranium = uranium;
        told
    }

    /// A long jump at the enemy fought, at `enemy` (in sight: `visible`): whether to press it this frame (a leap
    /// decided on is pressed for `LEAP_PRESS`).
    pub(crate) fn attack_leap(
        &mut self,
        body: &Body,
        ch: &Character,
        enemy: Vec3,
        visible: bool,
        nav: &mut dyn NavService,
        rng: &mut BotRng,
    ) -> bool {
        let now = body.now;
        if now < self.mind.tricks.leap_until {
            return body.on_ground;
        }
        let allowed = body.has_longjump && body.tricks.longjump && ch.skill.tricks && ch.tricks.lj_attack > 0.0;
        if !allowed || now < self.mind.tricks.next_leap || !visible {
            return false;
        }
        if !body.on_ground || body.on_ladder || body.waterlevel > 0 || body.velocity.truncate().length() < LEAP_SPEED {
            return false;
        }
        let m = &self.mind;
        if m.arms.busy() || m.reloading(now) || now < m.arms.hold_until {
            return false;
        }
        let to = enemy - body.origin;
        let d = to.truncate().length();
        let will = body.health.clamp(0.0, 100.0) * ch.aggression;
        if !(LEAP_BAND[0]..=LEAP_BAND[1]).contains(&d) || !(LEAP_DZ[0]..=LEAP_DZ[1]).contains(&to.z) || will < LEAP_WILL
        {
            return false;
        }
        let view = self.motor.view;
        let (forward, _, _) = view_angle_vectors(view);
        let facing = forward.truncate().normalize_or_zero().dot(to.truncate() / d.max(1.0));
        if facing < LEAP_FACING || view.x.abs() > LEAP_PITCH {
            return false;
        }
        self.mind.tricks.next_leap = now + LEAP_CHECK;
        if rng.combat.next_f32() >= ch.tricks.lj_attack {
            return false;
        }
        // Not into snarks, the bot's own or anyone's: they bite whoever comes down among them.
        let snarks_by = |p: Vec3| {
            self.explosives.flying.iter().any(|f| {
                f.kind == ProjectileKind::Snark
                    && now.since(f.seen) <= LEAP_SNARKS_SEEN
                    && f.pos.distance(p) < LEAP_SNARKS
            })
        };
        if snarks_by(enemy) || snarks_by(body.origin) {
            return false;
        }
        let Some(landing) = nav.leap_lands(view) else {
            return false;
        };
        if landing.distance(enemy) >= d || snarks_by(landing) {
            return false;
        }
        let tricks = &mut self.mind.tricks;
        tricks.leap_until = now + LEAP_PRESS;
        tricks.next_leap = now + LEAP_PRESS + f64::from(rng.combat.range_f32(LEAP_REST[0], LEAP_REST[1]));
        tricks.stats.leaps += 1;
        tracing::debug!(
            "long jump at an enemy {d:.0} units away, landing {:.0} from it",
            landing.distance(enemy)
        );
        true
    }

    /// A gauss jump on the way somewhere far, when it is time for one: asks navigation for a boost that lands
    /// further along the way.
    pub(crate) fn gauss_leap(&mut self, body: &Body, ch: &Character, nav: &mut dyn NavService, rng: &mut BotRng) {
        let now = body.now;
        let t = &self.mind.tricks;
        if now < t.next_gauss || !body.tricks.gauss_jump || !ch.skill.tricks || ch.tricks.gauss_jump <= 0.0 {
            return;
        }
        let travel = matches!(
            self.mind.goal.map(|g| g.kind),
            Some(
                GoalKind::Roam
                    | GoalKind::Hunt(_)
                    | GoalKind::CollectItem(_)
                    | GoalKind::Investigate(_)
                    | GoalKind::ControlItem(_)
                    | GoalKind::UseCharger(_)
                    | GoalKind::Camp(_)
                    | GoalKind::PlantTrap(_)
            )
        );
        let hands = self.hands(body);
        let uranium = hands.reserve(WeaponId::Gauss);
        let calm = self.beliefs.visible_enemies().next().is_none()
            && (self.mind.last_enemy_seen() == SimTime::ZERO || now.since(self.mind.last_enemy_seen()) > BOOST_CALM);
        let why = if !travel {
            "not on the way anywhere"
        } else if !hands.ready(WeaponId::Gauss) {
            "no gauss ready in hand"
        } else if uranium < GAUSS_URANIUM {
            "too little uranium"
        } else if body.health < BOOST_HEALTH {
            "too little health"
        } else if !calm {
            "an enemy about"
        } else if !body.on_ground || body.waterlevel > 0 || self.mind.arms.busy() {
            "busy"
        } else {
            ""
        };
        if !why.is_empty() {
            self.mind.tricks.gauss_why = why;
            return;
        }
        let far = nav
            .way_left()
            .is_some_and(|(d, nodes)| d > GAUSS_FAR || nodes > GAUSS_FAR_NODES);
        let tricks = &mut self.mind.tricks;
        if !far {
            tricks.gauss_why = "not far";
            tricks.next_gauss = now + f64::from(rng.decision.range_f32(GAUSS_NEAR_AGAIN[0], GAUSS_NEAR_AGAIN[1]));
            return;
        }
        tricks.next_gauss = now + f64::from(rng.decision.range_f32(GAUSS_ROLL[0], GAUSS_ROLL[1]));
        if rng.decision.next_f32() >= ch.tricks.gauss_jump {
            tricks.gauss_why = "not this time";
            return;
        }
        if nav.gauss_leap() {
            tricks.stats.gauss_jumps += 1;
            tricks.gauss_why = "found";
            tracing::debug!("gauss jump on the way");
        } else {
            tricks.gauss_why = "no boost lands further along";
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mind::WeaponLike;
    use lb_combat::Armed;
    use lb_combat::arms::boost::GaussBoost;
    use lb_config::skill::Presets;
    use lb_core::input::{IN_ATTACK2, IN_DUCK, IN_JUMP};
    use lb_core::rng::Pcg32;
    use lb_game::self_state::{PredictedWeapon, Prediction};
    use lb_knowledge::{BeliefParams, PlayerKey, Relation, RenderCue, Sighting, Stance, parts};
    use lb_nav_api::{BoostCall, NavStatus, NavStep};
    use lb_worldq::{Trace, TraceQuery, Tracer, contents};

    /// Open floor at z = -36; tells what the brain asked of it.
    struct Nav {
        tricks: Tricks,
        lands: Option<Vec3>,
        step: NavStep,
        leaps: u32,
    }

    impl Nav {
        fn new() -> Nav {
            Nav {
                tricks: Tricks::default(),
                lands: None,
                step: NavStep::hold(Vec3::new(1000.0, 0.0, 28.0)),
                leaps: 0,
            }
        }
    }

    impl Tracer for Nav {
        fn trace(&mut self, q: &TraceQuery) -> Trace {
            if q.end.z < -36.0 && q.start.z >= -36.0 {
                let f = (q.start.z + 36.0) / (q.start.z - q.end.z);
                let mut t = Trace::clear(q.start + (q.end - q.start) * f);
                t.fraction = f;
                t.normal = Vec3::Z;
                return t;
            }
            Trace::clear(q.end)
        }

        fn point_contents(&mut self, _p: Vec3) -> i32 {
            contents::EMPTY
        }
    }

    impl NavService for Nav {
        fn go_to(&mut self, _dest: Vec3) -> (NavStatus, Option<NavStep>) {
            (NavStatus::Moving, Some(self.step))
        }
        fn roam(&mut self, _rng: &mut Pcg32) -> Option<NavStep> {
            Some(self.step)
        }
        fn away_from(&mut self, _threat: Vec3) -> Option<Vec3> {
            None
        }
        fn available(&self) -> bool {
            true
        }
        fn set_tricks(&mut self, tricks: Tricks) {
            self.tricks = tricks;
        }
        fn leap_lands(&mut self, _view: Vec3) -> Option<Vec3> {
            self.lands
        }
        fn gauss_leap(&mut self) -> bool {
            self.leaps += 1;
            true
        }
        fn way_left(&self) -> Option<(f32, usize)> {
            Some((2400.0, 20))
        }
    }

    fn new_brain() -> BotBrain {
        BotBrain::new(
            1,
            lb_perception::PerceptionParams::from_skill(&Presets::default().at(75)),
        )
    }

    fn character(level: u8, tricks: lb_styles::TrickLikes) -> Character {
        Character {
            skill: Presets::default().at(level),
            level,
            aggression: 0.8,
            fear: 0.2,
            affinity: lb_styles::StyleId::Balanced.goal_affinity(),
            weapons: WeaponLike::default(),
            tricks,
        }
    }

    fn body(now: f64) -> Body {
        Body {
            now: SimTime(now),
            dt: 0.01,
            origin: Vec3::ZERO,
            eye: Vec3::new(0.0, 0.0, 28.0),
            velocity: Vec3::new(200.0, 0.0, 0.0),
            maxspeed: 300.0,
            health: 100.0,
            armor: 0.0,
            has_longjump: true,
            on_ground: true,
            on_ladder: false,
            underwater: false,
            waterlevel: 0,
            fov: 0.0,
            weapon: Some(WeaponId::Mp5),
            arsenal: [Armed::new(WeaponId::Mp5, Some(50), Some(100))].into_iter().collect(),
            prediction: None,
            ammo_need: [0.0; 7],
            opponents: 2,
            damages: lb_game::mechanics::Damages::default(),
            dll: lb_game::dll::DllProfile::default(),
            gravity: 800.0,
            allowed: u32::MAX,
            selfgauss: 0,
            tricks: lb_config::main_config::TricksConfig::default(),
        }
    }

    fn seen(t: f64, pos: Vec3) -> Sighting {
        Sighting {
            who: PlayerKey { slot: 5, userid: 50 },
            relation: Relation::Enemy,
            t: SimTime(t),
            pos,
            sigma: 1.0,
            distance: pos.length(),
            visibility: 1.0,
            parts: parts::CHEST,
            stance: Stance::Standing,
            on_ground: true,
            on_ladder: false,
            in_water: false,
            facing: 180.0,
            weapon: None,
            firing: false,
            render: RenderCue::default(),
            first: true,
            noticed_at: SimTime(t),
        }
    }

    const PARAMS: BeliefParams = BeliefParams {
        track_forget: 12.0,
        maxspeed: 300.0,
    };

    /// Frames of `secs` with an enemy in sight at `enemy` (none: calm), `dress` setting the body up; the buttons of
    /// every command.
    fn frames(
        brain: &mut BotBrain,
        ch: &Character,
        nav: &mut Nav,
        enemy: Option<Vec3>,
        secs: f64,
        dress: &mut dyn FnMut(f64, &mut Body, &mut Nav),
    ) -> Vec<u16> {
        let mut rng = BotRng::new(3, 3);
        let mut out = Vec::new();
        let mut t = 1.0;
        while t < 1.0 + secs {
            if let Some(pos) = enemy {
                brain.beliefs.on_sighting(&seen(t, pos));
            }
            brain.update(SimTime(t), &PARAMS, None, None);
            let mut b = body(t);
            dress(t, &mut b, nav);
            let cmd = brain.act(&b, ch, nav, None, &mut rng);
            brain.motor.sent(cmd.buttons);
            out.push(cmd.buttons);
            t += 0.01;
        }
        out
    }

    fn leapt(buttons: &[u16]) -> bool {
        buttons.iter().any(|b| b & (IN_JUMP | IN_DUCK) == IN_JUMP | IN_DUCK)
    }

    #[test]
    fn a_bot_with_the_module_long_jumps_at_an_enemy_in_its_band() {
        let rusher = lb_styles::StyleId::Rusher.trick_likes();
        let run = |level: u8, enemy: Vec3, module: bool| {
            let mut brain = new_brain();
            let mut nav = Nav::new();
            nav.lands = Some(enemy * 0.6);
            let ch = character(level, rusher);
            let buttons = frames(&mut brain, &ch, &mut nav, Some(enemy), 3.0, &mut |_, b, _| {
                b.has_longjump = module
            });
            (leapt(&buttons), brain.mind.tricks.stats.leaps)
        };
        let (leapt_at, leaps) = run(75, Vec3::new(600.0, 0.0, 0.0), true);
        assert!(leapt_at && leaps >= 1, "{leaps}");
        assert!(!run(75, Vec3::new(600.0, 0.0, 0.0), false).0, "no module");
        assert!(
            !run(10, Vec3::new(600.0, 0.0, 0.0), true).0,
            "a beginner does no tricks"
        );
        assert!(!run(75, Vec3::new(1300.0, 0.0, 0.0), true).0, "too far");
        assert!(!run(75, Vec3::new(600.0, 0.0, 120.0), true).0, "too high above");
    }

    #[test]
    fn navigation_is_told_what_tricks_the_bot_may_do() {
        let balanced = lb_styles::StyleId::Balanced.trick_likes();
        let gauss = |_: f64, b: &mut Body, _: &mut Nav| b.arsenal.push(Armed::new(WeaponId::Gauss, None, Some(60)));
        let mut brain = new_brain();
        let mut nav = Nav::new();
        frames(&mut brain, &character(75, balanced), &mut nav, None, 0.2, &mut {
            gauss
        });
        let t = nav.tricks;
        assert!(
            t.longjump && t.gauss_boost && t.boost_now && t.gauss_damage > 0.0,
            "{t:?}"
        );
        let mut brain = new_brain();
        let mut nav = Nav::new();
        frames(
            &mut brain,
            &character(75, balanced),
            &mut nav,
            Some(Vec3::new(800.0, 0.0, 0.0)),
            0.2,
            &mut { gauss },
        );
        assert!(
            nav.tricks.gauss_boost && !nav.tricks.boost_now,
            "no boost with an enemy about"
        );
        let mut brain = new_brain();
        let mut nav = Nav::new();
        frames(&mut brain, &character(10, balanced), &mut nav, None, 0.2, &mut {
            gauss
        });
        assert!(
            nav.tricks.longjump && !nav.tricks.boost_now,
            "beginners long jump on the way, no more"
        );
    }

    /// The gauss as the game has it: out when asked for, spinning while the secondary attack is held, firing when it
    /// is let go.
    fn gauss_game(spinning: &mut bool, buttons: Option<u16>, b: &mut Body) {
        if let Some(last) = buttons {
            *spinning = last & IN_ATTACK2 != 0;
        }
        b.weapon = Some(WeaponId::Gauss);
        b.arsenal = [Armed::new(WeaponId::Gauss, None, Some(60))].into_iter().collect();
        let mut p = Prediction {
            current: Some(WeaponId::Gauss),
            primary_ammo: 60,
            ..Prediction::default()
        };
        p.weapons[WeaponId::Gauss as usize] = Some(PredictedWeapon {
            in_attack: i32::from(*spinning),
            ..PredictedWeapon::default()
        });
        b.prediction = Some(p);
        b.velocity = Vec3::ZERO;
    }

    #[test]
    fn a_boost_navigation_asks_for_charges_turns_and_jumps() {
        let balanced = lb_styles::StyleId::Balanced.trick_likes();
        let view = Vec3::new(34.0, 180.0, 0.0);
        let mut brain = new_brain();
        let mut nav = Nav::new();
        nav.step.boost = Some(BoostCall { view, charge: 1.6 });
        let mut spinning = false;
        let mut last = None;
        let mut rng = BotRng::new(3, 3);
        let mut buttons = Vec::new();
        let mut jump_view = None;
        let mut t = 1.0;
        while t < 5.0 {
            brain.update(SimTime(t), &PARAMS, None, None);
            let mut b = body(t);
            gauss_game(&mut spinning, last, &mut b);
            let cmd = brain.act(&b, &character(75, balanced), &mut nav, None, &mut rng);
            brain.motor.sent(cmd.buttons);
            last = Some(cmd.buttons);
            buttons.push((t, cmd.buttons));
            // Thrown: navigation flies the bot now and asks no more.
            if cmd.buttons & IN_JUMP != 0 && nav.step.boost.take().is_some() {
                jump_view = Some(brain.motor.view);
            }
            t += 0.01;
        }
        let charged = buttons.iter().filter(|(_, b)| b & IN_ATTACK2 != 0).count();
        let jump = buttons.iter().find(|(_, b)| b & IN_JUMP != 0).map(|(t, _)| *t);
        assert!(charged as f64 * 0.01 >= 1.6, "charged {charged} frames");
        assert!(jump.is_some_and(|j| j >= 2.6), "jumped at {jump:?}");
        assert!(
            jump_view.is_some_and(|v| lb_combat::arms::settled(v, view, 3.0)),
            "{jump_view:?}"
        );
        assert_eq!(brain.mind.tricks.stats.boosts, 1);
    }

    #[test]
    fn a_boost_called_off_while_charging_leaves_the_charge_to_the_gauss() {
        let balanced = lb_styles::StyleId::Balanced.trick_likes();
        let mut brain = new_brain();
        let mut nav = Nav::new();
        nav.step.boost = Some(BoostCall {
            view: Vec3::new(34.0, 180.0, 0.0),
            charge: 1.6,
        });
        let mut spinning = false;
        let mut last = None;
        let mut rng = BotRng::new(3, 3);
        let mut t = 1.0;
        while t < 2.5 {
            if t > 2.0 {
                nav.step.boost = None;
            }
            brain.update(SimTime(t), &PARAMS, None, None);
            let mut b = body(t);
            gauss_game(&mut spinning, last, &mut b);
            let cmd = brain.act(&b, &character(75, balanced), &mut nav, None, &mut rng);
            brain.motor.sent(cmd.buttons);
            last = Some(cmd.buttons);
            t += 0.01;
        }
        assert!(
            brain.mind.arms.active.is_none() && brain.mind.arms.gauss.active(),
            "the gauss holds the charge"
        );
        assert!(spinning, "still charging");
    }

    #[test]
    fn a_boost_under_way_goes_on_as_its_charge_eats_the_uranium() {
        let ch = character(75, lb_styles::StyleId::Balanced.trick_likes());
        let mut brain = new_brain();
        let mut rng = BotRng::new(3, 3);
        let mut b = body(1.0);
        b.arsenal
            .push(Armed::new(WeaponId::Gauss, None, Some(BOOST_CHARGE_CELLS - 4)));
        assert!(
            !brain.nav_tricks(&b, &ch, &mut rng).boost_now,
            "none starts on less than a charge's"
        );
        brain.mind.arms.active = Some(Active::GaussBoost(GaussBoost::new(b.now, Vec3::ZERO, 1.6)));
        assert!(brain.nav_tricks(&b, &ch, &mut rng).boost_now, "one under way goes on");
    }

    #[test]
    fn a_gauss_jump_is_asked_for_on_the_way_somewhere_far() {
        let mut likes = lb_styles::StyleId::Balanced.trick_likes();
        likes.gauss_jump = 1.0;
        let mut brain = new_brain();
        let mut nav = Nav::new();
        let mut spinning = false;
        frames(
            &mut brain,
            &character(75, likes),
            &mut nav,
            None,
            2.0,
            &mut |_, b, _| gauss_game(&mut spinning, None, b),
        );
        assert_eq!(nav.leaps, 1, "once, then again in 10-18 s");
        assert_eq!(brain.mind.tricks.stats.gauss_jumps, 1);
        likes.gauss_jump = 0.0;
        let mut brain = new_brain();
        let mut nav = Nav::new();
        frames(
            &mut brain,
            &character(75, likes),
            &mut nav,
            None,
            2.0,
            &mut |_, b, _| gauss_game(&mut spinning, None, b),
        );
        assert_eq!(nav.leaps, 0, "a style that does not gauss-jump");
    }
}
