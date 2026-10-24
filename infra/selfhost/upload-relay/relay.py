#!/usr/bin/env python3
"""The front of the S3 store (Garage): bucket notifications and public access.

Every S3 request (from the services through the AWS gateway, and from browsers
through the public storage route) passes through here to Garage. After a
successful write it sends the S3 event notifications that AWS would send for
the bucket (NOTIFY): `ObjectCreated:Put`, `:Copy` or
`:CompleteMultipartUpload`. Upload parts and failed writes send nothing.

The internal listener (PORT, for the AWS gateway) forwards requests unchanged;
Garage checks the services' signatures. The public listener (PUBLIC_PORT, for
the public Caddy) sees presigned URLs that the services signed for the
internal host (SIGNED_HOST) and that the code rewrote to the public address. It
checks the signature and expiry itself (a forged or stale URL never reaches the
store) and forwards the request with the internal host, so Garage validates the
same signature. Unsigned GET and HEAD of a PUBLIC_READ bucket (the static-file
CDN) it signs itself, as CloudFront signs requests to its S3 origin.

Environment: S3_UPSTREAM (host:port of Garage), NOTIFY (comma-separated
`bucket=queue-url`, or `bucket/prefix*suffix=queue-url` to filter keys like an
S3 notification filter; a bucket may have several), PORT (default 8080),
PUBLIC_PORT (default 8081), LOCAL_AWS_ACCESS_KEY_ID and
LOCAL_AWS_SECRET_ACCESS_KEY (the services' credentials), SIGNED_HOST (default
localstack:4566), PUBLIC_READ (comma-separated buckets). The queue is
SQS-compatible (ElasticMQ, behind the AWS gateway) and reached without signing.
"""

from __future__ import annotations

import hashlib
import hmac
import http.client
import json
import os
import sys
import threading
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


class Target:
    """One notification: objects created in `bucket` matching prefix and suffix."""

    def __init__(self, bucket: str, prefix: str, suffix: str, queue: str):
        self.bucket, self.prefix, self.suffix, self.queue = bucket, prefix, suffix, queue

    def matches(self, bucket: str, key: str) -> bool:
        return bucket == self.bucket and key.startswith(self.prefix) and key.endswith(self.suffix)


def parse_notify(value: str) -> list[Target]:
    targets = []
    for entry in filter(None, (part.strip() for part in value.split(","))):
        source, sep, queue = entry.partition("=")
        bucket, _, pattern = source.partition("/")
        prefix, star, suffix = pattern.partition("*")
        if not sep or not bucket or not queue or (pattern and not star and "*" in pattern):
            raise ValueError(f"NOTIFY entry must be bucket[/prefix*suffix]=queue-url, got {entry!r}")
        targets.append(Target(bucket, prefix, suffix if star else "", queue))
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


REGION = "us-east-1"
UNSIGNED = "UNSIGNED-PAYLOAD"


def signing_key(secret: str, date: str, region: str = REGION, service: str = "s3") -> bytes:
    key = ("AWS4" + secret).encode()
    for part in (date, region, service, "aws4_request"):
        key = hmac.new(key, part.encode(), hashlib.sha256).digest()
    return key


def canonical_query(params: list[tuple[str, str]]) -> str:
    return "&".join(f"{quote(k)}={quote(v)}" for k, v in sorted(params))


def signature(secret: str, amz_date: str, canonical: str) -> str:
    scope = f"{amz_date[:8]}/{REGION}/s3/aws4_request"
    to_sign = "\n".join(["AWS4-HMAC-SHA256", amz_date, scope, hashlib.sha256(canonical.encode()).hexdigest()])
    return hmac.new(signing_key(secret, amz_date[:8]), to_sign.encode(), hashlib.sha256).hexdigest()


