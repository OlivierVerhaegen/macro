# Phase 1 plan: Zoho Mail through IMAP/SMTP plus the Zoho API

Status: agreed and built. Branch: `ota/fase-1-zoho-mail`.

Phase 1 is developed and tested on the local stack
(`docs/development/running-locally.md`). Hosting on the VPS is
[phase 0](plan-phase-0.md).

## 1. Exploration

### Current structure (upstream, October 2026)

Outlook does not run its own sync loop but a provider-neutral mailbox runtime.
That is the template for IMAP.

| Layer | Where | What it does |
| --- | --- | --- |
| Domain port | `crates/email/src/domain/mailbox.rs` | `MailboxGateway` (`folders`, `changes`, `message`) plus `MailboxSyncService`: catalog, per-folder streams with an opaque `StreamToken`, and per-message reconciliation, all under leases with fences. |
| Other domain ports | `crates/email/src/domain/mailbox/{commands,drafts,watches,contacts,settings,credentials}.rs` | Read/flag/move (`MailboxCommandGateway`), drafts and sending (`DraftGateway`), notifications (`MailboxWatchGateway`), address book, settings, tokens (`MailboxCredentialGateway`). |
| Provider vocabulary | `crates/email_api_client/src/domain/{models/provider.rs,ports/mailbox.rs}` | `MailFolder`, `MailboxMessage`, `MailboxChangePage`, `StreamPosition`, `MessageAction`, and a port per capability (`MailboxContentReader`, `FolderChangeReader`, `MailboxActionWriter`, `MailboxDraftClient`, ...). |
| Graph adapter | `crates/email_api_client/src/outbound/outlook/` | Implements those ports for Microsoft Graph. |
| Glue in the service | `services/email_service/src/outbound/email_api/microsoft.rs` | `ProviderMailboxGateway` connects `MailboxApiService` to the domain ports; `MicrosoftCredentialsClient` gets tokens from the auth service. |
| Composition | `services/email_service/src/bin/pubsub_workers/pubsub_workers.rs`, `src/server.rs` | Starts the workers (sync, contacts, watches, commands, drafts, projections) with the Outlook gateway, behind `outlook_sync_enabled` / `outlook_writes_enabled`. |
| OAuth custody | `services/authentication_service/src/{domain,outbound}/microsoft*`, `crates/microsoft_oauth_grant_db_utils` | PKCE linking, refresh tokens encrypted with an AES-256-GCM envelope and AWS KMS data keys, generation-bound tokens. |
| Storage | `crates/email/src/outbound/mailbox_pg/` | Leases and checkpoints in Postgres. Ingest threads on `provider_thread_id`. |

Three findings shape the plan:

1. **The runtime is generic, the SQL is not.** About 40 queries filter hard on
   `l.provider = 'OUTLOOK'` (claims in `mailbox_pg/*`, parts of
   `email_pg_repo/*`, `email_db_client/links`, `mailbox_init.rs`). A new
   provider therefore needs changes in upstream files; it cannot be kept
   entirely in crates of our own.
2. **One gateway per worker.** The sync workers claim work across all
   mailboxes and get one gateway. IMAP therefore gets worker instances of its
   own with their own claims (filtered on provider), instead of a dispatcher in
   the Outlook gateway.
3. **Threading happens at ingest, on `provider_thread_id`.** For IMAP it must be
   known when fetching; the Zoho API (30 calls a minute) cannot provide it per
   message.

### The IMAP branch (`claude/imap-smtp-support-hvt8e3`, upstream PR #3919)

Built on the old layout (`rust/cloud-storage/`, `js/app/`) and the old
Gmail-shaped poll loop. Assessment per part:

| Part | Verdict |
| --- | --- |
| `imap_smtp_client` (`imap.rs`, `smtp.rs`, `tls.rs`; `async-imap`, `mail-send`, rustls) | **Take over** as the base of an `imap_client` adapter, but rewrite it to the new ports and extend it with XOAUTH2, CONDSTORE/QRESYNC, IDLE, APPEND and MOVE. |
| `convert/imap.rs` (MIME through `mailparse` to the service model, plus tests) | **Take over**, converted to `MailboxMessage`. The synthetic Gmail labels go: the new runtime works with `FolderRole` and `MailboxState`. |
| Threading through `References`/`In-Reply-To` | **Take over as the rule**: `provider_thread_id` = the first Message-ID in `References`, else `In-Reply-To`, else the message's own Message-ID. |
| `email_utils::credential_crypto` (its own AES-GCM with a static key) | **Do not take over.** The auth service already has an envelope cipher with KMS (CS-26: no second crypto path). That one is made more general. |
| Table `email_imap_folder_states`, operation `ImapPoll`, `imap_modify_labels`, changes in `scheduled`/`link_manager` | **Dropped.** The runtime already has streams, checkpoints, commands and leases. |
| `POST /email/links/imap` and the link dialog | **Take over the idea, not the code**: a new route next to the existing link routes, the dialog in `apps/web/src/features/inbox/`. |

