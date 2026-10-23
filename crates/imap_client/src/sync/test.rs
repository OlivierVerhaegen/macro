use super::*;

#[test]
fn pages_list_the_newest_uids_first() {
    let uids = HashSet::from([3, 10, 7, 1]);

    assert_eq!(newest_first(uids.clone(), 2), [10, 7]);
    assert_eq!(newest_first(uids, usize::MAX), [10, 7, 3, 1]);
}

#[test]
fn positions_round_trip_through_the_opaque_token() {
    let position = FolderPosition {
        uid_validity: 7,
        highest_modseq: Some(90_060_115_205_545_359),
        uid_next: 120,
        below: Some(40),
    };

    let decoded = FolderPosition::from_token(&position.to_token().unwrap()).unwrap();

    assert_eq!(decoded, position);
    assert!(FolderPosition::from_token(&StreamToken::new("graph-delta-link".into())).is_err());
}
