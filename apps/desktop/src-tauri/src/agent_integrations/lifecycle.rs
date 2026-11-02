use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::provider_usage::live::sources::cli_locator::is_executable;

use super::artifact::{Artifact, vendor_directory};
use super::discovery::{self, Profile, Runtime};
use super::document;
use super::process::{PROBE_TIMEOUT, Runner};
use super::transaction::{self, Receipt, Result};
use super::{IntegrationPhase, IntegrationStatus, Vendor};

#[derive(Debug, Clone)]
pub struct CliConnection {
    pub executable: PathBuf,
    pub version: String,
    pub sha256: String,
    pub delivery_protocols: Vec<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Route {
    pub vendor: Vendor,
    pub version: String,
    pub os: String,
    pub arch: String,
    pub surface: String,
    pub route: String,
    pub native_validated: bool,
    pub reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Compatibility {
    schema: u32,
    routes: Vec<Route>,
}

impl Compatibility {
    fn read() -> Option<Self> {
        let manifest: Self = serde_json::from_str(include_str!("compatibility.json")).ok()?;
        (manifest.schema == 1).then_some(manifest)
    }

    fn accepts(route: &Route, vendor: Vendor) -> bool {
        let expected = match vendor {
            Vendor::Claude => "user_settings_hooks",
            Vendor::Codex | Vendor::Cursor => "user_hooks",
            Vendor::OpenCode => "user_plugin_reference",
            Vendor::Pi => "user_package_manager",
        };
        route.vendor == vendor
            && route.os == std::env::consts::OS
            && route.arch == std::env::consts::ARCH
            && route.surface == "cli"
            && route.route == expected
            && route.native_validated
    }

    fn has_accepted_route(&self, vendor: Vendor) -> bool {
        self.routes.iter().any(|route| Self::accepts(route, vendor))
    }

    fn accepted_route(&self, vendor: Vendor, version: &str) -> Option<&Route> {
        self.routes
            .iter()
            .find(|route| route.version == version && Self::accepts(route, vendor))
    }
}

pub fn accepted_route(vendor: Vendor, version: &str) -> Option<Route> {
    Compatibility::read()?
        .accepted_route(vendor, version)
        .cloned()
}

const HOST_PROBE_BUDGET: Duration = Duration::from_secs(4);

struct RuntimeDiscovery<'a> {
    directories: Vec<PathBuf>,
    compatibility: &'a Compatibility,
    node: Option<Runtime>,
    node_searched: bool,
    probe_budget: Duration,
}

impl RuntimeDiscovery<'_> {
    fn probe_timeout(&self, vendor: Vendor) -> Duration {
        let remaining = Vendor::ALL
            .into_iter()
            .skip_while(|candidate| *candidate != vendor)
            .filter(|candidate| self.compatibility.has_accepted_route(*candidate))
            .fold(0u32, |count, _| count + 1);
        (self.probe_budget / remaining).min(PROBE_TIMEOUT)
    }
}

#[derive(Deserialize)]
struct VoltaBinary {
    name: String,
    package: String,
    platform: VoltaPlatform,
    manager: String,
}

#[derive(Deserialize)]
struct VoltaPlatform {
    node: String,
}

struct ProbeDirectory(PathBuf);

impl ProbeDirectory {
    fn create() -> Result<Self> {
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(|_| "host_probe_unavailable")?;
        let path = std::env::temp_dir()
            .canonicalize()
            .map_err(|_| "host_probe_unavailable")?
            .join(format!(
                "antiburn-host-probe-{}",
                transaction::digest(&random)
            ));
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let mut builder = builder;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .map_err(|_| "host_probe_unavailable")?;
        Ok(Self(path))
    }
}

impl Drop for ProbeDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub struct Installation<'a, R> {
    pub state: &'a Path,
    pub resources: &'a Path,
    pub home: &'a Path,
    pub env: &'a BTreeMap<String, String>,
    pub runner: &'a R,
}

fn profile_environment_key(vendor: Vendor) -> &'static str {
    match vendor {
        Vendor::Claude => "CLAUDE_CONFIG_DIR",
        Vendor::Codex => "CODEX_HOME",
        Vendor::Cursor => "CURSOR_CONFIG_DIR",
        Vendor::OpenCode => "OPENCODE_CONFIG_DIR",
        Vendor::Pi => "PI_CODING_AGENT_DIR",
    }
}

fn posix(value: &Path) -> Result<String> {
    let value = value.to_str().ok_or("unsupported_path_encoding")?;
    if !value.starts_with('/') || value.chars().any(char::is_control) || value.len() > 8192 {
        return Err("unsafe_path");
    }
    Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
}

pub fn hook_command(node: &Path, hook: &Path, cli: &Path) -> Result<String> {
    let node = posix(node)?;
    let cli = posix(cli)?;
    Ok(format!(
        "if [ -x {node} ] && [ -x {cli} ]; then ANTIBURN_EXECUTABLE={cli} {node} {}; fi",
        posix(hook)?
    ))
}

fn host_version(vendor: Vendor, output: &str) -> Option<String> {
    let text = output.trim();
    let version = match vendor {
        Vendor::Claude => text.strip_suffix(" (Claude Code)"),
        Vendor::Codex => text.strip_prefix("codex-cli "),
        _ => Some(text),
    }?;
    if version.is_empty()
        || version.len() > 96
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
    {
        return None;
    }
    Some(version.into())
}

