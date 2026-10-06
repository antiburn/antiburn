//! Bounded transcript bundles for local indexing on another user-owned machine.

use std::collections::HashSet;
use std::io::{Read, Write};

use antiburn_local::discovery::scanner::parse_session_metadata_str;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::evidence::{AdmittedFile, RequestBudget};
use crate::{RemoteSession, collector};

pub const MAX_BUNDLE_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub const MAX_FILES: usize = 512;
pub const MAX_SIDECAR_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BundleFile {
    pub size: u64,
    pub version: String,
    pub subagent_id: Option<String>,
    pub label: Option<String>,
    pub sidecar_for: Option<usize>,
    #[serde(default)]
    pub fork_parent: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BundleManifest {
    pub session: RemoteSession,
    pub files: Vec<BundleFile>,
    pub fork_parent_session_id: Option<String>,
}

impl BundleManifest {
    pub fn signature(&self) -> Result<String> {
        use std::fmt::Write as _;

        let digest = Sha256::digest(serde_json::to_vec(self)?);
        let mut signature = String::with_capacity(digest.len() * 2);
        for byte in digest {
            write!(&mut signature, "{byte:02x}")?;
        }
        Ok(signature)
    }

    pub fn validate(&self) -> Result<()> {
        self.session.validate()?;
        ensure!(
            self.fork_parent_session_id
                .as_deref()
                .is_none_or(safe_identity),
            "Invalid fork identity"
        );
        ensure!(
            !self.files.is_empty() && self.files.len() <= MAX_FILES,
            "Invalid bundle file count"
        );
        ensure!(
            matches!(self.session.agent.as_str(), "claude-code" | "codex"),
            "Unsupported bundle agent"
        );
        ensure!(
            self.files[0].subagent_id.is_none()
                && self.files[0].sidecar_for.is_none()
                && !self.files[0].fork_parent,
            "Missing parent transcript"
        );

        let mut total = 0u64;
        let mut ids = HashSet::new();
        let mut fork_parents = 0;
        let mut sidecars = HashSet::new();
        let mut names = HashSet::new();
        for (index, file) in self.files.iter().enumerate() {
            total = total
                .checked_add(file.size)
                .context("Bundle size overflow")?;
            ensure!(total <= MAX_BUNDLE_BYTES, "Bundle exceeds 1 GiB");
            ensure!(file.version.len() <= 128, "Invalid file version");
            ensure!(
                file.label.as_ref().is_none_or(|label| label.len() <= 800),
                "Invalid companion label"
            );
            if file.fork_parent {
                fork_parents += 1;
                ensure!(
                    fork_parents == 1
                        && self
                            .fork_parent_session_id
                            .as_ref()
                            .is_some_and(|id| id != &self.session.session_id)
                        && file.sidecar_for.is_none()
                        && file.subagent_id.is_none(),
                    "Invalid fork companion"
                );
            } else if let Some(parent) = file.sidecar_for {
                ensure!(
                    parent < index
                        && self.files[parent].sidecar_for.is_none()
                        && !self.files[parent].fork_parent
                        && sidecars.insert(parent),
                    "Invalid sidecar reference"
                );
                ensure!(
                    file.size <= MAX_SIDECAR_BYTES && file.subagent_id.is_none(),
                    "Invalid sidecar"
                );
            } else if index != 0 {
                let id = file
                    .subagent_id
                    .as_ref()
                    .context("Missing subagent identity")?;
                ensure!(
                    safe_identity(id) && ids.insert(id),
                    "Invalid subagent identity"
                );
            }
            ensure!(names.insert(self.file_name(index)), "Duplicate bundle path");
        }
        ensure!(
            fork_parents == usize::from(self.fork_parent_session_id.is_some()),
            "Missing fork companion"
        );
        Ok(())
    }

    pub fn file_name(&self, index: usize) -> String {
        let file = &self.files[index];
        if file.fork_parent {
            return format!(
                "{}.jsonl",
                self.fork_parent_session_id.as_deref().unwrap_or("invalid")
            );
        }
        if let Some(parent) = file.sidecar_for {
            return self.file_name(parent).trim_end_matches(".jsonl").to_owned() + ".meta.json";
        }
        match &file.subagent_id {
            Some(id) => format!(
                "{}/subagents/agent-{}.jsonl",
                self.session.session_id,
                id.strip_prefix("agent-").unwrap_or(id)
            ),
            None => format!("{}.jsonl", self.session.session_id),
        }
    }
}

pub(crate) fn safe_identity(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

struct CompanionFile {
    source: AdmittedFile,
    subagent_id: Option<String>,
    label: Option<String>,
    sidecar_for: Option<usize>,
    fork_parent: bool,
}

struct PreparedBundle {
    manifest: BundleManifest,
    files: Vec<AdmittedFile>,
}

pub async fn write_bundle(
    agent: &str,
    session_id: &str,
    known: Option<&str>,
    out: &mut impl Write,
) -> Result<()> {
    let mut budget = RequestBudget::new();
    let entry = collector::discover_matching_with_budget(agent, session_id, &mut budget)?;
    let prepared = prepare_bundle(entry, agent, session_id, &mut budget)?;
    write_prepared_bundle(prepared, known, out)
}

fn prepare_bundle(
    entry: collector::Entry,
    agent: &str,
    session_id: &str,
    budget: &mut RequestBudget,
) -> Result<PreparedBundle> {
    let parent_root = entry.source.root.clone();
    let parent_relative = entry.source.relative.clone();
    ensure!(
        agent != "codex" || entry.companion_roster_complete,
        "Too many companion transcripts"
    );
    let mut companions = vec![CompanionFile {
        source: entry.source,
        subagent_id: None,
        label: None,
        sidecar_for: None,
        fork_parent: false,
    }];

    if agent == "claude-code" {
        let parent_dir = parent_relative
            .parent()
            .context("Missing parent directory")?;
        let children_relative = parent_dir.join(session_id).join("subagents");
        let children_path = parent_root.path().join(&children_relative);
        if let Ok(entries) = std::fs::read_dir(children_path) {
            let mut children = Vec::new();
            for child in entries {
                budget.visit_entry()?;
                let child = child?;
                let kind = child.file_type()?;
                if !kind.is_file() || kind.is_symlink() {
                    continue;
                }
                let relative = children_relative.join(child.file_name());
                if relative.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                    continue;
                }
                let id = relative
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .context("Missing subagent identity")?
                    .to_owned();
                ensure!(safe_identity(&id), "Invalid subagent identity");
                ensure!(
                    companions.len() + children.len() < MAX_FILES,
                    "Too many companion transcripts"
                );
                budget.inspect_candidate()?;
                children.push((id, parent_root.admit(&relative)?));
            }
            children.sort_by(|left, right| left.0.cmp(&right.0));
            for (id, source) in children {
                companions.push(CompanionFile {
                    source,
                    subagent_id: Some(id),
                    label: None,
                    sidecar_for: None,
                    fork_parent: false,
                });
            }
        }
    } else {
        for child in entry.codex_companions {
            ensure!(
                companions.len() < MAX_FILES,
                "Too many companion transcripts"
            );
            companions.push(CompanionFile {
                source: child.source,
                subagent_id: Some(child.session_id),
                label: Some(
                    parse_session_metadata_str(&child.preview)
                        .title
                        .filter(|title| !title.trim().is_empty())
                        .unwrap_or_else(|| "Sub-agent".to_owned())
                        .chars()
                        .take(200)
                        .collect(),
                ),
                sidecar_for: None,
                fork_parent: false,
            });
        }
    }

    if agent == "claude-code" {
        let mut sidecars = Vec::new();
        for (index, companion) in companions.iter().enumerate() {
            if companion.subagent_id.is_none() {
                continue;
            }
            let sidecar = companion.source.relative.with_extension("meta.json");
            match std::fs::symlink_metadata(parent_root.path().join(&sidecar)) {
                Ok(_) => {
                    ensure!(
                        companions.len() + sidecars.len() < MAX_FILES,
                        "Too many companion transcripts"
                    );
                    budget.inspect_candidate()?;
                    sidecars.push(CompanionFile {
                        source: parent_root.admit(&sidecar)?,
                        subagent_id: None,
                        label: None,
                        sidecar_for: Some(index),
                        fork_parent: false,
                    })
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        companions.extend(sidecars);
    }

    let fork_parent_session_id =
        antiburn_local::discovery::fork::fork_parent_from_content(&entry.preview);
    if agent == "claude-code"
        && let Some(id) = &fork_parent_session_id
    {
        ensure!(
            safe_identity(id) && id != session_id,
            "Invalid fork identity"
        );
        ensure!(
            companions.len() < MAX_FILES,
            "Too many companion transcripts"
        );
        budget.inspect_candidate()?;
        companions.push(CompanionFile {
            source: parent_root.admit(&parent_relative.with_file_name(format!("{id}.jsonl")))?,
            subagent_id: None,
            label: None,
            sidecar_for: None,
            fork_parent: true,
        });
    }
    let fork_parent_session_id = (agent == "claude-code")
        .then_some(fork_parent_session_id)
        .flatten();

    ensure!(
        companions.len() <= MAX_FILES,
        "Too many companion transcripts"
    );
    let mut total = 0u64;
    for companion in &companions {
        let size = companion.source.stat.size;
        if companion.sidecar_for.is_some() {
            ensure!(size <= MAX_SIDECAR_BYTES, "Sidecar exceeds 64 KiB");
        }
        total = total.checked_add(size).context("Bundle size overflow")?;
        ensure!(total <= MAX_BUNDLE_BYTES, "Bundle exceeds 1 GiB");
    }

    // Admission, file-count, and byte limits are complete before labels read.
    for companion in companions
        .iter_mut()
        .filter(|companion| companion.subagent_id.is_some() && companion.label.is_none())
    {
        let read = budget.read_label(&companion.source.file)?;
        companion.label = Some(
            parse_session_metadata_str(&read.text())
                .title
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| "Sub-agent".to_owned())
                .chars()
                .take(200)
                .collect(),
        );
    }

    let mut files = Vec::with_capacity(companions.len());
    let mut descriptors = Vec::with_capacity(companions.len());
    for companion in companions {
        companion.source.validate_unchanged()?;
        descriptors.push(BundleFile {
            size: companion.source.pinned_size(),
            version: companion.source.pinned_version(),
            subagent_id: companion.subagent_id,
            label: companion.label,
            sidecar_for: companion.sidecar_for,
            fork_parent: companion.fork_parent,
        });
        files.push(companion.source);
    }

    let manifest = BundleManifest {
        session: entry.session,
        files: descriptors,
        fork_parent_session_id,
    };
    manifest.validate()?;
    Ok(PreparedBundle { manifest, files })
}

fn write_prepared_bundle(
    mut prepared: PreparedBundle,
    known: Option<&str>,
    out: &mut impl Write,
) -> Result<()> {
    let manifest = &prepared.manifest;
    let header = serde_json::to_vec(&manifest)?;
    ensure!(header.len() <= MAX_MANIFEST_BYTES, "Manifest exceeds limit");
    let signature = manifest.signature()?;
    let unchanged = known == Some(signature.as_str());
    out.write_all(if unchanged { b"ABR2SAME" } else { b"ABR2DATA" })?;
    out.write_all(&(header.len() as u32).to_le_bytes())?;
    out.write_all(&header)?;
    if unchanged {
        for source in &prepared.files {
            source.validate_unchanged()?;
        }
        return Ok(());
    }
    for (index, source) in prepared.files.iter_mut().enumerate() {
        let copied = std::io::copy(
            &mut std::io::Read::by_ref(&mut source.file).take(manifest.files[index].size),
            out,
        )?;
        ensure!(
            copied == manifest.files[index].size && source.validate_unchanged().is_ok(),
            "Session changed during transfer; retry"
        );
    }
    for source in &prepared.files {
        source.validate_unchanged()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use antiburn_local::discovery::source_version;
    use std::fs::OpenOptions;
    use std::path::Path;

    fn claude_entry(
        child_count: usize,
        child_size: Option<u64>,
    ) -> (tempfile::TempDir, collector::Entry) {
        let directory = tempfile::tempdir().unwrap();
        let root_path = directory.path().join("projects");
        let project = root_path.join("synthetic");
        let children = project.join("parent/subagents");
        std::fs::create_dir_all(&children).unwrap();
        let preview = concat!(
            "{\"type\":\"user\",\"sessionId\":\"parent\",",
            "\"message\":{\"role\":\"user\",\"content\":\"Task\"}}\n"
        )
        .to_owned();
        std::fs::write(project.join("parent.jsonl"), &preview).unwrap();
        for index in 0..child_count {
            let path = children.join(format!("agent-child-{index:04}.jsonl"));
            if let Some(size) = child_size {
                let file = std::fs::File::create(path).unwrap();
                file.set_len(size).unwrap();
            } else {
                std::fs::write(
                    path,
                    format!(
                        "{{\"type\":\"user\",\"sessionId\":\"child-{index:04}\",\"message\":{{\"role\":\"user\",\"content\":\"Child {index}\"}}}}\n"
                    ),
                )
                .unwrap();
            }
        }
        let root = crate::evidence::TrustedRoot::open(&root_path).unwrap();
        let source = root.admit(Path::new("synthetic/parent.jsonl")).unwrap();
        let head_hash = source_version::head_hash_of(preview.as_bytes());
        (
            directory,
            collector::Entry {
                session: RemoteSession {
                    agent: "claude-code".into(),
                    session_id: "parent".into(),
                    title: "Task".into(),
                    cwd: None,
                    surface: "cli".into(),
                    updated_at: Some(1),
                },
                agent_type: antiburn_local::model::AgentKind::Claude,
                source,
                preview,
                head_hash,
                codex_companions: Vec::new(),
                companion_roster_complete: true,
            },
        )
    }

    fn manifest() -> BundleManifest {
        BundleManifest {
            session: RemoteSession {
                agent: "claude-code".into(),
                session_id: "parent".into(),
                title: "Synthetic".into(),
                cwd: None,
                surface: "cli".into(),
                updated_at: Some(1),
            },
            files: vec![BundleFile {
                size: 10,
                version: "10:1".into(),
                subagent_id: None,
                label: None,
                sidecar_for: None,
                fork_parent: false,
            }],
            fork_parent_session_id: None,
        }
    }

    #[test]
    fn manifest_accepts_only_derived_relative_paths() {
        let mut bundle = manifest();
        let mut child = bundle.files[0].clone();
        child.subagent_id = Some("child".into());
        bundle.files.push(child);
        let mut sidecar = bundle.files[0].clone();
        sidecar.sidecar_for = Some(1);
        bundle.files.push(sidecar);
        bundle.validate().unwrap();
        assert_eq!(bundle.file_name(1), "parent/subagents/agent-child.jsonl");
        assert_eq!(
            bundle.file_name(2),
            "parent/subagents/agent-child.meta.json"
        );
        bundle.files[1].subagent_id = Some("../../escape".into());
        assert!(bundle.validate().is_err());
    }

    #[test]
    fn manifest_rejects_oversize_duplicates_and_missing_fork_evidence() {
        let mut bundle = manifest();
        bundle.files[0].size = MAX_BUNDLE_BYTES + 1;
        assert!(bundle.validate().is_err());

        bundle = manifest();
        let mut child = bundle.files[0].clone();
        child.subagent_id = Some("child".into());
        bundle.files.extend([child.clone(), child]);
        assert!(bundle.validate().is_err());

        bundle = manifest();
        bundle.fork_parent_session_id = Some("ancestor".into());
        assert!(bundle.validate().is_err());
    }

    #[test]
    fn exact_file_limit_is_accepted_and_next_file_rejects_before_labels() {
        let (_directory, entry) = claude_entry(MAX_FILES - 1, None);
        let mut budget = RequestBudget::new();
        let prepared = prepare_bundle(entry, "claude-code", "parent", &mut budget).unwrap();
        assert_eq!(prepared.manifest.files.len(), MAX_FILES);
        assert!(budget.label_byte_count() > 0);

        let (_directory, entry) = claude_entry(MAX_FILES, None);
        let mut budget = RequestBudget::new();
        let error = prepare_bundle(entry, "claude-code", "parent", &mut budget)
            .err()
            .unwrap();
        assert!(error.to_string().contains("Too many companion"));
        assert_eq!(budget.label_byte_count(), 0);
    }

    #[test]
    fn bundle_size_rejects_sparse_child_before_label_read() {
        let (_directory, entry) = claude_entry(1, Some(MAX_BUNDLE_BYTES));
        let mut budget = RequestBudget::new();
        let error = prepare_bundle(entry, "claude-code", "parent", &mut budget)
            .err()
            .unwrap();
        assert!(error.to_string().contains("Bundle exceeds"));
        assert_eq!(budget.label_byte_count(), 0);
    }

    struct MutatingWriter {
        path: std::path::PathBuf,
        mutated: bool,
        bytes: Vec<u8>,
    }

    impl Write for MutatingWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if !self.mutated {
                OpenOptions::new()
                    .append(true)
                    .open(&self.path)?
                    .write_all(b"mutated-after-manifest")?;
                self.mutated = true;
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn mutation_during_transfer_is_rejected_against_admission_snapshot() {
        let (directory, entry) = claude_entry(0, None);
        let path = directory.path().join("projects/synthetic/parent.jsonl");
        let mut budget = RequestBudget::new();
        let prepared = prepare_bundle(entry, "claude-code", "parent", &mut budget).unwrap();
        let mut writer = MutatingWriter {
            path,
            mutated: false,
            bytes: Vec::new(),
        };

        let error = write_prepared_bundle(prepared, None, &mut writer).unwrap_err();

        assert!(error.to_string().contains("changed during transfer"));
        assert!(writer.bytes.starts_with(b"ABR2DATA"));
    }

    #[test]
    fn export_never_reads_a_replacement_path_after_parent_admission() {
        let (directory, entry) = claude_entry(0, None);
        let path = directory.path().join("projects/synthetic/parent.jsonl");
        std::fs::rename(&path, path.with_extension("admitted")).unwrap();
        std::fs::write(
            &path,
            concat!(
                "{\"type\":\"user\",\"sessionId\":\"parent\",",
                "\"message\":{\"role\":\"user\",",
                "\"content\":\"outside-replacement-marker\"}}\n"
            ),
        )
        .unwrap();
        let mut budget = RequestBudget::new();

        let Ok(prepared) = prepare_bundle(entry, "claude-code", "parent", &mut budget) else {
            return;
        };
        let mut output = Vec::new();
        let result = write_prepared_bundle(prepared, None, &mut output);

        assert!(!String::from_utf8_lossy(&output).contains("outside-replacement-marker"));
        if result.is_ok() {
            assert!(String::from_utf8_lossy(&output).contains("Task"));
        }
    }
}
