use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, header, method, path},
};

use crate::*;

fn endpoints(server: &MockServer) -> ZohoEndpoints {
    let origin = url::Url::parse(&server.uri()).expect("mock server uri should parse");
    ZohoEndpoints {
        accounts: origin.clone(),
        mail_api: origin,
    }
}

fn oauth_client() -> ZohoOAuthClient {
    ZohoOAuthClient::new(ZohoClientCredentials::new(
        "client-id".to_owned(),
        "client-secret".to_owned(),
    ))
    .expect("client should build")
}

#[test]
fn data_center_endpoints_follow_the_zoho_domain() {
    let eu = ZohoEndpoints::for_data_center(ZohoDataCenter::Eu);
    let canada = ZohoEndpoints::for_data_center(ZohoDataCenter::Ca);

    assert_eq!(eu.accounts.as_str(), "https://accounts.zoho.eu/");
    assert_eq!(eu.mail_api.as_str(), "https://mail.zoho.eu/");
    assert_eq!(ZohoDataCenter::Eu.imap_host(), "imap.zoho.eu");
    assert_eq!(canada.accounts.as_str(), "https://accounts.zohocloud.ca/");
    assert_eq!(
        "com.au".parse::<ZohoDataCenter>().ok(),
        Some(ZohoDataCenter::ComAu)
    );
}

#[test]
fn authorization_url_requests_offline_consent_with_pkce() {
    let url = oauth_client().authorization_url(
        &ZohoEndpoints::for_data_center(ZohoDataCenter::Eu),
        &ZohoAuthorizationRequest {
            redirect_uri: "https://macro.example.be/oauth2/zoho/callback",
            state: "signed-state",
            code_challenge: "challenge",
            scopes: &scopes::MAILBOX,
        },
    );
    let query = url.query_pairs().into_owned().collect::<Vec<_>>();
    let get = |name: &str| {
        query
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };

    assert_eq!(url.host_str(), Some("accounts.zoho.eu"));
    assert_eq!(url.path(), "/oauth/v2/auth");
    assert_eq!(get("access_type"), Some("offline"));
    assert_eq!(get("prompt"), Some("consent"));
    assert_eq!(get("code_challenge_method"), Some("S256"));
    assert_eq!(
        get("scope"),
        Some(
            "ZOHOMAIL.PROTOCOL.IMAP,ZohoMail.accounts.READ,ZohoMail.messages.ALL,ZohoMail.folders.ALL"
        )
    );
}

#[tokio::test]
async fn code_exchange_returns_both_tokens() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/v2/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code_verifier=verifier"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "expires_in": 3600,
            "token_type": "Bearer"
        })))
        .mount(&server)
        .await;

    let tokens = oauth_client()
        .exchange_code(&endpoints(&server), "code", "https://redirect", "verifier")
        .await
        .expect("exchange should succeed");

    assert_eq!(tokens.access_token.expose(), "access");
    assert_eq!(
        tokens.refresh_token.as_ref().map(ZohoRefreshToken::expose),
        Some("refresh")
    );
    assert_eq!(tokens.expires_in, 3600);
}

#[tokio::test]
async fn an_error_body_with_http_200_is_a_rejected_grant() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/v2/token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"error": "invalid_code"})),
        )
        .mount(&server)
        .await;

    let result = oauth_client()
        .refresh(
            &endpoints(&server),
            &ZohoRefreshToken::new("revoked".to_owned()),
        )
        .await;

    assert!(matches!(result, Err(ZohoError::Unauthorized)));
}

#[tokio::test]
async fn an_unknown_oauth_error_is_kept() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/v2/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"error": "invalid_client"})),
        )
        .mount(&server)
        .await;

    let result = oauth_client()
        .refresh(
            &endpoints(&server),
            &ZohoRefreshToken::new("token".to_owned()),
        )
        .await;

    assert!(matches!(result, Err(ZohoError::OAuth(code)) if code == "invalid_client"));
}

#[tokio::test]
async fn accounts_accept_numeric_ids_and_collect_send_addresses() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/accounts"))
        .and(header("authorization", "Zoho-oauthtoken access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": {"code": 200, "description": "success"},
            "data": [{
                "accountId": 2560636000000008002_u64,
                "zuid": "20069488",
                "primaryEmailAddress": "Olivier@Example.be",
                "sendMailDetails": [
                    {"fromAddress": "olivier@example.be"},
                    {"fromAddress": "Hello@Example.be"}
                ]
            }]
        })))
        .mount(&server)
        .await;

    let accounts = ZohoMailClient::new()
        .expect("client should build")
        .accounts(
            &endpoints(&server),
            &ZohoAccessToken::new("access".to_owned()),
        )
        .await
        .expect("accounts should load");

    assert_eq!(
        accounts,
        vec![ZohoMailAccount {
            account_id: "2560636000000008002".to_owned(),
            zuid: "20069488".to_owned(),
            primary_email_address: "olivier@example.be".to_owned(),
            send_addresses: vec![
                "olivier@example.be".to_owned(),
                "hello@example.be".to_owned()
            ],
        }]
    );
}

#[tokio::test]
async fn an_expired_access_token_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/accounts"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let result = ZohoMailClient::new()
        .expect("client should build")
        .accounts(
            &endpoints(&server),
            &ZohoAccessToken::new("expired".to_owned()),
        )
        .await;

    assert!(matches!(result, Err(ZohoError::Unauthorized)));
}
