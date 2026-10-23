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
| Development-CA en `localhost` | Geen geldig certificaat. | Caddy met Let's Encrypt op een eigen domein (bijvoorbeeld `macro.olivetrees-agency.be`). |
| Passwordless login "niet veilig om te publiceren" | De guide waarschuwt hier expliciet voor. | Uitzoeken waarom (open signup, FusionAuth-kickstart met testsleutels) en dichtzetten: eigen FusionAuth-API-sleutels, signup-allowlist voor je eigen domein en klantdomeinen. |
| Lambda-handlers | Draaien in AWS. | Nagaan hoe de lokale stack ze draait en dat overnemen als containers die luisteren op ElasticMQ of MinIO-notificaties. |
| `stack down` wist alles | Gevaarlijk in productie. | Eigen compose-bestand en beheerscripts (`ota-stack up/update/backup`), los van `just stack`. |
| Geen back-ups | — | Dagelijkse `pg_dump`, back-up van de MinIO-bucket en van de sync-state, naar een externe opslag (bijvoorbeeld Hetzner Storage Box), met een geteste restore. |
| Durable Objects/KV/R2/D1 van de sync-worker | Lokaal geëmuleerd door `wrangler` in het `sync_state`-volume. | Uitzoeken of die emulatie stabiel genoeg is voor productie, of dat de sync-worker op Cloudflare zelf moet draaien (gratis tot een bepaald volume). Dit is het grootste onbekende punt. |

## 2. VPS

- Indicatief: 8 vCPU, 32 GB RAM, 200 GB SSD. OpenSearch, Kafka en de Rust-services
  samen zijn zwaar; 16 GB is waarschijnlijk te krap. Dit bevestig ik in fase 0
  met een meting op de lokale stack (`docker stats`).
- In de EU (bijvoorbeeld Hetzner Falkenstein of Helsinki), voor de AVG.
- Docker en Compose; geen Kubernetes.
- Firewall: alleen 80 en 443 open, SSH met sleutel.

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

## 4. Open vragen

1. Heb je al een VPS-aanbieder, of mag ik Hetzner voorstellen?
2. Welk domein wil je gebruiken?
3. Wie moet kunnen inloggen: alleen jij, of ook klanten (fase 5)? Dat bepaalt
   de signup-regels.
