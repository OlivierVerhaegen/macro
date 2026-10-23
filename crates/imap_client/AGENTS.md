# imap_client

## What it is

Email feature, IMAP mailboxes (Zoho Mail and other IMAP servers): an IMAP
session that answers in the provider-neutral mailbox vocabulary of
`email_api_client`, so the mailbox runtime syncs IMAP like Outlook.

## Start here

- `src/connection.rs`: implicit TLS, XOAUTH2 or `LOGIN`, CONDSTORE detection.
- `src/sync.rs`: folder streams (first enumeration newest first, then
  CONDSTORE `CHANGEDSINCE`), message and attachment fetches.
- `src/convert.rs`: RFC 5322 to `MailboxMessage`, threading, MIME parts.
- `src/idle.rs`: IDLE waits that report only message changes, not keepalives.

## Scope

- Owns: IMAP protocol use and message conversion.
- Not here: credentials, server settings and storage → `services/email_service`
  (IMAP gateway); Zoho OAuth and Mail API → `crates/zoho_client`.

## Checks

- `cargo test -p imap_client`

## Gotchas

- Message ids encode UIDVALIDITY, UID and folder; a move or a recreated
  folder yields a new id, and a changed UIDVALIDITY is `OutdatedCursor`.
- `async-imap` does not parse QRESYNC `VANISHED`; `changes` never reports
  removals. Compare `folder_uids` with stored ids when `exists` drops.
- Threads key on the first `References` id, else `In-Reply-To`, else the
  message's own `Message-ID`; keep that stable or threads split.
- Attachment ids are IMAP part paths (`2`, `1.2`) into the raw message.
