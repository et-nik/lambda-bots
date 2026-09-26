# Observer

A top-down browser view of the bots: positions, view direction, state, HP, weapon, kill feed, session recording
and playback, sending `lb` commands to the server. Ported from yapb-halflife (MIT) to telemetry protocol v2
(`protocol.md`).

Flow: the core sends telemetry over UDP to `127.0.0.1:27070`; `bridge.py` serves it to the page over WebSocket
at `http://localhost:8090`; commands from the page go back to `udp://127.0.0.1:27071`, signed with HMAC.

Only Python 3.9+ with the standard library is required.

## Without a server

```sh
cd tools/observer
python3 fake_server.py --secret test &
LB_TELEMETRY_SECRET=test python3 bridge.py
# open the ?token=... URL printed by bridge.py
```

## With a server

1. Enable telemetry and set the command channel secret (in the server console or in `lambdabots.yaml`):

   ```
   lb_telemetry_secret "long-random-string"
   lb_telemetry 1
   ```
2. Start the bridge with the same secret and the maps directory (for walls on the map view):

   ```sh
   LB_TELEMETRY_SECRET="long-random-string" python3 bridge.py --maps-dir "<path>/valve/maps"
   ```
3. Open the link with the token from the `bridge.py` output. Without the token the page and the WebSocket answer
   403: other local pages cannot send commands to the server.

If the server runs on another machine, do not forward telemetry there: run the bridge next to the server and
reach the page through an SSH tunnel (`ssh -L 8090:127.0.0.1:8090 <server>`).

## Soak check

`soak_assert.py` listens on the same port instead of the bridge and exits with code 1 when invariants are violated
(safe mode, bot faults, stale commands, core p99, stuck dead bots, telemetry dropouts):

```sh
python3 soak_assert.py --duration 300 --min-bots 8 --max-p99-us 2000
```

## Files

| File             | Purpose                                                             |
|------------------|---------------------------------------------------------------------|
| `bridge.py`      | UDP → WebSocket, command signing, page token, recording, BSP → JSON |
| `index.html`     | the whole page, no build step                                       |
| `fake_server.py` | fake telemetry v2 source and command signature check                |
| `bsp2json.py`    | walls from BSP v30 for the map view (`--selftest`)                  |
| `soak_assert.py` | telemetry invariant checks for soak runs                            |
| `protocol.md`    | protocol v2 description                                             |
