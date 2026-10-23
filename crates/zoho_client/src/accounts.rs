use std::time::Duration;

use crate::{ZohoAccessToken, ZohoEndpoints, ZohoError};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// One Zoho Mail account of the authorized user. A user usually has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZohoMailAccount {
    /// Mail API account id; every message and folder call is scoped to it.
    pub account_id: String,
    /// The Zoho user id that owns the account.
    pub zuid: String,
    /// The account's primary address, lowercased.
    pub primary_email_address: String,
    /// Every address the account may send as, primary included, lowercased.
    pub send_addresses: Vec<String>,
}

/// Zoho sends some numeric ids as JSON numbers and others as strings.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum ZohoId {
    Text(String),
    Number(u64),
}

impl From<ZohoId> for String {
    fn from(id: ZohoId) -> Self {
        match id {
            ZohoId::Text(text) => text,
            ZohoId::Number(number) => number.to_string(),
        }
    }
}

#[derive(serde::Deserialize)]
struct AccountsBody {
    data: Vec<AccountBody>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountBody {
    account_id: ZohoId,
    zuid: ZohoId,
    primary_email_address: String,
    #[serde(default)]
    send_mail_details: Vec<SendMailBody>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendMailBody {
    from_address: Option<String>,
}

/// The Zoho Mail REST API.
pub struct ZohoMailClient {
    http: reqwest::Client,
}

impl ZohoMailClient {
    /// Build the client once and reuse it.
    pub fn new() -> Result<Self, ZohoError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()?,
        })
    }

    /// The mail accounts the token's user can use (`ZohoMail.accounts.READ`).
    #[tracing::instrument(name = "zoho.mail.accounts", skip_all, err)]
    pub async fn accounts(
        &self,
        endpoints: &ZohoEndpoints,
        token: &ZohoAccessToken,
    ) -> Result<Vec<ZohoMailAccount>, ZohoError> {
        let mut url = endpoints.mail_api.clone();
        url.set_path("/api/accounts");
        let response = self
            .http
            .get(url)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Zoho-oauthtoken {}", token.expose()),
            )
            .send()
            .await?;
        let body: AccountsBody = read_json(response).await?;
        Ok(body
            .data
            .into_iter()
            .map(|account| {
                let primary = account.primary_email_address.to_lowercase();
                let mut send_addresses = vec![primary.clone()];
                for address in account
                    .send_mail_details
                    .into_iter()
                    .filter_map(|detail| detail.from_address)
                {
                    let address = address.to_lowercase();
                    if !send_addresses.contains(&address) {
                        send_addresses.push(address);
                    }
                }
                ZohoMailAccount {
                    account_id: account.account_id.into(),
                    zuid: account.zuid.into(),
                    primary_email_address: primary,
                    send_addresses,
                }
            })
            .collect())
    }
}

async fn read_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, ZohoError> {
    let status = response.status();
    match status {
        reqwest::StatusCode::UNAUTHORIZED => return Err(ZohoError::Unauthorized),
        reqwest::StatusCode::FORBIDDEN => return Err(ZohoError::Forbidden),
        reqwest::StatusCode::TOO_MANY_REQUESTS => {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok())
                .map(Duration::from_secs);
            return Err(ZohoError::RateLimited { retry_after });
        }
        status if !status.is_success() => return Err(ZohoError::Status(status.as_u16())),
        _ => {}
    }
    response
        .json()
        .await
        .map_err(|_| ZohoError::MalformedResponse)
}
