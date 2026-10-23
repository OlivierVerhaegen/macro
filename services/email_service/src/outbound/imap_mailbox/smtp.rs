//! SMTP submission for IMAP mailboxes. Zoho's SMTP server takes no OAuth, so
//! sessions log in with the mailbox's app password.

use lettre::{
    AsyncSmtpTransport, AsyncTransport, Tokio1Executor,
    address::{Address, Envelope},
    transport::smtp::authentication::Credentials,
};
use std::time::Duration;
use zeroize::Zeroizing;

const SMTP_TIMEOUT: Duration = Duration::from_secs(60);

/// How to reach a mailbox's SMTP server.
#[derive(Debug, Clone)]
pub struct SmtpServer {
    pub host: String,
    pub port: u16,
    /// Implicit TLS (port 465) when true, STARTTLS (port 587) otherwise.
    pub implicit_tls: bool,
    pub username: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SmtpError {
    /// The server refused the login: a wrong or revoked app password.
    #[error("the SMTP server rejected the password")]
    Rejected,
    /// The server refused the message with a permanent (5xx) reply.
    #[error("the SMTP server refused the message")]
    Refused(#[source] lettre::transport::smtp::Error),
    /// A transport failure: the message may or may not have been accepted.
    #[error("the SMTP session failed")]
    Transport(#[source] lettre::transport::smtp::Error),
    #[error("an address cannot be used in the SMTP envelope")]
    InvalidAddress,
}

impl SmtpServer {
    fn transport(
        &self,
        password: &Zeroizing<String>,
    ) -> Result<AsyncSmtpTransport<Tokio1Executor>, SmtpError> {
        let builder = if self.implicit_tls {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&self.host)
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.host)
        }
        .map_err(SmtpError::Transport)?;
        Ok(builder
            .port(self.port)
            .timeout(Some(SMTP_TIMEOUT))
            .credentials(Credentials::new(
                self.username.clone(),
                password.as_str().to_owned(),
            ))
            .build())
    }

    /// Log in and leave, to check a password before it is stored.
    #[tracing::instrument(name = "smtp.verify", skip_all, err)]
    pub async fn verify(&self, password: &Zeroizing<String>) -> Result<(), SmtpError> {
        self.transport(password)?
            .test_connection()
            .await
            .map(drop)
            .map_err(classify)
    }

    /// Submit one prepared RFC 5322 message to its envelope recipients.
    #[tracing::instrument(name = "smtp.submit", skip_all, fields(bytes = message.len(), recipients = recipients.len()), err)]
    pub async fn send(
        &self,
        password: &Zeroizing<String>,
        from: &str,
        recipients: &[String],
        message: &[u8],
    ) -> Result<(), SmtpError> {
        let address = |value: &str| {
            value
                .parse::<Address>()
                .map_err(|_| SmtpError::InvalidAddress)
        };
        let envelope = Envelope::new(
            Some(address(from)?),
            recipients
                .iter()
                .map(|recipient| address(recipient))
                .collect::<Result<Vec<_>, _>>()?,
        )
        .map_err(|_| SmtpError::InvalidAddress)?;
        self.transport(password)?
            .send_raw(&envelope, message)
            .await
            .map(drop)
            .map_err(classify)
    }
}

/// 535 is a failed login; other 5xx replies refuse the message; anything
/// else (timeouts, dropped connections, 4xx) leaves the outcome unknown.
fn classify(error: lettre::transport::smtp::Error) -> SmtpError {
    if error
        .status()
        .is_some_and(|code| code.to_string().starts_with("535"))
    {
        SmtpError::Rejected
    } else if error.is_permanent() {
        SmtpError::Refused(error)
    } else {
        SmtpError::Transport(error)
    }
}
