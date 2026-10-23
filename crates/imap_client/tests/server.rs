//! End-to-end checks against a real IMAP server with CONDSTORE (Dovecot).
//!
//! Set `IMAP_TEST_SERVER` (`host:port`, implicit TLS), `IMAP_TEST_CA` (PEM of
//! the server certificate) and `IMAP_TEST_PASSWORD`; each test logs in as a
//! fresh user so runs do not interfere. Without them the tests are skipped.

use email_api_client::domain::models::{FolderRole, StreamPosition};
use imap_client::{
    IdleOutcome, ImapConnection, ImapCredentials, ImapMessageId, ImapServer, SystemFlag,
};
use std::time::Duration;
use uuid::Uuid;
use zeroize::Zeroizing;

struct TestServer {
    server: ImapServer,
    password: String,
}

/// The opt-in test server from the environment; the tests skip without one.
#[expect(
    clippy::disallowed_methods,
    reason = "test-only opt-in settings, not service config"
)]
fn test_server() -> Option<TestServer> {
    let address = std::env::var("IMAP_TEST_SERVER").ok()?;
    let (host, port) = address.rsplit_once(':')?;
    let pem = std::fs::read(std::env::var("IMAP_TEST_CA").ok()?).ok()?;
    let certificate = rustls_pemfile_certificate(&pem);
    Some(TestServer {
        server: ImapServer::new(host.to_owned(), port.parse().ok()?)
            .with_trusted_certificate(certificate)
            .ok()?,
        password: std::env::var("IMAP_TEST_PASSWORD").ok()?,
    })
}

fn rustls_pemfile_certificate(pem: &[u8]) -> rustls::pki_types::CertificateDer<'static> {
    use base64::Engine;
    let text = std::str::from_utf8(pem).expect("PEM is text");
    let body = text
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect::<String>();
    rustls::pki_types::CertificateDer::from(
        base64::engine::general_purpose::STANDARD
            .decode(body)
            .expect("PEM body is base64"),
    )
}

async fn connect(test: &TestServer, user: &str) -> ImapConnection {
    ImapConnection::connect(
        &test.server,
        &ImapCredentials::Password {
            username: user.to_owned(),
            password: Zeroizing::new(test.password.clone()),
        },
    )
    .await
    .expect("login should succeed")
}

fn message(id: &str, references: Option<&str>) -> Vec<u8> {
    let references = references
        .map(|root| format!("References: {root}\r\nIn-Reply-To: {root}\r\n"))
        .unwrap_or_default();
    format!(
        "Message-ID: {id}\r\n{references}From: Klant <klant@example.com>\r\nTo: olivier@example.be\r\nSubject: Test {id}\r\nDate: Thu, 22 Oct 2026 10:00:00 +0200\r\n\r\nHallo\r\n"
    )
    .into_bytes()
}

fn user() -> String {
    format!("{}@example.be", Uuid::now_v7())
}

#[tokio::test]
async fn folders_resolve_special_use_roles() {
    let Some(test) = test_server() else { return };
    let mut connection = connect(&test, &user()).await;

    let folders = connection.folders().await.unwrap();
    let role = |name: &str| {
        folders
            .iter()
            .find(|folder| folder.id.as_str() == name)
            .map(|folder| folder.role)
    };

    assert_eq!(role("INBOX"), Some(FolderRole::Inbox));
    assert_eq!(role("Sent"), Some(FolderRole::Sent));
    assert_eq!(role("Drafts"), Some(FolderRole::Drafts));
    assert_eq!(role("Trash"), Some(FolderRole::Trash));
    assert_eq!(role("Spam"), Some(FolderRole::Junk));
}

#[tokio::test]
async fn a_folder_is_enumerated_in_pages_then_followed_by_modseq() {
    let Some(test) = test_server() else { return };
    let mut connection = connect(&test, &user()).await;
    let inbox = email_api_client::domain::models::ProviderId::new("INBOX").unwrap();
    for index in 1..=3 {
        connection
            .append(
                "INBOX",
                &[],
                &message(&format!("<m{index}@example.com>"), None),
            )
            .await
            .unwrap();
    }

    let first = connection.changes(&inbox, None, 2).await.unwrap();
    let StreamPosition::Continue(token) = &first.page.position else {
        panic!("a full page continues the enumeration");
    };
    let second = connection.changes(&inbox, Some(token), 2).await.unwrap();
    let StreamPosition::Checkpoint(checkpoint) = &second.page.position else {
        panic!("the last page checkpoints");
    };
    let uids = |changes: &imap_client::FolderChanges| {
        changes
            .page
            .changed
            .iter()
            .map(|id| ImapMessageId::from_provider_id(id).unwrap().uid)
            .collect::<Vec<_>>()
    };
    assert_eq!(uids(&first), [3, 2]);
    assert_eq!(uids(&second), [1]);
    assert_eq!(second.exists, 3);

    let quiet = connection
        .changes(&inbox, Some(checkpoint), 2)
        .await
        .unwrap();
    assert!(quiet.page.changed.is_empty());

    let first_message = ImapMessageId::from_provider_id(&first.page.changed[1]).unwrap();
    assert!(
        connection
            .set_flag(&first_message, SystemFlag::Seen, true)
            .await
            .unwrap()
    );
    connection
        .append("INBOX", &[], &message("<m4@example.com>", None))
        .await
        .unwrap();
    let StreamPosition::Checkpoint(checkpoint) = &quiet.page.position else {
        panic!("a quiet round keeps a checkpoint");
    };
    let changed = connection
        .changes(&inbox, Some(checkpoint), 2)
        .await
        .unwrap();
    assert_eq!(uids(&changed), [4, 2]);
}

