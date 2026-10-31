use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::provider_usage::live::sources::cli_locator::{is_executable, version_bin_dirs};

use super::Vendor;
use super::process::{PROBE_TIMEOUT, Runner};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub vendor: Vendor,
    pub root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Runtime {
    pub executable: PathBuf,
    pub version: String,
}

fn absolute(value: &str) -> Option<PathBuf> {
    let path = PathBuf::from(value);
    (path.is_absolute()
        && value.len() <= 8192
        && !value.chars().any(char::is_control)
        && !value.starts_with("//")
        && !value.starts_with(r"\\"))
    .then_some(path)
}

pub fn profile(
    vendor: Vendor,
    home: &Path,
    env: &BTreeMap<String, String>,
    saved: Option<&Profile>,
) -> Result<Profile, &'static str> {
    if let Some(saved) = saved {
        if saved.vendor != vendor || !saved.root.is_absolute() {
            return Err("invalid_saved_profile");
        }
        return Ok(saved.clone());
    }
    let override_keys: &[&str] = match vendor {
        Vendor::Claude => &["CLAUDE_CONFIG_DIR"],
        Vendor::Codex => &["CODEX_HOME"],
        Vendor::Cursor => &[],
        Vendor::OpenCode => &["OPENCODE_CONFIG_DIR"],
        Vendor::Pi => &["PI_CODING_AGENT_DIR", "PI_AGENT_DIR"],
    };
    for key in override_keys {
        if let Some(value) = env.get(*key) {
            return Ok(Profile {
                vendor,
                root: absolute(value).ok_or("invalid_profile_override")?,
            });
        }
    }
    let root = match vendor {
        Vendor::Claude => home.join(".claude"),
        Vendor::Codex => home.join(".codex"),
        Vendor::Cursor => home.join(".cursor"),
        Vendor::Pi => home.join(".pi/agent"),
        Vendor::OpenCode => match env.get("XDG_CONFIG_HOME") {
            Some(value) => absolute(value)
                .ok_or("invalid_profile_override")?
                .join("opencode"),
            None => home.join(".config/opencode"),
        },
    };
    if !root.is_absolute() {
        return Err("invalid_home");
    }
    Ok(Profile { vendor, root })
}

pub fn search_directories(home: &Path, env: &BTreeMap<String, String>) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(path) = env.get("PATH").filter(|value| value.len() <= 32768) {
        directories.extend(
            std::env::split_paths(path)
                .filter(|p| p.is_absolute())
                .take(64),
        );
    }
    directories.extend([
        home.join(".local/bin"),
        home.join(".volta/bin"),
        home.join(".local/share/mise/shims"),
        home.join(".local/share/mise/installs/node/lts/bin"),
        home.join(".local/share/fnm/aliases/default/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
    ]);
    for root in [
        home.join(".nvm/versions/node"),
        home.join(".local/share/mise/installs/node"),
        home.join(".asdf/installs/nodejs"),
    ] {
        directories.extend(version_bin_dirs(&root, 8));
    }
    let mut seen = BTreeSet::new();
    directories.retain(|p| seen.insert(p.clone()));
    directories
}

pub fn find_node(directories: &[PathBuf], runner: &impl Runner) -> Option<Runtime> {
    let started = Instant::now();
    for directory in directories.iter().take(96) {
        if started.elapsed() >= Duration::from_secs(4) {
            break;
        }
        let candidate = directory.join(if cfg!(windows) { "node.exe" } else { "node" });
        if !candidate.is_absolute() || !is_executable(&candidate) {
            continue;
        }
        let Ok(path) = candidate.canonicalize() else {
            continue;
        };
        // Resolve manager shims to the native Node binary before registration.
        let Ok(output) = runner.run(
            &path,
            &["--eval".into(), "process.stdout.write(JSON.stringify({version:process.version,executable:process.execPath}))".into()],
            &BTreeMap::new(),
            PROBE_TIMEOUT.min(Duration::from_secs(4).saturating_sub(started.elapsed())),
        ) else {
            continue;
        };
        #[derive(Deserialize)]
        struct Probe {
            version: String,
            executable: PathBuf,
        }
        let Ok(probe) = serde_json::from_str::<Probe>(&output) else {
            continue;
        };
        let Ok(path) = probe.executable.canonicalize() else {
            continue;
        };
        if compatible_node(&probe.version) && path.is_absolute() && is_executable(&path) {
            return Some(Runtime {
                executable: path,
                version: probe.version,
            });
        }
    }
    None
}

pub fn compatible_node(version: &str) -> bool {
    let Some(version) = version.strip_prefix('v') else {
        return false;
    };
    let parts = version.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty() && part.len() <= 4 && part.bytes().all(|b| b.is_ascii_digit())
        })
        && parts[0].parse::<u32>().is_ok_and(|major| major >= 22)
}

