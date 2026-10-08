//! Private remote transcripts indexed by the ordinary evidence worker.

use crate::{
    analysis::{self, PassSignal},
    store::{FencedTurnRowStore, SessionKey, SessionRecord, Store},
};
use antiburn_local::{
    analysis::TurnRowStore,
    discovery::{
        SessionSource,
        source_version::{FINGERPRINT_HEAD_BYTES, FingerprintInputs, SourceStat, head_hash_of},
    },
};
use antiburn_remote::{
    PROTOCOL_VERSION, RemoteSession, Request,
    export::{BundleManifest, MAX_BUNDLE_BYTES, MAX_MANIFEST_BYTES},
    transport,
};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn manifest_for(record: &SessionRecord) -> Result<(PathBuf, BundleManifest)> {
    ensure!(
        record.key.remote_host_id().is_some(),
        "Not a remote session"
    );
    let dir = Path::new(&record.source_label)
        .parent()
        .context("Missing transcript directory")?
        .to_path_buf();
    let path = dir.join("manifest.json");
    ensure!(
        fs::metadata(&path)?.len() <= MAX_MANIFEST_BYTES as u64,
        "Manifest exceeds limit"
    );
    let manifest: BundleManifest = serde_json::from_slice(&fs::read(path)?)?;
    manifest.validate()?;
    ensure!(
        manifest.session.agent == record.key.agent
            && manifest.session.session_id == record.key.session_id,
        "Cached transcript identity mismatch"
    );
    Ok((dir, manifest))
}

fn parent_fingerprint(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let stat = SourceStat::from_open_std_file(&file).context("Cannot stat transcript")?;
    let mut head = Vec::new();
    (&mut file)
        .take(FINGERPRINT_HEAD_BYTES as u64)
        .read_to_end(&mut head)?;
    Ok(FingerprintInputs {
        stat,
        head_hash: Some(head_hash_of(&head)),
    }
    .fingerprint())
}

fn safe_cache_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub fn unpack(
    bundle: &Path,
    target: &Path,
    expected: &RemoteSession,
) -> Result<(BundleManifest, bool)> {
    let mut input = std::io::BufReader::new(fs::File::open(bundle)?);
    let mut magic = [0u8; 8];
    input.read_exact(&mut magic)?;
    ensure!(
        &magic == b"ABR2SAME" || &magic == b"ABR2DATA",
        "Update the remote helper to support transcript syncing"
    );
    let mut length = [0u8; 4];
    input.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    ensure!(length <= MAX_MANIFEST_BYTES, "Manifest exceeds limit");
    let mut bytes = vec![0u8; length];
    input.read_exact(&mut bytes)?;
    let manifest: BundleManifest = serde_json::from_slice(&bytes)?;
    manifest.validate()?;
    ensure!(
        manifest.session.agent == expected.agent
            && manifest.session.session_id == expected.session_id,
        "Exported session identity mismatch"
    );
    let unchanged = &magic == b"ABR2SAME";
    if !unchanged {
        for (index, descriptor) in manifest.files.iter().enumerate() {
            let path = target.join(manifest.file_name(index));
            private_dir(path.parent().context("Missing cache directory")?)?;
            let mut options = fs::OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(path)?;
            ensure!(
                std::io::copy(&mut (&mut input).take(descriptor.size), &mut file)?
                    == descriptor.size,
                "Incomplete transcript transfer"
            );
            file.sync_all()?;
        }
        fs::write(target.join("manifest.json"), bytes)?;
    }
    let mut trailing = [0u8; 1];
    ensure!(
        input.read(&mut trailing)? == 0,
        "Unexpected data after transcript bundle"
    );
    Ok((manifest, unchanged))
}

fn cache_size(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut size = 0u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            size = size.saturating_add(cache_size(&entry.path())?);
        } else if kind.is_file() {
            size = size.saturating_add(entry.metadata()?.len());
        }
    }
    Ok(size)
}

