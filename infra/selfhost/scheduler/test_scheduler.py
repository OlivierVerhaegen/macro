import json
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from scheduler import next_tick, run, send_message


class SchedulerTest(unittest.TestCase):
    def test_daily_cron_at_eight(self):
        self.assertEqual(next_tick(0, 86400, 8 * 3600), 8 * 3600)
        self.assertEqual(next_tick(9 * 3600, 86400, 8 * 3600), 86400 + 8 * 3600)

    def test_sends_the_body_every_interval(self):
        sent, stop = [], threading.Event()
        schedule = {"name": "sweep", "every": 0.05, "queue": "q", "body": {"operation": "sweep"}}
        threading.Thread(target=run, args=(schedule, lambda q, b: sent.append((q, b)), stop), daemon=True).start()
        time.sleep(0.18)
        stop.set()
        self.assertGreaterEqual(len(sent), 2)
        self.assertEqual(sent[0], ("q", {"operation": "sweep"}))

    def test_a_failed_send_does_not_stop_the_schedule(self):
        calls, stop = [], threading.Event()

        def send(queue, body):
            calls.append(body)
            raise ConnectionRefusedError()

        schedule = {"name": "s", "every": 0.05, "queue": "q", "body": {}}
        threading.Thread(target=run, args=(schedule, send, stop), daemon=True).start()
        time.sleep(0.18)
        stop.set()
        self.assertGreaterEqual(len(calls), 2)

    def test_send_message_speaks_the_sqs_json_protocol(self):
        received = []

        class Sqs(BaseHTTPRequestHandler):
            def do_POST(self):
                received.append((self.headers["X-Amz-Target"], json.loads(self.rfile.read(int(self.headers["Content-Length"])))))
                self.send_response(200)
                self.send_header("Content-Length", "2")
                self.end_headers()
                self.wfile.write(b"{}")

            def log_message(self, *_):
                pass

        server = ThreadingHTTPServer(("127.0.0.1", 0), Sqs)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            send_message(f"http://127.0.0.1:{server.server_port}", "http://x/q", {"operation": "sweep"})
        finally:
            server.shutdown()
            server.server_close()
        [(target, params)] = received
        self.assertEqual(target, "AmazonSQS.SendMessage")
        self.assertEqual(params, {"QueueUrl": "http://x/q", "MessageBody": '{"operation": "sweep"}'})


if __name__ == "__main__":
    unittest.main()
