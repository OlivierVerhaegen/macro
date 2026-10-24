#!/usr/bin/env python3
"""Render the self-hosted configuration from the upstream-generated local env.

`cargo x` generates the full service env and the FusionAuth kickstart for an
instance named `selfhost`. Both carry the local stack's public identity (a JWT
secret, API keys and an internal auth key that anyone with the code knows) and
local addresses. This script turns them into a configuration for one server:

* every secret in ROTATED_KEYS gets a random value; settings that shared one
  old value share one new value, because services compare them with each other;
* the RSA key pair for Macro API tokens is regenerated;
* local origins become the public domain, and mail goes to the SMTP relay;
* the kickstart gets the same secrets, redirect URLs and mail settings.

Secrets are kept in `<dir>/secrets.json`, so rendering again keeps them; the
outputs are `<dir>/selfhost.env` and `<dir>/kickstart/kickstart.json`, all
mode 600; the kickstart belongs to FusionAuth's container user, which must read
it. Nothing secret is written inside the repository.

Usage: render.py [--dir /etc/macro]   (as root, from a checkout, needs cargo)
"""

from __future__ import annotations

import argparse
import configparser
import json
import os
import re
import secrets
import subprocess
import sys
import tempfile
from pathlib import Path
from urllib.parse import urlsplit, urlunsplit

INSTANCE = "selfhost"
REPO = Path(__file__).resolve().parents[2]
GENERATED = REPO / "infra/local/generated" / INSTANCE

# Settings that guard access. Third-party API keys are not here: they are
# placeholders until the operator sets real ones in selfhost.conf [env].
ROTATED_KEYS = [
    "ACCOUNT_LINK_STATE_SECRET",
    "AUTHENTICATION_SERVICE_SECRET_KEY",
    "CAL_WEBHOOK_SECRET_KEY",
    "DOCUMENT_PERMISSION_JWT",
    "DOCUMENT_STORAGE_SERVICE_AUTH_KEY",
    "FUSIONAUTH_API_KEY",
    "FUSIONAUTH_API_KEY_SECRET_KEY",
    "FUSIONAUTH_CLIENT_SECRET_KEY",
    "GITHUB_INSTALLATION_STATE_SECRET",
    "GITHUB_WEBHOOK_SECRET_KEY",
    "INTERNAL_API_KEY",
    "INTERNAL_API_SECRET_KEY",
    "INTERNAL_AUTH_KEY",
    "INTERNAL_CALL_SECRET",
    "JWT_SECRET_KEY",
    "PIPEDREAM_WEBHOOK_SECRET",
    "SERVICE_INTERNAL_AUTH_KEY",
    "STRIPE_WEBHOOK_SECRET_KEY",
    "SYNC_SERVICE_AUTH_KEY",
    "URL_SIGNING_HMAC",
]
# FusionAuth API keys must look like its own (a UUID); the rest are opaque.
UUID_SHAPED = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
API_TOKEN_KEYS = ("MACRO_API_TOKEN_PRIVATE_SECRET_KEY", "MACRO_API_TOKEN_PUBLIC_KEY")
INFRA_PASSWORDS = ("POSTGRES_PASSWORD", "FUSIONAUTH_DB_PASSWORD")
# The FusionAuth image runs as this user and reads the kickstart.
FUSIONAUTH_UID = 1001


def fail(message: str) -> None:
    sys.exit(f"render: {message}")


