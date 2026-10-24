# Phase 0 plan: Macro on our own VPS

Status: agreed, in progress. Branch: `ota/fase-0-vps` (from `main`).

Goal: Macro runs in production on one VPS, without AWS, with persistent data,
real email and HTTPS on our own domain. Phase 1 (Zoho Mail) does not need
phase 0 to be developed: that happens on the local stack. Phase 0 must be done
before Macro is really used.

## 1. Starting point: the local stack

`docs/development/running-locally.md` describes how Macro runs locally.
`just stack up --no-doppler` starts everything in Docker without an AWS account
and without Doppler: Postgres, Redis, LocalStack (S3, SQS, DynamoDB, KMS),
OpenSearch, Kafka, FusionAuth, all Rust services, the sync worker and a Caddy
proxy that serves the app under one address (`<proxy>/app/`).

That is almost a self-hosted setup. What keeps it from being production:

| Local part | Problem on a VPS | Phase 0 solution |
| --- | --- | --- |
| LocalStack (S3, SQS, DynamoDB, KMS) | A test tool. Resources and KMS keys disappear on a restart, and whatever a lost KMS key encrypted is unreadable (see `tooling/xtask/crates/xtask_local/src/local/localstack.rs`). | API-compatible replacements with persistent volumes behind one endpoint (section 5.2). Only endpoints change in the configuration. |
| Kafka in a test configuration | One broker without tuned retention. | One Kafka node (KRaft) with a persistent volume and retention settings; Redpanda as an option to save memory. |
| Mailpit catches all mail | Login codes and invitations reach nobody. | FusionAuth and the services send through real SMTP (a Zoho sender address). |
| Stubbed integrations | Google, Stripe and CloudFront do not work. | Leave Stripe and CloudFront off; Google only when needed. |
| Fixed test secrets | Everybody knows them. | Generate every secret anew, in one env file outside the repository with mode 600. |
| Development CA and `localhost` | No valid certificate. | Caddy with Let's Encrypt on `macro.olivetrees-agency.be`. |
| Passwordless login "not safe to publish" | The guide warns about this explicitly. | Find out why (open signup, FusionAuth kickstart with test keys) and close it: our own FusionAuth API keys, signup only for our own addresses. |
| Lambda handlers | Run in AWS. | Run the same binaries as containers next to the services (section 5.2). |
| `stack down` wipes everything | Dangerous in production. | Our own compose file and scripts, separate from `just stack`. |
| No backups | — | Daily database dumps and archives of the file-based state, copied off the server, with a tested restore. |
| The sync worker's Durable Objects/KV/R2/D1 | Emulated locally by `wrangler` in the `sync_state` volume. | Find out whether that emulation is stable enough for production, or whether the sync worker should run on Cloudflare itself. This is the biggest unknown. |

## 2. Server

Decided: domain `macro.olivetrees-agency.be`; for now only Olivier logs in.
Candidates: an OVH VPS or our own Raspberry Pi.

| | OVH VPS | Raspberry Pi 5 |
| --- | --- | --- |
| Architecture | x86_64 | arm64; the stack already builds for `aarch64-unknown-linux-gnu`, and Postgres, OpenSearch, Redpanda and FusionAuth have arm64 images. To verify: every other image and the sync worker. |
| Memory | Choose ≥ 24 GB | At most 16 GB. Three JVMs (OpenSearch, FusionAuth, Kafka) plus ~30 Rust services is tight; probably only feasible with Redpanda instead of Kafka and a small OpenSearch heap. |
| Storage | SSD/NVMe | An NVMe or USB SSD is required; an SD card does not survive the write load of Postgres and Kafka. |
| Reachability | Fixed IP, ports 80/443 | Home IP: through Cloudflare Tunnel (`xtask_local` already supports tunnels, `cf_tunnel.rs`), without opening ports. |
| Building | In CI | Not on the Pi itself (Rust builds take hours): cross-compile to arm64 elsewhere. |
| Availability | Data center, redundant power and network | Home power and internet; an outage means no mail sync until it is back. |
| Location (GDPR) | EU data center (Gravelines, Strasbourg, Frankfurt) | At home |

Proposal: **an OVH VPS for production**. The Pi suits a test environment or a
backup destination, but with 16 GB and home internet it is the weak link for a
system that holds mail and documents. The first measurement (step 1 of
section 3) confirms whether 16 GB is enough at all; if it is, the Pi stays an
option.

In general: Docker and Compose, no Kubernetes; only 80 and 443 open (or nothing
at all with Cloudflare Tunnel), SSH with a key.

## 3. Approach

1. Measure: run the local stack for a day and note memory and CPU per container.
2. Inventory: every AWS, Doppler and Cloudflare dependency per service, from
   `local_env.rs`, the compose generator and `infra/stacks/`.
