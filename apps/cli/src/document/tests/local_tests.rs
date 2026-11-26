use std::fs;

use super::read_snapshot;

#[test]
fn read_snapshot_accepts_regular_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.jsonl");
    let content = "{}\n";
    fs::write(&path, content).unwrap();

    assert_eq!(read_snapshot(&path).unwrap(), (content.to_owned(), false));
}

#[cfg(windows)]
#[test]
fn read_snapshot_rejects_file_reparse_point() {
    use std::os::windows::fs::symlink_file;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.jsonl");
    let link = directory.path().join("link.jsonl");
    fs::write(&path, "{}\n").unwrap();
    if let Err(error) = symlink_file(&path, &link) {
        const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;
        if error.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD)
            && std::env::var_os("GITHUB_ACTIONS").is_none()
        {
            eprintln!(
                "Skipping symlink test: enable Windows Developer Mode or run with elevation."
            );
            return;
        }
        panic!("could not create the Windows file symlink: {error}");
    }

    assert_eq!(
        read_snapshot(&link).unwrap_err(),
        "the indexed local session file is missing or unreadable"
    );
}
