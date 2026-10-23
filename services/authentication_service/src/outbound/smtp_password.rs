use crate::domain::{microsoft::token::EncryptedMicrosoftToken, smtp_password::*};
use sqlx::PgPool;

#[cfg(test)]
mod test;

pub struct PgSmtpPasswords {
    db: PgPool,
}

impl PgSmtpPasswords {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }
}

fn database_error(error: sqlx::Error) -> SmtpPasswordError {
    tracing::error!(error = ?error, "SMTP password persistence failed");
    SmtpPasswordError::Unavailable
}

#[async_trait::async_trait]
impl SmtpPasswordRepository for PgSmtpPasswords {
    async fn owner(&self, binding: MailboxBinding) -> Result<SmtpPasswordOwner, SmtpPasswordError> {
        let row = sqlx::query!(
            r#"SELECT fusionauth_user_id, lower(email_address) AS "email!" FROM email_links
            WHERE id = $1 AND grant_generation = $2 AND sync_generation = $3
                AND is_sync_active AND provider = 'IMAP'"#,
            binding.link_id,
            binding.grant_generation,
            binding.sync_generation
        )
        .fetch_optional(&self.db)
        .await
        .map_err(database_error)?
        .ok_or(SmtpPasswordError::NotConnected)?;
        Ok(SmtpPasswordOwner {
            fusionauth_user_id: row.fusionauth_user_id,
            email: row.email,
        })
    }

    async fn store(
        &self,
        owner: &SmtpPasswordOwner,
        envelope: &EncryptedMicrosoftToken,
    ) -> Result<(), SmtpPasswordError> {
        sqlx::query!(
            r#"INSERT INTO smtp_passwords (fusionauth_user_id, email_address, password_ciphertext,
                encrypted_data_key, nonce, encryption_version, kms_key_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (fusionauth_user_id, email_address) DO UPDATE SET
                password_ciphertext = EXCLUDED.password_ciphertext,
                encrypted_data_key = EXCLUDED.encrypted_data_key, nonce = EXCLUDED.nonce,
                encryption_version = EXCLUDED.encryption_version,
                kms_key_id = EXCLUDED.kms_key_id, updated_at = now()"#,
            owner.fusionauth_user_id,
            owner.email,
            envelope.refresh_token_ciphertext,
            envelope.encrypted_data_key,
            envelope.nonce,
            i32::from(envelope.encryption_version),
            envelope.kms_key_id
        )
        .execute(&self.db)
        .await
        .map_err(database_error)?;
        Ok(())
    }

    async fn load(
        &self,
        owner: &SmtpPasswordOwner,
    ) -> Result<Option<EncryptedMicrosoftToken>, SmtpPasswordError> {
        sqlx::query!(
            r#"SELECT password_ciphertext, encrypted_data_key, nonce, encryption_version, kms_key_id
            FROM smtp_passwords WHERE fusionauth_user_id = $1 AND email_address = $2"#,
            owner.fusionauth_user_id,
            owner.email
        )
        .fetch_optional(&self.db)
        .await
        .map_err(database_error)?
        .map(|row| {
            Ok(EncryptedMicrosoftToken {
                refresh_token_ciphertext: row.password_ciphertext,
                encrypted_data_key: row.encrypted_data_key,
                nonce: row.nonce,
                encryption_version: row
                    .encryption_version
                    .try_into()
                    .map_err(|_| SmtpPasswordError::Unavailable)?,
                kms_key_id: row.kms_key_id,
            })
        })
        .transpose()
    }
}