## 2. CAPABILITY check

I cannot run this from the cloud session: the container cannot resolve
`imap.zoho.eu` (`Name or service not known`). Run it locally. Use a Zoho **app
password**, and afterwards paste only the lines starting with `* CAPABILITY`,
`* LIST` or `* ENABLED` (never the `LOGIN` line).

```sh
openssl s_client -quiet -crlf -connect imap.zoho.eu:993 -servername imap.zoho.eu
a1 CAPABILITY
a2 LOGIN you@example.be APP-PASSWORD
a3 CAPABILITY
a4 ENABLE QRESYNC
a5 LIST (SPECIAL-USE) "" "*"
a6 LIST "" "*"
a7 LOGOUT
```

And for SMTP (look for `AUTH ... XOAUTH2` in the `EHLO` answers):

```sh
openssl s_client -quiet -crlf -connect smtp.zoho.eu:465 -servername smtp.zoho.eu
EHLO test
QUIT
```

What I look for in the output, and what it changes:

| Capability | If present | If absent |
| --- | --- | --- |
| `CONDSTORE` + `QRESYNC` | Incremental sync through `HIGHESTMODSEQ` and `VANISHED`. | Follow `UIDNEXT` per folder for new mail and compare flags periodically for changes. |
| `IDLE` | Push for the inbox. | Polling only. |
| `MOVE`, `UIDPLUS` | Moves and new UIDs without extra searching. | COPY+EXPUNGE, and searching by Message-ID again after APPEND. |
| `SPECIAL-USE` | Reliable folder roles (Sent, Drafts, Trash, Junk, Archive). | Roles from a fixed list of Zoho names. |
| `OBJECTID` | A stable message ID across moves. | ID = folder + UIDVALIDITY + UID; a move becomes "gone and new". |
| `AUTH=XOAUTH2` (IMAP and SMTP) | OAuth for both. | Decides the fallback for sending (see 3.2). |

The `LIST` output also answers whether Zoho labels appear as IMAP folders or as
keywords.

## 3. Phase 1 design

### 3.1 Provider

- A new enum value `IMAP` in `email_user_provider_enum` and in the three Rust
  `UserProvider` enums (`crates/email`, `models_email` api and service), plus
  `graphql_email` and the generated frontend types.
- Per link a row in a new table `imap_accounts`: host, port, SMTP host,
  security, username, auth method (`zoho_oauth` or `password`), data center,
  `zoho_account_id` (for API calls). Zoho is a preset that fills these fields.
- The data center as an enum `ZohoDataCenter { Com, Eu, In, ComAu, Jp, Ca, Sa,
  Cn }` with the accounts, mail API, IMAP and SMTP hosts per value. These are
  provider endpoints a user chooses, not Macro service URLs, so they do not
  belong in `macro_service_urls`.
- The `'OUTLOOK'` filters in the mailbox runtime become
  `l.provider IN ('OUTLOOK', 'IMAP')` where the query is about the runtime, and
  stay `'OUTLOOK'` where it is about Graph-specific behavior. Every query gets a
  decision of its own; no global search and replace.
- IMAP workers claim only IMAP links (the provider as a parameter of the claim
  queries), so Outlook and IMAP workers never take each other's work.

### 3.2 Authentication

- **Zoho OAuth** in the auth service, following the Microsoft pattern: a server
  client (not a Self Client), PKCE, a signed state, and refresh tokens in a new
  table `zoho_oauth_grants`, encrypted with the same envelope cipher. That
  cipher becomes generic: the purpose (`microsoft-refresh-token`,
  `zoho-refresh-token`, `imap-password`) goes into the key context and the AAD.
  The Microsoft code keeps its behavior.
