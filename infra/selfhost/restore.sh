#!/usr/bin/env bash
# Restore a backup made by backup.sh into this server's stack. Replaces the
# databases and the file-based volumes; the configuration archive is restored
# only with --config (on a new server, before the first start).
#
# Usage: restore.sh <backup-dir> [--config]   (as root)
set -euo pipefail

src="${1:?usage: restore.sh <backup-dir> [--config]}"
here="$(cd "$(dirname "$0")" && pwd)"
config_dir="${MACRO_SELFHOST_DIR:-/etc/macro}"
project="${COMPOSE_PROJECT_NAME:-macro-selfhost}"
file_volumes=(s3 sync_state kms dynamodb sqs)
tool_image=python:3.13-alpine3.22
log() { echo "restore: $*"; }

(cd "$src" && sha256sum --quiet -c SHA256SUMS)

if [[ "${2:-}" == "--config" ]]; then
  log "configuration into $config_dir"
  install -d -m 700 "$config_dir"
  tar -xzf "$src/config.tar.gz" -C "$config_dir"
fi
compose=(docker compose -p "$project" -f "$here/compose.yml" --env-file "$config_dir/selfhost.env")

log "stopping the stack"
"${compose[@]}" down
"${compose[@]}" up -d --wait postgres fusionauth-db

log "volumes"
for volume in "${file_volumes[@]}"; do
  docker volume create "${project}_${volume}" >/dev/null
  docker run --rm -v "${project}_${volume}:/volume" -v "$src:/backup:ro" "$tool_image" \
    sh -c "find /volume -mindepth 1 -delete && tar -xzf /backup/volume-$volume.tar.gz -C /volume"
done

log "databases"
"${compose[@]}" exec -T postgres sh -c 'dropdb -U macro --if-exists --force macrodb && createdb -U macro macrodb'
"${compose[@]}" exec -T postgres pg_restore -U macro -d macrodb --no-owner < "$src/macrodb.dump"
# On a new server FusionAuth has not created its role yet; it uses the same
# password as the superuser (compose passes FUSIONAUTH_DB_PASSWORD to both).
"${compose[@]}" exec -T fusionauth-db sh -c 'psql -q -U postgres -v ON_ERROR_STOP=1 -v pw="$POSTGRES_PASSWORD"' <<'SQL'
SELECT format('CREATE ROLE fusionauth LOGIN PASSWORD %L', :'pw')
WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'fusionauth') \gexec
SQL
"${compose[@]}" exec -T fusionauth-db sh -c 'dropdb -U postgres --if-exists --force fusionauth && createdb -U postgres -O fusionauth fusionauth'
"${compose[@]}" exec -T fusionauth-db pg_restore -U postgres -d fusionauth --no-owner --role=fusionauth < "$src/fusionauth.dump"

log "done; start the stack with: docker compose ... up -d"
