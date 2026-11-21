//! Admission for agent storage. Transcript paths do not authorize project access.
use std::path::Path;

pub(super) fn admit(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    if let Some(home) = crate::platform::environment::verified_wsl_source_home(path) {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        return admit_mounted_wsl_source(path, &home, |ancestor| {
            std::fs::symlink_metadata(ancestor)
                .map(|metadata| metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
        });
    }
    admit_native(path)
}

fn admit_native(path: &Path) -> std::io::Result<()> {
    if super::source_path_access(path).is_allowed() {
        Ok(())
    } else {
        Err(access_deferred())
    }
}

fn access_deferred() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "source access deferred",
    )
}

#[cfg(windows)]
fn admit_mounted_wsl_source(
    path: &Path,
    home: &Path,
    mut is_reparse_point: impl FnMut(&Path) -> std::io::Result<bool>,
) -> std::io::Result<()> {
    use std::path::{Component, Prefix};

    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return Err(access_deferred());
    };
    let Prefix::UNC(server, _) = prefix.kind() else {
        return Err(access_deferred());
    };
    let is_wsl = server.to_str().is_some_and(|server| {
        server.eq_ignore_ascii_case("wsl.localhost") || server.eq_ignore_ascii_case("wsl$")
    });
    if !is_wsl
        || home.parent().is_none()
        || !path.starts_with(home)
        || path.to_str().is_none_or(|text| {
            text.split(['/', '\\'])
                .any(|part| part.ends_with([' ', '.']) || part.contains(['\0', ':']))
        })
    {
        return Err(access_deferred());
    }

    let ancestors = path.ancestors().collect::<Vec<_>>();
    for ancestor in ancestors.into_iter().rev() {
        match is_reparse_point(ancestor) {
            Ok(false) => {}
            Ok(true) => return Err(access_deferred()),
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && ancestor != home
                    && ancestor.starts_with(home) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(super) fn try_exists(path: impl AsRef<Path>) -> std::io::Result<bool> {
    admit(path.as_ref())?;
    path.as_ref().try_exists()
}

pub(super) fn read_dir(path: impl AsRef<Path>) -> std::io::Result<std::fs::ReadDir> {
    admit(path.as_ref())?;
    std::fs::read_dir(path)
}
pub(super) fn metadata(path: impl AsRef<Path>) -> std::io::Result<std::fs::Metadata> {
    admit(path.as_ref())?;
    std::fs::metadata(path)
}
pub(super) fn is_dir(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok_and(|m| m.is_dir())
}
pub(super) fn is_file(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok_and(|m| m.is_file())
}
pub(super) fn exists(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok()
}
pub(super) fn read_to_string(path: impl AsRef<Path>) -> std::io::Result<String> {
    admit(path.as_ref())?;
    std::fs::read_to_string(path)
}
pub(super) fn open(path: impl AsRef<Path>) -> std::io::Result<std::fs::File> {
    admit(path.as_ref())?;
    std::fs::File::open(path)
}
pub(super) fn open_database(
    path: impl AsRef<Path>,
    flags: rusqlite::OpenFlags,
) -> rusqlite::Result<rusqlite::Connection> {
    let path = path.as_ref();
    admit_native_database(path)?;
    rusqlite::Connection::open_with_flags(path, flags)
}

pub(super) fn admit_native_database(path: &Path) -> rusqlite::Result<[std::path::PathBuf; 3]> {
    let paths = [
        path.to_path_buf(),
        std::path::PathBuf::from(format!("{}-wal", path.display())),
        std::path::PathBuf::from(format!("{}-shm", path.display())),
    ];
    for candidate in &paths {
        admit_native(candidate).map_err(|_| rusqlite::Error::InvalidPath(candidate.clone()))?;
    }
    Ok(paths)
}

pub(super) mod asynchronous {
    use super::*;
    pub(crate) async fn read_dir(path: impl AsRef<Path>) -> std::io::Result<tokio::fs::ReadDir> {
        admit(path.as_ref())?;
        tokio::fs::read_dir(path).await
    }
    pub(crate) async fn metadata(path: impl AsRef<Path>) -> std::io::Result<std::fs::Metadata> {
        admit(path.as_ref())?;
        tokio::fs::metadata(path).await
    }
    pub(crate) async fn read(path: impl AsRef<Path>) -> std::io::Result<Vec<u8>> {
        admit(path.as_ref())?;
        tokio::fs::read(path).await
    }
    pub(crate) async fn read_to_string(path: impl AsRef<Path>) -> std::io::Result<String> {
        admit(path.as_ref())?;
        tokio::fs::read_to_string(path).await
    }
    pub(crate) async fn open(path: impl AsRef<Path>) -> std::io::Result<tokio::fs::File> {
        admit(path.as_ref())?;
        tokio::fs::File::open(path).await
    }
    pub(crate) async fn try_exists(path: impl AsRef<Path>) -> std::io::Result<bool> {
        admit(path.as_ref())?;
        tokio::fs::try_exists(path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn mounted_wsl_companion_databases_require_native_admission() {
        let db = Path::new(r"\\wsl.localhost\Ubuntu\home\dev\.cline\data\db\sessions.db");
        let result = super::super::open_native_companion_database(db);
        assert!(matches!(result, Err(rusqlite::Error::InvalidPath(path)) if path == db));
    }

    #[test]
    #[cfg(windows)]
    fn mounted_wsl_sources_require_a_verified_home_before_any_probe() {
        let home = Path::new(r"\\wsl.localhost\Ubuntu\home\dev");
        for path in [
            r"\\server\Ubuntu\home\dev\source.jsonl",
            r"\\wsl.localhost\Debian\home\dev\source.jsonl",
            r"\\wsl.localhost\Ubuntu\home\developer\source.jsonl",
            r"\\wsl.localhost\Ubuntu\home\dev\..\other\source.jsonl",
            r"\\wsl.localhost\Ubuntu\home\dev\.\source.jsonl",
            r"\\wsl.localhost\Ubuntu\home\dev\.. \source.jsonl",
            r"\\wsl.localhost\Ubuntu\home\dev\source.jsonl:stream",
            r"\\?\UNC\wsl.localhost\Ubuntu\home\dev\source.jsonl",
        ] {
            let error = admit_mounted_wsl_source(Path::new(path), home, |_| {
                panic!("unexpected filesystem probe for {path}")
            })
            .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        }
        let share = Path::new(r"\\wsl.localhost\Ubuntu");
        assert!(
            admit_mounted_wsl_source(&share.join("source.jsonl"), share, |_| {
                panic!("unexpected filesystem probe for a distro-wide root")
            })
            .is_err()
        );
    }

    #[test]
    #[cfg(windows)]
    fn mounted_wsl_source_probes_stop_at_reparse_points_and_missing_mounts() {
        for namespace in [r"\\wsl.localhost\Ubuntu\", r"\\wsl$\Ubuntu\"] {
            let share = Path::new(namespace);
            let home = share.join("home").join("dev");
            let source = home.join(".claude").join("session.jsonl");
            let mut visited = Vec::new();
            admit_mounted_wsl_source(&source, &home, |path| {
                visited.push(path.to_path_buf());
                Ok(false)
            })
            .unwrap();
            assert_eq!(visited.first().unwrap(), share);
            assert_eq!(visited.last().unwrap(), &source);

            let alias = home.join(".claude");
            let error = admit_mounted_wsl_source(&source, &home, |path| {
                assert_ne!(path, source);
                Ok(path == alias)
            })
            .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);

            let error = admit_mounted_wsl_source(&source, &home, |path| {
                assert_eq!(path, share);
                Err(std::io::ErrorKind::NotFound.into())
            })
            .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
            admit_mounted_wsl_source(&source, &home, |path| {
                if path == source {
                    return Err(std::io::ErrorKind::NotFound.into());
                }
                Ok(false)
            })
            .unwrap();

            assert!(admit_native(&source).is_err());
            assert!(
                !crate::paths::protected::git_project_access(&source, &Default::default())
                    .is_allowed()
            );
        }
    }

    #[tokio::test]
    async fn native_transcripts_remain_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, "synthetic transcript").unwrap();
        assert_eq!(
            asynchronous::read_to_string(&path).await.unwrap(),
            "synthetic transcript"
        );
        assert_eq!(read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn denied_source_existence_is_not_reported_as_missing() {
        let path = Path::new("/System/Volumes/Data/Users/synthetic/Documents/source.db");
        assert_eq!(
            try_exists(path).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    #[cfg(unix)]
    fn source_symlinks_never_follow_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("source");
        std::os::unix::fs::symlink("/Volumes/synthetic-unavailable", &link).unwrap();
        assert_eq!(
            read_dir(link).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }
}