#[tokio::test]
async fn snapshots_moves_and_deletes_follow_the_message() {
    let Some(test) = test_server() else { return };
    let mut connection = connect(&test, &user()).await;
    let folders = connection.folders().await.unwrap();
    let id = connection
        .append(
            "INBOX",
            &[SystemFlag::Flagged],
            &message("<reply@example.com>", Some("<root@example.com>")),
        )
        .await
        .unwrap()
        .expect("the appended message is found by Message-ID");

    let snapshot = connection
        .message(&id, &folders, Uuid::nil())
        .await
        .unwrap()
        .expect("message exists");
    assert!(snapshot.state.in_inbox && snapshot.state.is_flagged && !snapshot.state.is_read);
    assert_eq!(
        snapshot.content.message.provider_thread_id.as_deref(),
        Some("<root@example.com>")
    );

    let moved = connection
        .move_message(&id, "Trash")
        .await
        .unwrap()
        .expect("the moved message is found in the destination");
    assert!(
        connection
            .message(&id, &folders, Uuid::nil())
            .await
            .unwrap()
            .is_none()
    );
    let trashed = connection
        .message(&moved, &folders, Uuid::nil())
        .await
        .unwrap()
        .expect("message exists in Trash");
    assert!(trashed.state.in_trash);

    connection.delete(&moved).await.unwrap();
    connection.delete(&moved).await.unwrap();
    let trash = email_api_client::domain::models::ProviderId::new("Trash").unwrap();
    assert!(
        connection
            .folder_uids(&trash)
            .await
            .unwrap()
            .uids
            .is_empty()
    );
}

#[tokio::test]
async fn organization_follows_flags_and_archive_folders_are_created_once() {
    let Some(test) = test_server() else { return };
    let mut connection = connect(&test, &user()).await;
    let folders = connection.folders().await.unwrap();
    let id = connection
        .append("INBOX", &[], &message("<organize@example.com>", None))
        .await
        .unwrap()
        .expect("the appended message is found by Message-ID");
    assert_eq!(
        connection
            .find("INBOX", "<organize@example.com>")
            .await
            .unwrap(),
        Some(id.clone())
    );

    let before = connection
        .organization(&id, &folders)
        .await
        .unwrap()
        .expect("message exists");
    assert!(!before.state.is_read && before.state.in_inbox);
    connection
        .set_flag(&id, SystemFlag::Seen, true)
        .await
        .unwrap();
    let after = connection
        .organization(&id, &folders)
        .await
        .unwrap()
        .expect("message exists");
    assert!(after.state.is_read);
    assert_ne!(before.version, after.version);

    connection.create_folder("Archive").await.unwrap();
    connection.create_folder("Archive").await.unwrap();
    let archived = connection
        .move_message(&id, "Archive")
        .await
        .unwrap()
        .expect("the moved message is found in Archive");
    assert!(
        connection
            .organization(&id, &folders)
            .await
            .unwrap()
            .is_none()
    );
    connection.delete(&archived).await.unwrap();
}

#[tokio::test]
async fn idle_reports_a_new_message_and_times_out_quietly() {
    let Some(test) = test_server() else { return };
    let user = user();
    let listener = connect(&test, &user).await;
    assert!(listener.supports_idle());

    let (listener, outcome) = listener
        .idle("INBOX", Duration::from_millis(500))
        .await
        .unwrap();
    assert_eq!(outcome, IdleOutcome::TimedOut);

    let mut writer = connect(&test, &user).await;
    let waiting = tokio::spawn(listener.idle("INBOX", Duration::from_secs(20)));
    // IDLE is only active once the listener has selected and entered it; the
    // append below is the signal it waits for, retried until it is seen.
    let mut appended = 0;
    while !waiting.is_finished() && appended < 20 {
        writer
            .append(
                "INBOX",
                &[],
                &message(&format!("<idle-{appended}@example.com>"), None),
            )
            .await
            .unwrap();
        appended += 1;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let (listener, outcome) = waiting.await.unwrap().unwrap();
    assert_eq!(outcome, IdleOutcome::Changed);
    listener.logout().await;
    writer.logout().await;
}

#[tokio::test]
async fn wrong_credentials_are_an_authentication_failure() {
    let Some(test) = test_server() else { return };

    let result = ImapConnection::connect(
        &test.server,
        &ImapCredentials::Password {
            username: user(),
            password: Zeroizing::new("wrong".into()),
        },
    )
    .await;

    assert!(matches!(
        result,
        Err(imap_client::ImapError::AuthenticationFailed)
    ));
}
