use super::*;

const MULTIPART: &str = "Message-ID: <reply@example.be>\r\n\
References: <root@example.be> <middle@example.be>\r\n\
In-Reply-To: <middle@example.be>\r\n\
From: Olivier <Olivier@Example.be>\r\n\
To: klant@example.com, Team: a@example.com, b@example.com;\r\n\
Subject: Offerte\r\n\
Date: Thu, 22 Oct 2026 10:00:00 +0200\r\n\
X-Macro-Draft-Correlation: 0192f3a4-0000-7000-8000-000000000001\r\n\
Content-Type: multipart/mixed; boundary=outer\r\n\
\r\n\
--outer\r\n\
Content-Type: multipart/alternative; boundary=inner\r\n\
\r\n\
--inner\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
Hallo   wereld\r\n\
--inner\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<p>Hallo <b>wereld</b></p><script>alert(1)</script>\r\n\
--inner--\r\n\
--outer\r\n\
Content-Type: application/pdf; name=offerte.pdf\r\n\
Content-Disposition: attachment; filename=offerte.pdf\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0x\r\n\
--outer--\r\n";

fn folders() -> Vec<MailFolder> {
    vec![
        MailFolder {
            id: ProviderId::new("INBOX").unwrap(),
            parent_id: None,
            name: "INBOX".into(),
            role: FolderRole::Inbox,
            has_children: false,
        },
        MailFolder {
            id: ProviderId::new("Sent").unwrap(),
            parent_id: None,
            name: "Sent".into(),
            role: FolderRole::Sent,
            has_children: false,
        },
    ]
}

fn raw(body: &str, flags: &[&str]) -> RawMessage {
    RawMessage {
        body: body.as_bytes().to_vec(),
        flags: flags.iter().map(|flag| (*flag).to_owned()).collect(),
        internal_date: None,
        modseq: Some(42),
    }
}

fn id(folder: &str) -> ImapMessageId {
    ImapMessageId {
        uid_validity: 7,
        uid: 31,
        folder: folder.into(),
    }
}

#[test]
fn a_reply_threads_on_the_conversation_root() {
    let snapshot = to_mailbox_message(
        &raw(MULTIPART, &["\\Seen"]),
        &id("INBOX"),
        &folders(),
        Uuid::nil(),
    )
    .unwrap();
    let message = snapshot.content.message;

    assert_eq!(
        message.provider_thread_id.as_deref(),
        Some("<root@example.be>")
    );
    assert_eq!(message.global_id.as_deref(), Some("<reply@example.be>"));
    assert_eq!(message.subject.as_deref(), Some("Offerte"));
}

#[test]
fn a_message_without_references_starts_its_own_thread() {
    let body = "Message-ID: <alone@example.be>\r\nSubject: Hi\r\n\r\nBody\r\n";

    let snapshot =
        to_mailbox_message(&raw(body, &[]), &id("INBOX"), &folders(), Uuid::nil()).unwrap();

    assert_eq!(
        snapshot.content.message.provider_thread_id.as_deref(),
        Some("<alone@example.be>")
    );
}

#[test]
fn addresses_bodies_and_attachments_are_extracted() {
    let snapshot = to_mailbox_message(
        &raw(MULTIPART, &["\\Seen"]),
        &id("INBOX"),
        &folders(),
        Uuid::nil(),
    )
    .unwrap();
    let message = snapshot.content.message;

    assert_eq!(
        message.from.map(|from| from.email),
        Some("olivier@example.be".to_owned())
    );
    assert_eq!(
        message
            .to
            .iter()
            .map(|to| to.email.as_str())
            .collect::<Vec<_>>(),
        ["klant@example.com", "a@example.com", "b@example.com"]
    );
    assert_eq!(message.body_text.as_deref(), Some("Hallo   wereld"));
    assert_eq!(message.snippet.as_deref(), Some("Hallo wereld"));
    let html = message.body_html_sanitized.unwrap();
    assert!(html.contains("wereld") && !html.contains("<script"));
    assert_eq!(message.attachments.len(), 1);
    assert_eq!(message.attachments[0].provider_id.as_deref(), Some("2"));
    assert_eq!(
        part_bytes(MULTIPART.as_bytes(), "2").unwrap(),
        Some(b"%PDF-1".to_vec())
    );
    assert_eq!(part_bytes(MULTIPART.as_bytes(), "9").unwrap(), None);
}

#[test]
fn flags_and_folder_role_become_state_and_tags() {
    let snapshot = to_mailbox_message(
        &raw(MULTIPART, &["\\Flagged", "Klant", "$Forwarded"]),
        &id("Sent"),
        &folders(),
        Uuid::nil(),
    )
    .unwrap();

    assert!(!snapshot.state.is_read);
    assert!(snapshot.state.is_flagged);
    assert!(!snapshot.state.in_inbox);
    assert!(snapshot.content.message.is_sent);
    assert_eq!(snapshot.tags, ["Klant"]);
    assert_eq!(snapshot.version.as_deref(), Some("42"));
    assert_eq!(
        snapshot
            .draft_correlation
            .map(|id| id.to_string())
            .as_deref(),
        Some("0192f3a4-0000-7000-8000-000000000001")
    );
}

#[test]
fn ids_round_trip_with_any_folder_name() {
    let id = id("Klanten/Florenza \"2026\"");

    let decoded = ImapMessageId::from_provider_id(&id.to_provider_id().unwrap()).unwrap();

    assert_eq!(decoded, id);
}
