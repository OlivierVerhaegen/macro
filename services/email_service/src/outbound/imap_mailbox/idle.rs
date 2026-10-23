//! Near-instant sync for IMAP mailboxes: one IDLE connection per active
//! mailbox on its inbox. A reported change makes the mailbox's folder streams
//! due, as a Graph notification does for Outlook; the periodic poll still
//! covers other folders and anything missed while reconnecting.

use super::*;
use imap_client::IdleOutcome;
use std::collections::HashMap;
use std::time::Duration;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

/// Servers end idle sessions after 30 minutes (RFC 2177); renew before that.
const IDLE_RENEWAL: Duration = Duration::from_secs(25 * 60);
/// How often connected, disconnected and reconnected mailboxes are noticed.
const RESCAN_INTERVAL: Duration = Duration::from_secs(60);
const MIN_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(5 * 60);

/// Why a listener stopped for good rather than reconnecting.
enum Stop {
    Cancelled,
    Unsupported,
}

impl<T: MailboxTokenSource + Clone> ImapMailboxGateway<T> {
    /// Keep one IDLE listener per active IMAP mailbox until cancelled.
    pub async fn run_idle(self, cancel: CancellationToken) {
        let tracker = TaskTracker::new();
        let mut listeners: HashMap<Uuid, (MailboxKey, CancellationToken)> = HashMap::new();
        let mut rescan = tokio::time::interval(RESCAN_INTERVAL);
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                _ = rescan.tick() => {}
            }
            let active = match self.idle_mailboxes().await {
                Ok(active) => active,
                Err(error) => {
                    tracing::warn!(error = ?error, "could not list IMAP mailboxes to watch");
                    continue;
                }
            };
            // A reconnect changes the binding, so its old listener stops too.
            listeners.retain(|link_id, (key, listener)| {
                let keep = active.get(link_id) == Some(key);
                if !keep {
                    listener.cancel();
                }
                keep
            });
            for (link_id, key) in active {
                if listeners.contains_key(&link_id) {
                    continue;
                }
                let listener = cancel.child_token();
                tracker.spawn(self.clone().listen(key, listener.clone()));
                listeners.insert(link_id, (key, listener));
            }
        }
        tracker.close();
        tracker.wait().await;
    }

    pub(super) async fn idle_mailboxes(&self) -> Result<HashMap<Uuid, MailboxKey>, sqlx::Error> {
        let rows = sqlx::query!(
            "SELECT id, sync_generation, grant_generation FROM email_links WHERE provider = 'IMAP' AND is_sync_active AND NOT needs_reauth"
        )
        .fetch_all(&self.db)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    row.id,
                    MailboxKey {
                        link_id: row.id,
                        sync_generation: row.sync_generation,
                        grant_generation: row.grant_generation,
                    },
                )
            })
            .collect())
    }

    /// Reconnect with backoff until cancelled or the server lacks IDLE.
    #[tracing::instrument(name = "imap.idle.listen", skip_all, fields(link_id = %key.link_id))]
    async fn listen(self, key: MailboxKey, cancel: CancellationToken) {
        let mut backoff = MIN_BACKOFF;
        loop {
            let session = tokio::select! {
                () = cancel.cancelled() => Ok(Stop::Cancelled),
                session = self.idle_session(key) => session,
            };
            match session {
                Ok(Stop::Cancelled) => return,
                Ok(Stop::Unsupported) => {
                    tracing::warn!("IMAP server lacks IDLE; the mailbox syncs by polling only");
                    return;
                }
                Err(error) => {
                    tracing::warn!(error = ?error, backoff_secs = backoff.as_secs(), "IMAP IDLE session ended");
                }
            }
            tokio::select! {
                () = cancel.cancelled() => return,
                () = tokio::time::sleep(backoff) => {}
            }
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    /// One connection: catch up once, then wake the streams on every change.
    async fn idle_session(&self, key: MailboxKey) -> Result<Stop, EmailApiError> {
        let mut connection = self.connect(key).await?;
        if !connection.supports_idle() {
            connection.logout().await;
            return Ok(Stop::Unsupported);
        }
        let inbox = connection
            .folders()
            .await?
            .into_iter()
            .find(|folder| folder.role == FolderRole::Inbox)
            .map_or_else(
                || "INBOX".to_owned(),
                |folder| folder.id.as_str().to_owned(),
            );
        // Changes made while no session was idle surface on the next poll;
        // waking now brings them in without waiting for it.
        self.wake(key).await?;
        loop {
            let (next, outcome) = connection.idle(&inbox, IDLE_RENEWAL).await?;
            connection = next;
            if outcome == IdleOutcome::Changed {
                self.wake(key).await?;
            }
        }
    }

    #[tracing::instrument(name = "imap.idle.wake", skip_all, fields(link_id = %key.link_id), err)]
    pub(super) async fn wake(&self, key: MailboxKey) -> Result<(), EmailApiError> {
        sqlx::query!(
            "UPDATE email_sync_streams SET notified_at = clock_timestamp(), next_run_at = now() WHERE link_id = $1 AND generation = $2 AND kind IN ('folder_catalog','mail_folder')",
            key.link_id,
            key.sync_generation
        )
        .execute(&self.db)
        .await
        .map_err(|_| transient("the IMAP mailbox streams could not be woken"))?;
        Ok(())
    }
}
