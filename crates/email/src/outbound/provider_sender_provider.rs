//! Sender discovery per inbox provider: Gmail asks the Gmail API; IMAP
//! (Zoho Mail) inboxes use the addresses Zoho reported when they connected.

use macro_user_id::email::EmailStr;
use sqlx::PgPool;

use crate::domain::{
    models::{EmailErr, Link, UserProvider},
    senders::{EmailSenderProvider, SendAsAddress, SendAsFuture},
};

#[cfg(test)]
mod test;

/// Routes each inbox to its provider's sender facts.
pub struct ProviderSenderProvider<G> {
    gmail: G,
    db: PgPool,
}

impl<G: EmailSenderProvider> ProviderSenderProvider<G> {
    /// Construct at the composition root around the Gmail adapter.
    pub fn new(gmail: G, db: PgPool) -> Self {
        Self { gmail, db }
    }

    #[tracing::instrument(name = "email.imap_senders", skip_all, fields(link_id = %link.id), err)]
    async fn imap(&self, link: &Link) -> Result<Vec<SendAsAddress>, EmailErr> {
        let addresses = sqlx::query_scalar!(
            "SELECT send_addresses FROM imap_accounts WHERE link_id = $1",
            link.id
        )
        .fetch_optional(&self.db)
        .await
        .map_err(|error| EmailErr::ProviderErr(error.into()))?
        .unwrap_or_default();
        // Zoho lists only addresses the account may already send from.
        Ok(addresses
            .into_iter()
            .filter_map(|address| EmailStr::try_from(address).ok())
            .map(|email_address| SendAsAddress {
                email_address,
                display_name: None,
                verified: true,
            })
            .collect())
    }
}

impl<G: EmailSenderProvider> EmailSenderProvider for ProviderSenderProvider<G> {
    fn send_as<'a>(&'a self, link: &'a Link, refresh: bool) -> SendAsFuture<'a> {
        match link.provider {
            UserProvider::Gmail => self.gmail.send_as(link, refresh),
            UserProvider::Imap => Box::pin(self.imap(link)),
            UserProvider::Outlook => Box::pin(async { Err(EmailErr::SendAsUnavailable) }),
        }
    }
}
