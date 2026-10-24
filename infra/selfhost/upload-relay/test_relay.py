import http.client
import json
import threading
import unittest
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from relay import created_event, handler_for, notification, parse_notify


def decode_like_the_finalizer(key: str) -> str:
    # services/document_upload_finalizer_handler/src/inbound/s3_notification.rs
    return urllib.parse.unquote(key.replace("+", " "))


class EventTest(unittest.TestCase):
    def test_parse_notify(self):
        self.assertEqual(parse_notify("a=http://q/1, b=http://q/2"), {"a": "http://q/1", "b": "http://q/2"})
        self.assertEqual(parse_notify(""), {})
        with self.assertRaises(ValueError):
            parse_notify("bucket-without-queue")

    def test_which_requests_create_objects(self):
        self.assertEqual(created_event("PUT", "/b/k%20x", "", {}), ("b", "k x", "ObjectCreated:Put"))
        self.assertEqual(
            created_event("PUT", "/b/k", "", {"x-amz-copy-source": "/b/o"}), ("b", "k", "ObjectCreated:Copy")
        )
        self.assertEqual(
            created_event("POST", "/b/k", "uploadId=1", {}), ("b", "k", "ObjectCreated:CompleteMultipartUpload")
        )
        self.assertIsNone(created_event("PUT", "/b/k", "partNumber=1&uploadId=1", {}))
        self.assertIsNone(created_event("POST", "/b/k", "uploads", {}))
        self.assertIsNone(created_event("PUT", "/b/k", "tagging", {}))
        self.assertIsNone(created_event("PUT", "/b", "", {}))
        self.assertIsNone(created_event("GET", "/b/k", "", {}))

    def test_notification_key_round_trips_through_the_finalizer_decoding(self):
        key = "user/doc 1/ä+b%.docx"
        event = notification("doc-storage", key, "ObjectCreated:Put", 3, '"e"', 0)
        record = event["Records"][0]
        self.assertEqual(decode_like_the_finalizer(record["s3"]["object"]["key"]), key)
        self.assertEqual(record["s3"]["bucket"]["name"], "doc-storage")
        self.assertEqual(record["eventName"], "ObjectCreated:Put")
        self.assertEqual(record["s3"]["object"]["eTag"], "e")


