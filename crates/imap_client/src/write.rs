//! Changes to messages: flags, keywords, moves, appends and deletion.
//!
//! Every write first checks that the id's UIDVALIDITY still holds, so a write
//! never lands on another message after the folder was recreated.

use futures::TryStreamExt;
use mailparse::MailHeaderMap;

use crate::{ImapConnection, ImapError, ImapMessageId};

/// System flags Macro writes. Keywords are written by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemFlag {
    /// Read.
    Seen,
    /// Starred.
    Flagged,
    /// Marked for deletion.
    Deleted,
    /// A draft.
    Draft,
}

impl SystemFlag {
    fn as_imap(self) -> &'static str {
        match self {
            Self::Seen => "\\Seen",
            Self::Flagged => "\\Flagged",
            Self::Deleted => "\\Deleted",
            Self::Draft => "\\Draft",
        }
    }
}

impl ImapConnection {
    /// Select the message's folder; `false` when the folder was recreated.
    async fn select_for_write(&mut self, id: &ImapMessageId) -> Result<bool, ImapError> {
        Ok(self.select_folder(&id.folder).await?.uid_validity == id.uid_validity)
    }

    /// Set or clear a system flag. `Ok(false)` when the message is gone.
    #[tracing::instrument(name = "imap.store_flag", skip_all, err)]
    pub async fn set_flag(
        &mut self,
        id: &ImapMessageId,
        flag: SystemFlag,
        enabled: bool,
    ) -> Result<bool, ImapError> {
        self.store(id, if enabled { "+" } else { "-" }, flag.as_imap())
            .await
    }

    /// Replace the message's keywords with `desired`, keeping system flags.
    #[tracing::instrument(name = "imap.store_keywords", skip_all, err)]
    pub async fn set_keywords(
        &mut self,
        id: &ImapMessageId,
        current: &[String],
        desired: &[String],
    ) -> Result<bool, ImapError> {
        let removed = current
            .iter()
            .filter(|keyword| !desired.contains(keyword))
            .map(|keyword| keyword_atom(keyword))
            .collect::<Result<Vec<_>, _>>()?;
        let added = desired
            .iter()
            .filter(|keyword| !current.contains(keyword))
            .map(|keyword| keyword_atom(keyword))
            .collect::<Result<Vec<_>, _>>()?;
        if !removed.is_empty() && !self.store(id, "-", &removed.join(" ")).await? {
            return Ok(false);
        }
        if !added.is_empty() && !self.store(id, "+", &added.join(" ")).await? {
            return Ok(false);
        }
        Ok(true)
    }

    async fn store(
        &mut self,
        id: &ImapMessageId,
        operation: &str,
        flags: &str,
    ) -> Result<bool, ImapError> {
        if !self.select_for_write(id).await? {
            return Ok(false);
        }
        let changed = self
            .session
            .uid_store(id.uid.to_string(), format!("{operation}FLAGS ({flags})"))
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        // A store on a missing UID succeeds silently; confirm the message exists.
        Ok(!changed.is_empty() || self.exists(id).await?)
    }

    async fn exists(&mut self, id: &ImapMessageId) -> Result<bool, ImapError> {
        Ok(self
            .session
            .uid_search(format!("UID {}", id.uid))
            .await?
            .contains(&id.uid))
    }

    /// Move the message and return its id in `destination`, when it can be
    /// found there (by Message-ID). `Ok(None)` when the source is gone.
    #[tracing::instrument(name = "imap.move", skip_all, err)]
    pub async fn move_message(
        &mut self,
        id: &ImapMessageId,
        destination: &str,
    ) -> Result<Option<ImapMessageId>, ImapError> {
        if !self.select_for_write(id).await? || !self.exists(id).await? {
            return Ok(None);
        }
        let message_id = self.header_message_id(id).await?;
        if self.move_supported {
            self.session.uid_mv(id.uid.to_string(), destination).await?;
        } else {
            self.session
                .uid_copy(id.uid.to_string(), destination)
                .await?;
            self.expunge_uid(id).await?;
        }
        match message_id {
            Some(message_id) => self.find(destination, &message_id).await,
            None => Ok(None),
        }
    }

