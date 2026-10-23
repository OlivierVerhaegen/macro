#![deny(missing_docs)]

//! IMAP mailbox access for the Olive Trees fork (Zoho Mail and generic IMAP).
//!
//! The crate speaks IMAP and returns the provider-neutral vocabulary of
//! `email_api_client` (`MailFolder`, `MailboxChangePage`, `MailboxMessage`), so
//! the email domain's mailbox runtime syncs an IMAP account the same way it
//! syncs Outlook. It never touches storage: the caller supplies the server,
//! the credentials and the persisted stream positions.

mod connection;
mod convert;
mod error;
mod folders;
mod idle;
mod message_id;
mod position;
mod sync;
mod write;

pub use connection::{ImapConnection, ImapCredentials, ImapServer};
pub use error::ImapError;
pub use idle::IdleOutcome;
pub use message_id::ImapMessageId;
pub use sync::{FolderChanges, FolderUids};
pub use write::SystemFlag;

/// The header Macro writes on drafts it appends, to recognize them after sync.
pub const DRAFT_CORRELATION_HEADER: &str = "X-Macro-Draft-Correlation";
