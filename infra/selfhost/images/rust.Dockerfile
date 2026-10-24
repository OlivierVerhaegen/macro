# syntax=docker/dockerfile:1
# Runtime image for every self-hosted Rust service. The context is the staging
# directory build-rust.sh writes; each compose service picks its binary.
FROM denoland/deno:2.9.6 AS deno

FROM debian:trixie-slim
# docker-cli: the agent harness starts its sandboxes on the host's daemon
# through the mounted socket, as in the local stack. python3: the Lambda host
# that runs the handlers. ffmpeg: call recording previews.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl dumb-init docker-cli ffmpeg libssl3t64 python3 \
    && apt-get clean && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=deno /usr/bin/deno /usr/local/bin/deno
ENV DENO_BINARY=/usr/local/bin/deno
COPY bin/ /app/out/
# search_processing_service loads pdfium relative to its working directory.
COPY pdfium-lib/ /app/pdfium-lib/
# MacroDB migrations, applied by the `migrate` service with /app/out/sqlx.
COPY migrations/ /app/migrations/
# Runs a Lambda handler with its triggers: host.py -- /app/out/<handler>.
COPY lambda-host.py /app/lambda-host/host.py
ENV PORT=8080
EXPOSE 8080
ENTRYPOINT ["dumb-init", "--"]
