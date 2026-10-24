# Plan fase 0: Macro op een eigen VPS

Status: akkoord, in uitvoering. Branch: `ota/fase-0-vps` (vanaf `main`).

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
| Kafka in testconfiguratie | Eén broker zonder afgestemde retentie. | Eén Kafka-node (KRaft) met persistent volume en retentie-instellingen; Redpanda als optie om geheugen te sparen. |
| Mailpit vangt alle mail op | Inlogcodes en uitnodigingen bereiken niemand. | FusionAuth en de notificatieservice versturen via echte SMTP (bijvoorbeeld een Zoho-verzendadres). SES-gebruik gaat naar SMTP of krijgt een SES-compatibele relay. |
| Gestubde integraties | Google, Stripe, CloudFront werken niet. | Stripe en CloudFront uitgeschakeld laten; Google alleen als je het nodig hebt. |
| Vaste testgeheimen | Iedereen kent ze. | Alle geheimen nieuw genereren, in één `.env`-bestand buiten de repo (of een secretbeheerder), met rechten 600. |
| Development-CA en `localhost` | Geen geldig certificaat. | Caddy met Let's Encrypt op `macro.olivetrees-agency.be` (of het certificaat van Cloudflare bij een tunnel). |
| Passwordless login "niet veilig om te publiceren" | De guide waarschuwt hier expliciet voor. | Uitzoeken waarom (open signup, FusionAuth-kickstart met testsleutels) en dichtzetten: eigen FusionAuth-API-sleutels, signup alleen voor Olivier's eigen adres (de bestaande signup-allowlist van de auth-service). |
| Lambda-handlers | Draaien in AWS. | Nagaan hoe de lokale stack ze draait en dat overnemen als containers die luisteren op ElasticMQ of MinIO-notificaties. |
| `stack down` wist alles | Gevaarlijk in productie. | Eigen compose-bestand en beheerscripts (`selfhost up/update/backup`), los van `just stack`. |
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
3. Compose-bestand `infra/selfhost/compose.yml` met MinIO, ElasticMQ, DynamoDB Local,
   Kafka, Postgres, Redis, OpenSearch, FusionAuth en de services, plus
   Caddy met Let's Encrypt.
4. De sync-worker beslissen (zelf hosten of Cloudflare).
5. E-mailverzending en de beveiliging van het inloggen.
6. Back-ups en een restore-test.
7. `docs/ota/self-hosting.md` met installatie, update en restore.

Wijzigingen in upstream-code alleen waar een endpoint of sleutelbron niet
configureerbaar is; de rest leeft in `infra/selfhost/` en `docs/ota/`.

## 4. Beslist

- Domein: `macro.olivetrees-agency.be`.
- Gebruikers: alleen Olivier. Klanttoegang komt pas in fase 5.
- Server: OVH-VPS of Raspberry Pi; keuze na de meting (sectie 2).

## 5. Ontwerp na de inventaris (24 oktober 2026)

### 5.1 Wat de lokale stack draait

`docker/docker-compose.yml` (met `docker/docker-compose-databases.yml` en
`infra/stacks/fusionauth-instance/docker-compose.yml`) plus het gegenereerde
override van `cargo x` (`tooling/xtask/crates/xtask_local`) draait:

- Rust-services: `authentication-service`, `connection_gateway`,
  `contacts_service`, `document_storage_service`, `document_upload_finalizer`
  (lokale vervanger van een Lambda), `email_service`, `email_pubsub_workers`,
  `calendar_service`, `notification_service`, `static_file_service`,
  `unfurl_service`, `image_proxy_service`, `agent_harness_service`,
  `slack_import_worker`, en opt-in `search_processing_service`.
- Niet-Rust: `sync_service` (Cloudflare Worker via `wrangler dev`, state in het
  volume `sync_state`), `websocket_service`, `lexical_service`,
  `static_file_cdn` (nginx), `ai_editing_worker`, `analytics_proxy`.
- Infra: Postgres, Redis, OpenSearch, FusionAuth, Kafka, LocalStack
  (`sqs,dynamodb,s3,kms`), Mailpit en Caddy.
- De configuratie (~900 regels) komt uit `local_env.rs`: queue-, bucket- en
  tabelnamen in `resources.rs`, de FusionAuth-kickstart in `kickstart.rs`.

### 5.2 AWS-afhankelijkheden en vervangers

