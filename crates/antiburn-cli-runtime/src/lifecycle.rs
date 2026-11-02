use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_BINARY: u64 = 256 * 1024 * 1024;
const MAX_JSON: u64 = 16 * 1024;
const IDENTITY: &str = "ai.antiburn.desktop.cli";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub target: String,
    pub executable: String,
    pub version: String,
    pub sha256: String,
    pub delivery_protocols: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliRuntime {
    pub executable: PathBuf,
    pub version: String,
    pub sha256: String,
    pub delivery_protocols: Vec<String>,
}

/// Retain one verified binary digest for repeated status reads.
#[derive(Default)]
pub struct RuntimeDigestCache {
    #[cfg(target_os = "macos")]
    entry: Option<(PathBuf, BinaryIdentity, String)>,
}

#[cfg(target_os = "macos")]
#[derive(Debug, PartialEq, Eq)]
struct BinaryIdentity {
    device: u64,
    inode: u64,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

#[cfg(target_os = "macos")]
fn binary_identity(path: &Path) -> Result<Option<BinaryIdentity>> {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let native_path = CString::new(path.as_os_str().as_bytes())?;
    let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
    ensure!(
        unsafe { libc::statfs(native_path.as_ptr(), filesystem.as_mut_ptr()) } == 0,
        "CLI filesystem metadata is unavailable"
    );
    let filesystem = unsafe { filesystem.assume_init() };
    if unsafe { CStr::from_ptr(filesystem.f_fstypename.as_ptr()) }.to_bytes() != b"apfs" {
        return Ok(None);
    }
    let metadata = fs::symlink_metadata(path)?;
    ensure!(metadata.is_file(), "Expected a regular CLI file");
    Ok(Some(BinaryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        length: metadata.len(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    }))
}

impl RuntimeDigestCache {
    fn hash(&mut self, path: &Path) -> Result<String> {
        #[cfg(target_os = "macos")]
        {
            let Some(before) = binary_identity(path)? else {
                self.entry = None;
                return hash(path);
            };
            if let Some((cached_path, identity, digest)) = &self.entry
                && cached_path == path
                && *identity == before
            {
                return Ok(digest.clone());
            }
            self.entry = None;
            let digest = hash(path)?;
            ensure!(
                binary_identity(path)?.as_ref() == Some(&before),
                "CLI runtime changed during validation"
            );
            self.entry = Some((path.to_path_buf(), before, digest.clone()));
            Ok(digest)
        }
        // Other filesystems need reliable change-time checks before they can reuse a digest.
        #[cfg(not(target_os = "macos"))]
        hash(path)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliInstallState {
    pub runtime: Option<CliRuntime>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema: u32,
    identity: String,
    active: Option<Manifest>,
    pending: Option<Manifest>,
    previous: Option<Manifest>,
    terminal_seen: bool,
    terminal_removed: bool,
    terminal_sha256: Option<String>,
}

impl Default for Receipt {
    fn default() -> Self {
        Self {
            schema: 1,
            identity: IDENTITY.into(),
            active: None,
            pending: None,
            previous: None,
            terminal_seen: false,
            terminal_removed: false,
            terminal_sha256: None,
        }
    }
}

pub struct Installation {
    pub root: PathBuf,
    pub terminal: PathBuf,
    pub payload: PathBuf,
    pub target: String,
}

pub fn file_name() -> &'static str {
    if cfg!(windows) {
        "antiburn-cli.exe"
    } else {
        "antiburn-cli"
    }
}

fn validate_manifest(m: &Manifest, target: &str) -> Result<()> {
    ensure!(
        m.schema == 1 && m.target == target,
        "CLI manifest target or schema is unsupported"
    );
    ensure!(
        m.executable == file_name(),
        "CLI executable name is invalid"
    );
    ensure!(
        m.sha256.len() == 64
            && m.sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "CLI digest is invalid"
    );
    ensure!(
        !m.version.is_empty() && m.version.len() <= 128 && !m.version.chars().any(char::is_control),
        "CLI version is invalid"
    );
    ensure!(
        m.delivery_protocols == ["delivery-v1"],
        "CLI capabilities are unsupported"
    );
    Ok(())
}

fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Expected a regular file"
    );
    ensure!(metadata.len() <= limit, "CLI file exceeds its size limit");
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    ensure!(file.metadata()?.is_file(), "Expected a regular CLI file");
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "CLI file exceeds its size limit"
    );
    Ok(bytes)
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn hash(path: &Path) -> Result<String> {
    Ok(digest(&read_file(path, MAX_BINARY)?))
}

fn parent(path: &Path) -> Result<&Path> {
    path.parent().context("CLI destination has no parent")
}

fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn check_owner(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    #[cfg(windows)]
    super::windows::check_owned(path, metadata)?;
    #[cfg(target_os = "macos")]
    super::macos::check_policy(path)?;
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
        for name in [c"system.posix_acl_access", c"system.posix_acl_default"] {
            let size =
                unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0) };
            let error = std::io::Error::last_os_error().raw_os_error();
            ensure!(
                size < 0 && (error == Some(libc::ENODATA) || error == Some(libc::ENOTSUP)),
                "CLI path has an extended or unreadable ACL; its policy was preserved"
            );
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() },
            "CLI path belongs to another user"
        );
        ensure!(
            metadata.mode() & 0o022 == 0,
            "CLI path permits writes by other users"
        );
    }
    Ok(())
}

// Check every installation ancestor. Do not follow redirected directories.
fn safe_dir(path: &Path, create: bool) -> Result<()> {
    ensure!(
        path.is_absolute(),
        "CLI installation paths must be absolute"
    );
    if let Some(parent) = path.parent() {
        check_ancestors(parent)?;
    }
    if !present(path)? && create {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(path).context(
            "Cannot create the per-user CLI directory; check its ownership and permissions",
        )?;
        if let Some(parent) = path.parent() {
            check_ancestors(parent)?;
        }
    }
    let m = fs::symlink_metadata(path)?;
    ensure!(
        m.is_dir() && !m.file_type().is_symlink(),
        "CLI directory is redirected or is not a directory"
    );
    check_owner(path, &m)
}

