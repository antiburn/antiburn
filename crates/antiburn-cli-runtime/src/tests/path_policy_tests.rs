use super::*;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

#[test]
fn root_owned_platform_and_sticky_directories_have_explicit_owner_policy() {
    let user = 501;
    assert!(directory_owner_is_safe(0, 0o755, user));
    assert!(directory_owner_is_safe(0, 0o1777, user));
    assert!(directory_owner_is_safe(user, 0o700, user));
    assert!(!directory_owner_is_safe(0, 0o777, user));
    assert!(!directory_owner_is_safe(user, 0o777, user));
    assert!(!directory_owner_is_safe(user, 0o1777, user));
    assert!(!directory_owner_is_safe(user + 1, 0o700, user));
    validate_directory_ancestors(Path::new("/")).unwrap();
}

#[test]
fn rejects_a_shared_writable_ancestor_without_changing_its_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let parent = root.join("shared");
    let child = parent.join("private");
    fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&child).unwrap();
    validate_directory_ancestors(&child).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(validate_directory_ancestors(&child).is_err());
    assert_eq!(
        fs::metadata(parent).unwrap().permissions().mode() & 0o777,
        0o777
    );
}

#[test]
fn missing_or_redirected_ancestors_are_not_silently_accepted() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    assert!(validate_directory_ancestors(&root.join("missing/child")).is_err());
    std::os::unix::fs::symlink(&root, root.join("redirect")).unwrap();
    assert!(validate_directory_ancestors(&root.join("redirect")).is_err());
}
