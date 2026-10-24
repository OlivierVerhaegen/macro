# Plan fase 1: Zoho Mail via IMAP/SMTP plus Zoho API

Status: voorstel, wacht op akkoord. Branch: `ota/fase-1-zoho-mail`.

Fase 1 wordt ontwikkeld en getest op de lokale stack
(`docs/development/running-locally.md`). Hosting op de VPS is
[fase 0](plan-fase-0.md).

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
| `imap_smtp_client` (`imap.rs`, `smtp.rs`, `tls.rs`; `async-imap`, `mail-send`, rustls) | **Overnemen** als basis van een `imap_client`-adapter, maar herschrijven naar de nieuwe ports en uitbreiden met XOAUTH2, CONDSTORE/QRESYNC, IDLE, APPEND en MOVE. |
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
- Per link een rij in een nieuwe tabel `imap_accounts`: host, poort,
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
  refresh-tokens in een nieuwe tabel `zoho_oauth_grants`, versleuteld
  met dezelfde envelope-cipher. Die cipher wordt generiek: het doel
  (`microsoft-refresh-token`, `zoho-refresh-token`, `imap-password`) gaat mee in
  de sleutelcontext en de AAD. De Microsoft-code verandert niet van gedrag.
- **Lokale sleutelbron** (besloten: hosting op een eigen VPS, geen AWS): een
  tweede `DataKeyProvider` naast de KMS-variant. Die leidt per versleuteling
  een datasleutel af uit een hoofdsleutel van 32 bytes uit de configuratie
  (`LOCAL_TOKEN_ENCRYPTION_KEY`, base64), via HKDF-SHA256 met een willekeurige
  salt en de sleutelcontext als `info`. De salt wordt bewaard op de plek waar
  KMS de versleutelde datasleutel bewaart; `kms_key_id` krijgt een vaste
  waarde zoals `local:v1`, zodat sleutelrotatie later mogelijk is. Welke bron
  gebruikt wordt, volgt uit de configuratie. Ontbreekt de sleutel terwijl de
  lokale bron gekozen is, dan stopt de service bij het opstarten.
- Scopes in fase 1: `ZOHOMAIL.PROTOCOL.IMAP`, `ZohoMail.accounts.READ`,
  `ZohoMail.messages.ALL` en `ZohoMail.folders.ALL`. Scopes zijn per grant opgeslagen en uitbreidbaar;
  fase 6 voegt `ZohoCalendar.*` toe via een herkoppeling.
- Het token voor IMAP/SMTP gaat via een nieuwe `ImapCredentialGateway` die
  `MailboxCredentialGateway` volgt en de auth-service aanroept
  (`get_zoho_access_token`, naast `get_microsoft_access_token`).
- **Verzenden: besloten (optie 1).** Zoho-SMTP biedt geen XOAUTH2, dus IMAP
  leest met OAuth en SMTP logt in met een Zoho-app-wachtwoord. De gebruiker
  zet het via `PUT /email/links/{link_id}/smtp_password`; email_service test
  het eerst met een SMTP-login, daarna bewaart de auth-service het als
  envelope (doel `smtp-password`, tabel `smtp_passwords`). Het wachtwoord
  wordt nooit gelogd en nooit naar de frontend teruggestuurd. Zonder
  wachtwoord weigert verzenden met een duidelijke melding.
- Wachtwoord-login voor generieke IMAP (niet-Zoho) volgt later; het schema
  (`imap_accounts.auth_method`) voorziet het al.

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
- **IDLE (gebouwd)**: een kleine worker met één IDLE-verbinding per actieve IMAP-link,
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
| Concept opslaan | Besloten: concepten blijven in Macro en gaan niet naar de map Concept van Zoho |
| Verzenden | SMTP (`smtppro.zoho.eu:465` voor een eigen domein, app-wachtwoord); daarna wordt de kopie in Verzonden op Message-ID gezocht, en anders zelf met `APPEND` gearchiveerd |
| Labels/tags | Gebouwd: IMAP-keywords (Zoho bewaart eigen keywords). Labelnamen mogen geen spaties of `( ) { % * " \ ]` bevatten. Zoho's eigen webmail-labels zijn iets anders en worden niet gesynchroniseerd. |

Verzenden volgt de bestaande `SubmissionOutcome`-regels: een time-out na het
SMTP-`DATA`-commando geeft `Unknown`, nooit een blinde herhaling. Het
concept draagt de `draft_correlation` mee in een eigen header, zodat de
verzonden kopie terug te vinden is, zoals bij Outlook.

