#!/usr/bin/env bash
# CI wrapper around build-rust.sh: build inside rust:1.94.0-trixie (the runtime
# image's Debian), as the calling user so the cargo cache in ~/.cargo and
# target/ stays readable. RUSTUP_TOOLCHAIN skips rust-toolchain.toml's extra
# cross targets, which the image does not need.
#
# Usage: ci-build-rust.sh <out-dir>   (from the repository root)
set -euo pipefail

out="${1:?usage: ci-build-rust.sh <out-dir>}"
docker build -t selfhost-rust-builder - <<'DOCKERFILE'
FROM rust:1.94.0-trixie
RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake clang libclang-dev libcurl4-openssl-dev libglib2.0-dev liblzma-dev libssl-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*
DOCKERFILE
mkdir -p "$HOME/.cargo" target
docker run --rm --user "$(id -u):$(id -g)" \
  -e HOME=/tmp -e CARGO_HOME=/cargo-home -e RUSTUP_TOOLCHAIN=1.94.0 -e CARGO_INCREMENTAL=0 \
  -v "$HOME/.cargo:/cargo-home" -v "$PWD:/src" -w /src \
  selfhost-rust-builder infra/selfhost/images/build-rust.sh "/src/$out"
