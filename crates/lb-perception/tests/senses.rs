//! Perception on small synthetic scenes: walls are boxes, players are boxes, traces are exact segment tests.

use lb_core::Vec3;
use lb_core::rng::Pcg32;
use lb_core::time::SimTime;
use lb_game::sounds::{ATTN_NORM, SoundClass, SoundKind};
use lb_knowledge::*;
use lb_perception::hearing::gain;
use lb_perception::vision::{NEWCOMER_TRACES, PERIOD, TRACE_BUDGET};
use lb_perception::*;
use lb_raw::{ClientState, RawClient};
use lb_worldq::{AllVisible, Trace, TraceQuery, Tracer, contents};

const FL_ONGROUND: u32 = 1 << 9;
const VIEWER: u8 = 1;

const PARAMS: PerceptionParams = PerceptionParams {
    recognition_delay: [0.22, 0.35],
    recognition_floor: 0.16,
    peripheral_gain: 0.50,
    reacquire_delay: 0.10,
    reacquire_grace: 2.0,
    hearing_threshold: 0.04,
    sound_bearing_sigma: 20.0,
};

const BELIEFS: BeliefParams = BeliefParams {
    track_forget: 8.0,
    maxspeed: 300.0,
};

#[derive(Clone, Copy)]
struct Aabb {
    mins: Vec3,
    maxs: Vec3,
}

