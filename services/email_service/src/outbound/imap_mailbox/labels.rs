//! Labels on IMAP mailboxes are message keywords. Zoho keeps any keyword
//! (`PERMANENTFLAGS (\*)`), so there is no label catalog on the server.

use super::*;
use email_api_client::domain::models::CategoryMessage;

impl<T: MailboxTokenSource> ImapMailboxGateway<T> {
    /// Every message carrying `keyword`, in every folder, with its tags and
    /// version so the keyword can be removed by a version-checked write.
    #[tracing::instrument(name = "imap.keyword_messages", skip_all, fields(link_id = %mailbox.link_id), err)]
    pub async fn keyword_messages(
        &self,
        mailbox: MailboxKey,
        keyword: &str,
    ) -> Result<Vec<CategoryMessage>, EmailApiError> {
        let mut connection = self.connect(mailbox).await?;
        let result = async {
            let folders = connection.folders().await?;
            let mut messages = Vec::new();
            for folder in &folders {
                for id in connection.with_keyword(folder.id.as_str(), keyword).await? {
                    if let Some(organization) = connection.organization(&id, &folders).await? {
                        messages.push(CategoryMessage {
                            id: id.to_provider_id()?,
                            categories: organization.tags,
                            version: organization.version,
                        });
                    }
                }
            }
            Ok(messages)
        }
        .await;
        connection.logout().await;
        result
    }
}
