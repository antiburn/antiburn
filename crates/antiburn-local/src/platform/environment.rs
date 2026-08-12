// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Host and mounted-WSL discovery/execution environments.
//!
//! Windows users often keep repositories and agent state inside a WSL
//! distribution. Everything here works from the *mounted* namespace
//! (`\\\\wsl.localhost\\<distro>`): enumerating distributions, resolving each
//! one's default user, and translating paths between the Windows host and Linux
//! namespaces. Reading those files never starts a stopped distribution.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use super::process::headless_tokio_command;

const WSL_NAMESPACE: &str = r"\\wsl.localhost";
const WSL_LEGACY_NAMESPACE: &str = r"\\wsl$";
static MOUNTED_USERS: OnceLock<Mutex<std::collections::HashMap<String, String>>> = OnceLock::new();

/// Execution and identity boundary for an agent session or repository.
///
/// WSL identity is distribution-scoped and intentionally excludes the user:
/// scanning only ever uses the distribution's resolved default user, and a
/// later change to that user must not create a second logical environment for
/// sessions already persisted from the same distribution.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiscoveryEnvironment {
    /// The host operating system and its native process/filesystem namespace.
    #[default]
    Native,
    /// A currently mounted WSL distribution accessed from the Windows host.
    Wsl {
        /// Stable distribution name accepted by `wsl.exe --distribution`.
        distribution: String,
        /// Default Linux user used for commands inside the distribution.
        user: String,
    },
}

impl PartialEq for DiscoveryEnvironment {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Native, Self::Native) => true,
            (
                Self::Wsl {
                    distribution: left, ..
                },
                Self::Wsl {
                    distribution: right,
                    ..
                },
            ) => left.eq_ignore_ascii_case(right),
            _ => false,
        }
    }
}

impl Eq for DiscoveryEnvironment {}

impl std::hash::Hash for DiscoveryEnvironment {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&self.key(), state);
    }
}

/// Serde adapter that exposes only the optional distribution identity instead
/// of the internal user and execution model.
pub fn serialize_wsl_distro<S>(
    environment: &DiscoveryEnvironment,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    environment.wsl_distro().serialize(serializer)
}

/// Restores optional `wslDistro` data into an execution environment. Mounted
/// user data is used when available; persisted unavailable distros retain their
/// identity with an empty user and therefore cannot execute commands.
pub fn deserialize_wsl_distro<'de, D>(
    deserializer: D,
) -> std::result::Result<DiscoveryEnvironment, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let distro = Option::<String>::deserialize(deserializer)?;
    Ok(distro
        .and_then(|distribution| {
            environment_from_mounted_path(&PathBuf::from(WSL_NAMESPACE).join(&distribution))
                .or_else(|| {
                    valid_distribution_name(&distribution).then(|| DiscoveryEnvironment::Wsl {
                        distribution,
                        user: String::new(),
                    })
                })
        })
        .unwrap_or_default())
}

impl DiscoveryEnvironment {
    /// Stable, case-insensitive identity used in local cache and dedupe keys.
    pub fn key(&self) -> String {
        match self {
            Self::Native => "native".to_string(),
            Self::Wsl { distribution, .. } => format!("wsl:{}", distribution.to_ascii_lowercase()),
        }
    }

    /// Returns whether this environment uses host-native filesystem/processes.
    pub fn is_native(&self) -> bool {
        matches!(self, Self::Native)
    }

    /// Returns the user-visible WSL distribution name, when applicable.
    pub fn wsl_distro(&self) -> Option<&str> {
        match self {
            Self::Native => None,
            Self::Wsl { distribution, .. } => Some(distribution),
        }
    }

    /// Infers an environment from a mounted WSL UNC path.
    ///
    /// Recognized WSL paths remain WSL even if the distro disappears while the
    /// operation is in flight. In that case the user is empty, causing command
    /// configuration to fail closed rather than accidentally invoking host Git.
    pub fn from_mounted_path(path: &Path) -> Self {
        environment_from_mounted_path(path).unwrap_or_default()
    }

    /// Converts a transcript-derived absolute path into the host namespace.
    /// Native and already-host-addressable paths pass through unchanged.
    pub fn host_path(&self, path: &str) -> Result<PathBuf> {
        match self {
            Self::Wsl { distribution, .. } if path.starts_with('/') => {
                // Reuse the namespace that is actually mounted. Some Windows
                // configurations expose only the legacy `\\wsl$` namespace.
                if cfg!(target_os = "windows")
                    && let Some(root) = mounted_distribution_root(distribution)
                {
                    linux_path_under_root(&root, path)
                } else {
                    wsl_to_windows_path(distribution, path)
                }
            }
            _ => Ok(PathBuf::from(path)),
        }
    }
}

