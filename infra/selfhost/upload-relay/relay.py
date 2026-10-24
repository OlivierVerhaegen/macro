#!/usr/bin/env python3
"""S3 proxy that sends bucket notifications, which s3proxy does not.

Every S3 request (from the services through the AWS gateway, and from browsers
through the public storage route) passes through here to s3proxy unchanged.
After a successful write to a bucket in NOTIFY, it sends the S3 event
notification that AWS would send to that bucket's queue: `ObjectCreated:Put`,
`:Copy` or `:CompleteMultipartUpload`. Upload parts and failed writes send
nothing.

Environment: S3_UPSTREAM (host:port of s3proxy), NOTIFY (comma-separated
`bucket=queue-url`), PORT (default 8080). The queue is SQS-compatible
(ElasticMQ, behind the AWS gateway) and reached without signing.
"""

from __future__ import annotations

import http.client
import json
import os
import sys
import time
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CHUNK = 1 << 16
# Hop-by-hop headers are the proxy's own business, never forwarded.
HOP_BY_HOP = {"connection", "keep-alive", "proxy-connection", "transfer-encoding", "te", "trailer", "upgrade"}


def log(event: str, **fields) -> None:
    print(json.dumps({"event": event, **fields}), flush=True)


def parse_notify(value: str) -> dict[str, str]:
    targets = {}
    for entry in filter(None, (part.strip() for part in value.split(","))):
        bucket, sep, queue = entry.partition("=")
        if not sep or not bucket or not queue:
            raise ValueError(f"NOTIFY entry must be bucket=queue-url, got {entry!r}")
        targets[bucket] = queue
    return targets


def created_event(method: str, path: str, query: str, headers) -> tuple[str, str, str] | None:
    """(bucket, key, event name) when this request creates an object."""
    bucket, _, raw_key = path.lstrip("/").partition("/")
    if not bucket or not raw_key:
        return None
    params = urllib.parse.parse_qs(query, keep_blank_values=True)
    if method == "PUT" and "uploadId" not in params and not params.keys() & {"acl", "tagging", "retention", "legal-hold"}:
        name = "ObjectCreated:Copy" if headers.get("x-amz-copy-source") else "ObjectCreated:Put"
    elif method == "POST" and "uploadId" in params:
        name = "ObjectCreated:CompleteMultipartUpload"
    else:
        return None
    return bucket, urllib.parse.unquote(raw_key), name


