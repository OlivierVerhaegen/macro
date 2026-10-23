use crate::domain::{microsoft::token::EncryptedMicrosoftToken, zoho::*};
use sqlx::PgPool;
use uuid::Uuid;
use zeroize::Zeroizing;
use zoho_client::ZohoDataCenter;

#[cfg(test)]
mod test;

pub struct PgZohoGrants {
    db: PgPool,
}

impl PgZohoGrants {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }
}

fn database_error(error: sqlx::Error) -> ZohoAuthError {
    tracing::error!(error = ?error, "Zoho grant persistence failed");
    ZohoAuthError::Unavailable
}

fn data_center(value: &str) -> Result<ZohoDataCenter, ZohoAuthError> {
    value.parse().map_err(|_| ZohoAuthError::Unavailable)
}

fn envelope(
    refresh_token_ciphertext: Vec<u8>,
    encrypted_data_key: Vec<u8>,
    nonce: Vec<u8>,
    encryption_version: i32,
    kms_key_id: String,
) -> Result<EncryptedMicrosoftToken, ZohoAuthError> {
    Ok(EncryptedMicrosoftToken {
        refresh_token_ciphertext,
        encrypted_data_key,
        nonce,
        encryption_version: encryption_version
            .try_into()
            .map_err(|_| ZohoAuthError::Unavailable)?,
        kms_key_id,
    })
}

