use authentication_service_client::{AuthServiceClient, error::AuthServiceClientError};
use email::domain::mailbox::credentials::{MailboxCredentialGateway, MailboxCredentialToken};
use email_api_client::domain::models::{AccessToken, MailboxAccess, TokenError, TokenFreshness};

/// Zoho access tokens for an IMAP link's exact grant binding.
#[derive(Clone)]
pub struct ZohoCredentialsClient(pub AuthServiceClient);

impl MailboxCredentialGateway for ZohoCredentialsClient {
    async fn acquire(
        &self,
        mailbox: MailboxAccess,
        freshness: TokenFreshness,
    ) -> Result<MailboxCredentialToken, TokenError> {
        self.0
            .get_zoho_access_token(
                mailbox.link_id,
                mailbox.grant_generation,
                mailbox.sync_generation,
                matches!(freshness, TokenFreshness::Fresh),
            )
            .await
            .map(|response| MailboxCredentialToken {
                token: AccessToken::new(response.access_token),
                scopes: response.scopes,
            })
            .map_err(|error| match error {
                AuthServiceClientError::Unauthorized | AuthServiceClientError::Forbidden => {
                    TokenError::ReauthRequired
                }
                _ => TokenError::Transient {
                    message: "Zoho authorization is temporarily unavailable".into(),
                },
            })
    }
}
