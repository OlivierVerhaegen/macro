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


if __name__ == "__main__":
    unittest.main()
