-- Olive Trees: a Zoho account's mailbox address (its IMAP and SMTP login) can
-- differ from its primary address, which may be an alias. Grants made before
-- this column logged in with the primary address.
SET LOCAL lock_timeout = '1s';
SET LOCAL statement_timeout = '5s';

ALTER TABLE zoho_oauth_grants ADD COLUMN mailbox_address text
    CHECK (mailbox_address IS NULL OR (mailbox_address <> '' AND mailbox_address = lower(mailbox_address)));
