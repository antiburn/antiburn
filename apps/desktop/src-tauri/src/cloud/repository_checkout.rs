//! Verify a local checkout and read its configured fetch origin. This is local
//! evidence, not proof of remote ownership or permission to upload.

use crate::cloud::repository_origin::RepositoryOrigin;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::{collections::BTreeMap, sync::Arc};

#[cfg(unix)]
type ConfigDigest = [u8; 32];
#[cfg(unix)]
type ParsedOrigins = BTreeMap<ConfigDigest, Option<RepositoryOrigin>>;
#[cfg(unix)]
type ParsedOrigin = (ConfigDigest, Option<RepositoryOrigin>);
#[cfg(unix)]
const MAX_CACHED_ORIGINS: usize = 64;

/// Keep a plain directory separate from a directory whose Git proof failed.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CheckoutOrigin {
    Plain,
    Verified {
        checkout: PathBuf,
        origin: Option<RepositoryOrigin>,
    },
    UnverifiedGit,
}

/// Reuse only parsed origins within one request. Each lookup verifies the
/// checkout and reads current bounded config bytes, including on a cache hit.
#[derive(Default)]
pub(super) struct CheckoutOriginCache {
    #[cfg(unix)]
    origins: Arc<ParsedOrigins>,
    #[cfg(all(test, unix))]
    origin_parses: usize,
}

impl CheckoutOriginCache {
    /// Return fresh local checkout evidence without retaining paths, raw config,
    /// or permission to upload in the cache.
    pub(super) async fn lookup(&mut self, cwd: &Path) -> CheckoutOrigin {
        #[cfg(unix)]
        {
            let Some((result, parsed)) = unix::lookup(cwd, Arc::clone(&self.origins)).await else {
                return CheckoutOrigin::UnverifiedGit;
            };
            if let Some((digest, origin)) = parsed {
                let origins = Arc::make_mut(&mut self.origins);
                if origins.len() == MAX_CACHED_ORIGINS {
                    origins.pop_first();
                }
                origins.insert(digest, origin);
                #[cfg(test)]
                {
                    self.origin_parses += 1;
                }
            }
            result
        }
        #[cfg(not(unix))]
        {
            let _ = cwd;
            CheckoutOrigin::UnverifiedGit
        }
    }
}

/// Inspect only the nearest canonical native checkout. Use a fresh lookup
/// before approval or capture admission when no request cache is available.
pub(super) async fn lookup_checkout_origin(cwd: &Path) -> CheckoutOrigin {
    CheckoutOriginCache::default().lookup(cwd).await
}

#[cfg(unix)]
mod unix {
    use super::*;
    use antiburn_local::platform::process::{
        BoundedOutputOptions, BoundedOutputPolicy, bounded_headless_output_with_stdin_and_policy,
    };
    use sha2::{Digest, Sha256};
    use std::{
        ffi::OsStr, fs::Metadata, io, os::unix::fs::MetadataExt, path::Component, time::Duration,
    };
    use tokio::{
        fs::{self, OpenOptions},
        io::AsyncReadExt,
        time::{Instant, timeout_at},
    };

    const MAX_DIRECTORY_DEPTH: usize = 128;
    const MAX_PATH_BYTES: usize = 4096;
    const MAX_POINTER_BYTES: usize = 8192;
    const MAX_CONFIG_BYTES: usize = 64 * 1024;
    const MAX_FETCH_OUTPUT: usize = 4097;
    const LOOKUP_TIMEOUT: Duration = Duration::from_secs(1);
    static LOOKUP_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

    #[derive(PartialEq, Eq)]
    struct Checkout {
        path: PathBuf,
        git_path: PathBuf,
        common_path: PathBuf,
        directories: [(u64, u64); 3],
        config: Option<LocalFile>,
        worktree_config: Option<LocalFile>,
    }

    #[derive(PartialEq, Eq)]
    struct LocalFile {
        generation: FileGeneration,
        bytes: Vec<u8>,
    }

    #[derive(PartialEq, Eq)]
    struct FileGeneration {
        device: u64,
        inode: u64,
        size: u64,
        modified: (i64, i64),
        changed: (i64, i64),
    }

    impl From<Metadata> for FileGeneration {
        fn from(metadata: Metadata) -> Self {
            Self {
                device: metadata.dev(),
                inode: metadata.ino(),
                size: metadata.len(),
                modified: (metadata.mtime(), metadata.mtime_nsec()),
                changed: (metadata.ctime(), metadata.ctime_nsec()),
            }
        }
    }

