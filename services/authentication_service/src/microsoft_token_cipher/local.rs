//! Envelope data keys derived from a locally configured master key, for
//! self-hosted deployments without AWS KMS.
//!
//! Each envelope stores a random salt where KMS stores its encrypted data key.
//! The data key is HKDF-SHA256(master, salt, encryption context), so an
//! envelope only opens with the same master key and the same owner binding.

use std::collections::HashMap;

use base64::{Engine, engine::general_purpose::STANDARD};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

use super::{
    AES_256_KEY_LENGTH, DataKeyProvider, DataKeyProviderError, GeneratedDataKey,
    append_length_prefixed,
};

#[cfg(test)]
mod test;

/// Recorded as the envelope's key id; a new master key gets a new version.
const LOCAL_KEY_ID: &str = "local:v1";
const SALT_LENGTH: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum LocalMasterKeyError {
    #[error("the local token encryption key is not valid base64")]
    NotBase64,
    #[error("the local token encryption key must decode to exactly 32 bytes")]
    WrongLength,
}

/// Derives envelope data keys from a 32-byte master key.
pub struct LocalDataKeyProvider {
    master_key: Zeroizing<Vec<u8>>,
}

impl LocalDataKeyProvider {
    /// Accepts the standard base64 encoding of exactly 32 random bytes.
    pub fn from_base64(encoded: &str) -> Result<Self, LocalMasterKeyError> {
        let master_key = Zeroizing::new(
            STANDARD
                .decode(encoded.trim())
                .map_err(|_| LocalMasterKeyError::NotBase64)?,
        );
        if master_key.len() != AES_256_KEY_LENGTH {
            return Err(LocalMasterKeyError::WrongLength);
        }
        Ok(Self { master_key })
    }

    fn derive(
        &self,
        salt: &[u8],
        encryption_context: &HashMap<String, String>,
    ) -> Result<Zeroizing<Vec<u8>>, DataKeyProviderError> {
        let mut entries = encryption_context.iter().collect::<Vec<_>>();
        entries.sort();
        let mut info = Vec::new();
        for (name, value) in entries {
            append_length_prefixed(&mut info, name.as_bytes());
            append_length_prefixed(&mut info, value.as_bytes());
        }
        let mut data_key = Zeroizing::new(vec![0; AES_256_KEY_LENGTH]);
        Hkdf::<Sha256>::new(Some(salt), &self.master_key)
            .expand(&info, &mut data_key)
            .map_err(|_| DataKeyProviderError::MalformedResponse)?;
        Ok(data_key)
    }
}

#[async_trait::async_trait]
impl DataKeyProvider for LocalDataKeyProvider {
    async fn generate_data_key(
        &self,
        encryption_context: HashMap<String, String>,
    ) -> Result<GeneratedDataKey, DataKeyProviderError> {
        let salt = rand::random::<[u8; SALT_LENGTH]>().to_vec();
        Ok(GeneratedDataKey {
            plaintext: self.derive(&salt, &encryption_context)?,
            encrypted: salt,
            key_id: LOCAL_KEY_ID.to_owned(),
        })
    }

    async fn decrypt_data_key(
        &self,
        key_id: &str,
        encrypted_data_key: &[u8],
        encryption_context: HashMap<String, String>,
    ) -> Result<Zeroizing<Vec<u8>>, DataKeyProviderError> {
        // A KMS envelope cannot be opened locally; fail instead of deriving a wrong key.
        if key_id != LOCAL_KEY_ID || encrypted_data_key.len() != SALT_LENGTH {
            return Err(DataKeyProviderError::DecryptFailed);
        }
        self.derive(encrypted_data_key, &encryption_context)
    }
}
