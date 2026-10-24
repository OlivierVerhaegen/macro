# syntax=docker/dockerfile:1
# The analytics proxy (a Cloudflare Worker) under `wrangler dev --env local`,
# as the local stack runs it. It forwards /i/ph to PostHog and /i/otlp to the
# OTLP intake in OTLP_TRACES_INTAKE_URL and OTLP_LOGS_INTAKE_URL.
FROM node:22-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN npm install -g bun@1.3.5
ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
ENV WRANGLER_SEND_METRICS=false
WORKDIR /app
COPY services/analytics-proxy/ ./
RUN bun install --frozen-lockfile
EXPOSE 8098
CMD ["sh", "-c", "exec ./node_modules/.bin/wrangler dev --env local --ip 0.0.0.0 --port 8098 \
  --var OTLP_TRACES_INTAKE_URL:${OTLP_TRACES_INTAKE_URL} --var OTLP_LOGS_INTAKE_URL:${OTLP_LOGS_INTAKE_URL}"]
