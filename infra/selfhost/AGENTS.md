# infra/selfhost

## What it is

Olive Trees fork: Macro on one server without AWS. Docker Compose for the
infrastructure and the services, with self-hosted replacements for S3, SQS,
DynamoDB, KMS and the scheduled Lambdas behind a single AWS endpoint. Design
and decisions: `docs/ota/plan-fase-0.md`.

## How it works

Services know one AWS endpoint, `LOCAL_AWS_URL=http://localstack:4566`. The
`aws-gateway` container answers on that name and routes by `X-Amz-Target`:
`AmazonSQS.*` to ElasticMQ, `DynamoDB_20120810.*` to DynamoDB Local,
`TrentService.*` to local-kms, everything else to s3proxy through the upload
relay. Every emulator persists to a named volume.

The services run from GHCR images built by the `selfhost-images` workflow
(`tooling/xtask/crates/xtask_workflows/src/workflows/selfhost_images.rs`), pinned
by `MACRO_VERSION`: one `macro-rust` image for every Rust binary, plus
`macro-web`, `macro-sync`, `macro-lexical` and `macro-websocket`. A one-shot
`migrate` service applies the MacroDB migrations before the Rust services start.
Scheduled Lambda handlers run unchanged against `lambda-clock`, which serves the
Lambda Runtime API with one EventBridge event per interval. `upload-relay` sits
in front of s3proxy and sends `doc-storage`'s ObjectCreated events, which
s3proxy cannot; on its public port it admits only presigned requests signed with
the services' AWS secret.

`proxy` (the `macro-web` image: Caddy with the bundle) is the only public
entrance, with the routes of the local stack's proxy. `mailrelay` takes plain
SMTP from the services and FusionAuth and delivers it to Zoho with the app
password, which only it receives (`mailrelay.env`).

## Start here

- `compose.yml`: the services, their hostnames and volumes.
- `aws-gateway/Caddyfile`: the AWS routing.
- `elasticmq/elasticmq.conf`: SQS queue URLs and persistence.
- `images/`: the image builds. `rust-binaries.txt` lists the Rust binaries;
  `build-rust.sh` builds them, `publish.sh` pushes an image.
- `lambda-clock/clock.py`, `upload-relay/relay.py`, `mailrelay/mailrelay.py`:
  the Lambda, S3 notification/signature and SMTP stand-ins, Python standard
  library only.
- `proxy/Caddyfile`: the public routes.
- `render.py`: turns the upstream-generated local env and FusionAuth kickstart
  into the server's configuration (`/etc/macro`), with fresh secrets.
- `provision.sh`, `backup.sh`, `restore.sh`: first start, back-ups and restore.

## Scope

- Owns: the self-hosted runtime (compose, gateway, emulator config, images,
  clock, relay).
- Not here: the local development stack → `tooling/xtask/crates/xtask_local`;
  AWS deployments → `infra/stacks/`.

## Recipes

- Install, upgrade, back up and restore: `docs/ota/self-hosting.md`.
- Kafka topics, queues, buckets, tables and KMS keys: `provision.sh`
  (idempotent; topics from `.github/kafka-cluster-topics.json`, the rest via
  `cargo x aws-provision` against the gateway on 127.0.0.1:4566).
- Back up: `backup.sh <dir>`; restore: `restore.sh <backup> [--config]`.

## Checks

- `MACRO_VERSION=<sha> docker compose -f infra/selfhost/compose.yml --env-file <env> config -q`
- After changing the gateway: `caddy validate --config infra/selfhost/aws-gateway/Caddyfile --adapter caddyfile`
- Clock, relays and routes: `python3 -m unittest` in `lambda-clock/`,
  `upload-relay/` (uses botocore when installed), `mailrelay/` and `proxy/`.
- After changing the public routes:
  `MACRO_DOMAIN=example.test ACME_EMAIL=a@example.test caddy validate --config infra/selfhost/proxy/Caddyfile --adapter caddyfile`
- After changing the workflow module: `cargo x workflows` and
  `cargo test -p xtask_workflows`.

## Gotchas

- Hostnames must match the env `cargo x validate-local-env` generates
  (`postgres`, `redis`, `search`, `kafka`, `fusionauth`, `localstack`); the
  self-hosted config overrides values, never hostnames.
- s3proxy runs without signature checks because the code rewrites presigned
  URLs to the public address; never give it a public route except the storage
  path the app uses.
- s3proxy supports neither bucket notifications nor bucket CORS; the gateway
  answers those provisioning calls itself. Upload events need the relay.
- ElasticMQ and DynamoDB Local run as root: their images cannot write to a
  fresh named volume otherwise.
- The local identity in `xtask_local/src/local/identity.rs` is public; a
  self-hosted server must replace every secret. `render.py` rotates the keys in
  `ROTATED_KEYS` by value (settings that shared a value keep sharing one) and
  fails when one of them is no longer generated; review that list when the
  generated env gains a secret.
- FusionAuth runs as uid 1001 and must read the kickstart, so `render.py`
  (run as root) gives it `kickstart/`. The kickstart only applies to an empty
  FusionAuth database, and the container reports healthy before it finishes;
  wait for the API key to answer 200 before calling it.
- A new Rust service: add its binary to `images/rust-binaries.txt` and a
  service with its compose name and hyphenated alias to `compose.yml`, copying
  any `environment:` overrides from `docker/docker-compose.yml`.
- A new scheduled Lambda: a service running its binary with
  `AWS_LAMBDA_RUNTIME_API` pointing at its own `lambda-clock` service.
- The relay sends events only for buckets in its `NOTIFY`; a new bucket
  notification in `xtask_local`'s `localstack.rs` needs an entry there.
- The images are amd64 only.
- A new public path in the local proxy (`proxy.rs` or an inventory
  `path_prefix`) fails `proxy/test_routes.py` until `proxy/Caddyfile` routes it.
- Never route the public side to s3proxy, the gateway or the relay's port 8080:
  only the relay's port 8081 checks signatures.
- Compose interpolation (`${VAR}`) prefers the shell's variables over
  `--env-file`; secrets that a shell may also define (AWS_*) go through
  `env_file`.
- The presigned check is verified against botocore's signer; if the Rust SDK
  starts signing extra headers that browsers do not send, uploads get 403
  with `reason` in the relay log.
- `backup.sh` stops the services with file-based state (s3, sync, kms,
  dynamodb, sqs) for the length of the archive copy. A new volume with such
  state belongs in both scripts' `file_volumes`.
- `docker compose exec` in a `while read` loop reads the loop's stdin; give it
  `< /dev/null`.
