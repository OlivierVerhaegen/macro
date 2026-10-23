# Plan fase 1: Zoho Mail via IMAP/SMTP plus Zoho API

Status: voorstel, wacht op akkoord. Branch: `ota/fase-1-zoho-mail`.

## 1. Verkenning

### Huidige structuur (upstream, oktober 2026)

Outlook draait niet op een eigen sync-lus maar op een provider-neutrale
mailbox-runtime. Dat is het sjabloon voor IMAP.

| Laag | Waar | Wat het doet |
| --- | --- | --- |
| Domein-port | `crates/email/src/domain/mailbox.rs` | `MailboxGateway` (`folders`, `changes`, `message`) plus `MailboxSyncService`: catalogus, per-map streams met opaque `StreamToken`, en reconciliatie per bericht, alles onder leases met fences. |
| Andere domein-ports | `crates/email/src/domain/mailbox/{commands,drafts,watches,contacts,settings,credentials}.rs` | Gelezen/vlag/verplaatsen (`MailboxCommandGateway`), concepten en verzenden (`DraftGateway`), notificaties (`MailboxWatchGateway`), adresboek, instellingen, tokens (`MailboxCredentialGateway`). |
| Provider-vocabulaire | `crates/email_api_client/src/domain/{models/provider.rs,ports/mailbox.rs}` | `MailFolder`, `MailboxMessage`, `MailboxChangePage`, `StreamPosition`, `MessageAction`, en per capability een port (`MailboxContentReader`, `FolderChangeReader`, `MailboxActionWriter`, `MailboxDraftClient`, ...). |
| Graph-adapter | `crates/email_api_client/src/outbound/outlook/` | Implementeert die ports voor Microsoft Graph. |
| Lijm in de service | `services/email_service/src/outbound/email_api/microsoft.rs` | `ProviderMailboxGateway` koppelt `MailboxApiService` aan de domein-ports; `MicrosoftCredentialsClient` haalt tokens bij de auth-service. |
| Compositie | `services/email_service/src/bin/pubsub_workers/pubsub_workers.rs`, `src/server.rs` | Start de workers (sync, contacts, watches, commands, drafts, projections) met de Outlook-gateway, achter `outlook_sync_enabled` / `outlook_writes_enabled`. |
| OAuth-custody | `services/authentication_service/src/{domain,outbound}/microsoft*`, `crates/microsoft_oauth_grant_db_utils` | PKCE-koppeling, refresh-tokens versleuteld met een AES-256-GCM-envelope en AWS KMS-datasleutels, generatie-gebonden tokens. |
| Opslag | `crates/email/src/outbound/mailbox_pg/` | Leases en checkpoints in Postgres. Ingest threadt op `provider_thread_id`. |

Drie vaststellingen bepalen het plan:

1. **De runtime is generiek, de SQL niet.** Ongeveer 40 queries filteren
   hard op `l.provider = 'OUTLOOK'` (claims in `mailbox_pg/*`, delen van
   `email_pg_repo/*`, `email_db_client/links`, `mailbox_init.rs`). Een nieuwe
   provider vraagt dus aanpassingen in upstream-bestanden; dat valt niet
   volledig in eigen crates te houden.
2. **Eén gateway per worker.** De sync-workers claimen werk over alle
   mailboxen heen en krijgen één gateway mee. IMAP krijgt daarom eigen
   worker-instanties met eigen claims (filter op provider), in plaats van een
   dispatcher in de Outlook-gateway.
3. **Threading gebeurt bij ingest op `provider_thread_id`.** Voor IMAP moet
   die bij het ophalen al vastliggen; de Zoho API (30 calls/minuut) kan dat
   niet per bericht leveren.

### De IMAP-branch (`claude/imap-smtp-support-hvt8e3`, upstream PR #3919)

Gebouwd op de oude layout (`rust/cloud-storage/`, `js/app/`) en de oude
Gmail-vormige poll-lus. Beoordeling per onderdeel:

