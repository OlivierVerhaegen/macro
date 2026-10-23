use super::*;
use crate::microsoft_token_cipher::{EnvelopeMicrosoftTokenCipher, local::LocalDataKeyProvider};

// base64 of 32 bytes 0x00..=0x1f.
const MASTER_KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

fn cipher() -> Arc<dyn MicrosoftTokenCipher> {
    Arc::new(
        EnvelopeMicrosoftTokenCipher::new(
            LocalDataKeyProvider::from_base64(MASTER_KEY).expect("test key is valid"),
        )
        .with_purpose(SMTP_PASSWORD_PURPOSE),
    )
}

fn binding() -> MailboxBinding {
    MailboxBinding {
        link_id: Uuid::nil(),
        grant_generation: 1,
        sync_generation: 1,
    }
}

fn owner() -> SmtpPasswordOwner {
    SmtpPasswordOwner {
        fusionauth_user_id: "owner".into(),
        email: "olivier@example.be".into(),
    }
}

#[tokio::test]
async fn a_stored_password_reads_back_for_the_same_binding() {
    let stored = Arc::new(std::sync::Mutex::new(None::<EncryptedMicrosoftToken>));
    let mut repo = MockSmtpPasswordRepository::new();
    repo.expect_owner().returning(|_| Ok(owner()));
    let saved = stored.clone();
    repo.expect_store().once().returning(move |_, envelope| {
        *saved.lock().unwrap() = Some(envelope.clone());
        Ok(())
    });
    let loaded = stored.clone();
    repo.expect_load()
        .once()
        .returning(move |_| Ok(loaded.lock().unwrap().clone()));
    let service = SmtpPasswordService::new(repo, cipher());

    service
        .store(binding(), Zeroizing::new("app-password".into()))
        .await
        .unwrap();
    let password = service.password(binding()).await.unwrap();

    assert_eq!(password.as_str(), "app-password");
    let envelope = stored.lock().unwrap().clone().unwrap();
    assert_ne!(envelope.refresh_token_ciphertext, b"app-password");
}

#[tokio::test]
async fn a_missing_password_is_reported() {
    let mut repo = MockSmtpPasswordRepository::new();
    repo.expect_owner().returning(|_| Ok(owner()));
    repo.expect_load().returning(|_| Ok(None));
    let service = SmtpPasswordService::new(repo, cipher());

    assert_eq!(
        service.password(binding()).await.err(),
        Some(SmtpPasswordError::Missing)
    );
}

#[tokio::test]
async fn a_stale_binding_cannot_store() {
    let mut repo = MockSmtpPasswordRepository::new();
    repo.expect_owner()
        .returning(|_| Err(SmtpPasswordError::NotConnected));
    let service = SmtpPasswordService::new(repo, cipher());

    assert_eq!(
        service
            .store(binding(), Zeroizing::new("app-password".into()))
            .await
            .err(),
        Some(SmtpPasswordError::NotConnected)
    );
}