3. The compose file `infra/selfhost/compose.yml` with the infrastructure, the
   services and Caddy with Let's Encrypt.
4. Decide on the sync worker (self-hosted or Cloudflare).
5. Sending email and securing login.
6. Backups and a restore test.
7. `docs/ota/self-hosting.md` with installation, upgrade and restore.

Change upstream code only where an endpoint or key source is not configurable;
the rest lives in `infra/selfhost/` and `docs/ota/`.

## 4. Decided

- Domain: `macro.olivetrees-agency.be`.
- Users: only Olivier. Customer access comes in phase 5.
- Server: an OVH VPS or a Raspberry Pi; chosen after the measurement (section 2).

## 5. Design after the inventory (24 October 2026)

### 5.1 What the local stack runs

`docker/docker-compose.yml` (with `docker/docker-compose-databases.yml` and
`infra/stacks/fusionauth-instance/docker-compose.yml`) plus the override that
`cargo x` generates (`tooling/xtask/crates/xtask_local`) runs:

- Rust services: `authentication-service`, `connection_gateway`,
  `contacts_service`, `document_storage_service`, `document_upload_finalizer`
  (a local stand-in for a Lambda), `email_service`, `email_pubsub_workers`,
  `calendar_service`, `notification_service`, `static_file_service`,
  `unfurl_service`, `image_proxy_service`, `agent_harness_service`,
  `slack_import_worker`, and the opt-in `search_processing_service`.
- Not Rust: `sync_service` (a Cloudflare Worker through `wrangler dev`, state in
  the `sync_state` volume), `websocket_service`, `lexical_service`,
  `static_file_cdn` (nginx), `ai_editing_worker`, `analytics_proxy`.
- Infrastructure: Postgres, Redis, OpenSearch, FusionAuth, Kafka, LocalStack
  (`sqs,dynamodb,s3,kms`), Mailpit and Caddy.
- The configuration (~900 lines) comes from `local_env.rs`: queue, bucket and
  table names in `resources.rs`, the FusionAuth kickstart in `kickstart.rs`.

### 5.2 AWS dependencies and replacements

| AWS service | Used by | Replacement | Why |
| --- | --- | --- | --- |
| Endpoint | every service through `macro_aws_config` | One internal gateway (Caddy) at the address of `LOCAL_AWS_URL` | The code knows one endpoint for every service. The gateway routes on `X-Amz-Target` (`AmazonSQS.*`, `DynamoDB_*`, `TrentService.*`); the rest is S3. No code change. |
| S3 | documents, static files, attachments, uploads | Garage (one node) behind the upload relay | Keys are flat, as on S3: Macro stores `file/<id>` next to `file/<id>/size=64`, which a filesystem-backed store (s3proxy) and MinIO refuse. Garage checks every signature; the relay checks presigned URLs on the public side and forwards them with the internal host they were signed for (section 5.6). |
| S3 → SQS and EventBridge notifications | the upload finalizer, text extraction and search (documents), Word and bulk uploads, call recordings, static file metadata | `infra/selfhost/upload-relay/relay.py` in front of Garage, for all S3 traffic: after a successful write an S3 event to each matching queue (`NOTIFY`, with prefix and suffix filters) | Garage has no notifications. Handlers that production triggers through EventBridge get the EventBridge shape from the Lambda host. The event leaves before the client gets its answer. |
| SQS | almost every worker | ElasticMQ with persistent queues (`elasticmq.conf`) | API-compatible, including delay seconds (sending schedules with up to 900 s of delay). |
| DynamoDB | connection_gateway, document_storage, static_file, agent_harness, search | DynamoDB Local with `-sharedDb -dbPath` on a volume | AWS's official image, persistent. |
| KMS | authentication_service (Microsoft and Cursor keys), agent_harness | local-kms (`nsmithuk/local-kms`) with its data on a volume | Keys must survive restarts, or whatever they encrypted is unreadable. Zoho already uses the local key source from phase 1. |
| SES | authentication_service, notification_service | SMTP through `mailrelay` to Zoho | `ses_client` sends over SMTP when `SMTP_HOST` is set. |
| Secrets Manager | almost every service | Not needed | Locally `MacroConfig` reads everything from the environment. |
| Lambda | every handler with a trigger on one server (section 5.7) | `infra/selfhost/lambda-host/host.py` around each binary: it serves the Lambda Runtime API and turns the production triggers into invocations (schedules, SQS, S3 events directly or as EventBridge events, HTTP as a Function URL, the Invoke API) | The binaries use `lambda_runtime` and run unchanged. Our own host instead of the Runtime Interface Emulator: no extra binary, every trigger type in one place, and a slow scheduled run skips ticks like Lambda with a concurrency of one. |
| EventBridge → SQS | reminder sweeps (every minute), the nightly CRM cleanup | `infra/selfhost/scheduler/scheduler.py` | Rules whose target is a queue, with the same input. |
| ECS tasks and services | `sha_cleanup_worker` (hourly, through `worker_trigger`), `convert_service` (LibreOfficeKit) | A loop container; the convert service's own image | `worker_trigger` only starts the ECS task, so the loop replaces it. |