| Onderdeel | Oordeel |
| --- | --- |
| `imap_smtp_client` (`imap.rs`, `smtp.rs`, `tls.rs`; `async-imap`, `mail-send`, rustls) | **Overnemen** als basis van een `ota_imap`-adapter, maar herschrijven naar de nieuwe ports en uitbreiden met XOAUTH2, CONDSTORE/QRESYNC, IDLE, APPEND en MOVE. |
| `convert/imap.rs` (MIME via `mailparse` naar het servicemodel, plus tests) | **Overnemen**, omzetten naar `MailboxMessage`. De synthetische Gmail-labels vervallen: de nieuwe runtime werkt met `FolderRole` en `MailboxState`. |
| Threading via `References`/`In-Reply-To` | **Overnemen als regel**: `provider_thread_id` = eerste Message-ID uit `References`, anders `In-Reply-To`, anders de eigen Message-ID. |
| `email_utils::credential_crypto` (eigen AES-GCM met een statische sleutel) | **Niet overnemen.** De auth-service heeft al een envelope-cipher met KMS (CS-26: geen tweede cryptopad). Die wordt algemener gemaakt. |
| Tabel `email_imap_folder_states`, operatie `ImapPoll`, `imap_modify_labels`, aanpassingen in `scheduled`/`link_manager` | **Vervalt.** De runtime heeft al streams, checkpoints, commands en leases. |
| `POST /email/links/imap` en de koppeldialoog | **Idee overnemen, code niet**: nieuwe route naast de bestaande link-routes, dialoog in `apps/web/src/features/inbox/`. |

## 2. CAPABILITY-check

Vanuit deze cloudsessie kan ik dit niet uitvoeren: de container kan
`imap.zoho.eu` niet resolven (`Name or service not known`). Voer dit lokaal uit.
Gebruik een Zoho **app-wachtwoord**, en plak achteraf alleen de regels die met
`* CAPABILITY`, `* LIST` of `* ENABLED` beginnen (nooit de `LOGIN`-regel).

```sh
openssl s_client -quiet -crlf -connect imap.zoho.eu:993 -servername imap.zoho.eu
a1 CAPABILITY
a2 LOGIN jouw@adres.be APP-WACHTWOORD
a3 CAPABILITY
a4 ENABLE QRESYNC
a5 LIST (SPECIAL-USE) "" "*"
a6 LIST "" "*"
a7 LOGOUT
```

En voor SMTP (zoek in de `EHLO`-antwoorden naar `AUTH ... XOAUTH2`):

```sh
openssl s_client -quiet -crlf -connect smtp.zoho.eu:465 -servername smtp.zoho.eu
EHLO test
QUIT
```

Wat ik in de uitvoer zoek, en wat het verandert:

| Capability | Als aanwezig | Als afwezig |
| --- | --- | --- |
| `CONDSTORE` + `QRESYNC` | Incrementele sync via `HIGHESTMODSEQ` en `VANISHED`. | Per map `UIDNEXT` volgen voor nieuwe mail en periodiek de flags vergelijken voor wijzigingen. |
| `IDLE` | Push voor de inbox. | Alleen polling. |
| `MOVE`, `UIDPLUS` | Verplaatsen en nieuwe UID's zonder extra zoekwerk. | COPY+EXPUNGE, en na APPEND opnieuw zoeken op Message-ID. |
| `SPECIAL-USE` | Maprollen (Sent, Drafts, Trash, Junk, Archive) betrouwbaar. | Rollen via een vaste Zoho-namenlijst. |
| `OBJECTID` | Stabiele bericht-ID over verplaatsingen heen. | ID = map + UIDVALIDITY + UID; een verplaatsing wordt "weg en nieuw". |
| `AUTH=XOAUTH2` (IMAP en SMTP) | OAuth voor beide. | Bepaalt de terugvaloptie voor verzenden (zie 3.2). |

Ook de vraag of Zoho-labels als IMAP-mappen of als keywords verschijnen, haal
ik uit de `LIST`-uitvoer.

## 3. Ontwerp fase 1

### 3.1 Provider

- Nieuwe enumwaarde `IMAP` in `email_user_provider_enum` en in de drie Rust
  `UserProvider`-enums (`crates/email`, `models_email` api en service), plus
  `graphql_email` en de gegenereerde frontendtypes.
- Per link een rij in een nieuwe tabel `ota_imap_accounts`: host, poort,
  SMTP-host, beveiliging, gebruikersnaam, auth-methode (`zoho_oauth` of
  `password`), datacenter, `zoho_account_id` (voor API-calls). Zoho is een
  voorinstelling die deze velden invult.
