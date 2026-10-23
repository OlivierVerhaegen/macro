use super::*;
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use uuid::Uuid;

fn envelope(seed: u8) -> EncryptedMicrosoftToken {
    EncryptedMicrosoftToken {
        refresh_token_ciphertext: vec![seed; 32],
        encrypted_data_key: vec![seed; 32],
        nonce: vec![seed; 12],
        encryption_version: 1,
        kms_key_id: "local:v1".into(),
    }
}

async fn imap_link(db: &PgPool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query!("INSERT INTO email_links(id,macro_id,fusionauth_user_id,email_address,provider,grant_generation,is_sync_active) VALUES ($1,'macro|Olivier@Example.be','owner','Olivier@Example.be','IMAP',1,true)", id)
        .execute(db)
        .await
        .unwrap();
    id
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn only_the_active_binding_resolves_and_a_new_password_replaces_the_old(db: PgPool) {
    let repo = PgSmtpPasswords::new(db.clone());
    let link_id = imap_link(&db).await;
    let binding = MailboxBinding {
        link_id,
        grant_generation: 1,
        sync_generation: 1,
    };

    assert_eq!(
        repo.owner(MailboxBinding {
            sync_generation: 2,
            ..binding
        })
        .await
        .err(),
        Some(SmtpPasswordError::NotConnected)
    );
    let owner = repo.owner(binding).await.unwrap();
    assert_eq!(owner.email, "olivier@example.be");
    assert_eq!(repo.load(&owner).await.unwrap(), None);

    repo.store(&owner, &envelope(1)).await.unwrap();
    repo.store(&owner, &envelope(2)).await.unwrap();

    assert_eq!(repo.load(&owner).await.unwrap(), Some(envelope(2)));
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn a_password_is_collected_once_its_last_imap_link_is_gone(db: PgPool) {
    use crate::domain::zoho::ZohoGrantRepository;

    let repo = PgSmtpPasswords::new(db.clone());
    let grants = crate::outbound::zoho::PgZohoGrants::new(db.clone());
    let link_id = imap_link(&db).await;
    let owner = repo
        .owner(MailboxBinding {
            link_id,
            grant_generation: 1,
            sync_generation: 1,
        })
        .await
        .unwrap();
    repo.store(&owner, &envelope(1)).await.unwrap();

    grants
        .collect_expired_grants(chrono::Utc::now(), 10)
        .await
        .unwrap();
    assert_eq!(repo.load(&owner).await.unwrap(), Some(envelope(1)));

    sqlx::query!("DELETE FROM email_links WHERE id = $1", link_id)
        .execute(&db)
        .await
        .unwrap();
    grants
        .collect_expired_grants(chrono::Utc::now(), 10)
        .await
        .unwrap();
    assert_eq!(repo.load(&owner).await.unwrap(), None);
}