| AWS-dienst | Gebruikt door | Vervanger | Waarom |
| --- | --- | --- | --- |
| Endpoint | alle services via `macro_aws_config` | Eén interne gateway (Caddy) op het adres van `LOCAL_AWS_URL` | De code kent één endpoint voor alle diensten. De gateway routeert op `X-Amz-Target` (`AmazonSQS.*`, `DynamoDB_*`, `TrentService.*`); de rest is S3. Geen code-aanpassing. |
| S3 | documenten, statische bestanden, bijlagen, uploads | s3proxy met filesystem-backend en `authorization=none` | De code herschrijft presigned URLs naar het publieke adres (`transform_local_url`), waardoor de handtekening niet meer klopt. MinIO en Garage controleren handtekeningen en weigeren zulke URLs; s3proxy kan dat uitzetten. Toegang loopt alleen via de interne gateway en via Caddy-routes met presigned paden. |
| S3 → SQS-notificatie | `document_upload_finalizer` (ObjectCreated op de document-bucket) | `infra/selfhost/upload-relay/relay.py` vóór s3proxy, voor al het S3-verkeer (gateway en publiek uploadpad): na een geslaagde schrijfactie op `doc-storage` een S3-event naar de finalizer-queue | s3proxy kent geen notificaties. Het event vertrekt vóór het antwoord aan de client. |
| SQS | bijna alle workers | ElasticMQ met persistente queues (`elasticmq.conf`) | API-compatibel, ook met delay-seconds (verzenden plant met tot 900 s vertraging). |
| DynamoDB | connection_gateway, document_storage, static_file, agent_harness, search | DynamoDB Local met `-sharedDb -dbPath` op een volume | Officieel image van AWS, persistent. |
| KMS | authentication_service (Microsoft-, Cursor-sleutels), agent_harness | local-kms (`nsmithuk/local-kms`) met vaste sleutels in een seed-bestand | Sleutels moeten herstarts overleven, anders is versleutelde data onleesbaar. Zoho gebruikt al de lokale sleutelbron uit fase 1. |
| SES | authentication_service, notification_service | Via SMTP (Zoho-verzendadres) | Nog uit te zoeken: of beide altijd SES gebruiken of een SMTP-pad hebben. |
| Secrets Manager | bijna alle services | Niet nodig | Lokaal leest `MacroConfig` alles uit de omgeving; per service na te gaan. |
| Lambda (cron) | `email_scheduled_handler` (herstel en verzending verder dan 15 min), retentie-handlers, `deleted_item_poller`, Gmail-handlers | Een klok per handler (`infra/selfhost/lambda-clock/clock.py`) die de Lambda Runtime API spreekt en elke interval een EventBridge-event geeft | De binaries gebruiken `lambda_runtime` en draaien ongewijzigd. Een eigen klok in plaats van de Runtime Interface Emulator: geen extra binary, en een trage run slaat tikken over zoals Lambda met concurrency 1. Eerst alleen `email_scheduled_handler`; Gmail-handlers zijn niet nodig. |
| Lambda (S3-events) | `docx_unzip_handler`, `document_text_extractor`, `image_optimizer`, `convert_service` | Per pipeline uit te zoeken | Deze draaien ook lokaal niet; nagaan welke functies zonder ze ontbreken (Word-import, zoeken in tekst, afbeeldingen). |

Belangrijk voor fase 1: "nu verzenden" zet een bericht direct op de SQS-queue
met vertraging; de cron-handler is het vangnet bij fouten en voor berichten die
verder dan 15 minuten in de toekomst gepland zijn. Zonder die cron blijven zulke
berichten liggen.

### 5.3 Configuratie: upstream genereren, productie overschrijven

`cargo x validate-local-env --no-doppler --instance selfhost --env-file <override>`
schrijft de volledige env (185 sleutels) naar
`infra/local/generated/selfhost/local.generated.env`. De `--env-file`-laag ligt
boven de gegenereerde waarden. De self-host-configuratie is dus: wat upstream
genereert, plus één override-bestand met de productiewaarden (domein,
`LOCAL_AWS_URL` naar de gateway, SMTP, ...).
Er is geen kopie van `local_env.rs` die kan afdrijven.

**Beveiliging (blokkerend):** de lokale identiteit staat vast in de code
(`xtask_local/src/local/identity.rs`): de JWT-ondertekensleutel, de
FusionAuth-API-sleutel, de client-secret, de interne auth-sleutel en het
sleutelpaar voor API-tokens. Andere geheimen worden afgeleid van de
instancenaam. Wie de code kent, kan daarmee inlogtokens vervalsen. Een
render-stap in `infra/selfhost/` vervangt daarom elk geheim door een nieuwe
willekeurige waarde, op dezelfde manier in de env én in de FusionAuth-kickstart,
en bewaart ze in het geheimenbestand op de server. Daarnaast toont Caddy van
FusionAuth alleen de login- en OAuth-paden, nooit `/api` of de admin.

### 5.4 Overige keuzes

- **Images:** de workflow `selfhost-images` (gegenereerd door
  `cargo x workflows`, draait alleen in de fork) bouwt op GitHub-runners en
  pusht naar GHCR, getagd met commit en branch: één `macro-rust`-image met alle
  Rust-binaries (release, in `rust:1.94.0-trixie`, zonder lokale features),
  `macro-web` (de bundel) en `macro-sync`, `macro-lexical`,
  `macro-websocket` uit de Dockerfiles van de lokale stack. De VPS haalt alleen
  images op. Alleen amd64: een Pi (arm64) vraagt een tweede build.
