//! SMTP passwords for IMAP mailboxes whose SMTP server takes no OAuth (Zoho
//! app passwords). Only the active binding of an IMAP link may store or read
//! one; the email service verifies a password with the server before storing.

use std::sync::Arc;

use uuid::Uuid;
use zeroize::Zeroizing;

use crate::domain::microsoft::token::{
    EncryptedMicrosoftToken, MicrosoftRefreshToken, MicrosoftTokenCipher,
};

#[cfg(test)]
mod test;

/// Envelopes for SMTP passwords never decrypt as another secret kind.
pub const SMTP_PASSWORD_PURPOSE: &str = "smtp-password";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SmtpPasswordError {
    /// The link is not an active IMAP mailbox at this binding.
    #[error("the mailbox is not connected")]
    NotConnected,
    /// No password has been stored for the mailbox.
    #[error("no SMTP password is stored for this mailbox")]
    Missing,
    #[error("SMTP password storage is temporarily unavailable")]
    Unavailable,
}

/// Whose password it is: the mailbox's credential owner and address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpPasswordOwner {
    pub fusionauth_user_id: String,
    pub email: String,
}

/// The exact link binding the email service acts under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailboxBinding {
    pub link_id: Uuid,
    pub grant_generation: i64,
    pub sync_generation: i64,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait SmtpPasswordRepository: Send + Sync {
    /// The owner of an active IMAP link at exactly this binding.
    async fn owner(&self, binding: MailboxBinding) -> Result<SmtpPasswordOwner, SmtpPasswordError>;
    /// Insert or replace the owner's password envelope.
    async fn store(
        &self,
        owner: &SmtpPasswordOwner,
        envelope: &EncryptedMicrosoftToken,
    ) -> Result<(), SmtpPasswordError>;
    async fn load(
        &self,
        owner: &SmtpPasswordOwner,
    ) -> Result<Option<EncryptedMicrosoftToken>, SmtpPasswordError>;
}

/// Object-safe because the API context holds it as `Arc<dyn SmtpPasswords>`.
#[async_trait::async_trait]
pub trait SmtpPasswords: Send + Sync {
    async fn store(
        &self,
        binding: MailboxBinding,
        password: Zeroizing<String>,
    ) -> Result<(), SmtpPasswordError>;
    async fn password(
        &self,
        binding: MailboxBinding,
    ) -> Result<Zeroizing<String>, SmtpPasswordError>;
}

pub struct SmtpPasswordService<R> {
    repo: R,
    cipher: Arc<dyn MicrosoftTokenCipher>,
}

impl<R> SmtpPasswordService<R> {
    /// `cipher` must be bound to [`SMTP_PASSWORD_PURPOSE`].
    pub fn new(repo: R, cipher: Arc<dyn MicrosoftTokenCipher>) -> Self {
        Self { repo, cipher }
    }
}

#[async_trait::async_trait]
impl<R: SmtpPasswordRepository> SmtpPasswords for SmtpPasswordService<R> {
    async fn store(
        &self,
        binding: MailboxBinding,
        password: Zeroizing<String>,
    ) -> Result<(), SmtpPasswordError> {
        let owner = self.repo.owner(binding).await?;
        let envelope = self
            .cipher
            .encrypt(
                &owner.fusionauth_user_id,
                &owner.email,
                MicrosoftRefreshToken::new(password.to_string()),
            )
            .await
            .map_err(|_| SmtpPasswordError::Unavailable)?;
        self.repo.store(&owner, &envelope).await
    }

    async fn password(
        &self,
        binding: MailboxBinding,
    ) -> Result<Zeroizing<String>, SmtpPasswordError> {
        let owner = self.repo.owner(binding).await?;
        let envelope = self
            .repo
            .load(&owner)
            .await?
            .ok_or(SmtpPasswordError::Missing)?;
        let password = self
            .cipher
            .decrypt(&owner.fusionauth_user_id, &owner.email, &envelope)
            .await
            .map_err(|_| SmtpPasswordError::Unavailable)?;
        Ok(Zeroizing::new(password.as_str().to_owned()))
    }
}
