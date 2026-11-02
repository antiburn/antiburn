//! Pi package changes use its native package manager and inspected readback.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use super::IntegrationPhase;
use super::discovery::Profile;
use super::document;
use super::process::Runner;
use super::transaction::{self, Receipt, Result};

fn packages(bytes: &[u8]) -> Result<Vec<Value>> {
    match document::value(bytes, &["packages".into()])? {
        None => Ok(Vec::new()),
        Some(Value::Array(packages)) if packages.len() <= 128 => Ok(packages),
        _ => Err("native_package_schema_conflict"),
    }
}

fn source(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("source").and_then(Value::as_str))
}

fn matches(value: &Value, profile: &Profile, expected: &Path) -> bool {
    source(value).is_some_and(|value| {
        let path = Path::new(value);
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            profile.root.join(path)
        };
        absolute.canonicalize().ok().as_deref() == Some(expected)
    })
}

fn unrelated(bytes: &[u8]) -> Result<Value> {
    let mut value = document::value(bytes, &[])?.ok_or("native_package_schema_conflict")?;
    value
        .as_object_mut()
        .ok_or("native_package_schema_conflict")?
        .remove("packages");
    Ok(value)
}

fn inspect_change(
    before: &[u8],
    after: &[u8],
    profile: &Profile,
    owned: &Path,
    added: bool,
) -> Result<()> {
    if unrelated(before)? != unrelated(after)? {
        return Err("native_manager_unrelated_change");
    }
    let mut prior = packages(before)?;
    let mut current = packages(after)?;
    let selected = if added { &mut current } else { &mut prior };
    let matches = selected
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches(entry, profile, owned))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if matches.len() != 1 || !selected[matches[0]].is_string() {
        return Err("native_manager_readback_failed");
    }
    selected.remove(matches[0]);
    if prior != current {
        return Err("native_manager_unrelated_change");
    }
    Ok(())
}

fn command(
    runner: &impl Runner,
    host: &Path,
    env: &BTreeMap<String, String>,
    action: &str,
    source: &Path,
) -> Result<()> {
    if !source.is_absolute() {
        return Err("unsafe_package_source");
    }
    runner
        .run(
            host,
            &[
                action.into(),
                source.to_str().ok_or("unsupported_path_encoding")?.into(),
            ],
            env,
            Duration::from_secs(20),
        )
        .map_err(|_| "native_manager_requires_review")?;
    Ok(())
}

pub fn inspect(profile: &Profile, receipt_path: &Path) -> Result<IntegrationPhase> {
    let target = profile.root.join("settings.json");
    let Some(receipt) = transaction::load_receipt(receipt_path)? else {
        return Ok(IntegrationPhase::Unavailable);
    };
    if receipt.target != target {
        return Err("receipt_target_mismatch");
    }
    if receipt.removed {
        return Ok(IntegrationPhase::Disabled);
    }
    if !receipt.published || receipt.previous.is_some() {
        return Err("native_package_requires_review");
    }
    let owned = receipt.manager_source.as_ref().ok_or("state_invalid")?;
    let current = transaction::read(&target)?;
    let entries = packages(current.as_deref().unwrap_or(b"{}"))?;
    let matching = entries
        .iter()
        .filter(|entry| matches(entry, profile, owned))
        .collect::<Vec<_>>();
    Ok(if matching.len() == 1 && matching[0].is_string() {
        IntegrationPhase::ReloadRequired
    } else {
        IntegrationPhase::UserChanged
    })
}

