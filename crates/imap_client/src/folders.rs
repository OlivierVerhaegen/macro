use async_imap::types::NameAttribute;
use email_api_client::domain::models::{FolderRole, MailFolder, ProviderId};
use futures::TryStreamExt;

use crate::{ImapConnection, ImapError};

#[cfg(test)]
mod test;

/// Folder names used before SPECIAL-USE existed, Zoho's included. Matched
/// only when no folder advertises the role, never against a display name
/// the user can rename into a role.
const NAMED_ROLES: &[(&str, FolderRole)] = &[
    ("sent", FolderRole::Sent),
    ("sent items", FolderRole::Sent),
    ("sent messages", FolderRole::Sent),
    ("drafts", FolderRole::Drafts),
    ("trash", FolderRole::Trash),
    ("deleted items", FolderRole::Trash),
    ("spam", FolderRole::Junk),
    ("junk", FolderRole::Junk),
    ("archive", FolderRole::Archive),
];

pub(crate) struct ListedFolder {
    pub name: String,
    pub delimiter: Option<String>,
    pub special_use: Option<FolderRole>,
    pub selectable: bool,
}

impl ImapConnection {
    /// The selectable folder hierarchy with resolved roles.
    #[tracing::instrument(name = "imap.folders", skip_all, err)]
    pub async fn folders(&mut self) -> Result<Vec<MailFolder>, ImapError> {
        let listed = self
            .session
            .list(Some(""), Some("*"))
            .await?
            .map_ok(|name| ListedFolder {
                name: name.name().to_owned(),
                delimiter: name.delimiter().map(str::to_owned),
                special_use: name.attributes().iter().find_map(special_use),
                selectable: !name
                    .attributes()
                    .iter()
                    .any(|attribute| matches!(attribute, NameAttribute::NoSelect)),
            })
            .try_collect::<Vec<_>>()
            .await?;
        catalog(listed)
    }
}

fn special_use(attribute: &NameAttribute<'_>) -> Option<FolderRole> {
    match attribute {
        NameAttribute::Sent => Some(FolderRole::Sent),
        NameAttribute::Drafts => Some(FolderRole::Drafts),
        NameAttribute::Trash => Some(FolderRole::Trash),
        NameAttribute::Junk => Some(FolderRole::Junk),
        NameAttribute::Archive => Some(FolderRole::Archive),
        _ => None,
    }
}

/// Resolve roles and parents. Each role goes to at most one folder.
pub(crate) fn catalog(listed: Vec<ListedFolder>) -> Result<Vec<MailFolder>, ImapError> {
    let advertised =
        |role: FolderRole| listed.iter().any(|folder| folder.special_use == Some(role));
    let mut assigned = Vec::new();
    let mut folders = Vec::new();
    for folder in listed.iter().filter(|folder| folder.selectable) {
        let role = if folder.name.eq_ignore_ascii_case("INBOX") {
            FolderRole::Inbox
        } else if let Some(role) = folder.special_use {
            role
        } else {
            NAMED_ROLES
                .iter()
                .find(|(name, role)| folder.name.eq_ignore_ascii_case(name) && !advertised(*role))
                .map_or(FolderRole::Other, |(_, role)| *role)
        };
        let role = if role != FolderRole::Other && assigned.contains(&role) {
            FolderRole::Other
        } else {
            assigned.push(role);
            role
        };
        let parent = folder.delimiter.as_deref().and_then(|delimiter| {
            folder
                .name
                .rsplit_once(delimiter)
                .map(|(parent, _)| parent)
                .filter(|parent| listed.iter().any(|other| other.name == *parent))
        });
        let has_children = folder.delimiter.as_deref().is_some_and(|delimiter| {
            let prefix = format!("{}{delimiter}", folder.name);
            listed.iter().any(|other| other.name.starts_with(&prefix))
        });
        folders.push(MailFolder {
            id: ProviderId::new(folder.name.clone()).map_err(|_| ImapError::MalformedIdentifier)?,
            parent_id: parent
                .map(ProviderId::new)
                .transpose()
                .map_err(|_| ImapError::MalformedIdentifier)?,
            name: display_name(&folder.name, folder.delimiter.as_deref()),
            role,
            has_children,
        });
    }
    Ok(folders)
}

/// The last hierarchy segment. Names stay in modified UTF-7 (RFC 3501
/// 5.1.3); folders with non-ASCII names display with that encoding.
fn display_name(name: &str, delimiter: Option<&str>) -> String {
    delimiter
        .and_then(|delimiter| name.rsplit_once(delimiter))
        .map_or(name, |(_, last)| last)
        .to_owned()
}