    pub(super) async fn lookup(
        cwd: &Path,
        origins: Arc<ParsedOrigins>,
    ) -> Option<(CheckoutOrigin, Option<ParsedOrigin>)> {
        let deadline = Instant::now() + LOOKUP_TIMEOUT;
        let cwd = cwd.to_owned();
        // Let in-flight filesystem work finish with its permit held if the
        // caller times out. No further stage starts after the deadline.
        let task = tokio::spawn(async move {
            let _permit = timeout_at(deadline, LOOKUP_SLOTS.acquire())
                .await
                .ok()?
                .ok()?;
            inspect(&cwd, &origins, deadline).await
        });
        timeout_at(deadline, task).await.ok()?.ok()?
    }

    async fn inspect(
        cwd: &Path,
        origins: &ParsedOrigins,
        deadline: Instant,
    ) -> Option<(CheckoutOrigin, Option<ParsedOrigin>)> {
        let Some(first) = checkout(cwd, deadline).await.ok()? else {
            return Some((CheckoutOrigin::Plain, None));
        };
        if let Some(config) = &first.worktree_config
            && !worktree_config_preserves_origin(&config.bytes, deadline).await
        {
            return None;
        }
        let mut parsed = None;
        let origin = if let Some(config) = &first.config {
            if first.git_path == first.common_path
                && !primary_worktree_config(&config.bytes, deadline).await
            {
                return None;
            }
            let digest = Sha256::digest(&config.bytes).into();
            if let Some(origin) = origins.get(&digest) {
                origin.clone()
            } else {
                let origin = fetch_origin(&config.bytes, deadline).await?;
                parsed = Some((digest, origin.clone()));
                origin
            }
        } else {
            None
        };
        if checkout(cwd, deadline).await.ok().flatten().as_ref() != Some(&first) {
            return None;
        }
        Some((
            CheckoutOrigin::Verified {
                checkout: first.path,
                origin,
            },
            parsed,
        ))
    }

