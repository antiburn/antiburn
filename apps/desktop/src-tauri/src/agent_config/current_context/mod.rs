//! Bounded current file observations. This module does not resolve agent availability.

mod admission;
mod instructions;
mod read;
mod skills;
#[cfg(test)]
mod tests;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use antiburn_local::{
    analysis::SourceFormat,
    instructions::{CurrentContextRoot, CurrentContextSnapshot},
    model::AgentKind,
};
use sha2::{Digest, Sha256};

use super::ConfigContext;
use admission::PathAdmission;
use read::ReadBudget;

/// Use source-owned paths, not the desktop process's working directory or home.
#[derive(Debug, Clone, Copy)]
pub struct CurrentContextRequest<'a> {
    pub agent: AgentKind,
    pub source_format: Option<SourceFormat>,
    pub cwd: Option<&'a Path>,
    pub repo_root: Option<&'a Path>,
    pub home: &'a Path,
    pub granted_dirs: &'a HashSet<String>,
    pub native_environment: bool,
    pub runtime_override_present: bool,
    pub managed_configuration_present: bool,
}

impl<'a> CurrentContextRequest<'a> {
    /// Grants must come from the app's recorded consent snapshot, without probes.
    pub fn from_context(context: &'a ConfigContext, granted_dirs: &'a HashSet<String>) -> Self {
        Self {
            agent: context.agent,
            source_format: context.source_format,
            cwd: context.workspace_cwd.as_deref(),
            repo_root: context.trusted_workspace_root.as_deref(),
            home: &context.home_root,
            granted_dirs,
            native_environment: context.native_environment,
            runtime_override_present: context.runtime_override_present,
            managed_configuration_present: context.managed_configuration_present,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Producer {
    Claude,
    Codex,
    Pi,
    OpenCodeV1,
    OpenCodeV2,
}

impl Producer {
    fn for_request(request: CurrentContextRequest<'_>) -> Option<Self> {
        match (request.agent, request.source_format?) {
            (AgentKind::Claude, SourceFormat::ClaudeJsonl) => Some(Self::Claude),
            (AgentKind::Codex, SourceFormat::CodexRolloutJsonl) => Some(Self::Codex),
            (AgentKind::Pi, SourceFormat::PiV3Jsonl) => Some(Self::Pi),
            (AgentKind::OpenCode, SourceFormat::OpenCodeJsonl | SourceFormat::OpenCodeSqliteV1) => {
                Some(Self::OpenCodeV1)
            }
            (AgentKind::OpenCode, SourceFormat::OpenCodeSqliteV2) => Some(Self::OpenCodeV2),
            _ => None,
        }
    }
}

struct Collector<'a> {
    snapshot: CurrentContextSnapshot,
    observed_at: String,
    reads: ReadBudget<'a>,
    directories_left: usize,
    entries_left: usize,
    instructions_left: usize,
    skills_left: usize,
    config_entries_left: usize,
    source_binding: String,
}

impl Collector<'_> {
    fn root(&mut self, global: bool) -> &mut CurrentContextRoot {
        if global {
            &mut self.snapshot.global
        } else {
            &mut self.snapshot.repo
        }
    }

    fn limit(&mut self, global: bool, skills: bool, limit: &str) {
        let root = self.root(global);
        let limits = if skills {
            &mut root.skills_limits
        } else {
            &mut root.instructions_limits
        };
        if !limits.iter().any(|value| value == limit) {
            limits.push(limit.to_owned());
        }
    }

    fn unavailable(&mut self, global: bool, reason: &str) {
        self.limit(global, false, reason);
        self.limit(global, true, reason);
    }

    fn available(&mut self, global: bool) {
        let root = self.root(global);
        root.instructions = Some(Vec::new());
        root.skills = Some(Vec::new());
    }

    fn finish(mut self) -> CurrentContextSnapshot {
        for root in [&mut self.snapshot.global, &mut self.snapshot.repo] {
            if let Some(entries) = &mut root.instructions {
                entries.sort_by(|a, b| a.path.cmp(&b.path).then(a.scope.root.cmp(&b.scope.root)));
                entries.dedup();
            }
            if let Some(entries) = &mut root.skills {
                entries.sort_by(|a, b| a.path.cmp(&b.path));
            }
            root.instructions_limits.sort();
            root.skills_limits.sort();
            root.instructions_complete =
                root.instructions.is_some() && root.instructions_limits.is_empty();
            root.skills_complete = root.skills.is_some() && root.skills_limits.is_empty();
        }
        let bytes = serde_json::to_vec(&self.snapshot).expect("current context serializes");
        let mut hash = Sha256::new();
        hash.update(b"current_file_context_v2\0");
        hash.update(self.source_binding.as_bytes());
        hash.update(bytes);
        let revision: String = hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        for root in [&mut self.snapshot.global, &mut self.snapshot.repo] {
            if root.instructions.is_some() || root.skills.is_some() {
                root.snapshot_revision = Some(revision.clone());
            }
        }
        self.snapshot
    }
}

/// Collect once at the file inventory boundary, before candidate preparation.
/// Provider retries must reuse the saved snapshot and must not call this function.
pub fn collect_current_context(request: CurrentContextRequest<'_>) -> CurrentContextSnapshot {
    collect_at(
        request,
        time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .expect("current UTC timestamp formats"),
    )
}

fn collect_at(request: CurrentContextRequest<'_>, observed_at: String) -> CurrentContextSnapshot {
    let admission = PathAdmission::new(request.home, request.granted_dirs);
    let mut granted_dirs: Vec<_> = request.granted_dirs.iter().collect();
    granted_dirs.sort();
    let mut collector = Collector {
        snapshot: CurrentContextSnapshot::default(),
        observed_at,
        reads: ReadBudget::new(admission),
        directories_left: 128,
        entries_left: 4096,
        instructions_left: 64,
        skills_left: 128,
        config_entries_left: 64,
        source_binding: format!(
            "{:?}\0{:?}\0{:?}\0{:?}\0{:?}\0{}\0{}\0{}\0{granted_dirs:?}",
            request.agent,
            request.source_format,
            request.cwd,
            request.repo_root,
            request.home,
            request.native_environment,
            request.runtime_override_present,
            request.managed_configuration_present
        ),
    };
    let producer = if !request.native_environment {
        None
    } else {
        Producer::for_request(request)
    };
    let Some(producer) = producer else {
        let reason = if request.native_environment {
            "unsupported_agent_source"
        } else {
            "non_native_environment"
        };
        collector.unavailable(true, reason);
        collector.unavailable(false, reason);
        return collector.finish();
    };
    let home = admission.canonical_directory(request.home);
    let workspace = request
        .repo_root
        .zip(request.cwd)
        .ok_or("workspace_unavailable")
        .and_then(|(root, cwd)| {
            let root = admission.canonical_directory(root)?;
            let cwd = admission.canonical_directory(cwd)?;
            if !cwd.starts_with(&root) {
                return Err("workspace_unavailable");
            }
            Ok((root, cwd))
        });
    if let Err(reason) = home {
        collector.unavailable(true, reason);
    }
    if let Err(reason) = workspace {
        collector.unavailable(false, reason);
    }
    if let Ok(home) = &home {
        collector.available(true);
        collector.collect_global(
            producer,
            home,
            workspace.as_ref().ok().map(|(_, cwd)| cwd.as_path()),
        );
    }
    if let Ok((root, cwd)) = &workspace {
        collector.available(false);
        collector.collect_repo(producer, root, cwd);
    }
    if matches!(producer, Producer::OpenCodeV1 | Producer::OpenCodeV2) {
        collector.collect_opencode_config(
            producer,
            home.as_deref().ok(),
            workspace
                .as_ref()
                .ok()
                .map(|(root, cwd)| (root.as_path(), cwd.as_path())),
        );
    }
    for global in [true, false] {
        if request.runtime_override_present {
            collector.unavailable(global, "runtime_overrides_not_resolved");
        }
        if request.managed_configuration_present {
            collector.unavailable(global, "managed_configuration_not_read");
        }
    }
    collector.finish()
}

fn hierarchy(cwd: &Path, root: &Path) -> Result<Vec<PathBuf>, &'static str> {
    let mut directories = Vec::new();
    for directory in cwd.ancestors() {
        if directories.len() == 64 {
            return Err("workspace_depth_limit");
        }
        directories.push(directory.to_owned());
        if directory == root {
            directories.reverse();
            return Ok(directories);
        }
    }
    Err("workspace_unavailable")
}