def cargo_x(*args: str) -> str:
    # The local stack refuses real AWS credentials; none are needed here.
    env = {k: v for k, v in os.environ.items() if not k.startswith("AWS_")}
    result = subprocess.run(
        ["cargo", "x", *args, "--instance", INSTANCE],
        cwd=REPO,
        env=env,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        fail(f"cargo x {args[0]} failed:\n{result.stderr[-2000:]}")
    return result.stdout


def read_dotenv(path: Path) -> dict[str, str]:
    """The generator's dotenv: KEY=value, with double-quoted multi-line values."""
    env: dict[str, str] = {}
    text = path.read_text()
    for match in re.finditer(r'^([A-Z0-9_]+)=("(?:[^"\\]|\\.)*"|[^\n]*)$', text, re.M):
        key, raw = match.group(1), match.group(2)
        env[key] = json.loads(raw) if raw.startswith('"') else raw
    return env


def dotenv_line(key: str, value: str) -> str:
    plain = re.fullmatch(r"[A-Za-z0-9_./:@,+=%-]*", value)
    return f"{key}={value}" if plain else f"{key}={json.dumps(value, ensure_ascii=False)}"


def write_private(path: Path, text: str, owner: int | None = None) -> None:
    """Mode 600 (directory 700), optionally owned by a container's user."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as handle:
        handle.write(text)
    os.chmod(path, 0o600)
    if owner is not None:
        if os.geteuid() != 0:
            fail(f"run as root so {path} can belong to uid {owner}")
        os.chmod(path.parent, 0o700)
        os.chown(path.parent, owner, owner)
        os.chown(path, owner, owner)


def new_secret(old: str) -> str:
    return str(__import__("uuid").uuid4()) if UUID_SHAPED.match(old) else secrets.token_urlsafe(48)


def rsa_key_pair() -> tuple[str, str]:
    with tempfile.TemporaryDirectory() as tmp:
        private = Path(tmp) / "private.pem"
        subprocess.run(
            ["openssl", "genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", str(private)],
            check=True,
            capture_output=True,
        )
        public = subprocess.run(
            ["openssl", "pkey", "-in", str(private), "-pubout"], check=True, capture_output=True, text=True
        ).stdout
        return private.read_text(), public


def load_state(path: Path) -> dict:
    if path.exists():
        return json.loads(path.read_text())
    return {"by_old_value": {}, "api_token": None, "infra": {}}


def rotate(env: dict[str, str], state: dict) -> dict[str, str]:
    """Replace secrets by old value, so shared values stay shared. Returns old→new."""
    mapping = state["by_old_value"]
    for key in ROTATED_KEYS:
        if key not in env:
            fail(f"{key} is no longer generated; review ROTATED_KEYS")
        old = env[key]
        mapping.setdefault(old, new_secret(old))
        env[key] = mapping[old]
    if state["api_token"] is None:
        private, public = rsa_key_pair()
        state["api_token"] = {"private": private, "public": public}
    env[API_TOKEN_KEYS[0]] = state["api_token"]["private"]
    env[API_TOKEN_KEYS[1]] = state["api_token"]["public"]
    for key in INFRA_PASSWORDS:
        state["infra"].setdefault(key, secrets.token_urlsafe(32))
    return mapping


def origin(url: str) -> str:
    parts = urlsplit(url)
    return urlunsplit((parts.scheme, parts.netloc, "", "", ""))


def publish(env: dict[str, str], conf: configparser.ConfigParser, state: dict) -> None:
    """Local addresses become the public domain; infrastructure gets its passwords."""
    domain = conf["server"]["domain"]
    public = f"https://{domain}"
    proxy = origin(env["FRONTEND_ORIGIN"])
    auth = origin(env["FUSIONAUTH_OAUTH_REDIRECT_URI"])
    for key, value in env.items():
        env[key] = value.replace(proxy, public).replace(auth, f"{public}/auth")
    password = state["infra"]["POSTGRES_PASSWORD"]
    for key in ("DATABASE_URL", "DATABASE_URL_READONLY", "MACRO_DB_URL"):
        env[key] = re.sub(r"^postgres://[^@]*@", f"postgres://macro:{password}@", env[key])
    env.update(
        {
            # The generator's fixed internal hosts stay; only the public ones move.
            "FUSIONAUTH_PUBLIC_URL": public,
            "EMAIL_SERVICE_CLOUDFRONT_DISTRIBUTION_URL": f"{public}/static-file",
            "PIPEDREAM_ALLOWED_ORIGINS": public,
            "SENDER_BASE_ADDRESS": conf["mail"]["sender_domain"],
            "SMTP_HOST": "mailrelay",
            "SMTP_PORT": "25",
            "POSTGRES_PASSWORD": password,
            "FUSIONAUTH_DB_PASSWORD": state["infra"]["FUSIONAUTH_DB_PASSWORD"],
        }
    )
    env["PIPEDREAM_WEBHOOK_URI"] = re.sub(
        r"secret=[^&]*", f"secret={env['PIPEDREAM_WEBHOOK_SECRET']}", env["PIPEDREAM_WEBHOOK_URI"]
    )
    for key, value in conf["env"].items() if conf.has_section("env") else []:
        env[key.upper()] = value


def render_kickstart(path: Path, mapping: dict[str, str], env: dict[str, str], conf) -> str:
    doc = json.loads(path.read_text())
    public = f"https://{conf['server']['domain']}"
    mail_from = conf["mail"]["from"]

    def walk(node, key=None):
        if isinstance(node, dict):
            return {k: walk(v, k) for k, v in node.items()}
        if isinstance(node, list):
            return [walk(v, key) for v in node]
        if isinstance(node, str):
            # Exact matches only: "local" is also an ordinary word here.
            if node in mapping:
                return mapping[node]
            if key == "authorizedRedirectURLs":
                parts = urlsplit(node)
                if parts.hostname in ("localhost", "vm") or parts.hostname == urlsplit(env["FRONTEND_ORIGIN"]).hostname:
                    prefix = "/auth" if parts.path == "/oauth/redirect" else ""
                    return f"{public}{prefix}{parts.path}"
        return node

    doc = walk(doc)
    for request in doc["requests"]:
        body = request.get("body", {})
        if "emailTemplate" in body:
            body["emailTemplate"]["fromEmail"] = mail_from
        if "tenant" in body and "emailConfiguration" in body["tenant"]:
            mail = body["tenant"]["emailConfiguration"]
            mail.update({"host": "mailrelay", "port": 25, "security": "NONE", "defaultFromEmail": mail_from})
    # De-duplicate redirect URLs that collapsed onto the same public address.
    for request in doc["requests"]:
        oauth = request.get("body", {}).get("application", {}).get("oauthConfiguration")
        if oauth:
            oauth["authorizedRedirectURLs"] = list(dict.fromkeys(oauth["authorizedRedirectURLs"]))
    leftover = [s for s in mapping if len(s) >= 8 and s in json.dumps(doc)]
    if leftover:
        fail("the kickstart still contains a local secret; review render_kickstart")
    return json.dumps(doc, indent=2) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--dir", default=os.environ.get("MACRO_SELFHOST_DIR", "/etc/macro"))
    args = parser.parse_args()
    out = Path(args.dir)

    conf = configparser.ConfigParser()
    if not conf.read(out / "selfhost.conf"):
        fail(f"{out / 'selfhost.conf'} is missing; start from infra/selfhost/selfhost.conf.example")

    cargo_x("validate-local-env", "--no-doppler")
    kickstart_path = Path(cargo_x("write-kickstart").strip().splitlines()[-1])
    env = read_dotenv(GENERATED / "local.generated.env")

    state_path = out / "secrets.json"
    state = load_state(state_path)
    mapping = rotate(env, state)
    publish(env, conf, state)
    kickstart = render_kickstart(kickstart_path, mapping, env, conf)

    # Compose interpolates `$` in env files, which would silently change a value.
    dollar = sorted(k for k, v in env.items() if "$" in v)
    if dollar:
        fail(f"values must not contain '$': {', '.join(dollar)}")
    write_private(state_path, json.dumps(state, indent=2) + "\n")
    write_private(out / "selfhost.env", "".join(f"{dotenv_line(k, v)}\n" for k, v in sorted(env.items())))
    write_private(out / "kickstart" / "kickstart.json", kickstart, owner=FUSIONAUTH_UID)
    print(f"render: wrote {out / 'selfhost.env'} and {out / 'kickstart/kickstart.json'}")


if __name__ == "__main__":
    main()