    /// Stop at the first Git marker, even if it is broken or inaccessible.
    async fn checkout(cwd: &Path, deadline: Instant) -> io::Result<Option<Checkout>> {
        if cwd
            .components()
            .any(|component| matches!(component, Component::Normal(name) if name == ".git"))
        {
            return Err(invalid_checkout());
        }
        canonical_directory(cwd, deadline).await?;
        for root in cwd.ancestors() {
            check_deadline(deadline)?;
            match fs::symlink_metadata(root.join(".git")).await {
                Ok(marker) => return verify_checkout(root, marker, deadline).await.map(Some),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }

    async fn verify_checkout(
        root: &Path,
        marker_metadata: Metadata,
        deadline: Instant,
    ) -> io::Result<Checkout> {
        let marker = root.join(".git");
        let (git_path, common_path) = if marker_metadata.is_dir() {
            (marker.clone(), marker)
        } else if marker_metadata.is_file() {
            let pointer = read_file(&marker, MAX_POINTER_BYTES, deadline).await?;
            if FileGeneration::from(marker_metadata) != pointer.generation {
                return Err(invalid_checkout());
            }
            let git_path = pointer_path(root, &pointer.bytes, b"gitdir: ")?;
            canonical_directory(&git_path, deadline).await?;
            let registrations = git_path.parent().ok_or_else(invalid_checkout)?;
            if registrations.file_name() != Some(OsStr::new("worktrees")) {
                return Err(invalid_checkout());
            }
            let common_path = registrations.parent().ok_or_else(invalid_checkout)?;
            let common_pointer =
                read_file(&git_path.join("commondir"), MAX_POINTER_BYTES, deadline).await?;
            let backlink = read_file(&git_path.join("gitdir"), MAX_POINTER_BYTES, deadline).await?;
            if pointer_path(&git_path, &common_pointer.bytes, b"")? != common_path
                || pointer_path(&git_path, &backlink.bytes, b"")? != marker
            {
                return Err(invalid_checkout());
            }
            (git_path.clone(), common_path.to_owned())
        } else {
            return Err(invalid_checkout());
        };
        let directories = [
            canonical_directory(root, deadline).await?,
            canonical_directory(&git_path, deadline).await?,
            canonical_directory(&common_path, deadline).await?,
        ];
        verify_head(&git_path, deadline).await?;
        check_deadline(deadline)?;
        if !fs::symlink_metadata(common_path.join("objects"))
            .await?
            .is_dir()
            || !fs::symlink_metadata(common_path.join("refs"))
                .await?
                .is_dir()
        {
            return Err(invalid_checkout());
        }
        let config =
            read_optional_config(&common_path.join("config"), MAX_CONFIG_BYTES, deadline).await?;
        let remaining = MAX_CONFIG_BYTES - config.as_ref().map_or(0, |config| config.bytes.len());
        let worktree_config =
            read_optional_config(&git_path.join("config.worktree"), remaining, deadline).await?;
        Ok(Checkout {
            path: root.to_owned(),
            git_path,
            common_path,
            directories,
            config,
            worktree_config,
        })
    }

    async fn verify_head(git: &Path, deadline: Instant) -> io::Result<()> {
        let head = read_file(&git.join("HEAD"), MAX_POINTER_BYTES, deadline).await?;
        let head = path_record(&head.bytes)?;
        let is_symbolic = head
            .strip_prefix("ref: refs/")
            .is_some_and(|name| !name.is_empty());
        let is_detached =
            matches!(head.len(), 40 | 64) && head.bytes().all(|byte| byte.is_ascii_hexdigit());
        if !is_symbolic && !is_detached {
            return Err(invalid_checkout());
        }
        Ok(())
    }

    /// Normalize Git's relative pointers without following symlinks. The caller
    /// must still verify the target directory or match the exact backlink.
    fn pointer_path(base: &Path, bytes: &[u8], prefix: &[u8]) -> io::Result<PathBuf> {
        let value = Path::new(path_record(
            bytes.strip_prefix(prefix).ok_or_else(invalid_checkout)?,
        )?);
        let mut path = if value.is_absolute() {
            PathBuf::new()
        } else {
            base.to_owned()
        };
        let mut descended = value.is_absolute();
        for component in value.components() {
            match component {
                Component::RootDir => path.push("/"),
                Component::CurDir if !descended => {}
                Component::Normal(name) => {
                    descended = true;
                    path.push(name);
                }
                // An intermediate symlink can change the meaning of `name/..`.
                Component::ParentDir if !descended && path.pop() => {}
                _ => return Err(invalid_checkout()),
            }
        }
        Ok(path)
    }

    fn path_record(bytes: &[u8]) -> io::Result<&str> {
        let bytes = bytes
            .strip_suffix(b"\r\n")
            .or_else(|| bytes.strip_suffix(b"\n"))
            .ok_or_else(invalid_checkout)?;
        let text = std::str::from_utf8(bytes).map_err(|_| invalid_checkout())?;
        if text.is_empty() || text.len() > MAX_PATH_BYTES || text.chars().any(char::is_control) {
            return Err(invalid_checkout());
        }
        Ok(text)
    }

    async fn canonical_directory(path: &Path, deadline: Instant) -> io::Result<(u64, u64)> {
        let text = path.to_str().ok_or_else(invalid_checkout)?;
        let components: PathBuf = path.components().collect();
        if !path.is_absolute()
            || text.len() > MAX_PATH_BYTES
            || text.chars().any(char::is_control)
            || components.as_os_str() != path.as_os_str()
        {
            return Err(invalid_checkout());
        }
        let mut directory = PathBuf::from("/");
        check_deadline(deadline)?;
        let mut metadata = fs::symlink_metadata(&directory).await?;
        for (index, component) in path.components().skip(1).enumerate() {
            let Component::Normal(name) = component else {
                return Err(invalid_checkout());
            };
            if index == MAX_DIRECTORY_DEPTH {
                return Err(invalid_checkout());
            }
            check_deadline(deadline)?;
            directory.push(name);
            metadata = fs::symlink_metadata(&directory).await?;
            if !metadata.is_dir() {
                return Err(invalid_checkout());
            }
        }
        check_deadline(deadline)?;
        if fs::canonicalize(path).await?.as_os_str() != path.as_os_str() {
            return Err(invalid_checkout());
        }
        Ok((metadata.dev(), metadata.ino()))
    }

    /// Use the opened file's generation and length, not a path-only preflight.
    async fn read_file(path: &Path, max_bytes: usize, deadline: Instant) -> io::Result<LocalFile> {
        check_deadline(deadline)?;
        let named = fs::symlink_metadata(path).await?;
        if !named.is_file() || named.len() > max_bytes as u64 {
            return Err(invalid_checkout());
        }
        check_deadline(deadline)?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .await?;
        let metadata = file.metadata().await?;
        let generation = FileGeneration::from(named);
        if !metadata.is_file() || FileGeneration::from(metadata) != generation {
            return Err(invalid_checkout());
        }
        check_deadline(deadline)?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(max_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() as u64 != generation.size
            || FileGeneration::from(file.metadata().await?) != generation
            || FileGeneration::from(fs::symlink_metadata(path).await?) != generation
        {
            return Err(invalid_checkout());
        }
        Ok(LocalFile { generation, bytes })
    }

    fn check_deadline(deadline: Instant) -> io::Result<()> {
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(())
    }

    fn invalid_checkout() -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Git checkout could not be verified",
        )
    }

    async fn read_optional_config(
        path: &Path,
        max_bytes: usize,
        deadline: Instant,
    ) -> io::Result<Option<LocalFile>> {
        match read_file(path, max_bytes, deadline).await {
            Ok(config) => Ok(Some(config)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Reject a bare repository or a configured work tree override at `.git`.
    /// These options can make the parent directory differ from the checkout.
    async fn primary_worktree_config(bytes: &[u8], deadline: Instant) -> bool {
        let Some(output) = read_config(
            bytes,
            &["--get-regexp", "^core\\.(bare|worktree)$"],
            1024,
            deadline,
        )
        .await
        else {
            return false;
        };
        output.is_empty() || matches!(output.as_slice(), b"core.bare\nfalse\0")
    }

    /// Check any local file, even if the common config does not enable it.
    /// An include can hide an origin override and is not safe to follow here.
    async fn worktree_config_preserves_origin(bytes: &[u8], deadline: Instant) -> bool {
        read_config(
            bytes,
            &[
                "--name-only",
                "--get-regexp",
                "^(remote\\.origin\\.url|url\\..*\\.insteadof|include\\.path|includeif\\..*\\.path|core\\.worktree)$",
            ],
            1024,
            deadline,
        )
        .await
        .is_some_and(|output| output.is_empty())
    }

    async fn fetch_origin(bytes: &[u8], deadline: Instant) -> Option<Option<RepositoryOrigin>> {
        let output = read_config(
            bytes,
            &["--get-all", "remote.origin.url"],
            MAX_FETCH_OUTPUT,
            deadline,
        )
        .await?;
        Some(RepositoryOrigin::from_fetch_urls(&output))
    }

    /// Let Git parse the already bounded bytes. Do not permit ambient config,
    /// includes, network protocols, or logs of config input and command output.
    async fn read_config(
        bytes: &[u8],
        query: &[&str],
        max_stdout_bytes: usize,
        deadline: Instant,
    ) -> Option<Vec<u8>> {
        let timeout = deadline.checked_duration_since(Instant::now())?;
        let mut args = vec!["config", "--file", "-", "--no-includes", "--null"];
        args.extend_from_slice(query);
        let result = bounded_headless_output_with_stdin_and_policy(
            "git",
            &args,
            Some(bytes),
            BoundedOutputOptions {
                timeout,
                max_stdout_bytes,
                max_stderr_bytes: 1024,
                env: &[
                    ("LC_ALL", "C"),
                    ("GIT_CONFIG_NOSYSTEM", "1"),
                    ("GIT_CONFIG_GLOBAL", "/dev/null"),
                    ("GIT_ALLOW_PROTOCOL", ""),
                    ("GIT_NO_LAZY_FETCH", "1"),
                ],
                env_remove: &[],
                current_dir: Some(Path::new("/")),
            },
            BoundedOutputPolicy {
                clear_env: true,
                fail_on_stdout_limit: true,
                kill_descendants_on_exit: true,
            },
        )
        .await
        .ok()?;
        if result.truncated {
            return None;
        }
        if result.status.success() {
            return Some(result.stdout);
        }
        (result.status.code() == Some(1) && result.stdout.is_empty()).then(Vec::new)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };
    use tokio::{fs as async_fs, process::Command};

    const ORIGIN: &str = "https://git.example.test/Org/Repo.git";
    const ORIGIN_LABEL: &str = "endpoint-v2:git.example.test/Org/Repo.git";

    async fn git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .await
            .expect("run Git on the synthetic repository");
        assert!(output.status.success(), "synthetic Git command failed");
    }

    async fn repo(path: &Path) {
        async_fs::create_dir(path)
            .await
            .expect("create synthetic checkout");
        git(path, &["init", "--quiet"]).await;
        git(path, &["config", "--local", "remote.origin.url", ORIGIN]).await;
    }

    fn assert_origin(result: CheckoutOrigin, expected_checkout: &Path, expected_origin: &str) {
        let CheckoutOrigin::Verified {
            checkout,
            origin: Some(origin),
        } = result
        else {
            panic!("expected a verified checkout and origin");
        };
        assert_eq!(checkout, expected_checkout);
        assert_eq!(origin.as_label(), expected_origin);
    }

    #[tokio::test]
    async fn plain_and_originless_checkout_are_distinct() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        assert_eq!(lookup_checkout_origin(&root).await, CheckoutOrigin::Plain);
        let project = root.join("project");
        repo(&project).await;
        git(
            &project,
            &["config", "--local", "--unset", "remote.origin.url"],
        )
        .await;
        assert_eq!(
            lookup_checkout_origin(&project).await,
            CheckoutOrigin::Verified {
                checkout: project,
                origin: None
            }
        );
    }

    #[tokio::test]
    async fn linked_checkout_rejects_a_worktree_fetch_override_on_a_cache_hit() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let project = root.join("project");
        let linked = root.join("linked");
        repo(&project).await;
        git(
            &project,
            &[
                "-c",
                "user.name=Synthetic",
                "-c",
                "user.email=synthetic@example.test",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "initial",
            ],
        )
        .await;
        git(
            &project,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                linked.to_str().unwrap(),
            ],
        )
        .await;
        git(
            &project,
            &[
                "config",
                "--local",
                "remote.origin.pushurl",
                "https://push.example.test/Org/Repo",
            ],
        )
        .await;
        git(
            &project,
            &["config", "--local", "extensions.worktreeConfig", "true"],
        )
        .await;
        git(&linked, &["config", "--worktree", "user.name", "Synthetic"]).await;
        let nested = linked.join("src");
        fs::create_dir(&nested).unwrap();
        let mut cache = CheckoutOriginCache::default();
        assert_origin(cache.lookup(&nested).await, &linked, ORIGIN_LABEL);
        assert_eq!(cache.origin_parses, 1);
        git(
            &linked,
            &[
                "config",
                "--worktree",
                "remote.origin.url",
                "https://override.example.test/Org/Repo",
            ],
        )
        .await;
        assert_eq!(cache.lookup(&nested).await, CheckoutOrigin::UnverifiedGit);
        git(
            &linked,
            &["config", "--worktree", "--unset", "remote.origin.url"],
        )
        .await;
        git(
            &linked,
            &["config", "--worktree", "include.path", "unverified-config"],
        )
        .await;
        assert_eq!(cache.lookup(&nested).await, CheckoutOrigin::UnverifiedGit);
        assert_eq!(cache.origin_parses, 1);
        let copied = root.join("copied");
        fs::create_dir(&copied).unwrap();
        fs::copy(linked.join(".git"), copied.join(".git")).unwrap();
        assert_eq!(
            lookup_checkout_origin(&copied).await,
            CheckoutOrigin::UnverifiedGit
        );
    }

