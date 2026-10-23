-- Olive Trees: the addresses a Zoho Mail account may send as (its aliases),
-- read at consent and kept with the grant, then copied to the inbox it binds.
SET LOCAL lock_timeout = '1s';
SET LOCAL statement_timeout = '5s';

ALTER TABLE zoho_oauth_grants ADD COLUMN send_addresses text[] NOT NULL DEFAULT '{}';
ALTER TABLE imap_accounts ADD COLUMN send_addresses text[] NOT NULL DEFAULT '{}';