pub fn find_host(vendor: Vendor, directories: &[PathBuf]) -> Option<PathBuf> {
    let name = match vendor {
        Vendor::Claude => "claude",
        Vendor::Codex => "codex",
        Vendor::Cursor => "cursor-agent",
        Vendor::OpenCode => "opencode",
        Vendor::Pi => "pi",
    };
    directories
        .iter()
        .take(80)
        .map(|directory| directory.join(name))
        .find(|path| path.is_absolute() && is_executable(path))
    // Keep the vendor launcher so its own runtime setup remains intact.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch_executable(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"fixture").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn version_managers_select_newest_versions_after_filtering_all_entries() {
        let home = tempfile::tempdir().unwrap();
        for (manager, prefix) in [
            (".nvm/versions/node", "v"),
            (".local/share/mise/installs/node", ""),
            (".asdf/installs/nodejs", ""),
        ] {
            let root = home.path().join(manager);
            for index in 0..24 {
                std::fs::create_dir_all(root.join(format!("alias-{index}"))).unwrap();
                std::fs::create_dir_all(root.join(format!("{prefix}9.0.{index}/bin"))).unwrap();
            }
            for version in ["22.0.0", "22.1.0"] {
                std::fs::create_dir_all(root.join(format!("{prefix}{version}/bin"))).unwrap();
            }
        }

        let directories = search_directories(home.path(), &BTreeMap::new());
        assert_eq!(
            directories,
            search_directories(home.path(), &BTreeMap::new())
        );
        for (manager, prefix) in [
            (".nvm/versions/node", "v"),
            (".local/share/mise/installs/node", ""),
            (".asdf/installs/nodejs", ""),
        ] {
            let root = home.path().join(manager);
            let versions = directories
                .iter()
                .filter(|path| path.starts_with(&root) && !path.ends_with("lts/bin"))
                .collect::<Vec<_>>();
            assert_eq!(versions.len(), 8);
            assert_eq!(versions[0], &root.join(format!("{prefix}22.1.0/bin")));
            assert_eq!(versions[1], &root.join(format!("{prefix}22.0.0/bin")));
            assert_eq!(versions[2], &root.join(format!("{prefix}9.0.23/bin")));
        }
    }

    #[test]
    fn a_compatible_node_is_found_after_more_than_16_older_installs() {
        struct ProbeRunner;
        impl Runner for ProbeRunner {
            fn run(
                &self,
                executable: &Path,
                _: &[String],
                _: &BTreeMap<String, String>,
                _: Duration,
            ) -> Result<String, super::super::process::ProcessError> {
                let version = executable
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap();
                Ok(serde_json::json!({"version": version, "executable": executable}).to_string())
            }
        }

        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".nvm/versions/node");
        let binary = if cfg!(windows) { "node.exe" } else { "node" };
        for index in 0..24 {
            touch_executable(&root.join(format!("v9.0.{index}/bin/{binary}")));
        }
        let compatible = root.join("v22.0.0/bin").join(binary);
        touch_executable(&compatible);
        let directories = search_directories(home.path(), &BTreeMap::new())
            .into_iter()
            .filter(|path| path.starts_with(&root))
            .collect::<Vec<_>>();

        let runtime = find_node(&directories, &ProbeRunner).unwrap();
        assert_eq!(runtime.version, "v22.0.0");
        assert_eq!(runtime.executable, compatible.canonicalize().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn host_discovery_uses_the_shared_executable_rules_and_preserves_symlinks() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let non_executable = root.path().join("stray");
        std::fs::create_dir(&non_executable).unwrap();
        std::fs::write(non_executable.join("claude"), b"not executable").unwrap();
        let directory = root.path().join("directory");
        std::fs::create_dir_all(directory.join("claude")).unwrap();
        let installed = root.path().join("installed");
        touch_executable(&installed.join("native"));
        symlink(installed.join("native"), installed.join("claude")).unwrap();

        assert!(!is_executable(&non_executable.join("claude")));
        assert!(!is_executable(&directory.join("claude")));
        assert!(is_executable(&installed.join("claude")));
        assert_eq!(
            find_host(
                Vendor::Claude,
                &[non_executable, directory, installed.clone()]
            ),
            Some(installed.join("claude"))
        );
    }

    #[test]
    fn node_requires_a_stable_numeric_release_at_least_22() {
        for good in ["v22.0.0", "v25.9.0", "v24.13.1"] {
            assert!(compatible_node(good));
        }
        for bad in [
            "v21.9.0",
            "22.0.0",
            "v22.0",
            "v22.0.0-nightly",
            "v22.0.0\nextra",
            "v999999.1.1",
        ] {
            assert!(!compatible_node(bad), "{bad}");
        }
    }

    #[test]
    fn saved_profile_wins_over_changed_gui_environment() {
        let home = tempfile::tempdir().unwrap();
        let other = home.path().join("profiles/other");
        let saved = Profile {
            vendor: Vendor::Codex,
            root: home.path().join("profiles/selected"),
        };
        let env = BTreeMap::from([("CODEX_HOME".into(), other.to_str().unwrap().into())]);
        assert_eq!(
            profile(Vendor::Codex, home.path(), &env, Some(&saved)).unwrap(),
            saved
        );
        assert_eq!(
            profile(Vendor::Codex, home.path(), &env, None)
                .unwrap()
                .root,
            other
        );
    }

    #[test]
    fn invalid_overrides_never_fall_back_to_a_different_profile() {
        let env = BTreeMap::from([("PI_CODING_AGENT_DIR".into(), "relative".into())]);
        assert!(profile(Vendor::Pi, Path::new("/home/test"), &env, None).is_err());
    }
}
