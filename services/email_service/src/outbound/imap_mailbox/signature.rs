//! Zoho Mail signatures for IMAP inboxes, read through the Zoho Mail API with
//! the inbox's own grant.

use super::*;
use authentication_service_client::{AuthServiceClient, error::AuthServiceClientError};
use std::sync::Arc;
use zoho_client::{ZohoAccessToken, ZohoEndpoints, ZohoError, ZohoMailClient};

#[derive(Clone)]
pub struct ZohoSignatures {
    db: PgPool,
    auth: AuthServiceClient,
    mail: Arc<ZohoMailClient>,
}

impl ZohoSignatures {
    pub fn new(db: PgPool, auth: AuthServiceClient) -> Result<Self, ZohoError> {
        Ok(Self {
            db,
            auth,
            mail: Arc::new(ZohoMailClient::new()?),
        })
    }

    /// The signature Zoho would add to this inbox's mail, if one is saved.
    /// Zoho assigns signatures per send address and may assign none, so the
    /// first saved signature stands in when none is assigned.
    #[tracing::instrument(name = "zoho.signature.read", skip_all, fields(%link_id), err)]
    pub async fn signature(&self, link_id: Uuid) -> Result<Option<String>, EmailApiError> {
        let binding = sqlx::query!(
            "SELECT grant_generation, sync_generation FROM email_links WHERE id = $1 AND provider = 'IMAP' AND is_sync_active",
            link_id
        )
        .fetch_optional(&self.db)
        .await
        .map_err(|_| transient("the IMAP mailbox binding is unavailable"))?
        .ok_or(EmailApiError::AuthRequired)?;
        let grant = self
            .auth
            .get_zoho_access_token(
                link_id,
                binding.grant_generation,
                binding.sync_generation,
                false,
            )
            .await
            .map_err(|error| match error {
                AuthServiceClientError::Unauthorized | AuthServiceClientError::Forbidden => {
                    EmailApiError::AuthRequired
                }
                _ => transient("Zoho authorization is temporarily unavailable"),
            })?;
        let signatures = self
            .mail
            .signatures(
                &ZohoEndpoints::for_data_center(grant.data_center),
                &ZohoAccessToken::new(grant.access_token),
            )
            .await
            .map_err(|error| match error {
                ZohoError::Unauthorized | ZohoError::Forbidden => EmailApiError::AuthRequired,
                error => {
                    tracing::warn!(error = ?error, "Zoho signatures could not be read");
                    transient("Zoho signatures are temporarily unavailable")
                }
            })?;
        Ok(signatures
            .into_iter()
            .next()
            .map(|signature| signature.content))
    }
}