fn check_ancestors(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if let Ok(m) = fs::symlink_metadata(ancestor) {
            #[cfg(windows)]
            super::windows::check_ancestor(ancestor, &m)?;
            #[cfg(target_os = "macos")]
            super::macos::check_ancestor(ancestor)?;
            ensure!(
                m.is_dir() && !m.file_type().is_symlink(),
                "CLI directory ancestor is redirected"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                // Root-owned platform ancestors can contain user directories.
                ensure!(
                    m.uid() == 0 || m.uid() == unsafe { libc::geteuid() },
                    "CLI ancestor belongs to another user"
                );
                ensure!(
                    m.mode() & 0o022 == 0 || (m.uid() == 0 && m.mode() & 0o1000 != 0),
                    "CLI ancestor permits unrelated writes"
                );
            }
        }
    }
    Ok(())
}

fn atomic_bytes(
    path: &Path,
    bytes: &[u8],
    executable: bool,
    source: Option<&Path>,
    replace: bool,
) -> Result<()> {
    #[cfg(windows)]
    let _ = executable;
    #[cfg(not(target_os = "macos"))]
    let _ = source;
    let parent = path.parent().context("CLI destination has no parent")?;
    safe_dir(parent, true)?;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| anyhow::anyhow!("CLI temporary name: {e}"))?;
    let temp = parent.join(format!(".cli-stage-{:x}", u128::from_le_bytes(random)));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(if executable { 0o700 } else { 0o600 });
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        #[cfg(target_os = "macos")]
        if let Some(source) = source {
            super::macos::copy_attributes(source, &file)?;
        }
        file.sync_all()?;
        #[cfg(windows)]
        super::windows::check_owned(&temp, &file.metadata()?)?;
        // Windows replacement opens the staging file without sharing.
        drop(file);
        if replace {
            ensure!(
                present(path)?,
                "Owned CLI destination disappeared during replacement"
            );
            check_owner(path, &fs::symlink_metadata(path)?)?;
            #[cfg(windows)]
            super::windows::replace_preserving_policy(path, &temp)?;
            #[cfg(unix)]
            fs::rename(&temp, path)
                .context("CLI replacement deferred; destination may be locked")?;
        } else {
            fs::hard_link(&temp, path).context(
                "CLI destination appeared during publication or does not support native links",
            )?;
            fs::remove_file(&temp)?;
        }
        sync_dir(parent)?;
        Ok(())
    })();
    let _ = fs::remove_file(&temp);
    result
}

fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(windows)]
    let _ = path;
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    Ok(())
}

fn retry_startup(
    mut operation: impl FnMut() -> Result<()>,
    timeout: std::time::Duration,
) -> Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match operation() {
            Ok(()) => return Ok(()),
            Err(error) => {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                let busy = matches!(
                    error.downcast_ref::<std::fs::TryLockError>(),
                    Some(std::fs::TryLockError::WouldBlock)
                );
                #[cfg(windows)]
                let busy = busy
                    || error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| matches!(error.raw_os_error(), Some(32 | 33)));
                if !busy || remaining.is_zero() {
                    return Err(error);
                }
                std::thread::sleep(remaining.min(std::time::Duration::from_millis(50)));
            }
        }
    }
}

impl Installation {
    fn stable(&self) -> PathBuf {
        self.root.join("bin").join(file_name())
    }
    fn immutable(&self, m: &Manifest) -> PathBuf {
        self.root.join("versions").join(&m.sha256).join(file_name())
    }

    fn lock(&self) -> Result<File> {
        safe_dir(&self.root, true)?;
        let path = self.root.join("install.lock");
        if present(&path)? {
            let m = fs::symlink_metadata(&path)?;
            ensure!(
                m.is_file() && !m.file_type().is_symlink(),
                "CLI lock is redirected"
            );
            check_owner(&path, &m)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(path)?;
        file.try_lock()
            .context("CLI installation is busy; retry after the other operation finishes")?;
        Ok(file)
    }

    fn receipt(&self) -> Result<Receipt> {
        let path = self.root.join("receipt.json");
        if !present(&path)? {
            return Ok(Receipt::default());
        }
        check_owner(&path, &fs::symlink_metadata(&path)?)?;
        let r: Receipt = serde_json::from_slice(&read_file(&path, MAX_JSON)?)
            .context("CLI ownership receipt is invalid; preserve files for review")?;
        ensure!(
            r.schema == 1 && r.identity == IDENTITY,
            "CLI ownership receipt is not recognized"
        );
        for m in [&r.active, &r.pending, &r.previous].into_iter().flatten() {
            validate_manifest(m, &self.target)?;
        }
        if let Some(digest) = &r.terminal_sha256 {
            ensure!(
                digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()),
                "CLI terminal receipt is invalid"
            );
        }
        Ok(r)
    }

    fn save(&self, r: &Receipt) -> Result<()> {
        atomic_bytes(
            &self.root.join("receipt.json"),
            &serde_json::to_vec(r)?,
            false,
            None,
            present(&self.root.join("receipt.json"))?,
        )
    }

    fn matches(&self, path: &Path, m: &Manifest) -> bool {
        #[cfg(unix)]
        {
            fs::read_link(path).is_ok_and(|p| p == self.immutable(m))
        }
        #[cfg(windows)]
        {
            hash(path).is_ok_and(|h| h == m.sha256)
        }
    }

    fn owned_entry(&self, path: &Path, r: &Receipt) -> bool {
        [&r.active, &r.pending, &r.previous]
            .into_iter()
            .flatten()
            .any(|m| self.matches(path, m))
    }

