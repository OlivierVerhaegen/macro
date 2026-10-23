use std::collections::HashSet;

use email_api_client::domain::models::{
    MailFolder, MailboxChangePage, MailboxMessage, ProviderId, StreamPosition, StreamToken,
};
use futures::TryStreamExt;
use uuid::Uuid;

use crate::{
    ImapConnection, ImapError, ImapMessageId, convert::to_mailbox_message, position::FolderPosition,
};

#[cfg(test)]
mod test;

/// One page of a folder stream plus the folder's current size.
#[derive(Debug)]
pub struct FolderChanges {
    /// Messages to reconcile and the next position. `removed` stays empty:
    /// without QRESYNC the server does not report expunges, so the caller
    /// compares [`ImapConnection::folder_uids`] with what it stored.
    pub page: MailboxChangePage,
    /// Messages in the folder now. Fewer than the caller holds means expunges.
    pub exists: u32,
}

/// UIDs present in a folder under one UIDVALIDITY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderUids {
    /// The folder's UIDVALIDITY.
    pub uid_validity: u32,
    /// Every UID in the folder.
    pub uids: HashSet<u32>,
}

pub(crate) struct Selected {
    pub(crate) uid_validity: u32,
    pub(crate) uid_next: u32,
    pub(crate) highest_modseq: Option<u64>,
    exists: u32,
}

impl ImapConnection {
    pub(crate) async fn select_folder(&mut self, folder: &str) -> Result<Selected, ImapError> {
        let mailbox = if self.condstore {
            self.session.select_condstore(folder).await?
        } else {
            self.session.select(folder).await?
        };
        Ok(Selected {
            uid_validity: mailbox
                .uid_validity
                .ok_or_else(|| ImapError::Protocol("SELECT returned no UIDVALIDITY".into()))?,
            uid_next: mailbox
                .uid_next
                .ok_or_else(|| ImapError::Protocol("SELECT returned no UIDNEXT".into()))?,
            highest_modseq: mailbox.highest_modseq,
            exists: mailbox.exists,
        })
    }

