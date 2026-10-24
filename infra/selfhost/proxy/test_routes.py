"""The public Caddyfile must keep up with the local stack's proxy and compose."""

import re
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
LOCAL = REPO / "tooling/xtask/crates/xtask_local/src/local"
CADDYFILE = (HERE / "Caddyfile").read_text()


def local_prefixes() -> set[str]:
    """Path prefixes the local proxy routes: the inventory's plus its extras."""
    inventory = (LOCAL / "inventory.rs").read_text()
    prefixes = set(re.findall(r'path_prefix: Some\("(/[^"]+)"\)', inventory))
    proxy = (LOCAL / "proxy.rs").read_text()
    extras = re.search(r"\.chain\(\[(.*?)\]\)", proxy, re.S)
    prefixes |= set(re.findall(r'"(/[^"]+)"', extras[1]))
    return prefixes


def compose_names() -> set[str]:
    compose = (HERE.parent / "compose.yml").read_text()
    names = set(re.findall(r"^  ([a-z0-9_-]+):$", compose, re.M))
    names |= set(re.findall(r"^          - ([a-z0-9_-]+)$", compose, re.M))
    return names


class RoutesTest(unittest.TestCase):
    def test_every_local_prefix_is_routed(self):
        prefixes = local_prefixes()
        self.assertIn("/auth", prefixes)
        missing = [
            p for p in sorted(prefixes) if not re.search(rf"(handle_path|handle|path)[^\n]* {re.escape(p)}(/\*|\s)", CADDYFILE)
        ]
        self.assertEqual(missing, [], "add these prefixes to proxy/Caddyfile")

    def test_every_upstream_is_a_compose_service(self):
        upstreams = set(re.findall(r"reverse_proxy (?:@\w+ )?([a-z0-9_-]+):\d+", CADDYFILE))
        self.assertEqual(sorted(upstreams - compose_names()), [], "unknown upstream hosts")

    def test_internal_services_are_not_public(self):
        for host in ["fusionauth", "localstack", "aws-gateway", "s3:", "mailpit", "upload-relay:8080"]:
            self.assertNotIn(f"reverse_proxy {host}", CADDYFILE)


if __name__ == "__main__":
    unittest.main()