impl<R: Runner> Installation<'_, R> {
    pub fn reconcile(&self, cli: Option<&CliConnection>) -> Vec<IntegrationStatus> {
        self.statuses(cli, true)
    }

    pub fn inspect(&self, cli: Option<&CliConnection>) -> Vec<IntegrationStatus> {
        self.statuses(cli, false)
    }

    fn statuses(&self, cli: Option<&CliConnection>, setup: bool) -> Vec<IntegrationStatus> {
        let Some(compatibility) = Compatibility::read() else {
            return Vendor::ALL
                .into_iter()
                .map(|vendor| {
                    status(
                        vendor,
                        IntegrationPhase::Unavailable,
                        "compatibility_manifest_invalid",
                    )
                })
                .collect();
        };
        self.statuses_with_compatibility(cli, setup, &compatibility)
    }

    fn statuses_with_compatibility(
        &self,
        cli: Option<&CliConnection>,
        setup: bool,
        compatibility: &Compatibility,
    ) -> Vec<IntegrationStatus> {
        if cli.is_none() {
            return Vendor::ALL
                .into_iter()
                .map(|vendor| {
                    let mut result = status(
                        vendor,
                        IntegrationPhase::Unavailable,
                        "managed_cli_unavailable",
                    );
                    result.owned = transaction::load_receipt(&self.receipt_path(vendor))
                        .ok()
                        .flatten()
                        .is_some_and(|receipt| !receipt.removed);
                    result
                })
                .collect();
        }
        let _lock = match setup
            .then(|| transaction::Lock::acquire(self.state))
            .transpose()
        {
            Ok(lock) => lock,
            Err(reason) => {
                return Vendor::ALL
                    .into_iter()
                    .map(|vendor| status(vendor, IntegrationPhase::Unavailable, reason))
                    .collect();
            }
        };
        let mut discovery = RuntimeDiscovery {
            directories: discovery::search_directories(self.home, self.env),
            compatibility,
            node: None,
            node_searched: false,
            probe_budget: HOST_PROBE_BUDGET,
        };
        Vendor::ALL
            .into_iter()
            .map(|vendor| {
                let result = self.reconcile_vendor(vendor, cli, &mut discovery, setup);
                let mut result = result
                    .unwrap_or_else(|reason| status(vendor, IntegrationPhase::Unavailable, reason));
                result.owned = transaction::load_receipt(&self.receipt_path(vendor))
                    .ok()
                    .flatten()
                    .is_some_and(|receipt| !receipt.removed);
                result
            })
            .collect()
    }

    fn reconcile_vendor(
        &self,
        vendor: Vendor,
        cli: Option<&CliConnection>,
        discovery: &mut RuntimeDiscovery<'_>,
        setup: bool,
    ) -> Result<IntegrationStatus> {
        let saved_path = self
            .state
            .join(format!("{}-profile.json", vendor_directory(vendor)));
        let saved = transaction::read(&saved_path)?
            .map(|bytes| {
                serde_json::from_slice::<Profile>(&bytes).map_err(|_| "invalid_saved_profile")
            })
            .transpose()?;
        let profile = discovery::profile(vendor, self.home, self.env, saved.as_ref())?;
        if !profile.root.exists() {
            return Ok(status(
                vendor,
                IntegrationPhase::NotDetected,
                "profile_not_detected",
            ));
        }
        transaction::checked_directory(&profile.root)?;
        let Some(host) = discovery::find_host(vendor, &discovery.directories) else {
            return Ok(with_profile(
                status(
                    vendor,
                    IntegrationPhase::Unavailable,
                    "host_runtime_missing",
                ),
                &profile,
            ));
        };
        let mut result = with_profile(
            status(
                vendor,
                IntegrationPhase::PendingNativeGate,
                "native_route_not_validated",
            ),
            &profile,
        );
        if !discovery.compatibility.has_accepted_route(vendor) {
            return Ok(result);
        }
        let started = Instant::now();
        let version = self.probe_host_version(vendor, &host, discovery.probe_timeout(vendor));
        discovery.probe_budget = discovery.probe_budget.saturating_sub(started.elapsed());
        let version = version?;
        result.host_version = Some(version.clone());
        if discovery
            .compatibility
            .accepted_route(vendor, &version)
            .is_none()
        {
            return Ok(result);
        }
        let Some(cli) = cli else {
            result.phase = IntegrationPhase::Unavailable;
            result.reason = "managed_cli_unavailable".into();
            return Ok(result);
        };
        if !cli.executable.is_absolute()
            || !cli.delivery_protocols.iter().any(|p| p == "delivery-v1")
        {
            return Err("managed_cli_incompatible");
        }
        if !setup {
            result.phase = self.inspect_profile(&profile)?;
            result.reason = match result.phase {
                IntegrationPhase::Disabled => "disabled_in_vendor_or_antiburn",
                IntegrationPhase::UserChanged => "owned_entry_changed_or_removed",
                IntegrationPhase::Unavailable => "setup_not_registered",
                _ => "registered_delivery_unverified",
            }
            .into();
            return Ok(result);
        }
        if vendor.needs_node() && !discovery.node_searched {
            discovery.node = discovery::find_node(&discovery.directories, self.runner);
            discovery.node_searched = true;
        }
        let node = discovery.node.as_ref();
        if vendor.needs_node() && node.is_none() {
            result.phase = IntegrationPhase::Unavailable;
            result.reason = "compatible_node_22_required".into();
            return Ok(result);
        }
        result.node_version = node.map(|node| node.version.clone());
        if saved.is_none() {
            let bytes = serde_json::to_vec(&profile).map_err(|_| "invalid_saved_profile")?;
            transaction::publish(&saved_path, None, &bytes)?;
        }
        let artifact = Artifact::read(self.resources)?;
        let artifacts = self.state.join("artifacts");
        transaction::private_directory(&artifacts)?;
        let root = artifact.stage(&artifacts, &json!({"schema":1,"executable":cli.executable,"version":cli.version,"sha256":cli.sha256,"deliveryProtocols":cli.delivery_protocols}))?;
        let binding = format!(
            "{}:{}",
            root.display(),
            node.map(|node| node.executable.to_string_lossy())
                .unwrap_or_default()
        );
        let phase = if vendor == Vendor::Pi {
            self.reconcile_pi(&profile, &host, &root, &artifact.hash)?
        } else {
            self.reconcile_config(&profile, node, &root, &binding, &cli.executable)?
        };
        result.phase = phase;
        result.reason = match result.phase {
            IntegrationPhase::AwaitingTrust => "registered_review_native_hook_trust",
            IntegrationPhase::ReloadRequired => "registered_restart_or_reload_host",
            IntegrationPhase::UserChanged => "owned_entry_changed_or_removed",
            IntegrationPhase::Disabled => "disabled_in_vendor_or_antiburn",
            _ => "registered",
        }
        .into();
        Ok(result)
    }

    fn probe_host_version(&self, vendor: Vendor, host: &Path, timeout: Duration) -> Result<String> {
        if timeout.is_zero() {
            return Err("host_version_unavailable");
        }
        let scratch = ProbeDirectory::create()?;
        let mut env = BTreeMap::new();
        if let Some(path) = self.env.get("PATH") {
            env.insert("PATH".into(), path.clone());
        }
        let installed = self.installed_volta_command(host)?;
        let host = if let Some((executable, node_bin)) = installed.as_ref() {
            let mut paths = vec![node_bin.clone()];
            if let Some(path) = env.get("PATH") {
                paths.extend(std::env::split_paths(path));
            }
            let path = std::env::join_paths(paths).map_err(|_| "host_launcher_requires_review")?;
            env.insert(
                "PATH".into(),
                path.into_string()
                    .map_err(|_| "unsupported_path_encoding")?,
            );
            executable.as_path()
        } else {
            host
        };
        for name in [
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
            "TMPDIR",
        ] {
            env.insert(name.into(), scratch.0.to_string_lossy().into_owned());
        }
        env.insert(
            profile_environment_key(vendor).into(),
            scratch.0.to_string_lossy().into_owned(),
        );
        env.insert("PI_OFFLINE".into(), "1".into());
        env.insert("PI_TELEMETRY".into(), "0".into());
        env.insert("NO_COLOR".into(), "1".into());
        let output = self
            .runner
            .run(host, &["--version".into()], &env, timeout)
            .map_err(|_| "host_version_unavailable")?;
        host_version(vendor, &output).ok_or("host_version_unavailable")
    }

    fn installed_volta_command(&self, host: &Path) -> Result<Option<(PathBuf, PathBuf)>> {
        let default_root = self.home.join(".volta");
        let root = self
            .env
            .get("VOLTA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| default_root.clone());
        if host.parent() != Some(root.join("bin").as_path())
            && host.parent() != Some(default_root.join("bin").as_path())
        {
            return Ok(None);
        }
        let resolve = || {
            // Read installed paths without running Volta hooks or runtime downloads.
            transaction::read(&root.join("layout.v4"))?.ok_or("host_launcher_requires_review")?;
            let name = host
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("unsupported_path_encoding")?;
            let bytes =
                transaction::read(&root.join("tools/user/bins").join(format!("{name}.json")))?
                    .ok_or("host_launcher_requires_review")?;
            let config: VoltaBinary =
                serde_json::from_slice(&bytes).map_err(|_| "host_launcher_requires_review")?;
            if config.name != name
                || !matches!(config.manager.as_str(), "Npm" | "Pnpm" | "Yarn")
                || config.package.is_empty()
                || config.package.len() > 256
                || config.package.contains('\\')
                || config.package.chars().any(char::is_control)
                || config
                    .package
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
                || Path::new(&config.package)
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
                || config.platform.node.split('.').count() != 3
                || config
                    .platform
                    .node
                    .split('.')
                    .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
            {
                return Err("host_launcher_requires_review");
            }
            let images = root
                .join("tools/image")
                .canonicalize()
                .map_err(|_| "host_launcher_requires_review")?;
            let executable = images
                .join("packages")
                .join(config.package)
                .join("bin")
                .join(name)
                .canonicalize()
                .map_err(|_| "host_launcher_requires_review")?;
            let node = images
                .join("node")
                .join(config.platform.node)
                .join("bin/node")
                .canonicalize()
                .map_err(|_| "host_launcher_requires_review")?;
            if !executable.starts_with(&images)
                || !node.starts_with(&images)
                || !is_executable(&executable)
                || !is_executable(&node)
            {
                return Err("host_launcher_requires_review");
            }
            Ok(Some((
                executable,
                node.parent()
                    .ok_or("host_launcher_requires_review")?
                    .to_path_buf(),
            )))
        };
        resolve().map_err(|_| "host_launcher_requires_review")
    }

    pub(super) fn inspect_profile(&self, profile: &Profile) -> Result<IntegrationPhase> {
        if profile.vendor == Vendor::Pi {
            return super::manager::inspect(profile, &self.receipt_path(profile.vendor));
        }
        let target = self.config_path(profile)?;
        let current = transaction::read(&target)?;
        if let Some(bytes) = current.as_deref()
            && profile.vendor == Vendor::Claude
            && claude_hooks_disabled(bytes)?
        {
            return Ok(IntegrationPhase::Disabled);
        }
        let Some(receipt) = transaction::load_receipt(&self.receipt_path(profile.vendor))? else {
            return Ok(IntegrationPhase::Unavailable);
        };
        if receipt.target != target || receipt.manager_source.is_some() {
            return Err("receipt_target_mismatch");
        }
        if receipt.removed {
            return Ok(IntegrationPhase::Disabled);
        }
        if !receipt.published || receipt.previous.is_some() {
            return Err("interrupted_installation_requires_repair");
        }
        if current.as_deref() == receipt.original.as_deref()
            || current
                .as_deref()
                .is_none_or(|bytes| receipt.rollback(bytes).is_err())
        {
            return Ok(IntegrationPhase::UserChanged);
        }
        Ok(registered_phase(profile.vendor))
    }

    fn host_environment(&self, profile: &Profile) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        for name in [
            "PATH",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
        ] {
            if let Some(value) = self.env.get(name) {
                env.insert(name.into(), value.clone());
            }
        }
        env.insert("HOME".into(), self.home.to_string_lossy().into_owned());
        env.insert("PI_OFFLINE".into(), "1".into());
        env.insert("PI_TELEMETRY".into(), "0".into());
        env.insert("NO_COLOR".into(), "1".into());
        env.insert(
            profile_environment_key(profile.vendor).into(),
            profile.root.to_string_lossy().into_owned(),
        );
        env
    }

    fn receipt_path(&self, vendor: Vendor) -> PathBuf {
        self.state
            .join(format!("{}-receipt.json", vendor_directory(vendor)))
    }

    pub fn remove(&self, vendor: Vendor) -> Result<()> {
        let _lock = transaction::Lock::acquire(self.state)?;
        let saved = transaction::read(
            &self
                .state
                .join(format!("{}-profile.json", vendor_directory(vendor))),
        )?
        .ok_or("profile_not_detected")?;
        let profile: Profile =
            serde_json::from_slice(&saved).map_err(|_| "invalid_saved_profile")?;
        if profile.vendor != vendor {
            return Err("invalid_saved_profile");
        }
        let path = self.receipt_path(vendor);
        let Some(mut receipt) = transaction::load_receipt(&path)? else {
            return Ok(());
        };
        if receipt.removed {
            return Ok(());
        }
        if vendor == Vendor::Pi {
            let directories = discovery::search_directories(self.home, self.env);
            let host = discovery::find_host(vendor, &directories).ok_or("host_runtime_missing")?;
            return super::manager::remove(
                &profile,
                &host,
                &mut receipt,
                &path,
                &self.host_environment(&profile),
                self.runner,
            );
        }
        if receipt.target != self.config_path(&profile)? || receipt.manager_source.is_some() {
            return Err("receipt_target_mismatch");
        }
        let current = transaction::read(&receipt.target)?;
        receipt = recover_replacement(&path, &receipt.target.clone(), receipt, current.as_deref())?;
        if current != receipt.original
            && let Some(current) = current
        {
            let next = receipt.rollback(&current)?;
            transaction::publish(&receipt.target, Some(&current), &next)?;
        }
        receipt.removed = true;
        transaction::save_receipt(&path, &receipt)
    }

    pub fn repair(&self, vendor: Vendor) -> Result<()> {
        let _lock = transaction::Lock::acquire(self.state)?;
        let path = self.receipt_path(vendor);
        let Some(receipt) = transaction::load_receipt(&path)? else {
            return Ok(());
        };
        let saved = transaction::read(
            &self
                .state
                .join(format!("{}-profile.json", vendor_directory(vendor))),
        )?
        .ok_or("profile_not_detected")?;
        let profile: Profile =
            serde_json::from_slice(&saved).map_err(|_| "invalid_saved_profile")?;
        if !Compatibility::read().is_some_and(|manifest| manifest.has_accepted_route(vendor)) {
            return Err("native_route_not_validated");
        }
        let directories = discovery::search_directories(self.home, self.env);
        let host = discovery::find_host(vendor, &directories).ok_or("host_runtime_missing")?;
        let version = self.probe_host_version(vendor, &host, PROBE_TIMEOUT)?;
        if accepted_route(vendor, &version).is_none() {
            return Err("native_route_not_validated");
        }
        let expected = if vendor == Vendor::Pi {
            profile.root.join("settings.json")
        } else {
            self.config_path(&profile)?
        };
        if profile.vendor != vendor || receipt.target != expected {
            return Err("receipt_target_mismatch");
        }
        let current = transaction::read(&receipt.target)?;
        let receipt = if vendor == Vendor::Pi {
            receipt
        } else {
            recover_replacement(&path, &expected, receipt, current.as_deref())?
        };
        let unpublished_unchanged = !receipt.published && current == receipt.original;
        if vendor == Vendor::Pi && !receipt.removed && !unpublished_unchanged {
            if receipt.published && current.as_deref() == Some(&receipt.installed) {
                return Ok(());
            }
            return Err("native_package_repair_requires_review");
        }
        if !receipt.removed && !unpublished_unchanged {
            if current
                .as_deref()
                .is_some_and(|bytes| receipt.rollback(bytes).is_ok())
            {
                return Ok(());
            }
            self.refuse_duplicate(&profile, current.as_deref().unwrap_or(b"{}"))?;
        }
        transaction::save_receipt(&path.with_extension("previous.json"), &receipt)?;
        std::fs::remove_file(path).map_err(|_| "state_unavailable")?;
        Ok(())
    }

    fn config_path(&self, profile: &Profile) -> Result<PathBuf> {
        Ok(profile.root.join(match profile.vendor {
            Vendor::Claude => "settings.json",
            Vendor::Codex | Vendor::Cursor => "hooks.json",
            Vendor::OpenCode => {
                if self.env.contains_key("OPENCODE_CONFIG")
                    || self.env.contains_key("OPENCODE_CONFIG_CONTENT")
                {
                    return Err("custom_opencode_config_requires_review");
                }
                let json = profile.root.join("opencode.json").exists();
                let jsonc = profile.root.join("opencode.jsonc").exists();
                if json && jsonc {
                    return Err("ambiguous_opencode_config");
                }
                if jsonc {
                    "opencode.jsonc"
                } else {
                    "opencode.json"
                }
            }
            Vendor::Pi => return Err("native_package_manager_required"),
        }))
    }

    pub(super) fn reconcile_config(
        &self,
        profile: &Profile,
        node: Option<&Runtime>,
        root: &Path,
        artifact: &str,
        cli: &Path,
    ) -> Result<IntegrationPhase> {
        let target = self.config_path(profile)?;
        let receipt_path = self.receipt_path(profile.vendor);
        let current = transaction::read(&target)?;
        let mut prior = transaction::load_receipt(&receipt_path)?
            .map(|receipt| recover_replacement(&receipt_path, &target, receipt, current.as_deref()))
            .transpose()?;
        if let Some(bytes) = current.as_deref()
            && profile.vendor == Vendor::Claude
            && claude_hooks_disabled(bytes)?
        {
            return Ok(IntegrationPhase::Disabled);
        }
        if let Some(prior) = prior.as_mut()
            && !prior.published
            && current.as_deref() == Some(&prior.installed)
        {
            prior.published = true;
            transaction::save_receipt(&receipt_path, prior)?;
        }
        let mut baseline = current.clone().unwrap_or_else(|| b"{}".to_vec());
        if let Some(prior) = &prior {
            if prior.target != target {
                return Err("receipt_target_mismatch");
            }
            if prior.removed {
                return Ok(IntegrationPhase::Disabled);
            }
            if current.as_deref() == prior.original.as_deref() {
                // An interrupted write and a vendor-side removal can have the same bytes.
                if !prior.published {
                    return Err("interrupted_installation_requires_repair");
                }
                return Ok(IntegrationPhase::UserChanged);
            }
            let Some(current) = current.as_deref() else {
                return Ok(IntegrationPhase::UserChanged);
            };
            baseline = match prior.rollback(current) {
                Ok(bytes) => bytes,
                Err(_) => return Ok(IntegrationPhase::UserChanged),
            };
            if prior.artifact == artifact {
                return Ok(registered_phase(profile.vendor));
            }
        }
        let bytes = baseline.as_slice();
        document::validate(bytes)?;
        self.refuse_duplicate(profile, bytes)?;
        let (installed, deltas) = render_config(profile.vendor, bytes, node, root, cli)?;
        if current.as_deref() == Some(&installed)
            && let Some(mut active) = prior.take()
        {
            active.artifact = artifact.into();
            transaction::save_receipt(&receipt_path, &active)?;
            return Ok(registered_phase(profile.vendor));
        }
        let mut receipt = Receipt::prepare(
            target.clone(),
            current.clone(),
            installed,
            deltas,
            artifact.into(),
        )?;
        receipt.previous = prior.map(Box::new);
        transaction::save_receipt(&receipt_path, &receipt)?;
        transaction::publish(&target, current.as_deref(), &receipt.installed)?;
        receipt.published = true;
        receipt.previous = None;
        transaction::save_receipt(&receipt_path, &receipt)?;
        Ok(registered_phase(profile.vendor))
    }

    fn refuse_duplicate(&self, profile: &Profile, bytes: &[u8]) -> Result<()> {
        let text = std::str::from_utf8(bytes).map_err(|_| "malformed_jsonc")?;
        if text.contains("antiburn-session-checks")
            || text.contains("session-check-hook.mjs")
            || text.contains("antiburn-cli")
            || text.contains("antiburn checks")
            || (profile.vendor == Vendor::OpenCode && text.contains("/antiburn.mjs"))
        {
            return Err("existing_integration_requires_review");
        }
        if profile.vendor == Vendor::Claude {
            if claude_hooks_disabled(bytes)? {
                return Err("hook_policy_requires_review");
            }
            let plugins = profile.root.join("plugins");
            if plugins.exists()
                && let Some(bytes) = transaction::read(&plugins.join("installed_plugins.json"))?
                && String::from_utf8_lossy(&bytes).contains("antiburn-session-checks")
            {
                return Err("existing_companion_requires_review");
            }
        }
        Ok(())
    }

    fn reconcile_pi(
        &self,
        profile: &Profile,
        host: &Path,
        root: &Path,
        artifact: &str,
    ) -> Result<IntegrationPhase> {
        super::manager::reconcile(
            profile,
            host,
            root,
            &self.receipt_path(profile.vendor),
            artifact,
            &self.host_environment(profile),
            self.runner,
        )
    }
}