/// Agent filesystem locations expressed in both host and platform namespaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentContext {
    /// Environment that owns all paths and sessions discovered in this context.
    pub environment: DiscoveryEnvironment,
    /// Host-readable home (`C:\...`, `/home/...`, or a mounted WSL UNC path).
    pub home: PathBuf,
    /// Home as seen by processes in the environment (for example `/home/dev`).
    pub platform_home: PathBuf,
}

impl AgentContext {
    pub fn native(home: PathBuf) -> Self {
        Self {
            platform_home: home.clone(),
            home,
            environment: DiscoveryEnvironment::Native,
        }
    }
}

/// One mounted WSL distribution and its resolved default-user context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslEnvironmentInfo {
    pub context: AgentContext,
    pub distribution: String,
    pub user: String,
}

/// Enumerate only currently mounted distributions. Reading the namespace and
/// distro files does not start a stopped WSL VM.
pub async fn discover_wsl_environments() -> Vec<WslEnvironmentInfo> {
    if !cfg!(target_os = "windows") {
        return Vec::new();
    }
    discover_wsl_environments_in(Path::new(WSL_NAMESPACE), Path::new(WSL_LEGACY_NAMESPACE)).await
}

async fn discover_wsl_environments_in(
    namespace: &Path,
    legacy_namespace: &Path,
) -> Vec<WslEnvironmentInfo> {
    let distributions = match mounted_distribution_names(namespace)
        .await
        .filter(|items| !items.is_empty())
    {
        Some(distributions) => distributions,
        None => match mounted_distribution_names(legacy_namespace)
            .await
            .filter(|items| !items.is_empty())
        {
            Some(distributions) => distributions,
            None if namespace == Path::new(WSL_NAMESPACE) => running_distribution_names().await,
            None => Vec::new(),
        },
    };
    let mut found = Vec::new();
    for distribution in distributions {
        if !valid_distribution_name(&distribution) {
            continue;
        }
        let primary = namespace.join(&distribution);
        let legacy = legacy_namespace.join(&distribution);
        let root = if tokio::fs::metadata(&primary).await.is_ok() {
            primary
        } else if tokio::fs::metadata(&legacy).await.is_ok() {
            legacy
        } else {
            continue;
        };
        let wsl_conf = tokio::fs::read_to_string(root.join("etc/wsl.conf"))
            .await
            .unwrap_or_default();
        let passwd = match tokio::fs::read_to_string(root.join("etc/passwd")).await {
            Ok(passwd) => passwd,
            Err(_) => continue,
        };
        let configured_user = parse_wsl_conf_default_user(&wsl_conf);
        let Some((user, home)) = resolve_default_user(configured_user.as_deref(), &passwd) else {
            continue;
        };
        let Ok(mounted_home) = linux_path_under_root(&root, &home) else {
            continue;
        };
        found.push(WslEnvironmentInfo {
            context: AgentContext {
                environment: DiscoveryEnvironment::Wsl {
                    distribution: distribution.clone(),
                    user: user.clone(),
                },
                home: mounted_home,
                platform_home: PathBuf::from(&home),
            },
            distribution: distribution.clone(),
            user: user.clone(),
        });
        MOUNTED_USERS
            .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(distribution.to_ascii_lowercase(), user);
    }
    found.sort_by(|a, b| a.distribution.cmp(&b.distribution));
    found
}

async fn mounted_distribution_names(namespace: &Path) -> Option<Vec<String>> {
    let mut entries = tokio::fs::read_dir(namespace).await.ok()?;
    let mut distributions = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        distributions.push(entry.file_name().to_string_lossy().to_string());
    }
    Some(distributions)
}

/// Some Windows builds expose `\\wsl.localhost\Distro` while refusing to
/// enumerate the namespace root. `--list --running` is safe as a compatibility
/// fallback because it neither starts nor includes stopped distributions; each
/// result is still verified through the mounted namespace below.
async fn running_distribution_names() -> Vec<String> {
    let Ok(output) = headless_tokio_command("wsl.exe")
        .args(["--list", "--running", "--quiet"])
        .output()
        .await
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    decode_wsl_output(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|value| valid_distribution_name(value))
        .map(str::to_string)
        .collect()
}

