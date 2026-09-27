#!/usr/bin/env python3
"""UDP -> WebSocket bridge for the lambdabots observer (telemetry protocol v2, see protocol.md).

    lambdabots --UDP:27070 (v2 JSON)--> bridge.py --WebSocket--> http://localhost:8090/?token=...
               <--UDP:27071 (HMAC-signed `lb` commands)--

What it does:
  * translates v2 `frame` messages into the viewer's player list and keeps the map from `hello`;
  * signs commands typed in the page with the shared secret (`--secret` or LB_TELEMETRY_SECRET) exactly like
    lb-telemetry expects: HMAC-SHA256 over "lbcmd1\n{sid}\n{nonce}\n{ts_ms}\n{args}";
  * serves the page and the WebSocket only with the per-run token printed at start, so other local pages
    cannot drive the server;
  * records every message to recordings/*.ndjson (toggled from the page);
  * converts .bsp to wall JSON on the fly when --maps-dir is given.

Python 3.9+ standard library only. Derived from the yapb-halflife observer bridge (MIT, see NOTICE).

Usage:
    python3 bridge.py --maps-dir "$HOME/path/to/valve/maps"
    LB_TELEMETRY_SECRET=... python3 bridge.py
"""

import argparse
import asyncio
import base64
import datetime
import hashlib
import hmac
import json
import os
import pathlib
import secrets
import socket
import struct
import time
import urllib.parse

BASE_DIR = pathlib.Path(__file__).resolve().parent
RECORDINGS_DIR = BASE_DIR / "recordings"
PROTOCOL = 2

WS_MAGIC = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
CONTENT_TYPES = {
    ".html": "text/html; charset=utf-8",
    ".json": "application/json; charset=utf-8",
    ".ndjson": "text/plain; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".png": "image/png",
}

# Index of each bot state in the page's TASKS list (index.html).
STATES = ["roam", "engage", "hunt", "retreat", "collect",
          "dead", "respawning", "connecting", "spawned", "leaving", "faulted"]
# Weapon ids for the page's WEAPONS table.
WEAPON_IDS = {
    "weapon_crowbar": 1, "weapon_9mmhandgun": 2, "weapon_glock": 2, "weapon_357": 3, "weapon_python": 3,
    "weapon_9mmAR": 4, "weapon_mp5": 4, "weapon_crossbow": 6, "weapon_shotgun": 7, "weapon_rpg": 8,
    "weapon_gauss": 9, "weapon_egon": 10, "weapon_hornetgun": 11, "weapon_handgrenade": 12,
    "weapon_tripmine": 13, "weapon_satchel": 14, "weapon_snark": 15,
}


def sign(secret: bytes, sid: int, nonce: int, ts_ms: int, args: str) -> str:
    text = f"lbcmd1\n{sid}\n{nonce}\n{ts_ms}\n{args}".encode()
    return hmac.new(secret, text, hashlib.sha256).hexdigest()


# ---------------------------------------------------------------------------
# minimal websocket (server side, text frames only)
# ---------------------------------------------------------------------------
class WsConn:
    def __init__(self, reader, writer):
        self.reader = reader
        self.writer = writer
        self.closed = False

    async def send_text(self, text: str):
        if self.closed:
            return
        payload = text.encode("utf-8")
        length = len(payload)
        if length < 126:
            header = struct.pack("!BB", 0x81, length)
        elif length < 65536:
            header = struct.pack("!BBH", 0x81, 126, length)
        else:
            header = struct.pack("!BBQ", 0x81, 127, length)
        try:
            self.writer.write(header + payload)
            await self.writer.drain()
        except (ConnectionError, RuntimeError, OSError):
            self.closed = True

    async def _read_exact(self, n):
        return await self.reader.readexactly(n)

    async def recv(self):
        """Returns a text message, or None when the connection is done."""
        fragments = []
        while True:
            try:
                b1, b2 = await self._read_exact(2)
            except (asyncio.IncompleteReadError, ConnectionError, OSError):
                return None
            fin, opcode = b1 & 0x80, b1 & 0x0F
            masked, length = b2 & 0x80, b2 & 0x7F
            try:
                if length == 126:
                    (length,) = struct.unpack("!H", await self._read_exact(2))
                elif length == 127:
                    (length,) = struct.unpack("!Q", await self._read_exact(8))
                mask = await self._read_exact(4) if masked else b"\x00" * 4
                data = await self._read_exact(length)
            except (asyncio.IncompleteReadError, ConnectionError, OSError):
                return None
            if masked:
                data = bytes(b ^ mask[i % 4] for i, b in enumerate(data))

            if opcode == 0x8:  # close
                await self._send_raw(0x88, b"")
                return None
            if opcode == 0x9:  # ping -> pong
                await self._send_raw(0x8A, data)
                continue
            if opcode == 0xA:  # pong
                continue
            if opcode in (0x1, 0x0):  # text / continuation
                fragments.append(data)
                if fin:
                    try:
                        return b"".join(fragments).decode("utf-8")
                    except UnicodeDecodeError:
                        return None
                continue
            # binary and anything else: skip
            fragments = []

    async def _send_raw(self, first_byte, payload):
        try:
            self.writer.write(struct.pack("!BB", first_byte, len(payload)) + payload)
            await self.writer.drain()
        except (ConnectionError, RuntimeError, OSError):
            self.closed = True

    def close(self):
        self.closed = True
        try:
            self.writer.close()
        except Exception:
            pass