fn claude_hooks_disabled(bytes: &[u8]) -> Result<bool> {
    Ok(
        document::boolean(bytes, &["disableAllHooks".into()])? == Some(true)
            || document::boolean(bytes, &["allowManagedHooksOnly".into()])? == Some(true),
    )
}

fn recover_replacement(
    path: &Path,
    target: &Path,
    mut receipt: Receipt,
    current: Option<&[u8]>,
) -> Result<Receipt> {
    if receipt.target != target || receipt.manager_source.is_some() {
        return Err("receipt_target_mismatch");
    }
    if receipt.published || receipt.previous.is_none() {
        return Ok(receipt);
    }
    let previous = receipt.previous.take().ok_or("state_invalid")?;
    if previous.target != target
        || !previous.published
        || previous.removed
        || previous.manager_source.is_some()
        || previous.previous.is_some()
    {
        return Err("state_invalid");
    }
    if current == Some(receipt.installed.as_slice())
        || current.is_some_and(|bytes| receipt.rollback(bytes).is_ok())
    {
        receipt.published = true;
        transaction::save_receipt(path, &receipt)?;
        return Ok(receipt);
    }
    if current == receipt.original.as_deref()
        || current == previous.original.as_deref()
        || current.is_some_and(|bytes| previous.rollback(bytes).is_ok())
    {
        transaction::save_receipt(path, &previous)?;
        return Ok(*previous);
    }
    Err("interrupted_upgrade_requires_review")
}

