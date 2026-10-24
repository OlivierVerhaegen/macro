//! Adapters for the owner-bound Zoho OAuth lifecycle (Olive Trees fork).

use crate::{
    account_link_state::{
        AccountLinkState, AccountLinkStateKey, LinkProvider, sign_account_link_state,
    },
    domain::zoho::*,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;
use zoho_client::{
    ZohoAuthorizationRequest, ZohoDataCenter, ZohoEndpoints, ZohoError, ZohoMailClient,
    ZohoOAuthClient, ZohoRefreshToken, scopes,
};

mod repository;
pub use repository::PgZohoGrants;

/// Zoho is not a FusionAuth identity provider; this names it in signed state.
const IDENTITY_PROVIDER_ID: &str = "zoho";

pub struct ZohoOAuthProvider {
    oauth: ZohoOAuthClient,
    mail: ZohoMailClient,
    state_key: AccountLinkStateKey,
}

impl ZohoOAuthProvider {
    pub(crate) fn new(
        oauth: ZohoOAuthClient,
        mail: ZohoMailClient,
        state_key: AccountLinkStateKey,
    ) -> Self {
        Self {
            oauth,
            mail,
            state_key,
        }
    }
}

fn provider_error(error: ZohoError) -> ZohoAuthError {
    match error {
        ZohoError::Unauthorized | ZohoError::Forbidden => ZohoAuthError::ReauthorizationRequired,
        ZohoError::RateLimited { .. } => ZohoAuthError::Busy,
        error => {
            tracing::warn!(error = %error, "Zoho OAuth call failed");
            ZohoAuthError::Unavailable
        }
    }
}

#[async_trait::async_trait]
impl ZohoIdentityProvider for ZohoOAuthProvider {
    fn authorize(&self, attempt: &ZohoLinkAttempt) -> Result<String, ZohoAuthError> {
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(attempt.verifier.as_bytes()));
        // Bind the signed callback state to the same owner and expiration as
        // the server-owned attempt; the callback rejects unsigned links.
        let state = AccountLinkState {
            provider: LinkProvider::Zoho,
            identity_provider_id: IDENTITY_PROVIDER_ID.to_owned(),
            link_id: attempt.id,
            fusion_user_id: attempt.owner,
            original_url: attempt.return_uri.clone(),
            exp: attempt.expires_at.timestamp(),
        };
        let state = sign_account_link_state(&state, &self.state_key)
            .map_err(|_| ZohoAuthError::Unavailable)?;
        Ok(self
            .oauth
            .authorization_url(
                &ZohoEndpoints::for_data_center(attempt.data_center),
                &ZohoAuthorizationRequest {
                    redirect_uri: &attempt.redirect_uri,
                    state: &state,
                    code_challenge: &challenge,
                    scopes: &scopes::MAILBOX,
                },
            )
            .into())
    }

    async fn exchange(
        &self,
        attempt: &ZohoLinkAttempt,
        code: &str,
    ) -> Result<VerifiedZohoGrant, ZohoAuthError> {
        let endpoints = ZohoEndpoints::for_data_center(attempt.data_center);
        let tokens = self
            .oauth
            .exchange_code(&endpoints, code, &attempt.redirect_uri, &attempt.verifier)
            .await
            .map_err(|error| match error {
                ZohoError::Unauthorized => ZohoAuthError::InvalidAttempt,
                error => provider_error(error),
            })?;
        let refresh_token = tokens.refresh_token.ok_or(ZohoAuthError::InvalidIdentity)?;
        // The address comes from Zoho with the new token, never from the caller.
        let account = self
            .mail
            .accounts(&endpoints, &tokens.access_token)
            .await
            .map_err(|_| ZohoAuthError::InvalidIdentity)?
            .into_iter()
            .next()
            .ok_or(ZohoAuthError::InvalidIdentity)?;
        let email = email_validator::normalize_email(&account.primary_email_address)
            .ok_or(ZohoAuthError::InvalidIdentity)?
            .into_owned();
        Ok(VerifiedZohoGrant {
            zuid: account.zuid,
            account_id: account.account_id,
            email,
            mailbox_address: account.mailbox_address,
            send_addresses: account.send_addresses,
            // Zoho grants the requested scopes as a whole or not at all, and a
            // successful accounts read proves the grant is live.
            scopes: scopes::MAILBOX.map(str::to_owned).to_vec(),
            refresh_token,
        })
    }

    async fn refresh(
        &self,
        data_center: ZohoDataCenter,
        token: &ZohoRefreshToken,
    ) -> Result<RefreshedZohoToken, ZohoAuthError> {
        let refreshed = self
            .oauth
            .refresh(&ZohoEndpoints::for_data_center(data_center), token)
            .await
            .map_err(provider_error)?;
        Ok(RefreshedZohoToken {
            access_token: Zeroizing::new(refreshed.access_token.expose().to_owned()),
            expires_in: refreshed.expires_in,
        })
    }
}