    /// Store a complete RFC 5322 message, then find its UID by Message-ID.
    #[tracing::instrument(name = "imap.append", skip_all, fields(bytes = message.len()), err)]
    pub async fn append(
        &mut self,
        folder: &str,
        flags: &[SystemFlag],
        message: &[u8],
    ) -> Result<Option<ImapMessageId>, ImapError> {
        let flags = format!(
            "({})",
            flags
                .iter()
                .map(|flag| flag.as_imap())
                .collect::<Vec<_>>()
                .join(" ")
        );
        self.session
            .append(folder, Some(flags.as_str()), None, message)
            .await?;
        let message_id = mailparse::parse_headers(message)
            .ok()
            .and_then(|(headers, _)| headers.get_first_value("Message-ID"));
        match message_id {
            Some(message_id) => self.find(folder, message_id.trim()).await,
            None => Ok(None),
        }
    }

    /// Permanently remove the message. Already gone is success.
    #[tracing::instrument(name = "imap.delete", skip_all, err)]
    pub async fn delete(&mut self, id: &ImapMessageId) -> Result<(), ImapError> {
        if self.select_for_write(id).await? && self.exists(id).await? {
            self.expunge_uid(id).await?;
        }
        Ok(())
    }

    async fn expunge_uid(&mut self, id: &ImapMessageId) -> Result<(), ImapError> {
        self.session
            .uid_store(id.uid.to_string(), "+FLAGS.SILENT (\\Deleted)")
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        if self.uidplus {
            self.session
                .uid_expunge(id.uid.to_string())
                .await?
                .try_collect::<Vec<_>>()
                .await?;
        } else {
            // Without UIDPLUS EXPUNGE also removes other \Deleted messages,
            // which other clients already asked to remove.
            self.session
                .expunge()
                .await?
                .try_collect::<Vec<_>>()
                .await?;
        }
        Ok(())
    }

    async fn header_message_id(&mut self, id: &ImapMessageId) -> Result<Option<String>, ImapError> {
        let fetches = self
            .session
            .uid_fetch(id.uid.to_string(), "(UID BODY.PEEK[HEADER])")
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        Ok(fetches
            .iter()
            .find(|fetch| fetch.uid == Some(id.uid))
            .and_then(|fetch| fetch.header())
            .and_then(|header| mailparse::parse_headers(header).ok())
            .and_then(|(headers, _)| headers.get_first_value("Message-ID"))
            .map(|value| value.trim().to_owned()))
    }

    /// The newest message in `folder` with this Message-ID.
    async fn find(
        &mut self,
        folder: &str,
        message_id: &str,
    ) -> Result<Option<ImapMessageId>, ImapError> {
        if message_id.contains(['"', '\\', '\r', '\n']) {
            return Ok(None);
        }
        let selected = self.select_folder(folder).await?;
        let found = self
            .session
            .uid_search(format!("HEADER Message-ID \"{message_id}\""))
            .await?;
        Ok(found.into_iter().max().map(|uid| ImapMessageId {
            uid_validity: selected.uid_validity,
            uid,
            folder: folder.to_owned(),
        }))
    }
}

/// IMAP keywords are atoms: no spaces, parentheses, quotes or wildcards.
fn keyword_atom(keyword: &str) -> Result<&str, ImapError> {
    if keyword.is_empty()
        || keyword.starts_with('\\')
        || keyword
            .chars()
            .any(|c| c.is_control() || " (){%*\"\\]".contains(c))
    {
        return Err(ImapError::Protocol(format!(
            "the label cannot be stored as an IMAP keyword: {keyword}"
        )));
    }
    Ok(keyword)
}