class FakeS3(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    stored: dict = {}

    def do_PUT(self):
        if "chunked" in self.headers.get("Transfer-Encoding", ""):
            body = b""
            while size := int(self.rfile.readline().strip(), 16):
                body += self.rfile.read(size)
                self.rfile.readline()
            self.rfile.readline()
        else:
            body = self.rfile.read(int(self.headers["Content-Length"]))
        if self.path.startswith("/missing/"):
            return self.reply(404, b"<Error/>")
        FakeS3.stored[self.path] = (body, self.headers.get("Host"))
        self.reply(200, b"", {"ETag": '"abc"'})

    def do_GET(self):
        body = FakeS3.stored.get(self.path, (b"",))[0]
        self.reply(200, body)

    def reply(self, status, body, headers=None):
        self.send_response(status)
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


class RelayTest(unittest.TestCase):
    def setUp(self):
        FakeS3.stored = {}
        self.sent = []
        self.s3 = ThreadingHTTPServer(("127.0.0.1", 0), FakeS3)
        upstream = f"127.0.0.1:{self.s3.server_port}"
        handler = handler_for(upstream, {"doc-storage": "http://queue"}, send=lambda q, m: self.sent.append((q, m)))
        self.relay = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        for server in (self.s3, self.relay):
            threading.Thread(target=server.serve_forever, daemon=True).start()

    def tearDown(self):
        for server in (self.s3, self.relay):
            server.shutdown()
            server.server_close()

    def request(self, method, path, body=None, headers=None, chunked=False):
        connection = http.client.HTTPConnection("127.0.0.1", self.relay.server_port)
        connection.request(method, path, body=body, headers={"Host": "macro.example", **(headers or {})},
                           encode_chunked=chunked)
        response = connection.getresponse()
        data = response.read()
        connection.close()
        return response, data

    def test_put_is_stored_and_notified(self):
        payload = b"x" * 200_000
        response, _ = self.request("PUT", "/doc-storage/u/a%20b.pdf?X-Amz-Signature=s", payload)
        self.assertEqual(response.status, 200)
        self.assertEqual(response.getheader("ETag"), '"abc"')
        body, host = FakeS3.stored["/doc-storage/u/a%20b.pdf?X-Amz-Signature=s"]
        self.assertEqual(body, payload)
        self.assertEqual(host, "macro.example")
        [(queue, message)] = self.sent
        record = message["Records"][0]
        self.assertEqual(queue, "http://queue")
        self.assertEqual(decode_like_the_finalizer(record["s3"]["object"]["key"]), "u/a b.pdf")
        self.assertEqual(record["s3"]["object"]["size"], 200_000)

    def test_chunked_upload_is_forwarded(self):
        response, _ = self.request("PUT", "/doc-storage/k", iter([b"ab", b"cd"]), chunked=True)
        self.assertEqual(response.status, 200)
        self.assertEqual(FakeS3.stored["/doc-storage/k"][0], b"abcd")
        self.assertEqual(len(self.sent), 1)

    def test_other_buckets_and_failures_send_nothing(self):
        self.request("PUT", "/static-file-storage/k", b"a")
        response, _ = self.request("PUT", "/missing/k", b"a")
        self.assertEqual(response.status, 404)
        self.assertEqual(self.sent, [])

    def test_reads_pass_through(self):
        self.request("PUT", "/other/k", b"hello")
        response, data = self.request("GET", "/other/k")
        self.assertEqual((response.status, data), (200, b"hello"))


if __name__ == "__main__":
    unittest.main()


try:
    import botocore.session
    from botocore.config import Config
except ImportError:  # The relay itself needs only the standard library.
    botocore = None


@unittest.skipUnless(botocore, "botocore is not installed")
class PresignedAccessTest(unittest.TestCase):
    """Signatures as an AWS SDK makes them, for the internal host the services use."""

    def setUp(self):
        from relay import PresignedAccess

        session = botocore.session.get_session()
        self.s3 = session.create_client(
            "s3",
            region_name="us-east-1",
            endpoint_url="http://localstack:4566",
            aws_access_key_id="test",
            aws_secret_access_key="secret",
            config=Config(s3={"addressing_style": "path"}, signature_version="s3v4"),
        )
        self.access = PresignedAccess("test", "secret", "localstack:4566", {"static-file-storage"})

    def check(self, url: str, method: str, headers=None, access=None):
        parts = urllib.parse.urlsplit(url)
        return (access or self.access).denied(method, parts.path, parts.query, headers or {})

    def presign(self, operation: str, key: str, **params):
        return self.s3.generate_presigned_url(operation, Params={"Bucket": "doc-storage", "Key": key, **params})

    def test_accepts_sdk_signatures(self):
        for key in ["a.txt", "user/doc 1/ä+b%.pdf", "x y/(1)!*'.docx"]:
            self.assertIsNone(self.check(self.presign("get_object", key), "GET"), key)
            self.assertIsNone(self.check(self.presign("put_object", key), "PUT"), key)

    def test_signed_headers_must_match(self):
        url = self.presign("put_object", "a.pdf", ContentType="application/pdf")
        if "content-type" in urllib.parse.parse_qs(urllib.parse.urlsplit(url).query).get("X-Amz-SignedHeaders", [""])[0]:
            self.assertIsNone(self.check(url, "PUT", {"content-type": "application/pdf"}))
            self.assertEqual(self.check(url, "PUT", {"content-type": "text/html"}), "signature")
        else:
            self.assertIsNone(self.check(url, "PUT"))

    def test_rejects_tampering(self):
        url = self.presign("get_object", "mine.txt")
        self.assertEqual(self.check(url.replace("mine.txt", "yours.txt"), "GET"), "signature")
        self.assertEqual(self.check(url, "PUT"), "signature")
        self.assertEqual(self.check(url, "GET", access=type(self.access)("test", "other", "localstack:4566", set())), "signature")
        self.assertEqual(self.check(url, "GET", access=type(self.access)("other", "secret", "localstack:4566", set())), "credential")

    def test_rejects_expired(self):
        url = self.s3.generate_presigned_url(
            "get_object", Params={"Bucket": "doc-storage", "Key": "k"}, ExpiresIn=60
        )
        later = type(self.access)("test", "secret", "localstack:4566", set(), clock=lambda: time.time() + 120)
        self.assertEqual(self.check(url, "GET", access=later), "expired")

    def test_unsigned_only_reads_public_buckets(self):
        self.assertIsNone(self.access.denied("GET", "/static-file-storage/a.png", "", {}))
        self.assertIsNone(self.access.denied("HEAD", "/static-file-storage/a.png", "", {}))
        self.assertEqual(self.access.denied("PUT", "/static-file-storage/a.png", "", {}), "unsigned")
        self.assertEqual(self.access.denied("GET", "/doc-storage/a.pdf", "", {}), "unsigned")
        self.assertEqual(self.access.denied("GET", "/doc-storage", "list-type=2", {}), "unsigned")


class PublicListenerTest(RelayTest):
    def setUp(self):
        from relay import PresignedAccess

        FakeS3.stored = {}
        self.sent = []
        self.s3 = ThreadingHTTPServer(("127.0.0.1", 0), FakeS3)
        access = PresignedAccess("test", "secret", "localstack:4566", {"static-file-storage"})
        handler = handler_for(
            f"127.0.0.1:{self.s3.server_port}", {"doc-storage": "q"}, send=lambda q, m: self.sent.append(m), access=access
        )
        self.relay = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        for server in (self.s3, self.relay):
            threading.Thread(target=server.serve_forever, daemon=True).start()

    # The inherited tests send unsigned writes; here they must all be refused.
    def test_put_is_stored_and_notified(self):
        response, _ = self.request("PUT", "/doc-storage/k", b"x" * 100_000)
        self.assertEqual(response.status, 403)
        self.assertEqual((FakeS3.stored, self.sent), ({}, []))

    test_chunked_upload_is_forwarded = test_other_buckets_and_failures_send_nothing = test_put_is_stored_and_notified

    def test_reads_pass_through(self):
        FakeS3.stored["/static-file-storage/a.png"] = (b"png", None)
        self.assertEqual(self.request("GET", "/static-file-storage/a.png")[1], b"png")
        self.assertEqual(self.request("GET", "/doc-storage/a.pdf")[0].status, 403)
