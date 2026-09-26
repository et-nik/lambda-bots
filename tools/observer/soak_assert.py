#!/usr/bin/env python3
"""Headless watchdog over lambdabots telemetry (protocol v2): listens instead of bridge.py and exits non-zero when
an invariant breaks. For soak runs and CI on a stand.

Invariants:
  * telemetry keeps arriving (no gap longer than --max-gap-sec);
  * the core never enters safe mode, bot faults and stale moves stay at zero;
  * core time p99 per frame stays below --max-p99-us;
  * at least --min-bots bots are present once the warm-up is over;
  * no bot stays dead or respawning longer than --max-dead-sec, nor leaving or faulted longer than 10 s.

Usage:
    python3 soak_assert.py --duration 300 --min-bots 8 [--port 27070] [--max-p99-us 2000]
"""

import argparse
import json
import socket
import sys
import time


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=27070)
    ap.add_argument("--duration", type=float, default=300)
    ap.add_argument("--warmup", type=float, default=30, help="seconds before --min-bots is enforced")
    ap.add_argument("--min-bots", type=int, default=1)
    ap.add_argument("--max-p99-us", type=float, default=2000)
    ap.add_argument("--max-dead-sec", type=float, default=10)
    ap.add_argument("--max-gap-sec", type=float, default=5)
    args = ap.parse_args()

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.bind(("127.0.0.1", args.port))
    sock.settimeout(0.5)

    start = last_msg = time.time()
    since = {}          # slot -> (state, wall time it entered the state)
    failures = []
    frames = perfs = 0
    worst_p99 = 0.0
    first_faults = None

    def fail(text):
        if text not in failures:
            failures.append(text)
            print(f"FAIL {time.time() - start:7.1f}s  {text}", flush=True)

    while time.time() - start < args.duration:
        now = time.time()
        if now - last_msg > args.max_gap_sec:
            fail(f"no telemetry for {now - last_msg:.1f} s")
            last_msg = now
        try:
            data, _ = sock.recvfrom(65536)
        except socket.timeout:
            continue
        try:
            msg = json.loads(data)
        except ValueError:
            continue
        if msg.get("v") != 2:
            continue
        last_msg = now
        kind = msg.get("t")
        if kind == "frame":
            frames += 1
            bots = msg.get("bots", [])
            if now - start > args.warmup and len(bots) < args.min_bots:
                fail(f"only {len(bots)} bots (need {args.min_bots})")
            present = set()
            for b in bots:
                slot, state = b.get("slot"), b.get("st")
                present.add(slot)
                prev = since.get(slot)
                if prev is None or prev[0] != state:
                    since[slot] = (state, now)
                    continue
                held = now - prev[1]
                if state in ("dead", "respawning") and held > args.max_dead_sec:
                    fail(f"bot {b.get('n')} {state} for {held:.0f} s")
                if state in ("leaving", "faulted") and held > 10:
                    fail(f"bot {b.get('n')} {state} for {held:.0f} s")
            for slot in list(since):
                if slot not in present:
                    del since[slot]
        elif kind == "perf":
            perfs += 1
            stats = msg.get("stats", {})
            if msg.get("safe_mode"):
                fail(f"safe mode: {msg['safe_mode']}")
            faults = stats.get("bot_faults", 0)
            first_faults = faults if first_faults is None else first_faults
            if faults > first_faults:
                fail(f"bot faults: {faults}")
            if stats.get("stale_moves", 0):
                fail(f"stale moves: {stats['stale_moves']}")
            p99 = msg.get("core", {}).get("p99_us", 0.0)
            worst_p99 = max(worst_p99, p99)
            if p99 > args.max_p99_us:
                fail(f"core p99 {p99:.0f} us > {args.max_p99_us:.0f} us")

    print(f"soak: {time.time() - start:.0f} s, {frames} frames, {perfs} perf reports, worst core p99 {worst_p99:.0f} us, "
          f"{len(failures)} failure(s)")
    if frames == 0:
        print("no telemetry received: is lb_telemetry 1 set and nothing else bound to the port?")
        return 1
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
