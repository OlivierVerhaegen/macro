import unittest

import render


def provider(name: str) -> dict:
    return {"method": "POST", "url": "/api/identity-provider/x", "body": {"identityProvider": {"name": name}}}


class GmailPlaceholderTest(unittest.TestCase):
    def test_adds_a_disabled_provider_when_missing(self):
        doc = {"requests": [provider("google")]}
        render.add_gmail_placeholder(doc)
        added = doc["requests"][-1]["body"]["identityProvider"]
        self.assertEqual(added["name"], "google_gmail")
        self.assertFalse(added["enabled"])
        self.assertTrue(doc["requests"][-1]["url"].endswith(render.GOOGLE_GMAIL_IDP_ID))

    def test_keeps_a_configured_provider(self):
        doc = {"requests": [provider("google_gmail")]}
        render.add_gmail_placeholder(doc)
        self.assertEqual(len(doc["requests"]), 1)


class QueueUrlTest(unittest.TestCase):
    def test_names_become_urls_and_the_rest_stays(self):
        env = {
            "LOCAL_AWS_URL": "http://localstack:4566",
            "CONTACTS_QUEUE": "contacts-queue",
            "NOTIFICATION_QUEUE": "http://localstack:4566/000000000000/notification-queue",
            "ENABLE_GMAIL_OPS_QUEUE": "true",
            "CONTACTS_BUCKET": "contacts",
        }
        render.queue_urls(env, {"OVERRIDE_CONTACTS_QUEUE": "contacts-queue"})
        self.assertEqual(env["CONTACTS_QUEUE"], "http://localstack:4566/000000000000/contacts-queue")
        self.assertEqual(env["NOTIFICATION_QUEUE"], "http://localstack:4566/000000000000/notification-queue")
        self.assertEqual(env["ENABLE_GMAIL_OPS_QUEUE"], "true")
        self.assertEqual(env["CONTACTS_BUCKET"], "contacts")
        self.assertEqual(env["OVERRIDE_CONTACTS_QUEUE"], "http://localstack:4566/000000000000/contacts-queue")

    def test_typed_queues_come_from_macro_queues(self):
        typed = render.typed_queues(render.TYPED_QUEUES.read_text())
        self.assertEqual(typed["OVERRIDE_CONTACTS_QUEUE"], "contacts-queue")
        self.assertEqual(typed["OVERRIDE_WEBHOOK_EVENT_QUEUE"], "webhook-event-queue.fifo")


if __name__ == "__main__":
    unittest.main()
