# lambdabots telemetry, protocol v2

The core sends JSON datagrams over UDP (default `127.0.0.1:27070`) and accepts signed commands on
port +1. Telemetry is enabled with `lb_telemetry 1` or `telemetry.enabled: true` in `config/lambdabots.yaml`.
Packet loss is acceptable; the core never waits on the network.

## Envelope

Each message is one JSON object in one datagram:

```json
{"v": 2, "t": "frame", "sid": 7952386, "seq": 1841, "ep": 3, "ts": 125.25, "...": "body fields"}
```

| Field | Type   | Meaning                                                             |
|-------|--------|---------------------------------------------------------------------|
| `v`   | number | protocol version, always 2                                          |
| `t`   | string | message type                                                        |
| `sid` | number | session (core master seed); changes when the server restarts        |
| `seq` | number | message number within the session, increments by 1; gaps = lost UDP |
| `ep`  | number | map epoch: increments on every map change                           |
| `ts`  | number | map simulation time, seconds                                        |

## Messages

| `t`          | When                             | Body                                              |
|--------------|----------------------------------|---------------------------------------------------|
| `hello`      | map start                        | `core`, `adapter`, `abi`, `map`, `seed`, `engine` |
| `frame`      | `telemetry.hz` times a second    | `bots[]`, `players[]`                             |
| `perf`       | once a second                    | `safe_mode`, `bots`, `stats`, `core`, `drivers[]` |
| `event`      | on event                         | `kind` + event fields                             |
| `cmd_result` | after a command from the channel | `args`, `out[]` (response lines)                  |

**`frame.bots[]`** — our bots: `slot`, `n` (name), `st` (state: `connecting`, `spawned`, `alive`,
`dead`, `respawning`, `leaving`, `faulted`), `o` ([x, y, z]), `ya` (view yaw), `hp`, `ap`, `w` (weapon
classname or `null`).

**`frame.players[]`** — other players from engine snapshots: `slot`, `n`, `o`, `ya`, `al` (alive). Human HP and
weapons are not in the telemetry: the core does not receive them.

**`perf`**:
- `safe_mode` — `null` or the reason the core entered safe mode;
- `stats` — counters since start: `frames`, `commands_sent`, `stale_moves`, `move_calls_failed`,
  `bot_faults`, `dropped_events`, `malformed_records`, `max_frame_ms` (maximum over the last second);
- `core` — core time per frame over the last 4096 frames: `p50_us`, `p95_us`, `p99_us`, `max_us`, `samples`;
- `drivers[]` — per bot: `slot`, `cmds`, `sent_ms`, `debt_dropped_ms`.

**`event.kind`**: `connect`, `putinserver`, `disconnect`, `info`, `connectrejected` (with fields `slot`, `ours`,
`name`) and `kill` (`killer`, `victim` — slots, `weapon`).

## Command channel

A datagram to the telemetry port +1:

```json
{"v": 2, "cmd": "lb", "args": "quota 8", "sid": 12345, "nonce": 17, "ts_ms": 1790000000000, "mac": "9f2c…"}
```

- `mac` — hex HMAC-SHA256 keyed with `telemetry.secret` (or the `lb_telemetry_secret` cvar) over the string
  `"lbcmd1\n{sid}\n{nonce}\n{ts_ms}\n{args}"`.
- `nonce` strictly increases within a `sid` (replay protection).
- `ts_ms` — sender time in Unix milliseconds; allowed skew from the server is ±30 s.
- Only `lb …` commands are accepted; the response arrives as a `cmd_result` message.
- Without a secret the channel is closed. The exception is `telemetry.allow_unauthenticated_loopback: true`: then
  unsigned commands are accepted from 127.0.0.1 only.
- Rejected commands are counted in `lb status` ("command channel rejected N") and logged with the reason.

`bridge.py` signs commands itself (`--secret` or `LB_TELEMETRY_SECRET`).

## Changes from yapb-halflife

The old protocol (`{"type": "frame", "players": [...]}` without a version, plain-text `cmd <console command>`
commands) is not supported. `bridge.py` converts v2 frames into the `index.html` page format, so the page itself
barely changed.
