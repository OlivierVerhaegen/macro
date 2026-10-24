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
    assert_eq!(
        ZohoDataCenter::Eu.mail_servers("tech@olivetrees-agency.be", ZohoServerChoice::Auto),
        ZohoMailServers {
            imap_host: "imappro.zoho.eu".to_owned(),
            smtp_host: "smtppro.zoho.eu".to_owned(),
        }
    );
    assert_eq!(
        ZohoDataCenter::Com.mail_servers("someone@ZohoMail.com", ZohoServerChoice::Auto),
        ZohoMailServers {
            imap_host: "imap.zoho.com".to_owned(),
            smtp_host: "smtp.zoho.com".to_owned(),
        }
    );
    assert_eq!(
        ZohoDataCenter::Eu
            .mail_servers("someone@zoho.eu", ZohoServerChoice::Auto)
            .imap_host,
        "imap.zoho.eu"
    );
    assert_eq!(
        ZohoDataCenter::Eu
            .mail_servers("tech@olivetrees-agency.be", ZohoServerChoice::Standard)
            .smtp_host,
        "smtp.zoho.eu"
    );
    assert_eq!(
        ZohoDataCenter::Eu
            .mail_servers("someone@zohomail.eu", ZohoServerChoice::Pro)
            .imap_host,
        "imappro.zoho.eu"
    );
    assert_eq!(
        "standard".parse::<ZohoServerChoice>().ok(),
        Some(ZohoServerChoice::Standard)
    );
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
                "mailboxAddress": "Tech@Example.be",
                "sendMailDetails": [
                    {"fromAddress": "tech@example.be", "validated": false, "status": true},
                    {"fromAddress": "olivier@example.be", "status": true},
                    {"fromAddress": "Hello@Example.be"},
                    {"fromAddress": "old@example.be", "status": false}
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
            mailbox_address: "tech@example.be".to_owned(),
            send_addresses: vec![
                "olivier@example.be".to_owned(),
                "tech@example.be".to_owned(),
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

#[tokio::test]
async fn signatures_keep_saved_content_and_skip_empty_ones() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/accounts/signature"))
        .and(header("authorization", "Zoho-oauthtoken access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": {"code": 200, "description": "success"},
            "data": [
                {"id": 1842613000000105001_u64, "name": "Kantoor", "content": "<div>Olivier</div>"},
                {"id": "1842613000000105002", "name": "Leeg", "content": "  "}
            ]
        })))
        .mount(&server)
        .await;

    let signatures = ZohoMailClient::new()
        .expect("client should build")
        .signatures(
            &endpoints(&server),
            &ZohoAccessToken::new("access".to_owned()),
        )
        .await
        .expect("signatures should load");

    assert_eq!(
        signatures,
        vec![ZohoSignature {
            id: "1842613000000105001".to_owned(),
            name: Some("Kantoor".to_owned()),
            content: "<div>Olivier</div>".to_owned(),
        }]
    );
}
