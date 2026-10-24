#!/usr/bin/env python3
"""Run a scheduled Lambda handler without AWS: an EventBridge clock that speaks
the Lambda Runtime API.

A Rust handler built on `lambda_runtime` polls
`GET /2018-06-01/runtime/invocation/next` on `AWS_LAMBDA_RUNTIME_API` and
reports `.../{id}/response` or `.../{id}/error`. This server answers that poll
once per interval with an EventBridge "Scheduled Event", like a
`rate(N minutes)` rule. The handler polls again only after it finished, so a
slow run skips ticks instead of piling them up, as Lambda's concurrency limit
of one would.

Environment: FUNCTION_NAME (the rule name), SCHEDULE_SECONDS (default 60),
TIMEOUT_SECONDS (the handler's deadline, default 300), PORT (default 9001).
"""

from __future__ import annotations

import json
import os
import re
import sys
import threading
import time
import uuid
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

API = "/2018-06-01/runtime"
REGION = "us-east-1"
ACCOUNT = "000000000000"
RESULT = re.compile(rf"^{API}/invocation/([0-9a-f-]+)/(response|error)$")


def log(event: str, **fields) -> None:
    print(json.dumps({"event": event, **fields}), flush=True)


def next_tick(now: float, interval: float) -> float:
    """The next multiple of the interval after `now`, like a rate rule."""
    return (now // interval + 1) * interval


def scheduled_event(function: str, at: float) -> dict:
    return {
        "version": "0",
        "id": str(uuid.uuid4()),
        "detail-type": "Scheduled Event",
        "source": "aws.events",
        "account": ACCOUNT,
        "time": datetime.fromtimestamp(at, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "region": REGION,
        "resources": [f"arn:aws:events:{REGION}:{ACCOUNT}:rule/{function}"],
        "detail": {},
    }


class Clock:
    def __init__(self, function: str, interval: float, timeout: float, clock=time.time, sleep=time.sleep):
        self.function = function
        self.interval = interval
        self.timeout = timeout
        self.clock = clock
        self.sleep = sleep
        self.started: dict[str, float] = {}
        self.lock = threading.Lock()

    def wait_for_invocation(self) -> tuple[str, dict, dict]:
        """Block until the next tick; return the request id, headers and event."""
        at = next_tick(self.clock(), self.interval)
        while (remaining := at - self.clock()) > 0:
            self.sleep(remaining)
        request_id = str(uuid.uuid4())
        with self.lock:
            self.started[request_id] = self.clock()
        headers = {
            "Lambda-Runtime-Aws-Request-Id": request_id,
            "Lambda-Runtime-Deadline-Ms": str(int((at + self.timeout) * 1000)),
            "Lambda-Runtime-Invoked-Function-Arn": f"arn:aws:lambda:{REGION}:{ACCOUNT}:function:{self.function}",
        }
        log("invocation.start", function=self.function, request_id=request_id)
        return request_id, headers, scheduled_event(self.function, at)

    def finish(self, request_id: str, outcome: str, body: bytes) -> bool:
        with self.lock:
            started = self.started.pop(request_id, None)
        if started is None:
            return False
        fields = {"function": self.function, "request_id": request_id, "outcome": outcome}
        fields["duration_ms"] = int((self.clock() - started) * 1000)
        if outcome == "error":
            try:
                fields["error_type"] = json.loads(body).get("errorType")
            except ValueError:
                pass
        log("invocation.end", **fields)
        return True


def handler_for(clock: Clock):
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path != f"{API}/invocation/next":
                return self.reply(404)
            request_id, headers, event = clock.wait_for_invocation()
            self.reply(200, json.dumps(event).encode(), headers)

        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
            if self.path == f"{API}/init/error":
                log("init.error", function=clock.function)
                return self.reply(202)
            match = RESULT.match(self.path)
            if not match or not clock.finish(match[1], match[2], body):
                return self.reply(404)
            self.reply(202)

        def reply(self, status: int, body: bytes = b"", headers: dict | None = None):
            self.send_response(status)
            for key, value in (headers or {}).items():
                self.send_header(key, value)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    return Handler


def main() -> None:
    function = os.environ.get("FUNCTION_NAME") or sys.exit("FUNCTION_NAME is required")
    clock = Clock(
        function,
        float(os.environ.get("SCHEDULE_SECONDS", "60")),
        float(os.environ.get("TIMEOUT_SECONDS", "300")),
    )
    server = ThreadingHTTPServer(("0.0.0.0", int(os.environ.get("PORT", "9001"))), handler_for(clock))
    log("clock.ready", function=function, interval_s=clock.interval)
    server.serve_forever()


if __name__ == "__main__":
    main()