    /// One bounded page of a folder stream.
    ///
    /// The first round lists existing messages newest first in pages of
    /// `page_size`. Later rounds return every message whose flags or presence
    /// changed since the checkpoint (CONDSTORE), or only new messages when the
    /// server lacks CONDSTORE.
    #[tracing::instrument(name = "imap.folder_changes", skip_all, fields(page_size), err)]
    pub async fn changes(
        &mut self,
        folder: &ProviderId,
        position: Option<&StreamToken>,
        page_size: usize,
    ) -> Result<FolderChanges, ImapError> {
        let previous = position.map(FolderPosition::from_token).transpose()?;
        let selected = self.select_folder(folder.as_str()).await?;
        if previous
            .as_ref()
            .is_some_and(|previous| previous.uid_validity != selected.uid_validity)
        {
            return Err(ImapError::FolderRecreated);
        }
        let (uids, next) = match previous {
            // First enumeration, or its continuation.
            None | Some(FolderPosition { below: Some(_), .. }) => {
                let start = previous.unwrap_or(FolderPosition {
                    uid_validity: selected.uid_validity,
                    highest_modseq: selected.highest_modseq,
                    uid_next: selected.uid_next,
                    below: None,
                });
                let query = match start.below {
                    Some(below) if below <= 1 => None,
                    Some(below) => Some(format!("UID 1:{}", below - 1)),
                    None => Some("ALL".to_owned()),
                };
                let found = match query {
                    Some(query) => self.session.uid_search(query).await?,
                    None => HashSet::new(),
                };
                let page = newest_first(found, page_size);
                // Checkpoint at the state seen when enumeration began, so
                // changes made while it ran are picked up next round.
                let below = page.last().copied().filter(|_| page.len() == page_size);
                (page, FolderPosition { below, ..start })
            }
            Some(checkpoint) => {
                let changed = match (self.condstore, checkpoint.highest_modseq) {
                    (true, Some(modseq)) if selected.highest_modseq != Some(modseq) => {
                        self.session
                            .uid_fetch("1:*", format!("(UID) (CHANGEDSINCE {modseq})"))
                            .await?
                            .try_filter_map(|fetch| async move { Ok(fetch.uid) })
                            .try_collect::<HashSet<_>>()
                            .await?
                    }
                    (true, Some(_)) => HashSet::new(),
                    _ if selected.uid_next > checkpoint.uid_next => {
                        self.session
                            .uid_search(format!("UID {}:*", checkpoint.uid_next))
                            .await?
                    }
                    _ => HashSet::new(),
                };
                (
                    newest_first(changed, usize::MAX),
                    FolderPosition {
                        uid_validity: selected.uid_validity,
                        highest_modseq: selected.highest_modseq,
                        uid_next: selected.uid_next,
                        below: None,
                    },
                )
            }
        };
        let changed = uids
            .into_iter()
            .map(|uid| {
                ImapMessageId {
                    uid_validity: selected.uid_validity,
                    uid,
                    folder: folder.as_str().to_owned(),
                }
                .to_provider_id()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let token = next.to_token()?;
        Ok(FolderChanges {
            page: MailboxChangePage {
                changed,
                removed: Vec::new(),
                position: if next.below.is_some() {
                    StreamPosition::Continue(token)
                } else {
                    StreamPosition::Checkpoint(token)
                },
            },
            exists: selected.exists,
        })
    }

    /// Every UID currently in `folder`, to find expunged messages.
    #[tracing::instrument(name = "imap.folder_uids", skip_all, err)]
    pub async fn folder_uids(&mut self, folder: &ProviderId) -> Result<FolderUids, ImapError> {
        let selected = self.select_folder(folder.as_str()).await?;
        Ok(FolderUids {
            uid_validity: selected.uid_validity,
            uids: self.session.uid_search("ALL").await?,
        })
    }

    /// A current snapshot, or `None` when the message is no longer at `id`.
    #[tracing::instrument(name = "imap.fetch_message", skip_all, err)]
    pub async fn message(
        &mut self,
        id: &ImapMessageId,
        folders: &[MailFolder],
        link_id: Uuid,
    ) -> Result<Option<MailboxMessage>, ImapError> {
        let Some(fetched) = self.fetch_raw(id).await? else {
            return Ok(None);
        };
        to_mailbox_message(&fetched, id, folders, link_id).map(Some)
    }

    /// The decoded bytes of one MIME part, addressed as in the snapshot.
    #[tracing::instrument(name = "imap.fetch_attachment", skip_all, err)]
    pub async fn attachment(
        &mut self,
        id: &ImapMessageId,
        part: &str,
    ) -> Result<Option<Vec<u8>>, ImapError> {
        let Some(fetched) = self.fetch_raw(id).await? else {
            return Ok(None);
        };
        crate::convert::part_bytes(&fetched.body, part)
    }

    async fn fetch_raw(&mut self, id: &ImapMessageId) -> Result<Option<RawMessage>, ImapError> {
        let selected = self.select_folder(&id.folder).await?;
        if selected.uid_validity != id.uid_validity {
            return Ok(None);
        }
        let items = if self.condstore {
            "(UID FLAGS INTERNALDATE MODSEQ BODY.PEEK[])"
        } else {
            "(UID FLAGS INTERNALDATE BODY.PEEK[])"
        };
        let fetches = self
            .session
            .uid_fetch(id.uid.to_string(), items)
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        // `UID FETCH n` may answer for a different message when n is gone.
        let Some(fetch) = fetches.into_iter().find(|fetch| fetch.uid == Some(id.uid)) else {
            return Ok(None);
        };
        let Some(body) = fetch.body() else {
            return Ok(None);
        };
        Ok(Some(RawMessage {
            body: body.to_vec(),
            flags: fetch.flags().map(|flag| flag_name(&flag)).collect(),
            internal_date: fetch.internal_date().map(|date| date.to_utc()),
            modseq: fetch.modseq,
        }))
    }
}

/// What a fetch returns, independent of the IMAP library's borrow types.
pub(crate) struct RawMessage {
    pub body: Vec<u8>,
    pub flags: Vec<String>,
    pub internal_date: Option<chrono::DateTime<chrono::Utc>>,
    pub modseq: Option<u64>,
}

fn flag_name(flag: &async_imap::types::Flag<'_>) -> String {
    use async_imap::types::Flag;
    match flag {
        Flag::Seen => "\\Seen".to_owned(),
        Flag::Answered => "\\Answered".to_owned(),
        Flag::Flagged => "\\Flagged".to_owned(),
        Flag::Deleted => "\\Deleted".to_owned(),
        Flag::Draft => "\\Draft".to_owned(),
        Flag::Recent => "\\Recent".to_owned(),
        Flag::MayCreate => "\\*".to_owned(),
        Flag::Custom(name) => name.to_string(),
    }
}

pub(crate) fn newest_first(uids: HashSet<u32>, limit: usize) -> Vec<u32> {
    let mut uids = uids.into_iter().collect::<Vec<_>>();
    uids.sort_unstable_by(|a, b| b.cmp(a));
    uids.truncate(limit);
    uids
}
