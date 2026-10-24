use crate::{ZohoAccessToken, ZohoEndpoints, ZohoError, ZohoMailClient, accounts::read_json};

/// One signature the user saved in Zoho Mail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZohoSignature {
    /// Zoho's id, which a send address may reference as its `signatureId`.
    pub id: String,
    /// The name shown in Zoho's signature settings.
    pub name: Option<String>,
    /// The signature as Zoho stores it: HTML, not yet sanitized.
    pub content: String,
}

#[derive(serde::Deserialize)]
struct SignaturesBody {
    #[serde(default)]
    data: Vec<SignatureBody>,
}

#[derive(serde::Deserialize)]
struct SignatureBody {
    id: Option<crate::accounts::ZohoId>,
    name: Option<String>,
    content: Option<String>,
}

impl ZohoMailClient {
    /// The user's saved signatures, empty ones left out (`ZohoMail.accounts.READ`).
    #[tracing::instrument(name = "zoho.mail.signatures", skip_all, err)]
    pub async fn signatures(
        &self,
        endpoints: &ZohoEndpoints,
        token: &ZohoAccessToken,
    ) -> Result<Vec<ZohoSignature>, ZohoError> {
        let mut url = endpoints.mail_api.clone();
        url.set_path("/api/accounts/signature");
        let response = self
            .http
            .get(url)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Zoho-oauthtoken {}", token.expose()),
            )
            .send()
            .await?;
        let body: SignaturesBody = read_json(response).await?;
        Ok(body
            .data
            .into_iter()
            .filter_map(|signature| {
                let content = signature
                    .content
                    .filter(|content| !content.trim().is_empty())?;
                Some(ZohoSignature {
                    id: signature.id.map(String::from).unwrap_or_default(),
                    name: signature.name,
                    content,
                })
            })
            .collect())
    }
}