pub fn reconcile(
    profile: &Profile,
    host: &Path,
    root: &Path,
    receipt_path: &Path,
    artifact: &str,
    env: &BTreeMap<String, String>,
    runner: &impl Runner,
) -> Result<IntegrationPhase> {
    let target = profile.root.join("settings.json");
    let current = transaction::read(&target)?;
    let before = current.as_deref().unwrap_or(b"{}");
    let source_path = root
        .join("pi")
        .canonicalize()
        .map_err(|_| "artifact_missing")?;
    if let Some(mut receipt) = transaction::load_receipt(receipt_path)? {
        if receipt.target != target {
            return Err("receipt_target_mismatch");
        }
        if receipt.removed {
            return Ok(IntegrationPhase::Disabled);
        }
        let owned = receipt.manager_source.as_ref().ok_or("state_invalid")?;
        let entries = packages(before)?;
        let matching = entries
            .iter()
            .filter(|entry| matches(entry, profile, owned))
            .collect::<Vec<_>>();
        if matching.len() != 1 || !matching[0].is_string() {
            return Ok(IntegrationPhase::UserChanged);
        }
        if !receipt.published {
            inspect_change(
                receipt.original.as_deref().unwrap_or(b"{}"),
                before,
                profile,
                owned,
                true,
            )?;
            receipt.installed = before.to_vec();
            receipt.published = true;
            transaction::save_receipt(receipt_path, &receipt)?;
        }
        if receipt.artifact != artifact || owned != &source_path {
            return Err("native_package_upgrade_requires_review");
        }
        return Ok(IntegrationPhase::ReloadRequired);
    }
    if packages(before)?
        .iter()
        .filter_map(source)
        .any(|value| value.contains("antiburn"))
    {
        return Err("existing_integration_requires_review");
    }
    let mut receipt = Receipt::prepare(
        target.clone(),
        current.clone(),
        before.to_vec(),
        Vec::new(),
        artifact.into(),
    )?;
    receipt.manager_source = Some(source_path.clone());
    transaction::save_receipt(receipt_path, &receipt)?;
    if transaction::read(&target)? != current {
        return Err("concurrent_config_change");
    }
    command(runner, host, env, "install", &source_path)?;
    let after = transaction::read(&target)?.ok_or("native_manager_readback_failed")?;
    inspect_change(before, &after, profile, &source_path, true)?;
    let listing = runner
        .run(host, &["list".into()], env, Duration::from_secs(10))
        .map_err(|_| "native_manager_readback_failed")?;
    if !listing
        .lines()
        .any(|line| line.trim() == source_path.to_string_lossy())
    {
        return Err("native_manager_readback_failed");
    }
    receipt.installed = after;
    receipt.published = true;
    transaction::save_receipt(receipt_path, &receipt)?;
    Ok(IntegrationPhase::ReloadRequired)
}

