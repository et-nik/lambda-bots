#!/usr/bin/env python3
"""An OpenAI-compatible chat completions server for stand tests: answers every request with a short line and prints
what it was asked, so the chat can be tried without a model or a key.

With --fail STATUS it refuses every request instead, notes included, answering {"error": {"message", "type"}} with
that status; the message and type default to Moonshot's for an account out of money, which it sends with a 429.
--fail-for limits the refusals to the first seconds after the start, to see the chat come back: after a refusal for
lack of money (402, or 400, 403 or 429 with the default message or type) the worker tries again 10 minutes after the
last one, or at once on `lb chat reload`; after 401, 404 or another 403 only on `lb chat reload` or
`lb config reload`; after 408, 409, another 429 or 5xx within a minute; after another 4xx at once.

Point the stand's config at it:
    chat: { enabled: true, require_humans: false,
            provider: { kind: openai, base_url: "http://127.0.0.1:8099/v1", api_key_env: "" } }

Usage: tools/chat/fake_llm.py [--port 8099] [--delay 0.8] [--line TEXT] [--quiet]
                              [--fail STATUS [--error TEXT] [--error-type TYPE] [--fail-for SECONDS]]
"""

import argparse
import itertools
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LINES = ["привет", "гг", "лол", "ну ты и кемпер", "-", "изи))", "wp", "кто тут бот? я? ну может быть"]
# Moonshot's refusal once the money ran out, the account and key ids replaced.
UNPAID = ("Your account org-test <ak-test> is suspended due to insufficient balance, please recharge your account or "
          "check your plan and billing details")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--port", type=int, default=8099)
    parser.add_argument("--delay", type=float, default=0.8, help="seconds before each answer")
    parser.add_argument("--line", help="always answer this line")
    parser.add_argument("--quiet", action="store_true", help="do not print the prompts")
    parser.add_argument("--fail", type=int, metavar="STATUS", help="refuse every request with this HTTP status")
    parser.add_argument("--error", default=UNPAID, metavar="TEXT", help="the refusal's message (default: Moonshot's)")
    parser.add_argument("--error-type", default="exceeded_current_quota_error", metavar="TYPE",
                        help="the refusal's type (default: %(default)s)")
    parser.add_argument("--fail-for", type=float, default=0, metavar="SECONDS",
                        help="refuse only this long after the start (default: 0, always); the worker still waits "
                             "after the last refusal: 10 min for lack of money; after 401, 404 or another 403, until "
                             "`lb chat reload` or `lb config reload`; after 408, 409, another 429 or 5xx, up to a "
                             "minute")
    args = parser.parse_args()
    if args.fail is not None and not 400 <= args.fail <= 599:
        parser.error("--fail takes an HTTP error status, 400-599")
    lines = itertools.cycle([args.line] if args.line else LINES)
    started = time.monotonic()

    def failing():
        return args.fail is not None and (args.fail_for <= 0 or time.monotonic() - started < args.fail_for)

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("content-length", 0)))
            try:
                request = json.loads(body)
            except ValueError:
                self.send_error(400, "not JSON")
                return
            messages = request.get("messages", [])
            if failing():
                status = args.fail
                answer = f"HTTP {status}: {args.error}"
                reply = {"error": {"message": args.error, "type": args.error_type}}
            else:
                user = " ".join(m.get("content", "") for m in messages if m.get("role") == "user")
                status = 200
                answer = "{}" if "JSON" in user else next(lines)
                choice = {"index": 0, "message": {"role": "assistant", "content": answer}, "finish_reason": "stop"}
                usage = {"prompt_tokens": len(json.dumps(messages)) // 4, "completion_tokens": len(answer)}
                reply = {"choices": [choice], "usage": usage}
            if not args.quiet:
                for m in messages:
                    print(f"--- {m.get('role')}:\n{m.get('content')}", flush=True)
                print(f">>> {answer}\n", flush=True)
            time.sleep(args.delay)
            data = json.dumps(reply).encode()
            self.send_response(status)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, fmt, *a):
            if not args.quiet:
                sys.stderr.write(fmt % a + "\n")

    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    refusing = ""
    if args.fail is not None:
        refusing = f", refusing with HTTP {args.fail}" + (f" for {args.fail_for:g} s" if args.fail_for > 0 else "")
    print(f"fake chat model on http://127.0.0.1:{args.port}/v1{refusing}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
