use super::fixtures::lifecycle;

#[test]
fn content_height_reports_ignore_duplicates_but_accept_later_changes() {
    let mut lifecycle = lifecycle();

    assert!(lifecycle.record_height(180.0));
    assert_eq!(lifecycle.height, 180.0);
    assert!(!lifecycle.record_height(180.8));
    assert_eq!(lifecycle.height, 180.0);
    assert!(lifecycle.record_height(182.0));
    assert_eq!(lifecycle.height, 182.0);
}
