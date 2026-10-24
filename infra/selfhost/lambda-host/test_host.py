import base64
import json
import threading
import time
import unittest
import urllib.error
import urllib.parse
import urllib.request
from http.server import ThreadingHTTPServer

from host import (
    Invocation,
    Runtime,
    api_handler_for,
    dispatch,
    eventbridge_object_created,
    failed_ids,
    function_url_event,
    function_url_response,
    http_handler_for,
    next_tick,
    run_schedule,
    scheduled_event,
    sqs_record,
)


def s3_notification(key: str, bucket: str = "doc-storage") -> dict:
    # As the upload relay sends it.
    return {
        "Records": [
            {
                "eventName": "ObjectCreated:Put",
                "eventTime": "2026-10-24T10:00:00.000Z",
                "s3": {"bucket": {"name": bucket}, "object": {"key": key, "size": 3, "eTag": "e"}},
            }
        ]
    }


class FakeHandler:
    """Plays the Rust handler: takes invocations off the runtime and answers."""

    def __init__(self, runtime: Runtime, answer=lambda event: ("response", b"null")):
        self.runtime, self.answer, self.events = runtime, answer, []
        threading.Thread(target=self.loop, daemon=True).start()

    def loop(self):
        while True:
            invocation, headers = self.runtime.next()
            event = json.loads(invocation.payload)
            self.events.append(event)
            outcome, body = self.answer(event)
            self.runtime.finish(headers["Lambda-Runtime-Aws-Request-Id"], outcome, body)


class FakeSqs:
    def __init__(self):
        self.deleted = []

    def delete(self, queue_url, receipt_handle):
        self.deleted.append(receipt_handle)


def message(id_: str, body) -> dict:
    return {"MessageId": id_, "ReceiptHandle": f"rh-{id_}", "Body": json.dumps(body), "Attributes": {}}


class EventTest(unittest.TestCase):
    def test_next_tick_with_offset_is_a_daily_cron(self):
        day, eight = 86400, 8 * 3600
        self.assertEqual(next_tick(0, day, eight), eight)
        self.assertEqual(next_tick(eight, day, eight), day + eight)
        self.assertEqual(next_tick(120.0, 60), 180.0)

    def test_scheduled_event_has_the_eventbridge_shape(self):
        event = scheduled_event("email-scheduled", 0)
        self.assertEqual((event["detail-type"], event["source"]), ("Scheduled Event", "aws.events"))
        self.assertEqual(event["time"], "1970-01-01T00:00:00Z")

    def test_sqs_record(self):
        record = sqs_record({"MessageId": "m", "ReceiptHandle": "r", "Body": "b"}, "http://x/000000000000/q-1")
        self.assertEqual(record["eventSourceARN"], "arn:aws:sqs:us-east-1:000000000000:q-1")
        self.assertEqual((record["messageId"], record["body"], record["eventSource"]), ("m", "b", "aws:sqs"))

    def test_sqs_record_message_attributes_are_camel_case(self):
        message = {
            "MessageId": "m",
            "ReceiptHandle": "r",
            "MessageAttributes": {"chat_id": {"StringValue": "c1", "DataType": "String"}},
        }
        record = sqs_record(message, "http://x/q")
        self.assertEqual(record["messageAttributes"], {"chat_id": {"stringValue": "c1", "dataType": "String"}})

    def test_eventbridge_object_created_decodes_the_key(self):
        notification = s3_notification("user/a+b%2Bc.pdf")
        event = eventbridge_object_created(notification["Records"][0])
        self.assertEqual((event["source"], event["detail-type"]), ("aws.s3", "Object Created"))
        self.assertEqual(event["detail"]["object"]["key"], "user/a b+c.pdf")
        self.assertEqual(event["detail"]["bucket"]["name"], "doc-storage")
        self.assertEqual(event["detail"]["reason"], "Put")

    def test_function_url_event(self):
        event = function_url_event(
            "GET", "/file/abc/size=1080?x=1&x=2", {"Host": "h", "Cookie": "a=1; b=2"}, b"", "1.2.3.4", 0
        )
        self.assertEqual((event["version"], event["rawPath"], event["rawQueryString"]), ("2.0", "/file/abc/size=1080", "x=1&x=2"))
        self.assertEqual(event["queryStringParameters"], {"x": "1,2"})
        self.assertEqual(event["cookies"], ["a=1", "b=2"])
        self.assertEqual(event["requestContext"]["http"]["method"], "GET")
        self.assertNotIn("body", event)
        binary = function_url_event("POST", "/", {}, b"\xff\x00", "1.2.3.4", 0)
        self.assertTrue(binary["isBase64Encoded"])
        self.assertEqual(base64.b64decode(binary["body"]), b"\xff\x00")

    def test_function_url_response(self):
        image = base64.b64encode(b"\x89PNG").decode()
        result = json.dumps({"statusCode": 201, "headers": {"Content-Type": "image/png"}, "body": image, "isBase64Encoded": True})
        self.assertEqual(function_url_response(result.encode()), (201, {"content-type": "image/png"}, b"\x89PNG"))
        self.assertEqual(function_url_response(b'"plain"')[0], 200)

    def test_failed_ids(self):
        self.assertEqual(failed_ids(b'{"batchItemFailures":[{"itemIdentifier":"a"}]}'), {"a"})
        self.assertEqual(failed_ids(b"null"), set())


