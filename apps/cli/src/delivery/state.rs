use super::{DeliveryCheck, Update};
use crate::checks::SessionIdentity;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const LIMIT: usize = 1024;
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    consumers: BTreeMap<String, Consumer>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Consumer {
    acknowledged: BTreeMap<String, String>,
    exposed: BTreeMap<String, String>,
    pending: Option<Pending>,
    last_receipt: Option<String>,
    #[serde(default)]
    last_used: u64,
}
#[derive(PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    receipt: String,
    fingerprints: BTreeMap<String, String>,
}

pub(super) fn path(legacy: &Path) -> PathBuf {
    let mut path = legacy.as_os_str().to_os_string();
    path.push(".delivery-v1");
    PathBuf::from(path)
}
pub(super) fn key(identity: &SessionIdentity, consumer: &str) -> Result<String, String> {
    digest(&(identity, consumer))
}
fn digest(value: &impl Serialize) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
pub(super) fn stage(
    path: &Path,
    key: &str,
    checks: &[DeliveryCheck],
) -> Result<(Vec<Update>, Option<String>), String> {
    edit(path, |state| {
        if state.consumers.len() >= LIMIT && !state.consumers.contains_key(key) {
            let oldest = state
                .consumers
                .iter()
                .filter(|(_, consumer)| {
                    consumer.pending.is_none() && consumer.exposed == consumer.acknowledged
                })
                .min_by_key(|(_, consumer)| consumer.last_used)
                .map(|(key, _)| key.clone())
                .ok_or(
                    "delivery consumer state limit reached: all consumers have pending handoffs",
                )?;
            state.consumers.remove(&oldest);
        }
        let last_used = next_use(state);
        let consumer = state.consumers.entry(key.into()).or_default();
        let mut fingerprints = BTreeMap::new();
        let mut updates = Vec::new();
        for check in checks.iter().filter(|c| {
            c.status.as_deref() == Some("failure")
                && c.eligibility == "ready"
                && c.message.is_some()
        }) {
            let fingerprint = digest(&(&check.id, &check.name, &check.message))?;
            if consumer.acknowledged.get(&check.id) != Some(&fingerprint) {
                updates.push(Update {
                    id: check.id.clone(),
                    kind: "finding",
                    name: check.name.clone(),
                    message: check.message.clone(),
                });
            }
            fingerprints.insert(check.id.clone(), fingerprint);
        }
        for id in consumer
            .exposed
            .keys()
            .filter(|id| !fingerprints.contains_key(*id))
        {
            let current = checks.iter().find(|c| &c.id == id);
            updates.push(Update {
                id: id.clone(),
                kind: if current.is_some_and(|c| c.status.as_deref() == Some("success")) {
                    "cleared"
                } else {
                    "invalidated"
                },
                name: current.map_or_else(|| id.clone(), |c| c.name.clone()),
                message: current.and_then(|c| c.message.clone()),
            });
        }
        if updates.is_empty() {
            if consumer.last_used == 0
                || consumer.pending.is_some()
                || consumer.exposed != consumer.acknowledged
            {
                consumer.last_used = last_used;
            }
            consumer.pending = None;
            consumer.exposed = consumer.acknowledged.clone();
            return Ok((updates, None));
        }
        let receipt = digest(&(
            key,
            &consumer.acknowledged,
            &fingerprints,
            &consumer.last_receipt,
        ))?;
        let pending = Pending {
            receipt: receipt.clone(),
            fingerprints,
        };
        if consumer.pending.as_ref() != Some(&pending) {
            consumer.last_used = last_used;
        }
        consumer.exposed.extend(pending.fingerprints.clone());
        consumer.pending = Some(pending);
        Ok((updates, Some(receipt)))
    })
}
pub(super) fn acknowledge(path: &Path, key: &str, receipt: &str) -> Result<(), String> {
    edit(path, |state| {
        let last_used = next_use(state);
        let consumer = state
            .consumers
            .get_mut(key)
            .ok_or("unknown delivery consumer")?;
        if consumer
            .pending
            .as_ref()
            .is_none_or(|p| p.receipt != receipt)
        {
            return if consumer.last_receipt.as_deref() == Some(receipt) {
                Ok(())
            } else {
                Err("obsolete or unknown delivery receipt".into())
            };
        }
        let pending = consumer.pending.take().ok_or("missing delivery receipt")?;
        consumer.last_used = last_used;
        consumer.acknowledged = pending.fingerprints;
        consumer.exposed = consumer.acknowledged.clone();
        consumer.last_receipt = Some(pending.receipt);
        Ok(())
    })
}
fn next_use(state: &State) -> u64 {
    state
        .consumers
        .values()
        .map(|consumer| consumer.last_used)
        .max()
        .unwrap_or_default()
        .saturating_add(1)
}
fn edit<T>(
    path: &Path,
    operation: impl FnOnce(&mut State) -> Result<T, String>,
) -> Result<T, String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(PathBuf::from(lock_path))
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_millis(200);
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) => return Err(format!("delivery state lock unavailable: {error}")),
        }
    }
    if fs::metadata(path).is_ok_and(|metadata| metadata.len() > 4 * 1024 * 1024) {
        return Err("delivery state exceeds size limit".into());
    }
    let mut state = match fs::read(path) {
        Ok(bytes) if bytes.len() <= 4 * 1024 * 1024 => {
            serde_json::from_slice::<State>(&bytes).map_err(|e| e.to_string())?
        }
        Ok(_) => return Err("delivery state exceeds size limit".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State {
            version: 1,
            ..State::default()
        },
        Err(e) => return Err(e.to_string()),
    };
    if state.version != 1 {
        return Err("incompatible delivery state version".into());
    }
    let previous = serde_json::to_vec(&state).map_err(|e| e.to_string())?;
    let result = operation(&mut state)?;
    let bytes = serde_json::to_vec(&state).map_err(|e| e.to_string())?;
    if bytes == previous && path.is_file() {
        return Ok(result);
    }
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("delivery state exceeds size limit".into());
    }
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".antiburn-delivery-{}-{sequence}.tmp",
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    fs::rename(&temporary, path).map_err(|e| e.to_string())?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn check(status: &str, message: &str) -> DeliveryCheck {
        DeliveryCheck {
            id: "dynamic-id".into(),
            check_id: "retry".into(),
            name: "Retry".into(),
            message: Some(message.into()),
            status: Some(status.into()),
            assessed_at_epoch: Some(1),
            checking: false,
            eligibility: "ready".into(),
        }
    }
    #[test]
    fn capacity_reclaims_legacy_settled_consumers_and_preserves_recent_receipts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let consumers: serde_json::Map<String, serde_json::Value> = (0..LIMIT)
            .map(|index| {
                (
                    format!("legacy-{index}"),
                    serde_json::json!({
                        "acknowledged": {}, "exposed": {}, "pending": null, "last_receipt": null
                    }),
                )
            })
            .collect();
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "version": 1, "consumers": consumers
            }))
            .unwrap(),
        )
        .unwrap();
        let failure = check("failure", "Try another approach");
        let (_, token) = stage(&path, "recent", std::slice::from_ref(&failure)).unwrap();
        let token = token.unwrap();
        acknowledge(&path, "recent", &token).unwrap();
        for index in 0..LIMIT - 1 {
            stage(&path, &format!("new-{index}"), &[]).unwrap();
        }
        let mut changed = failure.clone();
        changed.message = Some("Updated advice".into());
        let (_, token) = stage(&path, "recent", std::slice::from_ref(&changed)).unwrap();
        acknowledge(&path, "recent", token.as_deref().unwrap()).unwrap();
        stage(&path, "one-more", &[]).unwrap();
        assert!(stage(&path, "recent", &[changed]).unwrap().0.is_empty());
        let state: State = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(state.consumers.len(), LIMIT);
        assert!(!state.consumers.keys().any(|key| key.starts_with("legacy-")));
    }

    #[test]
    fn capacity_preserves_pending_findings_and_withdrawals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let failure = check("failure", "Try another approach");
        let (_, finding) = stage(&path, "finding", std::slice::from_ref(&failure)).unwrap();
        let (_, receipt) = stage(&path, "withdrawal", &[failure]).unwrap();
        acknowledge(&path, "withdrawal", receipt.as_deref().unwrap()).unwrap();
        let (_, withdrawal) = stage(&path, "withdrawal", &[]).unwrap();
        edit(&path, |state| {
            for index in 0..LIMIT - 2 {
                state.consumers.insert(
                    format!("pending-{index}"),
                    Consumer {
                        pending: Some(Pending {
                            receipt: "pending".into(),
                            fingerprints: BTreeMap::new(),
                        }),
                        ..Consumer::default()
                    },
                );
            }
            Ok(())
        })
        .unwrap();
        let before = fs::read(&path).unwrap();
        assert!(stage(&path, "new", &[]).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        acknowledge(&path, "finding", finding.as_deref().unwrap()).unwrap();
        stage(&path, "new", &[]).unwrap();
        acknowledge(&path, "withdrawal", withdrawal.as_deref().unwrap()).unwrap();
    }

    #[test]
    fn acknowledgement_is_independent_and_clear_retriggers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let failure = check("failure", "Try another approach");
        let (first, token) = stage(&path, "context", std::slice::from_ref(&failure)).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(
            stage(&path, "context", std::slice::from_ref(&failure))
                .unwrap()
                .1,
            token
        );
        acknowledge(&path, "context", token.as_deref().unwrap()).unwrap();
        acknowledge(&path, "context", token.as_deref().unwrap()).unwrap();
        assert!(
            stage(&path, "context", std::slice::from_ref(&failure))
                .unwrap()
                .0
                .is_empty()
        );
        assert_eq!(
            stage(&path, "ui", std::slice::from_ref(&failure))
                .unwrap()
                .0
                .len(),
            1
        );
        let (clear, token) = stage(&path, "context", &[check("success", "Recovered")]).unwrap();
        assert_eq!(clear[0].kind, "cleared");
        acknowledge(&path, "context", token.as_deref().unwrap()).unwrap();
        assert_eq!(
            stage(&path, "context", &[failure]).unwrap().0[0].kind,
            "finding"
        );
    }
    #[test]
    fn obsolete_ack_cannot_overwrite_newer_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let (_, old) = stage(&path, "context", &[check("failure", "First")]).unwrap();
        let (_, new) = stage(&path, "context", &[check("failure", "Second")]).unwrap();
        assert!(acknowledge(&path, "context", old.as_deref().unwrap()).is_err());
        acknowledge(&path, "context", new.as_deref().unwrap()).unwrap();
        let (updates, _) = stage(&path, "context", &[]).unwrap();
        assert_eq!(updates[0].kind, "invalidated");
    }
    #[test]
    fn activity_and_assessment_time_do_not_repeat_context() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let mut failure = check("failure", "Same advice");
        let (_, token) = stage(&path, "context", std::slice::from_ref(&failure)).unwrap();
        acknowledge(&path, "context", token.as_deref().unwrap()).unwrap();
        failure.assessed_at_epoch = Some(2);
        failure.checking = true;
        assert!(stage(&path, "context", &[failure]).unwrap().0.is_empty());
    }
    #[test]
    fn corrupt_state_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        fs::write(&path, b"not JSON").unwrap();
        assert!(stage(&path, "context", &[check("failure", "Advice")]).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"not JSON");
    }

    #[test]
    fn state_contains_fingerprints_without_authored_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        stage(
            &path,
            "context",
            &[check("failure", "Private authored advice")],
        )
        .unwrap();
        let bytes = fs::read_to_string(path).unwrap();
        assert!(!bytes.contains("Private authored advice"));
        assert!(!bytes.contains("Retry"));
    }
    #[test]
    fn a_lost_acknowledgement_still_withdraws_exposed_advice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let (_, old) = stage(&path, "context", &[check("failure", "Advice")]).unwrap();
        let (clear, _) =
            stage(&path, "context", &[check("success", "No longer reported")]).unwrap();
        assert_eq!(clear[0].kind, "cleared");
        let (replayed, token) =
            stage(&path, "context", &[check("success", "No longer reported")]).unwrap();
        assert_eq!(replayed[0].kind, "cleared");
        assert!(acknowledge(&path, "context", old.as_deref().unwrap()).is_err());
        acknowledge(&path, "context", token.as_deref().unwrap()).unwrap();
        assert!(stage(&path, "context", &[]).unwrap().0.is_empty());
    }
    #[test]
    fn ineligible_failures_never_stage_advice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let mut failure = check("failure", "Obsolete advice");
        failure.eligibility = "unsupported".into();
        let (updates, receipt) = stage(&path, "context", &[failure]).unwrap();
        assert!(updates.is_empty());
        assert!(receipt.is_none());
    }
    #[test]
    fn lost_finding_acknowledgements_do_not_repeat_cleared_receipts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let failure = check("failure", "Advice");
        stage(&path, "context", std::slice::from_ref(&failure)).unwrap();
        let (_, first_clear) = stage(&path, "context", &[]).unwrap();
        let first_clear = first_clear.unwrap();
        acknowledge(&path, "context", &first_clear).unwrap();
        stage(&path, "context", &[failure]).unwrap();
        let (_, second_clear) = stage(&path, "context", &[]).unwrap();
        let second_clear = second_clear.unwrap();
        assert_ne!(first_clear, second_clear);
        acknowledge(&path, "context", &first_clear).unwrap();
        assert_eq!(
            stage(&path, "context", &[]).unwrap().1,
            Some(second_clear.clone())
        );
        acknowledge(&path, "context", &second_clear).unwrap();
        assert!(stage(&path, "context", &[]).unwrap().0.is_empty());
    }

    #[test]
    fn legacy_reused_receipt_settles_matching_pending_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        stage(&path, "context", &[check("failure", "Advice")]).unwrap();
        let (_, receipt) = stage(&path, "context", &[]).unwrap();
        let receipt = receipt.unwrap();
        edit(&path, |state| {
            state.consumers.get_mut("context").unwrap().last_receipt = Some(receipt.clone());
            Ok(())
        })
        .unwrap();
        acknowledge(&path, "context", &receipt).unwrap();
        assert!(stage(&path, "context", &[]).unwrap().0.is_empty());
    }

    #[test]
    fn reverted_finding_settles_consumer_and_allows_reclamation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let first = check("failure", "First advice");
        let (_, receipt) = stage(&path, "context", std::slice::from_ref(&first)).unwrap();
        acknowledge(&path, "context", receipt.as_deref().unwrap()).unwrap();
        let (_, obsolete) = stage(&path, "context", &[check("failure", "Changed advice")]).unwrap();
        assert!(stage(&path, "context", &[first]).unwrap().0.is_empty());
        assert!(acknowledge(&path, "context", obsolete.as_deref().unwrap()).is_err());
        let state: State = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let consumer = &state.consumers["context"];
        assert!(consumer.pending.is_none());
        assert_eq!(consumer.exposed, consumer.acknowledged);
        edit(&path, |state| {
            for index in 0..LIMIT - 1 {
                state.consumers.insert(
                    format!("pending-{index}"),
                    Consumer {
                        pending: Some(Pending {
                            receipt: "pending".into(),
                            fingerprints: BTreeMap::new(),
                        }),
                        ..Consumer::default()
                    },
                );
            }
            Ok(())
        })
        .unwrap();
        stage(&path, "new", &[]).unwrap();
        let state: State = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(!state.consumers.contains_key("context"));
    }

    #[test]
    fn unchanged_polls_and_duplicate_acknowledgements_do_not_write_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        stage(&path, "empty", &[]).unwrap();
        let before = fs::read(&path).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        stage(&path, "empty", &[]).unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        let failure = check("failure", "Advice");
        let (_, receipt) = stage(&path, "context", std::slice::from_ref(&failure)).unwrap();
        let before = fs::read(&path).unwrap();
        stage(&path, "context", std::slice::from_ref(&failure)).unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        acknowledge(&path, "context", receipt.as_deref().unwrap()).unwrap();
        stage(&path, "other", &[]).unwrap();
        let before = fs::read(&path).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        stage(&path, "context", &[failure]).unwrap();
        acknowledge(&path, "context", receipt.as_deref().unwrap()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
    }
}