def sign_headers(
    method: str, path: str, query: str, host: str, access_key: str, secret: str, now: float, payload_hash: str = UNSIGNED
) -> dict:
    """SigV4 headers for a request; the relay streams bodies, so their payload is unsigned."""
    amz_date = datetime.fromtimestamp(now, timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    signed = "host;x-amz-content-sha256;x-amz-date"
    params = urllib.parse.parse_qsl(query, keep_blank_values=True)
    canonical = "\n".join(
        [method, path, canonical_query(params), f"host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n", signed, payload_hash]
    )
    credential = f"{access_key}/{amz_date[:8]}/{REGION}/s3/aws4_request"
    return {
        "Host": host,
        "x-amz-date": amz_date,
        "x-amz-content-sha256": payload_hash,
        "Authorization": f"AWS4-HMAC-SHA256 Credential={credential}, SignedHeaders={signed}, Signature={signature(secret, amz_date, canonical)}",
    }


class PresignedAccess:
    """Allow a public request only with a valid presigned SigV4 query signature."""

    def __init__(self, access_key: str, secret_key: str, signed_host: str, public_read: set[str], clock=time.time):
        self.access_key = access_key
        self.secret_key = secret_key
        self.signed_host = signed_host
        self.public_read = public_read
        self.clock = clock

    def is_public_read(self, method: str, path: str, query: str) -> bool:
        """An unsigned read of a public bucket, which the relay signs itself."""
        if "X-Amz-Signature" in dict(urllib.parse.parse_qsl(query, keep_blank_values=True)):
            return False
        bucket, _, key = path.lstrip("/").partition("/")
        return method in ("GET", "HEAD") and bucket in self.public_read and bool(key)

    def denied(self, method: str, path: str, query: str, headers) -> str | None:
        """None when allowed, else the reason (for the log, never the client)."""
        params = urllib.parse.parse_qsl(query, keep_blank_values=True)
        values = dict(params)
        if "X-Amz-Signature" not in values:
            return None if self.is_public_read(method, path, query) else "unsigned"
        if values.get("X-Amz-Algorithm") != "AWS4-HMAC-SHA256":
            return "algorithm"
        credential = values.get("X-Amz-Credential", "").split("/")
        if len(credential) != 5 or credential[0] != self.access_key or credential[3:] != ["s3", "aws4_request"]:
            return "credential"
        try:
            signed_at = datetime.strptime(values["X-Amz-Date"], "%Y%m%dT%H%M%SZ").replace(tzinfo=timezone.utc)
            expires = int(values["X-Amz-Expires"])
        except (KeyError, ValueError):
            return "date"
        if not signed_at.strftime("%Y%m%d") == credential[1]:
            return "date"
        if not signed_at.timestamp() - 300 <= self.clock() <= signed_at.timestamp() + expires:
            return "expired"
        signed_headers = values.get("X-Amz-SignedHeaders", "host").split(";")
        header_lines = []
        for name in signed_headers:
            value = self.signed_host if name == "host" else headers.get(name)
            if value is None:
                return "headers"
            header_lines.append(f"{name}:{' '.join(value.split())}\n")
        if credential[2] != REGION:
            return "credential"
        canonical = "\n".join(
            [
                method,
                path,
                canonical_query([(k, v) for k, v in params if k != "X-Amz-Signature"]),
                "".join(header_lines),
                ";".join(signed_headers),
                values.get("X-Amz-Content-Sha256", UNSIGNED),
            ]
        )
        expected = signature(self.secret_key, values["X-Amz-Date"], canonical)
        if not hmac.compare_digest(expected, values["X-Amz-Signature"]):
            return "signature"
        return None


def quote(value: str) -> str:
    """SigV4's URI encoding: everything but unreserved characters."""
    return urllib.parse.quote(value, safe="-_.~")


def handler_for(upstream: str, targets: list[Target], send=send_message, access: PresignedAccess | None = None):
    host, _, port = upstream.partition(":")

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def proxy(self):
            started = time.monotonic()
            path, _, query = self.path.partition("?")
            if access and (reason := access.denied(self.command, path, query, self.headers)):
                log("relay.denied", method=self.command, reason=reason)
                # The body was not read, so this connection cannot be reused.
                self.close_connection = True
                self.send_response(403)
                self.send_header("Content-Length", "0")
                self.send_header("Connection", "close")
                self.end_headers()
                return
            headers = {k: v for k, v in self.headers.items() if k.lower() not in HOP_BY_HOP}
            if access:
                # Public requests reach the store as the internal host they were
                # signed for; public reads get the relay's own signature.
                extra = {"Host": access.signed_host}
                if access.is_public_read(self.command, path, query):
                    extra = sign_headers(
                        self.command, path, query, access.signed_host, access.access_key, access.secret_key, time.time()
                    )
                drop = {k.lower() for k in extra} | {"authorization", "x-amz-security-token"}
                headers = {k: v for k, v in headers.items() if k.lower() not in drop} | extra
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
            if not created:
                return
            bucket, key, name = created
            size = int(self.headers.get("x-amz-decoded-content-length") or length or 0) if name.endswith("Put") else None
            message = notification(bucket, key, name, size, etag, time.time())
            for target in (t for t in targets if t.matches(bucket, key)):
                try:
                    send(target.queue, message)
                    log("relay.notified", bucket=bucket, event_name=name, queue=target.queue.rsplit("/", 1)[-1])
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
    access = PresignedAccess(
        os.environ.get("LOCAL_AWS_ACCESS_KEY_ID") or sys.exit("LOCAL_AWS_ACCESS_KEY_ID is required"),
        os.environ.get("LOCAL_AWS_SECRET_ACCESS_KEY") or sys.exit("LOCAL_AWS_SECRET_ACCESS_KEY is required"),
        os.environ.get("SIGNED_HOST", "localstack:4566"),
        {b.strip() for b in os.environ.get("PUBLIC_READ", "").split(",") if b.strip()},
    )
    servers = [
        ThreadingHTTPServer(("0.0.0.0", int(os.environ.get("PORT", "8080"))), handler_for(upstream, targets)),
        ThreadingHTTPServer(
            ("0.0.0.0", int(os.environ.get("PUBLIC_PORT", "8081"))), handler_for(upstream, targets, access=access)
        ),
    ]
    for server in servers:
        server.daemon_threads = True
        threading.Thread(target=server.serve_forever, daemon=True).start()
    log("relay.ready", buckets=sorted({t.bucket for t in targets}), public_read=sorted(access.public_read))
    threading.Event().wait()


if __name__ == "__main__":
    main()
