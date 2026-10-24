# infra/selfhost

## What it is

Olive Trees fork: Macro on one server without AWS. Docker Compose for the
infrastructure, with self-hosted replacements for S3, SQS, DynamoDB and KMS
behind a single AWS endpoint. Design and decisions: `docs/ota/plan-fase-0.md`.

## How it works

Services know one AWS endpoint, `LOCAL_AWS_URL=http://localstack:4566`. The
`aws-gateway` container answers on that name and routes by `X-Amz-Target`:
`AmazonSQS.*` to ElasticMQ, `DynamoDB_20120810.*` to DynamoDB Local,
`TrentService.*` to local-kms, everything else to s3proxy. Every emulator
persists to a named volume.

## Start here

- `compose.yml`: the services, their hostnames and volumes.
- `aws-gateway/Caddyfile`: the AWS routing.
- `elasticmq/elasticmq.conf`: SQS queue URLs and persistence.
- `render.py`: turns the upstream-generated local env and FusionAuth kickstart
  into the server's configuration (`/etc/macro`), with fresh secrets.

## Scope

- Owns: the self-hosted runtime (compose, gateway, emulator config).
- Not here: the local development stack → `tooling/xtask/crates/xtask_local`;
  AWS deployments → `infra/stacks/`.

## Recipes

- Create or restore queues, buckets, tables and KMS keys:
  `cargo x aws-provision --url http://127.0.0.1:4566` against the gateway
  (the same provisioning as the local stack's LocalStack).

## Checks

- `docker compose -f infra/selfhost/compose.yml --env-file <env> config -q`
- After changing the gateway: `caddy validate --config infra/selfhost/aws-gateway/Caddyfile --adapter caddyfile`

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
