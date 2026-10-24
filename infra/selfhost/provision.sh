#!/usr/bin/env bash
# Create what the services expect before their first start, idempotently (run
# again after an upgrade): the Kafka topics of .github/kafka-cluster-topics.json
# (the list the deployed cluster uses, 6 partitions as locally) and the queues,
# buckets, tables and KMS keys of the local stack, through the AWS gateway on
# 127.0.0.1:4566 (`cargo x aws-provision`).
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

# The local stack refuses real AWS credentials; none are needed here.
(cd "$repo" && env $(env | grep -o '^AWS_[A-Z_]*' | sed 's/^/-u /') cargo x aws-provision --url http://127.0.0.1:4566)
echo "provision: queues, buckets, tables and keys"
