# infra/selfhost

## What it is

Olive Trees fork: Macro on one server without AWS. Docker Compose for the
infrastructure, the services and the Lambda handlers, with self-hosted
replacements for S3, SQS, DynamoDB, KMS, EventBridge and the Lambda runtime
behind a single AWS endpoint. Design and decisions: `docs/ota/plan-phase-0.md`;
operating it: `docs/ota/self-hosting.md`.

## How it works

Services know one AWS endpoint, `LOCAL_AWS_URL=http://localstack:4566`. The
`aws-gateway` container answers on that name and routes by `X-Amz-Target`:
`AmazonSQS.*` to ElasticMQ, `DynamoDB_20120810.*` to DynamoDB Local,
`TrentService.*` to local-kms; Lambda Invoke paths to `<function>:9001`;
everything else is S3, through the upload relay to Garage. Every store persists
to a named volume.

Storage is signed end to end. The services sign with the server's own key
(`LOCAL_AWS_ACCESS_KEY_ID`/`LOCAL_AWS_SECRET_ACCESS_KEY`, read by
`macro_aws_config`; without them the SDK's public test key would be used), and
Garage checks every request. `upload-relay` sends the bucket notifications
Garage cannot, and on its public port checks presigned URLs before forwarding
them with the internal host they were signed for; unsigned reads of the
static-file bucket it signs itself.

Lambda handlers run unchanged under `lambda-host/host.py`, which serves them the
Lambda Runtime API and turns their production triggers into invocations:
schedules, SQS queues, the relay's S3 events (as S3 or EventBridge events) and
HTTP. `scheduler` sends the messages of the EventBridge rules whose target is a
queue.

The services run from GHCR images built by the `selfhost-images` workflow
(`tooling/xtask/crates/xtask_workflows/src/workflows/selfhost_images.rs`), pinned
by `MACRO_VERSION`: `macro-rust` holds every Rust binary and the Lambda host;
`macro-web`, `macro-sync`, `macro-lexical`, `macro-websocket`,
`macro-convert`, `macro-ai-editing` and `macro-analytics` the rest. The two
Cloudflare Workers the local stack runs from a mounted checkout (AI editing,
analytics) have their own Dockerfiles in `images/` running `wrangler dev`. A one-shot `migrate` service applies the MacroDB
migrations before the Rust services start.

`proxy` (the `macro-web` image: Caddy with the bundle) is the only public
entrance, with the routes of the local stack's proxy and CloudFront's image
variant behaviour. `mailrelay` takes plain SMTP from the services and FusionAuth
and delivers it to Zoho with the app password, which only it receives
(`mailrelay.env`).

## Start here

- `compose.yml`: the services, their hostnames and volumes.
- `aws-gateway/Caddyfile`: the AWS routing.
- `garage/garage.toml`, `elasticmq/elasticmq.conf`: the S3 store and SQS.
- `images/`: the image builds. `rust-binaries.txt` lists the Rust binaries;
  `build-rust.sh` builds them, `publish.sh` pushes an image.
- `lambda-host/host.py`, `scheduler/scheduler.py`, `upload-relay/relay.py`,
  `mailrelay/mailrelay.py`: the Lambda runtime, EventBridge-to-queue schedules,
  the S3 front and the SMTP relay; Python standard library only.
- `proxy/Caddyfile`: the public routes.
- `render.py`: turns the upstream-generated local env and FusionAuth kickstart
  into the server's configuration (`/etc/macro`), with fresh secrets.
- `provision.sh`, `backup.sh`, `restore.sh`: first start, backups and restore.

## Scope

- Owns: the self-hosted runtime (compose, gateway, stores' configuration,
  images, Lambda host, scheduler, relays, public proxy).
- Not here: the local development stack → `tooling/xtask/crates/xtask_local`;
  AWS deployments → `infra/stacks/`.

## Recipes

- Install, upgrade, back up and restore: `docs/ota/self-hosting.md`.
- Garage's layout and key, Kafka topics, queues, buckets, tables and KMS keys:
  `provision.sh` (idempotent).
- A new Rust service: add its binary to `images/rust-binaries.txt` and a
  service with its compose name and hyphenated alias to `compose.yml`, copying
  any `environment:` overrides from `docker/docker-compose.yml`.
- A new Lambda handler: its binary in `images/rust-binaries.txt`, a service with
  `<<: *lambda`, its function name as service name and alias, and `SOURCES`
  matching its production triggers in `infra/stacks/`. An S3 trigger needs a
  `NOTIFY` entry on the relay and a `selfhost-*` queue named in the compose
  file (provision.sh creates those).
- A new EventBridge rule with a queue target: an entry in the scheduler's
  `SCHEDULES`.

## Checks

- `MACRO_VERSION=<sha> docker compose -f infra/selfhost/compose.yml --env-file <env> config -q`
- After changing the gateway: `caddy validate --config infra/selfhost/aws-gateway/Caddyfile --adapter caddyfile`
- After changing the public routes:
  `MACRO_DOMAIN=example.test ACME_EMAIL=a@example.test caddy validate --config infra/selfhost/proxy/Caddyfile --adapter caddyfile`
- Python: `python3 -m unittest` in `lambda-host/`, `scheduler/`,
  `upload-relay/` (uses botocore when installed), `mailrelay/` and `proxy/`.
- After changing the workflow module: `cargo x workflows` and
  `cargo test -p xtask_workflows`.

## Gotchas

- Hostnames must match the env `cargo x validate-local-env` generates
  (`postgres`, `redis`, `search`, `kafka`, `fusionauth`, `localstack`); the
  self-hosted config overrides values, never hostnames.
- Garage needs its own key format (`GK` + 24 hex digits, a 64-digit secret);
  `render.py` generates it. Buckets created through S3 get a local alias of
  that key, so only that key sees them; everything signs with it.
- Never route the public side to Garage, the gateway or the relay's port 8080:
  only the relay's port 8081 checks presigned URLs.
- Garage has no bucket notifications and the gateway answers those
  provisioning calls itself; the relay sends events only for its `NOTIFY`
  entries. A notification added in `xtask_local`'s `localstack.rs` or in
  `infra/stacks/` needs an entry there.
- Garage binds IPv4 (`0.0.0.0`): the Docker network has no IPv6 and `[::]`
  panics at startup.
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
- Lambda event shapes are AWS's, not the SQS API's: the host converts message
  attributes to camelCase and S3 notifications to EventBridge events. A handler
  that reads another field shape needs the host to produce it.
- `dataloss_prevention_handler` and `email_suppression_handler` do not run:
  nothing here produces their triggers (Macro's S3 access logs, SES bounces).
- A handler that invokes a Lambda that does not run here (the websocket
  response Lambda of docx and convert failures) gets a 502 from the gateway and
  logs it.
- The images are amd64 only.
- A new public path in the local proxy (`proxy.rs` or an inventory
  `path_prefix`) fails `proxy/test_routes.py` until `proxy/Caddyfile` routes it.
- Compose interpolation (`${VAR}`) prefers the shell's variables over
  `--env-file`; secrets that a shell may also define go through `env_file`.
- `backup.sh` stops the services with file-based state (Garage, sync, AI
  editing, kms, dynamodb, sqs) for the length of the archive copy. A new volume with such
  state belongs in both scripts' `file_volumes`.
- `docker compose exec` in a `while read` loop reads the loop's stdin; give it
  `< /dev/null`. `pgrep -f`/`pkill -f` match the shell running them when its
  command line contains the pattern.