pub(super) fn status(vendor: Vendor, phase: IntegrationPhase, reason: &str) -> IntegrationStatus {
    IntegrationStatus {
        vendor,
        phase,
        reason: reason.into(),
        host_version: None,
        profile: None,
        node_version: None,
        owned: false,
    }
}

fn with_profile(mut status: IntegrationStatus, profile: &Profile) -> IntegrationStatus {
    status.profile = Some(profile.root.clone());
    status
}

fn registered_phase(vendor: Vendor) -> IntegrationPhase {
    if matches!(vendor, Vendor::Claude | Vendor::Codex) {
        IntegrationPhase::AwaitingTrust
    } else {
        IntegrationPhase::ReloadRequired
    }
}

pub fn render_config(
    vendor: Vendor,
    original: &[u8],
    node: Option<&Runtime>,
    root: &Path,
    cli: &Path,
) -> Result<(Vec<u8>, Vec<document::Delta>)> {
    let mut bytes = original.to_vec();
    let mut deltas = Vec::new();
    if vendor == Vendor::Cursor {
        match document::value(&bytes, &["version".into()])? {
            None => {
                let (next, delta) = document::insert_member(&bytes, "version", json!(1))?;
                bytes = next;
                deltas.push(delta);
            }
            Some(version) if version == json!(1) => (),
            _ => return Err("cursor_schema_version_requires_review"),
        }
    }
    let entries: Vec<(&[&str], Value)> = match vendor {
        Vendor::Claude | Vendor::Codex => {
            let node = node.ok_or("compatible_node_22_required")?;
            let command = hook_command(
                &node.executable,
                &root
                    .join(vendor_directory(vendor))
                    .join("hooks/session-check-hook.mjs"),
                cli,
            )?;
            let hook = json!({"hooks":[{"type":"command","command":command,"timeout":5}]});
            if vendor == Vendor::Claude {
                vec![
                    (&["hooks", "UserPromptSubmit"], hook.clone()),
                    (&["hooks", "PostToolUse"], hook),
                ]
            } else {
                vec![(&["hooks", "UserPromptSubmit"], hook)]
            }
        }
        Vendor::Cursor => {
            let node = node.ok_or("compatible_node_22_required")?;
            let command = hook_command(
                &node.executable,
                &root.join("cursor/hooks/session-check-hook.mjs"),
                cli,
            )?;
            // Cursor's version is part of its native schema, never inferred from a project hook.
            vec![(&["hooks", "postToolUse"], json!({"command":command}))]
        }
        Vendor::OpenCode => {
            let url = url::Url::from_file_path(root.join("opencode/runtime/antiburn.mjs"))
                .map_err(|_| "unsafe_path")?;
            vec![(&["plugin"], Value::String(url.to_string()))]
        }
        Vendor::Pi => return Err("native_package_manager_required"),
    };
    for (path, value) in entries {
        let path = path.iter().map(|key| (*key).to_owned()).collect::<Vec<_>>();
        let (next, delta) = document::append(&bytes, &path, value)?;
        bytes = next;
        deltas.push(delta);
    }
    Ok((bytes, deltas))
}

