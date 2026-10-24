# syntax=docker/dockerfile:1
# The AI editing worker (a Cloudflare Worker) under `wrangler dev --env local`,
# as the local stack runs it, built into an image instead of mounting the
# repository. It imports workspace packages, so it installs from the root.
FROM node:22-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN npm install -g bun@1.3.5
ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
ENV WRANGLER_SEND_METRICS=false
WORKDIR /app
COPY . .
RUN bun install --frozen-lockfile
WORKDIR /app/services/ai-editing-worker
RUN bun scripts/generate-sandbox.ts
EXPOSE 8933
# Secrets reach the worker through .dev.vars, written from the container's
# environment at start; its trace database (D1) lives in .wrangler/state.
CMD ["/app/infra/selfhost/images/ai-editing-start.sh"]