- Datacenter als enum `ZohoDataCenter { Com, Eu, In, ComAu, Jp, Ca, Sa, Cn }`
  met per waarde de accounts-, mail-API-, IMAP- en SMTP-host. Dit zijn
  providerendpoints die een gebruiker kiest, geen Macro-service-URL's, dus
  ze horen niet in `macro_service_urls`.
- De `'OUTLOOK'`-filters in de mailbox-runtime worden
  `l.provider IN ('OUTLOOK', 'IMAP')` waar de query over de runtime gaat, en
  blijven `'OUTLOOK'` waar het om Graph-specifiek gedrag gaat. Elke query
  krijgt een eigen beslissing; geen globale zoek-en-vervang.
- IMAP-workers claimen alleen IMAP-links (provider als parameter in de
  claim-queries), zodat Outlook- en IMAP-workers elkaars werk niet pakken.

### 3.2 Authenticatie

- **Zoho OAuth** in de auth-service, naar het Microsoft-patroon: een
  server-client (geen Self-Client), PKCE, ondertekende state, en
  refresh-tokens in een nieuwe tabel `ota_zoho_oauth_grants`, versleuteld
  met dezelfde envelope-cipher. Die cipher wordt generiek: het doel
  (`microsoft-refresh-token`, `zoho-refresh-token`, `imap-password`) gaat mee in
  de KMS-context en de AAD. De Microsoft-code verandert niet van gedrag.
- Scopes in fase 1: `ZOHOMAIL.PROTOCOL.IMAP`, `ZOHOMAIL.PROTOCOL.SMTP` (als de
  test slaagt), `ZohoMail.accounts.READ`, `ZohoMail.messages.READ` en
  `ZohoMail.folders.READ`. Scopes zijn per grant opgeslagen en uitbreidbaar;
  fase 6 voegt `ZohoCalendar.*` toe via een herkoppeling.
- Het token voor IMAP/SMTP gaat via een nieuwe `ImapCredentialGateway` die
  `MailboxCredentialGateway` volgt en de auth-service aanroept
  (`get_zoho_access_token`, naast `get_microsoft_access_token`).
- **Wachtwoord als terugval** voor generieke IMAP (en voor Zoho-SMTP als
  XOAUTH2 daar niet werkt): via dezelfde envelope-cipher opgeslagen, nooit
  gelogd, nooit naar de frontend teruggestuurd.
- Terugval voor verzenden als SMTP-XOAUTH2 niet werkt: eerst de Zoho Mail
  API (`POST /accounts/{id}/messages`). Een app-wachtwoord gebruiken we alleen
  als de API om een andere reden niet kan. Ik kies op basis van jouw
  testresultaat.

### 3.3 Sync (`MailboxGateway` voor IMAP)

- **`folders`**: `LIST` met `SPECIAL-USE`; rollen via die vlaggen, anders een
  Zoho-namenlijst.
- **`changes`**, per map: `StreamToken` = `UIDVALIDITY:HIGHESTMODSEQ:UIDNEXT`
  (of zonder modseq als CONDSTORE ontbreekt). Een andere UIDVALIDITY geeft
  `EmailApiError::OutdatedCursor`, zodat de runtime de stream reset. De
  eerste ronde gaat in pagina's van vaste grootte (`Continue`), daarna
  `Checkpoint`. Met QRESYNC komen verwijderingen uit `VANISHED`; zonder
  QRESYNC uit een periodieke `UID SEARCH ALL`-vergelijking.
- **`message`**: `UID FETCH` van `BODY.PEEK[]`, `FLAGS`, `INTERNALDATE` en
  `MODSEQ`, daarna MIME-conversie (overgenomen uit de branch) naar
  `MailboxMessage`. `version` = `MODSEQ`, of de flags-hash als er geen
  modseq is.
- **Bericht-ID** = `OBJECTID` als Zoho dat ondersteunt, anders
  `<map>/<UIDVALIDITY>/<UID>`.
- **Threading**: zie de regel in de tabel hierboven. Zoho's eigen `threadId`
  past daar niet in, omdat de API per bericht te traag is (30
  calls/minuut). Mijn voorstel is die stap te laten vallen; zie open vraag 3.
