# Macro self-hosten op één server

Handleiding voor de Olive Trees-fork: Macro op één VPS, zonder AWS. Het ontwerp
en de keuzes staan in [plan-fase-0.md](plan-fase-0.md); de technische details
per bestand in [`infra/selfhost/AGENTS.md`](../../infra/selfhost/AGENTS.md).

## Wat je nodig hebt

- Een amd64-server (OVH-VPS) met minstens 24 GB RAM en 100 GB schijf, Debian
  of Ubuntu, met Docker Engine en de compose-plugin.
- Een DNS-record `macro.olivetrees-agency.be` naar het IP van de server;
  poorten 80 en 443 open, al de rest dicht.
- Op de server: `git`, `python3`, `openssl`, `ssh-keygen` en een Rust-toolchain
  (`rustup`). De render- en provisioning-stap gebruiken `cargo x` uit de repo.
- Een Zoho-app-wachtwoord voor SMTP (Zoho Mail → Instellingen → Beveiliging →
  App-wachtwoorden). Dat vul je alleen op de server in, in `selfhost.conf`.
- Toegang tot de images: de packages `macro-rust`, `macro-web`, `macro-sync`,
  `macro-lexical` en `macro-websocket` op GHCR publiek zetten (GitHub →
  Packages → Package settings), of op de server inloggen met
  `docker login ghcr.io` en een token met `read:packages`.

## Images

De workflow **Self-hosted images** bouwt bij elke push naar `main` of
`ota/**` alle images en tagt ze met de commit en de branch. Kies de commit
waarvan de run groen is; dat is `MACRO_VERSION`. Check dezelfde commit uit op
de server, zodat compose-bestand, scripts en images bij elkaar horen.

## Eerste installatie

Alles als root, vanuit een checkout van de repo (bijvoorbeeld `/opt/macro`).

```sh
git clone https://github.com/OlivierVerhaegen/macro /opt/macro
cd /opt/macro && git checkout <MACRO_VERSION>

install -d -m 700 /etc/macro
install -m 600 infra/selfhost/selfhost.conf.example /etc/macro/selfhost.conf
# Vul in: domein, signup_allowlist (jouw adres), afzender, het app-wachtwoord.
$EDITOR /etc/macro/selfhost.conf

# Genereert /etc/macro/selfhost.env, mailrelay.env, kickstart/ en secrets.json
# met nieuwe geheimen voor deze server.
python3 infra/selfhost/render.py

export MACRO_VERSION=<commit>
compose="docker compose -f infra/selfhost/compose.yml --env-file /etc/macro/selfhost.env"

# Eerst de infrastructuur, dan topics, queues, buckets, tabellen en sleutels.
$compose up -d --wait postgres redis kafka search fusionauth aws-gateway
infra/selfhost/provision.sh

# Daarna alles: de migraties lopen eerst, de services wachten erop.
$compose up -d
```

Open `https://macro.olivetrees-agency.be`, log in met je adres uit
`signup_allowlist`; de code komt per mail via Zoho.

`secrets.json` bevat alle geheimen van de server. Opnieuw renderen (na een
wijziging in `selfhost.conf` of een upgrade) houdt ze, dus verlies dat bestand
niet: het zit in elke back-up.

## Upgraden

```sh
cd /opt/macro && git fetch && git checkout <nieuwe commit>
python3 infra/selfhost/render.py
infra/selfhost/provision.sh
MACRO_VERSION=<nieuwe commit> docker compose -f infra/selfhost/compose.yml \
  --env-file /etc/macro/selfhost.env up -d
```

`up -d` start `migrate` opnieuw; de services herstarten pas daarna. Maak
vooraf een back-up.

## Back-ups

`infra/selfhost/backup.sh <map>` schrijft een map per back-up: dumps van
MacroDB en FusionAuth, archieven van de volumes met bestanden (S3-data, de
live staat van gedeelde documenten, KMS-sleutels, DynamoDB, queues) en
`/etc/macro`. Die bestanden-volumes staan daarbij enkele seconden stil. Het
script bewaart de laatste 14 (`KEEP`).

Dagelijks, met een kopie buiten de server (bijvoorbeeld OVH Object Storage via
`rclone`):

```cron
15 3 * * * cd /opt/macro && infra/selfhost/backup.sh /var/backups/macro && rclone sync /var/backups/macro remote:macro-backups
```

Een back-up bevat elk geheim van de server; versleutel de kopie buiten de
server (`rclone crypt`).

Niet in de back-up, want ze worden opnieuw opgebouwd: de zoekindex, Redis,
Kafka en de TLS-certificaten.

## Terugzetten

Op dezelfde server:

```sh
infra/selfhost/restore.sh /var/backups/macro/<tijdstip>
MACRO_VERSION=<commit> docker compose -f infra/selfhost/compose.yml \
  --env-file /etc/macro/selfhost.env up -d
```

Op een nieuwe server: Docker, de checkout van dezelfde commit, dan
`restore.sh <map> --config` (zet ook `/etc/macro` terug) en dezelfde `up -d`.
Daarna `provision.sh` voor de Kafka-topics.

## Wat publiek is

Alleen Caddy (`proxy`) op 80 en 443. Daarachter: de app onder `/app`, de
services onder hun pad, en opslag onder `/local-storage` en `/static-file`,
waar de upload-relay elke aanvraag zonder geldige presigned handtekening
weigert. FusionAuth, de databases, de AWS-vervangers en de mailrelay hebben
geen publieke route. De AWS-gateway luistert op `127.0.0.1:4566` voor
`provision.sh`.

## Beperkingen nu

- Alleen amd64; een Raspberry Pi vraagt arm64-images.
- Niet inbegrepen: de AI-editing-worker, de analytics-proxy en de meeste
  geplande Lambdas (alleen `email_scheduled_handler` draait).
- `agent_harness_service` krijgt de Docker-socket van de host, zoals lokaal.
- Inloggen met Google of Microsoft vraagt een publieke FusionAuth-route; nu
  alleen inloggen met een code per mail.