    fn publish(&self, path: &Path, m: &Manifest, r: &Receipt) -> Result<()> {
        safe_dir(
            path.parent().context("CLI destination has no parent")?,
            true,
        )?;
        ensure!(
            !present(path)? || self.owned_entry(path, r),
            "CLI destination collides with an unrelated or modified command; move it yourself before Repair"
        );
        if self.matches(path, m) {
            return Ok(());
        }
        #[cfg(unix)]
        {
            if !present(path)? {
                std::os::unix::fs::symlink(self.immutable(m), path)?;
                return sync_dir(parent(path)?);
            }
            let mut random = [0u8; 16];
            getrandom::fill(&mut random).map_err(|e| anyhow::anyhow!("CLI temporary name: {e}"))?;
            let temp = path.with_file_name(format!(".cli-link-{:x}", u128::from_le_bytes(random)));
            std::os::unix::fs::symlink(self.immutable(m), &temp)?;
            let result = fs::rename(&temp, path).context("Could not publish CLI link");
            let _ = fs::remove_file(&temp);
            result?;
            sync_dir(parent(path)?)?;
        }
        #[cfg(windows)]
        atomic_bytes(
            path,
            &read_file(&self.immutable(m), MAX_BINARY)?,
            true,
            Some(&self.immutable(m)),
            present(path)?,
        )?;
        Ok(())
    }