Important for phase 1: "send now" puts a message on the SQS queue directly,
with a delay; the scheduled handler is the safety net for failures and for
messages scheduled more than 15 minutes ahead. Without it such messages stay
put.

### 5.3 Configuration: generate upstream's, override for production

`cargo x validate-local-env --no-doppler --instance selfhost` writes the full
env (185 keys) to `infra/local/generated/selfhost/local.generated.env`. The
self-hosted configuration is what upstream generates plus the production
values that `infra/selfhost/render.py` applies (domain, mail, secrets). There
is no copy of `local_env.rs` that can drift.

**Security (blocking):** the local identity is fixed in the code
(`xtask_local/src/local/identity.rs`): the JWT signing key, the FusionAuth API
key, the client secret, the internal auth key and the key pair for API tokens.
Other secrets derive from the instance name. Anyone who knows the code can
forge login tokens with them. The render step therefore replaces every secret
with a new random value, the same way in the env and in the FusionAuth
kickstart, and keeps them in the secrets file on the server. Caddy does not
expose FusionAuth at all.

### 5.4 Other choices

- **Images:** the `selfhost-images` workflow (generated by `cargo x workflows`,
  runs only in the fork) builds on GitHub runners and pushes to GHCR, tagged
  with commit and branch: one `macro-rust` image with every Rust binary
  (release, in `rust:1.94.0-trixie`, without local features), `macro-web` (the
  bundle) and `macro-sync`, `macro-lexical`, `macro-websocket` and
  `macro-convert` from upstream's Dockerfiles. The VPS only pulls images. amd64 only: a Pi (arm64)
  needs a second build.
- **Kafka:** the same broker as locally (`apache/kafka`, KRaft, one node), so
  topics and client settings stay identical. Redpanda can save memory later.
- **Sync worker:** `wrangler dev` (workerd) with its state on a volume, as
  locally. Acceptable for one user; the risk is that `wrangler dev` is not a
  production runtime. Cloudflare itself remains the alternative.
- **Configuration:** the compose file and `selfhost.conf.example` in
  `infra/selfhost/`; secrets in `/etc/macro/` (mode 600), never in the
  repository.
- **Access:** one public Caddy with Let's Encrypt for
  `macro.olivetrees-agency.be`; internal services (gateway, Postgres,
  OpenSearch) without a public port.

### 5.5 Build order

Status: 1 to 5 are in `infra/selfhost/` and `docs/ota/self-hosting.md`, tested
locally but not yet on the VPS; 6 follows. All Lambda handlers that can be
triggered on one server run (section 5.7).

1. `infra/selfhost/compose.yml` with the infrastructure (Postgres, Redis,
   OpenSearch, Kafka, FusionAuth, Garage, ElasticMQ, DynamoDB Local,
   local-kms, the AWS gateway) and a provisioning step for queues, buckets,
   tables and KMS keys (taken from `localstack.rs` and `resources.rs`).
2. The Rust and non-Rust services, with the env from 5.3.
3. The Lambda handlers through the Lambda host; the scheduler; the upload
   relay; migrations (sqlx-cli).
4. The public Caddy, email (SMTP) and closing signups.
5. Backups and restore; `docs/ota/self-hosting.md`.
6. Measure on the real VPS (step 1 of section 3) and settle the server size.

### 5.6 Findings while building

- **`ENVIRONMENT=local` stays.** Production mode hard-codes `macro.com`
  (redirects, cookie domain, CORS); local follows `FRONTEND_ORIGIN` and sets
  host-only cookies, which is right for one domain. What else local mode does,
  and what phase 0 had to close:
  - signups were open to anyone (`SignupPolicy::allow_all`). Fixed with a small
    upstream change: under Local, `DEVELOPMENT_SIGNUP_ALLOWLIST_JSON` applies
    when it is set; `render.py` requires `[server] signup_allowlist`. Macro
    always admits `@macro.com` addresses;
  - webhooks may target `http://` addresses, internal ones too; only signed-in
    users create them;
  - cookies have `SameSite=None` (but `Secure` and `HttpOnly`).
