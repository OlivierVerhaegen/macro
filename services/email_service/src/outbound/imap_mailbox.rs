//! IMAP mailboxes (Zoho Mail and other IMAP servers) for the email domain's
//! stream runtime. Server settings come from `imap_accounts`; Zoho access
//! tokens from the authentication service through the credential health port.

use email::domain::mailbox::{MailboxError, MailboxGateway, MailboxKey};
use email_api_client::domain::{
    models::{
        EmailApiError, MailFolder, MailboxAccess, MailboxChangePage, MailboxMessage, ProviderId,
        StreamPosition, StreamToken, TokenError, TokenFreshness,
    },
    ports::MailboxTokenSource,
};
use imap_client::{ImapConnection, ImapCredentials, ImapError, ImapMessageId, ImapServer};
use sqlx::PgPool;
use uuid::Uuid;
use zeroize::Zeroizing;

mod credentials;
pub use credentials::ZohoCredentialsClient;

#[cfg(test)]
mod test;

/// Messages listed per page during a folder's first enumeration.
const PAGE_SIZE: usize = 200;

#[derive(Clone)]
pub struct ImapMailboxGateway<T> {
    db: PgPool,
    tokens: T,
}

impl<T> ImapMailboxGateway<T> {
    pub fn new(db: PgPool, tokens: T) -> Self {
        Self { db, tokens }
    }
}

struct AccountSettings {
    host: String,
    port: u16,
    username: String,
}

fn access(mailbox: MailboxKey) -> MailboxAccess {
    MailboxAccess {
        link_id: mailbox.link_id,
        sync_generation: mailbox.sync_generation,
        grant_generation: mailbox.grant_generation,
    }
}

fn transient(message: &str) -> EmailApiError {
    EmailApiError::Transient {
        message: message.to_owned(),
    }
}

fn token_error(error: TokenError) -> EmailApiError {
    match error {
        TokenError::ReauthRequired => EmailApiError::AuthRequired,
        _ => transient("IMAP credentials are temporarily unavailable"),
    }
}

impl<T: MailboxTokenSource> ImapMailboxGateway<T> {
    async fn settings(&self, link_id: Uuid) -> Result<AccountSettings, EmailApiError> {
        let row = sqlx::query!(
            "SELECT imap_host, imap_port, username, auth_method FROM imap_accounts WHERE link_id = $1",
            link_id
        )
        .fetch_optional(&self.db)
        .await
        .map_err(|error| {
            tracing::error!(?error, %link_id, "failed to load IMAP account settings");
            transient("IMAP account settings are unavailable")
        })?
        .ok_or_else(|| EmailApiError::Permanent {
            message: "the IMAP account has no server settings".into(),
        })?;
        // Password accounts are stored by the schema but not yet connectable.
        if row.auth_method != "zoho_oauth" {
            return Err(EmailApiError::Permanent {
                message: "password IMAP accounts are not supported yet".into(),
            });
        }
        Ok(AccountSettings {
            host: row.imap_host,
            port: u16::try_from(row.imap_port).map_err(|_| EmailApiError::Permanent {
                message: "invalid IMAP port".into(),
            })?,
            username: row.username,
        })
    }

    /// Connect with a cached token, and once more with a fresh one when the
    /// server rejects it: a cached token can outlive a revoked grant.
    async fn connect(&self, mailbox: MailboxKey) -> Result<ImapConnection, EmailApiError> {
        let settings = self.settings(mailbox.link_id).await?;
        let server = ImapServer::new(settings.host, settings.port);
        for freshness in [TokenFreshness::Cached, TokenFreshness::Fresh] {
            let token = self
                .tokens
                .access_token(access(mailbox), freshness)
                .await
                .map_err(token_error)?;
            let credentials = ImapCredentials::XOAuth2 {
                username: settings.username.clone(),
                access_token: Zeroizing::new(token.expose_secret().to_owned()),
            };
            match ImapConnection::connect(&server, &credentials).await {
                Err(ImapError::AuthenticationFailed) if freshness == TokenFreshness::Cached => {}
                result => return result.map_err(EmailApiError::from),
            }
        }
        Err(EmailApiError::AuthRequired)
    }