fn declared_bundle_size(path: &Path) -> Result<(u64, u64)> {
    let mut input = std::io::BufReader::new(fs::File::open(path)?);
    let mut magic = [0_u8; 8];
    input.read_exact(&mut magic)?;
    ensure!(
        &magic == b"ABR2SAME" || &magic == b"ABR2DATA",
        "Invalid transcript bundle"
    );
    let mut length = [0_u8; 4];
    input.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    ensure!(length <= MAX_MANIFEST_BYTES, "Manifest exceeds limit");
    let mut bytes = vec![0_u8; length];
    input.read_exact(&mut bytes)?;
    let manifest: BundleManifest = serde_json::from_slice(&bytes)?;
    manifest.validate()?;
    let files = manifest.files.iter().try_fold(0_u64, |total, file| {
        total.checked_add(file.size).context("Bundle size overflow")
    })?;
    Ok((files, length as u64))
}

const CACHE_LIMIT_BYTES: u64 = 14 * 1024 * 1024 * 1024;
const STAGING_LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

fn staging_projection_fits(
    existing: u64,
    bundle: u64,
    extracted_files: u64,
    extracted_manifest: u64,
) -> bool {
    existing
        .saturating_add(bundle)
        .saturating_add(extracted_files)
        .saturating_add(extracted_manifest)
        <= STAGING_LIMIT_BYTES
}

fn generation_name() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncResult {
    Imported,
    Unchanged,
}

