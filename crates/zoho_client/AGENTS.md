# zoho_client

## What it is

Olive Trees fork, Zoho Mail support: the Zoho OAuth server and Zoho Mail REST
API, resolved per Zoho data center. Used to link a Zoho mailbox and to reach
the Mail API beside IMAP.

## Start here

- `src/data_center.rs`: `ZohoDataCenter` and its hosts; `ZohoEndpoints` is what
  every call takes, so tests point it at `wiremock`.
- `src/oauth.rs`: consent URL, code exchange, refresh, and the scope names.
- `src/accounts.rs`: `GET /api/accounts`, the identity behind a grant.

## Scope

- Owns: Zoho wire formats and error mapping.
- Not here: grant storage, encryption and refresh serialization →
  `services/authentication_service` (`zoho`); IMAP → `crates/imap_client`.

## Checks

- `cargo test -p zoho_client`

## Gotchas

- Zoho's token endpoint reports grant errors with HTTP 200 and an `error`
  field; `invalid_code` also means a revoked refresh token.
- Zoho documents XOAUTH2 scopes for IMAP and POP only, not SMTP; Zoho mailboxes
  send through the Mail API.
- The Mail API allows about 30 calls per minute per account: never call it per
  synced message.
- Some ids arrive as JSON numbers and others as strings; keep `ZohoId`.
