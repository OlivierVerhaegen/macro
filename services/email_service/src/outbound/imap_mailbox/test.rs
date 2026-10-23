use super::*;
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