def ws_accept_key(client_key: str) -> str:
    digest = hashlib.sha1((client_key + WS_MAGIC).encode()).digest()
    return base64.b64encode(digest).decode()


# ---------------------------------------------------------------------------
# bridge
# ---------------------------------------------------------------------------
class Bridge:
    def __init__(self, args):
        self.args = args
        self.token = secrets.token_urlsafe(16)
        self.secret = (args.secret or os.environ.get("LB_TELEMETRY_SECRET", "")).encode()
        self.sid = secrets.randbits(63)
        self.nonce = 0
        self.clients = set()
        self.map_name = None
        self.last_frame = None
        self.last_perf = None
        self.pending_events = []
        self.recording = None
        self.recording_path = None
        self.cmd_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.bsp_cache = {}
        if args.record:
            self.start_recording()

    # ---- recording ------------------------------------------------------
    def start_recording(self):
        RECORDINGS_DIR.mkdir(exist_ok=True)
        stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
        self.recording_path = RECORDINGS_DIR / f"session-{stamp}.ndjson"
        self.recording = self.recording_path.open("w", encoding="utf-8")
        print(f"[rec] recording to {self.recording_path}")

    def stop_recording(self):
        if self.recording:
            self.recording.close()
            print(f"[rec] stopped: {self.recording_path}")
        self.recording = None
        self.recording_path = None

    def status_json(self):
        return json.dumps({
            "type": "bridge_status",
            "recording": bool(self.recording),
            "recording_file": self.recording_path.name if self.recording_path else None,
            "signed_commands": bool(self.secret),
        })

    # ---- udp from the game ---------------------------------------------
    def to_viewer_frame(self, msg):
        players = []
        for p in msg.get("players", []):
            players.append({
                "e": p.get("slot"), "n": p.get("n", "?"), "bot": False, "al": bool(p.get("al")),
                "o": p.get("o", [0, 0, 0]), "ya": p.get("ya", 0), "hp": 0, "ap": 0,
            })
        for b in msg.get("bots", []):
            state = b.get("st", "alive")
            # A living bot shows its goal; the others their lifecycle state.
            task = (b.get("goal") or "roam") if state == "alive" else state
            players = [p for p in players if p["e"] != b.get("slot")]
            players.append({
                "e": b.get("slot"), "n": b.get("n", "?"), "bot": True,
                "al": state == "alive", "o": b.get("o", [0, 0, 0]), "ya": b.get("ya", 0),
                "hp": int(b.get("hp", 0)), "ap": int(b.get("ap", 0)),
                "w": WEAPON_IDS.get(b.get("w") or "", 0),
                "task": STATES.index(task) if task in STATES else -1,
                "tstk": [STATES.index(c[0]) for c in b.get("cand", []) if c and c[0] in STATES],
                "cand": b.get("cand", []), "gw": b.get("gw"),
                "en": b.get("tg", -1), "see": bool(b.get("see")), "fire": bool(b.get("fire")),
                "le": -1, "at": b.get("at"), "tr": b.get("tr", []),
                "agr": b.get("agr"), "fear": b.get("fear"), "sty": b.get("sty"), "sk": b.get("sk"),
            })
        events, self.pending_events = self.pending_events, []
        return {"type": "frame", "seq": msg.get("seq", 0), "t": msg.get("ts", 0), "map": self.map_name,
                "players": players, "ev": events}

    def on_udp(self, data):
        try:
            msg = json.loads(data.decode("utf-8", errors="replace"))
        except (ValueError, UnicodeDecodeError):
            return
        if msg.get("v") != PROTOCOL:
            return
        kind = msg.get("t")
        out = None
        if kind == "hello":
            self.map_name = msg.get("map")
            out = {"type": "hello", **msg}
        elif kind == "frame":
            out = self.to_viewer_frame(msg)
            self.last_frame = json.dumps(out)
        elif kind == "event" and msg.get("kind") == "kill":
            self.pending_events.append({"k": "kill", "a": msg.get("killer"), "v": msg.get("victim"),
                                        "t": msg.get("ts", 0)})
        elif kind == "perf":
            self.last_perf = msg
            out = {"type": "perf", **msg}
        elif kind == "cmd_result":
            print(f"[cmd] lb {msg.get('args', '')}:")
            for line in msg.get("out", []):
                print(f"      {line}")
            out = {"type": kind, **msg}
        elif kind in ("event", "log"):
            out = {"type": kind, **msg}

        if self.recording:
            stamp = datetime.datetime.now().timestamp()
            self.recording.write(json.dumps({"rt": round(stamp, 3), "msg": msg}, separators=(",", ":")) + "\n")
        if out is not None:
            self.broadcast(json.dumps(out))

    def broadcast(self, text):
        for ws in list(self.clients):
            if ws.closed:
                self.clients.discard(ws)
                continue
            asyncio.ensure_future(ws.send_text(text))

    # ---- commands to the game ------------------------------------------
    def send_command(self, command: str):
        """Sends `lb <args>` to the command channel; returns an error text or None."""
        parts = command.strip().split(None, 1)
        if not parts or parts[0] != "lb":
            return "only `lb ...` commands can be sent"
        args = parts[1] if len(parts) > 1 else "help"
        self.nonce += 1
        ts_ms = int(time.time() * 1000)
        wire = {"v": PROTOCOL, "cmd": "lb", "args": args, "sid": self.sid, "nonce": self.nonce, "ts_ms": ts_ms,
                "mac": sign(self.secret, self.sid, self.nonce, ts_ms, args) if self.secret else ""}
        try:
            self.cmd_sock.sendto(json.dumps(wire).encode(), (self.args.game_host, self.args.udp_port + 1))
        except OSError as e:
            return f"send failed: {e}"
        return None

    # ---- websocket session ---------------------------------------------
    async def ws_session(self, ws):
        self.clients.add(ws)
        print(f"[ws] client connected ({len(self.clients)} online)")
        if self.last_frame:
            await ws.send_text(self.last_frame)
        await ws.send_text(self.status_json())
        while True:
            raw = await ws.recv()
            if raw is None:
                break
            try:
                msg = json.loads(raw)
            except ValueError:
                continue
            op = msg.get("op")
            if op == "cmd":
                command = str(msg.get("cmd", ""))[:512]
                error = self.send_command(command)
                print(f"[cmd] {command}" + (f" -> {error}" if error else ""))
                if error:
                    await ws.send_text(json.dumps({"type": "cmd_error", "cmd": command, "error": error}))
            elif op == "rec":
                if msg.get("on") and not self.recording:
                    self.start_recording()
                elif not msg.get("on") and self.recording:
                    self.stop_recording()
                self.broadcast(self.status_json())
        self.clients.discard(ws)
        ws.close()
        print(f"[ws] client left ({len(self.clients)} online)")

    # ---- http -----------------------------------------------------------
    async def http_session(self, reader, writer):
        try:
            request_line = await asyncio.wait_for(reader.readline(), 10)
        except (asyncio.TimeoutError, ConnectionError):
            writer.close()
            return
        parts = request_line.decode("latin-1", errors="replace").split()
        if len(parts) < 2:
            writer.close()
            return
        method, target = parts[0], parts[1]
        headers = {}
        while True:
            line = await reader.readline()
            if line in (b"\r\n", b"\n", b""):
                break
            if b":" in line:
                k, v = line.decode("latin-1", errors="replace").split(":", 1)
                headers[k.strip().lower()] = v.strip()

        path, _, query = target.partition("?")
        token_ok = hmac.compare_digest(urllib.parse.parse_qs(query).get("token", [""])[0], self.token)

        if path == "/ws" and "websocket" in headers.get("upgrade", "").lower():
            if not token_ok:
                writer.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                await writer.drain()
                writer.close()
                return
            key = headers.get("sec-websocket-key", "")
            resp = ("HTTP/1.1 101 Switching Protocols\r\n"
                    "Upgrade: websocket\r\nConnection: Upgrade\r\n"
                    f"Sec-WebSocket-Accept: {ws_accept_key(key)}\r\n\r\n")
            writer.write(resp.encode())
            await writer.drain()
            await self.ws_session(WsConn(reader, writer))
            return

        status, body, ctype = 404, b"not found", "text/plain; charset=utf-8"
        try:
            if method != "GET":
                status, body = 405, b"method not allowed"
            elif path == "/" and not token_ok:
                status, body = 403, b"open the URL with ?token=... printed by bridge.py"
            elif path == "/":
                body, status = (BASE_DIR / "index.html").read_bytes(), 200
                ctype = CONTENT_TYPES[".html"]
            elif path == "/api/recordings":
                RECORDINGS_DIR.mkdir(exist_ok=True)
                files = sorted(RECORDINGS_DIR.glob("*.ndjson"), reverse=True)
                body = json.dumps([{"name": f.name, "size": f.stat().st_size} for f in files[:50]]).encode()
                status, ctype = 200, CONTENT_TYPES[".json"]
            elif path.startswith("/recordings/"):
                name = pathlib.Path(path.split("/recordings/", 1)[1]).name
                file = RECORDINGS_DIR / name
                if file.is_file():
                    body, status = file.read_bytes(), 200
                    ctype = CONTENT_TYPES.get(file.suffix, "application/octet-stream")
            elif path.startswith("/maps/"):
                name = pathlib.Path(path.split("/maps/", 1)[1]).stem
                data = await self.map_json(name)
                if data is None:
                    body = b"map not found (or bridge started without --maps-dir)"
                else:
                    body, status = json.dumps(data).encode(), 200
                    ctype = CONTENT_TYPES[".json"]
        except Exception as e:  # noqa: BLE001 - surface any handler problem
            status, body = 500, f"error: {e}".encode()

        reasons = {200: "OK", 403: "Forbidden", 404: "Not Found", 405: "Method Not Allowed",
                   500: "Internal Server Error"}
        head = (f"HTTP/1.1 {status} {reasons.get(status, 'OK')}\r\nContent-Type: {ctype}\r\n"
                f"Content-Length: {len(body)}\r\nCache-Control: no-store\r\n"
                "Connection: close\r\n\r\n")
        try:
            writer.write(head.encode() + body)
            await writer.drain()
        except (ConnectionError, OSError):
            pass
        writer.close()

    async def map_json(self, name):
        if name in self.bsp_cache:
            return self.bsp_cache[name]
        if not self.args.maps_dir:
            return None
        bsp_path = pathlib.Path(self.args.maps_dir).expanduser() / f"{name}.bsp"
        if not bsp_path.exists():
            return None
        import bsp2json
        loop = asyncio.get_running_loop()
        data = await loop.run_in_executor(None, bsp2json.convert, str(bsp_path))
        self.bsp_cache[name] = data
        return data