    fn runtime_with_cache(
        &self,
        r: &Receipt,
        cache: &mut RuntimeDigestCache,
    ) -> Result<CliRuntime> {
        let m = [&r.active, &r.pending, &r.previous]
            .into_iter()
            .flatten()
            .find(|m| self.matches(&self.stable(), m))
            .context("CLI runtime is not installed")?;
        safe_dir(&self.root.join("bin"), false)?;
        safe_dir(parent(&self.immutable(m))?, false)?;
        check_owner(
            &self.immutable(m),
            &fs::symlink_metadata(self.immutable(m))?,
        )?;
        #[cfg(windows)]
        check_owner(&self.stable(), &fs::symlink_metadata(self.stable())?)?;
        ensure!(
            cache.hash(&self.immutable(m))? == m.sha256,
            "CLI runtime digest does not match its receipt"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                fs::metadata(self.immutable(m))?.permissions().mode() & 0o100 != 0,
                "CLI runtime is not executable"
            );
        }
        Ok(CliRuntime {
            executable: self.stable(),
            version: m.version.clone(),
            sha256: m.sha256.clone(),
            delivery_protocols: m.delivery_protocols.clone(),
        })
    }

    fn runtime(&self, r: &Receipt) -> Result<CliRuntime> {
        self.runtime_with_cache(r, &mut RuntimeDigestCache::default())
    }

    pub fn resolve(&self) -> Result<CliRuntime> {
        let _lock = self.lock()?;
        self.runtime(&self.receipt()?)
    }

    /// Recheck ownership and receipts, but reuse an unchanged binary's verified digest.
    pub fn resolve_cached(&self, cache: &mut RuntimeDigestCache) -> Result<CliRuntime> {
        let _lock = self.lock()?;
        self.runtime_with_cache(&self.receipt()?, cache)
    }

    pub fn runtime_for_test(&self) -> Result<(File, CliRuntime)> {
        let lock = self.lock()?;
        let runtime = self.runtime(&self.receipt()?)?;
        Ok((lock, runtime))
    }

    pub fn state(&self, diagnostic: Option<String>) -> CliInstallState {
        match self.resolve() {
            Ok(runtime) => CliInstallState {
                runtime: Some(runtime),
                diagnostic,
            },
            Err(error) => CliInstallState {
                runtime: None,
                diagnostic: diagnostic.or_else(|| Some(error.to_string())),
            },
        }
    }

    pub fn reconcile_at_startup(&self) -> Result<()> {
        retry_startup(|| self.reconcile(), std::time::Duration::from_secs(5))
    }

    pub fn reconcile(&self) -> Result<()> {
        let _lock = self.lock()?;
        let mut r = self.receipt()?;
        let obsolete = r.previous.clone();
        let m: Manifest = serde_json::from_slice(
            &read_file(&self.payload.join("manifest.json"), MAX_JSON).context(
                "Bundled CLI manifest is missing or unreadable; reinstall the desktop app",
            )?,
        )
        .context("Bundled CLI manifest is invalid; reinstall the desktop app")?;
        validate_manifest(&m, &self.target)?;
        let bytes = read_file(&self.payload.join(file_name()), MAX_BINARY).context(
            "Bundled CLI executable is missing or unreadable; reinstall the desktop app",
        )?;
        ensure!(
            digest(&bytes) == m.sha256,
            "Bundled CLI is corrupt; reinstall the desktop app"
        );
        let immutable = self.immutable(&m);
        safe_dir(parent(&immutable)?, true)?;
        if present(&immutable)? {
            check_owner(&immutable, &fs::symlink_metadata(&immutable)?)?;
            ensure!(
                hash(&immutable)? == m.sha256,
                "Immutable CLI payload was modified; preserve it for review"
            );
        } else {
            atomic_bytes(
                &immutable,
                &bytes,
                true,
                Some(&self.payload.join(file_name())),
                false,
            )?;
        }
        ensure!(
            !present(&self.stable())? || self.owned_entry(&self.stable(), &r),
            "Runtime destination is unrelated or changed; preserve it for review"
        );
        safe_dir(parent(&self.stable())?, true)?;
        let current = r.pending.is_none()
            && r.active.as_ref() == Some(&m)
            && self.matches(&self.stable(), &m);
        if !current {
            // Persist both generations before the stable entry changes.
            r.pending = Some(m.clone());
            self.save(&r)?;
            self.publish(&self.stable(), &m, &r)?;
            if let Some(active) = r.active.take()
                && active.sha256 != m.sha256
            {
                r.previous = Some(active);
            }
            r.active = Some(m.clone());
            r.pending = None;
            self.save(&r)?;
        }
        if let Some(obsolete) = obsolete
            && obsolete.sha256 != m.sha256
            && r.previous
                .as_ref()
                .is_none_or(|p| p.sha256 != obsolete.sha256)
        {
            let path = self.immutable(&obsolete);
            if present(&path)? {
                safe_dir(parent(&path)?, false)?;
                check_owner(&path, &fs::symlink_metadata(&path)?)?;
                ensure!(
                    hash(&path)? == obsolete.sha256,
                    "Old CLI artifact was modified; it was preserved"
                );
                fs::remove_file(&path).context("Old CLI artifact is locked; cleanup deferred")?;
                // Remove only this empty, known digest directory.
                let _ = fs::remove_dir(parent(&path)?);
            }
        }
        self.retire_terminal(&mut r)
    }

    fn retire_terminal(&self, r: &mut Receipt) -> Result<()> {
        if !r.terminal_seen || r.terminal_removed {
            return Ok(());
        }
        if present(&self.terminal)? {
            safe_dir(parent(&self.terminal)?, false)?;
            #[cfg(unix)]
            let owned = fs::read_link(&self.terminal).is_ok_and(|target| target == self.stable());
            #[cfg(windows)]
            let owned = {
                use std::os::windows::fs::MetadataExt;
                let metadata = fs::symlink_metadata(&self.terminal)?;
                match &r.terminal_sha256 {
                    Some(expected)
                        if metadata.is_file() && metadata.file_attributes() & 0x400 == 0 =>
                    {
                        hash(&self.terminal)
                            .context("Cannot read legacy CLI command; cleanup deferred")?
                            == *expected
                    }
                    _ => false,
                }
            };
            if owned {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    // Symlink mode bits do not control removal; the parent policy does.
                    ensure!(
                        fs::symlink_metadata(&self.terminal)?.uid() == unsafe { libc::geteuid() },
                        "Legacy CLI command belongs to another user"
                    );
                }
                #[cfg(windows)]
                check_owner(&self.terminal, &fs::symlink_metadata(&self.terminal)?)?;
                fs::remove_file(&self.terminal)
                    .context("Legacy CLI command is locked; cleanup deferred")?;
                sync_dir(parent(&self.terminal)?)?;
            }
        }
        // Retire the receipt claim without changing a replacement owned by the user.
        r.terminal_removed = true;
        self.save(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _temp: tempfile::TempDir,
        i: Installation,
    }
    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let base = temp.path().canonicalize().unwrap();
            let i = Installation {
                root: base.join("data/cli"),
                terminal: base.join("local/bin").join(file_name()),
                payload: base.join("bundle"),
                target: "synthetic-target".into(),
            };
            fs::create_dir(&i.payload).unwrap();
            let f = Self { _temp: temp, i };
            f.payload(b"synthetic native CLI one");
            f
        }
        fn payload(&self, bytes: &[u8]) {
            let m = Manifest {
                schema: 1,
                target: self.i.target.clone(),
                executable: file_name().into(),
                version: "0.1.0".into(),
                sha256: digest(bytes),
                delivery_protocols: vec!["delivery-v1".into()],
            };
            fs::write(self.i.payload.join(file_name()), bytes).unwrap();
            fs::write(
                self.i.payload.join("manifest.json"),
                serde_json::to_vec(&m).unwrap(),
            )
            .unwrap();
        }
    }

    #[test]
    fn fresh_two_restarts_and_bundle_relocation_keep_a_durable_runtime() {
        let f = Fixture::new();
        f.i.reconcile().unwrap();
        let first = f.i.resolve().unwrap();
        let receipt = fs::read(f.i.root.join("receipt.json")).unwrap();
        f.i.reconcile().unwrap();
        f.i.reconcile().unwrap();
        assert_eq!(fs::read(f.i.root.join("receipt.json")).unwrap(), receipt);
        assert!(f.i.state(None).runtime.is_some());
        fs::remove_dir_all(&f.i.payload).unwrap();
        assert_eq!(f.i.resolve().unwrap().executable, first.executable);
        assert!(!first.executable.starts_with(&f.i.payload));
    }

    #[test]
    fn cached_resolution_rechecks_the_receipt_and_follows_upgrades() {
        let f = Fixture::new();
        f.i.reconcile().unwrap();
        let mut cache = RuntimeDigestCache::default();
        let first = f.i.resolve_cached(&mut cache).unwrap();
        assert_eq!(f.i.resolve_cached(&mut cache).unwrap().sha256, first.sha256);
        f.payload(b"synthetic native CLI two");
        f.i.reconcile().unwrap();
        let second = f.i.resolve_cached(&mut cache).unwrap();
        assert_ne!(second.sha256, first.sha256);
        fs::write(f.i.root.join("receipt.json"), b"{}").unwrap();
        assert!(f.i.resolve_cached(&mut cache).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn digest_cache_reuses_only_the_same_unchanged_path() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::write(&first, b"synthetic binary").unwrap();
        fs::write(&second, b"synthetic binary").unwrap();
        let mut cache = RuntimeDigestCache::default();
        let actual = cache.hash(&first).unwrap();
        if binary_identity(&first).unwrap().is_none() {
            assert!(cache.entry.is_none());
            return;
        }
        cache.entry.as_mut().unwrap().2 = "cached digest".into();
        assert_eq!(cache.hash(&first).unwrap(), "cached digest");
        assert_eq!(cache.hash(&second).unwrap(), actual);
    }

    #[cfg(unix)]
    #[test]
    fn cached_resolution_rejects_same_size_edits_with_restored_modification_time() {
        let f = Fixture::new();
        f.i.reconcile().unwrap();
        let mut cache = RuntimeDigestCache::default();
        f.i.resolve_cached(&mut cache).unwrap();
        let manifest = f.i.receipt().unwrap().active.unwrap();
        let path = f.i.immutable(&manifest);
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        fs::write(&path, b"synthetic native CLI bad").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert!(
            f.i.resolve_cached(&mut cache)
                .unwrap_err()
                .to_string()
                .contains("digest")
        );
    }

    #[cfg(unix)]
    #[test]
    fn cached_resolution_rechecks_file_and_directory_policy() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        f.i.reconcile().unwrap();
        let mut cache = RuntimeDigestCache::default();
        f.i.resolve_cached(&mut cache).unwrap();
        let manifest = f.i.receipt().unwrap().active.unwrap();
        let path = f.i.immutable(&manifest);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(f.i.resolve_cached(&mut cache).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        f.i.resolve_cached(&mut cache).unwrap();
        let bin = f.i.root.join("bin");
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(f.i.resolve_cached(&mut cache).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn cached_resolution_rejects_a_replaced_binary_or_symlink() {
        let f = Fixture::new();
        f.i.reconcile().unwrap();
        let mut cache = RuntimeDigestCache::default();
        f.i.resolve_cached(&mut cache).unwrap();
        let manifest = f.i.receipt().unwrap().active.unwrap();
        let path = f.i.immutable(&manifest);
        let original = path.with_extension("original");
        fs::rename(&path, &original).unwrap();
        fs::copy(f.i.payload.join(file_name()), &path).unwrap();
        fs::set_permissions(&path, fs::metadata(&original).unwrap().permissions()).unwrap();
        f.i.resolve_cached(&mut cache).unwrap();
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&original, &path).unwrap();
        assert!(f.i.resolve_cached(&mut cache).is_err());
    }

    #[test]
    fn healthy_reconciliation_does_not_replace_the_receipt() {
        let f = Fixture::new();
        f.i.reconcile().unwrap();
        let path = f.i.root.join("receipt.json");
        let file = File::open(&path).unwrap();
        let before = file.metadata().unwrap();
        let bytes = fs::read(&path).unwrap();
        f.i.reconcile().unwrap();
        let after = fs::metadata(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(before.ino(), after.ino());
        }
    }

    #[cfg(unix)]
    #[test]
    fn healthy_reconciliation_rejects_a_redirected_stable_parent() {
        let f = Fixture::new();
        f.i.reconcile().unwrap();
        let receipt = fs::read(f.i.root.join("receipt.json")).unwrap();
        let bin = f.i.stable().parent().unwrap().to_path_buf();
        let outside = f.i.payload.join("redirected-bin");
        fs::rename(&bin, &outside).unwrap();
        std::os::unix::fs::symlink(&outside, &bin).unwrap();
        assert!(f.i.matches(
            &f.i.stable(),
            f.i.receipt().unwrap().active.as_ref().unwrap()
        ));
        assert!(
            f.i.reconcile()
                .unwrap_err()
                .to_string()
                .contains("redirected")
        );
        assert_eq!(fs::read(f.i.root.join("receipt.json")).unwrap(), receipt);
        assert_eq!(fs::read_link(&bin).unwrap(), outside);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ancestor_acls_preserve_safe_entries_and_reject_mutation_grants() {
        use std::process::Command;
        let f = Fixture::new();
        let ancestor = f.i.root.parent().unwrap();
        safe_dir(ancestor, true).unwrap();
        for rule in ["everyone deny delete", "everyone allow read"] {
            assert!(
                Command::new("/bin/chmod")
                    .args(["+a", rule])
                    .arg(ancestor)
                    .status()
                    .unwrap()
                    .success()
            );
            f.i.reconcile().unwrap();
            assert!(
                Command::new("/bin/chmod")
                    .arg("-N")
                    .arg(ancestor)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        for permission in ["delete_child", "add_file", "writesecurity", "chown"] {
            let rule = format!("everyone allow {permission}");
            assert!(
                Command::new("/bin/chmod")
                    .args(["+a", &rule])
                    .arg(ancestor)
                    .status()
                    .unwrap()
                    .success()
            );
            let before = Command::new("/bin/ls")
                .arg("-lde")
                .arg(ancestor)
                .output()
                .unwrap()
                .stdout;
            assert!(f.i.reconcile().unwrap_err().to_string().contains("ACL"));
            assert_eq!(
                Command::new("/bin/ls")
                    .arg("-lde")
                    .arg(ancestor)
                    .output()
                    .unwrap()
                    .stdout,
                before
            );
            assert!(
                Command::new("/bin/chmod")
                    .arg("-N")
                    .arg(ancestor)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        f.i.reconcile().unwrap();
    }

    #[test]
    fn fresh_install_and_repair_do_not_publish_a_terminal_command() {
        let f = Fixture::new();
        f.i.reconcile().unwrap();
        f.i.reconcile().unwrap();
        assert!(!present(&f.i.terminal).unwrap());
        assert!(!f.i.terminal.parent().unwrap().exists());
        assert!(!f.i.receipt().unwrap().terminal_seen);
        assert!(f.i.resolve().is_ok());
    }

    #[test]
    fn state_keeps_the_actionable_reconciliation_error() {
        let f = Fixture::new();
        let state =
            f.i.state(Some("Bundled runtime is corrupt; reinstall the app".into()));
        assert!(state.runtime.is_none());
        assert_eq!(
            state.diagnostic.as_deref(),
            Some("Bundled runtime is corrupt; reinstall the app")
        );
    }

    #[test]
    fn unrelated_export_is_preserved_while_the_internal_runtime_remains_available() {
        let f = Fixture::new();
        safe_dir(f.i.terminal.parent().unwrap(), true).unwrap();
        fs::write(&f.i.terminal, b"unrelated").unwrap();
        f.i.reconcile().unwrap();
        assert_eq!(fs::read(&f.i.terminal).unwrap(), b"unrelated");
        assert!(f.i.resolve().is_ok());
    }

    fn seed_legacy_export(f: &Fixture) {
        f.i.reconcile().unwrap();
        safe_dir(f.i.terminal.parent().unwrap(), true).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(f.i.stable(), &f.i.terminal).unwrap();
        #[cfg(windows)]
        fs::copy(f.i.stable(), &f.i.terminal).unwrap();
        let mut receipt = f.i.receipt().unwrap();
        receipt.terminal_seen = true;
        receipt.terminal_sha256 = Some(f.i.resolve().unwrap().sha256);
        f.i.save(&receipt).unwrap();
    }

    #[test]
    fn owned_legacy_export_is_retired_even_during_an_upgrade() {
        let f = Fixture::new();
        seed_legacy_export(&f);
        f.payload(b"new internal runtime");
        f.i.reconcile().unwrap();
        f.i.reconcile().unwrap();
        assert!(!present(&f.i.terminal).unwrap());
        assert!(f.i.receipt().unwrap().terminal_removed);
        assert!(f.i.resolve().is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn unreadable_legacy_export_remains_retryable_until_the_handle_is_released() {
        use std::os::windows::fs::OpenOptionsExt;

        let f = Fixture::new();
        seed_legacy_export(&f);
        let handle = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&f.i.terminal)
            .unwrap();
        let error = f.i.reconcile().unwrap_err();
        assert!(error.to_string().contains("Cannot read legacy CLI command"));
        assert!(!f.i.receipt().unwrap().terminal_removed);
        assert!(present(&f.i.terminal).unwrap());
        assert!(f.i.resolve().is_ok());

        drop(handle);
        f.i.reconcile().unwrap();
        assert!(!present(&f.i.terminal).unwrap());
        assert!(f.i.receipt().unwrap().terminal_removed);
        assert!(f.i.resolve().is_ok());
    }

    #[test]
    fn missing_owned_export_is_not_recreated() {
        let f = Fixture::new();
        seed_legacy_export(&f);
        fs::remove_file(&f.i.terminal).unwrap();
        f.i.reconcile().unwrap();
        assert!(!present(&f.i.terminal).unwrap());
        assert!(f.i.receipt().unwrap().terminal_removed);
        assert!(f.i.resolve().is_ok());
    }

    #[test]
    fn replacement_directory_is_preserved_without_blocking_reconciliation() {
        let f = Fixture::new();
        seed_legacy_export(&f);
        fs::remove_file(&f.i.terminal).unwrap();
        fs::create_dir(&f.i.terminal).unwrap();
        fs::write(f.i.terminal.join("keep"), b"user data").unwrap();
        f.i.reconcile().unwrap();
        f.i.reconcile().unwrap();
        assert_eq!(fs::read(f.i.terminal.join("keep")).unwrap(), b"user data");
        assert!(f.i.receipt().unwrap().terminal_removed);
        assert!(f.i.resolve().is_ok());
    }

    #[test]
    fn changed_legacy_export_is_preserved() {
        let f = Fixture::new();
        seed_legacy_export(&f);
        fs::remove_file(&f.i.terminal).unwrap();
        fs::write(&f.i.terminal, b"user replacement").unwrap();
        f.i.reconcile().unwrap();
        f.i.reconcile().unwrap();
        assert_eq!(fs::read(&f.i.terminal).unwrap(), b"user replacement");
        assert!(f.i.receipt().unwrap().terminal_removed);
        assert!(f.i.resolve().is_ok());
    }

    #[test]
    fn upgrade_preserves_previous_and_rejects_corrupt_bundle() {
        let f = Fixture::new();
        f.i.reconcile().unwrap();
        let first = f.i.resolve().unwrap();
        f.payload(b"synthetic native CLI two");
        f.i.reconcile().unwrap();
        let second = f.i.resolve().unwrap();
        assert_ne!(first.sha256, second.sha256);
        let r = f.i.receipt().unwrap();
        assert_eq!(r.previous.as_ref().unwrap().sha256, first.sha256);
        assert!(f.i.immutable(r.previous.as_ref().unwrap()).is_file());
        fs::write(f.i.payload.join(file_name()), b"corrupt").unwrap();
        assert!(f.i.reconcile().is_err());
        assert_eq!(f.i.resolve().unwrap().sha256, second.sha256);
    }

    #[test]
    fn upgrades_keep_only_the_current_and_previous_unchanged_artifacts() {
        let f = Fixture::new();
        let mut paths = Vec::new();
        for bytes in [b"version one".as_slice(), b"version two", b"version three"] {
            f.payload(bytes);
            f.i.reconcile().unwrap();
            let r = f.i.receipt().unwrap();
            paths.push(f.i.immutable(r.active.as_ref().unwrap()));
        }
        assert!(!paths[0].exists());
        assert!(paths[1].exists() && paths[2].exists());
    }

    #[test]
    fn legacy_desktop_command_remains_unchanged() {
        let f = Fixture::new();
        safe_dir(f.i.terminal.parent().unwrap(), true).unwrap();
        let legacy = f.i.terminal.parent().unwrap().join(if cfg!(windows) {
            "antiburn.exe"
        } else {
            "antiburn"
        });
        fs::write(&legacy, b"existing desktop launcher").unwrap();
        f.i.reconcile().unwrap();
        f.i.reconcile().unwrap();
        assert_eq!(fs::read(legacy).unwrap(), b"existing desktop launcher");
    }

    #[test]
    fn cli_test_holds_the_installation_generation_until_completion() {
        let f = Fixture::new();
        f.i.reconcile().unwrap();
        let (guard, runtime) = f.i.runtime_for_test().unwrap();
        assert!(f.i.reconcile().unwrap_err().to_string().contains("busy"));
        drop(guard);
        assert_eq!(f.i.resolve().unwrap().sha256, runtime.sha256);
        f.i.reconcile().unwrap();
    }

    #[test]
    fn startup_retries_after_a_concurrent_readiness_lock_is_released() {
        use std::sync::mpsc;
        use std::time::Duration;

        let f = Fixture::new();
        let readiness_lock = f.i.lock().unwrap();
        std::thread::scope(|scope| {
            let (blocked_tx, blocked_rx) = mpsc::channel();
            let (released_tx, released_rx) = mpsc::channel();
            let installation = &f.i;
            let startup = scope.spawn(move || {
                let mut attempts = 0;
                let result = retry_startup(
                    || {
                        attempts += 1;
                        let result = installation.reconcile();
                        if attempts == 1 {
                            assert!(result.is_err());
                            blocked_tx.send(()).unwrap();
                            released_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                        }
                        result
                    },
                    Duration::from_secs(5),
                );
                (result, attempts)
            });
            blocked_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(!f.i.root.join("receipt.json").exists());
            drop(readiness_lock);
            released_tx.send(()).unwrap();
            let (result, attempts) = startup.join().unwrap();
            result.unwrap();
            assert_eq!(attempts, 2);
        });
        assert!(f.i.resolve().is_ok());
        f.i.reconcile_at_startup().unwrap();
    }

    #[test]
    fn startup_lock_wait_is_bounded_and_does_not_publish_without_a_lock() {
        let f = Fixture::new();
        let _lock = f.i.lock().unwrap();
        let mut attempts = 0;
        let result = retry_startup(
            || {
                attempts += 1;
                f.i.reconcile()
            },
            std::time::Duration::ZERO,
        );
        assert!(result.is_err());
        assert_eq!(attempts, 1);
        assert!(!f.i.root.join("receipt.json").exists());
        assert!(!f.i.stable().exists());
    }

    #[test]
    fn startup_does_not_retry_payload_or_ownership_errors() {
        let f = Fixture::new();
        fs::write(f.i.payload.join(file_name()), b"corrupt").unwrap();
        let mut attempts = 0;
        let error = retry_startup(
            || {
                attempts += 1;
                f.i.reconcile()
            },
            std::time::Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(error.to_string().contains("corrupt"));
        assert_eq!(attempts, 1);
        assert!(!f.i.root.join("receipt.json").exists());
    }

    #[test]
    fn concurrent_installation_has_a_bounded_busy_result() {
        let f = Fixture::new();
        let lock = f.i.lock().unwrap();
        assert!(f.i.reconcile().unwrap_err().to_string().contains("busy"));
        drop(lock);
        f.i.reconcile().unwrap();
    }

    #[test]
    fn state_reports_current_resolution_after_lock_contention() {
        let f = Fixture::new();
        f.i.reconcile().unwrap();
        let lock = f.i.lock().unwrap();
        let busy = f.i.state(None);
        assert!(busy.runtime.is_none());
        assert!(busy.diagnostic.unwrap().contains("busy"));
        drop(lock);
        let ready = f.i.state(None);
        assert!(ready.runtime.is_some());
        assert!(ready.diagnostic.is_none());
    }

    #[test]
    fn altered_receipt_or_immutable_bytes_cannot_authorize_runtime() {
        let f = Fixture::new();
        f.i.reconcile().unwrap();
        let r = f.i.receipt().unwrap();
        fs::write(f.i.immutable(r.active.as_ref().unwrap()), b"modified").unwrap();
        assert!(f.i.resolve().is_err());
        assert!(f.i.reconcile().is_err());
        fs::write(f.i.root.join("receipt.json"), b"{}").unwrap();
        assert!(f.i.resolve().is_err());
        assert!(f.i.reconcile().is_err());
    }

    #[test]
    fn an_existing_runtime_without_a_receipt_is_not_adopted() {
        let f = Fixture::new();
        safe_dir(f.i.stable().parent().unwrap(), true).unwrap();
        fs::write(f.i.stable(), b"unrelated runtime").unwrap();
        assert!(f.i.reconcile().is_err());
        assert_eq!(fs::read(f.i.stable()).unwrap(), b"unrelated runtime");
        assert!(!present(&f.i.root.join("receipt.json")).unwrap());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn staging_preserves_quarantine_attributes() {
        use std::process::Command;
        let f = Fixture::new();
        let source = f.i.payload.join(file_name());
        let quarantine = "0081;00000000;Antiburn synthetic fixture;";
        assert!(
            Command::new("/usr/bin/xattr")
                .args(["-w", "com.apple.quarantine", quarantine])
                .arg(&source)
                .status()
                .unwrap()
                .success()
        );
        f.i.reconcile().unwrap();
        let r = f.i.receipt().unwrap();
        let output = Command::new("/usr/bin/xattr")
            .args(["-p", "com.apple.quarantine"])
            .arg(f.i.immutable(r.active.as_ref().unwrap()))
            .output()
            .unwrap();
        assert!(output.status.success());
        let value = String::from_utf8(output.stdout).unwrap();
        let flags = u32::from_str_radix(value.split(';').next().unwrap(), 16).unwrap();
        // macOS can change the timestamp and upper flags when it copies quarantine.
        assert_eq!(flags & 0xff, 0x81);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn staging_inherits_quarantine_from_the_enclosing_app_bundle() {
        use std::process::Command;
        let mut f = Fixture::new();
        let bundle = f._temp.path().join("Antiburn.app");
        let payload = bundle.join("Contents/Resources/cli-payload");
        fs::create_dir_all(&payload).unwrap();
        fs::rename(f.i.payload.join(file_name()), payload.join(file_name())).unwrap();
        fs::rename(
            f.i.payload.join("manifest.json"),
            payload.join("manifest.json"),
        )
        .unwrap();
        f.i.payload = payload;
        assert!(
            Command::new("/usr/bin/xattr")
                .args([
                    "-w",
                    "com.apple.quarantine",
                    "0081;00000000;Antiburn fixture;"
                ])
                .arg(&bundle)
                .status()
                .unwrap()
                .success()
        );
        f.i.reconcile().unwrap();
        let receipt = f.i.receipt().unwrap();
        let result = Command::new("/usr/bin/xattr")
            .args(["-p", "com.apple.quarantine"])
            .arg(f.i.immutable(receipt.active.as_ref().unwrap()))
            .output()
            .unwrap();
        assert!(result.status.success());
        let value = String::from_utf8(result.stdout).unwrap();
        assert_eq!(
            u32::from_str_radix(value.split(';').next().unwrap(), 16).unwrap() & 0xff,
            0x81
        );
        assert!(value.contains("Antiburn fixture"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn extended_acl_is_preserved_and_blocks_installation() {
        use std::process::Command;
        let f = Fixture::new();
        safe_dir(&f.i.root, true).unwrap();
        assert!(
            Command::new("/bin/chmod")
                .args(["+a", "everyone deny delete"])
                .arg(&f.i.root)
                .status()
                .unwrap()
                .success()
        );
        let result = f.i.reconcile();
        assert!(
            Command::new("/bin/chmod")
                .arg("-N")
                .arg(&f.i.root)
                .status()
                .unwrap()
                .success()
        );
        assert!(result.unwrap_err().to_string().contains("ACL"));
    }

    #[cfg(windows)]
    #[test]
    fn inherited_child_file_write_permissions_block_publication() {
        use std::process::Command;
        let f = Fixture::new();
        safe_dir(&f.i.root, true).unwrap();
        assert!(
            Command::new("icacls")
                .arg(&f.i.root)
                .args(["/grant", "*S-1-1-0:(OI)(IO)(M)"])
                .status()
                .unwrap()
                .success()
        );
        let result = atomic_bytes(&f.i.root.join("test.json"), b"{}", false, None, false);
        assert!(
            Command::new("icacls")
                .arg(&f.i.root)
                .args(["/remove:g", "*S-1-1-0"])
                .status()
                .unwrap()
                .success()
        );
        assert!(result.unwrap_err().to_string().contains("unrelated users"));
        assert!(!f.i.root.join("test.json").exists());
    }

    #[cfg(windows)]
    #[test]
    fn unrelated_ancestor_write_permissions_block_installation() {
        use std::process::Command;
        let f = Fixture::new();
        safe_dir(&f.i.root, true).unwrap();
        let parent = f.i.root.parent().unwrap();
        assert!(
            Command::new("icacls")
                .arg(parent)
                .args(["/grant", "*S-1-1-0:(DC)"])
                .status()
                .unwrap()
                .success()
        );
        let result = f.i.reconcile();
        assert!(
            Command::new("icacls")
                .arg(parent)
                .args(["/remove:g", "*S-1-1-0"])
                .status()
                .unwrap()
                .success()
        );
        assert!(result.unwrap_err().to_string().contains("unrelated users"));
    }

    #[cfg(windows)]
    #[test]
    fn locked_windows_entry_defers_upgrade_and_keeps_the_prior_runtime() {
        use std::os::windows::fs::OpenOptionsExt;
        let f = Fixture::new();
        f.i.reconcile().unwrap();
        let prior = f.i.resolve().unwrap().sha256;
        let lock = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(f.i.stable())
            .unwrap();
        f.payload(b"next synthetic native runtime");
        assert!(f.i.reconcile().is_err());
        assert_eq!(f.i.resolve().unwrap().sha256, prior);
        drop(lock);
        f.i.reconcile().unwrap();
        assert_ne!(f.i.resolve().unwrap().sha256, prior);
    }

    #[test]
    #[ignore = "Build a native CLI payload with scripts/build-cli-sidecar.mjs first"]
    fn native_payload_runs_offline_with_a_minimal_path_after_bundle_removal() {
        let mut f = Fixture::new();
        let original =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/src-tauri/cli-payload");
        for name in [file_name(), "manifest.json"] {
            fs::copy(original.join(name), f.i.payload.join(name)).unwrap();
        }
        f.i.target = crate::TARGET.into();
        f.i.reconcile().unwrap();
        fs::remove_dir_all(&f.i.payload).unwrap();
        let runtime = f.i.resolve().unwrap();
        let output = std::process::Command::new(&runtime.executable)
            .args(["capabilities", "--json"])
            .env_clear()
            .env("PATH", "")
            .env("HOME", f._temp.path())
            .env("USERPROFILE", f._temp.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["version"], runtime.version);
        assert_eq!(
            value["delivery_protocols"],
            serde_json::json!(["delivery-v1"])
        );
    }

    #[test]
    fn wrong_target_or_oversized_manifest_is_rejected() {
        let mut f = Fixture::new();
        f.i.target = "another-target".into();
        assert!(f.i.reconcile().is_err());
        fs::write(
            f.i.payload.join("manifest.json"),
            vec![b' '; MAX_JSON as usize + 1],
        )
        .unwrap();
        assert!(f.i.reconcile().is_err());
        assert!(!present(&f.i.stable()).unwrap());
    }

    #[test]
    fn interruption_after_publish_is_resolvable_and_reconciles() {
        let f = Fixture::new();
        f.i.reconcile().unwrap();
        f.payload(b"synthetic next runtime");
        let m: Manifest =
            serde_json::from_slice(&fs::read(f.i.payload.join("manifest.json")).unwrap()).unwrap();
        let mut r = f.i.receipt().unwrap();
        safe_dir(f.i.immutable(&m).parent().unwrap(), true).unwrap();
        atomic_bytes(
            &f.i.immutable(&m),
            b"synthetic next runtime",
            true,
            None,
            false,
        )
        .unwrap();
        r.pending = Some(m.clone());
        f.i.save(&r).unwrap();
        f.i.publish(&f.i.stable(), &m, &r).unwrap();
        assert_eq!(f.i.resolve().unwrap().sha256, m.sha256);
        f.i.reconcile().unwrap();
        assert!(f.i.receipt().unwrap().pending.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn redirected_directories_locks_and_commands_are_preserved() {
        let f = Fixture::new();
        let outside = f.i.payload.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::create_dir_all(f.i.root.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &f.i.root).unwrap();
        assert!(f.i.reconcile().is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        fs::remove_file(&f.i.root).unwrap();
        safe_dir(&f.i.root, true).unwrap();
        std::os::unix::fs::symlink(f.i.payload.join(file_name()), f.i.root.join("install.lock"))
            .unwrap();
        assert!(f.i.reconcile().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn broken_owned_runtime_is_repaired_and_changed_export_is_preserved() {
        let f = Fixture::new();
        f.i.reconcile().unwrap();
        fs::remove_file(f.i.stable()).unwrap();
        f.i.reconcile().unwrap();
        assert!(f.i.state(None).runtime.is_some());
        assert!(!present(&f.i.terminal).unwrap());

        seed_legacy_export(&f);
        fs::remove_file(&f.i.terminal).unwrap();
        let changed_target = f.i.payload.join(file_name());
        std::os::unix::fs::symlink(&changed_target, &f.i.terminal).unwrap();
        f.i.reconcile().unwrap();
        assert_eq!(fs::read_link(&f.i.terminal).unwrap(), changed_target);
        assert!(f.i.resolve().is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_permissions_fail_without_broadening_access() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        safe_dir(&f.i.root, true).unwrap();
        fs::set_permissions(&f.i.root, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(f.i.reconcile().is_err());
        assert_eq!(
            fs::metadata(&f.i.root).unwrap().permissions().mode() & 0o777,
            0o777
        );
    }
}