- **Local key source** (decided: hosting on our own VPS, no AWS): a second
  `DataKeyProvider` next to the KMS one. Per encryption it derives a data key
  from a 32-byte master key in the configuration
  (`LOCAL_TOKEN_ENCRYPTION_KEY`, base64), through HKDF-SHA256 with a random salt
  and the key context as `info`. The salt is stored where KMS stores the
  encrypted data key; `kms_key_id` gets a fixed value such as `local:v1`, so the
  key can be rotated later. The configuration decides which source is used. If
  the key is missing while the local source is chosen, the service stops at
  startup.
- Scopes in phase 1: `ZOHOMAIL.PROTOCOL.IMAP`, `ZohoMail.accounts.READ`,
  `ZohoMail.messages.ALL` and `ZohoMail.folders.ALL`. Scopes are stored per
  grant and can be extended; phase 6 adds `ZohoCalendar.*` through a relink.
- The token for IMAP/SMTP goes through a new `ImapCredentialGateway` that
  follows `MailboxCredentialGateway` and calls the auth service
  (`get_zoho_access_token`, next to `get_microsoft_access_token`).
- **Sending: decided (option 1).** Zoho SMTP offers no XOAUTH2, so IMAP reads
  with OAuth and SMTP logs in with a Zoho app password. The user sets it through
  `PUT /email/links/{link_id}/smtp_password`; email_service first tests it with
  an SMTP login, then the auth service stores it as an envelope (purpose
  `smtp-password`, table `smtp_passwords`). The password is never logged and
  never sent back to the frontend. Without a password, sending refuses with a
  clear message.
- Password login for generic IMAP (not Zoho) follows later; the schema
  (`imap_accounts.auth_method`) already provides for it.

### 3.3 Sync (`MailboxGateway` for IMAP)

- **`folders`**: `LIST` with `SPECIAL-USE`; roles from those flags, else a list
  of Zoho names.
- **`changes`**, per folder: `StreamToken` = `UIDVALIDITY:HIGHESTMODSEQ:UIDNEXT`
  (without modseq when CONDSTORE is missing). A different UIDVALIDITY gives
  `EmailApiError::OutdatedCursor`, so the runtime resets the stream. The first
  round goes in pages of a fixed size (`Continue`), then `Checkpoint`. With
  QRESYNC deletions come from `VANISHED`; without QRESYNC from a periodic
  `UID SEARCH ALL` comparison.
- **`message`**: `UID FETCH` of `BODY.PEEK[]`, `FLAGS`, `INTERNALDATE` and
  `MODSEQ`, then MIME conversion (taken from the branch) to `MailboxMessage`.
  `version` = `MODSEQ`, or the flags hash without modseq.
- **Message ID** = `OBJECTID` if Zoho supports it, else
  `<folder>/<UIDVALIDITY>/<UID>`.
- **Threading**: see the rule in the table above. Zoho's own `threadId` does not
  fit there, because the API is too slow per message (30 calls a minute). My
  proposal is to drop that step; see question 3.
- **IDLE (built)**: a small worker with one IDLE connection per active IMAP
  link, on INBOX only. On `EXISTS`, `EXPUNGE` or `FETCH` it marks the inbox
  stream "due", as a Graph webhook does. The periodic poll catches other folders
  and missed events.
- **Zoho limits**: at most one IDLE connection plus a small pool (3 by default)
  per account, far below the limit of 100. A per-link fetch budget in Redis
  follows the pattern of `RedisMicrosoftRequestGate`, set to 1 GB per 15
  minutes. Large attachments are fetched only when opened (`BODYSTRUCTURE`
  first, the body parts on demand through `MailboxContentReader::attachment`).

### 3.4 Writing

| Action | IMAP |
| --- | --- |
| Read/unread | `UID STORE +/-FLAGS (\Seen)` |
| Star | `\Flagged` |
| Move, archive, trash | `UID MOVE` (or COPY+EXPUNGE) to the folder with the right role |
| Save a draft | Decided: drafts stay in Macro and do not go to Zoho's Drafts folder |
| Send | SMTP (`smtppro.zoho.eu:465` for an own domain, app password); then the copy in Sent is looked up by Message-ID, or archived with `APPEND` otherwise |
| Labels/tags | Built: IMAP keywords (Zoho keeps custom keywords). Label names may not contain spaces or `( ) { % * " \ ]`. Zoho's own webmail labels are something else and are not synchronized. |

Sending follows the existing `SubmissionOutcome` rules: a timeout after the
SMTP `DATA` command gives `Unknown`, never a blind retry. The draft carries the
`draft_correlation` in a header of its own, so the sent copy can be found, as
with Outlook.