class UdpProtocol(asyncio.DatagramProtocol):
    def __init__(self, bridge):
        self.bridge = bridge

    def datagram_received(self, data, _addr):
        self.bridge.on_udp(data)


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--udp-port", type=int, default=27070, help="telemetry port (lb telemetry.port)")
    ap.add_argument("--game-host", default="127.0.0.1", help="where the command channel listens")
    ap.add_argument("--http-port", type=int, default=8090)
    ap.add_argument("--maps-dir", default=None, help="game maps dir (valve/maps) for wall rendering")
    ap.add_argument("--secret", default=None, help="command channel secret (default: $LB_TELEMETRY_SECRET)")
    ap.add_argument("--record", action="store_true", help="start recording immediately")
    args = ap.parse_args()

    bridge = Bridge(args)
    loop = asyncio.get_running_loop()
    await loop.create_datagram_endpoint(lambda: UdpProtocol(bridge), local_addr=("127.0.0.1", args.udp_port))
    server = await asyncio.start_server(bridge.http_session, "127.0.0.1", args.http_port)

    RECORDINGS_DIR.mkdir(exist_ok=True)
    print(f"observer:  http://localhost:{args.http_port}/?token={bridge.token}")
    print(f"telemetry: udp://127.0.0.1:{args.udp_port}  commands -> udp://{args.game_host}:{args.udp_port + 1} "
          f"({'signed' if bridge.secret else 'unsigned: the server must allow unauthenticated loopback'})")
    if args.maps_dir:
        print(f"maps:      {args.maps_dir}")
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
