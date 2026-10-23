use super::*;
use crate::microsoft_token_cipher::{
    EncryptedMicrosoftToken, EnvelopeMicrosoftTokenCipher, MicrosoftRefreshToken,
    MicrosoftTokenCipher,
};

// base64 of 32 bytes 0x00..=0x1f.
const MASTER_KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
// base64 of 32 bytes 0x20..=0x3f.
const OTHER_MASTER_KEY: &str = "ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8=";
const OWNER: &str = "fusionauth-user-id";
const MAILBOX: &str = "person@example.com";
const REFRESH_TOKEN: &str = "private-zoho-refresh-token";

fn cipher(key: &str, purpose: &'static str) -> EnvelopeMicrosoftTokenCipher<LocalDataKeyProvider> {
    EnvelopeMicrosoftTokenCipher::new(
        LocalDataKeyProvider::from_base64(key).expect("test key should be valid"),
    )
    .with_purpose(purpose)
}

async fn encrypt(
    cipher: &EnvelopeMicrosoftTokenCipher<LocalDataKeyProvider>,
) -> EncryptedMicrosoftToken {
    cipher
        .encrypt(
            OWNER,
            MAILBOX,
            MicrosoftRefreshToken::new(REFRESH_TOKEN.to_owned()),
        )
        .await
        .expect("token should encrypt")
}

#[tokio::test]
async fn round_trip_records_a_local_key_id_and_salt() {
    let cipher = cipher(MASTER_KEY, "zoho-refresh-token");

    let envelope = encrypt(&cipher).await;
    let plaintext = cipher
        .decrypt(OWNER, MAILBOX, &envelope)
        .await
        .expect("token should decrypt");

    assert_eq!(plaintext.as_str(), REFRESH_TOKEN);
    assert_eq!(envelope.kms_key_id, "local:v1");
    assert_eq!(envelope.encrypted_data_key.len(), 32);
}

#[tokio::test]
async fn every_envelope_gets_a_fresh_salt() {
    let cipher = cipher(MASTER_KEY, "zoho-refresh-token");

    let first = encrypt(&cipher).await;
    let second = encrypt(&cipher).await;

    assert_ne!(first.encrypted_data_key, second.encrypted_data_key);
    assert_ne!(
        first.refresh_token_ciphertext,
        second.refresh_token_ciphertext
    );
}

#[tokio::test]
async fn another_master_key_cannot_decrypt() {
    let envelope = encrypt(&cipher(MASTER_KEY, "zoho-refresh-token")).await;

    let result = cipher(OTHER_MASTER_KEY, "zoho-refresh-token")
        .decrypt(OWNER, MAILBOX, &envelope)
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn another_purpose_or_owner_cannot_decrypt() {
    let envelope = encrypt(&cipher(MASTER_KEY, "zoho-refresh-token")).await;

    let other_purpose = cipher(MASTER_KEY, "microsoft-refresh-token")
        .decrypt(OWNER, MAILBOX, &envelope)
        .await;
    let other_owner = cipher(MASTER_KEY, "zoho-refresh-token")
        .decrypt("another-user", MAILBOX, &envelope)
        .await;

    assert!(other_purpose.is_err());
    assert!(other_owner.is_err());
}

#[tokio::test]
async fn kms_envelopes_are_rejected() {
    let mut envelope = encrypt(&cipher(MASTER_KEY, "zoho-refresh-token")).await;
    envelope.kms_key_id = "arn:aws:kms:eu-west-1:000000000000:key/example".to_owned();

    let result = cipher(MASTER_KEY, "zoho-refresh-token")
        .decrypt(OWNER, MAILBOX, &envelope)
        .await;

    assert!(result.is_err());
}

#[test]
fn master_key_must_be_32_bytes_of_base64() {
    assert!(matches!(
        LocalDataKeyProvider::from_base64("not base64!"),
        Err(LocalMasterKeyError::NotBase64)
    ));
    assert!(matches!(
        LocalDataKeyProvider::from_base64("AAECAw=="),
        Err(LocalMasterKeyError::WrongLength)
    ));
}
