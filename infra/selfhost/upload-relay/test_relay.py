import http.client
import json
import threading
import unittest
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
