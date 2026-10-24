import smtplib
import threading
import unittest
from email.message import EmailMessage

from mailrelay import Server, Upstream, handler_for


class FakeUpstream:
    def __init__(self, error=None):
        self.sent, self.error = [], error

    def deliver(self, sender, recipients, data, options):
        if self.error:
            raise self.error
        self.sent.append((sender, recipients, data, options))


class RelayTest(unittest.TestCase):
    def start(self, upstream, max_bytes=1_000_000):
        server = Server(("127.0.0.1", 0), handler_for(upstream, max_bytes))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        return smtplib.SMTP("127.0.0.1", server.server_address[1], timeout=10)

    def message(self, body="Je code is 123456.\n.leading dot line"):
        message = EmailMessage()
        message["From"] = "noreply@example.test"
        message["To"] = "owner@example.test"
        message["Subject"] = "Inloggen bij Macro – code"
        message.set_content(body)
        return message

    def test_delivers_the_message_unchanged(self):
        upstream = FakeUpstream()
        with self.start(upstream) as client:
            client.send_message(self.message(), to_addrs=["owner@example.test", "b@example.test"])
        [(sender, recipients, data, _)] = upstream.sent
        self.assertEqual(sender, "noreply@example.test")
        self.assertEqual(recipients, ["owner@example.test", "b@example.test"])
        self.assertIn(b"\n.leading dot line", data)
        self.assertIn(b"Subject: Inloggen bij Macro =?utf-8?b?4oCT?= code", data)

    def test_upstream_errors_reach_the_sender(self):
        cases = [
            (smtplib.SMTPResponseException(535, b"auth failed"), 554),
            (smtplib.SMTPResponseException(421, b"busy"), 451),
            (ConnectionRefusedError(), 451),
            (smtplib.SMTPRecipientsRefused({}), 550),
        ]
        for error, code in cases:
            with self.start(FakeUpstream(error)) as client:
                with self.assertRaises(smtplib.SMTPDataError) as caught:
                    client.send_message(self.message())
                self.assertEqual(caught.exception.smtp_code, code, error)

    def test_rejects_oversized_messages(self):
        upstream = FakeUpstream()
        with self.start(upstream, max_bytes=100) as client:
            with self.assertRaises(smtplib.SMTPDataError) as caught:
                client.send_message(self.message("x" * 500))
        self.assertEqual(caught.exception.smtp_code, 552)
        self.assertEqual(upstream.sent, [])

    def test_order_of_commands(self):
        with self.start(FakeUpstream()) as client:
            client.ehlo()
            self.assertEqual(client.rcpt("a@example.test")[0], 503)
            self.assertEqual(client.docmd("DATA")[0], 503)
            self.assertTrue(client.has_extn("smtputf8"))


class UpstreamTest(unittest.TestCase):
    def test_uses_starttls_and_login(self):
        calls = []

        class FakeSmtp:
            def __init__(self, host, port, timeout):
                calls.append(("connect", host, port))

            def __enter__(self):
                return self

            def __exit__(self, *_):
                calls.append(("quit",))

            def starttls(self):
                calls.append(("starttls",))

            def login(self, user, password):
                calls.append(("login", user, password))

            def sendmail(self, sender, recipients, data, mail_options):
                calls.append(("sendmail", sender, tuple(recipients)))

        Upstream("smtp.example.test", 587, "user", "app-password", connect=FakeSmtp).deliver(
            "a@example.test", ["b@example.test"], b"x", []
        )
        self.assertEqual(
            calls,
            [
                ("connect", "smtp.example.test", 587),
                ("starttls",),
                ("login", "user", "app-password"),
                ("sendmail", "a@example.test", ("b@example.test",)),
                ("quit",),
            ],
        )


if __name__ == "__main__":
    unittest.main()