- **The login code in the response** depends on the compile feature
  `return_passwordless_code`, not on `ENVIRONMENT`. The image builds without
  that feature and without `no_rate_limit`.
- **The web bundle** is the local-backend bundle of `cargo x stack up`
  (`VITE_LOCAL_BACKEND_ORIGIN=same-origin`): every service call goes to our own
  domain, without a code change. `import.meta.env.DEV` is on in it; among other
  things that enables automatic login with `?email=`, which only works when the
  backend returns the code, which it does not here.
- **`ai_editing_worker` and `analytics_proxy`** are Cloudflare Workers that the
  local stack runs from a mounted checkout; here they get images of their own
  (`macro-ai-editing`, `macro-analytics`) running `wrangler dev`. The AI editing
  worker refuses to start without the Anthropic, Cerebras and OpenAI keys
  (`selfhost.conf [env]`); its D1 trace store is the `ai_editing_state` volume.
  The bundle is built with `VITE_AI_EDITING_WORKER_URL=/ai-editing`, otherwise
  it calls Macro's hosted worker. The analytics proxy only forwards what the
  bundle sends, which is nothing without a PostHog key or a browser OTLP
  endpoint.
- **Backups:** besides Postgres and the S3 data, the `sync_state` volume holds
  the live state of shared documents.
- **`agent_harness_service`** gets the host's Docker socket, as locally, to
  start sandboxes. That is root on the host for whoever takes over that service.
- **GHCR:** new packages are private. Make them public or log in on the server
  with a token that has `read:packages`.
- **Storage was public, twice over.** s3proxy checks no signatures, so a public
  `/local-storage` route gave anyone access to every bucket. And with
  `LOCAL_AWS_URL` every service signed with the SDK's fixed test credentials
  (`Credentials::for_tests`), which anyone can look up, so even a checked
  signature proved nothing. Fixed with a small upstream change: `macro_aws_config`
  signs with `LOCAL_AWS_ACCESS_KEY_ID`/`LOCAL_AWS_SECRET_ACCESS_KEY` when both
  are set (`render.py` generates them per server). Garage checks every request;
  the relay checks presigned URLs before they reach it, and signs reads of the
  static-file bucket itself, like CloudFront for its S3 origin. Tested with
  presigned URLs from the Rust SDK, rewritten to the public address.
- **Flat keys:** s3proxy's filesystem backend stores `/` as directories, so the
  image optimizer's `file/<id>/size=64` failed next to `file/<id>`; MinIO refuses
  the same. Garage stores keys flat.
- **Email:** `mailrelay` (our own, Python standard library) instead of a Postfix
  image: no queue, a failure at Zoho goes back to the sending service. The app
  password is in an env file of its own.

### 5.7 Lambda handlers and their triggers

From `infra/stacks/` (24 October 2026). The host's sources reproduce the
trigger; timeouts follow production where it sets one.

| Handler | Production trigger | Self-hosted source |
| --- | --- | --- |
| `email_scheduled_handler` | rate(1 minute) | schedule |
| `deleted_item_poller` | rate(4 hours) | schedule |
| `notification_retention_handler` | rate(5 minutes), `{"dry_run": true}` (production keeps it in dry run) | schedule with the same input |
| `organization_retention_trigger` / `_handler` | rate(1 day) / its SQS queue | schedule / SQS |
| `user_link_cleanup_handler` | rate(8 hours) | schedule |
| `email_sfs_delete_handler` | cron(0 8 * * ? *) | schedule at 08:00 UTC |
| `email_refresh_handler` | rate(1 hour) | schedule (finds nothing without Gmail inboxes) |
| `delete_chat_handler` | its SQS queue | SQS |
| `docx_unzip_handler` | S3 ObjectCreated on `docx-upload` | S3 event |
| `document_text_extractor` | EventBridge Object Created on the document bucket, and its SQS queue | EventBridge event, SQS |
| `search_upload_handler` | EventBridge Object Created on the document bucket | EventBridge event |
| `image_optimizer` | Function URL behind CloudFront's origin group, self-invoked for resizes | HTTP behind the public Caddy, Invoke API |
| `upload_extractor_lambda_trigger` / `_handler` | EventBridge Object Created on the bulk upload bucket / its SQS queue | EventBridge event / SQS |
| `call_recording_preview_handler` | S3 ObjectCreated on `calls/*.mp4` | S3 event (ffmpeg in the image) |
| `dataloss_prevention_handler` | S3 access logs of Macro's document bucket, alerts through SNS | Not run: nothing here produces its trigger |
| `email_suppression_handler` | SNS from SES bounces for `macro.com` | Not run: no SES |
| `worker_trigger` | starts `sha_cleanup_worker` as an ECS task hourly | Replaced by a loop around `sha_cleanup_worker` |

