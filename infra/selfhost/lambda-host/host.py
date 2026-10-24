#!/usr/bin/env python3
"""Run a Lambda handler without AWS: its binary, unchanged, plus its triggers.

A Rust handler built on `lambda_runtime` polls the Lambda Runtime API on
`AWS_LAMBDA_RUNTIME_API` for its next event and reports the result. This host
serves that API to the handler, which it starts as a child process (and starts
again if it exits), and turns the handler's AWS triggers into invocations:

- `schedule`: an EventBridge "Scheduled Event" (or a fixed `input`, like an
  EventBridge target input) every `every` seconds, `offset` seconds past the
  interval boundary, like `rate(...)` and `cron(...)` rules. A run that is still
  going skips the tick, as Lambda with a concurrency of one would.
- `sqs`: messages of `queue` as an SQS event of up to `batch` records. Records
  are deleted when the handler succeeds (minus its `batchItemFailures`); failed
  ones come back after the visibility timeout, and after `max_receives` they are
  dropped with an error, where AWS would move them to a dead-letter queue.
- `s3`: S3 event notifications from `queue` (the upload relay sends them), as
  the S3 event itself; `s3-eventbridge` turns each record into the EventBridge
  "Object Created" event that an S3 bucket with EventBridge enabled sends.
- `http`: HTTP requests on `port` as Lambda Function URL events (payload 2.0).

It also serves the Lambda Invoke API (`POST /2015-03-31/functions/<name>/
invocations`) on the runtime port, which the AWS gateway routes to, so a
handler can invoke itself or another handler asynchronously or synchronously.

One invocation runs at a time, in arrival order.

Environment: FUNCTION_NAME, SOURCES (a JSON list of the sources above),
TIMEOUT_SECONDS (the handler's deadline, default 300), PORT (default 9001),
AWS_URL (the SQS endpoint, default http://localstack:4566).
Usage: host.py -- <handler binary> [args...]
"""

from __future__ import annotations

import base64
import json
import os
import queue
import re
import signal
import subprocess
import sys
import threading
import time
import urllib.parse
import urllib.request
import uuid
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

API = "/2018-06-01/runtime"
REGION = "us-east-1"
ACCOUNT = "000000000000"
RESULT = re.compile(rf"^{API}/invocation/([0-9a-f-]+)/(response|error)$")
INVOKE = re.compile(r"^/2015-03-31/functions/([^/]+)/invocations$")


def log(event: str, **fields) -> None:
    # Never payloads: they can hold content and personal data.
    print(json.dumps({"event": event, **fields}), flush=True)


def iso(at: float, millis: bool = False) -> str:
    stamp = datetime.fromtimestamp(at, timezone.utc)
    return stamp.strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z" if millis else stamp.strftime("%Y-%m-%dT%H:%M:%SZ")


class Invocation:
    """One event for the handler, and a way to wait for its outcome."""

    def __init__(self, event, source: str):
        self.id = str(uuid.uuid4())
        self.payload = event if isinstance(event, bytes) else json.dumps(event).encode()
        self.source = source
        self.outcome: str | None = None
        self.body = b""
        self.finished = threading.Event()

    def finish(self, outcome: str, body: bytes) -> None:
        self.outcome, self.body = outcome, body
        self.finished.set()

    def wait(self, timeout: float) -> bool:
        """True when the handler answered in time."""
        return self.finished.wait(timeout) and self.outcome == "response"