#[async_trait::async_trait]
impl ZohoGrantRepository for PgZohoGrants {
    async fn collect_expired_grants(
        &self,
        older_than: chrono::DateTime<chrono::Utc>,
        limit: i64,
    ) -> Result<u64, ZohoAuthError> {
        let mut tx = self.db.begin().await.map_err(database_error)?;
        // Initialization consumes the same row before adopting its grant;
        // skip rows it holds and keep the grant they reference.
        sqlx::query!(
            r#"DELETE FROM in_progress_user_link WHERE id IN (
            SELECT p.id FROM in_progress_user_link p JOIN zoho_link_attempts a ON a.id = p.id
            WHERE p.email_provider = 'IMAP' AND p.created_at <= $1::timestamptz
            ORDER BY p.created_at LIMIT $2 FOR UPDATE OF p SKIP LOCKED)
        "#,
            older_than,
            limit
        )
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        let removed = sqlx::query!(
            r#"DELETE FROM zoho_oauth_grants WHERE grant_id IN (
            SELECT g.grant_id FROM zoho_oauth_grants g
            WHERE g.created_at <= $1
                AND NOT EXISTS(SELECT 1 FROM email_links l WHERE l.grant_id = g.grant_id)
                AND NOT EXISTS(SELECT 1 FROM email_mailbox_custodians c WHERE c.grant_id = g.grant_id)
                AND NOT EXISTS(SELECT 1 FROM zoho_link_attempts a WHERE a.grant_id = g.grant_id)
            ORDER BY g.created_at LIMIT $2 FOR UPDATE OF g SKIP LOCKED)
        "#,
            older_than,
            limit
        )
        .execute(&mut *tx)
        .await
        .map_err(database_error)?
        .rows_affected();
        tx.commit().await.map_err(database_error)?;
        Ok(removed)
    }

    async fn connection_facts(
        &self,
        owner: Uuid,
        reconnect: Option<Uuid>,
    ) -> Result<email::domain::inbox_entitlement::InboxConnectionFacts, ZohoAuthError> {
        let facts = sqlx::query!(r#"SELECT
            EXISTS(SELECT 1 FROM "RolesOnUsers" ru JOIN "RolesOnPermissions" rp ON rp."roleId"=ru."roleId" WHERE ru."userId"=u.id AND rp."permissionId"='read:professional_features') AS "professional!",
            (SELECT count(*) FROM email_links l WHERE l.macro_id=u.id OR EXISTS(SELECT 1 FROM macro_user_links a WHERE a.link_id=l.id AND a.primary_macro_id=u.id)) AS "inboxes!",
            EXISTS(SELECT 1 FROM email_links l WHERE l.id=$2 AND l.provider='IMAP' AND (l.macro_id=u.id OR EXISTS(SELECT 1 FROM macro_user_links a WHERE a.link_id=l.id AND a.primary_macro_id=u.id))) AS "reconnecting!"
            FROM "User" u WHERE u.macro_user_id=$1 LIMIT 1"#, owner, reconnect)
            .fetch_optional(&self.db)
            .await
            .map_err(database_error)?
            .ok_or(ZohoAuthError::InvalidIdentity)?;
        Ok(email::domain::inbox_entitlement::InboxConnectionFacts {
            professional: facts.professional,
            accessible_inboxes: facts.inboxes,
            reconnecting: facts.reconnecting,
        })
    }

    async fn begin_link(&self, attempt: &ZohoLinkAttempt) -> Result<(), ZohoAuthError> {
        let mut tx = self.db.begin().await.map_err(database_error)?;
        // Serialize the cap check for this principal, including concurrent starts.
        sqlx::query!(
            "SELECT 1 AS locked FROM pg_advisory_xact_lock(hashtextextended($1, 0))",
            attempt.owner.to_string()
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        let result = sqlx::query!(
            r#"
            INSERT INTO in_progress_user_link (id, macro_user_id, email_provider)
            SELECT $1, $2, 'IMAP'
            WHERE (SELECT count(*) FROM in_progress_user_link
                WHERE macro_user_id = $2 AND created_at > now() - interval '24 hours') < 5
        "#,
            attempt.id,
            attempt.owner
        )
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if result.rows_affected() != 1 {
            return Err(ZohoAuthError::TooManyAttempts);
        }
        sqlx::query!(
            r#"
            INSERT INTO zoho_link_attempts
                (id, data_center, redirect_uri, return_uri, pkce_verifier, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6)
        "#,
            attempt.id,
            attempt.data_center.as_ref(),
            attempt.redirect_uri,
            attempt.return_uri,
            attempt.verifier.as_str(),
            attempt.expires_at
        )
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)
    }

    async fn claim_link(&self, id: Uuid) -> Result<ZohoLinkAttempt, ZohoAuthError> {
        let row = sqlx::query!(
            r#"
            UPDATE zoho_link_attempts a SET claimed_at = now()
            FROM in_progress_user_link p
            WHERE a.id = $1 AND a.id = p.id AND p.email_provider = 'IMAP'
                AND a.claimed_at IS NULL AND a.completed_at IS NULL AND a.expires_at > now()
            RETURNING a.id, p.macro_user_id, a.data_center, a.redirect_uri, a.return_uri,
                a.pkce_verifier, a.expires_at
        "#,
            id
        )
        .fetch_optional(&self.db)
        .await
        .map_err(database_error)?
        .ok_or(ZohoAuthError::InvalidAttempt)?;
        Ok(ZohoLinkAttempt {
            id: row.id,
            owner: row.macro_user_id,
            data_center: data_center(&row.data_center)?,
            redirect_uri: row.redirect_uri,
            return_uri: row.return_uri,
            verifier: Zeroizing::new(row.pkce_verifier),
            expires_at: row.expires_at,
        })
    }

    async fn abandon_link(&self, id: Uuid) -> Result<(), ZohoAuthError> {
        sqlx::query!(
            "DELETE FROM in_progress_user_link WHERE id = $1 AND email_provider = 'IMAP'",
            id
        )
        .execute(&self.db)
        .await
        .map_err(database_error)?;
        Ok(())
    }

    async fn complete_link(
        &self,
        attempt: &ZohoLinkAttempt,
        identity: &VerifiedZohoGrant,
        envelope: &EncryptedMicrosoftToken,
        grant_id: Uuid,
    ) -> Result<(), ZohoAuthError> {
        let mut tx = self.db.begin().await.map_err(database_error)?;
        let owner = attempt.owner.to_string();
        // Serialize reconnects for this verified address.
        sqlx::query!(
            "SELECT 1 AS locked FROM pg_advisory_xact_lock(hashtextextended($1, 0))",
            identity.email
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        // A different Zoho user or account behind a known address is a conflict.
        let saved = sqlx::query!(
            r#"
            INSERT INTO zoho_oauth_grants
                (grant_id, fusionauth_user_id, email_address, data_center, zoho_user_id,
                 zoho_account_id, scopes, refresh_token_ciphertext, encrypted_data_key, nonce,
                 encryption_version, kms_key_id, generation)
            SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                COALESCE((SELECT max(g.generation) FROM zoho_oauth_grants g
                    WHERE g.fusionauth_user_id = $2 AND g.email_address = $3), 0) + 1
            WHERE NOT EXISTS(SELECT 1 FROM zoho_oauth_grants g
                WHERE g.fusionauth_user_id = $2 AND g.email_address = $3
                    AND (g.zoho_user_id <> $5 OR g.zoho_account_id <> $6 OR g.data_center <> $4))
        "#,
            grant_id,
            owner,
            identity.email,
            attempt.data_center.as_ref(),
            identity.zuid,
            identity.account_id,
            &identity.scopes,
            envelope.refresh_token_ciphertext,
            envelope.encrypted_data_key,
            envelope.nonce,
            i32::from(envelope.encryption_version),
            envelope.kms_key_id
        )
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if saved.rows_affected() != 1 {
            return Err(ZohoAuthError::OwnershipConflict);
        }
        let marked = sqlx::query!(
            r#"
            UPDATE zoho_link_attempts a SET completed_at = now(), pkce_verifier = '', grant_id = $3
            FROM in_progress_user_link p
            WHERE a.id = $1 AND p.id = a.id AND p.macro_user_id = $2
                AND p.email_provider = 'IMAP' AND a.claimed_at IS NOT NULL
                AND a.completed_at IS NULL AND a.expires_at > now()
        "#,
            attempt.id,
            attempt.owner,
            grant_id
        )
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if marked.rows_affected() != 1 {
            return Err(ZohoAuthError::InvalidAttempt);
        }
        sqlx::query!(
            "UPDATE in_progress_user_link SET linked_email = $2 WHERE id = $1",
            attempt.id,
            identity.email
        )
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)
    }

    async fn completed_grant(
        &self,
        attempt: Uuid,
        owner: Uuid,
    ) -> Result<CompletedZohoGrant, ZohoAuthError> {
        let row = sqlx::query!(
            r#"
            SELECT g.grant_id, g.generation, g.email_address, g.data_center, g.zoho_user_id,
                g.zoho_account_id, g.scopes
            FROM in_progress_user_link p
            JOIN zoho_link_attempts a ON a.id = p.id AND a.completed_at IS NOT NULL
            JOIN zoho_oauth_grants g ON g.grant_id = a.grant_id
                AND g.fusionauth_user_id = p.macro_user_id::text AND g.email_address = p.linked_email
            WHERE p.id = $1 AND p.macro_user_id = $2 AND p.email_provider = 'IMAP'
                AND p.created_at > now() - interval '24 hours' AND g.revoked_at IS NULL
        "#,
            attempt,
            owner
        )
        .fetch_optional(&self.db)
        .await
        .map_err(database_error)?
        .ok_or(ZohoAuthError::InvalidAttempt)?;
        Ok(CompletedZohoGrant {
            grant_id: row.grant_id,
            generation: row.generation,
            owner,
            email: row.email_address,
            data_center: data_center(&row.data_center)?,
            zuid: row.zoho_user_id,
            account_id: row.zoho_account_id,
            scopes: row.scopes,
        })
    }

    async fn active_grant(
        &self,
        link_id: Uuid,
        generation: i64,
        sync_generation: i64,
    ) -> Result<StoredZohoGrant, ZohoAuthError> {
        let row = sqlx::query!(
            r#"
            SELECT g.grant_id, g.generation, g.revision, g.fusionauth_user_id, g.email_address,
                g.data_center, g.zoho_account_id, g.scopes, g.refresh_token_ciphertext,
                g.encrypted_data_key, g.nonce, g.encryption_version, g.kms_key_id
            FROM zoho_oauth_grants g JOIN email_links l ON l.grant_id = g.grant_id
                AND l.grant_generation = g.generation AND l.fusionauth_user_id = g.fusionauth_user_id
                AND lower(l.email_address) = g.email_address
            WHERE l.id = $1 AND g.generation = $2 AND l.provider = 'IMAP'
                AND l.is_sync_active AND l.sync_generation = $3 AND g.revoked_at IS NULL
        "#,
            link_id,
            generation,
            sync_generation
        )
        .fetch_optional(&self.db)
        .await
        .map_err(database_error)?
        .ok_or(ZohoAuthError::ReauthorizationRequired)?;
        Ok(StoredZohoGrant {
            id: row.grant_id,
            generation: row.generation,
            revision: row.revision,
            owner: row.fusionauth_user_id,
            email: row.email_address,
            data_center: data_center(&row.data_center)?,
            account_id: row.zoho_account_id,
            scopes: row.scopes,
            envelope: envelope(
                row.refresh_token_ciphertext,
                row.encrypted_data_key,
                row.nonce,
                row.encryption_version,
                row.kms_key_id,
            )?,
        })
    }

    async fn acquire_refresh(
        &self,
        grant: &StoredZohoGrant,
        lease: Uuid,
    ) -> Result<bool, ZohoAuthError> {
        let result = sqlx::query!(
            r#"
            UPDATE zoho_oauth_grants SET refresh_lease_id = $4,
                refresh_lease_until = now() + interval '1 minute'
            WHERE grant_id = $1 AND generation = $2 AND revision = $3 AND revoked_at IS NULL
                AND (refresh_lease_until IS NULL OR refresh_lease_until < now())
        "#,
            grant.id,
            grant.generation,
            grant.revision,
            lease
        )
        .execute(&self.db)
        .await
        .map_err(database_error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn finish_refresh(
        &self,
        grant: &StoredZohoGrant,
        lease: Uuid,
    ) -> Result<bool, ZohoAuthError> {
        let result = sqlx::query!(
            r#"
            UPDATE zoho_oauth_grants SET revision = revision + 1, refresh_lease_id = NULL,
                refresh_lease_until = NULL, updated_at = now(), last_refreshed_at = now()
            WHERE grant_id = $1 AND generation = $2 AND revision = $3 AND refresh_lease_id = $4
                AND revoked_at IS NULL AND refresh_lease_until > now()
        "#,
            grant.id,
            grant.generation,
            grant.revision,
            lease
        )
        .execute(&self.db)
        .await
        .map_err(database_error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn release_refresh(
        &self,
        grant: &StoredZohoGrant,
        lease: Uuid,
        revoke: bool,
    ) -> Result<(), ZohoAuthError> {
        sqlx::query!(
            r#"
            UPDATE zoho_oauth_grants SET refresh_lease_id = NULL, refresh_lease_until = NULL,
                revoked_at = CASE WHEN $5 THEN now() ELSE revoked_at END
            WHERE grant_id = $1 AND generation = $2 AND revision = $3 AND refresh_lease_id = $4
        "#,
            grant.id,
            grant.generation,
            grant.revision,
            lease,
            revoke
        )
        .execute(&self.db)
        .await
        .map_err(database_error)?;
        Ok(())
    }
}