def notification(bucket: str, key: str, name: str, size: int | None, etag: str | None, at: float) -> dict:
    # S3 form-encodes keys in events; consumers decode `+` and `%XX`.
    obj = {"key": urllib.parse.quote_plus(key, safe="/"), "sequencer": f"{int(at * 1e6):016X}"}
    if size is not None:
        obj["size"] = size
    if etag:
        obj["eTag"] = etag.strip('"')
    return {
        "Records": [
            {
                "eventVersion": "2.1",
                "eventSource": "aws:s3",
                "awsRegion": "us-east-1",
                "eventTime": datetime.fromtimestamp(at, timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z",
                "eventName": name,
                "s3": {
                    "s3SchemaVersion": "1.0",
                    "configurationId": f"{bucket}-notification",
                    "bucket": {"name": bucket, "arn": f"arn:aws:s3:::{bucket}"},
                    "object": obj,
                },
            }
        ]
    }


def send_message(queue_url: str, body: dict) -> None:
    # SQS's JSON protocol, like the AWS SDKs: the gateway routes on X-Amz-Target.
    payload = json.dumps({"QueueUrl": queue_url, "MessageBody": json.dumps(body)}).encode()
    request = urllib.request.Request(
        queue_url,
        data=payload,
        headers={"Content-Type": "application/x-amz-json-1.0", "X-Amz-Target": "AmazonSQS.SendMessage"},
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        response.read()


def handler_for(upstream: str, targets: dict[str, str], send=send_message):
    host, _, port = upstream.partition(":")

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def proxy(self):
            started = time.monotonic()
            path, _, query = self.path.partition("?")
            headers = {k: v for k, v in self.headers.items() if k.lower() not in HOP_BY_HOP}
            chunked = "chunked" in self.headers.get("Transfer-Encoding", "").lower()
            length = None if chunked else int(self.headers.get("Content-Length") or 0)
            connection = http.client.HTTPConnection(host, int(port or 80), timeout=300)
            try:
                connection.putrequest(self.command, self.path, skip_host=True, skip_accept_encoding=True)
                for key, value in headers.items():
                    connection.putheader(key, value)
                if chunked:
                    connection.putheader("Transfer-Encoding", "chunked")
                connection.endheaders()
                self.forward_body(connection, chunked, length)
                response = connection.getresponse()
                if 200 <= response.status < 300:
                    # Before answering, so a client that saw 200 can count on the event.
                    self.notify(path, query, length, response.getheader("ETag"))
                self.send_response_only(response.status, response.reason)
                for key, value in response.getheaders():
                    if key.lower() not in HOP_BY_HOP:
                        self.send_header(key, value)
                body_chunked = response.getheader("Content-Length") is None and self.command != "HEAD"
                if body_chunked:
                    self.send_header("Transfer-Encoding", "chunked")
                self.end_headers()
                self.copy_response(response, body_chunked)
            except (OSError, http.client.HTTPException) as error:
                log("relay.upstream_error", method=self.command, error=type(error).__name__)
                self.close_connection = True
                if not self.wfile.closed:
                    try:
                        self.send_error(502)
                    except OSError:
                        pass
                return
            finally:
                connection.close()
            log(
                "relay.request",
                method=self.command,
                status=response.status,
                duration_ms=int((time.monotonic() - started) * 1000),
            )

        def forward_body(self, connection, chunked: bool, length: int | None):
            if chunked:
                while True:
                    size = int(self.rfile.readline().split(b";")[0].strip() or b"0", 16)
                    if not size:
                        # The last chunk; skip any trailers up to the empty line.
                        while self.rfile.readline() not in (b"\r\n", b"\n", b""):
                            pass
                        connection.send(b"0\r\n\r\n")
                        return
                    data = self.rfile.read(size)
                    self.rfile.readline()
                    connection.send(f"{size:X}\r\n".encode() + data + b"\r\n")
            remaining = length or 0
            while remaining:
                data = self.rfile.read(min(CHUNK, remaining))
                if not data:
                    raise ConnectionError("client closed the request body early")
                connection.send(data)
                remaining -= len(data)

        def copy_response(self, response, chunked: bool):
            while data := response.read(CHUNK):
                self.wfile.write(f"{len(data):X}\r\n".encode() + data + b"\r\n" if chunked else data)
            if chunked:
                self.wfile.write(b"0\r\n\r\n")

        def notify(self, path: str, query: str, length: int | None, etag: str | None):
            created = created_event(self.command, path, query, self.headers)
            if not created or created[0] not in targets:
                return
            bucket, key, name = created
            size = int(self.headers.get("x-amz-decoded-content-length") or length or 0) if name.endswith("Put") else None
            message = notification(bucket, key, name, size, etag, time.time())
            try:
                send(targets[bucket], message)
                log("relay.notified", bucket=bucket, event_name=name)
            except OSError as error:
                # The object is stored; only the event is lost. Say so loudly.
                log("relay.notify_failed", bucket=bucket, event_name=name, error=type(error).__name__)

        do_GET = do_HEAD = do_PUT = do_POST = do_DELETE = do_OPTIONS = proxy

        def log_message(self, *_):
            pass

    return Handler


def main() -> None:
    upstream = os.environ.get("S3_UPSTREAM") or sys.exit("S3_UPSTREAM is required")
    targets = parse_notify(os.environ.get("NOTIFY", ""))
    server = ThreadingHTTPServer(("0.0.0.0", int(os.environ.get("PORT", "8080"))), handler_for(upstream, targets))
    server.daemon_threads = True
    log("relay.ready", buckets=sorted(targets))
    server.serve_forever()


if __name__ == "__main__":
    main()