### 3.5 Zoho Mail API (aanvulling, niet de sync)

Een eigen crate `zoho_client` met de datacenterkeuze, een token-provider
via de auth-service en een rate-limiter (30 calls/minuut per account). In
fase 1 gebruikt voor:

- accountgegevens: `accountId`, primair adres, aliassen en verzendadressen (gebouwd: bij het koppelen bewaard in `imap_accounts.send_addresses` en aangeboden als afzenders)
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

- **Gebouwd.** De dialoog "Connect email" biedt "Zoho Mail" met een
  datacenterkeuze (Europa voorgeselecteerd). Code in
  `apps/web/src/lib/core/email-link/` (`zoho.ts`, de dialoog en de
  koppelflow) en `src/lib/queries/auth/zoho-link.ts`.
- In Instellingen → Email krijgt een Zoho-inbox het veld "App password for
  sending" (`features/settings/SmtpPasswordForm.tsx`).
- Geen aparte frontend-flag: de knop verschijnt alleen als de auth-service
  Zoho geconfigureerd heeft (`zoho` in `/link/email/providers`), net zoals
  Outlook. Zo is er één bron van waarheid.
- "Andere IMAP-server" (host, poort, wachtwoord) volgt later.

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
| Naamgeving | Gewone namen voor wat iets is (`imap_client`, `zoho_client`, `zoho_oauth_grants`, `imap_accounts`, `features/zoho-mail/`). Het prefix `ota` alleen voor fork-documentatie (`docs/ota/`) en branches (`ota/fase-N-...`). Nieuwe code staat zoveel mogelijk in eigen crates, modules en bestanden. |
| Feature flag | Backend: config-schakelaar `IMAP_MAILBOXES_ENABLED` (workers en route), volgens de feature-flags-skill. Frontend: geen eigen flag; de Zoho-optie volgt `zoho` uit `/link/email/providers`. |
| Zoho-connectie | Grant-custody in de auth-service (zelfde beveiligingsmodel als Microsoft); API-gebruik via `zoho_client`. Een Zoho-grant hoort bij een gebruiker en een Zoho-account, niet bij één mailbox, zodat Calendar dezelfde grant met extra scopes kan gebruiken. |
| Credential-opslag | De envelope-cipher van de auth-service wordt generiek per doel, met een lokale sleutelbron naast KMS; geen tweede crypto. |
| Migraties | Alleen additief (nieuwe tabellen, `ADD VALUE` aan een enum). Upstream-migraties worden nooit aangepast. |
| Self-hosting | `docs/ota/self-hosting.md`, gestart in deze fase, voor een eigen VPS: Postgres, Redis, OpenSearch, LiveKit, FusionAuth, de lokale sleutel, S3- en SQS-vervangers, Zoho-client-ID/secret, AI-sleutels. |

## 5. Volgorde van werken (na akkoord)

1. Migraties: enumwaarde, `imap_accounts`, `zoho_oauth_grants`.
2. Generieke envelope-cipher met de lokale sleutelbron, en de
   Zoho-grant-flow in de auth-service, met tests.
3. `imap_client`: verbinding, XOAUTH2/LOGIN, `folders`, `changes`, `message`, met
   tests tegen een lokale IMAP-server (GreenMail in Docker).
4. IMAP-gateway en workers in `email_service`, provider-parameter in de
   claim-queries, `sqlx::test` voor de claims.
5. Schrijfpad: commands, concepten, SMTP-verzending.
6. `zoho_client`: accounts, aliassen, handtekeningen (gebouwd: "Import from Zoho"), labels (gebouwd: als IMAP-keywords, want Zoho geeft `PERMANENTFLAGS (\*)`; namen zonder spaties).
7. Route `POST /email/links/imap`, OpenAPI, SDK-coverage, frontenddialoog.
8. IDLE-worker.
9. Docs: `AGENTS.md` van de geraakte mappen, `docs/ota/self-hosting.md`, agent
   guide.

Elke stap is een eigen commit; `just check` en de tests van de geraakte crates
draaien voor elke commit.

## 6. Open vragen voor jou

