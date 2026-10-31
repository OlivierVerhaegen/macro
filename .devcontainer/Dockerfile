# The development container for the Olive Trees fork: the repository's Nix
# toolchain (added by the nix feature) plus Python for infra/selfhost.
FROM mcr.microsoft.com/devcontainers/base:bookworm
RUN apt-get update \
    && apt-get install -y --no-install-recommends python3 \
    && rm -rf /var/lib/apt/lists/*
