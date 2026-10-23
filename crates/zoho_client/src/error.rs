use std::time::Duration;

/// A failed Zoho OAuth or Mail API call.
#[derive(Debug, thiserror::Error)]
pub enum ZohoError {
    /// The code, refresh token or access token is invalid, expired or revoked.
    /// Retrying cannot help; the user must reconnect.
    #[error("Zoho rejected the authorization")]
    Unauthorized,
    /// The token lacks a scope this call needs.
    #[error("Zoho denied access to this resource")]
    Forbidden,
    /// Zoho throttled the caller.
    #[error("Zoho rate limit reached")]
    RateLimited {
        /// Zoho's `Retry-After`, when it sent one.
        retry_after: Option<Duration>,
    },
    /// The OAuth server answered with an error code other than an invalid grant.
    #[error("Zoho OAuth error: {0}")]
    OAuth(String),
    /// A non-success HTTP status without a more specific meaning.
    #[error("Zoho responded with HTTP status {0}")]
    Status(u16),
    /// A success response that does not have the documented shape.
    #[error("Zoho returned an unexpected response")]
    MalformedResponse,
    /// The request did not complete.
    #[error("Zoho request failed")]
    Transport(#[from] reqwest::Error),
}

impl ZohoError {
    /// Whether a later retry of the same request can succeed.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::RateLimited { .. } | Self::Transport(_) => true,
            Self::Status(status) => *status >= 500,
            Self::Unauthorized | Self::Forbidden | Self::OAuth(_) | Self::MalformedResponse => {
                false
            }
        }
    }
}
