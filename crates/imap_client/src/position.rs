use email_api_client::domain::models::StreamToken;

use crate::ImapError;

/// The persisted progress of one folder stream.
///
/// While the first enumeration runs, `below` is the lowest UID already
/// listed; the next page lists older messages. Afterwards `below` is `None`
/// and `highest_modseq` / `uid_next` mark where the next round starts.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct FolderPosition {
    pub uid_validity: u32,
    pub highest_modseq: Option<u64>,
    pub uid_next: u32,
    pub below: Option<u32>,
}

impl FolderPosition {
    pub fn to_token(&self) -> Result<StreamToken, ImapError> {
        serde_json::to_string(self)
            .map(StreamToken::new)
            .map_err(|_| ImapError::MalformedIdentifier)
    }

    pub fn from_token(token: &StreamToken) -> Result<Self, ImapError> {
        serde_json::from_str(token.expose()).map_err(|_| ImapError::MalformedIdentifier)
    }
}