/// Entry fraction of the segment `a → b` into the box, 0 when it starts inside.
fn enter(a: Vec3, b: Vec3, bx: &Aabb) -> Option<f32> {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for k in 0..3 {
        if d[k].abs() < 1e-6 {
            if a[k] < bx.mins[k] || a[k] > bx.maxs[k] {
                return None;
            }
            continue;
        }
        let (mut lo, mut hi) = ((bx.mins[k] - a[k]) / d[k], (bx.maxs[k] - a[k]) / d[k]);
        if lo > hi {
            std::mem::swap(&mut lo, &mut hi);
        }
        t0 = t0.max(lo);
        t1 = t1.min(hi);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

#[derive(Clone, Default)]
struct Scene {
    walls: Vec<Aabb>,
    clients: Vec<RawClient>,
    traces: u32,
    /// Teams by slot, and the viewer's; none by default.
    teams: Vec<(u8, u8)>,
    viewer_team: u8,
}

impl Tracer for Scene {
    fn trace(&mut self, q: &TraceQuery) -> Trace {
        self.traces += 1;
        let mut best = (1.0f32, None);
        for w in &self.walls {
            if let Some(t) = enter(q.start, q.end, w)
                && t < best.0
            {
                best = (t, None);
            }
        }
        if !q.ignore_monsters {
            for c in &self.clients {
                if Some(u16::from(c.slot)) == q.ignore || c.deadflag != 0 {
                    continue;
                }
                let bx = Aabb {
                    mins: c.origin + c.mins,
                    maxs: c.origin + c.maxs,
                };
                if let Some(t) = enter(q.start, q.end, &bx)
                    && t < best.0
                {
                    best = (t, Some(u32::from(c.slot)));
                }
            }
        }
        let mut tr = Trace::clear(q.start + (q.end - q.start) * best.0);
        tr.fraction = best.0;
        tr.hit = best.1;
        tr
    }

    fn point_contents(&mut self, _p: Vec3) -> i32 {
        contents::EMPTY
    }
}

fn player(slot: u8, origin: Vec3, velocity: Vec3) -> RawClient {
    RawClient {
        slot,
        state: ClientState::Spawned,
        is_fake: true,
        is_ours: false,
        userid: 100 + i32::from(slot),
        origin,
        velocity,
        angles: Vec3::ZERO,
        view_ofs: Vec3::new(0.0, 0.0, 28.0),
        mins: Vec3::new(-16.0, -16.0, -36.0),
        maxs: Vec3::new(16.0, 16.0, 36.0),
        flags: FL_ONGROUND,
        effects: 0,
        movetype: 3,
        solid: 3,
        deadflag: 0,
        waterlevel: 0,
        rendermode: 0,
        renderfx: 0,
        renderamt: 0.0,
        rendercolor: Vec3::ZERO,
        step_left: 0,
        frame: 0.0,
        frags: 0.0,
        sequence: 0,
        gaitsequence: 0,
        weaponmodel: 0,
        model: 0,
        ping_ms: 0,
    }
}

fn wall(x: f32, half_width: f32) -> Aabb {
    Aabb {
        mins: Vec3::new(x, -half_width, -100.0),
        maxs: Vec3::new(x + 16.0, half_width, 200.0),
    }
}

#[derive(Debug, PartialEq)]
struct Tick {
    sightings: Vec<Sighting>,
    cues: Vec<AnonymousCue>,
    recognitions: Vec<Recognition>,
}

struct Run {
    ticks: Vec<Tick>,
    rng: Pcg32,
    vision: Vision,
    beliefs: Beliefs,
}

/// Viewer in slot 1 at the origin looking along `yaw`; scene players move with their velocity every tick.
fn run(mut scene: Scene, yaw: f32, ticks: usize, seed: u64) -> Run {
    scene.clients.insert(0, player(VIEWER, Vec3::ZERO, Vec3::ZERO));
    let viewer = Viewer {
        index: u16::from(VIEWER),
        eye: Vec3::new(0.0, 0.0, 28.0),
        angles: Vec3::new(0.0, yaw, 0.0),
        fov: 0.0,
        aspect: vision::DEFAULT_ASPECT,
        head_in_water: false,
        team: scene.viewer_team,
    };
    let mut r = Run {
        ticks: Vec::new(),
        rng: Pcg32::new(seed, 1),
        vision: Vision::default(),
        beliefs: Beliefs::default(),
    };
    let mut out = VisionOutput::default();
    for i in 0..ticks {
        let now = SimTime(i as f64 * PERIOD);
        let clients = scene.clients.clone();
        let subjects: Vec<Subject<'_>> = clients
            .iter()
            .map(|c| Subject {
                raw: c,
                key: PlayerKey {
                    slot: c.slot,
                    userid: c.userid,
                },
                team: scene
                    .teams
                    .iter()
                    .find(|(slot, _)| *slot == c.slot)
                    .map_or(0, |(_, t)| *t),
                weapon: None,
                shot_at: None,
            })
            .collect();
        out.clear();
        r.vision.tick(
            now,
            &viewer,
            &subjects,
            &r.beliefs,
            &AllVisible,
            &mut scene,
            &PARAMS,
            &mut r.rng,
            &mut out,
        );
        for s in &out.sightings {
            r.beliefs.on_sighting(s);
        }
        for c in &out.cues {
            r.beliefs.on_cue(c);
        }
        r.beliefs.update(now, &BELIEFS);
        r.ticks.push(Tick {
            sightings: out.sightings.clone(),
            cues: out.cues.clone(),
            recognitions: out.recognitions.clone(),
        });
        for c in scene.clients.iter_mut().skip(1) {
            c.origin += c.velocity * PERIOD as f32;
        }
    }
    r
}

fn first_recognition(r: &Run) -> Option<(usize, Recognition)> {
    r.ticks
        .iter()
        .enumerate()
        .find_map(|(i, t)| t.recognitions.first().map(|x| (i, *x)))
}

#[test]
fn frustum_widens_with_the_screen() {
    let f = Frustum::new(Vec3::ZERO, Vec3::ZERO, 0.0, 16.0 / 9.0);
    assert!((f.horizontal_fov() - 106.26).abs() < 0.05, "{}", f.horizontal_fov());
    let square = Frustum::new(Vec3::ZERO, Vec3::ZERO, 0.0, 4.0 / 3.0);
    assert!((square.horizontal_fov() - 90.0).abs() < 0.01);
    assert!(f.contains(Vec3::new(100.0, 120.0, 0.0)) && !square.contains(Vec3::new(100.0, 120.0, 0.0)));
    assert!(!f.contains(Vec3::new(-100.0, 0.0, 0.0)));
    assert!(
        !f.contains(Vec3::new(100.0, 0.0, 90.0)),
        "vertical half-angle is about 37 degrees"
    );
    let zoomed = Frustum::new(Vec3::ZERO, Vec3::ZERO, 40.0, 16.0 / 9.0);
    assert!(zoomed.horizontal_fov() < 55.0);
}

#[test]
fn a_running_enemy_in_plain_sight_is_recognized_after_the_drawn_delay() {
    let scene = Scene {
        clients: vec![player(2, Vec3::new(650.0, -200.0, 0.0), Vec3::new(0.0, 200.0, 0.0))],
        ..Scene::default()
    };
    // 40 ticks: the enemy stays within the view.
    let r = run(scene, 0.0, 40, 7);
    let (tick, rec) = first_recognition(&r).expect("recognized");
    let contact = r.vision.contacts.iter().find(|c| c.who.slot == 2).unwrap();
    assert!((0.22..=0.35).contains(&contact.delay), "{}", contact.delay);
    // Beyond the near range at full rate but for the middle band at worst (0.7), one tick of slack.
    assert!(
        rec.latency >= f64::from(contact.delay) - PERIOD && rec.latency <= f64::from(contact.delay) / 0.7 + PERIOD,
        "latency {} delay {}",
        rec.latency,
        contact.delay
    );
    assert!(r.ticks[tick].sightings[0].first);
    assert!(
        r.ticks[tick + 1..]
            .iter()
            .all(|t| t.sightings.len() == 1 && !t.sightings[0].first),
        "seen every tick afterwards"
    );
    assert!(
        r.ticks[..tick].iter().any(|t| !t.cues.is_empty()),
        "an anonymous cue comes first"
    );
    let track = r.beliefs.track_by_slot(2).unwrap();
    assert_eq!(track.state, TrackState::Visible);
    assert!(track.vel.y > 120.0, "velocity from sightings: {:?}", track.vel);
}

#[test]
fn an_enemy_close_in_front_is_recognized_at_once_standing_or_ducked() {
    for ducked in [false, true] {
        let mut enemy = player(2, Vec3::new(180.0, 10.0, 0.0), Vec3::ZERO);
        if ducked {
            enemy.flags |= 1 << 14;
        }
        let r = run(
            Scene {
                clients: vec![enemy],
                ..Scene::default()
            },
            0.0,
            40,
            3,
        );
        let rec = first_recognition(&r).expect("recognized").1;
        let contact = r.vision.contacts.iter().find(|c| c.who.slot == 2).unwrap();
        let floor = f64::from(PARAMS.recognition_floor);
        assert!(
            rec.latency >= floor && rec.latency <= (f64::from(contact.delay) / 3.5).max(floor) + PERIOD,
            "ducked {ducked}: latency {} delay {}",
            rec.latency,
            contact.delay
        );
    }
}

#[test]
fn a_still_enemy_far_away_takes_longer() {
    let near = run(
        Scene {
            clients: vec![player(2, Vec3::new(250.0, 0.0, 0.0), Vec3::new(0.0, 200.0, 0.0))],
            ..Scene::default()
        },
        0.0,
        200,
        11,
    );
    let far = run(
        Scene {
            clients: vec![player(2, Vec3::new(1800.0, 0.0, 0.0), Vec3::ZERO)],
            ..Scene::default()
        },
        0.0,
        200,
        11,
    );
    let (a, b) = (first_recognition(&near).unwrap().1, first_recognition(&far).unwrap().1);
    assert!(b.latency > a.latency * 2.0, "near {} far {}", a.latency, b.latency);
}

#[test]
fn nothing_behind_the_bot_or_behind_walls_is_seen() {
    let behind = run(
        Scene {
            clients: vec![player(2, Vec3::new(-300.0, 0.0, 0.0), Vec3::ZERO)],
            ..Scene::default()
        },
        0.0,
        100,
        3,
    );
    assert!(behind.ticks.iter().all(|t| t.sightings.is_empty() && t.cues.is_empty()));
    let walled = run(
        Scene {
            walls: vec![wall(200.0, 300.0)],
            clients: vec![player(2, Vec3::new(400.0, 0.0, 0.0), Vec3::ZERO)],
            ..Scene::default()
        },
        0.0,
        100,
        3,
    );
    assert!(walled.ticks.iter().all(|t| t.sightings.is_empty() && t.cues.is_empty()));
    assert!(walled.vision.contacts.is_empty());
}

/// The honesty invariant: a player the bot cannot perceive changes nothing, not even the random stream.
#[test]
fn hidden_players_change_nothing() {
    let visible = player(2, Vec3::new(300.0, -150.0, 0.0), Vec3::new(0.0, 150.0, 0.0));
    let base = Scene {
        walls: vec![wall(600.0, 400.0)],
        clients: vec![visible.clone()],
        ..Scene::default()
    };
    let mut hidden = base.clone();
    // One walks behind the wall, one behind the bot; neither leaves its cover during the run.
    hidden
        .clients
        .push(player(3, Vec3::new(900.0, 0.0, 0.0), Vec3::new(0.0, 30.0, 0.0)));
    hidden
        .clients
        .push(player(4, Vec3::new(-400.0, 50.0, 0.0), Vec3::new(0.0, 100.0, 0.0)));
    let (mut a, mut b) = (run(base, 0.0, 120, 5), run(hidden, 0.0, 120, 5));
    assert_eq!(a.ticks, b.ticks);
    assert_eq!(a.rng.next_u32(), b.rng.next_u32());
    assert!(
        a.ticks.iter().any(|t| !t.sightings.is_empty()),
        "the visible one was seen"
    );
}

#[test]
fn players_block_the_view_of_players_behind_them() {
    let scene = Scene {
        clients: vec![
            player(2, Vec3::new(200.0, 0.0, 0.0), Vec3::ZERO),
            player(3, Vec3::new(600.0, 0.0, 0.0), Vec3::ZERO),
        ],
        ..Scene::default()
    };
    let r = run(scene, 0.0, 200, 9);
    let seen: Vec<u8> = r
        .ticks
        .iter()
        .flat_map(|t| t.sightings.iter().map(|s| s.who.slot))
        .collect();
    assert!(seen.contains(&2));
    let behind = r.ticks.iter().flat_map(|t| &t.sightings).rfind(|s| s.who.slot == 3);
    if let Some(s) = behind {
        assert!(s.visibility < 0.5, "mostly covered: {}", s.visibility);
        assert_eq!(s.parts & parts::CHEST, 0);
    }
}

#[test]
fn a_player_lost_for_a_moment_is_recognized_again_quickly() {
    // Walks behind a pillar and out again.
    let scene = Scene {
        walls: vec![Aabb {
            mins: Vec3::new(300.0, -40.0, -100.0),
            maxs: Vec3::new(340.0, 40.0, 200.0),
        }],
        clients: vec![player(2, Vec3::new(500.0, -300.0, 0.0), Vec3::new(0.0, 200.0, 0.0))],
        ..Scene::default()
    };
    let r = run(scene, 0.0, 80, 21);
    let recs: Vec<(usize, Recognition)> = r
        .ticks
        .iter()
        .enumerate()
        .flat_map(|(i, t)| t.recognitions.iter().map(move |x| (i, *x)))
        .collect();
    assert!(recs.len() >= 2, "{recs:?}");
    let again = recs[1].1;
    assert!(again.reacquired, "{again:?}");
    assert!(again.latency <= 0.35, "{}", again.latency);
}

#[test]
fn the_trace_budget_is_respected() {
    let clients: Vec<RawClient> = (2..8)
        .map(|s| {
            player(
                s,
                Vec3::new(400.0 + 60.0 * f32::from(s), 80.0 * f32::from(s) - 400.0, 0.0),
                Vec3::ZERO,
            )
        })
        .collect();
    let scene = Scene {
        clients,
        ..Scene::default()
    };
    let r = run(scene, 0.0, 40, 1);
    assert!(r.vision.stats.traces <= r.vision.stats.ticks * u64::from(TRACE_BUDGET + NEWCOMER_TRACES));
    assert!(r.vision.stats.skipped > 0);
}

#[test]
fn sounds_fade_with_distance_and_carry_a_bearing_error() {
    assert!((gain(1.0, ATTN_NORM, 500.0) - 0.6).abs() < 1e-6);
    assert_eq!(gain(1.0, ATTN_NORM, 1300.0), 0.0);
    assert_eq!(gain(0.7, 0.0, 5000.0), 0.7, "no attenuation");
    let mut hearing = Hearing::default();
    let mut rng = Pcg32::new(4, 4);
    let listener = Listener {
        slot: 1,
        origin: Vec3::ZERO,
        eye: Vec3::new(0.0, 0.0, 28.0),
        yaw: 0.0,
        speed: 0.0,
    };
    let shot = |x: f32| SoundEvent {
        t: SimTime(1.0),
        source: Some(2),
        origin: Vec3::new(x, x, 0.0),
        class: SoundClass {
            kind: SoundKind::Shot,
            weapon: None,
        },
        volume: 1.0,
        attenuation: ATTN_NORM,
        global: false,
    };
    let mut errors = Vec::new();
    for _ in 0..400 {
        let s = hearing
            .hear(&shot(400.0), &listener, &AllVisible, &PARAMS, &mut rng)
            .unwrap();
        errors.push(lb_core::math::angle_diff(s.bearing, 45.0));
    }
    let front = errors.iter().filter(|e| e.abs() < 90.0).count();
    assert!(front > 340, "mostly heard from the front: {front}");
    let spread = (errors.iter().filter(|e| e.abs() < 90.0).map(|e| e * e).sum::<f32>() / front as f32).sqrt();
    assert!(spread > 15.0 && spread < 40.0, "{spread}");
    assert!(
        hearing
            .hear(&shot(1000.0), &listener, &AllVisible, &PARAMS, &mut rng)
            .is_none(),
        "too far"
    );
    let mut own = shot(0.0);
    own.source = Some(1);
    assert!(hearing.hear(&own, &listener, &AllVisible, &PARAMS, &mut rng).is_none());
    // About 1130 units away: gain 0.09, audible alone but not right after the bot's own shot.
    let faint = shot(800.0);
    assert!(
        hearing
            .hear(&faint, &listener, &AllVisible, &PARAMS, &mut rng)
            .is_none(),
        "own shot masks faint sounds"
    );
    // A grenade bounces with no player behind the sound: heard all the same, the bot's own grenade too.
    let bounce = SoundEvent {
        t: SimTime(5.0),
        source: None,
        origin: Vec3::new(150.0, 0.0, 0.0),
        class: SoundClass {
            kind: SoundKind::Bounce,
            weapon: None,
        },
        volume: 0.25,
        attenuation: ATTN_NORM,
        global: false,
    };
    let heard = hearing.hear(&bounce, &listener, &AllVisible, &PARAMS, &mut rng);
    assert!(heard.is_some_and(|s| s.pos.distance(bounce.origin) < 3.0 * s.sigma().max(1.0)));
}

#[test]
fn steps_follow_the_multiplayer_rule() {
    let mut synth = StepSynth::default();
    let mut out = Vec::new();
    let mut runner = player(2, Vec3::ZERO, Vec3::new(250.0, 0.0, 0.0));
    let mut walker = player(3, Vec3::ZERO, Vec3::new(200.0, 0.0, 0.0));
    let mut climber = player(4, Vec3::ZERO, Vec3::new(0.0, 0.0, 100.0));
    climber.movetype = 5;
    synth.frame(
        &[runner.clone(), walker.clone(), climber.clone()],
        true,
        SimTime(0.0),
        &mut out,
    );
    assert!(out.is_empty(), "the first frame only learns the step state");
    for p in [&mut runner, &mut walker, &mut climber] {
        p.step_left = 1;
    }
    synth.frame(
        &[runner.clone(), walker.clone(), climber.clone()],
        true,
        SimTime(0.3),
        &mut out,
    );
    let sources: Vec<Option<u8>> = out.iter().map(|e| e.source).collect();
    assert_eq!(sources, vec![Some(2), Some(4)], "walking below 220 is silent");
    assert_eq!(out[1].volume, 0.35);
    out.clear();
    runner.step_left = 0;
    synth.frame(&[runner], false, SimTime(0.6), &mut out);
    assert!(out.is_empty(), "mp_footsteps 0");
}

#[test]
fn a_far_player_is_not_starved_by_nearer_hidden_ones() {
    // Four players hide behind a wall close by; one stands in plain sight further away.
    let mut clients: Vec<RawClient> = (0..4)
        .map(|i| {
            player(
                2 + i,
                Vec3::new(300.0 + 20.0 * f32::from(i), 60.0 * f32::from(i) - 90.0, 0.0),
                Vec3::ZERO,
            )
        })
        .collect();
    // Runs straight away from the bot past the edge of the wall, staying in sight.
    let away = Vec3::new(600.0, -467.0, 0.0).normalize() * 200.0;
    clients.push(player(9, Vec3::new(600.0, -467.0, 0.0), away));
    let scene = Scene {
        walls: vec![Aabb {
            mins: Vec3::new(250.0, -150.0, -100.0),
            maxs: Vec3::new(266.0, 150.0, 200.0),
        }],
        clients,
        ..Scene::default()
    };
    let r = run(scene, 0.0, 80, 2);
    let rec = r
        .ticks
        .iter()
        .flat_map(|t| &t.recognitions)
        .find(|x| x.who.slot == 9)
        .copied();
    assert!(rec.is_some(), "never recognized");
    assert!(r.ticks.iter().all(|t| t.sightings.iter().all(|s| s.who.slot == 9)));
}

#[test]
fn an_enemy_stepping_out_close_in_front_is_looked_at_while_others_far_off_are_followed() {
    // Three players crouch far off in plain sight: noticed at once, recognized only slowly.
    let mut clients: Vec<RawClient> = [(2200.0, -400.0), (2250.0, 0.0), (2300.0, 400.0)]
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| {
            let mut c = player(2 + i as u8, Vec3::new(x, y, 0.0), Vec3::ZERO);
            c.flags |= 1 << 14;
            c
        })
        .collect();
    // One walks out from behind a wall 250 units in front after some 0.7 s.
    clients.push(player(9, Vec3::new(250.0, -200.0, 0.0), Vec3::new(0.0, 150.0, 0.0)));
    let scene = Scene {
        walls: vec![Aabb {
            mins: Vec3::new(150.0, -300.0, -100.0),
            maxs: Vec3::new(166.0, -60.0, 200.0),
        }],
        clients,
        ..Scene::default()
    };
    let r = run(scene, 0.0, 40, 3);
    let recognized = |slot: u8| {
        r.ticks
            .iter()
            .position(|t| t.recognitions.iter().any(|x| x.who.slot == slot))
    };
    assert!(
        (2..5).all(|s| recognized(s).is_none_or(|t| t > 20)),
        "the far ones are still being made out"
    );
    let rec = recognized(9).expect("never recognized");
    assert!(rec <= 22, "out about tick 14, recognized at tick {rec}");
}

#[test]
fn something_is_noticed_early_on_the_way_to_recognizing_it() {
    let scene = Scene {
        clients: vec![player(2, Vec3::new(650.0, -200.0, 0.0), Vec3::new(0.0, 200.0, 0.0))],
        ..Scene::default()
    };
    let r = run(scene, 0.0, 40, 7);
    let (recognized, _) = first_recognition(&r).expect("recognized");
    let cue = r.ticks.iter().position(|t| !t.cues.is_empty()).expect("a cue");
    assert!(
        cue <= recognized / 3 + 1,
        "cue at tick {cue}, recognized at {recognized}"
    );
}

#[test]
fn a_player_lost_a_moment_ago_draws_no_glance_when_it_comes_back() {
    let scene = Scene {
        walls: vec![Aabb {
            mins: Vec3::new(300.0, -40.0, -100.0),
            maxs: Vec3::new(340.0, 40.0, 200.0),
        }],
        clients: vec![player(2, Vec3::new(500.0, -300.0, 0.0), Vec3::new(0.0, 200.0, 0.0))],
        ..Scene::default()
    };
    let r = run(scene, 0.0, 80, 21);
    let recognized: Vec<usize> = (0..r.ticks.len())
        .filter(|&i| !r.ticks[i].recognitions.is_empty())
        .collect();
    assert!(recognized.len() >= 2 && r.ticks[recognized[1]].recognitions[0].reacquired);
    assert!(
        r.ticks[recognized[0] + 1..=recognized[1]]
            .iter()
            .all(|t| t.cues.is_empty()),
        "seen again where it was expected"
    );
}

#[test]
fn a_teammate_known_to_be_about_there_draws_no_glance() {
    // Walks behind a wall for some 2.5 s: longer than a quick re-acquisition, not long enough to be forgotten.
    let scene = |team: u8| Scene {
        walls: vec![Aabb {
            mins: Vec3::new(400.0, -100.0, -100.0),
            maxs: Vec3::new(416.0, 100.0, 200.0),
        }],
        clients: vec![player(2, Vec3::new(800.0, -700.0, 0.0), Vec3::new(0.0, 160.0, 0.0))],
        teams: vec![(2, team)],
        viewer_team: 1,
        ..Scene::default()
    };
    // Cues once the player was recognized: it has gone behind the wall and come out again.
    let cues_after_return = |team: u8| {
        let r = run(scene(team), 0.0, 170, 4);
        let (recognized, rec) = first_recognition(&r).expect("recognized before the wall");
        assert!(!rec.reacquired && recognized < 60, "{recognized}");
        let again = r.ticks[recognized + 1..].iter().flat_map(|t| &t.recognitions).next();
        assert!(again.is_some_and(|x| !x.reacquired), "{again:?}");
        r.ticks[recognized + 1..].iter().filter(|t| !t.cues.is_empty()).count()
    };
    assert!(cues_after_return(2) > 0, "an enemy coming out is noticed first");
    assert_eq!(cues_after_return(1), 0, "the teammate is not");
}