    /// Without QRESYNC the server does not report expunges. When the folder
    /// holds fewer messages than Macro stored, list its UIDs and report the
    /// stored ids that are gone; the runtime reconciles before removing.
    async fn expunged(
        &self,
        connection: &mut ImapConnection,
        mailbox: MailboxKey,
        folder: &ProviderId,
        exists: u32,
    ) -> Result<Vec<ProviderId>, EmailApiError> {
        let stored = sqlx::query_scalar!(
            r#"SELECT provider_id AS "provider_id!" FROM email_messages
            WHERE link_id = $1 AND provider_folder_id = $2 AND provider_id IS NOT NULL
                AND NOT COALESCE((mailbox_state->>'provider_missing')::bool, false)"#,
            mailbox.link_id,
            folder.as_str()
        )
        .fetch_all(&self.db)
        .await
        .map_err(|error| {
            tracing::error!(?error, "failed to load stored IMAP message ids");
            transient("stored IMAP message ids are unavailable")
        })?;
        if stored.len() <= exists as usize {
            return Ok(Vec::new());
        }
        let present = connection
            .folder_uids(folder)
            .await
            .map_err(EmailApiError::from)?;
        Ok(gone(stored, &present))
    }
}

/// Stored ids whose UID is no longer in the folder, or whose folder was recreated.
fn gone(stored: Vec<String>, present: &imap_client::FolderUids) -> Vec<ProviderId> {
    stored
        .into_iter()
        .filter_map(|id| ProviderId::new(id).ok())
        .filter(|id| {
            ImapMessageId::from_provider_id(id).is_ok_and(|message| {
                message.uid_validity != present.uid_validity || !present.uids.contains(&message.uid)
            })
        })
        .collect()
}

impl<T: MailboxTokenSource> MailboxGateway for ImapMailboxGateway<T> {
    #[tracing::instrument(name = "imap.mailbox.folders", skip_all, fields(link_id = %mailbox.link_id), err)]
    async fn folders(&self, mailbox: MailboxKey) -> Result<Vec<MailFolder>, EmailApiError> {
        let mut connection = self.connect(mailbox).await?;
        let folders = connection.folders().await.map_err(EmailApiError::from);
        connection.logout().await;
        folders
    }

    #[tracing::instrument(name = "imap.mailbox.changes", skip_all, fields(link_id = %mailbox.link_id, changed, removed), err)]
    async fn changes(
        &self,
        mailbox: MailboxKey,
        folder: &ProviderId,
        position: Option<&StreamToken>,
    ) -> Result<MailboxChangePage, EmailApiError> {
        let mut connection = self.connect(mailbox).await?;
        let result = async {
            let mut changes = connection
                .changes(folder, position, PAGE_SIZE)
                .await
                .map_err(EmailApiError::from)?;
            // Removals are only meaningful once the first enumeration is done.
            if matches!(changes.page.position, StreamPosition::Checkpoint(_)) {
                changes.page.removed = self
                    .expunged(&mut connection, mailbox, folder, changes.exists)
                    .await?;
            }
            let span = tracing::Span::current();
            span.record("changed", changes.page.changed.len());
            span.record("removed", changes.page.removed.len());
            Ok(changes.page)
        }
        .await;
        connection.logout().await;
        result
    }

    #[tracing::instrument(name = "imap.mailbox.message", skip_all, fields(link_id = %mailbox.link_id), err)]
    async fn message(
        &self,
        mailbox: MailboxKey,
        id: &ProviderId,
        folders: &[MailFolder],
    ) -> Result<Option<MailboxMessage>, EmailApiError> {
        let message_id = ImapMessageId::from_provider_id(id)?;
        let mut connection = self.connect(mailbox).await?;
        let message = connection
            .message(&message_id, folders, mailbox.link_id)
            .await
            .map_err(EmailApiError::from);
        connection.logout().await;
        message
    }
}

impl<T: MailboxTokenSource> email::domain::mailbox::projection::MailboxAttachmentAccess
    for ImapMailboxGateway<T>
{
    async fn download(
        &self,
        mailbox: MailboxKey,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, MailboxError> {
        let message_id = ImapMessageId::from_provider_id(&ProviderId::new(message_id)?)
            .map_err(EmailApiError::from)?;
        let mut connection = self.connect(mailbox).await?;
        let bytes = connection
            .attachment(&message_id, attachment_id)
            .await
            .map_err(EmailApiError::from);
        connection.logout().await;
        bytes?.ok_or(MailboxError::Provider(EmailApiError::NotFound))
    }
}
