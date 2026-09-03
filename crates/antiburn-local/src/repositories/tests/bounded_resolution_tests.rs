//! Repository-probe concurrency configuration and canonical-root identity.

use std::path::Path;

use tokio::sync::Semaphore;

use crate::repositories::{concurrency_from, repo_root_identity};

#[test]
fn concurrency_configuration_accepts_positive_overrides() {
    assert_eq!(concurrency_from(Some("1"), Some(16)), 1);
    assert_eq!(concurrency_from(Some(" 12 "), Some(2)), 12);
    assert_eq!(
        concurrency_from(Some(&usize::MAX.to_string()), Some(2)),
        Semaphore::MAX_PERMITS
    );
}

#[test]
fn concurrency_configuration_clamps_cpu_default_and_uses_fallback() {
    assert_eq!(concurrency_from(None, Some(1)), 2);
    assert_eq!(concurrency_from(None, Some(6)), 6);
    assert_eq!(concurrency_from(None, Some(64)), 8);
    assert_eq!(concurrency_from(None, None), 4);
    assert_eq!(concurrency_from(Some("0"), Some(5)), 5);
    assert_eq!(concurrency_from(Some("nope"), None), 4);
}

#[test]
fn identity_is_stable_for_a_plain_path() {
    assert_eq!(
        repo_root_identity(Path::new("/repos/main/")),
        repo_root_identity(Path::new("/repos/main"))
    );
}
