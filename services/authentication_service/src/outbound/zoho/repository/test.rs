use super::*;
use chrono::{Duration, Utc};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use zoho_client::ZohoRefreshToken;

async fn owner(db: &PgPool) -> Uuid {
    let id = macro_uuid::generate_uuid_v7();
    sqlx::query!("INSERT INTO macro_user (id, username, email, stripe_customer_id) VALUES ($1, 'zoho-tester', 'zoho-owner@example.com', 'cus_zoho_test')", id)
        .execute(db)
        .await
        .unwrap();
    id
}

fn pending(owner: Uuid) -> ZohoLinkAttempt {
    ZohoLinkAttempt {
        id: macro_uuid::generate_uuid_v7(),
        owner,
        data_center: ZohoDataCenter::Eu,
        servers: zoho_client::ZohoServerChoice::Standard,
        redirect_uri: "https://auth.example.com/oauth2/zoho/callback".into(),
        return_uri: None,
        verifier: Zeroizing::new("verifier".into()),
        expires_at: Utc::now() + Duration::minutes(10),
    }
}

fn identity(zuid: &str) -> VerifiedZohoGrant {
    VerifiedZohoGrant {
        zuid: zuid.into(),
        account_id: "2560636000000008002".into(),
        email: "zoho-test@example.com".into(),
        mailbox_address: "zoho-mailbox@example.com".into(),
        send_addresses: vec!["zoho-test@example.com".into(), "info@example.com".into()],
        scopes: vec!["ZOHOMAIL.PROTOCOL.IMAP".into()],
        refresh_token: ZohoRefreshToken::new("refresh".into()),
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

async fn link(repo: &PgZohoGrants, owner: Uuid, zuid: &str) -> Result<Uuid, ZohoAuthError> {
    let attempt = pending(owner);
    repo.begin_link(&attempt).await?;
    let attempt = repo.claim_link(attempt.id).await?;
    let grant = macro_uuid::generate_uuid_v7();
    repo.complete_link(&attempt, &identity(zuid), &envelope(), grant)
        .await?;
    Ok(attempt.id)
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn an_attempt_completes_once_and_reconnects_add_a_generation(db: PgPool) {
    let repo = PgZohoGrants::new(db.clone());
    let owner = owner(&db).await;

    let first = link(&repo, owner, "20069488").await.unwrap();
    let second = link(&repo, owner, "20069488").await.unwrap();

    assert_eq!(
        repo.claim_link(first).await.err(),
        Some(ZohoAuthError::InvalidAttempt)
    );
    let first = repo.completed_grant(first, owner).await.unwrap();
    let second = repo.completed_grant(second, owner).await.unwrap();
    assert_eq!(first.generation, 1);
    assert_eq!(second.generation, 2);
    assert_eq!(second.data_center, ZohoDataCenter::Eu);
    assert_eq!(second.account_id, "2560636000000008002");
    assert_eq!(
        second.send_addresses,
        ["zoho-test@example.com", "info@example.com"]
    );
    assert_eq!(
        second.mailbox_address.as_deref(),
        Some("zoho-mailbox@example.com")
    );
    assert_eq!(second.servers, zoho_client::ZohoServerChoice::Standard);
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn another_zoho_user_behind_a_known_address_is_refused(db: PgPool) {
    let repo = PgZohoGrants::new(db.clone());
    let owner = owner(&db).await;
    link(&repo, owner, "20069488").await.unwrap();

    let result = link(&repo, owner, "99999999").await;

    assert_eq!(result.err(), Some(ZohoAuthError::OwnershipConflict));
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn only_the_bound_generation_refreshes_and_a_revoked_grant_stops(db: PgPool) {
    let repo = PgZohoGrants::new(db.clone());
    let owner = owner(&db).await;
    let attempt = link(&repo, owner, "20069488").await.unwrap();
    let grant = repo.completed_grant(attempt, owner).await.unwrap();
    let link_id = macro_uuid::generate_uuid_v7();
    sqlx::query!("INSERT INTO email_links(id,macro_id,fusionauth_user_id,email_address,provider,grant_id,grant_generation) VALUES ($1,'macro|zoho-test@example.com',$2,'zoho-test@example.com','IMAP',$3,1)", link_id, owner.to_string(), grant.grant_id)
        .execute(&db)
        .await
        .unwrap();

    assert!(repo.active_grant(link_id, 2, 1).await.is_err());
    let stored = repo.active_grant(link_id, 1, 1).await.unwrap();
    let lease = macro_uuid::generate_uuid_v7();
    assert!(repo.acquire_refresh(&stored, lease).await.unwrap());
    assert!(
        !repo
            .acquire_refresh(&stored, macro_uuid::generate_uuid_v7())
            .await
            .unwrap()
    );
    assert!(repo.finish_refresh(&stored, lease).await.unwrap());

    let refreshed = repo.active_grant(link_id, 1, 1).await.unwrap();
    assert_eq!(refreshed.revision, 1);
    let lease = macro_uuid::generate_uuid_v7();
    assert!(repo.acquire_refresh(&refreshed, lease).await.unwrap());
    repo.release_refresh(&refreshed, lease, true).await.unwrap();
    assert_eq!(
        repo.active_grant(link_id, 1, 1).await.err(),
        Some(ZohoAuthError::ReauthorizationRequired)
    );
}
