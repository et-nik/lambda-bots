#!/usr/bin/env python3
"""Fake lambdabots telemetry source (protocol v2) for working on the observer without a game server.

Sends `hello`, `frame` (bots running circles on a fake map, one human) at --hz, `perf` once a second and a kill
event now and then; listens for commands on port+1 and checks their signature the same way lb-telemetry does.

Usage:
    python3 fake_server.py [--port 27070] [--hz 10] [--bots 8] [--secret S]
"""

import argparse
import hashlib
import hmac
import json
import math
import random
import socket
import time

PROTOCOL = 2
STATES = ["alive"] * 6 + ["dead", "respawning"]
WEAPONS = ["weapon_9mmhandgun", "weapon_shotgun", "weapon_9mmAR", "weapon_crossbow", "weapon_rpg"]


def envelope(kind, seq, sid, ts, body):
    return json.dumps({"v": PROTOCOL, "t": kind, "sid": sid, "seq": seq, "ep": 1, "ts": round(ts, 3), **body})


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=27070)
    ap.add_argument("--hz", type=float, default=10)
    ap.add_argument("--bots", type=int, default=8)
    ap.add_argument("--secret", default="")
    args = ap.parse_args()

    out = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    cmd = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    cmd.bind(("127.0.0.1", args.port + 1))
    cmd.setblocking(False)
    dest = ("127.0.0.1", args.port)
    sid = random.getrandbits(63)
    seq = 0
    start = time.time()
    last_hello = last_perf = -10.0
    last_nonce = {}
    names = [f"bot{i + 1}" for i in range(args.bots)]
    print(f"fake server: telemetry -> udp://127.0.0.1:{args.port}, commands <- udp://127.0.0.1:{args.port + 1}")

    while True:
        t = time.time() - start
        if t - last_hello >= 5:
            last_hello = t
            seq += 1
            out.sendto(envelope("hello", seq, sid, t, {"core": "0.1.0-fake", "adapter": "fake", "abi": 1,
                                                     "map": "crossfire", "seed": sid, "engine": "fake"}).encode(), dest)
        bots = []
        for i, name in enumerate(names):
            a = t * 0.4 + i * 2 * math.pi / len(names)
            r = 300 + 80 * math.sin(t * 0.3 + i)
            state = STATES[(int(t / 7) + i) % len(STATES)]
            bots.append({"slot": i + 1, "n": name, "st": state, "o": [r * math.cos(a), r * math.sin(a), -100],
                         "ya": math.degrees(a) + 90, "hp": 100 if state == "alive" else 0, "ap": 25,
                         "w": WEAPONS[i % len(WEAPONS)]})
        human = {"slot": len(names) + 1, "n": "player", "o": [0, 0, -100], "ya": (t * 30) % 360, "al": True}
        seq += 1
        out.sendto(envelope("frame", seq, sid, t, {"bots": bots, "players": [human]}).encode(), dest)
        if t - last_perf >= 1:
            last_perf = t
            seq += 1
            perf = {"safe_mode": None, "bots": len(names),
                    "stats": {"frames": int(t * 1000), "bot_faults": 0, "stale_moves": 0, "commands_sent": int(t * 800)},
                    "core": {"p50_us": 8.0, "p95_us": 30.0, "p99_us": 55.0, "max_us": 400.0, "samples": 4096}}
            out.sendto(envelope("perf", seq, sid, t, perf).encode(), dest)
            if random.random() < 0.3:
                seq += 1
                killer, victim = random.sample(range(1, len(names) + 1), 2)
                out.sendto(envelope("event", seq, sid, t, {"kind": "kill", "killer": killer, "victim": victim,
                                                           "weapon": "9mmhandgun"}).encode(), dest)
        try:
            while True:
                data, addr = cmd.recvfrom(4096)
                handle_command(data, addr, args.secret.encode(), last_nonce, out, dest, sid, t)
        except BlockingIOError:
            pass
        time.sleep(1 / args.hz)


def handle_command(data, addr, secret, last_nonce, out, dest, sid, t):
    try:
        w = json.loads(data)
    except ValueError:
        print(f"[cmd] {addr}: malformed")
        return
    verdict = "accepted"
    if w.get("v") != PROTOCOL or w.get("cmd") != "lb":
        verdict = "rejected (not lb v2)"
    elif secret:
        text = f"lbcmd1\n{w.get('sid')}\n{w.get('nonce')}\n{w.get('ts_ms')}\n{w.get('args')}".encode()
        good = hmac.compare_digest(hmac.new(secret, text, hashlib.sha256).hexdigest(), str(w.get("mac", "")))
        if not good:
            verdict = "rejected (signature)"
        elif w.get("nonce", 0) <= last_nonce.get(w.get("sid"), -1):
            verdict = "rejected (replay)"
        else:
            last_nonce[w.get("sid")] = w.get("nonce", 0)
    print(f"[cmd] lb {w.get('args')} from {addr[0]}: {verdict}")
    if verdict == "accepted":
        out.sendto(envelope("cmd_result", 0, sid, t, {"args": w.get("args"), "out": [f"(fake) lb {w.get('args')}"]})
                   .encode(), dest)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