pub async fn sync_session(
    app: &tauri::AppHandle,
    root: &Path,
    host_id: &str,
    ssh_alias: &str,
    scan_generation: u64,
    session: &RemoteSession,
    store: &Store,
) -> Result<SyncResult> {
    let cache_root = root.join("transcripts");
    let staging_root = root.join("staging");
    private_dir(&staging_root)?;
    let cache_root_for_size = cache_root.clone();
    let staging_for_size = staging_root.clone();
    let (cache_before, staging_before) = tokio::task::spawn_blocking(move || {
        Ok::<_, anyhow::Error>((
            cache_size(&cache_root_for_size)?,
            cache_size(&staging_for_size)?,
        ))
    })
    .await??;
    ensure!(
        cache_before < CACHE_LIMIT_BYTES,
        "Remote cache reached 14 GiB; remove an unused host in Settings before syncing more sessions"
    );
    ensure!(
        staging_before
            .saturating_add(MAX_BUNDLE_BYTES)
            .saturating_add(MAX_MANIFEST_BYTES as u64)
            .saturating_add(12)
            <= STAGING_LIMIT_BYTES,
        "Remote staging reached 2 GiB; wait for another remote scan to finish"
    );
    ensure!(
        cache_before
            .saturating_add(MAX_BUNDLE_BYTES)
            .saturating_add(MAX_MANIFEST_BYTES as u64)
            <= CACHE_LIMIT_BYTES,
        "Remote cache reached 14 GiB; remove an unused host in Settings before syncing more sessions"
    );
    let key = SessionKey::for_origin(&session.agent, &session.session_id, None, Some(host_id))
        .map_err(anyhow::Error::msg)?;
    let previous = store.session(&key)?;
    let known = previous
        .as_ref()
        .and_then(|record| manifest_for(record).ok())
        .filter(|(dir, manifest)| {
            manifest.files.iter().enumerate().all(|(index, file)| {
                fs::metadata(dir.join(manifest.file_name(index)))
                    .is_ok_and(|meta| meta.len() == file.size)
            })
        })
        .and_then(|(_, manifest)| manifest.signature().ok());
    let parent = cache_parent(root, &key)?;
    private_dir(&parent)?;
    let retained = previous
        .as_ref()
        .and_then(|record| Path::new(&record.source_label).parent())
        .map(Path::to_path_buf);
    for entry in fs::read_dir(&parent)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() && retained.as_deref() != Some(entry.path().as_path()) {
            fs::remove_dir_all(entry.path())?;
        }
    }
    let staging = staging_root.join(generation_name()?);
    let staging_cleanup = staging.clone();
    private_dir(&staging)?;
    let bundle = staging.join("transfer.bin");
    let result = async {
        transport::export_to(
            ssh_alias,
            &Request::Export {
                version: PROTOCOL_VERSION,
                agent: session.agent.clone(),
                session_id: session.session_id.clone(),
                known,
            },
            &bundle,
        )
        .await?;
        let bundle_size = fs::metadata(&bundle)?.len();
        let (declared_size, manifest_size) = declared_bundle_size(&bundle)?;
        ensure!(
            staging_projection_fits(
                staging_before,
                bundle_size,
                declared_size,
                manifest_size,
            ),
            "Remote staging reached 2 GiB; wait for another remote scan to finish"
        );
        let bundle_copy = bundle.clone();
        let target_copy = staging.clone();
        let expected = session.clone();
        let (manifest, unchanged) =
            tokio::task::spawn_blocking(move || unpack(&bundle_copy, &target_copy, &expected))
                .await??;
        fs::remove_file(&bundle)?;
        if unchanged {
            let old = previous.as_ref().context("Missing existing snapshot")?;
            let (old_dir, old_manifest) = manifest_for(old)?;
            ensure!(
                manifest.signature()? == old_manifest.signature()?,
                "Unchanged response does not match the cached snapshot"
            );
            ensure!(
                old_manifest.files.iter().enumerate().all(|(index, file)| {
                    fs::metadata(old_dir.join(old_manifest.file_name(index)))
                        .is_ok_and(|meta| meta.len() == file.size)
                }),
                "Cached transcript is incomplete; remove and re-add the host to resync"
            );
            fs::remove_dir_all(&staging)?;
            return Ok(SyncResult::Unchanged);
        }
        ensure!(
            crate::remote_sync::is_current(app, host_id, scan_generation),
            "scan cancelled"
        );
        let staged_size = cache_size(&staging)?;
        ensure!(
            staging_before.saturating_add(staged_size) <= STAGING_LIMIT_BYTES,
            "Remote staging reached 2 GiB; wait for another remote scan to finish"
        );
        ensure!(
            cache_before.saturating_add(staged_size) <= CACHE_LIMIT_BYTES,
            "Remote cache reached 14 GiB; remove an unused host in Settings before syncing more sessions"
        );
        let target = parent.join(generation_name()?);
        let path = target.join(manifest.file_name(0));
        let staged_path = staging.join(manifest.file_name(0));
        let record = SessionRecord {
            key,
            source_kind: "file".to_owned(),
            source_label: path.to_string_lossy().into_owned(),
            wsl_distro: None,
            title: Some(manifest.session.title.clone()),
            title_source: Some("indexed".to_owned()),
            cwd: manifest.session.cwd.clone(),
            surface: manifest.session.surface.clone(),
            client: "unknown".into(),
            updated_at_epoch: manifest.session.updated_at,
            activity_cursor: manifest.signature()?,
            activity_source: "mtime".to_owned(),
            subagent_count: manifest
                .files
                .iter()
                .filter(|file| file.subagent_id.is_some())
                .count() as u32,
            fork_parent_session_id: manifest.fork_parent_session_id,
            source_fingerprint: Some(parent_fingerprint(&staged_path)?),
        };
        crate::remote_sync::with_commit_guard(app, host_id, scan_generation, || {
            fs::rename(&staging, &target)?;
            if let Err(error) = store.upsert_sessions(&[record], &["claude-code", "codex"])
            {
                if let Err(cleanup) = fs::remove_dir_all(&target) {
                    return Err(error.context(format!(
                        "Cache rollback failed and needs lifecycle cleanup: {cleanup}"
                    )));
                }
                return Err(error);
            }
            Ok(())
        })?;
        if let Some(old) = previous
            && let Some(old_dir) = Path::new(&old.source_label)
                .parent()
                .filter(|dir| dir.parent() == Some(parent.as_path()))
        {
            let _ = fs::remove_dir_all(old_dir);
        }
        Ok(SyncResult::Imported)
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging_cleanup);
    }
    result
}