1. **Geverifieerd op `imap.zoho.eu` (23 oktober 2026).** Zoho meldt
   `IMAP4rev1 UNSELECT CHILDREN XLIST NAMESPACE IDLE MOVE ID AUTH=PLAIN SASL-IR
   AUTH=XOAUTH2 UIDPLUS ESEARCH LIST-EXTENDED LIST-STATUS WITHIN LITERAL- ACL
   CONDSTORE`, voor en na het inloggen gelijk. Gevolgen:
   - CONDSTORE, IDLE, MOVE, UIDPLUS en XOAUTH2 (IMAP) zijn er; het ontwerp
     klopt.
   - Geen QRESYNC (`ENABLE QRESYNC` antwoordt OK zonder `* ENABLED`):
     verwijderingen komen uit de vergelijking van UID's, zoals gebouwd.
   - SPECIAL-USE staat niet in CAPABILITY, maar `LIST` geeft de vlaggen wel
     (`\Drafts` Concept, `\Sent` Verzonden, `\Junk` Spam, `\Trash` Afval).
     De namen zijn gelokaliseerd, dus de rollen komen uit de vlaggen. Er is
     geen archiefmap.
   - Geen OBJECTID: bericht-ID's blijven `map/UIDVALIDITY/UID`.
   - Volgens de Zoho-docs (IMAP- en SMTP-instellingen) gebruiken betaalde
     organisaties met een eigen domein `imappro`/`smtppro`, en persoonlijke
     `@zoho`/`@zohomail`-adressen en gratis organisaties `imap`/`smtp`.
     Bij het koppelen kiest de gebruiker de servers (standaard Automatisch:
     `imappro`/`smtppro` voor een eigen domein), met een link naar de Zoho-docs. De test hierboven liep op `imap.zoho.eu`;
     de juiste servers staan in Zoho onder Instellingen → Mail-accounts →
     Serverconfiguratie.
   - Handtekeningen (`GET /api/accounts/signature`, scope
     `ZohoMail.accounts.READ`) zijn geverifieerd tegen de docs. Afbeeldingen
     erin verwijzen naar `/zm/ImageSignature?...` achter Zoho-login en kunnen
     niet mee; de import vraagt dan om ze in Macro toe te voegen.
   - De IMAP- en SMTP-login is het mailboxadres (`mailboxAddress`, bv. `tech@`), niet het primaire adres (`contact@`, een alias). Dat wordt nu bij de grant bewaard.
   - SMTP op `smtp.zoho.eu:465` biedt alleen `AUTH LOGIN PLAIN` (geen
     XOAUTH2), met `SIZE 53477376`.
2. **Akkoord.** Merge-vriendelijkheid: een nieuwe provider raakt onvermijdelijk
   upstream-bestanden (enum, ~40 SQL-filters, workercompositie). Akkoord dat
   die aanpassingen klein en expliciet in upstream-bestanden komen, en al het
   overige in eigen crates en bestanden?
3. **Akkoord.** Threading: threads op `References` in plaats van Zoho's
   `threadId`? De API-limiet maakt `threadId` per bericht onhaalbaar.
4. ~~KMS bij zelf hosten~~ **Besloten**: eigen VPS, dus een lokale
   sleutelbron (sectie 3.2).
5. **Akkoord.** UI-taal: de web-app heeft geen i18n-laag; alle teksten zijn Engels.
   Voorstel: nieuwe teksten in het Engels, zodat ze passen bij de rest.
   Nederlands vraagt eerst een i18n-laag voor de hele app, en dat valt buiten
   fase 1.
6. **Zoho-client**: maak je in de Zoho API Console (EU) een "Server-based
   Application" aan, met als redirect-URI de callback van je eigen
   auth-service? Ik heb de client-ID nodig als configwaarde; het secret gaat
   in je eigen secretbeheer, nooit in de chat.

## Zoho-limieten (rates-and-limits, gecontroleerd 24 oktober 2026)

| Limiet | Waarde | Gevolg voor Macro |
| --- | --- | --- |
| IMAP-verbindingen | 100 per account | Sync, commands, IDLE en verzenden openen korte verbindingen plus één IDLE-verbinding; ruim eronder. |
| IMAP-fetch | 1 GB per 15 minuten (organisatie), 250 MB (persoonlijk) | De eerste sync van een grote mailbox kan dit raken; Zoho blokkeert dan 15 minuten en de runtime probeert later opnieuw. Een fetch-budget per link is nog niet gebouwd. |
| API | 30 verzoeken per minuut, daarna een geheime blokkeerperiode | Macro gebruikt de API alleen bij koppelen (accounts) en voor de handtekening-import. Verzenden via de API zou per bericht meetellen. |
| Verzenden extern | 50-500 per uur, dynamisch op reputatie | Daarna een uur geen verzending; SMTP weigert met een melding. |
| Verzenden intern | 1.000 per uur | |
| Ontvangers per bericht | 150 extern (betaald), 100 (gratis), 500 intern | Meer ontvangers weigert de server; de verzending faalt dan definitief. |
| Inkomend bericht | 40 MB | |