fn decode_wsl_output(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && (bytes[1] == 0 || bytes.starts_with(&[0xff, 0xfe])) {
        let start = usize::from(bytes.starts_with(&[0xff, 0xfe])) * 2;
        let words = bytes[start..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&words)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Parses `\\wsl.localhost\<distro>` and `\\wsl$\<distro>` paths without
/// starting a distro. A recognized path remains WSL when user lookup fails so
/// callers fail closed at the command boundary instead of treating it as native.
pub fn environment_from_mounted_path(path: &Path) -> Option<DiscoveryEnvironment> {
    let raw = path.to_string_lossy().replace('/', "\\");
    for namespace in [WSL_NAMESPACE, WSL_LEGACY_NAMESPACE] {
        let prefix = format!(r"{namespace}\");
        if !raw
            .get(..prefix.len())
            .is_some_and(|value| value.eq_ignore_ascii_case(&prefix))
        {
            continue;
        }
        let distribution = raw[prefix.len()..].split('\\').next()?;
        if !valid_distribution_name(distribution) {
            return None;
        }
        let user = MOUNTED_USERS
            .get()
            .and_then(|users| {
                users
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get(&distribution.to_ascii_lowercase())
                    .cloned()
            })
            .or_else(|| default_user_from_mounted_distribution(distribution))
            .unwrap_or_default();
        return Some(DiscoveryEnvironment::Wsl {
            distribution: distribution.to_string(),
            user,
        });
    }
    None
}

fn default_user_from_mounted_distribution(distribution: &str) -> Option<String> {
    let root = mounted_distribution_root(distribution)?;
    let conf = std::fs::read_to_string(root.join("etc/wsl.conf")).unwrap_or_default();
    let passwd = std::fs::read_to_string(root.join("etc/passwd")).ok()?;
    resolve_default_user(parse_wsl_conf_default_user(&conf).as_deref(), &passwd)
        .map(|(user, _)| user)
}

fn valid_distribution_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
}

fn valid_user_name(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '$'))
}

fn parse_wsl_conf_default_user(contents: &str) -> Option<String> {
    let mut in_user = false;
    for raw in contents.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_user = line[1..line.len() - 1].trim().eq_ignore_ascii_case("user");
            continue;
        }
        if in_user
            && let Some((key, value)) = line.split_once('=')
            && key.trim().eq_ignore_ascii_case("default")
        {
            let value = value.trim().trim_matches(['\'', '"']);
            return valid_user_name(value).then(|| value.to_string());
        }
    }
    None
}

fn parse_passwd(contents: &str) -> Vec<(String, u32, String)> {
    contents
        .lines()
        .filter_map(|line| {
            let fields = line.split(':').collect::<Vec<_>>();
            if fields.len() < 7 || !valid_user_name(fields[0]) {
                return None;
            }
            let uid = fields[2].parse().ok()?;
            normalize_linux_absolute_path(fields[5]).ok()?;
            Some((fields[0].to_string(), uid, fields[5].to_string()))
        })
        .collect()
}

fn resolve_default_user(configured: Option<&str>, passwd: &str) -> Option<(String, String)> {
    let users = parse_passwd(passwd);
    if let Some(configured) = configured {
        return users
            .into_iter()
            .find(|(name, _, _)| name == configured)
            .map(|(name, _, home)| (name, home));
    }
    users
        .iter()
        .find(|(name, uid, home)| *uid == 1000 && name != "nobody" && home != "/")
        .cloned()
        .or_else(|| {
            users.into_iter().find(|(name, uid, home)| {
                *uid > 1000 && *uid < 65_534 && name != "nobody" && home != "/"
            })
        })
        .map(|(name, _, home)| (name, home))
}

fn normalize_linux_absolute_path(path: &str) -> Result<String> {
    if !path.starts_with('/') || path.contains('\0') || path.contains('\\') {
        bail!("invalid Linux absolute path");
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => bail!("Linux path traversal is not allowed"),
            value => parts.push(value),
        }
    }
    Ok(format!("/{}", parts.join("/")))
}

fn linux_path_under_root(root: &Path, path: &str) -> Result<PathBuf> {
    let normalized = normalize_linux_absolute_path(path)?;
    let mut output = root.to_path_buf();
    for part in normalized.trim_start_matches('/').split('/') {
        if !part.is_empty() {
            output.push(part);
        }
    }
    Ok(output)
}

