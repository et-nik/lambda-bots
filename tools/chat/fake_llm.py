#!/usr/bin/env python3
"""An OpenAI-compatible chat completions server for stand tests: answers every request with a short line and prints
what it was asked, so the chat can be tried without a model or a key.

Point the stand's config at it:
    chat: { enabled: true, require_humans: false,
            provider: { kind: openai, base_url: "http://127.0.0.1:8099/v1", api_key_env: "" } }

Usage: tools/chat/fake_llm.py [--port 8099] [--delay 0.8] [--line TEXT] [--quiet]
"""

import argparse
import itertools
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LINES = ["привет", "гг", "лол", "ну ты и кемпер", "-", "изи))", "wp", "кто тут бот? я? ну может быть"]


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--port", type=int, default=8099)
    parser.add_argument("--delay", type=float, default=0.8, help="seconds before each answer")
    parser.add_argument("--line", help="always answer this line")
    parser.add_argument("--quiet", action="store_true", help="do not print the prompts")
    args = parser.parse_args()
    lines = itertools.cycle([args.line] if args.line else LINES)

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("content-length", 0)))
            try:
                request = json.loads(body)
            except ValueError:
                self.send_error(400, "not JSON")
                return
            messages = request.get("messages", [])
            system = " ".join(m.get("content", "") for m in messages if m.get("role") == "system")
            notes = "notes" in system or "заметки" in system
            answer = "{}" if notes else next(lines)
            if not args.quiet:
                for m in messages:
                    print(f"--- {m.get('role')}:\n{m.get('content')}", flush=True)
                print(f">>> {answer}\n", flush=True)
            time.sleep(args.delay)
            reply = json.dumps({
                "choices": [{"index": 0, "message": {"role": "assistant", "content": answer}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": len(json.dumps(messages)) // 4, "completion_tokens": len(answer)},
            }).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(reply)))
            self.end_headers()
            self.wfile.write(reply)

        def log_message(self, fmt, *a):
            if not args.quiet:
                sys.stderr.write(fmt % a + "\n")

    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"fake chat model on http://127.0.0.1:{args.port}/v1", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