#[cfg(all(test, unix))]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::agent_integrations::process::ProcessError;

    struct NoCommands;

    impl Runner for NoCommands {
        fn run(
            &self,
            _: &Path,
            _: &[String],
            _: &BTreeMap<String, String>,
            _: Duration,
        ) -> std::result::Result<String, ProcessError> {
            panic!("Closed native gates must not run host or Node probes")
        }
    }

    fn validated_compatibility() -> Compatibility {
        let mut manifest = Compatibility::read().expect("read the bundled compatibility fixture");
        for route in &mut manifest.routes {
            route.os = std::env::consts::OS.into();
            route.arch = std::env::consts::ARCH.into();
            route.native_validated = true;
        }
        manifest
    }

    fn detected_profiles(home: &Path, bin: &Path) -> BTreeMap<String, String> {
        fs::create_dir(bin).expect("create the fixture host directory");
        let env = BTreeMap::from([("PATH".into(), bin.to_string_lossy().into_owned())]);
        for vendor in Vendor::ALL {
            let profile = discovery::profile(vendor, home, &env, None)
                .expect("derive the fixture vendor profile");
            fs::create_dir_all(profile.root).expect("create the fixture vendor profile");
            let host = bin.join(if vendor == Vendor::Cursor {
                "cursor-agent"
            } else {
                vendor_directory(vendor)
            });
            fs::write(&host, "#!/bin/sh\nexit 1\n").expect("write the fixture host launcher");
            fs::set_permissions(host, fs::Permissions::from_mode(0o700))
                .expect("make the fixture host launcher executable");
        }
        env
    }

    fn cli() -> CliConnection {
        CliConnection {
            executable: "/internal/antiburn-cli".into(),
            version: "fixture".into(),
            sha256: "fixture".into(),
            delivery_protocols: vec!["delivery-v1".into()],
        }
    }

    #[test]
    fn compatibility_requires_the_exact_native_route_for_each_vendor() {
        for vendor in Vendor::ALL {
            let mut manifest = validated_compatibility();
            let version = manifest
                .routes
                .iter()
                .find(|route| route.vendor == vendor)
                .expect("find the fixture vendor version")
                .version
                .clone();
            assert!(manifest.accepted_route(vendor, &version).is_some());
            manifest
                .routes
                .iter_mut()
                .find(|route| route.vendor == vendor)
                .expect("find the fixture route to change")
                .route = "different_native_route".into();
            assert!(manifest.accepted_route(vendor, &version).is_none());
            assert!(!manifest.has_accepted_route(vendor));
        }
    }

    #[test]
    fn closed_gates_skip_all_host_and_node_probes_and_profile_snapshots() {
        let temp = tempfile::tempdir().expect("create the closed-gate fixture directory");
        let root = temp
            .path()
            .canonicalize()
            .expect("resolve the closed-gate fixture directory");
        let state = root.join("state");
        transaction::private_directory(&state).expect("create the closed-gate state directory");
        let env = detected_profiles(&root, &root.join("bin"));
        let installation = Installation {
            state: &state,
            resources: &root,
            home: &root,
            env: &env,
            runner: &NoCommands,
        };
        for setup in [false, true] {
            let statuses = installation.statuses(Some(&cli()), setup);
            assert!(statuses.iter().all(|status| {
                status.phase == IntegrationPhase::PendingNativeGate && status.host_version.is_none()
            }));
            assert_eq!(
                fs::read_dir(&state)
                    .expect("list the closed-gate state files")
                    .count(),
                usize::from(setup)
            );
        }
        for vendor in Vendor::ALL {
            let profile = discovery::profile(vendor, &root, &env, None)
                .expect("derive the closed-gate repair profile");
            transaction::publish(
                &state.join(format!("{}-profile.json", vendor_directory(vendor))),
                None,
                &serde_json::to_vec(&profile).expect("encode the closed-gate repair profile"),
            )
            .expect("save the closed-gate repair profile");
            let receipt = Receipt::prepare(
                profile.root.join("settings.json"),
                None,
                b"{}".to_vec(),
                Vec::new(),
                "fixture".into(),
            )
            .expect("prepare the closed-gate repair receipt");
            let receipt_path = installation.receipt_path(vendor);
            transaction::save_receipt(&receipt_path, &receipt)
                .expect("save the closed-gate repair receipt");
            let before = fs::read(&receipt_path).expect("read the receipt before gated repair");
            assert_eq!(
                installation.repair(vendor),
                Err("native_route_not_validated")
            );
            assert_eq!(
                fs::read(&receipt_path).expect("read the receipt after gated repair"),
                before
            );
        }
    }

    #[derive(Default)]
    struct ProfileWritingProbe {
        environments: RefCell<Vec<BTreeMap<String, String>>>,
        fail: bool,
    }

    impl Runner for ProfileWritingProbe {
        fn run(
            &self,
            executable: &Path,
            args: &[String],
            env: &BTreeMap<String, String>,
            _: Duration,
        ) -> std::result::Result<String, ProcessError> {
            assert_eq!(args, &["--version"]);
            self.environments.borrow_mut().push(env.clone());
            for (key, value) in env {
                if key == "HOME"
                    || key.starts_with("XDG_")
                    || key.ends_with("_DIR")
                    || key == "CODEX_HOME"
                {
                    fs::write(Path::new(value).join("probe-marker"), b"probe wrote config")
                        .expect("write the isolated probe marker");
                }
            }
            if self.fail {
                return Err(ProcessError::Failed);
            }
            let vendor = Vendor::ALL
                .into_iter()
                .find(|vendor| {
                    let name = if *vendor == Vendor::Cursor {
                        "cursor-agent"
                    } else {
                        vendor_directory(*vendor)
                    };
                    executable
                        .file_name()
                        .expect("read the fixture launcher name")
                        == name
                })
                .expect("identify the fixture launcher vendor");
            let version = validated_compatibility()
                .routes
                .into_iter()
                .find(|route| route.vendor == vendor)
                .expect("find the fixture launcher version")
                .version;
            Ok(match vendor {
                Vendor::Claude => format!("{version} (Claude Code)"),
                Vendor::Codex => format!("codex-cli {version}"),
                _ => version,
            })
        }
    }

    #[test]
    fn accepted_inspection_isolates_host_writes_and_skips_node_discovery() {
        let temp = tempfile::tempdir().expect("create the accepted-route inspection fixture");
        let root = temp
            .path()
            .canonicalize()
            .expect("resolve the accepted-route inspection fixture");
        let state = root.join("state");
        transaction::private_directory(&state).expect("create the accepted-route state directory");
        let mut env = detected_profiles(&root, &root.join("bin"));
        for name in ["XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME"] {
            env.insert(name.into(), root.to_string_lossy().into_owned());
        }
        let runner = ProfileWritingProbe::default();
        let installation = Installation {
            state: &state,
            resources: &root,
            home: &root,
            env: &env,
            runner: &runner,
        };
        let statuses = installation.statuses_with_compatibility(
            Some(&cli()),
            false,
            &validated_compatibility(),
        );
        assert!(statuses.iter().all(|status| {
            status.reason == "setup_not_registered" && status.host_version.is_some()
        }));
        assert_eq!(runner.environments.borrow().len(), Vendor::ALL.len());
        for environment in runner.environments.borrow().iter() {
            let scratch = Path::new(&environment["HOME"]);
            assert_ne!(scratch, root);
            assert!(!scratch.exists());
            for name in [
                "XDG_CONFIG_HOME",
                "XDG_DATA_HOME",
                "XDG_CACHE_HOME",
                "XDG_STATE_HOME",
                "TMPDIR",
            ] {
                assert_eq!(Path::new(&environment[name]), scratch);
            }
        }
        assert!(!root.join("probe-marker").exists());
        assert_eq!(
            fs::read_dir(state)
                .expect("list state files after accepted-route inspection")
                .count(),
            0
        );
        for vendor in Vendor::ALL {
            let profile = discovery::profile(vendor, &root, &env, None)
                .expect("derive the inspected fixture profile");
            assert_eq!(
                fs::read_dir(profile.root)
                    .expect("list the inspected vendor profile files")
                    .count(),
                0
            );
        }
    }

    #[test]
    fn failed_host_probe_removes_its_isolated_profile() {
        let temp = tempfile::tempdir().expect("create the failed-probe fixture directory");
        let runner = ProfileWritingProbe {
            fail: true,
            ..Default::default()
        };
        let installation = Installation {
            state: temp.path(),
            resources: temp.path(),
            home: temp.path(),
            env: &BTreeMap::new(),
            runner: &runner,
        };
        assert_eq!(
            installation.probe_host_version(Vendor::Pi, Path::new("/unused"), PROBE_TIMEOUT),
            Err("host_version_unavailable")
        );
        let environments = runner.environments.borrow();
        assert_eq!(environments.len(), 1);
        assert!(!Path::new(&environments[0]["HOME"]).exists());
    }

    #[test]
    fn host_probe_shares_reserve_time_for_each_accepted_vendor() {
        let manifest = validated_compatibility();
        let budget = Duration::from_millis(100);
        let mut discovery = RuntimeDiscovery {
            directories: Vec::new(),
            compatibility: &manifest,
            node: None,
            node_searched: false,
            probe_budget: budget,
        };
        let mut allocated = Duration::ZERO;
        for vendor in Vendor::ALL {
            let timeout = discovery.probe_timeout(vendor);
            assert!(!timeout.is_zero() && timeout <= PROBE_TIMEOUT);
            allocated += timeout;
            discovery.probe_budget -= timeout;
        }
        assert_eq!(allocated, budget);
        assert!(discovery.probe_budget.is_zero());
    }

    fn volta_fixture(root: &Path) -> (PathBuf, PathBuf) {
        // Volta 2.0.2, commit e83f04ccd79be994c0e65c0bb56838fbcda6fcf9:
        // crates/volta-layout/src/v4.rs and crates/volta-core/src/tool/package/metadata.rs.
        let volta = root.join(".volta");
        let shim = volta.join("bin/opencode");
        let config = volta.join("tools/user/bins/opencode.json");
        let executable = volta.join("tools/image/packages/@fixture/agent/bin/opencode");
        let node = volta.join("tools/image/node/22.0.0/bin/node");
        for path in [&shim, &config, &executable, &node] {
            fs::create_dir_all(
                path.parent()
                    .expect("find the Volta fixture file directory"),
            )
            .expect("create the Volta fixture file directory");
        }
        fs::write(volta.join("layout.v4"), b"").expect("write the Volta layout marker fixture");
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "name":"opencode", "package":"@fixture/agent", "version":"1.0.0",
                "platform":{"node":"22.0.0", "npm":null, "pnpm":null, "yarn":null}, "manager":"Npm",
            }))
            .expect("encode the Volta command metadata fixture"),
        )
        .expect("write the Volta command metadata fixture");
        fs::write(
            &shim,
            r#"#!/bin/sh
[ -f "${VOLTA_HOME:-$HOME/.volta}/tools/user/bins/opencode.json" ] || exit 1
printf '1.19.3'
"#,
        )
        .expect("write the metadata-dependent Volta shim fixture");
        fs::write(&node, "#!/bin/sh\nexec /bin/sh \"$@\"\n")
            .expect("write the pinned Node launcher fixture");
        fs::write(
            &executable,
            r#"#!/usr/bin/env node
[ "$1" = '--version' ] || exit 2
[ -z "$VOLTA_HOME" ] || exit 3
[ ! -e "$HOME/.volta" ] || exit 4
printf probe > "$HOME/probe-marker"
printf config > "$OPENCODE_CONFIG_DIR/probe-marker"
printf '1.19.3'
"#,
        )
        .expect("write the installed Volta command fixture");
        for path in [&shim, &executable, &node] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .expect("make the Volta fixture command executable");
        }
        (shim, config)
    }

    #[test]
    fn isolated_probe_resolves_installed_volta_command_and_pinned_node() {
        use crate::agent_integrations::process::NativeRunner;

        let temp = tempfile::tempdir().expect("create the installed Volta probe fixture");
        let root = temp
            .path()
            .canonicalize()
            .expect("resolve the installed Volta probe fixture");
        let (shim, config) = volta_fixture(&root);
        let state = root.join("state");
        transaction::private_directory(&state)
            .expect("create the installed Volta probe state directory");
        let profile = root.join(".config/opencode");
        fs::create_dir_all(&profile).expect("create the Volta probe vendor profile fixture");
        let env = BTreeMap::from([(
            "PATH".into(),
            shim.parent()
                .expect("find the Volta shim fixture directory")
                .to_string_lossy()
                .into_owned(),
        )]);
        assert_eq!(
            NativeRunner
                .run(
                    &shim,
                    &["--version".into()],
                    &BTreeMap::from([("HOME".into(), root.to_string_lossy().into_owned())]),
                    PROBE_TIMEOUT,
                )
                .expect("run the metadata-dependent fixture with its normal HOME"),
            "1.19.3"
        );
        let before = fs::read(&config).expect("read Volta command metadata before inspection");
        let installation = Installation {
            state: &state,
            resources: &root,
            home: &root,
            env: &env,
            runner: &NativeRunner,
        };
        let statuses = installation.statuses_with_compatibility(
            Some(&cli()),
            false,
            &validated_compatibility(),
        );
        let opencode = statuses
            .iter()
            .find(|status| status.vendor == Vendor::OpenCode)
            .expect("find the inspected OpenCode fixture status");
        assert_eq!(opencode.host_version.as_deref(), Some("1.19.3"));
        assert_eq!(opencode.reason, "setup_not_registered");
        assert_eq!(
            fs::read(config).expect("read Volta command metadata after inspection"),
            before
        );
        assert_eq!(
            fs::read_dir(state)
                .expect("list state files after the Volta probe")
                .count(),
            0
        );
        assert_eq!(
            fs::read_dir(profile)
                .expect("list vendor profile files after the Volta probe")
                .count(),
            0
        );
        assert!(!root.join("probe-marker").exists());
    }

    #[test]
    fn captured_custom_volta_root_keeps_the_probe_profile_isolated() {
        use crate::agent_integrations::process::NativeRunner;

        struct IsolatedVoltaRunner {
            home: PathBuf,
            volta: PathBuf,
        }

        impl Runner for IsolatedVoltaRunner {
            fn run(
                &self,
                executable: &Path,
                args: &[String],
                env: &BTreeMap<String, String>,
                timeout: Duration,
            ) -> std::result::Result<String, ProcessError> {
                assert_eq!(
                    executable,
                    self.volta
                        .join("tools/image/packages/@fixture/agent/bin/opencode")
                );
                assert_eq!(
                    std::env::split_paths(
                        env.get("PATH").expect("read the custom Volta probe PATH")
                    )
                    .next(),
                    Some(self.volta.join("tools/image/node/22.0.0/bin"))
                );
                assert_ne!(
                    Path::new(
                        env.get("HOME")
                            .expect("read the isolated custom Volta HOME")
                    ),
                    self.home
                );
                assert!(!env.contains_key("VOLTA_HOME"));
                assert_ne!(
                    Path::new(
                        env.get("OPENCODE_CONFIG_DIR")
                            .expect("read the isolated custom Volta profile")
                    ),
                    self.home.join(".config/opencode")
                );
                NativeRunner.run(executable, args, env, timeout)
            }
        }

        let temp = tempfile::tempdir().expect("create the captured custom Volta fixture");
        let root = temp
            .path()
            .canonicalize()
            .expect("resolve the captured custom Volta fixture");
        let home = root.join("home");
        let profile = home.join(".config/opencode");
        fs::create_dir_all(&profile).expect("create the custom Volta vendor profile fixture");
        let (shim, config) = volta_fixture(&root.join("runtime"));
        let volta = root.join("runtime/.volta");
        assert!(!volta.starts_with(&home));
        let state = root.join("state");
        transaction::private_directory(&state).expect("create the custom Volta state directory");
        let source = BTreeMap::from([
            (
                "PATH",
                shim.parent()
                    .expect("find the custom Volta shim directory")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("VOLTA_HOME", volta.to_string_lossy().into_owned()),
            (
                "OPENCODE_CONFIG_DIR",
                profile.to_string_lossy().into_owned(),
            ),
        ]);
        let env = super::super::desktop::capture_environment(|key| source.get(key).cloned());
        assert_eq!(env.get("VOLTA_HOME"), source.get("VOLTA_HOME"));
        let before = fs::read(&config).expect("read the custom Volta metadata before inspection");
        let runner = IsolatedVoltaRunner {
            home: home.clone(),
            volta,
        };
        let installation = Installation {
            state: &state,
            resources: &root,
            home: &home,
            env: &env,
            runner: &runner,
        };
        let statuses = installation.statuses_with_compatibility(
            Some(&cli()),
            false,
            &validated_compatibility(),
        );
        let opencode = statuses
            .iter()
            .find(|status| status.vendor == Vendor::OpenCode)
            .expect("find the captured custom Volta inspection status");
        assert_eq!(opencode.host_version.as_deref(), Some("1.19.3"));
        assert_eq!(opencode.reason, "setup_not_registered");
        assert_eq!(
            fs::read(config).expect("read the custom Volta metadata after inspection"),
            before
        );
        assert_eq!(
            fs::read_dir(state)
                .expect("list state files after the custom Volta probe")
                .count(),
            0
        );
        assert_eq!(
            fs::read_dir(profile)
                .expect("list vendor profile files after the custom Volta probe")
                .count(),
            0
        );
        assert!(!home.join("probe-marker").exists());
    }

    #[test]
    fn unsupported_volta_metadata_never_runs_the_shim() {
        let temp = tempfile::tempdir().expect("create the unsupported Volta metadata fixture");
        let root = temp
            .path()
            .canonicalize()
            .expect("resolve the unsupported Volta metadata fixture");
        let (shim, config) = volta_fixture(&root);
        let installation = Installation {
            state: &root,
            resources: &root,
            home: &root,
            env: &BTreeMap::new(),
            runner: &NoCommands,
        };
        let valid: Value = serde_json::from_slice(
            &fs::read(&config).expect("read the supported Volta metadata fixture"),
        )
        .expect("parse the supported Volta metadata fixture");
        for (field, value) in [
            ("package", json!("../../outside")),
            ("manager", json!("unknown")),
        ] {
            let mut changed = valid.clone();
            changed[field] = value;
            fs::write(
                &config,
                serde_json::to_vec(&changed)
                    .expect("encode the unsupported Volta metadata fixture"),
            )
            .expect("write the unsupported Volta metadata fixture");
            assert_eq!(
                installation.probe_host_version(Vendor::OpenCode, &shim, PROBE_TIMEOUT),
                Err("host_launcher_requires_review")
            );
        }
        fs::write(
            &config,
            serde_json::to_vec(&valid).expect("encode the restored Volta metadata fixture"),
        )
        .expect("restore the supported Volta metadata fixture");
        fs::remove_file(root.join(".volta/tools/image/node/22.0.0/bin/node"))
            .expect("remove the pinned Node fixture");
        assert_eq!(
            installation.probe_host_version(Vendor::OpenCode, &shim, PROBE_TIMEOUT),
            Err("host_launcher_requires_review")
        );
    }

    struct MixedProbes {
        calls: RefCell<Vec<(PathBuf, Duration)>>,
        healthy: ProfileWritingProbe,
    }

    impl Runner for MixedProbes {
        fn run(
            &self,
            executable: &Path,
            args: &[String],
            env: &BTreeMap<String, String>,
            timeout: Duration,
        ) -> std::result::Result<String, ProcessError> {
            self.calls
                .borrow_mut()
                .push((executable.to_path_buf(), timeout));
            if matches!(
                executable
                    .file_name()
                    .expect("read the mixed-probe launcher name")
                    .to_str(),
                Some("claude" | "codex")
            ) {
                std::thread::sleep(timeout);
                Err(ProcessError::Timeout)
            } else {
                self.healthy.run(executable, args, env, timeout)
            }
        }
    }

    #[test]
    fn two_hanging_early_hosts_leave_time_for_healthy_later_hosts() {
        let temp = tempfile::tempdir().expect("create the mixed-probe budget fixture");
        let root = temp
            .path()
            .canonicalize()
            .expect("resolve the mixed-probe budget fixture");
        let env = detected_profiles(&root, &root.join("bin"));
        let runner = MixedProbes {
            calls: RefCell::new(Vec::new()),
            healthy: ProfileWritingProbe::default(),
        };
        let installation = Installation {
            state: &root,
            resources: &root,
            home: &root,
            env: &env,
            runner: &runner,
        };
        let manifest = validated_compatibility();
        let budget = Duration::from_secs(1);
        let mut discovery = RuntimeDiscovery {
            directories: vec![root.join("bin")],
            compatibility: &manifest,
            node: None,
            node_searched: false,
            probe_budget: budget,
        };
        let started = Instant::now();
        for vendor in Vendor::ALL {
            let result = installation.reconcile_vendor(vendor, Some(&cli()), &mut discovery, false);
            if matches!(vendor, Vendor::Claude | Vendor::Codex) {
                assert!(matches!(result, Err("host_version_unavailable")));
            } else {
                let result = result.expect("inspect the healthy later-vendor fixture");
                assert!(result.host_version.is_some());
                assert_eq!(result.reason, "setup_not_registered");
            }
        }
        let calls = runner.calls.borrow();
        assert_eq!(calls.len(), Vendor::ALL.len());
        assert_eq!(calls[0].1, budget / 5);
        assert!(
            calls
                .iter()
                .all(|(_, timeout)| !timeout.is_zero() && *timeout <= PROBE_TIMEOUT)
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(!discovery.probe_budget.is_zero());
    }
}
