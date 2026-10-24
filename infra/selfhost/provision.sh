#!/usr/bin/env bash
# Create what the services expect before their first start, idempotently (run
# again after an upgrade): Garage's layout and the services' storage key, the
# Kafka topics of .github/kafka-cluster-topics.json
# (the list the deployed cluster uses, 6 partitions as locally), the queues,
# buckets, tables and KMS keys of the local stack, through the AWS gateway on
# 127.0.0.1:4566 (`cargo x aws-provision`), and the `selfhost-*` event queues
# that compose.yml names.
#
# Usage: provision.sh   (from a checkout, with the infrastructure running)
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
config_dir="${MACRO_SELFHOST_DIR:-/etc/macro}"
compose=(docker compose -p "${COMPOSE_PROJECT_NAME:-macro-selfhost}" -f "$here/compose.yml" --env-file "$config_dir/selfhost.env")

python3 -c 'import json, sys; print("\n".join(json.load(open(sys.argv[1]))))' \
  "$repo/.github/kafka-cluster-topics.json" |
  while read -r topic; do
    "${compose[@]}" exec -T kafka /opt/kafka/bin/kafka-topics.sh --bootstrap-server localhost:29092 \
      --create --if-not-exists --topic "$topic" --partitions 6 --replication-factor 1 < /dev/null > /dev/null
  done
echo "provision: kafka topics"

# Garage: a one-node layout on first start, and the services' key, which may
# create buckets (it owns those it creates). Its id and secret are hex.
garage() { "${compose[@]}" exec -T garage /garage "$@" < /dev/null; }
setting() { grep "^$1=" "$config_dir/selfhost.env" | cut -d= -f2-; }
if ! garage layout show | grep -q "Current cluster layout version: [1-9]"; then
  node="$(garage status | awk '/HEALTHY NODES/ {f = 1; next} f && $1 ~ /^[0-9a-f]{16}$/ {print $1; exit}')"
  garage layout assign -z dc1 -c 1T "$node" > /dev/null
  garage layout apply --version 1 > /dev/null
fi
export LOCAL_AWS_ACCESS_KEY_ID LOCAL_AWS_SECRET_ACCESS_KEY
LOCAL_AWS_ACCESS_KEY_ID="$(setting LOCAL_AWS_ACCESS_KEY_ID)"
LOCAL_AWS_SECRET_ACCESS_KEY="$(setting LOCAL_AWS_SECRET_ACCESS_KEY)"
if ! garage key info "$LOCAL_AWS_ACCESS_KEY_ID" > /dev/null 2>&1; then
  garage key import --yes -n services "$LOCAL_AWS_ACCESS_KEY_ID" "$LOCAL_AWS_SECRET_ACCESS_KEY" > /dev/null
fi
garage key allow --create-bucket "$LOCAL_AWS_ACCESS_KEY_ID" > /dev/null
echo "provision: garage"

# The local stack refuses real AWS credentials, so AWS_* goes; the bucket
# calls sign with LOCAL_AWS_* (macro_aws_config), as the services do.
(cd "$repo" && env $(env | grep -o '^AWS_[A-Z_]*' | sed 's/^/-u /') cargo x aws-provision --url http://127.0.0.1:4566)
echo "provision: queues, buckets, tables and keys"

# The self-hosted stack's own queues, which carry the upload relay's S3 events
# to the Lambda hosts (see compose.yml). CreateQueue is idempotent.
python3 - "$here/compose.yml" <<'PY'
import json, re, sys, urllib.request
names = sorted(set(re.findall(r"/000000000000/(selfhost-[a-z0-9-]+)", open(sys.argv[1]).read())))
for name in names:
    request = urllib.request.Request(
        "http://127.0.0.1:4566",
        data=json.dumps({"QueueName": name}).encode(),
        headers={"Content-Type": "application/x-amz-json-1.0", "X-Amz-Target": "AmazonSQS.CreateQueue"},
    )
    urllib.request.urlopen(request, timeout=10).read()
print(f"provision: {len(names)} self-hosted event queues")
PY