pub fn remove(
    profile: &Profile,
    host: &Path,
    receipt: &mut Receipt,
    receipt_path: &Path,
    env: &BTreeMap<String, String>,
    runner: &impl Runner,
) -> Result<()> {
    if receipt.target != profile.root.join("settings.json") {
        return Err("receipt_target_mismatch");
    }
    let source: PathBuf = receipt.manager_source.clone().ok_or("state_invalid")?;
    let before = transaction::read(&receipt.target)?.ok_or("owned_entry_removed")?;
    let entries = packages(&before)?;
    let matching = entries
        .iter()
        .filter(|entry| matches(entry, profile, &source))
        .collect::<Vec<_>>();
    if matching.is_empty() {
        inspect_change(&receipt.installed, &before, profile, &source, false)?;
        receipt.removed = true;
        return transaction::save_receipt(receipt_path, receipt);
    }
    if matching.len() != 1 || !matching[0].is_string() {
        return Err("owned_entry_changed");
    }
    if receipt.installed != before {
        // A retry uses these bytes to verify an interrupted removal.
        receipt.installed = before.clone();
        transaction::save_receipt(receipt_path, receipt)?;
    }
    if transaction::read(&receipt.target)?.as_deref() != Some(before.as_slice()) {
        return Err("concurrent_config_change");
    }
    command(runner, host, env, "remove", &source)?;
    let after = transaction::read(&receipt.target)?.ok_or("native_manager_readback_failed")?;
    inspect_change(&before, &after, profile, &source, false)?;
    receipt.removed = true;
    transaction::save_receipt(receipt_path, receipt)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::agent_integrations::Vendor;
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
            panic!("A changed package must not invoke the native manager")
        }
    }

    #[test]
    fn readback_preserves_unknown_values_and_exact_package_order() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let owned = root.join("owned");
        std::fs::create_dir(&owned).unwrap();
        let profile = Profile {
            vendor: Vendor::Pi,
            root,
        };
        let before = br#"{"packages":["npm:keep"],"unknown":{"on":true}}"#;
        let after = serde_json::to_vec(
            &serde_json::json!({"packages":["npm:keep",owned],"unknown":{"on":true}}),
        )
        .unwrap();
        assert!(inspect_change(before, &after, &profile, &owned, true).is_ok());
        assert!(inspect_change(&after, before, &profile, &owned, false).is_ok());
        let unrelated = serde_json::to_vec(
            &serde_json::json!({"packages":["npm:keep",owned],"unknown":{"on":false}}),
        )
        .unwrap();
        assert_eq!(
            inspect_change(before, &unrelated, &profile, &owned, true),
            Err("native_manager_unrelated_change")
        );
    }

    #[test]
    fn removal_refuses_user_package_filters_before_running_a_command() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let owned = root.join("owned");
        std::fs::create_dir(&owned).unwrap();
        let profile = Profile {
            vendor: Vendor::Pi,
            root: root.clone(),
        };
        let target = root.join("settings.json");
        let bytes = serde_json::to_vec(
            &serde_json::json!({"packages":[{"source": owned,"extensions":[]}]}),
        )
        .unwrap();
        std::fs::write(&target, &bytes).unwrap();
        let mut receipt = Receipt::prepare(
            target.clone(),
            Some(b"{}".to_vec()),
            bytes.clone(),
            Vec::new(),
            "fixture".into(),
        )
        .unwrap();
        receipt.manager_source = Some(owned);
        assert_eq!(
            remove(
                &profile,
                Path::new("/unused"),
                &mut receipt,
                &root.join("receipt.json"),
                &BTreeMap::new(),
                &NoCommands
            ),
            Err("owned_entry_changed")
        );
        assert_eq!(std::fs::read(target).unwrap(), bytes);
    }

    struct RemoveThenFail {
        target: PathBuf,
        after: Vec<u8>,
    }

    impl Runner for RemoveThenFail {
        fn run(
            &self,
            _: &Path,
            args: &[String],
            _: &BTreeMap<String, String>,
            _: Duration,
        ) -> std::result::Result<String, ProcessError> {
            assert_eq!(args[0], "remove");
            std::fs::write(&self.target, &self.after)
                .expect("write the interrupted Pi removal result");
            Err(ProcessError::Failed)
        }
    }

    #[test]
    fn interrupted_removal_retries_without_a_second_native_command() {
        let temp = tempfile::tempdir().expect("create the interrupted Pi removal fixture");
        let root = temp
            .path()
            .canonicalize()
            .expect("resolve the interrupted Pi removal fixture");
        let owned = root.join("owned");
        std::fs::create_dir(&owned).expect("create the interrupted-removal package directory");
        let profile = Profile {
            vendor: Vendor::Pi,
            root: root.clone(),
        };
        let target = root.join("settings.json");
        let installed = serde_json::to_vec(&serde_json::json!({
            "packages": ["npm:keep", owned], "unknown": 1,
        }))
        .expect("encode the installed Pi package fixture");
        let mut receipt = Receipt::prepare(
            target.clone(),
            Some(br#"{"packages":["npm:keep"],"unknown":1}"#.to_vec()),
            installed,
            Vec::new(),
            "fixture".into(),
        )
        .expect("prepare the interrupted Pi removal receipt");
        receipt.manager_source = Some(owned.clone());
        receipt.published = true;
        let receipt_path = root.join("receipt.json");
        transaction::save_receipt(&receipt_path, &receipt)
            .expect("save the interrupted Pi removal receipt");
        let current = serde_json::to_vec(&serde_json::json!({
            "packages": ["npm:keep", owned], "unknown": 2,
        }))
        .expect("encode the user-changed Pi config fixture");
        std::fs::write(&target, &current).expect("write the user-changed Pi config fixture");
        let after = br#"{"packages":["npm:keep"],"unknown":2}"#.to_vec();
        assert_eq!(
            remove(
                &profile,
                Path::new("/unused"),
                &mut receipt,
                &receipt_path,
                &BTreeMap::new(),
                &RemoveThenFail {
                    target: target.clone(),
                    after: after.clone()
                },
            ),
            Err("native_manager_requires_review")
        );
        let mut pending = transaction::load_receipt(&receipt_path)
            .expect("load the interrupted Pi removal receipt")
            .expect("find the pending Pi removal receipt");
        assert!(!pending.removed);
        assert_eq!(pending.installed, current);
        remove(
            &profile,
            Path::new("/unused"),
            &mut pending,
            &receipt_path,
            &BTreeMap::new(),
            &NoCommands,
        )
        .expect("finish the interrupted Pi removal without another command");
        assert_eq!(
            std::fs::read(target).expect("read the recovered Pi config"),
            after
        );
        assert!(
            transaction::load_receipt(&receipt_path)
                .expect("load the completed Pi removal receipt")
                .expect("find the completed Pi removal receipt")
                .removed
        );
        assert_eq!(
            inspect(&profile, &receipt_path).expect("inspect the completed Pi removal"),
            IntegrationPhase::Disabled
        );
    }

    #[test]
    fn absent_package_recovery_refuses_changed_config_and_package_entries() {
        let temp = tempfile::tempdir().expect("create the changed Pi recovery fixture");
        let root = temp
            .path()
            .canonicalize()
            .expect("resolve the changed Pi recovery fixture");
        let owned = root.join("owned");
        std::fs::create_dir(&owned).expect("create the changed-recovery package directory");
        let profile = Profile {
            vendor: Vendor::Pi,
            root: root.clone(),
        };
        let target = root.join("settings.json");
        let installed = serde_json::to_vec(&serde_json::json!({
            "packages": ["npm:keep", owned], "unknown": 1,
        }))
        .expect("encode the pre-removal Pi config fixture");
        let mut receipt = Receipt::prepare(
            target.clone(),
            Some(b"{}".to_vec()),
            installed,
            Vec::new(),
            "fixture".into(),
        )
        .expect("prepare the changed Pi recovery receipt");
        receipt.manager_source = Some(owned.clone());
        receipt.published = true;
        let receipt_path = root.join("receipt.json");
        transaction::save_receipt(&receipt_path, &receipt)
            .expect("save the changed Pi recovery receipt");
        let saved =
            std::fs::read(&receipt_path).expect("read the receipt before changed-config recovery");
        for changed in [
            serde_json::json!({"packages":["npm:keep"], "unknown": 2}),
            serde_json::json!({"packages":["npm:different"], "unknown": 1}),
            serde_json::json!({"packages":["npm:keep", {"source": owned, "extensions":[]}], "unknown":1}),
            serde_json::json!({"packages":["npm:keep", "/user/package"], "unknown":1}),
        ] {
            let bytes =
                serde_json::to_vec(&changed).expect("encode the unsafe Pi recovery config fixture");
            std::fs::write(&target, &bytes).expect("write the unsafe Pi recovery config fixture");
            assert!(
                remove(
                    &profile,
                    Path::new("/unused"),
                    &mut receipt,
                    &receipt_path,
                    &BTreeMap::new(),
                    &NoCommands,
                )
                .is_err()
            );
            assert_eq!(
                std::fs::read(&target).expect("read the unchanged unsafe Pi recovery config"),
                bytes
            );
            assert_eq!(
                std::fs::read(&receipt_path).expect("read the receipt after refused Pi recovery"),
                saved
            );
            assert!(!receipt.removed);
        }
    }
}
