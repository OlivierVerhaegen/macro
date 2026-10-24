#!/usr/bin/env bash
# Build the self-hosted Rust binaries in release mode and stage the image
# context: <out>/bin/<binary> for each line of rust-binaries.txt, plus pdfium,
# sqlx-cli, the MacroDB migrations and the Lambda host.
#
# Runs inside rust:1.94.0-trixie (the CI job does that), so the binaries link
# against the same Debian as the runtime image. No local-only features:
# authentication_service is built without `return_passwordless_code` or
# `no_rate_limit`.
#
# Usage: build-rust.sh <out-dir>
set -euo pipefail

out="${1:?usage: build-rust.sh <out-dir>}"
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
mapfile -t binaries < <(grep -vE '^\s*(#|$)' "$here/rust-binaries.txt")

cd "$repo"
args=()
for binary in "${binaries[@]}"; do args+=(--bin "$binary"); done
SQLX_OFFLINE=true cargo build --release --locked "${args[@]}"

rm -rf "$out"
mkdir -p "$out/bin" "$out/pdfium-lib/linux"
for binary in "${binaries[@]}"; do
  install -m 755 "target/release/$binary" "$out/bin/$binary"
  strip "$out/bin/$binary"
done
cp services/search_processing_service/pdfium-lib/linux/libpdfium.so "$out/pdfium-lib/linux/"

# Migrations run the way the local stack runs them: sqlx-cli, at the version
# Cargo.lock pins, against crates/macro_db_client/migrations.
sqlx_version="$(grep -A1 '^name = "sqlx"$' Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p')"
cargo install sqlx-cli --locked --version "$sqlx_version" --no-default-features \
  --features postgres,rustls --root "$out/sqlx"
install -m 755 "$out/sqlx/bin/sqlx" "$out/bin/sqlx"
rm -rf "$out/sqlx"
cp -r crates/macro_db_client/migrations "$out/migrations"
cp infra/selfhost/lambda-host/host.py "$out/lambda-host.py"