- **Kafka:** dezelfde broker als lokaal (`apache/kafka`, KRaft, één node), zodat
  topics en clientinstellingen identiek blijven. Redpanda kan later geheugen
  besparen.
- **Sync-worker:** `wrangler dev` (workerd) met state op een volume, zoals
  lokaal. Voor één gebruiker aanvaardbaar; het risico is dat `wrangler dev`
  geen productieruntime is. Alternatief blijft Cloudflare zelf.
- **Configuratie:** het compose-bestand en een `.env.example` in
  `infra/selfhost/`; geheimen in `/etc/macro/selfhost.env` (rechten 600),
  nooit in de repo. De env-waarden volgen `local_env.rs` met productieadressen.
- **Toegang:** één publieke Caddy met Let's Encrypt voor
  `macro.olivetrees-agency.be`; interne diensten (gateway, Postgres,
  OpenSearch) zonder publieke poort.

### 5.5 Volgorde van bouwen

Status: 1 tot 5 staan in `infra/selfhost/` en `docs/ota/self-hosting.md`, lokaal getest maar nog niet op de VPS; 6 volgt.

1. `infra/selfhost/compose.yml` met de infra (Postgres, Redis, OpenSearch,
   Kafka, FusionAuth, s3proxy, ElasticMQ, DynamoDB Local, local-kms, de
   AWS-gateway) en een provisioning-stap voor queues, buckets, tabellen en
   KMS-sleutels (overgenomen uit `localstack.rs` en `resources.rs`).
2. De Rust-services en de niet-Rust-services, met de env uit 5.3.
3. Cron-handlers via de Lambda-klok; de upload-relay; migraties (sqlx-cli).
4. Publieke Caddy, e-mail (SMTP) en het afschermen van signup.
5. Back-ups en restore; `docs/ota/self-hosting.md`.
6. Meten op de echte VPS (stap 1 van sectie 3) en de servergrootte vastleggen.

### 5.6 Bevindingen tijdens het bouwen

- **`ENVIRONMENT=local` blijft.** Productie-modus codeert `macro.com` in
  (redirects, cookie-domein, CORS); lokaal volgt `FRONTEND_ORIGIN` en zet
  host-only cookies, wat voor één domein klopt. Wat lokaal nog meer doet en
  in fase 0 afgeschermd moet worden:
  - signup stond voor iedereen open (`SignupPolicy::allow_all`). Opgelost met
    een kleine upstream-wijziging: onder Local geldt
    `DEVELOPMENT_SIGNUP_ALLOWLIST_JSON` als die gezet is; `render.py` eist
    `[server] signup_allowlist`. Macro laat `@macro.com`-adressen altijd toe;
  - webhooks mogen naar `http://`-adressen, ook interne; alleen ingelogde
    gebruikers maken ze aan;
  - cookies hebben `SameSite=None` (wel `Secure` en `HttpOnly`).
- **De inlogcode in het antwoord** hangt aan de compile-feature
  `return_passwordless_code`, niet aan `ENVIRONMENT`. Het image bouwt zonder die
  feature en zonder `no_rate_limit`.
- **De webbundel** is de local-backend-bundel van `cargo x stack up`
  (`VITE_LOCAL_BACKEND_ORIGIN=same-origin`): alle servicecalls gaan naar het
  eigen domein, zonder codewijziging. `import.meta.env.DEV` staat daarin aan;
  dat zet onder meer de automatische login met `?email=` aan, die alleen werkt
  als de backend de code teruggeeft, wat hij hier niet doet.
- **Niet in fase 0:** `ai_editing_worker` en `analytics_proxy` (Cloudflare
  Workers die lokaal de repo mounten); de overige cron- en S3-event-Lambdas.
- **Back-up:** naast Postgres en de S3-data bevat het volume `sync_state` de
  live staat van gedeelde documenten.
- **`agent_harness_service`** krijgt de Docker-socket van de host, zoals
  lokaal, om sandboxes te starten. Dat is root op de host voor wie die service
  overneemt.
- **GHCR:** nieuwe packages zijn privé. Zet ze publiek of log op de server in
  met een token met `read:packages`.
- **Opslag was publiek:** s3proxy controleert geen handtekeningen, dus een
  publieke `/local-storage`-route gaf iedereen toegang tot elke bucket. De
  upload-relay controleert op de publieke poort de presigned SigV4-handtekening
  met een eigen AWS-secret per server; alleen leesacties op de
  static-file-bucket zijn vrij, zoals een CDN.
- **E-mail:** `mailrelay` (eigen, Python standaardbibliotheek) in plaats van
  een Postfix-image: geen wachtrij, een fout bij Zoho gaat terug naar de
  verzendende service. Het app-wachtwoord staat in een apart env-bestand.