fn prepare_fork_parent(
    store: &Store,
    record: &SessionRecord,
    dir: &Path,
    manifest: &BundleManifest,
    parent: Option<&str>,
) -> Result<bool> {
    let Some(parent) = parent.filter(|_| record.key.agent == "claude-code") else {
        return Ok(false);
    };
    ensure!(
        parent != record.key.session_id,
        "A session cannot be its own fork parent"
    );
    ensure!(safe_cache_identity(parent), "Invalid fork parent identity");
    if manifest.fork_parent_session_id.as_deref() == Some(parent)
        && manifest.files.iter().any(|file| file.fork_parent)
    {
        return Ok(false);
    }
    let key = SessionKey::new(&record.key.environment_key, &record.key.agent, parent);
    let Some(parent_record) = store.session(&key)? else {
        return Ok(false);
    };
    let _ = manifest_for(&parent_record)?;
    let target = dir.join(format!("{parent}.jsonl"));
    if parent_fingerprint(&target).ok()
        == Some(parent_fingerprint(Path::new(&parent_record.source_label))?)
    {
        return Ok(false);
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let temp = dir.join(format!("fork-parent-{nonce}.tmp"));
    // Immutable cached generations can share the parent file without duplicating its content.
    fs::hard_link(&parent_record.source_label, &temp)?;
    if let Err(error) = fs::rename(&temp, target) {
        let _ = fs::remove_file(temp);
        return Err(error.into());
    }
    Ok(true)
}

pub fn restore_fork_companions(store: &Store) -> Result<usize> {
    let mut repaired = 0;
    for record in store.recent_sessions(0, 10_000)? {
        if record.key.remote_host_id().is_none() || record.key.agent != "claude-code" {
            continue;
        }
        let Some(parent) = store.fork_parent(&record.key)? else {
            continue;
        };
        let repair = manifest_for(&record).and_then(|(dir, manifest)| {
            prepare_fork_parent(store, &record, &dir, &manifest, Some(&parent))
        });
        match repair {
            Ok(true) => {
                store.requeue_session_evidence(&record.key)?;
                repaired += 1;
            }
            Ok(false) => {}
            Err(error) => {
                if error.downcast_ref::<rusqlite::Error>().is_some() {
                    return Err(error);
                }
                tracing::warn!(event = "remote_fork_repair_failed", error = %error);
            }
        }
    }
    Ok(repaired)
}

pub fn prune_after_commit(store: &Store, root: &Path) {
    if let Err(error) = prune_unreferenced(store, root) {
        tracing::warn!(event = "remote_cache_cleanup_failed", error = %error);
    }
}

pub fn delete_session(
    store: &Store,
    root: Option<&Path>,
    key: &SessionKey,
) -> Result<Option<(crate::store::Incarnation, crate::store::Revision)>> {
    ensure!(key.remote_host_id().is_some(), "Not a remote session");
    let record = store.session(key)?;
    let removed = store.delete_session(key)?;
    if let Some(record) = record {
        let cleanup = root
            .context("Remote cache directory is unavailable")
            .and_then(|root| remove_cached_generation(root, &record));
        if let Err(error) = cleanup {
            tracing::warn!(event = "remote_cache_cleanup_failed", error = %error);
        }
    }
    Ok(removed)
}

fn cache_parent(root: &Path, key: &SessionKey) -> Result<PathBuf> {
    let host_id = key.remote_host_id().context("Not a remote session")?;
    ensure!(safe_cache_identity(host_id), "Invalid remote host identity");
    let identity = Sha256::digest(format!("{}\0{}", key.agent, key.session_id).as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(root.join("transcripts").join(host_id).join(identity))
}

fn remove_cached_generation(root: &Path, record: &SessionRecord) -> Result<()> {
    let directory = Path::new(&record.source_label)
        .parent()
        .context("Missing transcript directory")?;
    let canonical = match fs::canonicalize(directory) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let transcripts = fs::canonicalize(root.join("transcripts"))?;
    let expected = fs::canonicalize(cache_parent(root, &record.key)?)?;
    ensure!(
        expected.starts_with(&transcripts),
        "Remote cache path escaped storage"
    );
    ensure!(
        canonical.parent() == Some(expected.as_path()),
        "Remote cache identity mismatch"
    );
    fs::remove_dir_all(canonical)?;
    Ok(())
}

pub fn prune_unreferenced(store: &Store, root: &Path) -> Result<usize> {
    let retained = store
        .recent_sessions(0, usize::MAX)?
        .into_iter()
        .filter(|record| record.key.remote_host_id().is_some())
        .filter_map(|record| {
            Path::new(&record.source_label)
                .parent()
                .map(Path::to_path_buf)
        })
        .collect::<std::collections::HashSet<_>>();
    let transcripts = root.join("transcripts");
    if !transcripts.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    for host in fs::read_dir(transcripts)? {
        let host = host?;
        if !host.file_type()?.is_dir() {
            continue;
        }
        for identity in fs::read_dir(host.path())? {
            let identity = identity?;
            if !identity.file_type()?.is_dir() {
                continue;
            }
            for generation in fs::read_dir(identity.path())? {
                let generation = generation?;
                if generation.file_type()?.is_dir() && !retained.contains(&generation.path()) {
                    fs::remove_dir_all(generation.path())?;
                    removed += 1;
                }
            }
        }
    }
    Ok(removed)
}

pub fn cleanup_staging(root: &Path) -> Result<()> {
    let staging = root.join("staging");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    private_dir(&staging)
}

pub fn run_pass(
    record: SessionRecord,
    signal: PassSignal,
    fence: i64,
    store: Store,
) -> crate::insights_worker::PassFuture {
    Box::pin(async move {
        let Ok((dir, manifest)) = manifest_for(&record) else {
            return analysis::unsupported_evidence_pass();
        };
        let Some(agent) = crate::agents::kind_from_slug(&record.key.agent) else {
            return analysis::unsupported_evidence_pass();
        };
        let children = manifest
            .files
            .iter()
            .enumerate()
            .filter_map(|(index, file)| {
                file.subagent_id.as_ref().map(|id| {
                    (
                        id.clone(),
                        file.label.clone().unwrap_or_else(|| "Sub-agent".to_owned()),
                        dir.join(manifest.file_name(index)),
                    )
                })
            })
            .collect();
        let parent = store.fork_parent(&record.key).ok().flatten();
        if prepare_fork_parent(&store, &record, &dir, &manifest, parent.as_deref()).is_err() {
            return analysis::unavailable_evidence_pass(
                analysis::PassOutcome::Unreadable(analysis::UnreadableReason::ClaimFailed),
                None,
                None,
            );
        }
        let writer: Arc<dyn TurnRowStore> =
            Arc::new(FencedTurnRowStore::new(store, record.key.clone(), fence));
        analysis::analyze_located_for_evidence(
            agent,
            &record.key.session_id,
            analysis::ClaimedSource {
                fingerprint: record.source_fingerprint,
                generation: 0,
            },
            signal,
            Some(writer),
            parent,
            analysis::LocatedTranscripts {
                source: SessionSource::File(PathBuf::from(record.source_label)),
                children,
            },
        )
        .await
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use antiburn_remote::export::BundleFile;

    fn manifest(data: &[u8]) -> BundleManifest {
        BundleManifest {
            session: RemoteSession {
                agent: "claude-code".into(),
                session_id: "synthetic".into(),
                title: "Synthetic".into(),
                cwd: None,
                surface: "cli".into(),
                updated_at: Some(1),
            },
            files: vec![BundleFile {
                size: data.len() as u64,
                version: "fixture".into(),
                subagent_id: None,
                label: None,
                sidecar_for: None,
                fork_parent: false,
            }],
            fork_parent_session_id: None,
        }
    }

    fn transfer(path: &Path, manifest: &BundleManifest, data: &[u8], same: bool) {
        let header = serde_json::to_vec(manifest).unwrap();
        let mut bytes = if same {
            b"ABR2SAME".to_vec()
        } else {
            b"ABR2DATA".to_vec()
        };
        bytes.extend((header.len() as u32).to_le_bytes());
        bytes.extend(header);
        bytes.extend(data);
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn imports_private_transcripts_and_rejects_incomplete_or_mismatched_bundles() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("bundle");
        let data = b"synthetic transcript\n";
        let manifest = manifest(data);
        transfer(&bundle, &manifest, data, false);
        let target = temp.path().join("snapshot");
        private_dir(&target).unwrap();
        assert!(!unpack(&bundle, &target, &manifest.session).unwrap().1);
        let path = target.join("synthetic.jsonl");
        assert_eq!(fs::read(&path).unwrap(), data);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let mut wrong = manifest.session.clone();
        wrong.session_id = "another".into();
        assert!(unpack(&bundle, &temp.path().join("wrong"), &wrong).is_err());
        transfer(&bundle, &manifest, b"short", false);
        assert!(unpack(&bundle, &temp.path().join("partial"), &manifest.session).is_err());
        assert_eq!(fs::read(path).unwrap(), data);
        transfer(&bundle, &manifest, b"", true);
        assert!(unpack(&bundle, &target, &manifest.session).unwrap().1);
        transfer(&bundle, &manifest, b"extra", true);
        assert!(unpack(&bundle, &target, &manifest.session).is_err());
    }

    fn cached_record(path: &Path) -> SessionRecord {
        SessionRecord {
            key: SessionKey::new("ssh:host", "claude-code", "synthetic"),
            source_kind: "file".into(),
            source_label: path.to_string_lossy().into_owned(),
            wsl_distro: None,
            title: None,
            title_source: None,
            cwd: None,
            surface: "cli".into(),
            client: "unknown".into(),
            updated_at_epoch: Some(1),
            activity_cursor: "fixture".into(),
            activity_source: "mtime".into(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: None,
        }
    }

    #[test]
    fn committed_deletion_survives_cleanup_errors_and_rejects_unowned_paths() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(temp.path()).unwrap();
        let root = temp.path().join("remote");
        let parent = cache_parent(
            &root,
            &SessionKey::new("ssh:host", "claude-code", "synthetic"),
        )
        .unwrap();
        fs::create_dir_all(&parent).unwrap();
        let broken = parent.join("generation");
        fs::write(&broken, "not a directory").unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), "keep").unwrap();
        for directory in [&broken, &outside] {
            let record = cached_record(&directory.join("synthetic.jsonl"));
            store
                .upsert_sessions(std::slice::from_ref(&record), &["claude-code"])
                .unwrap();
            assert!(remove_cached_generation(&root, &record).is_err());
            assert!(
                delete_session(&store, Some(&root), &record.key)
                    .unwrap()
                    .is_some()
            );
            assert!(store.session(&record.key).unwrap().is_none());
            assert!(directory.exists());
        }
        assert_eq!(fs::read_to_string(outside.join("keep")).unwrap(), "keep");
        let good = parent.join("valid");
        fs::create_dir(&good).unwrap();
        fs::write(good.join("synthetic.jsonl"), "fixture").unwrap();
        let record = cached_record(&good.join("synthetic.jsonl"));
        store
            .upsert_sessions(std::slice::from_ref(&record), &["claude-code"])
            .unwrap();
        assert!(
            delete_session(&store, Some(&root), &record.key)
                .unwrap()
                .is_some()
        );
        assert!(!good.exists());
    }

    #[test]
    fn pruning_failure_does_not_undo_committed_retention() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(temp.path()).unwrap();
        let record = cached_record(&temp.path().join("synthetic.jsonl"));
        store
            .upsert_sessions(std::slice::from_ref(&record), &["claude-code"])
            .unwrap();
        fs::write(temp.path().join("transcripts"), "not a directory").unwrap();
        store
            .update_settings(|settings| settings.session_data_retention_days = 30)
            .unwrap();
        let (removed, revision) = store.apply_session_retention(4_000_000_000).unwrap();
        assert_eq!(removed, 1);
        assert!(prune_unreferenced(&store, temp.path()).is_err());
        prune_after_commit(&store, temp.path());
        assert_eq!(revision, store.revision());
        assert!(store.session(&record.key).unwrap().is_none());
        let mut settings = store.settings().unwrap();
        settings.session_data_retention_days = 90;
        let (_, saved, ()) = store
            .replace_settings_preserving_interface_scale(&settings, |_, _, _| Ok(()))
            .unwrap();
        prune_after_commit(&store, temp.path());
        assert_eq!(saved.session_data_retention_days, 90);
        assert_eq!(store.settings().unwrap().session_data_retention_days, 90);
    }

    #[test]
    fn staging_projection_accounts_for_the_extracted_manifest_copy() {
        assert!(staging_projection_fits(
            STAGING_LIMIT_BYTES - 30,
            10,
            10,
            10,
        ));
        assert!(!staging_projection_fits(
            STAGING_LIMIT_BYTES - 29,
            10,
            10,
            10,
        ));
    }

    #[tokio::test]
    async fn imported_transcript_uses_full_analysis_and_retains_tools_and_billable_usage() {
        let temp = tempfile::tempdir().unwrap();
        let data = br#"{"type":"user","uuid":"u1","timestamp":"2026-09-15T10:00:00Z","message":{"role":"user","content":"Inspect the file"}}
{"type":"assistant","uuid":"a1","timestamp":"2026-09-15T10:00:01Z","message":{"id":"m1","role":"assistant","model":"claude-sonnet-4-20250514","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/synthetic/example.rs"}}],"usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}
{"type":"user","uuid":"u2","timestamp":"2026-09-15T10:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"Example"}]}}
{"type":"assistant","uuid":"a2","timestamp":"2026-09-15T10:00:03Z","message":{"id":"m2","role":"assistant","model":"claude-sonnet-4-20250514","content":[{"type":"text","text":"Reviewed"}],"usage":{"input_tokens":120,"output_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}
"#;
        let manifest = manifest(data);
        let bundle = temp.path().join("bundle");
        transfer(&bundle, &manifest, data, false);
        let target = temp.path().join("snapshot");
        unpack(&bundle, &target, &manifest.session).unwrap();
        let path = target.join(manifest.file_name(0));
        let pass = analysis::analyze_located_for_evidence(
            crate::agents::kind_from_slug("claude-code").unwrap(),
            "synthetic",
            analysis::ClaimedSource {
                fingerprint: Some(parent_fingerprint(&path).unwrap()),
                generation: 0,
            },
            PassSignal::new(),
            Some(antiburn_local::analysis::MemoryTurnRowStore::new(
                "claude-code",
                "synthetic",
            )),
            None,
            analysis::LocatedTranscripts {
                source: SessionSource::File(path),
                children: vec![],
            },
        )
        .await;
        assert!(matches!(pass.outcome, analysis::PassOutcome::Published));
        assert!(pass.evidence.is_some());
        assert!(pass.analysis.summary.is_some());
        assert_eq!(
            pass.analysis
                .metrics
                .as_ref()
                .unwrap()
                .billable_input_tokens,
            220
        );
        assert_eq!(
            pass.analysis
                .metrics
                .as_ref()
                .unwrap()
                .billable_output_tokens,
            30
        );
        assert!(!pass.analysis.inclusive_model_breakdown.is_empty());
        let encoded = serde_json::to_string(&pass.analysis.summary).unwrap();
        assert!(
            encoded.contains("Read"),
            "tool analysis must survive import"
        );
    }

    #[tokio::test]
    async fn inferred_forks_exclude_inherited_usage_from_the_same_host_only() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(temp.path()).unwrap();
        let inherited = r#"{"type":"assistant","uuid":"inherited","timestamp":"2026-09-15T10:00:00Z","message":{"id":"first","role":"assistant","model":"claude-sonnet-4-6","content":[{"type":"text","text":"Inherited"}],"usage":{"input_tokens":10,"output_tokens":2}}}
"#;
        let own = r#"{"type":"assistant","uuid":"own","timestamp":"2026-09-15T10:01:00Z","message":{"id":"second","role":"assistant","model":"claude-sonnet-4-6","content":[{"type":"text","text":"New work"}],"usage":{"input_tokens":5,"output_tokens":1}}}
"#;
        let mut records = Vec::new();
        for (host, id, data) in [
            ("one", "ancestor", inherited.to_owned()),
            ("one", "synthetic", format!("{inherited}{own}")),
            ("two", "synthetic", format!("{inherited}{own}")),
        ] {
            let dir = temp.path().join(host).join(id);
            private_dir(&dir).unwrap();
            let mut manifest = manifest(data.as_bytes());
            manifest.session.session_id = id.to_owned();
            let bundle = dir.join("bundle");
            transfer(&bundle, &manifest, data.as_bytes(), false);
            unpack(&bundle, &dir, &manifest.session).unwrap();
            let path = dir.join(manifest.file_name(0));
            let record = SessionRecord {
                key: SessionKey::for_origin("claude-code", id, None, Some(host)).unwrap(),
                source_kind: "file".into(),
                source_label: path.to_string_lossy().into_owned(),
                wsl_distro: None,
                title: None,
                title_source: None,
                cwd: None,
                surface: "cli".into(),
                client: "unknown".into(),
                updated_at_epoch: Some(1),
                activity_cursor: "fixture".into(),
                activity_source: "mtime".into(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: Some(parent_fingerprint(&path).unwrap()),
            };
            store
                .upsert_sessions(std::slice::from_ref(&record), &["claude-code"])
                .unwrap();
            records.push(record);
        }
        store
            .record_fork_parent(&records[1].key, "ancestor")
            .unwrap();
        let mut damaged = records[1].clone();
        damaged.key = SessionKey::new("ssh:one", "claude-code", "damaged");
        damaged.source_label = temp
            .path()
            .join("missing/damaged.jsonl")
            .to_string_lossy()
            .into_owned();
        damaged.updated_at_epoch = Some(2);
        store
            .upsert_sessions(std::slice::from_ref(&damaged), &["claude-code"])
            .unwrap();
        store.record_fork_parent(&damaged.key, "ancestor").unwrap();
        assert_eq!(store.recent_sessions(0, 10).unwrap()[0].key, damaged.key);
        assert_eq!(restore_fork_companions(&store).unwrap(), 1);
        assert_eq!(restore_fork_companions(&store).unwrap(), 0);
        assert_eq!(
            fs::read_to_string(&records[1].source_label).unwrap(),
            format!("{inherited}{own}"),
            "fork repair must preserve the child transcript",
        );
        for (record, expected) in [(&records[1], 5), (&records[2], 15)] {
            let (dir, manifest) = manifest_for(record).unwrap();
            prepare_fork_parent(&store, record, &dir, &manifest, Some("ancestor")).unwrap();
            let pass = analysis::analyze_located_for_evidence(
                crate::agents::kind_from_slug("claude-code").unwrap(),
                "synthetic",
                analysis::ClaimedSource {
                    fingerprint: record.source_fingerprint.clone(),
                    generation: 0,
                },
                PassSignal::new(),
                Some(antiburn_local::analysis::MemoryTurnRowStore::new(
                    "claude-code",
                    "synthetic",
                )),
                Some("ancestor".into()),
                analysis::LocatedTranscripts {
                    source: SessionSource::File(PathBuf::from(&record.source_label)),
                    children: vec![],
                },
            )
            .await;
            assert!(matches!(pass.outcome, analysis::PassOutcome::Published));
            assert_eq!(
                pass.analysis.metrics.unwrap().billable_input_tokens,
                expected
            );
        }
        fs::remove_dir_all(Path::new(&records[0].source_label).parent().unwrap()).unwrap();
        assert!(
            Path::new(&records[1].source_label)
                .parent()
                .unwrap()
                .join("ancestor.jsonl")
                .exists()
        );
    }

    #[test]
    fn origin_identity_isolates_identical_ids_and_host_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(temp.path()).unwrap();
        let keys: Vec<_> = [None, Some("one"), Some("two")]
            .into_iter()
            .map(|host| SessionKey::for_origin("codex", "same", None, host).unwrap())
            .collect();
        let now = antiburn_remote::now();
        for key in &keys {
            let record = SessionRecord {
                key: key.clone(),
                source_kind: "file".into(),
                source_label: "/synthetic/session.jsonl".into(),
                wsl_distro: None,
                title: None,
                title_source: None,
                cwd: key.remote_host_id().map(|_| "/remote/project".to_owned()),
                surface: "cli".into(),
                client: "unknown".into(),
                updated_at_epoch: Some(now),
                activity_cursor: "1".into(),
                activity_source: "mtime".into(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: None,
            };
            store.upsert_sessions(&[record], &["codex"]).unwrap();
        }
        store
            .observe_provider_account("codex", "openai", &"a".repeat(64), now + 1, "provider_live")
            .unwrap();
        let bindings = store.session_bound_accounts(&keys).unwrap();
        assert!(
            bindings
                .keys()
                .any(|(key, _)| key.environment_key == "native")
        );
        assert!(
            bindings
                .keys()
                .all(|(key, _)| key.remote_host_id().is_none())
        );
        assert_eq!(store.usage_evidence(0).unwrap().len(), 1);
        store.delete_remote_host("one").unwrap();
        assert!(store.session(&keys[0]).unwrap().is_some());
        assert!(store.session(&keys[1]).unwrap().is_none());
        assert!(store.session(&keys[2]).unwrap().is_some());
    }
}
