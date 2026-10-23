//! Setting the SMTP password of an IMAP mailbox (a Zoho app password).
//!
//! Only someone with access to the inbox may set it, the server must accept it
//! before it is stored, and the credential service keeps it, never this domain.

use macro_user_id::user_id::MacroUserIdStr;
use std::future::Future;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::MailboxKey;

#[cfg(test)]
mod test;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SmtpPasswordError {
    /// Not an accessible, connected IMAP inbox.
    #[error("inbox not found")]
    NotFound,
    /// The SMTP server refused the login.
    #[error("the mail server rejected this password")]
    Rejected,
    #[error("the password cannot be saved right now")]
    Unavailable,
}

pub trait SmtpPasswordRepository: Send + Sync + 'static {
    /// The current binding of an active IMAP inbox the actor can access.
    fn accessible_imap_mailbox(
        &self,
        actor: &MacroUserIdStr<'_>,
        link_id: Uuid,
    ) -> impl Future<Output = Result<Option<MailboxKey>, SmtpPasswordError>> + Send;
}

pub trait SmtpPasswordGateway: Send + Sync + 'static {
    /// Log in to the inbox's SMTP server with this password.
    fn verify(
        &self,
        mailbox: MailboxKey,
        password: &Zeroizing<String>,
    ) -> impl Future<Output = Result<(), SmtpPasswordError>> + Send;
    /// Hand the verified password to the credential service.
    fn store(
        &self,
        mailbox: MailboxKey,
        password: &Zeroizing<String>,
    ) -> impl Future<Output = Result<(), SmtpPasswordError>> + Send;
}

pub struct SmtpPasswordService<R, G> {
    repo: R,
    gateway: G,
}

impl<R, G> SmtpPasswordService<R, G> {
    pub fn new(repo: R, gateway: G) -> Self {
        Self { repo, gateway }
    }
}

impl<R: SmtpPasswordRepository, G: SmtpPasswordGateway> SmtpPasswordService<R, G> {
    #[tracing::instrument(name = "email.smtp_password.set", skip(self, actor, password), err)]
    pub async fn set(
        &self,
        actor: &MacroUserIdStr<'_>,
        link_id: Uuid,
        password: Zeroizing<String>,
    ) -> Result<(), SmtpPasswordError> {
        if password.trim().is_empty() {
            return Err(SmtpPasswordError::Rejected);
        }
        let mailbox = self
            .repo
            .accessible_imap_mailbox(actor, link_id)
            .await?
            .ok_or(SmtpPasswordError::NotFound)?;
        self.gateway.verify(mailbox, &password).await?;
        self.gateway.store(mailbox, &password).await
    }
}
