# Local development in a devcontainer

The Olive Trees fork runs on a laptop without installing a toolchain: the
repository's devcontainer (`.devcontainer/`) brings the Nix shell (Rust, Bun,
`just`, `cargo x`), Python and its own Docker daemon. Two ways to use it:

- **The self-hosted stack**: the same images and compose file as the server
  ([self-hosting.md](self-hosting.md)), at `https://macro.localhost`. Use it to
  try a build before it goes to the VPS.
- **The local stack**: upstream's development stack (`just run_local`), which
  builds every service from your checkout. Use it to change code.

Run one at a time; together they need more memory than a laptop gives Docker.

## What the laptop needs

- A Docker runtime: Docker Desktop or OrbStack. Give it at least 12 GB of
  memory (16 GB is comfortable) and 100 GB of disk. On Apple Silicon, turn on
  x86_64/amd64 emulation with Rosetta (Docker Desktop: Settings → General).
- An editor with devcontainer support: VS Code with the Dev Containers
  extension, or any client of the devcontainer CLI.
- Chrome or Firefox for `*.localhost` addresses.

Nothing else: Rust, Bun, Python and Compose live in the container.

## Open the container

In VS Code: **Dev Containers: Clone Repository in Container Volume…**, with
`https://github.com/OlivierVerhaegen/macro` and the branch to work on. The
checkout then lives in a Docker volume, which is much faster than a folder
shared from macOS for Rust builds and `node_modules`.

The first start downloads the Nix toolchain (several GB, once; the store is
kept in the `macro-devcontainer-nix` volume). Run every command below in the
container's terminal, from the repository root.

## The self-hosted stack

The images are amd64 only; on Apple Silicon they run under emulation, so
starting is slower than on the server. Pick a commit whose **Self-hosted
images** run is green; that is `MACRO_VERSION`, and the checkout should be on
the same commit.

```sh
mkdir -p .selfhost
cp infra/selfhost/selfhost.conf.example .selfhost/selfhost.conf
# Set: domain = macro.localhost, signup_allowlist (your address), the sender
# and the Zoho app password; and the model keys under [env] for agent editing.
$EDITOR .selfhost/selfhost.conf

nix develop --command python3 infra/selfhost/render.py --dir .selfhost

export MACRO_SELFHOST_DIR=$PWD/.selfhost MACRO_VERSION=<commit>
compose="docker compose -f infra/selfhost/compose.yml --env-file .selfhost/selfhost.env"
$compose up -d --wait postgres redis kafka search fusionauth garage aws-gateway
nix develop --command infra/selfhost/provision.sh
$compose up -d
```

Open `https://macro.localhost` and log in with an address from
`signup_allowlist`; the code arrives by mail through Zoho. Caddy signs
`macro.localhost` with its own local authority, so the browser warns once. To
trust it instead, copy the root certificate out and add it to the macOS
keychain:

```sh
$compose cp proxy:/data/caddy/pki/authorities/local/root.crt .selfhost/caddy-root.crt
```

`.selfhost/` holds this stack's secrets and is ignored by git. Stop the stack
with `$compose down`; never add `-v` unless you want to lose its data.

## The local stack

Upstream's guide applies unchanged inside the container:
[running-locally.md](../development/running-locally.md#run-the-local-stack).

```sh
nix develop
just doctor-local
just run_local --no-doppler
```

The services build natively for the container's architecture. The proxy is at
`https://localhost:8090` and Mailpit, where login codes land, at
`http://localhost:8025`; trust `infra/local/certs/ca.pem` in the keychain once.

## Limits

- Not yet tried on a Mac: the container and both stacks are built for it, but
  the first run there may still show a problem.
- Self-hosted images run emulated on Apple Silicon; native arm64 images would
  need an arm64 build in CI (also what a Raspberry Pi needs).
- Login codes of the self-hosted stack go out through Zoho to real addresses;
  the local stack keeps them in Mailpit.