class Runtime:
    """The Lambda Runtime API side: hands invocations to the handler, one at a time."""

    def __init__(self, function: str, timeout: float, clock=time.time):
        self.function = function
        self.timeout = timeout
        self.clock = clock
        self.pending: queue.Queue[Invocation] = queue.Queue()
        self.running: dict[str, tuple[Invocation, float]] = {}
        self.lock = threading.Lock()

    def submit(self, invocation: Invocation) -> Invocation:
        self.pending.put(invocation)
        return invocation

    def next(self) -> tuple[Invocation, dict]:
        invocation = self.pending.get()
        started = self.clock()
        with self.lock:
            self.running[invocation.id] = (invocation, started)
        log("invocation.start", function=self.function, source=invocation.source, request_id=invocation.id)
        headers = {
            "Lambda-Runtime-Aws-Request-Id": invocation.id,
            "Lambda-Runtime-Deadline-Ms": str(int((started + self.timeout) * 1000)),
            "Lambda-Runtime-Invoked-Function-Arn": f"arn:aws:lambda:{REGION}:{ACCOUNT}:function:{self.function}",
        }
        return invocation, headers

    def finish(self, request_id: str, outcome: str, body: bytes) -> bool:
        with self.lock:
            entry = self.running.pop(request_id, None)
        if entry is None:
            return False
        invocation, started = entry
        fields = {"function": self.function, "source": invocation.source, "request_id": request_id}
        fields.update(outcome=outcome, duration_ms=int((self.clock() - started) * 1000))
        if outcome == "error":
            try:
                fields["error_type"] = json.loads(body).get("errorType")
            except (ValueError, AttributeError):
                pass
        log("invocation.end", **fields)
        invocation.finish(outcome, body)
        return True

    def abandon(self) -> None:
        """The handler died: what it was running fails, so its source can retry."""
        with self.lock:
            running, self.running = list(self.running.values()), {}
        for invocation, _ in running:
            log("invocation.end", function=self.function, source=invocation.source, request_id=invocation.id, outcome="crash")
            invocation.finish("error", b'{"errorType":"HandlerCrashed"}')


# --- Events --------------------------------------------------------------------


def scheduled_event(function: str, at: float) -> dict:
    return {
        "version": "0",
        "id": str(uuid.uuid4()),
        "detail-type": "Scheduled Event",
        "source": "aws.events",
        "account": ACCOUNT,
        "time": iso(at),
        "region": REGION,
        "resources": [f"arn:aws:events:{REGION}:{ACCOUNT}:rule/{function}"],
        "detail": {},
    }


