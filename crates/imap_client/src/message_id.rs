use email_api_client::domain::models::ProviderId;

use crate::ImapError;

/// Where a message lives: IMAP UIDs are only unique within one folder and
/// one UIDVALIDITY. A move gives the message a new id, as on Outlook.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImapMessageId {
    /// The folder's UIDVALIDITY when the UID was read.
    pub uid_validity: u32,
    /// The message UID.
    pub uid: u32,
    /// The raw (modified UTF-7) folder name.
    pub folder: String,
}

impl ImapMessageId {
    /// Encode as the opaque provider id stored with the message.
    pub fn to_provider_id(&self) -> Result<ProviderId, ImapError> {
        let encoded = serde_json::to_string(&(self.uid_validity, self.uid, &self.folder))
            .map_err(|_| ImapError::MalformedIdentifier)?;
        ProviderId::new(encoded).map_err(|_| ImapError::MalformedIdentifier)
    }

    /// Decode an id produced by [`Self::to_provider_id`].
    pub fn from_provider_id(id: &ProviderId) -> Result<Self, ImapError> {
        let (uid_validity, uid, folder): (u32, u32, String) =
            serde_json::from_str(id.as_str()).map_err(|_| ImapError::MalformedIdentifier)?;
        Ok(Self {
            uid_validity,
            uid,
            folder,
        })
    }
}