/// Converts an absolute Linux path to the canonical `\\wsl.localhost` path for
/// `distribution`, rejecting traversal and malformed distribution names.
pub fn wsl_to_windows_path(distribution: &str, path: &str) -> Result<PathBuf> {
    if !valid_distribution_name(distribution) {
        bail!("invalid WSL distribution name");
    }
    let normalized = normalize_linux_absolute_path(path)?;
    let suffix = normalized.trim_start_matches('/').replace('/', "\\");
    let root = format!(r"{WSL_NAMESPACE}\{distribution}");
    Ok(PathBuf::from(if suffix.is_empty() {
        root
    } else {
        format!(r"{root}\{suffix}")
    }))
}

/// Converts a WSL UNC path, or a Windows drive path visible under `/mnt`, to an
/// absolute Linux path suitable for a command in `distribution`.
pub fn windows_to_wsl_path(distribution: &str, path: &Path) -> Result<String> {
    if !valid_distribution_name(distribution) {
        bail!("invalid WSL distribution name");
    }
    let raw = path.to_string_lossy().replace('/', "\\");
    for namespace in [WSL_NAMESPACE, WSL_LEGACY_NAMESPACE] {
        let root = format!(r"{namespace}\{distribution}");
        if raw.eq_ignore_ascii_case(&root) {
            return Ok("/".to_string());
        }
        let prefix = format!(r"{namespace}\{distribution}\");
        if raw
            .get(..prefix.len())
            .is_some_and(|value| value.eq_ignore_ascii_case(&prefix))
        {
            return normalize_linux_absolute_path(&format!(
                "/{}",
                raw[prefix.len()..].replace('\\', "/")
            ));
        }
    }
    let bytes = raw.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        return normalize_linux_absolute_path(&format!(
            "/mnt/{drive}/{}",
            raw[3..].replace('\\', "/")
        ));
    }
    bail!("path is not addressable from WSL: {}", path.display())
}

fn mounted_distribution_root(distribution: &str) -> Option<PathBuf> {
    mounted_distribution_root_in(
        distribution,
        Path::new(WSL_NAMESPACE),
        Path::new(WSL_LEGACY_NAMESPACE),
    )
}

fn mounted_distribution_root_in(
    distribution: &str,
    namespace: &Path,
    legacy_namespace: &Path,
) -> Option<PathBuf> {
    if !valid_distribution_name(distribution) {
        return None;
    }
    [namespace, legacy_namespace]
        .into_iter()
        .map(|namespace| namespace.join(distribution))
        .find(|path| std::fs::metadata(path).is_ok())
}

/// Builds a headless native or `wsl.exe` command without shell interpolation.
///
/// WSL commands are pinned to the distribution's resolved user and re-check
/// mounted accessibility immediately before execution. `repo`, when supplied,
/// is converted to the process namespace and passed as the program's `-C` path.
pub fn configure_command_for_environment(
    environment: &DiscoveryEnvironment,
    program: &str,
    repo: Option<&Path>,
) -> Result<tokio::process::Command> {
    match environment {
        DiscoveryEnvironment::Native => {
            let mut command = headless_tokio_command(program);
            if let Some(repo) = repo {
                command.arg("-C").arg(repo);
            }
            Ok(command)
        }
        DiscoveryEnvironment::Wsl { distribution, user } => {
            mounted_distribution_root(distribution)
                .with_context(|| format!("WSL distribution {distribution:?} is not mounted"))?;
            if !valid_user_name(user) {
                bail!("invalid WSL user name");
            }
            let mut command = headless_tokio_command("wsl.exe");
            command.args([
                "--distribution",
                distribution,
                "--user",
                user,
                "--",
                program,
            ]);
            if let Some(repo) = repo {
                command.args(["-C", &windows_to_wsl_path(distribution, repo)?]);
            }
            Ok(command)
        }
    }
}

