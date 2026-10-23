//! Push notifications through IMAP IDLE (RFC 2177).

use crate::{ImapConnection, ImapError};
use async_imap::extensions::idle::IdleResponse;
use async_imap::imap_proto::{MailboxDatum, Response};
use std::time::{Duration, Instant};

/// What ended one IDLE wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleOutcome {
    /// The server reported new, changed or removed messages.
    Changed,
    /// Nothing happened before the timeout; IDLE again to stay subscribed.
    TimedOut,
}

impl ImapConnection {
    /// Whether the server supports IDLE.
    pub fn supports_idle(&self) -> bool {
        self.idle_supported
    }

    /// Select `folder` and wait up to `timeout` for the server to report a
    /// change. Servers drop idle clients after 30 minutes, so keep `timeout`
    /// under that and call again. The connection is handed back for reuse.
    #[tracing::instrument(name = "imap.idle", skip_all, fields(timeout_secs = timeout.as_secs()), err)]
    pub async fn idle(
        mut self,
        folder: &str,
        timeout: Duration,
    ) -> Result<(Self, IdleOutcome), ImapError> {
        if !self.idle_supported {
            return Err(ImapError::Protocol(
                "the server does not support IDLE".into(),
            ));
        }
        self.select_folder(folder).await?;
        let Self {
            session,
            condstore,
            move_supported,
            uidplus,
            idle_supported,
        } = self;
        let mut handle = session.idle();
        handle.init().await?;
        let deadline = Instant::now() + timeout;
        let outcome = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break IdleOutcome::TimedOut;
            }
            // Dropping the stop source interrupts the wait, so keep it alive.
            let (wait, _stop) = handle.wait_with_timeout(remaining);
            match wait.await? {
                IdleResponse::NewData(data) if is_change(data.parsed()) => {
                    break IdleOutcome::Changed;
                }
                // Keepalives such as `* OK Still here` are not changes.
                IdleResponse::NewData(_) => {}
                IdleResponse::Timeout | IdleResponse::ManualInterrupt => {
                    break IdleOutcome::TimedOut;
                }
            }
        };
        let session = handle.done().await?;
        Ok((
            Self {
                session,
                condstore,
                move_supported,
                uidplus,
                idle_supported,
            },
            outcome,
        ))
    }
}

/// New, removed or re-flagged messages; everything else is status chatter.
fn is_change(response: &Response<'_>) -> bool {
    matches!(
        response,
        Response::Expunge(_)
            | Response::Vanished { .. }
            | Response::Fetch(..)
            | Response::MailboxData(MailboxDatum::Exists(_) | MailboxDatum::Recent(_))
    )
}