### 3.5 Zoho Mail API (a supplement, not the sync)

A crate of its own, `zoho_client`, with the data center choice, a token provider
through the auth service and a rate limiter (30 calls a minute per account).
Used in phase 1 for:

- account details: `accountId`, primary address, aliases and send addresses
  (`GET /accounts`), when linking and daily afterwards (built: stored in
  `imap_accounts.send_addresses` when linking and offered as senders);
- signatures (`GET /accounts/{id}/signatures`), at the same moments;
- reading and setting labels, if labels are not available through IMAP.

Phase 6 reuses the crate for Calendar: only new scopes and a new adapter.

### 3.6 Turning off Gmail-specific parts

Filters, contact sync through Google and send-as through the Gmail API are
skipped for `IMAP`. The settings UI shows a notice ("Not available for IMAP
accounts"). For each of the ~30 files that match on `UserProvider::Outlook`, I
check what the `IMAP` branch must do. Without a wildcard arm, so the compiler
enforces every place.

### 3.7 Frontend

- **Built.** The "Connect email" dialog offers "Zoho Mail" with a data center
  choice (Europe preselected). Code in `apps/web/src/lib/core/email-link/`
  (`zoho.ts`, the dialog and the link flow) and
  `src/lib/queries/auth/zoho-link.ts`.
- Settings → Email gives a Zoho inbox the field "App password for sending"
  (`features/settings/SmtpPasswordForm.tsx`).
- No frontend flag of its own: the button appears only when the auth service
  has Zoho configured (`zoho` in `/link/email/providers`), as with Outlook. So
  there is one source of truth.
- "Other IMAP server" (host, port, password) follows later.

### 3.8 MCP tools

The existing email tools (`crates/email/src/inbound/toolset.rs`) work through
the domain service and are provider-neutral. Phase 1 therefore needs no new
tools. I check every tool for Gmail-only paths and let an IMAP inbox use the
same tools. I do not add a "link IMAP account" tool for the agent: linking needs
a browser OAuth flow and a secret.

### 3.9 Observability

- Spans `imap.sync_folder`, `imap.fetch_message`, `imap.idle_session`,
  `smtp.submit` and `zoho_api.request`, with `link_id`, folder role, counts,
  bytes and outcome.
- Never email addresses, subjects or tokens in the spans.

## 4. Shared foundations

| Foundation | Choice |
| --- | --- |
| Naming | Plain names for what something is (`imap_client`, `zoho_client`, `zoho_oauth_grants`, `imap_accounts`, `features/zoho-mail/`). The `ota` prefix only for fork documentation (`docs/ota/`) and branches (`ota/fase-N-...`). New code lives in crates, modules and files of its own where possible. |
| Feature flag | Backend: the config switch `IMAP_MAILBOXES_ENABLED` (workers and route), following the feature-flags skill. Frontend: no flag of its own; the Zoho option follows `zoho` in `/link/email/providers`. |
| Zoho connection | Grant custody in the auth service (the same security model as Microsoft); API use through `zoho_client`. A Zoho grant belongs to a user and a Zoho account, not to one mailbox, so Calendar can use the same grant with extra scopes. |
| Credential storage | The auth service's envelope cipher becomes generic per purpose, with a local key source next to KMS; no second crypto. |
| Migrations | Additive only (new tables, `ADD VALUE` to an enum). Upstream migrations are never changed. |
| Self-hosting | `docs/ota/self-hosting.md` (phase 0) for our own VPS: Postgres, Redis, OpenSearch, LiveKit, FusionAuth, the local key, S3 and SQS replacements, the Zoho client ID and secret, AI keys. |

## 5. Work order

1. Migrations: the enum value, `imap_accounts`, `zoho_oauth_grants`.
2. The generic envelope cipher with the local key source, and the Zoho grant
   flow in the auth service, with tests.
3. `imap_client`: connection, XOAUTH2/LOGIN, `folders`, `changes`, `message`,
   with tests against a local IMAP server (GreenMail in Docker).
4. The IMAP gateway and workers in `email_service`, the provider parameter in
   the claim queries, `sqlx::test` for the claims.
5. The write path: commands, drafts, SMTP sending.
6. `zoho_client`: accounts, aliases, signatures (built: "Import from Zoho"),
   labels (built: as IMAP keywords, because Zoho reports
   `PERMANENTFLAGS (\*)`; names without spaces).
7. The route `POST /email/links/imap`, OpenAPI, SDK coverage, the frontend
   dialog.
8. The IDLE worker.
9. Docs: the `AGENTS.md` of the touched directories, `docs/ota/self-hosting.md`,
   the agent guide.

Every step is a commit of its own; `just check` and the tests of the touched
crates run before every commit.

## 6. Questions for you

1. **Verified on `imap.zoho.eu` (23 October 2026).** Zoho reports
   `IMAP4rev1 UNSELECT CHILDREN XLIST NAMESPACE IDLE MOVE ID AUTH=PLAIN SASL-IR
   AUTH=XOAUTH2 UIDPLUS ESEARCH LIST-EXTENDED LIST-STATUS WITHIN LITERAL- ACL
   CONDSTORE`, the same before and after login. Consequences:
   - CONDSTORE, IDLE, MOVE, UIDPLUS and XOAUTH2 (IMAP) are there; the design
     holds.
   - No QRESYNC (`ENABLE QRESYNC` answers OK without `* ENABLED`): deletions
     come from comparing UIDs, as built.
   - SPECIAL-USE is not in CAPABILITY, but `LIST` does give the flags
     (`\Drafts` "Concept", `\Sent` "Verzonden", `\Junk` "Spam", `\Trash`
     "Afval"). The names are localized, so the roles come from the flags. There
     is no archive folder.
   - No OBJECTID: message IDs stay `folder/UIDVALIDITY/UID`.
   - According to the Zoho docs (IMAP and SMTP settings), paid organizations on
     their own domain use `imappro`/`smtppro`, and personal `@zoho`/`@zohomail`
     addresses and free organizations use `imap`/`smtp`. When linking, the user
     picks the servers (Automatic by default: `imappro`/`smtppro` for an own
     domain), with a link to the Zoho docs. The test above ran on
     `imap.zoho.eu`; the right servers are listed in Zoho under Settings → Mail
     accounts → Server configuration.
   - Signatures (`GET /api/accounts/signature`, scope `ZohoMail.accounts.READ`)
     are verified against the docs. Images in them point to
     `/zm/ImageSignature?...` behind the Zoho login and cannot come along; the
     import then asks to add them in Macro.
   - The IMAP and SMTP login is the mailbox address (`mailboxAddress`, e.g.
     `tech@`), not the primary address (`contact@`, an alias). It is now stored
     with the grant.
   - SMTP on `smtp.zoho.eu:465` offers only `AUTH LOGIN PLAIN` (no XOAUTH2),
     with `SIZE 53477376`.
2. **Agreed.** Merge-friendliness: a new provider inevitably touches upstream
   files (the enum, ~40 SQL filters, worker composition). Agreed that those
   changes go into upstream files small and explicit, and everything else into
   crates and files of our own.
3. **Agreed.** Threading: threads on `References` instead of Zoho's `threadId`.
   The API limit makes `threadId` per message infeasible.
4. ~~KMS when self-hosting~~ **Decided**: our own VPS, so a local key source
   (section 3.2).
5. **Agreed.** UI language: the web app has no i18n layer; every text is
   English. New texts are in English, so they match the rest. Dutch would need
   an i18n layer for the whole app first, which is outside phase 1.
6. **Zoho client**: create a "Server-based Application" in the Zoho API Console
   (EU), with the callback of your own auth service as the redirect URI. I need
   the client ID as a config value; the secret goes into your own secret store,
   never into the chat.

## Zoho limits (rates and limits, checked 24 October 2026)

| Limit | Value | Consequence for Macro |
| --- | --- | --- |
| IMAP connections | 100 per account | Sync, commands, IDLE and sending open short connections plus one IDLE connection; far below it. |
| IMAP fetch | 1 GB per 15 minutes (organization), 250 MB (personal) | The first sync of a large mailbox can hit this; Zoho then blocks for 15 minutes and the runtime retries later. A per-link fetch budget is not built yet. |
| API | 30 requests a minute, then an undisclosed block period | Macro uses the API only when linking (accounts) and for the signature import. Sending through the API would count per message. |
| Sending, external | 50-500 an hour, dynamic on reputation | Then an hour without sending; SMTP refuses with a message. |
| Sending, internal | 1,000 an hour | |
| Recipients per message | 150 external (paid), 100 (free), 500 internal | The server refuses more recipients; the send then fails for good. |
| Incoming message | 40 MB | |
