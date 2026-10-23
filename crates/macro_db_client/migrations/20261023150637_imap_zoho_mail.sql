-- Olive Trees: IMAP mailboxes, with Zoho Mail OAuth as the preset.
-- Additive only: existing Gmail and Outlook readers and writers are unaffected.
SET LOCAL lock_timeout = '1s';
SET LOCAL statement_timeout = '5s';

ALTER TYPE email_user_provider_enum ADD VALUE IF NOT EXISTS 'IMAP';

-- One version per (owner, Zoho user) reconnect, like microsoft_oauth_grant_versions.
-- A grant belongs to a Zoho user, not to one mailbox, so later Zoho services
-- (calendar, contacts) extend its scopes instead of creating a second grant.
CREATE TABLE zoho_oauth_grants (
    grant_id uuid PRIMARY KEY,
    fusionauth_user_id text NOT NULL CHECK (fusionauth_user_id <> ''),
    email_address text NOT NULL CHECK (email_address <> '' AND email_address = lower(email_address)),
    data_center text NOT NULL CHECK (data_center IN ('com','eu','in','com.au','jp','ca','sa','com.cn')),
    zoho_user_id text NOT NULL CHECK (zoho_user_id <> ''),
    zoho_account_id text NOT NULL CHECK (zoho_account_id <> ''),
    generation bigint NOT NULL CHECK (generation > 0),
    revision bigint NOT NULL DEFAULT 0 CHECK (revision >= 0),
    scopes text[] NOT NULL,
    refresh_token_ciphertext bytea NOT NULL CHECK (octet_length(refresh_token_ciphertext) > 0),
    encrypted_data_key bytea NOT NULL CHECK (octet_length(encrypted_data_key) > 0),
    nonce bytea NOT NULL CHECK (octet_length(nonce) = 12),
    encryption_version integer NOT NULL CHECK (encryption_version > 0),
    kms_key_id text NOT NULL CHECK (kms_key_id <> ''),
    revoked_at timestamptz,
    refresh_lease_id uuid,
    refresh_lease_until timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    last_refreshed_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (fusionauth_user_id, email_address, generation)
);

-- Server-owned PKCE state for one Zoho consent round trip. The row is removed
-- with its in_progress_user_link; grants it produced survive until collected.
CREATE TABLE zoho_link_attempts (
    id uuid PRIMARY KEY REFERENCES in_progress_user_link(id) ON DELETE CASCADE,
    data_center text NOT NULL CHECK (data_center IN ('com','eu','in','com.au','jp','ca','sa','com.cn')),
    redirect_uri text NOT NULL,
    return_uri text,
    pkce_verifier text NOT NULL,
    expires_at timestamptz NOT NULL,
    claimed_at timestamptz,
    completed_at timestamptz,
    grant_id uuid REFERENCES zoho_oauth_grants(grant_id) ON DELETE SET NULL
);

-- Server settings for every IMAP mailbox. Deleted with its link. A password is
-- stored only for password authentication, as an envelope like OAuth grants.
CREATE TABLE imap_accounts (
    link_id uuid PRIMARY KEY REFERENCES email_links(id) ON DELETE CASCADE,
    imap_host text NOT NULL CHECK (imap_host <> ''),
    imap_port integer NOT NULL CHECK (imap_port BETWEEN 1 AND 65535),
    smtp_host text NOT NULL CHECK (smtp_host <> ''),
    smtp_port integer NOT NULL CHECK (smtp_port BETWEEN 1 AND 65535),
    smtp_security text NOT NULL CHECK (smtp_security IN ('tls','starttls')),
    username text NOT NULL CHECK (username <> ''),
    auth_method text NOT NULL CHECK (auth_method IN ('zoho_oauth','password')),
    password_ciphertext bytea,
    password_data_key bytea,
    password_nonce bytea CHECK (octet_length(password_nonce) = 12),
    password_encryption_version integer,
    password_key_id text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT imap_accounts_password_envelope CHECK (
        (auth_method = 'password') = (
            password_ciphertext IS NOT NULL AND password_data_key IS NOT NULL
            AND password_nonce IS NOT NULL AND password_encryption_version IS NOT NULL
            AND password_key_id IS NOT NULL
        )
    )
);
