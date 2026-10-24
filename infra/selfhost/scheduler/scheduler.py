#!/usr/bin/env python3
"""EventBridge rules that put a fixed message on an SQS queue, without AWS.

Some schedules in production have a queue as their target, not a Lambda: every
interval EventBridge sends the rule's input to the queue, and a service's
worker picks it up (reminder sweeps, the nightly CRM cleanup). This sends the
same messages on the same schedule.

Environment: SCHEDULES, a JSON list of {"name", "every" (seconds), "offset"
(seconds past the interval boundary, for a daily cron), "queue" (URL),
"body" (the message, JSON)}; AWS_URL (the SQS endpoint, default
http://localstack:4566).
"""

from __future__ import annotations

import json
import os
import sys
import threading
import time
import urllib.request


def log(event: str, **fields) -> None:
    print(json.dumps({"event": event, **fields}), flush=True)


def next_tick(now: float, every: float, offset: float = 0) -> float:
    """The next `offset + k * every` after `now`, like a rate or cron rule."""
    return ((now - offset) // every + 1) * every + offset


def send_message(endpoint: str, queue_url: str, body: dict) -> None:
    # SQS's JSON protocol, like the AWS SDKs: the gateway routes on X-Amz-Target.
    request = urllib.request.Request(
        endpoint,
        data=json.dumps({"QueueUrl": queue_url, "MessageBody": json.dumps(body)}).encode(),
        headers={"Content-Type": "application/x-amz-json-1.0", "X-Amz-Target": "AmazonSQS.SendMessage"},
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        response.read()


def run(schedule: dict, send, stop: threading.Event, clock=time.time) -> None:
    every, offset = float(schedule["every"]), float(schedule.get("offset", 0))
    while not stop.is_set():
        if stop.wait(max(0.0, next_tick(clock(), every, offset) - clock())):
            return
        try:
            send(schedule["queue"], schedule["body"])
            log("schedule.sent", name=schedule["name"])
        except OSError as error:
            # A missed tick is caught up by the next one, as with EventBridge retries.
            log("schedule.failed", name=schedule["name"], error=type(error).__name__)


def main() -> None:
    schedules = json.loads(os.environ.get("SCHEDULES") or sys.exit("SCHEDULES is required"))
    endpoint = os.environ.get("AWS_URL", "http://localstack:4566")
    stop = threading.Event()
    for schedule in schedules:
        threading.Thread(
            target=run, args=(schedule, lambda q, b: send_message(endpoint, q, b), stop), daemon=True
        ).start()
    log("scheduler.ready", schedules=[s["name"] for s in schedules])
    stop.wait()


if __name__ == "__main__":
    main()
