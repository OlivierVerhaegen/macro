#!/bin/sh
# Start the AI editing worker: its secrets into .dev.vars (wrangler dev's secret
# store), its trace database's migrations, then the worker.
set -eu
umask 077
: > .dev.vars
for key in ANTHROPIC_API_KEY CEREBRAS_API_KEY OPENAI_API_KEY GOOGLE_GENERATIVE_AI_API_KEY TRACE_ADMIN_KEY INTERNAL_API_KEY; do
  printf '%s=%s\n' "$key" "$(printenv "$key" || true)" >> .dev.vars
done
# CI=true answers the confirmation prompt, as the sync image does.
CI=true npx wrangler d1 migrations apply TRACES_DB --local --env local
exec npx wrangler dev --env local --ip 0.0.0.0 --port 8933
