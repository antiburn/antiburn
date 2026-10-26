use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_BINARY: u64 = 256 * 1024 * 1024;
const MAX_JSON: u64 = 16 * 1024;
const IDENTITY: &str = "ai.antiburn.desktop.cli";

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliInstallState {
    pub runtime: Option<CliRuntime>,
    pub terminal_path: PathBuf,
    pub terminal_installed: bool,
    pub bin_dir_on_app_path: bool,
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

    fn runtime(&self, r: &Receipt) -> Result<CliRuntime> {
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
            hash(&self.immutable(m))? == m.sha256,
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

    pub fn resolve(&self) -> Result<CliRuntime> {
        let _lock = self.lock()?;
        self.runtime(&self.receipt()?)
    }

    pub fn state(&self, diagnostic: Option<String>) -> CliInstallState {
        let result = (|| -> Result<(Option<CliRuntime>, bool, Option<String>)> {
            let _lock = self.lock()?;
            let r = self.receipt()?;
            let runtime = self.runtime(&r).ok();
            #[cfg(unix)]
            let terminal_matches = fs::read_link(&self.terminal).is_ok_and(|p| p == self.stable());
            #[cfg(windows)]
            let terminal_matches = r.terminal_sha256.is_some()
                && hash(&self.terminal).ok().as_ref() == r.terminal_sha256.as_ref();
            let installed =
                r.terminal_seen && !r.terminal_removed && terminal_matches && runtime.is_some();
            let detail = if present(&self.terminal)? && !terminal_matches {
                Some("Terminal command collides with an unrelated or modified command; it was preserved".into())
            } else {
                diagnostic.clone()
            };
            Ok((runtime, installed, detail))
        })();
        let (runtime, terminal_installed, diagnostic) = match result {
            Ok((runtime, installed, detail)) => (runtime, installed, detail),
            Err(e) => (None, false, Some(e.to_string())),
        };
        CliInstallState {
            runtime,
            terminal_installed,
            terminal_path: self.terminal.clone(),
            bin_dir_on_app_path: std::env::var_os("PATH").is_some_and(|p| {
                std::env::split_paths(&p).any(|p| Some(p.as_path()) == self.terminal.parent())
            }),
            diagnostic,
        }
    }

    pub fn reconcile(&self, repair: bool) -> Result<()> {
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
        // Persist both generations before the stable entry changes.
        r.pending = Some(m.clone());
        self.save(&r)?;
        self.publish(&self.stable(), &m, &r)?;
        let terminal_present = present(&self.terminal)?;
        if r.terminal_seen && !terminal_present && !repair {
            r.terminal_removed = true;
        }
        if repair {
            r.terminal_removed = false;
        }
        let terminal_result = if !r.terminal_removed {
            self.publish_terminal(&m, &mut r)
        } else {
            Ok(())
        };
        if let Some(active) = r.active.take()
            && active.sha256 != m.sha256
        {
            r.previous = Some(active);
        }
        r.active = Some(m.clone());
        r.pending = None;
        if terminal_result.is_ok() && !r.terminal_removed {
            r.terminal_sha256 = Some(m.sha256.clone());
        }
        self.save(&r)?;
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
        terminal_result
    }

    fn publish_terminal(&self, m: &Manifest, r: &mut Receipt) -> Result<()> {
        #[cfg(unix)]
        {
            // Terminal links target the stable entry, not the app bundle.
            safe_dir(parent(&self.terminal)?, true)?;
            if present(&self.terminal)? {
                ensure!(
                    r.terminal_seen
                        && fs::read_link(&self.terminal).is_ok_and(|p| p == self.stable()),
                    "Terminal command collides with an unrelated command"
                );
                return Ok(());
            }
            r.terminal_seen = true;
            self.save(r)?;
            std::os::unix::fs::symlink(self.stable(), &self.terminal)?;
            sync_dir(parent(&self.terminal)?)?;
            let _ = m;
            Ok(())
        }
        #[cfg(windows)]
        {
            safe_dir(parent(&self.terminal)?, true)?;
            let replacing = present(&self.terminal)?;
            if replacing {
                let digest = hash(&self.terminal)?;
                ensure!(
                    r.terminal_seen
                        && (r.terminal_sha256.as_ref() == Some(&digest)
                            || self.owned_entry(&self.terminal, r)),
                    "Terminal command collides with an unrelated command"
                );
            }
            r.terminal_seen = true;
            self.save(r)?;
            atomic_bytes(
                &self.terminal,
                &read_file(&self.immutable(m), MAX_BINARY)?,
                true,
                Some(&self.immutable(m)),
                replacing,
            )
        }
    }

    pub fn remove_terminal(&self) -> Result<()> {
        let _lock = self.lock()?;
        let mut r = self.receipt()?;
        if present(&self.terminal)? {
            safe_dir(parent(&self.terminal)?, false)?;
            #[cfg(unix)]
            let owned =
                r.terminal_seen && fs::read_link(&self.terminal).is_ok_and(|p| p == self.stable());
            #[cfg(windows)]
            let owned = r.terminal_seen
                && hash(&self.terminal).ok().as_ref() == r.terminal_sha256.as_ref()
                && r.terminal_sha256.is_some();
            ensure!(
                owned,
                "Terminal command was modified or is unrelated; it was preserved"
            );
            r.terminal_removed = true;
            self.save(&r)?;
            fs::remove_file(&self.terminal)?;
        } else {
            r.terminal_removed = true;
            self.save(&r)?;
        }
        Ok(())
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
        f.i.reconcile(false).unwrap();
        let first = f.i.resolve().unwrap();
        let receipt = fs::read(f.i.root.join("receipt.json")).unwrap();
        f.i.reconcile(false).unwrap();
        f.i.reconcile(false).unwrap();
        assert_eq!(fs::read(f.i.root.join("receipt.json")).unwrap(), receipt);
        assert!(f.i.state(None).terminal_installed);
        fs::remove_dir_all(&f.i.payload).unwrap();
        assert_eq!(f.i.resolve().unwrap().executable, first.executable);
        assert!(!first.executable.starts_with(&f.i.payload));
    }

    #[test]
    fn unrelated_command_survives_startup_repair_and_remove() {
        let f = Fixture::new();
        safe_dir(f.i.terminal.parent().unwrap(), true).unwrap();
        fs::write(&f.i.terminal, b"unrelated").unwrap();
        for repair in [false, true] {
            assert!(f.i.reconcile(repair).is_err());
        }
        assert!(f.i.remove_terminal().is_err());
        assert_eq!(fs::read(&f.i.terminal).unwrap(), b"unrelated");
        assert!(f.i.resolve().is_ok());
    }

    #[test]
    fn user_removal_is_persistent_until_repair() {
        let f = Fixture::new();
        f.i.reconcile(false).unwrap();
        fs::remove_file(&f.i.terminal).unwrap();
        f.i.reconcile(false).unwrap();
        f.i.reconcile(false).unwrap();
        assert!(!present(&f.i.terminal).unwrap());
        f.i.reconcile(true).unwrap();
        assert!(f.i.state(None).terminal_installed);
        f.i.remove_terminal().unwrap();
        f.i.reconcile(false).unwrap();
        assert!(!present(&f.i.terminal).unwrap());
        assert!(f.i.resolve().is_ok());
    }

    #[test]
    fn upgrade_preserves_previous_and_rejects_corrupt_bundle() {
        let f = Fixture::new();
        f.i.reconcile(false).unwrap();
        let first = f.i.resolve().unwrap();
        f.payload(b"synthetic native CLI two");
        f.i.reconcile(false).unwrap();
        let second = f.i.resolve().unwrap();
        assert_ne!(first.sha256, second.sha256);
        let r = f.i.receipt().unwrap();
        assert_eq!(r.previous.as_ref().unwrap().sha256, first.sha256);
        assert!(f.i.immutable(r.previous.as_ref().unwrap()).is_file());
        fs::write(f.i.payload.join(file_name()), b"corrupt").unwrap();
        assert!(f.i.reconcile(false).is_err());
        assert_eq!(f.i.resolve().unwrap().sha256, second.sha256);
    }

    #[test]
    fn upgrades_keep_only_the_current_and_previous_unchanged_artifacts() {
        let f = Fixture::new();
        let mut paths = Vec::new();
        for bytes in [b"version one".as_slice(), b"version two", b"version three"] {
            f.payload(bytes);
            f.i.reconcile(false).unwrap();
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
        f.i.reconcile(false).unwrap();
        f.i.remove_terminal().unwrap();
        f.i.reconcile(true).unwrap();
        assert_eq!(fs::read(legacy).unwrap(), b"existing desktop launcher");
    }

    #[test]
    fn concurrent_installation_has_a_bounded_busy_result() {
        let f = Fixture::new();
        let lock = f.i.lock().unwrap();
        assert!(
            f.i.reconcile(false)
                .unwrap_err()
                .to_string()
                .contains("busy")
        );
        drop(lock);
        f.i.reconcile(false).unwrap();
    }

    #[test]
    fn altered_receipt_or_immutable_bytes_cannot_authorize_runtime() {
        let f = Fixture::new();
        f.i.reconcile(false).unwrap();
        let r = f.i.receipt().unwrap();
        fs::write(f.i.immutable(r.active.as_ref().unwrap()), b"modified").unwrap();
        assert!(f.i.resolve().is_err());
        assert!(f.i.reconcile(true).is_err());
        fs::write(f.i.root.join("receipt.json"), b"{}").unwrap();
        assert!(f.i.resolve().is_err());
        assert!(f.i.reconcile(true).is_err());
    }

    #[test]
    fn an_existing_runtime_without_a_receipt_is_not_adopted() {
        let f = Fixture::new();
        safe_dir(f.i.stable().parent().unwrap(), true).unwrap();
        fs::write(f.i.stable(), b"unrelated runtime").unwrap();
        assert!(f.i.reconcile(false).is_err());
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
        f.i.reconcile(false).unwrap();
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
        f.i.reconcile(false).unwrap();
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
        let result = f.i.reconcile(false);
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
        let result = f.i.reconcile(false);
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
        f.i.reconcile(false).unwrap();
        let prior = f.i.resolve().unwrap().sha256;
        let lock = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(f.i.stable())
            .unwrap();
        f.payload(b"next synthetic native runtime");
        assert!(f.i.reconcile(false).is_err());
        assert_eq!(f.i.resolve().unwrap().sha256, prior);
        drop(lock);
        f.i.reconcile(false).unwrap();
        assert_ne!(f.i.resolve().unwrap().sha256, prior);
    }

    #[test]
    #[ignore = "Build a native CLI payload with scripts/build-cli-sidecar.mjs first"]
    fn native_payload_runs_offline_with_a_minimal_path_after_bundle_removal() {
        let mut f = Fixture::new();
        let original = Path::new(env!("CARGO_MANIFEST_DIR")).join("cli-payload");
        for name in [file_name(), "manifest.json"] {
            fs::copy(original.join(name), f.i.payload.join(name)).unwrap();
        }
        f.i.target = env!("ANTIBURN_CLI_TARGET").into();
        f.i.reconcile(false).unwrap();
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
        assert!(f.i.reconcile(false).is_err());
        fs::write(
            f.i.payload.join("manifest.json"),
            vec![b' '; MAX_JSON as usize + 1],
        )
        .unwrap();
        assert!(f.i.reconcile(false).is_err());
        assert!(!present(&f.i.stable()).unwrap());
    }

    #[test]
    fn interruption_after_publish_is_resolvable_and_reconciles() {
        let f = Fixture::new();
        f.i.reconcile(false).unwrap();
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
        f.i.reconcile(false).unwrap();
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
        assert!(f.i.reconcile(false).is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        fs::remove_file(&f.i.root).unwrap();
        safe_dir(&f.i.root, true).unwrap();
        std::os::unix::fs::symlink(f.i.payload.join(file_name()), f.i.root.join("install.lock"))
            .unwrap();
        assert!(f.i.reconcile(false).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn broken_owned_link_is_repairable_but_changed_link_is_preserved() {
        let f = Fixture::new();
        f.i.reconcile(false).unwrap();
        fs::remove_file(f.i.stable()).unwrap();
        f.i.reconcile(false).unwrap();
        assert!(f.i.state(None).terminal_installed);
        fs::remove_file(&f.i.terminal).unwrap();
        std::os::unix::fs::symlink("unrelated", &f.i.terminal).unwrap();
        assert!(f.i.reconcile(true).is_err());
        assert_eq!(
            fs::read_link(&f.i.terminal).unwrap(),
            Path::new("unrelated")
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_permissions_fail_without_broadening_access() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        safe_dir(&f.i.root, true).unwrap();
        fs::set_permissions(&f.i.root, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(f.i.reconcile(false).is_err());
        assert_eq!(
            fs::metadata(&f.i.root).unwrap().permissions().mode() & 0o777,
            0o777
        );
    }
}
