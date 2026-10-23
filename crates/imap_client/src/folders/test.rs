use super::*;

fn listed(name: &str, special_use: Option<FolderRole>) -> ListedFolder {
    ListedFolder {
        name: name.to_owned(),
        delimiter: Some("/".to_owned()),
        special_use,
        selectable: true,
    }
}

fn role_of(folders: &[MailFolder], name: &str) -> FolderRole {
    folders
        .iter()
        .find(|folder| folder.id.as_str() == name)
        .map(|folder| folder.role)
        .expect("folder should be listed")
}

#[test]
fn special_use_wins_over_names() {
    let folders = catalog(vec![
        listed("INBOX", None),
        listed("Sent", None),
        listed("Verzonden", Some(FolderRole::Sent)),
        listed("Spam", None),
    ])
    .unwrap();

    assert_eq!(role_of(&folders, "INBOX"), FolderRole::Inbox);
    assert_eq!(role_of(&folders, "Verzonden"), FolderRole::Sent);
    assert_eq!(role_of(&folders, "Sent"), FolderRole::Other);
    assert_eq!(role_of(&folders, "Spam"), FolderRole::Junk);
}

#[test]
fn hierarchy_sets_parents_children_and_display_names() {
    let folders = catalog(vec![
        listed("Projects", None),
        listed("Projects/Florenza", None),
        ListedFolder {
            selectable: false,
            ..listed("Shared", None)
        },
        listed("Shared/Team", None),
    ])
    .unwrap();

    let child = folders
        .iter()
        .find(|folder| folder.id.as_str() == "Projects/Florenza")
        .unwrap();
    let parent = folders
        .iter()
        .find(|folder| folder.id.as_str() == "Projects")
        .unwrap();
    let orphan = folders
        .iter()
        .find(|folder| folder.id.as_str() == "Shared/Team")
        .unwrap();
    assert_eq!(child.name, "Florenza");
    assert_eq!(
        child.parent_id.as_ref().map(ProviderId::as_str),
        Some("Projects")
    );
    assert!(parent.has_children);
    assert!(!folders.iter().any(|folder| folder.id.as_str() == "Shared"));
    // A non-selectable parent is still the hierarchy parent.
    assert_eq!(
        orphan.parent_id.as_ref().map(ProviderId::as_str),
        Some("Shared")
    );
}

#[test]
fn a_role_is_assigned_once() {
    let folders = catalog(vec![
        listed("Trash", Some(FolderRole::Trash)),
        listed("Old Trash", Some(FolderRole::Trash)),
    ])
    .unwrap();

    assert_eq!(role_of(&folders, "Trash"), FolderRole::Trash);
    assert_eq!(role_of(&folders, "Old Trash"), FolderRole::Other);
}
