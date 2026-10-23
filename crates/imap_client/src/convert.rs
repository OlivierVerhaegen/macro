//! RFC 5322 messages to the provider-neutral mailbox snapshot.

use chrono::{TimeZone, Utc};
use email_api_client::domain::models::{
    Attention, CalendarPart, FolderRole, MailFolder, MailboxMessage, MailboxState,
    MessageWithCalendarParts, ProviderId,
};
use mailparse::{DispositionType, MailAddr, MailHeaderMap, ParsedMail};
use models_email::service::{address::ContactInfo, attachment::Attachment, message::Message};
use uuid::Uuid;

use crate::{DRAFT_CORRELATION_HEADER, ImapError, ImapMessageId, sync::RawMessage};

#[cfg(test)]
mod test;

const SNIPPET_MAX_CHARS: usize = 200;

pub(crate) fn to_mailbox_message(
    raw: &RawMessage,
    id: &ImapMessageId,
    folders: &[MailFolder],
    link_id: Uuid,
) -> Result<MailboxMessage, ImapError> {
    let parsed = mailparse::parse_mail(&raw.body)
        .map_err(|error| ImapError::Protocol(format!("unparseable message: {error}")))?;
    let headers = &parsed.headers;
    let header = |name: &str| {
        headers
            .get_first_value(name)
            .filter(|value| !value.trim().is_empty())
    };

    let provider_id = id.to_provider_id()?;
    let folder_id =
        ProviderId::new(id.folder.clone()).map_err(|_| ImapError::MalformedIdentifier)?;
    let role = MailFolder::effective_role(folders, Some(&folder_id));
    let state = state(&raw.flags, role);

    // A message without Message-ID still needs a stable thread key.
    let message_id = header("Message-ID")
        .map(|value| value.trim().to_owned())
        .unwrap_or_else(|| format!("<imap-{link_id}-{}-{}>", id.uid_validity, id.uid));
    let thread_id = thread_root(
        header("References").as_deref(),
        header("In-Reply-To").as_deref(),
    )
    .unwrap_or_else(|| message_id.clone());

    let sent_at = header("Date")
        .and_then(|date| mailparse::dateparse(&date).ok())
        .and_then(|seconds| Utc.timestamp_opt(seconds, 0).single());
    let contacts = |name: &str| {
        headers
            .get_all_headers(name)
            .into_iter()
            .filter_map(|header| mailparse::addrparse_header(header).ok())
            .flat_map(|list| list.into_inner())
            .flat_map(|address| match address {
                MailAddr::Single(single) => vec![single],
                MailAddr::Group(group) => group.addrs,
            })
            .map(|single| ContactInfo {
                email: single.addr.to_lowercase(),
                name: single.display_name,
                photo_url: None,
            })
            .collect::<Vec<_>>()
    };

    let parts = extract_parts(&parsed);
    let snippet = parts.text.as_deref().map(snippet);
    let now = Utc::now();
    let header_json = serde_json::to_value(
        headers
            .iter()
            .map(|header| {
                serde_json::json!({
                    "name": header.get_key(),
                    "value": header.get_value(),
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|_| ImapError::Protocol("unserializable headers".into()))?;
    let message = Message {
        db_id: Uuid::now_v7(),
        provider_id: Some(provider_id.as_str().to_owned()),
        thread_db_id: Uuid::now_v7(),
        provider_thread_id: Some(thread_id),
        replying_to_id: None,
        global_id: Some(message_id),
        link_id,
        subject: header("Subject"),
        snippet,
        provider_history_id: None,
        internal_date_ts: raw.internal_date.or(sent_at),
        sent_at,
        size_estimate: i64::try_from(raw.body.len()).ok(),
        is_read: state.is_read,
        is_starred: state.is_flagged,
        is_sent: role == FolderRole::Sent,
        is_draft: state.is_draft,
        scheduled_send_time: None,
        has_attachments: !parts.attachments.is_empty(),
        from: contacts("From").into_iter().next(),
        to: contacts("To"),
        cc: contacts("Cc"),
        bcc: contacts("Bcc"),
        labels: Vec::new(),
        body_text: parts.text,
        body_html_sanitized: parts.html,
        body_macro: None,
        attachments: parts.attachments,
        attachments_draft: Vec::new(),
        attachments_forwarded: Vec::new(),
        headers_json: Some(header_json),
        created_at: now,
        updated_at: now,
    };
    Ok(MailboxMessage {
        draft_correlation: header(DRAFT_CORRELATION_HEADER)
            .and_then(|value| Uuid::parse_str(value.trim()).ok()),
        content: MessageWithCalendarParts {
            message,
            calendar_parts: parts.calendar,
        },
        state,
        folder_id: Some(folder_id),
        tags: tags(&raw.flags),
        version: Some(version(&raw.flags, raw.modseq)),
    })
}

/// Mailbox facts carried by flags and the folder's role.
pub(crate) fn state(flags: &[String], role: FolderRole) -> MailboxState {
    let has_flag = |name: &str| flags.iter().any(|flag| flag.eq_ignore_ascii_case(name));
    MailboxState {
        is_read: has_flag("\\Seen"),
        is_flagged: has_flag("\\Flagged"),
        is_draft: has_flag("\\Draft") || role == FolderRole::Drafts,
        in_inbox: role == FolderRole::Inbox,
        in_trash: role == FolderRole::Trash,
        in_junk: role == FolderRole::Junk,
        attention: Attention::Unknown,
    }
}

/// IMAP keywords are the closest thing to tags; system flags are state.
pub(crate) fn tags(flags: &[String]) -> Vec<String> {
    flags
        .iter()
        .filter(|flag| !flag.starts_with('\\') && !flag.starts_with('$'))
        .cloned()
        .collect()
}

/// MODSEQ when the server has CONDSTORE, else the sorted flags.
pub(crate) fn version(flags: &[String], modseq: Option<u64>) -> String {
    match modseq {
        Some(modseq) => modseq.to_string(),
        None => {
            let mut flags = flags.to_vec();
            flags.sort();
            flags.join(" ")
        }
    }
}

/// The conversation root: the first `References` id, else `In-Reply-To`.
pub(crate) fn thread_root(references: Option<&str>, in_reply_to: Option<&str>) -> Option<String> {
    [references, in_reply_to]
        .into_iter()
        .flatten()
        .flat_map(str::split_whitespace)
        .find(|token| token.starts_with('<') && token.ends_with('>') && token.len() > 2)
        .map(str::to_owned)
}

#[derive(Default)]
struct Parts {
    text: Option<String>,
    html: Option<String>,
    attachments: Vec<Attachment>,
    calendar: Vec<CalendarPart>,
}

/// Walk the MIME tree depth first. Part paths follow IMAP numbering
/// (`1`, `1.2`, ...) and identify attachments for later download.
fn extract_parts(root: &ParsedMail) -> Parts {
    let mut parts = Parts::default();
    let mut stack = vec![(root, String::new())];
    while let Some((part, path)) = stack.pop() {
        let mime_type = part.ctype.mimetype.to_lowercase();
        if mime_type.starts_with("multipart/") {
            for (index, child) in part.subparts.iter().enumerate().rev() {
                let child_path = if path.is_empty() {
                    (index + 1).to_string()
                } else {
                    format!("{path}.{}", index + 1)
                };
                stack.push((child, child_path));
            }
            continue;
        }
        let path = if path.is_empty() {
            "1".to_owned()
        } else {
            path
        };
        let disposition = part.get_content_disposition();
        let filename = disposition
            .params
            .get("filename")
            .or_else(|| part.ctype.params.get("name"))
            .cloned();
        if mime_type == "text/calendar" {
            parts.calendar.push(CalendarPart {
                part_id: Some(path.clone()),
                filename: filename.clone(),
                mime_type: mime_type.clone(),
                inline_data: part.get_body_raw().ok(),
                provider_attachment_id: None,
            });
        }
        let is_attachment = disposition.disposition == DispositionType::Attachment
            || (filename.is_some() && !mime_type.starts_with("text/"))
            || (disposition.disposition == DispositionType::Inline
                && !mime_type.starts_with("text/"));
        if is_attachment {
            parts.attachments.push(Attachment {
                reference_url: None,
                db_id: Uuid::now_v7(),
                provider_id: Some(path),
                data_url: None,
                filename,
                mime_type: Some(part.ctype.mimetype.clone()),
                size_bytes: part
                    .get_body_raw()
                    .ok()
                    .and_then(|bytes| i64::try_from(bytes.len()).ok()),
                sfs_id: None,
                content_id: part.headers.get_first_value("Content-ID"),
            });
        } else if mime_type == "text/plain" && parts.text.is_none() {
            parts.text = part.get_body().ok();
        } else if mime_type == "text/html" && parts.html.is_none() {
            parts.html = part
                .get_body()
                .ok()
                .map(|html| email_utils::sanitize_email_html(&html));
        }
    }
    if parts.text.is_none() {
        parts.text = parts
            .html
            .as_deref()
            .and_then(email_utils::body_parsed::html_to_plaintext);
    }
    parts
}

/// Decoded bytes of the part at `path`, or `None` when it does not exist.
pub(crate) fn part_bytes(raw: &[u8], path: &str) -> Result<Option<Vec<u8>>, ImapError> {
    let parsed = mailparse::parse_mail(raw)
        .map_err(|error| ImapError::Protocol(format!("unparseable message: {error}")))?;
    let mut part = &parsed;
    for segment in path.split('.') {
        let index = segment
            .parse::<usize>()
            .map_err(|_| ImapError::MalformedIdentifier)?;
        if part.subparts.is_empty() && index == 1 {
            continue;
        }
        let Some(child) = index
            .checked_sub(1)
            .and_then(|index| part.subparts.get(index))
        else {
            return Ok(None);
        };
        part = child;
    }
    Ok(part.get_body_raw().ok())
}

fn snippet(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(SNIPPET_MAX_CHARS)
        .collect()
}