class DispatchTest(unittest.TestCase):
    def setUp(self):
        self.runtime = Runtime("f", 5)
        self.sqs = FakeSqs()

    def test_sqs_deletes_what_the_handler_handled(self):
        FakeHandler(self.runtime, lambda e: ("response", b'{"batchItemFailures":[{"itemIdentifier":"2"}]}'))
        dispatch(self.runtime, self.sqs, "sqs", "http://x/q", [message("1", "a"), message("2", "b")])
        self.assertEqual(self.sqs.deleted, ["rh-1"])

    def test_sqs_keeps_everything_when_the_handler_fails(self):
        FakeHandler(self.runtime, lambda e: ("error", b'{"errorType":"Boom"}'))
        dispatch(self.runtime, self.sqs, "sqs", "http://x/q", [message("1", "a")])
        self.assertEqual(self.sqs.deleted, [])

    def test_s3_passes_the_notification_as_the_event(self):
        handler = FakeHandler(self.runtime)
        dispatch(self.runtime, self.sqs, "s3", "http://x/q", [message("1", s3_notification("k"))])
        self.assertEqual(handler.events[0]["Records"][0]["s3"]["object"]["key"], "k")
        self.assertEqual(self.sqs.deleted, ["rh-1"])

    def test_s3_eventbridge_sends_one_event_per_record(self):
        handler = FakeHandler(self.runtime)
        notification = s3_notification("a")
        notification["Records"].append(s3_notification("b")["Records"][0])
        dispatch(self.runtime, self.sqs, "s3-eventbridge", "http://x/q", [message("1", notification)])
        self.assertEqual([e["detail"]["object"]["key"] for e in handler.events], ["a", "b"])
        self.assertEqual(self.sqs.deleted, ["rh-1"])

    def test_a_message_that_is_no_s3_event_is_dropped(self):
        FakeHandler(self.runtime)
        dispatch(self.runtime, self.sqs, "s3", "http://x/q", [message("1", {"hello": 1})])
        self.assertEqual(self.sqs.deleted, ["rh-1"])

    def test_a_crashed_handler_fails_its_invocation(self):
        invocation = self.runtime.submit(Invocation({}, "test"))
        self.runtime.next()
        self.runtime.abandon()
        self.assertFalse(invocation.wait(1))


