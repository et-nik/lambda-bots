# M1 acceptance: vertical slice

State as of 2026-09-27. Test stand: Xash3D FWGS 0.21 (arm64) + Metamod-FWGS + hlsdk-portable, macOS, map crossfire
with its yapb graph. The ReHLDS server has not run M1 yet (package `lambdabots-0.1.0-m1-linux-i386.tar.gz`).

## Results against the plan's criteria

| Criterion                                          | Status | How it was checked                                            |
|----------------------------------------------------|--------|---------------------------------------------------------------|
| Bots walk, notice, attack and lose contact         | yes    | stand soak below; `lb brain`, `lb vision`                     |
| Nothing hidden leaks into behavior                 | yes    | tests: hidden players change no percept, no random draw, no command |
| Reaction chain measured                            | yes    | `lb brain`: first glimpse → first shot, recognition → shot    |
| No conflicting commands                            | yes    | test over 600 frames of a two-enemy fight; arbiter per channel |
| YAML: difficulty, styles, profiles, names          | yes    | `lb-cli config check data/`: 10 files valid                   |
| Decision trace in telemetry and the observer       | yes    | `frame` messages carry goals, candidates, targets; observer panel |

## Stand soak: 8 bots, crossfire, 1000 fps, release build, 5 minutes

| Measure                                      | Value                                         |
|----------------------------------------------|-----------------------------------------------|
| Kills                                        | 85 (17 per minute), every bot 7–17 frags      |
| Faults, dropped events, stale moves          | 0                                             |
| Core time per frame                          | p50 13 µs, p95 69 µs, p99 106 µs, avg 19 µs   |
| First glimpse → first shot, median per bot   | 0.66–1.22 s (skills 43–57)                    |
| Recognition → first shot, mean per bot       | 0.55–0.90 s                                   |
| Contacts answered per bot                    | 83–123                                        |

Perception alone (8 bots, 3 minutes):
- Recognition averages 0.83–1.17 s.
- A look costs 4–6 traces.
- The 12-trace budget runs out on 6–24% of looks; candidates then wait for a later look.

## What M1 contains

- **Navigation:** BSP loading with exact hull traces; yapb graph import with link re-validation; A*; a path
  follower with recovery; `go_to` with a straight final stretch; a fall-back point away from a threat.
- **Perception** (`docs/perception.md`):
  - vision with PVS, frustum, body-point traces and evidence;
  - hearing with PAS, attenuation and localization error; footsteps from the ReHLDS hook or rebuilt from
    `iStepLeft`;
  - the HUD damage compass;
  - item spots checked by sight.
- **Beliefs:** tracks with uncertainty, sound fusion and forgetting; hypotheses; item states with respawn windows.
- **Decisions** (`docs/behavior.md`): engage, hunt, retreat, collect, roam; rank and weight, commitment and
  failure cooldowns; style weights from `config/styles/*.yaml`.
- **Combat:**
  - weapon choice by expected damage per second;
  - target priority;
  - aim with a per-contact head roll, perceptual latency and drifting error;
  - yapb fire cones and click cadence;
  - yapb fight movement with wall and ledge checks.
- **Motor:** priority arbiter over look, movement, stance and weapon. The look controller is yapb's spring or newbie
  model on a fixed step. The weapon controller handles select, confirm, deploy lock, cadence and reload.

## Known limitations (planned later)

- Grenades, satchels, tripmines and snarks are not thrown yet (M4); gauss charge, zoom and M203 are not used.
- No explosion or near-miss hearing, no projectile or mine observation, no dodging (M4).
- Hunting walks to the last known position; beliefs do not spread over the graph yet (M4).
- Only the imported yapb graph is used; maps without one leave bots standing (they still see and shoot).
- Teamplay relations come from the scoreboard, but there is no friendly-fire check yet (M5).
