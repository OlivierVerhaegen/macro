use super::*;
use crate::domain::microsoft::token::MockMicrosoftTokenCipher;

fn attempt() -> ZohoLinkAttempt {
    ZohoLinkAttempt {
        id: macro_uuid::generate_uuid_v7(),
        owner: macro_uuid::generate_uuid_v7(),
        data_center: ZohoDataCenter::Eu,
        redirect_uri: "https://auth.example.com/oauth2/zoho/callback".into(),
        return_uri: Some("https://app.example.com".into()),
        verifier: Zeroizing::new("verifier".into()),
        expires_at: Utc::now() + Duration::minutes(10),
    }
}

fn envelope() -> EncryptedMicrosoftToken {
    EncryptedMicrosoftToken {
        refresh_token_ciphertext: vec![1; 32],
        encrypted_data_key: vec![2; 32],
        nonce: vec![3; 12],
        encryption_version: 1,
        kms_key_id: "local:v1".into(),
    }
}

fn mailbox_scopes() -> Vec<String> {
    zoho_client::scopes::MAILBOX.map(str::to_owned).to_vec()
}

fn identity(scopes: Vec<String>) -> VerifiedZohoGrant {
    VerifiedZohoGrant {
        zuid: "20069488".into(),
        account_id: "2560636000000008002".into(),
        email: "olivier@example.be".into(),
        send_addresses: vec!["olivier@example.be".into()],
        scopes,
        refresh_token: ZohoRefreshToken::new("refresh".into()),
    }
}

fn stored_grant() -> StoredZohoGrant {
    StoredZohoGrant {
        id: macro_uuid::generate_uuid_v7(),
        generation: 1,
        revision: 0,
        owner: "owner".into(),
        email: "olivier@example.be".into(),
        data_center: ZohoDataCenter::Eu,
        account_id: "2560636000000008002".into(),
        scopes: mailbox_scopes(),
        envelope: envelope(),
    }
}

#[tokio::test]
async fn completed_link_is_encrypted_for_the_initiating_owner() {
    let pending = attempt();
    let id = pending.id;
    let owner = pending.owner.to_string();
    let mut repo = MockZohoGrantRepository::new();
    repo.expect_claim_link()
        .once()
        .return_once(move |_| Ok(pending));
    repo.expect_complete_link()
        .once()
        .withf(|_, identity, _, _| identity.account_id == "2560636000000008002")
        .returning(|_, _, _, _| Ok(()));
    let mut provider = MockZohoIdentityProvider::new();
    provider
        .expect_exchange()
        .once()
        .returning(|_, _| Ok(identity(mailbox_scopes())));
    let mut cipher = MockMicrosoftTokenCipher::new();
    cipher
        .expect_encrypt()
        .once()
        .withf(move |encrypted_for, email, token| {
            encrypted_for == owner && email == "olivier@example.be" && token.as_str() == "refresh"
        })
        .returning(|_, _, _| Ok(envelope()));
    let service = ZohoAuthService::new(repo, provider, Arc::new(cipher));

    assert_eq!(
        service.complete_link(id, "code").await,
        Ok(Some("https://app.example.com".into()))
    );
}

#[tokio::test]
async fn a_consent_without_imap_access_is_abandoned() {
    let pending = attempt();
    let id = pending.id;
    let mut repo = MockZohoGrantRepository::new();
    repo.expect_claim_link()
        .once()
        .return_once(move |_| Ok(pending));
    repo.expect_abandon_link().once().returning(|_| Ok(()));
    let mut provider = MockZohoIdentityProvider::new();
    provider
        .expect_exchange()
        .once()
        .returning(|_, _| Ok(identity(vec!["ZohoMail.accounts.READ".into()])));
    let service = ZohoAuthService::new(repo, provider, Arc::new(MockMicrosoftTokenCipher::new()));

    assert_eq!(
        service.complete_link(id, "code").await,
        Err(ZohoAuthError::InvalidIdentity)
    );
}

#[tokio::test]
async fn a_cached_token_is_reused_but_the_binding_is_checked_every_time() {
    let mut repo = MockZohoGrantRepository::new();
    let grant_id = macro_uuid::generate_uuid_v7();
    repo.expect_active_grant()
        .times(2)
        .returning(move |_, _, _| {
            Ok(StoredZohoGrant {
                id: grant_id,
                ..stored_grant()
            })
        });
    repo.expect_acquire_refresh()
        .once()
        .returning(|_, _| Ok(true));
    repo.expect_finish_refresh()
        .once()
        .returning(|_, _| Ok(true));
    let mut provider = MockZohoIdentityProvider::new();
    provider.expect_refresh().once().returning(|_, _| {
        Ok(RefreshedZohoToken {
            access_token: Zeroizing::new("access".into()),
            expires_in: 3600,
        })
    });
    let mut cipher = MockMicrosoftTokenCipher::new();
    cipher
        .expect_decrypt()
        .once()
        .returning(|_, _, _| Ok(MicrosoftRefreshToken::new("refresh".into())));
    let service = ZohoAuthService::new(repo, provider, Arc::new(cipher));
    let link = macro_uuid::generate_uuid_v7();

    let first = service
        .access_token(link, 1, 1, false)
        .await
        .expect("first token");
    let second = service
        .access_token(link, 1, 1, false)
        .await
        .expect("cached token");

    assert_eq!(first.as_str(), "access");
    assert_eq!(second.as_str(), "access");
    assert_eq!(second.data_center, ZohoDataCenter::Eu);
}

#[tokio::test]
async fn a_rejected_refresh_token_revokes_the_grant() {
    let mut repo = MockZohoGrantRepository::new();
    repo.expect_active_grant()
        .once()
        .returning(|_, _, _| Ok(stored_grant()));
    repo.expect_acquire_refresh()
        .once()
        .returning(|_, _| Ok(true));
    repo.expect_release_refresh()
        .once()
        .withf(|_, _, revoke| *revoke)
        .returning(|_, _, _| Ok(()));
    let mut provider = MockZohoIdentityProvider::new();
    provider
        .expect_refresh()
        .once()
        .returning(|_, _| Err(ZohoAuthError::ReauthorizationRequired));
    let mut cipher = MockMicrosoftTokenCipher::new();
    cipher
        .expect_decrypt()
        .once()
        .returning(|_, _, _| Ok(MicrosoftRefreshToken::new("refresh".into())));
    let service = ZohoAuthService::new(repo, provider, Arc::new(cipher));

    let result = service
        .access_token(macro_uuid::generate_uuid_v7(), 1, 1, false)
        .await;

    assert!(matches!(
        result,
        Err(ZohoAuthError::ReauthorizationRequired)
    ));
}