- **IDLE**: een kleine worker met één IDLE-verbinding per actieve IMAP-link,
  alleen op INBOX. Bij `EXISTS`, `EXPUNGE` of `FETCH` zet hij de
  inbox-stream op "due", net zoals een Graph-webhook dat doet. Andere mappen
  en gemiste events vangt de periodieke poll op.
- **Limieten van Zoho**: maximaal één IDLE-verbinding plus een kleine pool
  (standaard 3) per account, ver onder de grens van 100. Een fetch-budget per
  link in Redis volgt het patroon van `RedisMicrosoftRequestGate`, ingesteld
  op 1 GB per 15 minuten. Grote bijlagen worden pas bij openen opgehaald
  (`BODYSTRUCTURE` eerst, de body-parts op aanvraag via
  `MailboxContentReader::attachment`).

### 3.4 Schrijven

| Actie | IMAP |
| --- | --- |
| Gelezen/ongelezen | `UID STORE +/-FLAGS (\Seen)` |
| Ster | `\Flagged` |
| Verplaatsen, archiveren, prullenbak | `UID MOVE` (of COPY+EXPUNGE) naar de map met de juiste rol |
| Concept opslaan | `APPEND` naar Drafts met `\Draft`; bewerken = nieuwe APPEND plus het oude verwijderen (concepten zijn in IMAP onveranderlijk) |
| Verzenden | SMTP (`smtp.zoho.eu:465`), daarna `APPEND` naar Sent als Zoho dat niet zelf doet (te testen), en het concept verwijderen |
| Labels/tags | Via de Zoho API als labels geen IMAP-keywords zijn; anders keywords |

Verzenden volgt de bestaande `SubmissionOutcome`-regels: een time-out na het
SMTP-`DATA`-commando geeft `Unknown`, nooit een blinde herhaling. Het
concept draagt de `draft_correlation` mee in een eigen header, zodat de
verzonden kopie terug te vinden is, zoals bij Outlook.

### 3.5 Zoho Mail API (aanvulling, niet de sync)

Een eigen crate `ota_zoho_client` met de datacenterkeuze, een token-provider
via de auth-service en een rate-limiter (30 calls/minuut per account). In
fase 1 gebruikt voor:

- accountgegevens: `accountId`, primair adres, aliassen en verzendadressen
  (`GET /accounts`), bij het koppelen en daarna dagelijks;
- handtekeningen (`GET /accounts/{id}/signatures`), op dezelfde momenten;
- labels lezen en zetten, als labels niet via IMAP beschikbaar zijn.

Fase 6 hergebruikt de crate voor Calendar: enkel nieuwe scopes en een nieuwe
adapter.

### 3.6 Gmail-specifieke onderdelen uitschakelen

