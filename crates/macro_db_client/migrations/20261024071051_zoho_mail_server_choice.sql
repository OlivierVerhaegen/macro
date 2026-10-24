-- Olive Trees: which Zoho IMAP/SMTP servers a connection uses. Zoho serves
-- paid organizations from imappro/smtppro and personal and free accounts from
-- imap/smtp; the address alone cannot tell a free organization apart, so the
-- user may choose when connecting. 'auto' picks pro for a domain address.
SET LOCAL lock_timeout = '1s';
SET LOCAL statement_timeout = '5s';

ALTER TABLE zoho_link_attempts ADD COLUMN mail_servers text NOT NULL DEFAULT 'auto'
    CHECK (mail_servers IN ('auto','pro','standard'));