class ScheduleTest(unittest.TestCase):
    def test_ticks_carry_the_input_and_skip_while_running(self):
        runtime = Runtime("f", 5)
        stop = threading.Event()
        source = {"type": "schedule", "every": 0.05, "input": {"dry_run": True}}
        threading.Thread(target=run_schedule, args=(runtime, source, stop), daemon=True).start()
        invocation, _ = runtime.next()
        self.assertEqual(json.loads(invocation.payload), {"dry_run": True})
        time.sleep(0.2)  # Several ticks pass while the first run is still going.
        stop.set()
        self.assertTrue(runtime.pending.empty())


class ApiTest(unittest.TestCase):
    """The HTTP contract `lambda_runtime` and the AWS SDK's Lambda client rely on."""

    def setUp(self):
        self.runtime = Runtime("image-optimizer", 5)
        self.servers = [
            ThreadingHTTPServer(("127.0.0.1", 0), api_handler_for(self.runtime)),
            ThreadingHTTPServer(("127.0.0.1", 0), http_handler_for(self.runtime)),
        ]
        for server in self.servers:
            threading.Thread(target=server.serve_forever, daemon=True).start()
        self.api = f"http://127.0.0.1:{self.servers[0].server_port}"
        self.http = f"http://127.0.0.1:{self.servers[1].server_port}"

    def tearDown(self):
        for server in self.servers:
            server.shutdown()
            server.server_close()

    def post(self, url, body=b"", headers=None):
        request = urllib.request.Request(url, data=body, method="POST", headers=headers or {})
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                return response.status, dict(response.headers), response.read()
        except urllib.error.HTTPError as error:
            return error.code, dict(error.headers), error.read()

    def next(self):
        with urllib.request.urlopen(self.api + "/2018-06-01/runtime/invocation/next", timeout=10) as response:
            return response.headers["Lambda-Runtime-Aws-Request-Id"], response.headers, json.load(response)

    def respond(self, request_id, body):
        return self.post(f"{self.api}/2018-06-01/runtime/invocation/{request_id}/response", body)[0]

    def test_next_then_response_once(self):
        self.runtime.submit(Invocation({"k": 1}, "test"))
        request_id, headers, event = self.next()
        self.assertTrue(headers["Lambda-Runtime-Deadline-Ms"].isdigit())
        self.assertEqual(event, {"k": 1})
        self.assertEqual(self.respond(request_id, b"null"), 202)
        self.assertEqual(self.respond(request_id, b"null"), 404)

    def test_async_invoke_returns_at_once(self):
        status, _, _ = self.post(
            self.api + "/2015-03-31/functions/image-optimizer/invocations",
            b'{"size":1}',
            {"X-Amz-Invocation-Type": "Event"},
        )
        self.assertEqual(status, 202)
        _, _, event = self.next()
        self.assertEqual(event, {"size": 1})

    def test_sync_invoke_returns_the_result(self):
        def handler():
            request_id, _, _ = self.next()
            self.post(f"{self.api}/2018-06-01/runtime/invocation/{request_id}/error", b'{"errorType":"Boom"}')

        threading.Thread(target=handler, daemon=True).start()
        status, headers, body = self.post(self.api + "/2015-03-31/functions/image-optimizer/invocations", b"{}")
        self.assertEqual((status, headers.get("X-Amz-Function-Error")), (200, "Unhandled"))
        self.assertEqual(json.loads(body), {"errorType": "Boom"})

    def test_http_request_becomes_a_function_url_invocation(self):
        def handler():
            request_id, _, event = self.next()
            result = {"statusCode": 200, "headers": {"content-type": "text/plain"}, "body": event["rawPath"]}
            self.respond(request_id, json.dumps(result).encode())

        threading.Thread(target=handler, daemon=True).start()
        with urllib.request.urlopen(self.http + "/file/abc/size=64", timeout=10) as response:
            self.assertEqual((response.status, response.read()), (200, b"/file/abc/size=64"))


if __name__ == "__main__":
    unittest.main()