Filters, contactsync via Google en send-as via de Gmail API worden voor
`IMAP` overgeslagen. De settings-UI toont een melding ("Niet beschikbaar voor
IMAP-accounts"). Voor elk van de ~30 bestanden die op `UserProvider::Outlook`
matchen, controleer ik wat de `IMAP`-tak moet doen. Zonder wildcard-arm, zodat
de compiler elke plek afdwingt.

### 3.7 Frontend

- In `AddInboxDialog`: een optie "Zoho Mail" (OAuth, met datacenterkeuze) en
  "Andere IMAP-server" (formulier met host, poort, beveiliging,
  gebruikersnaam en wachtwoord).
- Code in `apps/web/src/features/ota/zoho-mail/`, ingeplugd in de
  bestaande dialoog met zo weinig mogelijk regels in upstream-bestanden.
- Achter een feature flag `ota_imap` (via `defineFlag`, als env-flag
  `VITE_ENABLE_OTA_IMAP` omdat PostHog bij zelf hosten optioneel is).

### 3.8 MCP-tools

De bestaande e-mailtools (`crates/email/src/inbound/toolset.rs`) werken via de
domeinservice en zijn provider-neutraal. Fase 1 heeft daarom geen nieuwe tools
nodig. Ik controleer elke tool op Gmail-only paden en laat een IMAP-inbox
dezelfde tools gebruiken. Een tool "IMAP-account koppelen" voor de agent voeg
ik niet toe: koppelen vraagt een browser-OAuth en een geheim.

### 3.9 Observability

- Spans `imap.sync_folder`, `imap.fetch_message`, `imap.idle_session`,
  `smtp.submit` en `zoho_api.request`, met `link_id`, map-rol, aantallen,
  bytes en uitkomst.
- Nooit e-mailadressen, onderwerpen of tokens in de spans.

## 4. Gedeelde fundamenten

| Fundament | Keuze |
| --- | --- |
| Naamgeving | Rust-crates `ota_*` (`ota_imap`, `ota_zoho_client`), tabellen `ota_*`, frontend `apps/web/src/features/ota/`, flags `ota_*`. |
| Feature flag | Backend: config-schakelaars `OTA_IMAP_ENABLED` (workers en route), volgens de feature-flags-skill. Frontend: `ota_imap`. |
| Zoho-connectie | Grant-custody in de auth-service (zelfde beveiligingsmodel als Microsoft); API-gebruik via `ota_zoho_client`. Een Zoho-grant hoort bij een gebruiker en een Zoho-account, niet bij één mailbox, zodat Calendar dezelfde grant met extra scopes kan gebruiken. |
| Credential-opslag | De envelope-cipher van de auth-service wordt generiek per doel; geen tweede crypto. |
| Migraties | Bestandsnamen `<timestamp>_ota_<onderwerp>.sql`, alleen additief (nieuwe tabellen, `ADD VALUE` aan een enum). Upstream-migraties worden nooit aangepast. |
| Self-hosting | `docs/ota/self-hosting.md`, gestart in deze fase: Postgres, Redis, OpenSearch, LiveKit, FusionAuth, AWS KMS (of een vervanger, zie open vraag 4), SQS/LocalStack, Zoho-client-ID/secret, AI-sleutels. |

## 5. Volgorde van werken (na akkoord)

1. Migraties: enumwaarde, `ota_imap_accounts`, `ota_zoho_oauth_grants`.
2. Generieke envelope-cipher en de Zoho-grant-flow in de auth-service,
   met tests.
3. `ota_imap`: verbinding, XOAUTH2/LOGIN, `folders`, `changes`, `message`, met
   tests tegen een lokale IMAP-server (GreenMail in Docker).
4. IMAP-gateway en workers in `email_service`, provider-parameter in de
   claim-queries, `sqlx::test` voor de claims.
5. Schrijfpad: commands, concepten, SMTP-verzending.
6. `ota_zoho_client`: accounts, aliassen, handtekeningen, labels.
7. Route `POST /email/links/imap`, OpenAPI, SDK-coverage, frontenddialoog.
8. IDLE-worker.
9. Docs: `AGENTS.md` van de geraakte mappen, `docs/ota/self-hosting.md`, agent
   guide.

Elke stap is een eigen commit; `just check` en de tests van de geraakte crates
draaien voor elke commit.

## 6. Open vragen voor jou

1. **CAPABILITY-uitvoer** (sectie 2). Bepaalt CONDSTORE/QRESYNC, OBJECTID en
   de SMTP-auth.
2. **Merge-vriendelijkheid**: een nieuwe provider raakt onvermijdelijk
   upstream-bestanden (enum, ~40 SQL-filters, workercompositie). Akkoord dat
   die aanpassingen klein en expliciet in upstream-bestanden komen, en al het
   overige in `ota_*`?
3. **Threading**: akkoord met threads op `References` in plaats van Zoho's
   `threadId`? De API-limiet maakt `threadId` per bericht onhaalbaar.
4. **KMS bij zelf hosten**: de bestaande cipher gebruikt AWS KMS. Host je op
   AWS, of moet er een lokale sleutelbron komen (een sleutel uit
   config/Doppler) als alternatieve `DataKeyProvider`?
5. **UI-taal**: de web-app heeft geen i18n-laag; alle teksten zijn Engels.
   Voorstel: nieuwe teksten in het Engels, zodat ze passen bij de rest.
   Nederlands vraagt eerst een i18n-laag voor de hele app, en dat valt buiten
   fase 1.
6. **Zoho-client**: maak je in de Zoho API Console (EU) een "Server-based
   Application" aan, met als redirect-URI de callback van je eigen
   auth-service? Ik heb de client-ID nodig als configwaarde; het secret gaat
   in je eigen secretbeheer, nooit in de chat.
