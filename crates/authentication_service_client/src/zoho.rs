//! Zoho Mail credentials held by the authentication service (Olive Trees fork).

use crate::{
    AuthServiceClient,
    error::{AuthServiceClientError, GenericErrorResponse},
};
use zoho_client::ZohoDataCenter;

/// Secret-bearing response intentionally does not implement Debug.
#[derive(serde::Deserialize)]
pub struct ZohoAccessToken {
    pub access_token: String,
    pub scopes: Vec<String>,
    pub data_center: ZohoDataCenter,
    pub account_id: String,
    pub email: String,
}

/// Non-secret, owner-bound result of a completed Zoho OAuth attempt.
#[derive(Debug, serde::Deserialize)]
pub struct CompletedZohoGrant {
    pub grant_id: uuid::Uuid,
    pub generation: i64,
    pub owner: uuid::Uuid,
    pub email: String,
    pub data_center: ZohoDataCenter,
    pub zuid: String,
    pub account_id: String,
    pub scopes: Vec<String>,
    /// Every address the account may send as, primary included.
    #[serde(default)]
    pub send_addresses: Vec<String>,
    /// The IMAP and SMTP login when it differs from `email`'s alias.
    #[serde(default)]
    pub mailbox_address: Option<String>,
}

impl AuthServiceClient {
    pub async fn completed_zoho_grant(
        &self,
        attempt_id: uuid::Uuid,
        owner: uuid::Uuid,
    ) -> Result<CompletedZohoGrant, AuthServiceClientError> {
        let response = self
            .client
            .get(format!("{}/internal/zoho_grant", self.url))
            .query(&[("attempt_id", attempt_id), ("owner", owner)])
            .send()
            .await
            .map_err(|_| unavailable())?;
        match response.status() {
            reqwest::StatusCode::OK => response.json().await.map_err(|_| unavailable()),
            reqwest::StatusCode::FORBIDDEN => Err(AuthServiceClientError::Forbidden),
            reqwest::StatusCode::NOT_FOUND => Err(AuthServiceClientError::NotFound),
            _ => Err(unavailable()),
        }
    }

    /// Fetch a token bound to an existing IMAP link and its current grant
    /// generation. Owner and mailbox address are resolved by authentication.
    pub async fn get_zoho_access_token(
        &self,
        link_id: uuid::Uuid,
        generation: i64,
        sync_generation: i64,
        force_refresh: bool,
    ) -> Result<ZohoAccessToken, AuthServiceClientError> {
        let response = self
            .client
            .get(format!("{}/internal/zoho_access_token", self.url))
            .timeout(std::time::Duration::from_secs(60))
            .query(&[
                ("link_id", link_id.to_string()),
                ("generation", generation.to_string()),
                ("sync_generation", sync_generation.to_string()),
                ("force_refresh", force_refresh.to_string()),
            ])
            .send()
            .await
            .map_err(|_| unavailable())?;
        match response.status() {
            reqwest::StatusCode::OK => {
                let token: ZohoAccessToken = response.json().await.map_err(|_| unavailable())?;
                if token.access_token.is_empty() {
                    return Err(unavailable());
                }
                Ok(token)
            }
            reqwest::StatusCode::UNAUTHORIZED => Err(AuthServiceClientError::Unauthorized),
            reqwest::StatusCode::FORBIDDEN => Err(AuthServiceClientError::Forbidden),
            reqwest::StatusCode::NOT_FOUND => Err(AuthServiceClientError::NotFound),
            _ => Err(unavailable()),
        }
    }
}

fn unavailable() -> AuthServiceClientError {
    AuthServiceClientError::Generic(GenericErrorResponse {
        message: "Zoho access token is temporarily unavailable".into(),
    })
}

/// Secret-bearing response intentionally does not implement Debug.
#[derive(serde::Deserialize)]
pub struct SmtpPassword {
    pub password: String,
}

impl AuthServiceClient {
    /// Store an SMTP password the caller verified with the mailbox's server.
    pub async fn store_smtp_password(
        &self,
        link_id: uuid::Uuid,
        generation: i64,
        sync_generation: i64,
        password: &str,
    ) -> Result<(), AuthServiceClientError> {
        #[derive(serde::Serialize)]
        struct Request<'a> {
            link_id: uuid::Uuid,
            generation: i64,
            sync_generation: i64,
            password: &'a str,
        }
        let response = self
            .client
            .post(format!("{}/internal/smtp_password", self.url))
            .json(&Request {
                link_id,
                generation,
                sync_generation,
                password,
            })
            .send()
            .await
            .map_err(|_| unavailable())?;
        match response.status() {
            reqwest::StatusCode::NO_CONTENT => Ok(()),
            reqwest::StatusCode::UNAUTHORIZED => Err(AuthServiceClientError::Unauthorized),
            reqwest::StatusCode::NOT_FOUND => Err(AuthServiceClientError::NotFound),
            _ => Err(unavailable()),
        }
    }

    /// The SMTP password for an active IMAP link's exact binding.
    pub async fn get_smtp_password(
        &self,
        link_id: uuid::Uuid,
        generation: i64,
        sync_generation: i64,
    ) -> Result<SmtpPassword, AuthServiceClientError> {
        let response = self
            .client
            .get(format!("{}/internal/smtp_password", self.url))
            .query(&[
                ("link_id", link_id.to_string()),
                ("generation", generation.to_string()),
                ("sync_generation", sync_generation.to_string()),
            ])
            .send()
            .await
            .map_err(|_| unavailable())?;
        match response.status() {
            reqwest::StatusCode::OK => response.json().await.map_err(|_| unavailable()),
            reqwest::StatusCode::UNAUTHORIZED => Err(AuthServiceClientError::Unauthorized),
            reqwest::StatusCode::NOT_FOUND => Err(AuthServiceClientError::NotFound),
            _ => Err(unavailable()),
        }
    }
}
