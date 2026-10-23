use email_api_client::domain::models::EmailApiError;

/// A failed IMAP operation.
#[derive(Debug, thiserror::Error)]
pub enum ImapError {
    /// The server refused the credentials; the user must reconnect.
    #[error("the IMAP server rejected the credentials")]
    AuthenticationFailed,
    /// The folder's UIDVALIDITY changed, so every stored UID is meaningless.
    #[error("the IMAP folder was recreated")]
    FolderRecreated,
    /// The server answered something this client cannot use.
    #[error("the IMAP server sent an unexpected response: {0}")]
    Protocol(String),
    /// The connection could not be opened or broke.
    #[error("the IMAP connection failed")]
    Connection(#[source] std::io::Error),
    /// An IMAP command failed.
    #[error("an IMAP command failed")]
    Command(#[from] async_imap::error::Error),
    /// A position or message id that this crate did not produce.
    #[error("the IMAP stream position or message id is malformed")]
    MalformedIdentifier,
}

impl From<ImapError> for EmailApiError {
    fn from(error: ImapError) -> Self {
        match error {
            ImapError::AuthenticationFailed => EmailApiError::AuthRequired,
            ImapError::FolderRecreated => EmailApiError::OutdatedCursor,
            ImapError::MalformedIdentifier => EmailApiError::Permanent {
                message: error.to_string(),
            },
            ImapError::Protocol(_) | ImapError::Connection(_) | ImapError::Command(_) => {
                EmailApiError::Transient {
                    message: error.to_string(),
                }
            }
        }
    }
}