    #[tokio::test]
    async fn broken_nested_marker_does_not_inherit_the_parent_origin() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let project = root.join("project");
        repo(&project).await;
        let child = project.join("child");
        fs::create_dir(&child).unwrap();
        fs::write(child.join(".git"), "broken\n").unwrap();
        assert_eq!(
            lookup_checkout_origin(&child).await,
            CheckoutOrigin::UnverifiedGit
        );
        assert_origin(
            lookup_checkout_origin(&project).await,
            &project,
            ORIGIN_LABEL,
        );
    }

    #[tokio::test]
    async fn a_live_origin_edit_replaces_old_evidence() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let project = root.join("project");
        repo(&project).await;
        let mut cache = CheckoutOriginCache::default();
        assert_origin(cache.lookup(&project).await, &project, ORIGIN_LABEL);
        assert_eq!(cache.origin_parses, 1);
        let config = project.join(".git/config");
        let modified = config.metadata().unwrap().modified().unwrap();
        let bytes = fs::read_to_string(&config).unwrap();
        let updated = bytes.replace(ORIGIN, "https://git.example.test/Org/Next.git");
        assert_eq!(updated.len(), bytes.len());
        fs::write(&config, updated).unwrap();
        fs::File::open(&config)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_origin(
            cache.lookup(&project).await,
            &project,
            "endpoint-v2:git.example.test/Org/Next.git",
        );
        assert_eq!(cache.origin_parses, 2);
        assert_origin(
            lookup_checkout_origin(&project).await,
            &project,
            "endpoint-v2:git.example.test/Org/Next.git",
        );
    }

    #[tokio::test]
    async fn lookup_reuses_origin_with_4096_registered_worktrees() {
        let temporary = tempfile::tempdir().unwrap();
        let root = async_fs::canonicalize(temporary.path()).await.unwrap();
        let project = root.join("project");
        let template = root.join("template");
        repo(&project).await;
        git(
            &project,
            &[
                "-c",
                "user.name=Synthetic",
                "-c",
                "user.email=synthetic@example.test",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "initial",
            ],
        )
        .await;
        git(
            &project,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                template.to_str().unwrap(),
            ],
        )
        .await;
        let registry = project.join(".git/worktrees");
        let head = async_fs::read(registry.join("template/HEAD"))
            .await
            .unwrap();
        let worktrees = root.join("synthetic");
        async_fs::create_dir(&worktrees).await.unwrap();
        for index in 1..4096 {
            let name = format!("worktree-{index:04}");
            let checkout = worktrees.join(&name);
            let registration = registry.join(&name);
            let (checkout_dir, registration_dir) = tokio::join!(
                async_fs::create_dir(&checkout),
                async_fs::create_dir(&registration)
            );
            checkout_dir.unwrap();
            registration_dir.unwrap();
            let marker = checkout.join(".git");
            let (pointer, backlink, common, detached_head) = tokio::join!(
                async_fs::write(&marker, format!("gitdir: {}\n", registration.display())),
                async_fs::write(
                    registration.join("gitdir"),
                    format!("{}\n", marker.display())
                ),
                async_fs::write(registration.join("commondir"), b"../..\n"),
                async_fs::write(registration.join("HEAD"), &head)
            );
            pointer.unwrap();
            backlink.unwrap();
            common.unwrap();
            detached_head.unwrap();
        }
        let mut entries = async_fs::read_dir(&registry).await.unwrap();
        let mut count = 0;
        while entries.next_entry().await.unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 4096);
        drop(entries);

        // Search permission permits exact pointer lookup but not enumeration.
        let permissions = async_fs::metadata(&registry).await.unwrap().permissions();
        async_fs::set_permissions(&registry, fs::Permissions::from_mode(0o111))
            .await
            .unwrap();
        let listing = async_fs::read_dir(&registry).await;
        let mut cache = CheckoutOriginCache::default();
        let first = cache.lookup(&template).await;
        let last_path = worktrees.join("worktree-4095");
        let last = cache.lookup(&last_path).await;
        async_fs::set_permissions(&registry, permissions)
            .await
            .unwrap();

        match listing {
            Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied),
            Ok(_) => {
                // SAFETY: `geteuid` reads the current process identity only.
                assert_eq!(
                    unsafe { libc::geteuid() },
                    0,
                    "registry must not be listable"
                );
            }
        }
        assert_origin(first, &template, ORIGIN_LABEL);
        assert_origin(last, &last_path, ORIGIN_LABEL);
        assert_eq!(cache.origin_parses, 1);

        async_fs::write(registry.join("worktree-4095/gitdir"), b"/wrong/.git\n")
            .await
            .unwrap();
        assert_eq!(
            cache.lookup(&last_path).await,
            CheckoutOrigin::UnverifiedGit
        );
        assert_eq!(cache.origin_parses, 1);
    }

    #[tokio::test]
    async fn aliases_metadata_directories_and_oversized_config_are_unverified() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let project = root.join("project");
        repo(&project).await;
        let alias = root.join("alias");
        symlink(&project, &alias).unwrap();
        assert_eq!(
            lookup_checkout_origin(&alias).await,
            CheckoutOrigin::UnverifiedGit
        );
        assert_eq!(
            lookup_checkout_origin(&project.join(".git/objects")).await,
            CheckoutOrigin::UnverifiedGit
        );
        let config = project.join(".git/config");
        fs::OpenOptions::new()
            .write(true)
            .open(config)
            .unwrap()
            .set_len(64 * 1024 + 1)
            .unwrap();
        assert_eq!(
            lookup_checkout_origin(&project).await,
            CheckoutOrigin::UnverifiedGit
        );
    }
}