def next_tick(now: float, every: float, offset: float = 0) -> float:
    """The next `offset + k * every` after `now`, like a rate or cron rule."""
    return ((now - offset) // every + 1) * every + offset


def lambda_message_attributes(attributes: dict) -> dict:
    """SQS API attributes (`StringValue`, `DataType`) in the Lambda event's camelCase."""
    return {
        name: {key[0].lower() + key[1:]: value for key, value in attribute.items()}
        for name, attribute in attributes.items()
    }


def sqs_record(message: dict, queue_url: str) -> dict:
    name = queue_url.rstrip("/").rsplit("/", 1)[-1]
    return {
        "messageId": message["MessageId"],
        "receiptHandle": message["ReceiptHandle"],
        "body": message.get("Body", ""),
        "attributes": message.get("Attributes", {}),
        "messageAttributes": lambda_message_attributes(message.get("MessageAttributes", {})),
        "md5OfBody": message.get("MD5OfBody", ""),
        "eventSource": "aws:sqs",
        "eventSourceARN": f"arn:aws:sqs:{REGION}:{ACCOUNT}:{name}",
        "awsRegion": REGION,
    }


def eventbridge_object_created(record: dict) -> dict:
    """What S3 sends to EventBridge for one ObjectCreated record."""
    s3 = record["s3"]
    obj = {"key": urllib.parse.unquote_plus(s3["object"]["key"])}
    for source, target in (("size", "size"), ("eTag", "etag"), ("sequencer", "sequencer")):
        if source in s3["object"]:
            obj[target] = s3["object"][source]
    return {
        "version": "0",
        "id": str(uuid.uuid4()),
        "detail-type": "Object Created",
        "source": "aws.s3",
        "account": ACCOUNT,
        "time": record.get("eventTime", iso(time.time()))[:19] + "Z",
        "region": REGION,
        "resources": [f"arn:aws:s3:::{s3['bucket']['name']}"],
        "detail": {
            "version": "0",
            "bucket": {"name": s3["bucket"]["name"]},
            "object": obj,
            "reason": record.get("eventName", "ObjectCreated:Put").split(":", 1)[-1],
        },
    }


def function_url_event(method: str, target: str, headers, body: bytes, source_ip: str, at: float) -> dict:
    path, _, raw_query = target.partition("?")
    joined: dict[str, str] = {}
    cookies = []
    for key, value in headers.items():
        key = key.lower()
        if key == "cookie":
            cookies.extend(part.strip() for part in value.split(";") if part.strip())
            continue
        joined[key] = f"{joined[key]},{value}" if key in joined else value
    query = {k: ",".join(v) for k, v in urllib.parse.parse_qs(raw_query, keep_blank_values=True).items()}
    try:
        text, encoded = body.decode("utf-8"), False
    except UnicodeDecodeError:
        text, encoded = base64.b64encode(body).decode(), True
    event = {
        "version": "2.0",
        "routeKey": "$default",
        "rawPath": path,
        "rawQueryString": raw_query,
        "headers": joined,
        "requestContext": {
            "accountId": ACCOUNT,
            "apiId": "selfhost",
            "domainName": joined.get("host", "localhost"),
            "domainPrefix": joined.get("host", "localhost").split(".")[0],
            "http": {
                "method": method,
                "path": path,
                "protocol": "HTTP/1.1",
                "sourceIp": source_ip,
                "userAgent": joined.get("user-agent", ""),
            },
            "requestId": str(uuid.uuid4()),
            "routeKey": "$default",
            "stage": "$default",
            "time": datetime.fromtimestamp(at, timezone.utc).strftime("%d/%b/%Y:%H:%M:%S +0000"),
            "timeEpoch": int(at * 1000),
        },
        "isBase64Encoded": encoded,
    }
    if query:
        event["queryStringParameters"] = query
    if cookies:
        event["cookies"] = cookies
    if body:
        event["body"] = text
    return event


def function_url_response(body: bytes) -> tuple[int, dict, bytes]:
    """A Function URL result: a response object, or any JSON value as a 200 body."""
    try:
        result = json.loads(body) if body else None
    except ValueError:
        return 200, {"content-type": "application/json"}, body
    if not isinstance(result, dict) or "statusCode" not in result:
        return 200, {"content-type": "application/json"}, body
    payload = result.get("body") or ""
    raw = base64.b64decode(payload) if result.get("isBase64Encoded") else payload.encode()
    headers = {k.lower(): str(v) for k, v in (result.get("headers") or {}).items()}
    for cookie in result.get("cookies") or []:
        headers.setdefault("set-cookie", cookie)
    return int(result["statusCode"]), headers, raw


# --- Sources -------------------------------------------------------------------


class Sqs:
    """The few SQS calls the sources need, in SQS's JSON protocol, unsigned."""

    def __init__(self, endpoint: str):
        self.endpoint = endpoint

    def call(self, action: str, **params) -> dict:
        request = urllib.request.Request(
            self.endpoint,
            data=json.dumps(params).encode(),
            headers={"Content-Type": "application/x-amz-json-1.0", "X-Amz-Target": f"AmazonSQS.{action}"},
        )
        with urllib.request.urlopen(request, timeout=60) as response:
            return json.loads(response.read() or b"{}")

    def receive(self, queue_url: str, batch: int, visibility: int) -> list[dict]:
        result = self.call(
            "ReceiveMessage",
            QueueUrl=queue_url,
            MaxNumberOfMessages=batch,
            WaitTimeSeconds=20,
            VisibilityTimeout=visibility,
            AttributeNames=["All"],
            MessageAttributeNames=["All"],
        )
        return result.get("Messages", [])

    def delete(self, queue_url: str, receipt_handle: str) -> None:
        self.call("DeleteMessage", QueueUrl=queue_url, ReceiptHandle=receipt_handle)


def failed_ids(body: bytes) -> set[str]:
    """The record ids a handler reported in `batchItemFailures`."""
    try:
        result = json.loads(body) if body else None
    except ValueError:
        return set()
    if not isinstance(result, dict):
        return set()
    return {item.get("itemIdentifier") for item in result.get("batchItemFailures") or [] if isinstance(item, dict)}


def run_schedule(runtime: Runtime, source: dict, stop: threading.Event, clock=time.time) -> None:
    every, offset = float(source["every"]), float(source.get("offset", 0))
    current: Invocation | None = None
    while not stop.is_set():
        at = next_tick(clock(), every, offset)
        if stop.wait(max(0.0, at - clock())):
            return
        if current is not None and not current.finished.is_set():
            log("schedule.skipped", function=runtime.function, reason="still running")
            continue
        event = source["input"] if "input" in source else scheduled_event(runtime.function, at)
        current = runtime.submit(Invocation(event, "schedule"))


def run_queue(runtime: Runtime, sqs: Sqs, source: dict, stop: threading.Event) -> None:
    kind, queue_url = source["type"], source["queue"]
    batch = int(source.get("batch", 1 if kind != "sqs" else 10))
    max_receives = int(source.get("max_receives", 5))
    visibility = int(runtime.timeout) + 30
    while not stop.is_set():
        try:
            messages = sqs.receive(queue_url, batch, visibility)
        except OSError as error:
            log("source.error", function=runtime.function, source=kind, error=type(error).__name__)
            stop.wait(5)
            continue
        for message in list(messages):
            receives = int(message.get("Attributes", {}).get("ApproximateReceiveCount", "1"))
            if receives > max_receives:
                log("message.dropped", function=runtime.function, source=kind, receives=receives)
                sqs.delete(queue_url, message["ReceiptHandle"])
                messages.remove(message)
        if messages:
            dispatch(runtime, sqs, kind, queue_url, messages)


def dispatch(runtime: Runtime, sqs: Sqs, kind: str, queue_url: str, messages: list[dict]) -> None:
    """Invoke the handler for received messages; delete what it handled."""
    if kind == "sqs":
        invocation = runtime.submit(Invocation({"Records": [sqs_record(m, queue_url) for m in messages]}, kind))
        if invocation.wait(runtime.timeout + 60):
            failed = failed_ids(invocation.body)
            for message in messages:
                if message["MessageId"] not in failed:
                    sqs.delete(queue_url, message["ReceiptHandle"])
        return
    for message in messages:
        try:
            notification = json.loads(message.get("Body") or "{}")
        except ValueError:
            notification = {}
        records = notification.get("Records") or []
        if not records:
            log("message.dropped", function=runtime.function, source=kind, reason="not an S3 event")
            sqs.delete(queue_url, message["ReceiptHandle"])
            continue
        events = [notification] if kind == "s3" else [eventbridge_object_created(r) for r in records]
        if all(runtime.submit(Invocation(event, kind)).wait(runtime.timeout + 60) for event in events):
            sqs.delete(queue_url, message["ReceiptHandle"])


def http_handler_for(runtime: Runtime):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def serve(self):
            body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
            event = function_url_event(self.command, self.path, self.headers, body, self.client_address[0], time.time())
            invocation = runtime.submit(Invocation(event, "http"))
            if not invocation.wait(runtime.timeout + 5):
                return self.reply(502, {"content-type": "text/plain"}, b"handler failed")
            self.reply(*function_url_response(invocation.body))

        do_GET = do_HEAD = do_POST = do_PUT = do_DELETE = do_PATCH = do_OPTIONS = serve

        def reply(self, status: int, headers: dict, body: bytes):
            self.send_response(status)
            for key, value in headers.items():
                if key not in ("content-length", "transfer-encoding", "connection"):
                    self.send_header(key, value)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(body)

        def log_message(self, *_):
            pass

    return Handler


# --- Runtime and Invoke API --------------------------------------------------------


def api_handler_for(runtime: Runtime):
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path != f"{API}/invocation/next":
                return self.reply(404)
            invocation, headers = runtime.next()
            self.reply(200, invocation.payload, headers)

        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
            if self.path == f"{API}/init/error":
                log("init.error", function=runtime.function)
                return self.reply(202)
            if match := RESULT.match(self.path):
                return self.reply(202 if runtime.finish(match[1], match[2], body) else 404)
            if INVOKE.match(self.path.split("?")[0]):
                return self.invoke(body)
            self.reply(404)

        def invoke(self, body: bytes):
            invocation = runtime.submit(Invocation(body or b"{}", "invoke"))
            if self.headers.get("X-Amz-Invocation-Type", "RequestResponse") == "Event":
                return self.reply(202)
            invocation.finished.wait(runtime.timeout + 5)
            headers = {"X-Amz-Executed-Version": "$LATEST"}
            if invocation.outcome != "response":
                headers["X-Amz-Function-Error"] = "Unhandled"
            self.reply(200, invocation.body, headers)

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


def supervise(command: list[str], runtime: Runtime, port: int, stop: threading.Event) -> None:
    """Keep the handler running; a crash fails its current invocation."""
    env = {
        **os.environ,
        "AWS_LAMBDA_RUNTIME_API": f"127.0.0.1:{port}",
        "AWS_LAMBDA_FUNCTION_NAME": runtime.function,
        "AWS_LAMBDA_FUNCTION_MEMORY_SIZE": os.environ.get("AWS_LAMBDA_FUNCTION_MEMORY_SIZE", "1024"),
        "AWS_LAMBDA_FUNCTION_VERSION": "$LATEST",
    }
    while not stop.is_set():
        child = subprocess.Popen(command, env=env)
        code = child.wait()
        runtime.abandon()
        if stop.is_set():
            return
        log("handler.exited", function=runtime.function, code=code)
        stop.wait(2)


def main() -> None:
    args = sys.argv[1:]
    if not args or args[0] != "--" or len(args) < 2:
        sys.exit("usage: host.py -- <handler binary> [args...]")
    function = os.environ.get("FUNCTION_NAME") or sys.exit("FUNCTION_NAME is required")
    sources = json.loads(os.environ.get("SOURCES", "[]"))
    runtime = Runtime(function, float(os.environ.get("TIMEOUT_SECONDS", "300")))
    sqs = Sqs(os.environ.get("AWS_URL", "http://localstack:4566"))
    port = int(os.environ.get("PORT", "9001"))
    stop = threading.Event()
    signal.signal(signal.SIGTERM, lambda *_: (stop.set(), sys.exit(0)))

    servers = [ThreadingHTTPServer(("0.0.0.0", port), api_handler_for(runtime))]
    for source in sources:
        kind = source["type"]
        if kind == "schedule":
            target = run_schedule
            threading.Thread(target=target, args=(runtime, source, stop), daemon=True).start()
        elif kind in ("sqs", "s3", "s3-eventbridge"):
            threading.Thread(target=run_queue, args=(runtime, sqs, source, stop), daemon=True).start()
        elif kind == "http":
            servers.append(ThreadingHTTPServer(("0.0.0.0", int(source["port"])), http_handler_for(runtime)))
        else:
            sys.exit(f"unknown source type {kind!r}")
    for server in servers:
        server.daemon_threads = True
        threading.Thread(target=server.serve_forever, daemon=True).start()
    log("host.ready", function=function, sources=[s["type"] for s in sources])
    supervise(args[1:], runtime, port, stop)


if __name__ == "__main__":
    main()
