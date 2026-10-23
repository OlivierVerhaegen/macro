use std::time::Duration;

use url::Url;
use zeroize::Zeroizing;

use crate::{ZohoEndpoints, ZohoError};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Scope names, in Zoho's own spelling.
pub mod scopes {
    /// XOAUTH2 login to Zoho's IMAP server.
    pub const IMAP: &str = "ZOHOMAIL.PROTOCOL.IMAP";
    /// Read the user's mail accounts, aliases and send-as addresses.
    pub const ACCOUNTS_READ: &str = "ZohoMail.accounts.READ";
    /// Read, send and update messages through the Mail API.
    pub const MESSAGES_ALL: &str = "ZohoMail.messages.ALL";
    /// Read and change folders and labels through the Mail API.
    pub const FOLDERS_ALL: &str = "ZohoMail.folders.ALL";

    /// Everything a linked Zoho mailbox needs.
    pub const MAILBOX: [&str; 4] = [IMAP, ACCOUNTS_READ, MESSAGES_ALL, FOLDERS_ALL];
}

/// The registered server-based client. The secret never leaves this process.
pub struct ZohoClientCredentials {
    client_id: String,
    client_secret: Zeroizing<String>,
}

impl ZohoClientCredentials {
    /// Credentials from the Zoho API Console.
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            client_id,
            client_secret: Zeroizing::new(client_secret),
        }
    }
}

/// A short-lived bearer token. Redacted from `Debug`; zeroized on drop.
pub struct ZohoAccessToken(Zeroizing<String>);

impl ZohoAccessToken {
    /// Wrap a token received from Zoho.
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    /// Borrow the token to place it in a request.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ZohoAccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ZohoAccessToken([REDACTED])")
    }
}

/// A long-lived refresh token. Redacted from `Debug`; zeroized on drop.
pub struct ZohoRefreshToken(Zeroizing<String>);

impl ZohoRefreshToken {
    /// Wrap a token received from Zoho or decrypted from storage.
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    /// Borrow the token to place it in a request or encrypt it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ZohoRefreshToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ZohoRefreshToken([REDACTED])")
    }
}

/// What Zoho returns from its token endpoint.
#[derive(Debug)]
pub struct ZohoTokenResponse {
    /// Bearer token for the Mail API and IMAP XOAUTH2.
    pub access_token: ZohoAccessToken,
    /// Present on a code exchange with `access_type=offline`; absent on refresh.
    pub refresh_token: Option<ZohoRefreshToken>,
    /// Seconds until `access_token` expires.
    pub expires_in: u64,
}

/// One consent request.
pub struct ZohoAuthorizationRequest<'a> {
    /// The callback registered for this client.
    pub redirect_uri: &'a str,
    /// Signed state the callback verifies.
    pub state: &'a str,
    /// Base64url SHA-256 of the PKCE verifier.
    pub code_challenge: &'a str,
    /// Requested scopes.
    pub scopes: &'a [&'a str],
}

#[derive(serde::Deserialize)]
struct TokenBody {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
}

/// Zoho's OAuth 2.0 server, for a server-based client.
pub struct ZohoOAuthClient {
    http: reqwest::Client,
    credentials: ZohoClientCredentials,
}

impl ZohoOAuthClient {
    /// Build the client once and reuse it.
    pub fn new(credentials: ZohoClientCredentials) -> Result<Self, ZohoError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()?,
            credentials,
        })
    }

    /// The consent page. `access_type=offline` and `prompt=consent` make Zoho
    /// return a refresh token on every link, including reconnects.
    pub fn authorization_url(
        &self,
        endpoints: &ZohoEndpoints,
        request: &ZohoAuthorizationRequest<'_>,
    ) -> Url {
        let mut url = endpoints.accounts.clone();
        url.set_path("/oauth/v2/auth");
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.credentials.client_id)
            .append_pair("scope", &request.scopes.join(","))
            .append_pair("redirect_uri", request.redirect_uri)
            .append_pair("access_type", "offline")
            .append_pair("prompt", "consent")
            .append_pair("state", request.state)
            .append_pair("code_challenge", request.code_challenge)
            .append_pair("code_challenge_method", "S256");
        url
    }

    /// Exchange the callback's code. The code is single use.
    #[tracing::instrument(name = "zoho.oauth.exchange", skip_all, err)]
    pub async fn exchange_code(
        &self,
        endpoints: &ZohoEndpoints,
        code: &str,
        redirect_uri: &str,
        code_verifier: &str,
    ) -> Result<ZohoTokenResponse, ZohoError> {
        self.token(
            endpoints,
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("code_verifier", code_verifier),
            ],
        )
        .await
    }

    /// Obtain a new access token. Zoho keeps the refresh token unchanged.
    #[tracing::instrument(name = "zoho.oauth.refresh", skip_all, err)]
    pub async fn refresh(
        &self,
        endpoints: &ZohoEndpoints,
        refresh_token: &ZohoRefreshToken,
    ) -> Result<ZohoTokenResponse, ZohoError> {
        self.token(
            endpoints,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token.expose()),
            ],
        )
        .await
    }

    async fn token(
        &self,
        endpoints: &ZohoEndpoints,
        grant: &[(&str, &str)],
    ) -> Result<ZohoTokenResponse, ZohoError> {
        let mut url = endpoints.accounts.clone();
        url.set_path("/oauth/v2/token");
        let mut form = vec![
            ("client_id", self.credentials.client_id.as_str()),
            ("client_secret", self.credentials.client_secret.as_str()),
        ];
        form.extend_from_slice(grant);
        let response = self.http.post(url).form(&form).send().await?;
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ZohoError::RateLimited { retry_after: None });
        }
        if status.is_server_error() {
            return Err(ZohoError::Status(status.as_u16()));
        }
        // Zoho reports grant errors with HTTP 200 and an `error` field.
        let body: TokenBody = response
            .json()
            .await
            .map_err(|_| ZohoError::MalformedResponse)?;
        if let Some(error) = body.error {
            return Err(match error.as_str() {
                "invalid_code" | "invalid_token" | "access_denied" => ZohoError::Unauthorized,
                _ => ZohoError::OAuth(error),
            });
        }
        if !status.is_success() {
            return Err(ZohoError::Status(status.as_u16()));
        }
        let access_token = body
            .access_token
            .filter(|token| !token.is_empty())
            .ok_or(ZohoError::MalformedResponse)?;
        Ok(ZohoTokenResponse {
            access_token: ZohoAccessToken::new(access_token),
            refresh_token: body
                .refresh_token
                .filter(|token| !token.is_empty())
                .map(ZohoRefreshToken::new),
            expires_in: body.expires_in.ok_or(ZohoError::MalformedResponse)?,
        })
    }
}
