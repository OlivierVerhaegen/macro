//! Delivery of scheduled messages from IMAP mailboxes: SMTP submission, then
//! the copy in the Sent folder gives the message its provider identity.

use super::*;
use email::domain::mailbox::smtp_password::SmtpPasswordError;
use email_api_client::domain::models::{SendRequest, SentIds};
use smtp::{SmtpError, SmtpServer};

/// Zoho files messages sent over SMTP in Sent shortly after acceptance.
const SENT_COPY_ATTEMPTS: u32 = 5;
const SENT_COPY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// MIME, credentials and envelope ready for one submission.
pub struct PreparedImapSend {
    mailbox: MailboxKey,
    server: SmtpServer,
    password: Zeroizing<String>,
    from: String,
    recipients: Vec<String>,
    message: Vec<u8>,
    message_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ImapSendError {
    /// The server refused the message or the password; it was not sent.
    #[error("the IMAP mailbox's SMTP server refused the message")]
    Refused(#[source] SmtpError),
    /// The message may have been sent; reconcile before any retry.
    #[error("the outcome of the SMTP submission is unknown")]
    Unknown(#[source] anyhow::Error),
}

#[derive(Clone)]
pub struct ImapSending<T> {
    pub gateway: ImapMailboxGateway<T>,
    pub auth: authentication_service_client::AuthServiceClient,
}

impl<T: MailboxTokenSource> ImapSending<T> {
    async fn binding(&self, link_id: Uuid) -> Result<MailboxKey, EmailApiError> {
        let row = sqlx::query!(
            "SELECT grant_generation, sync_generation FROM email_links WHERE id = $1 AND provider = 'IMAP' AND is_sync_active",
            link_id
        )
        .fetch_optional(&self.gateway.db)
        .await
        .map_err(|_| transient("the IMAP mailbox binding is unavailable"))?
        .ok_or(EmailApiError::AuthRequired)?;
        Ok(MailboxKey {
            link_id,
            sync_generation: row.sync_generation,
            grant_generation: row.grant_generation,
        })
    }

    async fn smtp_server(&self, link_id: Uuid) -> Result<SmtpServer, EmailApiError> {
        let row = sqlx::query!(
            "SELECT smtp_host, smtp_port, smtp_security, username FROM imap_accounts WHERE link_id = $1",
            link_id
        )
        .fetch_optional(&self.gateway.db)
        .await
        .map_err(|_| transient("IMAP account settings are unavailable"))?
        .ok_or_else(|| EmailApiError::Permanent {
            message: "the IMAP account has no server settings".into(),
        })?;
        Ok(SmtpServer {
            host: row.smtp_host,
            port: u16::try_from(row.smtp_port).map_err(|_| EmailApiError::Permanent {
                message: "invalid SMTP port".into(),
            })?,
            implicit_tls: row.smtp_security == "tls",
            username: row.username,
        })
    }

    /// Load everything a submission needs, without sending anything.
    #[tracing::instrument(name = "imap.send.prepare", skip_all, fields(%link_id), err)]
    pub async fn prepare(
        &self,
        link_id: Uuid,
        request: &SendRequest,
    ) -> Result<PreparedImapSend, EmailApiError> {
        let mailbox = self.binding(link_id).await?;
        let server = self.smtp_server(link_id).await?;
        let password = self
            .auth
            .get_smtp_password(link_id, mailbox.grant_generation, mailbox.sync_generation)
            .await
            .map_err(|error| match error {
                authentication_service_client::error::AuthServiceClientError::NotFound => {
                    EmailApiError::SendRejected {
                        message: "add the mailbox's SMTP app password before sending".into(),
                    }
                }
                authentication_service_client::error::AuthServiceClientError::Unauthorized => {
                    EmailApiError::AuthRequired
                }
                _ => transient("the SMTP password is temporarily unavailable"),
            })?;
        let message_id = request
            .message_id
            .as_deref()
            .ok_or_else(|| EmailApiError::Permanent {
                message: "an IMAP send needs a stable Message-ID".into(),
            })?;
        let recipients = [
            &request.message.to,
            &request.message.cc,
            &request.message.bcc,
        ]
        .into_iter()
        .flatten()
        .flatten()
        .map(|contact| contact.email.clone())
        .collect::<Vec<_>>();
        if recipients.is_empty() {
            return Err(EmailApiError::SendRejected {
                message: "the message has no recipients".into(),
            });
        }
        Ok(PreparedImapSend {
            mailbox,
            server,
            password: Zeroizing::new(password.password),
            from: request.from.email.clone(),
            recipients,
            message: request.build_mime()?,
            message_id: format!("<{message_id}>"),
        })
    }

    /// Submit exactly once, then bind the message to its Sent copy.
    #[tracing::instrument(name = "imap.send.submit", skip_all, fields(link_id = %prepared.mailbox.link_id), err)]
    pub async fn send(&self, prepared: PreparedImapSend) -> Result<SentIds, ImapSendError> {
        prepared
            .server
            .send(
                &prepared.password,
                &prepared.from,
                &prepared.recipients,
                &prepared.message,
            )
            .await
            .map_err(|error| match error {
                SmtpError::Rejected | SmtpError::Refused(_) | SmtpError::InvalidAddress => {
                    ImapSendError::Refused(error)
                }
                SmtpError::Transport(_) => ImapSendError::Unknown(error.into()),
            })?;
        // Accepted. Failing to find or file the Sent copy leaves the
        // delivery to reconciliation, which looks for the same Message-ID.
        self.file_sent_copy(&prepared)
            .await
            .map_err(|error| ImapSendError::Unknown(error.into()))
    }

    async fn file_sent_copy(&self, prepared: &PreparedImapSend) -> Result<SentIds, EmailApiError> {
        let mut connection = self.gateway.connect(prepared.mailbox).await?;
        let result = async {
            let folders = connection.folders().await?;
            let sent = sent_folder(&folders)?;
            // The server gives no signal when it files the copy, so check a
            // few times before filing one ourselves (which could duplicate it).
            let mut found = None;
            for attempt in 0..SENT_COPY_ATTEMPTS {
                if attempt > 0 {
                    tokio::time::sleep(SENT_COPY_INTERVAL).await;
                }
                found = connection.find(sent.as_str(), &prepared.message_id).await?;
                if found.is_some() {
                    break;
                }
            }
            let id = match found {
                Some(id) => id,
                None => connection
                    .append(sent.as_str(), &[SystemFlag::Seen], &prepared.message)
                    .await?
                    .ok_or_else(|| transient("the filed Sent copy was not found"))?,
            };
            sent_ids(&mut connection, &id, &folders, prepared.mailbox.link_id).await
        }
        .await;
        connection.logout().await;
        result
    }

    /// The Sent copy of an earlier submission, if the server has one.
    #[tracing::instrument(name = "imap.send.reconcile", skip_all, fields(%link_id), err)]
    pub async fn find_sent(
        &self,
        link_id: Uuid,
        message_id: &str,
    ) -> Result<Option<SentIds>, EmailApiError> {
        let mailbox = self.binding(link_id).await?;
        let mut connection = self.gateway.connect(mailbox).await?;
        let result = async {
            let folders = connection.folders().await?;
            let sent = sent_folder(&folders)?;
            match connection
                .find(sent.as_str(), &format!("<{message_id}>"))
                .await?
            {
                Some(id) => sent_ids(&mut connection, &id, &folders, link_id)
                    .await
                    .map(Some),
                None => Ok(None),
            }
        }
        .await;
        connection.logout().await;
        result
    }
}

fn sent_folder(folders: &[MailFolder]) -> Result<ProviderId, EmailApiError> {
    folders
        .iter()
        .find(|folder| folder.role == FolderRole::Sent)
        .map(|folder| folder.id.clone())
        .ok_or_else(|| EmailApiError::Permanent {
            message: "the IMAP mailbox has no Sent folder".into(),
        })
}

/// The identities sync would give the same message, so it is never duplicated.
async fn sent_ids(
    connection: &mut ImapConnection,
    id: &ImapMessageId,
    folders: &[MailFolder],
    link_id: Uuid,
) -> Result<SentIds, EmailApiError> {
    let snapshot = connection
        .message(id, folders, link_id)
        .await?
        .ok_or_else(|| transient("the Sent copy disappeared"))?;
    Ok(SentIds {
        provider_message_id: id.to_provider_id()?.as_str().to_owned(),
        provider_thread_id: snapshot
            .content
            .message
            .provider_thread_id
            .ok_or_else(|| transient("the Sent copy has no thread"))?,
    })
}

impl<T: MailboxTokenSource> email::domain::mailbox::smtp_password::SmtpPasswordRepository
    for ImapSending<T>
{
    async fn accessible_imap_mailbox(
        &self,
        actor: &macro_user_id::user_id::MacroUserIdStr<'_>,
        link_id: Uuid,
    ) -> Result<Option<MailboxKey>, SmtpPasswordError> {
        let row = sqlx::query!(
            r#"SELECT l.grant_generation, l.sync_generation FROM email_links l
            WHERE l.id = $1 AND l.provider = 'IMAP' AND l.is_sync_active
                AND (l.macro_id = $2 OR EXISTS (SELECT 1 FROM macro_user_links u
                    WHERE u.link_id = l.id AND u.primary_macro_id = $2))"#,
            link_id,
            actor.as_ref()
        )
        .fetch_optional(&self.gateway.db)
        .await
        .map_err(|_| SmtpPasswordError::Unavailable)?;
        Ok(row.map(|row| MailboxKey {
            link_id,
            sync_generation: row.sync_generation,
            grant_generation: row.grant_generation,
        }))
    }
}

impl<T: MailboxTokenSource> email::domain::mailbox::smtp_password::SmtpPasswordGateway
    for ImapSending<T>
{
    async fn verify(
        &self,
        mailbox: MailboxKey,
        password: &Zeroizing<String>,
    ) -> Result<(), SmtpPasswordError> {
        let server = self
            .smtp_server(mailbox.link_id)
            .await
            .map_err(|_| SmtpPasswordError::Unavailable)?;
        server.verify(password).await.map_err(|error| match error {
            SmtpError::Rejected => SmtpPasswordError::Rejected,
            error => {
                tracing::warn!(error = %error, "SMTP password check failed");
                SmtpPasswordError::Unavailable
            }
        })
    }

    async fn store(
        &self,
        mailbox: MailboxKey,
        password: &Zeroizing<String>,
    ) -> Result<(), SmtpPasswordError> {
        self.auth
            .store_smtp_password(
                mailbox.link_id,
                mailbox.grant_generation,
                mailbox.sync_generation,
                password,
            )
            .await
            .map_err(|error| match error {
                authentication_service_client::error::AuthServiceClientError::Unauthorized => {
                    SmtpPasswordError::NotFound
                }
                _ => SmtpPasswordError::Unavailable,
            })
    }
}
