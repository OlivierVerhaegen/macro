use super::*;
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

/// Stands in for the Gmail API: one alias, so its answers are recognizable.
struct Gmail;

impl EmailSenderProvider for Gmail {
    fn send_as<'a>(&'a self, _link: &'a Link, _refresh: bool) -> SendAsFuture<'a> {
        Box::pin(async {
            Ok(vec![SendAsAddress {
                email_address: EmailStr::try_from("gmail-alias@example.com".to_owned()).unwrap(),
                display_name: None,
                verified: true,
            }])
        })
    }
}

fn link(id: Uuid, provider: UserProvider) -> Link {
    Link {
        id,
        macro_id: MacroUserIdStr::try_from_email("olivier@example.be").unwrap(),
        fusionauth_user_id: "owner".to_owned(),
        email_address: EmailStr::try_from("olivier@example.be".to_owned()).unwrap(),
        provider,
        is_sync_active: true,
        is_primary: true,
        created_at: Default::default(),
        updated_at: Default::default(),
    }
}

fn addresses(senders: Vec<SendAsAddress>) -> Vec<String> {
    senders
        .into_iter()
        .map(|sender| sender.email_address.0.as_ref().to_owned())
        .collect()
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn imap_inboxes_offer_their_zoho_aliases_and_gmail_asks_gmail(db: PgPool) {
    let id = Uuid::now_v7();
    sqlx::query!("INSERT INTO email_links(id,macro_id,fusionauth_user_id,email_address,provider) VALUES ($1,'macro|olivier@example.be','owner','olivier@example.be','IMAP')", id)
        .execute(&db)
        .await
        .unwrap();
    sqlx::query!("INSERT INTO imap_accounts(link_id,imap_host,imap_port,smtp_host,smtp_port,smtp_security,username,auth_method,send_addresses) VALUES ($1,'imap.zoho.eu',993,'smtp.zoho.eu',465,'tls','olivier@example.be','zoho_oauth','{olivier@example.be,info@example.be}')", id)
        .execute(&db)
        .await
        .unwrap();
    let senders = ProviderSenderProvider::new(Gmail, db);

    assert_eq!(
        addresses(
            senders
                .send_as(&link(id, UserProvider::Imap), false)
                .await
                .unwrap()
        ),
        ["olivier@example.be", "info@example.be"]
    );
    assert_eq!(
        addresses(
            senders
                .send_as(&link(Uuid::now_v7(), UserProvider::Imap), false)
                .await
                .unwrap()
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        addresses(
            senders
                .send_as(&link(id, UserProvider::Gmail), false)
                .await
                .unwrap()
        ),
        ["gmail-alias@example.com"]
    );
}
