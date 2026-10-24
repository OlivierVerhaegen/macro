#!/usr/bin/env bash
# Back up a self-hosted Macro server into <dir>/<timestamp>/ and keep the last
# KEEP (default 14) backups there. Copy that directory off the server (it holds
# every secret of the server, so treat it like the server itself).
#
# Contents: pg_dump of MacroDB and FusionAuth (live), archives of the volumes
# with file-based state (taken with their containers briefly stopped: Garage,
# the sync worker, KMS, DynamoDB, the queues), and the
# configuration directory. Not included, because they are rebuilt: the search
# index, Redis, Kafka and the TLS certificates.
#
# Usage: backup.sh <dir>   (as root, next to the running stack)
set -euo pipefail

dest_root="${1:?usage: backup.sh <dir>}"
here="$(cd "$(dirname "$0")" && pwd)"
config_dir="${MACRO_SELFHOST_DIR:-/etc/macro}"
project="${COMPOSE_PROJECT_NAME:-macro-selfhost}"
keep="${KEEP:-14}"
compose=(docker compose -p "$project" -f "$here/compose.yml" --env-file "$config_dir/selfhost.env")
# Volumes whose services write files: archived while these services are stopped.
file_volumes=(garage-meta garage-data sync_state kms dynamodb sqs)
file_services=(garage sync_service kms dynamodb sqs)
# The image is already on the server for the clock and relays.
tool_image=python:3.13-alpine3.22

dest="$dest_root/$(date -u +%Y%m%dT%H%M%SZ)"
umask 077
mkdir -p "$dest"
log() { echo "backup: $*"; }

log "databases"
"${compose[@]}" exec -T postgres pg_dump -U macro -d macrodb -Fc > "$dest/macrodb.dump"
"${compose[@]}" exec -T fusionauth-db pg_dump -U postgres -d fusionauth -Fc > "$dest/fusionauth.dump"

log "volumes (stopping ${file_services[*]})"
"${compose[@]}" stop "${file_services[@]}" >/dev/null
restart() { "${compose[@]}" start "${file_services[@]}" >/dev/null; }
trap restart EXIT
for volume in "${file_volumes[@]}"; do
  docker run --rm -v "${project}_${volume}:/volume:ro" -v "$dest:/backup" "$tool_image" \
    tar -czf "/backup/volume-$volume.tar.gz" -C /volume .
done
restart
trap - EXIT

log "configuration"
tar -czf "$dest/config.tar.gz" -C "$config_dir" .

# Integrity first: a dump that cannot be listed is not a backup.
"${compose[@]}" exec -T postgres pg_restore --list < "$dest/macrodb.dump" > /dev/null
(cd "$dest" && sha256sum -- * > SHA256SUMS)

ls -1d "$dest_root"/*/ 2>/dev/null | sort | head -n "-$keep" | xargs -r rm -rf
log "done: $dest ($(du -sh "$dest" | cut -f1))"
