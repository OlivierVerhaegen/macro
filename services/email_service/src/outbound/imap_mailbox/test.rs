use super::*;
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use std::collections::HashSet;

fn stored(uid_validity: u32, uid: u32) -> String {
    ImapMessageId {
        uid_validity,
        uid,
        folder: "INBOX".into(),
    }
    .to_provider_id()
    .unwrap()
    .as_str()
    .to_owned()
}

#[test]
fn expunged_and_recreated_messages_are_gone() {
    let present = imap_client::FolderUids {
        uid_validity: 7,
        uids: HashSet::from([1, 3]),
    };

    let gone = gone(
        vec![stored(7, 1), stored(7, 2), stored(7, 3), stored(6, 1)],
        &present,
    );

    assert_eq!(
        gone.iter()
            .map(|id| ImapMessageId::from_provider_id(id).unwrap())
            .map(|id| (id.uid_validity, id.uid))
            .collect::<Vec<_>>(),
        [(7, 2), (6, 1)]
    );
}

#[derive(Clone)]
struct NoTokens;

impl MailboxTokenSource for NoTokens {
    async fn access_token(
        &self,
        _mailbox: MailboxAccess,
        _freshness: TokenFreshness,
    ) -> Result<email_api_client::domain::models::AccessToken, TokenError> {
        Err(TokenError::ReauthRequired)
    }
}

async fn imap_link(db: &PgPool, active: bool, needs_reauth: bool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO email_links(id,macro_id,fusionauth_user_id,email_address,provider,grant_generation,sync_generation,is_sync_active,needs_reauth) VALUES ($1,'macro|olivier@example.be','owner',$2,'IMAP',1,3,$3,$4)",
        id,
        format!("{id}@example.be"),
        active,
        needs_reauth
    )
    .execute(db)
    .await
    .unwrap();
    id
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn idle_watches_active_mailboxes_and_wakes_their_folder_streams(db: PgPool) {
    let gateway = ImapMailboxGateway::new(db.clone(), NoTokens);
    let watched = imap_link(&db, true, false).await;
    imap_link(&db, false, false).await;
    imap_link(&db, true, true).await;

    let mailboxes = gateway.idle_mailboxes().await.unwrap();
    assert_eq!(mailboxes.keys().collect::<Vec<_>>(), [&watched]);
    let key = mailboxes[&watched];
    assert_eq!((key.sync_generation, key.grant_generation), (3, 1));

    for (kind, generation) in [("mail_folder", 3), ("contacts", 3), ("mail_folder", 2)] {
        sqlx::query!(
            "INSERT INTO email_sync_streams(id,link_id,generation,kind,scope_id,next_run_at) VALUES ($1,$2,$3,$4,$5,now() + interval '1 hour')",
            Uuid::now_v7(),
            watched,
            generation,
            kind,
            format!("{kind}-{generation}")
        )
        .execute(&db)
        .await
        .unwrap();
    }
    gateway.wake(key).await.unwrap();

    let due = sqlx::query!(
        r#"SELECT kind, generation, next_run_at <= now() AS "due!", notified_at IS NOT NULL AS "notified!" FROM email_sync_streams WHERE link_id = $1 ORDER BY kind, generation"#,
        watched
    )
    .fetch_all(&db)
    .await
    .unwrap()
    .into_iter()
    .map(|row| (row.kind, row.generation, row.due, row.notified))
    .collect::<Vec<_>>();
    assert_eq!(
        due,
        [
            ("contacts".to_owned(), 3, false, false),
            ("mail_folder".to_owned(), 2, false, false),
            ("mail_folder".to_owned(), 3, true, true),
        ]
    );
}
