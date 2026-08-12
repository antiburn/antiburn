// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local repository identity: remote normalization, name parsing, and the path
//! folding that makes two spellings of one clone the same repository.

use std::path::Path;

use crate::platform::git::parse_owner_from_url;
use crate::repositories::{
    normalize_for_prefix, normalize_remote_url, parse_repo_name_from_url,
    repo_root_identity_for_platform,
};

#[test]
fn normalize_strips_git_suffix() {
    assert_eq!(
        normalize_remote_url("git@github.com:avery/widgets.git"),
        "git@github.com:avery/widgets"
    );
}

#[test]
fn normalize_strips_trailing_slash() {
    assert_eq!(
        normalize_remote_url("https://github.com/avery/widgets/"),
        "https://github.com/avery/widgets"
    );
}

#[test]
fn normalize_lowercases() {
    assert_eq!(
        normalize_remote_url("https://github.com/Avery/Widgets.git"),
        "https://github.com/avery/widgets"
    );
}

#[test]
fn parse_repo_name_ssh() {
    assert_eq!(
        parse_repo_name_from_url("git@github.com:avery/my-repo.git"),
        Some("my-repo".to_string())
    );
}

#[test]
fn parse_repo_name_https() {
    assert_eq!(
        parse_repo_name_from_url("https://github.com/avery/my-repo.git"),
        Some("my-repo".to_string())
    );
}

#[test]
fn parse_repo_name_no_git_suffix() {
    assert_eq!(
        parse_repo_name_from_url("https://github.com/avery/my-repo"),
        Some("my-repo".to_string())
    );
}

#[test]
fn parse_repo_name_rejects_owner_only_remote() {
    assert_eq!(parse_repo_name_from_url("git@github.com:avery/"), None);
    assert_eq!(parse_repo_name_from_url("not a url"), None);
}

/// SSH and HTTPS remotes for the same repository must produce an identical
/// canonical slug, so the two spellings deduplicate onto one repository.
#[test]
fn canonical_slug_deduplicates_ssh_and_https() {
    fn canonical_slug(url: &str) -> String {
        let owner = parse_owner_from_url(url).unwrap_or_default();
        let name = parse_repo_name_from_url(url).unwrap_or_default();
        format!(
            "{}/{}",
            owner.to_ascii_lowercase(),
            name.to_ascii_lowercase()
        )
    }

    let ssh = "git@github.com:Avery/Widgets.git";
    let https = "https://github.com/Avery/Widgets.git";
    let https_no_git = "https://github.com/Avery/Widgets";

    assert_eq!(canonical_slug(ssh), "avery/widgets");
    assert_eq!(canonical_slug(https), "avery/widgets");
    assert_eq!(canonical_slug(https_no_git), "avery/widgets");
    assert_eq!(canonical_slug(ssh), canonical_slug(https));
}

#[test]
fn normalize_for_prefix_strips_verbatim_and_unifies_separators() {
    // Windows `fs::canonicalize` returns a `\\?\` verbatim path; git repository
    // roots are non-verbatim, so the prefix check must compare on the stripped,
    // separator-unified form. That is what lets a canonicalized parent match a
    // git-canonicalized child under it on Windows.
    assert_eq!(normalize_for_prefix(Path::new(r"\\?\C:\a\b")), "C:/a/b");
    assert_eq!(normalize_for_prefix(Path::new(r"C:\a\b\")), "C:/a/b");
    assert_eq!(
        normalize_for_prefix(Path::new(r"\\?\UNC\srv\share\x")),
        "//srv/share/x"
    );
    // POSIX paths are unaffected beyond a trailing-slash trim.
    assert_eq!(normalize_for_prefix(Path::new("/a/b/")), "/a/b");
    assert_eq!(normalize_for_prefix(Path::new("/a/b")), "/a/b");
}

#[test]
fn windows_identity_folds_native_spelling_but_preserves_wsl_case() {
    let native_a = repo_root_identity_for_platform(Path::new(r"C:\Users\Avery\Widgets"), true);
    let native_b = repo_root_identity_for_platform(Path::new("c:/users/avery/widgets/"), true);
    assert_eq!(native_a, native_b);

    // The filesystem behind the WSL UNC namespace is case-sensitive, so casing
    // there is identity, not spelling.
    let wsl_a = repo_root_identity_for_platform(
        Path::new(r"\\wsl.localhost\Ubuntu\home\avery\Widgets"),
        true,
    );
    let wsl_b = repo_root_identity_for_platform(
        Path::new(r"\\wsl.localhost\Ubuntu\home\avery\widgets"),
        true,
    );
    assert_ne!(wsl_a, wsl_b);
}
