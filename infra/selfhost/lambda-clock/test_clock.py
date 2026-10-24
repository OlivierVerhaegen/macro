import json
import threading
import unittest
import urllib.request
from http.server import ThreadingHTTPServer

from clock import Clock, handler_for, next_tick, scheduled_event


class FakeTime:
    def __init__(self, now: float):
        self.now = now

    def clock(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        self.now += seconds


class ClockTest(unittest.TestCase):
    def test_next_tick_is_the_next_interval_boundary(self):
        self.assertEqual(next_tick(120.0, 60), 180.0)
        self.assertEqual(next_tick(179.9, 60), 180.0)

    def test_scheduled_event_has_the_eventbridge_shape(self):
        event = scheduled_event("email-scheduled", 0)
        self.assertEqual(event["detail-type"], "Scheduled Event")
        self.assertEqual(event["source"], "aws.events")
        self.assertEqual(event["time"], "1970-01-01T00:00:00Z")
        self.assertTrue(event["resources"][0].endswith(":rule/email-scheduled"))

    def test_invocation_waits_for_the_tick_and_sets_the_deadline(self):
        fake = FakeTime(100.0)
        clock = Clock("f", 60, 300, clock=fake.clock, sleep=fake.sleep)
        request_id, headers, event = clock.wait_for_invocation()
        self.assertEqual(fake.now, 120.0)
        self.assertEqual(headers["Lambda-Runtime-Aws-Request-Id"], request_id)
        self.assertEqual(headers["Lambda-Runtime-Deadline-Ms"], str(420_000))
        self.assertEqual(event["time"], "1970-01-01T00:02:00Z")

    def test_finish_accepts_each_invocation_once(self):
        fake = FakeTime(0.0)
        clock = Clock("f", 60, 300, clock=fake.clock, sleep=fake.sleep)
        request_id, _, _ = clock.wait_for_invocation()
        self.assertTrue(clock.finish(request_id, "response", b"null"))
        self.assertFalse(clock.finish(request_id, "response", b"null"))
        self.assertFalse(clock.finish("unknown", "error", b"{}"))


class RuntimeApiTest(unittest.TestCase):
    """The HTTP contract `lambda_runtime` relies on."""

    def setUp(self):
        self.clock = Clock("f", 0.05, 300)
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(self.clock))
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.base = f"http://127.0.0.1:{self.server.server_port}/2018-06-01/runtime"

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()

    def post(self, path: str, body: bytes = b"") -> int:
        request = urllib.request.Request(self.base + path, data=body, method="POST")
        try:
            with urllib.request.urlopen(request) as response:
                return response.status
        except urllib.error.HTTPError as error:
            return error.code

    def test_next_then_response(self):
        with urllib.request.urlopen(self.base + "/invocation/next") as response:
            request_id = response.headers["Lambda-Runtime-Aws-Request-Id"]
            self.assertTrue(response.headers["Lambda-Runtime-Deadline-Ms"].isdigit())
            self.assertEqual(json.load(response)["source"], "aws.events")
        self.assertEqual(self.post(f"/invocation/{request_id}/response", b"null"), 202)
        self.assertEqual(self.post(f"/invocation/{request_id}/response", b"null"), 404)

    def test_error_report_is_accepted(self):
        with urllib.request.urlopen(self.base + "/invocation/next") as response:
            request_id = response.headers["Lambda-Runtime-Aws-Request-Id"]
        body = json.dumps({"errorType": "Boom", "errorMessage": "x"}).encode()
        self.assertEqual(self.post(f"/invocation/{request_id}/error", body), 202)


if __name__ == "__main__":
    unittest.main()
