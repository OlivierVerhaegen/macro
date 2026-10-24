# Self-hosting Macro on one server

A guide for the Olive Trees fork: Macro on one VPS, without AWS. The design and
its choices are in [plan-phase-0.md](plan-phase-0.md); the technical details per
file in [`infra/selfhost/AGENTS.md`](../../infra/selfhost/AGENTS.md).

## What you need

- An amd64 server (OVH VPS) with at least 24 GB of RAM and 100 GB of disk,
  Debian or Ubuntu, with Docker Engine and the compose plugin.
- A DNS record `macro.olivetrees-agency.be` pointing at the server's IP; ports
  80 and 443 open, everything else closed.
- On the server: `git`, `python3`, `openssl`, `ssh-keygen` and a Rust toolchain
  (`rustup`). The render and provisioning steps use `cargo x` from the
  repository.
- A Zoho app password for SMTP (Zoho Mail → Settings → Security → App
  passwords). Enter it only on the server, in `selfhost.conf`.
- Access to the images: make the GHCR packages `macro-rust`, `macro-web`,
  `macro-sync`, `macro-lexical`, `macro-websocket`, `macro-convert`,
  `macro-ai-editing` and `macro-analytics` public (GitHub → Packages
  → Package settings), or log in on the server with `docker login ghcr.io` and
  a token with `read:packages`.

## Images

The **Self-hosted images** workflow builds every image on each push to `main`
or `ota/**` and tags them with the commit and the branch. Pick a commit whose
run is green; that is `MACRO_VERSION`. Check out the same commit on the server,
so the compose file, the scripts and the images belong together.

## First installation

Everything as root, from a checkout of the repository (for example
`/opt/macro`).

```sh
git clone https://github.com/OlivierVerhaegen/macro /opt/macro
cd /opt/macro && git checkout <MACRO_VERSION>

install -d -m 700 /etc/macro
install -m 600 infra/selfhost/selfhost.conf.example /etc/macro/selfhost.conf
# Fill in: domain, signup_allowlist (your address), sender, the app password.
$EDITOR /etc/macro/selfhost.conf

# Writes /etc/macro/selfhost.env, mailrelay.env, kickstart/ and secrets.json
# with new secrets for this server.
python3 infra/selfhost/render.py

export MACRO_VERSION=<commit>
compose="docker compose -f infra/selfhost/compose.yml --env-file /etc/macro/selfhost.env"

# The infrastructure first, then topics, queues, buckets, tables and keys.
$compose up -d --wait postgres redis kafka search fusionauth garage aws-gateway
infra/selfhost/provision.sh

# Then everything: the migrations run first, the services wait for them.
$compose up -d
```

Open `https://macro.olivetrees-agency.be` and log in with an address from
`signup_allowlist`; the code arrives by mail through Zoho.

`secrets.json` holds every secret of the server. Rendering again (after a change
to `selfhost.conf` or an upgrade) keeps them, so do not lose that file: every
backup contains it.

## Upgrading

```sh
cd /opt/macro && git fetch && git checkout <new commit>
python3 infra/selfhost/render.py
infra/selfhost/provision.sh
MACRO_VERSION=<new commit> docker compose -f infra/selfhost/compose.yml \
  --env-file /etc/macro/selfhost.env up -d
```

`up -d` runs `migrate` again; the services restart only after it. Make a backup
first.

## Backups

`infra/selfhost/backup.sh <dir>` writes one directory per backup: dumps of
MacroDB and FusionAuth, archives of the volumes with files (Garage's objects
and metadata, the live state of shared documents, KMS keys, DynamoDB, queues)
and `/etc/macro`. The
services owning those volumes stop for a few seconds meanwhile. The script keeps
the last 14 (`KEEP`).

Daily, with a copy off the server (for example OVH Object Storage through
`rclone`):

```cron
15 3 * * * cd /opt/macro && infra/selfhost/backup.sh /var/backups/macro && rclone sync /var/backups/macro remote:macro-backups
```

A backup contains every secret of the server; encrypt the copy off the server
(`rclone crypt`).

Not in the backup, because they are rebuilt: the search index, Redis, Kafka and
the TLS certificates.

## Restoring

On the same server:

```sh
infra/selfhost/restore.sh /var/backups/macro/<timestamp>
MACRO_VERSION=<commit> docker compose -f infra/selfhost/compose.yml \
  --env-file /etc/macro/selfhost.env up -d
```

On a new server: Docker, a checkout of the same commit, then
`restore.sh <dir> --config` (which also restores `/etc/macro`) and the same
`up -d`. Then `provision.sh` for the Kafka topics.

## What is public

Only Caddy (`proxy`) on 80 and 443. Behind it: the app under `/app`, the
services under their paths, and storage under `/local-storage` and
`/static-file`, where the upload relay refuses every request without a valid
presigned signature (reads of public static files excepted). FusionAuth, the
databases, the AWS replacements, the Lambda handlers and the mail relay have no
public route. The AWS gateway listens on `127.0.0.1:4566`
for `provision.sh`.

## Current limits

- amd64 only; a Raspberry Pi needs arm64 images.
- The AI editing worker needs `anthropic_api_key`, `cerebras_api_key` and
  `openai_api_key` in `selfhost.conf [env]`; without them it keeps restarting
  and agent editing fails. The analytics proxy sends nothing anywhere: the
  bundle has no PostHog key, and the OTLP targets have no collector.
- Two Lambda handlers do not run because nothing on one server triggers them:
  data loss prevention (reads Macro's S3 access logs) and email suppression
  (SES bounces). Notification retention runs in dry-run mode, as in
  production: it only logs what it would delete.
- `agent_harness_service` gets the host's Docker socket, as locally.
- Signing in with Google or Microsoft needs a public FusionAuth route; for now
  only signing in with an emailed code works.