#[cfg(test)]
fn path_has_traversal(path: &Path) -> bool {
    path.components()
        .any(|component| component == std::path::Component::ParentDir)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "root:x:0:0:root:/root:/bin/bash\ndev:x:1000:1000:Dev:/home/dev:/bin/bash\nnobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin\n";

    #[test]
    fn parses_wsl_conf_and_passwd_default_user() {
        assert_eq!(
            parse_wsl_conf_default_user("[boot]\nsystemd=true\n[user]\ndefault = dev\n"),
            Some("dev".into())
        );
        assert_eq!(
            resolve_default_user(Some("dev"), PASSWD),
            Some(("dev".into(), "/home/dev".into()))
        );
        assert_eq!(
            resolve_default_user(None, PASSWD),
            Some(("dev".into(), "/home/dev".into()))
        );
        assert_eq!(resolve_default_user(Some("missing"), PASSWD), None);

        let out_of_order =
            "later:x:1001:1001::/home/later:/bin/bash\ndev:x:1000:1000::/home/dev:/bin/bash\n";
        assert_eq!(
            resolve_default_user(None, out_of_order),
            Some(("dev".into(), "/home/dev".into()))
        );
    }

    #[test]
    fn rejects_traversal_and_invalid_names() {
        assert!(wsl_to_windows_path("../Ubuntu", "/home/dev").is_err());
        assert!(wsl_to_windows_path("Ubuntu", "/home/../etc").is_err());
        assert!(
            windows_to_wsl_path("Ubuntu", Path::new(r"\\wsl.localhost\Ubuntu\home\..\etc"))
                .is_err()
        );
        assert!(path_has_traversal(Path::new("home/../etc")));
    }

    #[test]
    fn converts_namespace_and_mounted_drive_paths() {
        assert_eq!(
            windows_to_wsl_path("Ubuntu", Path::new(r"\\wsl.localhost\Ubuntu\home\dev\repo"))
                .unwrap(),
            "/home/dev/repo"
        );
        assert_eq!(
            windows_to_wsl_path("Ubuntu", Path::new(r"C:\Users\dev\repo")).unwrap(),
            "/mnt/c/Users/dev/repo"
        );
        assert_eq!(
            wsl_to_windows_path("Ubuntu", "/home/dev").unwrap(),
            PathBuf::from(r"\\wsl.localhost\Ubuntu\home\dev")
        );
    }

    #[test]
    fn mounted_unc_path_retains_wsl_identity_when_user_is_unavailable() {
        let environment = DiscoveryEnvironment::from_mounted_path(Path::new(
            r"\\wsl.localhost\DefinitelyNotMounted\home\dev\repo",
        ));
        assert_eq!(environment.wsl_distro(), Some("DefinitelyNotMounted"));
        assert!(!environment.is_native());
    }

    #[test]
    fn host_path_converts_linux_candidates_per_distro() {
        let ubuntu = DiscoveryEnvironment::Wsl {
            distribution: "Ubuntu".into(),
            user: "dev".into(),
        };
        assert_eq!(
            ubuntu.host_path("/home/dev/repo").unwrap(),
            PathBuf::from(r"\\wsl.localhost\Ubuntu\home\dev\repo")
        );
        assert_eq!(
            DiscoveryEnvironment::Native
                .host_path(r"C:\dev\repo")
                .unwrap(),
            PathBuf::from(r"C:\dev\repo")
        );
    }

    #[test]
    fn native_is_backward_compatible_serde_default() {
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(default)]
            environment: DiscoveryEnvironment,
        }
        assert_eq!(
            serde_json::from_str::<Wrapper>("{}").unwrap().environment,
            DiscoveryEnvironment::Native
        );
    }

    #[test]
    fn decodes_utf16_running_distribution_output() {
        let bytes = "Ubuntu\r\nDebian\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(decode_wsl_output(&bytes), "Ubuntu\r\nDebian\r\n");
    }

    #[tokio::test]
    async fn discovers_only_namespace_entries_and_resolves_home_without_processes() {
        let namespace = tempfile::TempDir::new().unwrap();
        let legacy = tempfile::TempDir::new().unwrap();
        let distro = namespace.path().join("Ubuntu-24.04");
        tokio::fs::create_dir_all(distro.join("etc")).await.unwrap();
        tokio::fs::write(distro.join("etc/wsl.conf"), "[user]\ndefault=dev\n")
            .await
            .unwrap();
        tokio::fs::write(distro.join("etc/passwd"), PASSWD)
            .await
            .unwrap();
        tokio::fs::create_dir_all(distro.join("home/dev"))
            .await
            .unwrap();

        let found = discover_wsl_environments_in(namespace.path(), legacy.path()).await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].distribution, "Ubuntu-24.04");
        assert_eq!(found[0].user, "dev");
        assert_eq!(found[0].context.home, distro.join("home/dev"));
        assert_eq!(found[0].context.platform_home, PathBuf::from("/home/dev"));
    }

    #[test]
    fn mounted_check_does_not_accept_unenumerated_or_traversing_distro() {
        let namespace = tempfile::TempDir::new().unwrap();
        let legacy = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(namespace.path().join("Running")).unwrap();
        assert_eq!(
            mounted_distribution_root_in("Running", namespace.path(), legacy.path()),
            Some(namespace.path().join("Running"))
        );
        assert!(mounted_distribution_root_in("Stopped", namespace.path(), legacy.path()).is_none());
        assert!(
            mounted_distribution_root_in("../Running", namespace.path(), legacy.path()).is_none()
        );
    }
}
