# Plan fase 0: Macro op een eigen VPS

Status: voorstel, wacht op akkoord. Branch: `ota/fase-0-vps`.

Doel: Macro draait in productie op één VPS, zonder AWS, met persistente data,
echte e-mail en HTTPS op een eigen domein. Fase 1 (Zoho Mail) heeft fase 0
niet nodig om ontwikkeld te worden: dat gebeurt op de lokale stack. Fase 0 moet
af zijn voor Macro echt gebruikt wordt.

## 1. Vertrekpunt: de lokale stack

`docs/development/running-locally.md` beschrijft hoe Macro lokaal draait.
`just stack up --no-doppler` start alles in Docker zonder AWS-account en zonder
Doppler: Postgres, Redis, LocalStack (S3, SQS, DynamoDB, KMS), OpenSearch,
Kafka, FusionAuth, alle Rust-services, de sync-worker en een Caddy-proxy die
de app onder één adres serveert (`<proxy>/app/`).

Dat is bijna een self-hosted opstelling. Wat het nog geen productie maakt:

| Onderdeel lokaal | Probleem op een VPS | Oplossing in fase 0 |
| --- | --- | --- |
| LocalStack (S3, SQS, DynamoDB, KMS) | Een testtool. Resources en KMS-sleutels verdwijnen bij een herstart, en wat met een verdwenen KMS-sleutel versleuteld is, is onleesbaar (zie `tooling/xtask/crates/xtask_local/src/local/localstack.rs`). | S3 → MinIO of Garage. SQS → ElasticMQ. DynamoDB → DynamoDB Local met een persistent volume. KMS → de lokale sleutelbron uit fase 1 (sectie 3.2), ook voor de bestaande Cursor- en Codex-sleutels. Allemaal API-compatibel, dus alleen andere endpoints in de configuratie. |
| Kafka in testconfiguratie | Eén broker zonder afgestemde retentie. | Redpanda als één node, met persistent volume en retentie-instellingen. |
| Mailpit vangt alle mail op | Inlogcodes en uitnodigingen bereiken niemand. | FusionAuth en de notificatieservice versturen via echte SMTP (bijvoorbeeld een Zoho-verzendadres). SES-gebruik gaat naar SMTP of krijgt een SES-compatibele relay. |
| Gestubde integraties | Google, Stripe, CloudFront werken niet. | Stripe en CloudFront uitgeschakeld laten; Google alleen als je het nodig hebt. |
| Vaste testgeheimen | Iedereen kent ze. | Alle geheimen nieuw genereren, in één `.env`-bestand buiten de repo (of een secretbeheerder), met rechten 600. |
| Development-CA en `localhost` | Geen geldig certificaat. | Caddy met Let's Encrypt op `macro.olivetrees-agency.be` (of het certificaat van Cloudflare bij een tunnel). |
| Passwordless login "niet veilig om te publiceren" | De guide waarschuwt hier expliciet voor. | Uitzoeken waarom (open signup, FusionAuth-kickstart met testsleutels) en dichtzetten: eigen FusionAuth-API-sleutels, signup alleen voor Olivier's eigen adres (de bestaande signup-allowlist van de auth-service). |
| Lambda-handlers | Draaien in AWS. | Nagaan hoe de lokale stack ze draait en dat overnemen als containers die luisteren op ElasticMQ of MinIO-notificaties. |
| `stack down` wist alles | Gevaarlijk in productie. | Eigen compose-bestand en beheerscripts (`ota-stack up/update/backup`), los van `just stack`. |
| Geen back-ups | — | Dagelijkse `pg_dump`, back-up van de MinIO-bucket en van de sync-state, naar een externe opslag (bijvoorbeeld Hetzner Storage Box), met een geteste restore. |
| Durable Objects/KV/R2/D1 van de sync-worker | Lokaal geëmuleerd door `wrangler` in het `sync_state`-volume. | Uitzoeken of die emulatie stabiel genoeg is voor productie, of dat de sync-worker op Cloudflare zelf moet draaien (gratis tot een bepaald volume). Dit is het grootste onbekende punt. |

## 2. Server

Besloten: domein `macro.olivetrees-agency.be`; voorlopig logt alleen Olivier in.
Kandidaten: een OVH-VPS of een eigen Raspberry Pi.

| | OVH-VPS | Raspberry Pi 5 |
| --- | --- | --- |
| Architectuur | x86_64 | arm64; de stack bouwt al voor `aarch64-unknown-linux-gnu`, en Postgres, OpenSearch, Redpanda, MinIO en FusionAuth hebben arm64-images. Te verifiëren: elk overige image en de sync-worker. |
| Geheugen | Kies ≥ 24 GB | Max 16 GB. Drie JVM's (OpenSearch, FusionAuth, eventueel Kafka) plus ~30 Rust-services is krap; vermoedelijk enkel haalbaar met Redpanda in plaats van Kafka en een kleine OpenSearch-heap. |
| Opslag | SSD/NVMe | Verplicht een NVMe- of USB-SSD; een SD-kaart overleeft de schrijflast van Postgres en Kafka niet. |
| Bereikbaarheid | Vast IP, poort 80/443 | Thuis-IP: via Cloudflare Tunnel (`xtask_local` heeft al tunnelondersteuning, `cf_tunnel.rs`), zonder poorten open te zetten. |
| Bouwen | Op de server of in CI | Niet op de Pi zelf (Rust-builds duren uren): cross-compilen naar arm64 op een andere machine en de binaries kopiëren. |
| Beschikbaarheid | Datacenter, stroom en netwerk redundant | Thuisstroom en -internet; een storing betekent geen mailsync tot herstel. |
| Locatie (AVG) | EU-datacenter (Gravelines, Straatsburg, Frankfurt) | Bij jou thuis |

Voorstel: **OVH-VPS voor productie**. De Pi is geschikt als testomgeving of als
back-upbestemming, maar met 16 GB en thuisinternet is hij de zwakke schakel
voor een systeem dat je mail en documenten bevat. De eerste meting in stap 1
van de werkwijze bevestigt of 16 GB überhaupt volstaat; is dat zo, dan blijft
de Pi een optie.

Algemeen: Docker en Compose, geen Kubernetes; alleen 80 en 443 open (of
helemaal niets bij Cloudflare Tunnel), SSH met sleutel.

## 3. Werkwijze

1. Meten: de lokale stack een dag draaien, geheugen en CPU per container noteren.
2. Inventaris: elke AWS-, Doppler- en Cloudflare-afhankelijkheid per service,
   vanuit `local_env.rs`, de compose-generator en `infra/stacks/`.
3. Compose-bestand `infra/ota/compose.yml` met MinIO, ElasticMQ, DynamoDB Local,
   Redpanda, Postgres, Redis, OpenSearch, FusionAuth en de services, plus
   Caddy met Let's Encrypt.
4. De sync-worker beslissen (zelf hosten of Cloudflare).
5. E-mailverzending en de beveiliging van het inloggen.
6. Back-ups en een restore-test.
7. `docs/ota/self-hosting.md` met installatie, update en restore.

Wijzigingen in upstream-code alleen waar een endpoint of sleutelbron niet
configureerbaar is; de rest leeft in `infra/ota/` en `docs/ota/`.

## 4. Beslist

- Domein: `macro.olivetrees-agency.be`.
- Gebruikers: alleen Olivier. Klanttoegang komt pas in fase 5.
- Server: OVH-VPS of Raspberry Pi; keuze na de meting (sectie 2).
