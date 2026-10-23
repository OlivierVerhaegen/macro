use super::*;
use std::sync::Mutex;

const ACTOR: &str = "macro|olivier@example.be";

fn mailbox() -> MailboxKey {
    MailboxKey {
        link_id: Uuid::nil(),
        sync_generation: 1,
        grant_generation: 1,
    }
}

struct Repo(Option<MailboxKey>);

impl SmtpPasswordRepository for Repo {
    async fn accessible_imap_mailbox(
        &self,
        _actor: &MacroUserIdStr<'_>,
        _link_id: Uuid,
    ) -> Result<Option<MailboxKey>, SmtpPasswordError> {
        Ok(self.0)
    }
}

#[derive(Default)]
struct Gateway {
    accepts: bool,
    stored: Mutex<Vec<String>>,
}

impl SmtpPasswordGateway for Gateway {
    async fn verify(
        &self,
        _mailbox: MailboxKey,
        _password: &Zeroizing<String>,
    ) -> Result<(), SmtpPasswordError> {
        if self.accepts {
            Ok(())
        } else {
            Err(SmtpPasswordError::Rejected)
        }
    }
    async fn store(
        &self,
        _mailbox: MailboxKey,
        password: &Zeroizing<String>,
    ) -> Result<(), SmtpPasswordError> {
        self.stored.lock().unwrap().push(password.to_string());
        Ok(())
    }
}

fn actor() -> MacroUserIdStr<'static> {
    MacroUserIdStr::parse_from_str(ACTOR).unwrap()
}

#[tokio::test]
async fn an_accepted_password_is_stored() {
    let service = SmtpPasswordService::new(
        Repo(Some(mailbox())),
        Gateway {
            accepts: true,
            ..Gateway::default()
        },
    );

    service
        .set(&actor(), Uuid::nil(), Zeroizing::new("app-password".into()))
        .await
        .unwrap();

    assert_eq!(*service.gateway.stored.lock().unwrap(), ["app-password"]);
}

#[tokio::test]
async fn a_rejected_password_is_not_stored() {
    let service = SmtpPasswordService::new(Repo(Some(mailbox())), Gateway::default());

    let result = service
        .set(&actor(), Uuid::nil(), Zeroizing::new("wrong".into()))
        .await;

    assert_eq!(result, Err(SmtpPasswordError::Rejected));
    assert!(service.gateway.stored.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_inaccessible_inbox_is_not_found() {
    let service = SmtpPasswordService::new(
        Repo(None),
        Gateway {
            accepts: true,
            ..Gateway::default()
        },
    );

    let result = service
        .set(&actor(), Uuid::nil(), Zeroizing::new("app-password".into()))
        .await;

    assert_eq!(result, Err(SmtpPasswordError::NotFound));
}
