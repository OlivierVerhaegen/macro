#!/usr/bin/env bash
# Build one self-hosted image and push it to GHCR as <name>:<commit> and
# <name>:<branch>. With --check-libs, fail when a binary in /app/out misses a
# shared library in the image.
#
# Env: REGISTRY (e.g. ghcr.io/owner), GITHUB_TOKEN, GITHUB_ACTOR, GITHUB_SHA,
# GITHUB_SERVER_URL, GITHUB_REPOSITORY, GITHUB_REF_NAME.
# Usage: publish.sh <name> <dockerfile> <context> [--check-libs]
set -euo pipefail

name="${1:?usage: publish.sh <name> <dockerfile> <context> [--check-libs]}"
dockerfile="${2:?}"
context="${3:?}"
image="$REGISTRY/$name"

printf '%s' "$GITHUB_TOKEN" | docker login ghcr.io -u "$GITHUB_ACTOR" --password-stdin
DOCKER_BUILDKIT=1 docker build -f "$dockerfile" \
  --label "org.opencontainers.image.source=$GITHUB_SERVER_URL/$GITHUB_REPOSITORY" \
  --label "org.opencontainers.image.revision=$GITHUB_SHA" \
  -t "$image:$GITHUB_SHA" "$context"
if [[ "${4:-}" == "--check-libs" ]]; then
  docker run --rm --entrypoint sh "$image:$GITHUB_SHA" -c \
    'for f in /app/out/*; do if ldd "$f" | grep "not found"; then echo "missing libraries: $f"; exit 1; fi; done'
fi
branch="${GITHUB_REF_NAME//\//-}"
docker tag "$image:$GITHUB_SHA" "$image:$branch"
docker push "$image:$GITHUB_SHA"
docker push "$image:$branch"
