//! One projection worker serves every stream-synced provider; attachment
//! downloads go to the gateway of the link's own provider.

use email::domain::mailbox::{MailboxError, MailboxKey, projection::MailboxAttachmentAccess};
use sqlx::PgPool;

#[derive(Clone)]
pub struct ProviderMailboxAttachments<O, I> {
    pub db: PgPool,
    pub outlook: O,
    pub imap: I,
}

impl<O: MailboxAttachmentAccess, I: MailboxAttachmentAccess> MailboxAttachmentAccess
    for ProviderMailboxAttachments<O, I>
{
    async fn download(
        &self,
        mailbox: MailboxKey,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, MailboxError> {
        let provider = sqlx::query_scalar!(
            "SELECT provider::text AS \"provider!\" FROM email_links WHERE id = $1",
            mailbox.link_id
        )
        .fetch_optional(&self.db)
        .await
        .map_err(|_| MailboxError::Persistence)?
        .ok_or(MailboxError::Stale)?;
        if provider == "IMAP" {
            self.imap.download(mailbox, message_id, attachment_id).await
        } else {
            self.outlook
                .download(mailbox, message_id, attachment_id)
                .await
        }
    }
}
