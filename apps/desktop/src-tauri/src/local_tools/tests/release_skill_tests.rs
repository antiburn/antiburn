use super::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, symlink};

struct Fixture {
    _root: tempfile::TempDir,
    home: PathBuf,
    data: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().canonicalize().unwrap();
        let data = home.join(RELEASE_IDENTIFIER);
        fs::DirBuilder::new().mode(0o700).create(&data).unwrap();
        Self {
            _root: root,
            home,
            data,
        }
    }

    fn skill(&self) -> PathBuf {
        self.home.join(".claude/skills/antiburn/SKILL.md")
    }

    fn receipt(&self) -> PathBuf {
        self.data.join("skill-install/receipt.json")
    }

    fn install(&self) -> Result<(), Reason> {
        reconcile_bound(&self.data, &self.home, &self.runtime())
    }

    fn runtime(&self) -> CliRuntime {
        CliRuntime {
            executable: self
                .data
                .join("cli/bin")
                .join(antiburn_cli_runtime::file_name()),
            version: "0.1.0".into(),
            sha256: "synthetic".into(),
            delivery_protocols: vec!["delivery-v1".into()],
        }
    }

    fn proposed(&self) -> Vec<u8> {
        render_skill(SKILL, &self.runtime().executable, None).unwrap()
    }

    /// Stop with a durable stage and pending receipt, before the skill is visible.
    fn stage_pending_first(&self, proposed: &[u8]) -> (PathBuf, FirstPublication) {
        ensure_directory(self.receipt().parent().unwrap()).unwrap();
        fs::create_dir_all(self.skill().parent().unwrap()).unwrap();
        let (stage, proof) = stage_first(&self.skill(), proposed).unwrap();
        let receipt = Receipt {
            owner: OWNER.into(),
            profile: self.home.join(".claude"),
            skill: FileReceipt {
                active: None,
                pending: Some(proposed.to_vec()),
            },
            first_publication: Some(proof),
        };
        save(&self.receipt(), &receipt, &mut None).unwrap();
        (stage, receipt.first_publication.unwrap())
    }

    /// Stop after the rename, before the release writes the active receipt.
    fn publish_pending_first(&self, proposed: &[u8], race: impl FnOnce()) -> Result<(), Reason> {
        let (stage, proof) = self.stage_pending_first(proposed);
        race();
        publish_first(&stage, &self.skill(), proposed, &proof).map(|_| ())
    }
}

#[test]
fn initial_receipt_publish_leaves_one_link_and_never_replaces_a_collision() {
    let f = Fixture::new();
    ensure_directory(f.receipt().parent().unwrap()).unwrap();
    let receipt = Receipt {
        owner: OWNER.into(),
        profile: f.home.join(".claude"),
        skill: FileReceipt {
            active: None,
            pending: Some(f.proposed()),
        },
        first_publication: None,
    };
    let mut previous = None;
    save(&f.receipt(), &receipt, &mut previous).unwrap();
    let metadata = fs::metadata(f.receipt()).unwrap();
    assert_eq!(metadata.nlink(), 1);
    assert_eq!(read_optional(&f.receipt()).unwrap(), previous);
    let mut missing = None;
    assert_eq!(
        save(&f.receipt(), &receipt, &mut missing),
        Err(Reason::SetupUnavailable)
    );
    assert!(missing.is_none());
    assert_eq!(read_optional(&f.receipt()).unwrap(), previous);
    assert_eq!(fs::metadata(f.receipt()).unwrap().ino(), metadata.ino());
}

#[test]
fn retry_publishes_the_recorded_stage_without_replacing_its_inode() {
    let f = Fixture::new();
    let proposed = f.proposed();
    let (stage, _) = f.stage_pending_first(&proposed);
    let inode = fs::metadata(&stage).unwrap().ino();
    assert!(!f.skill().exists());
    f.install().unwrap();
    assert!(!stage.exists());
    assert_eq!(fs::metadata(f.skill()).unwrap().ino(), inode);
    assert_eq!(fs::read(f.skill()).unwrap(), proposed);
}

#[test]
fn retry_does_not_recreate_a_skill_removed_before_finalizing_the_receipt() {
    let f = Fixture::new();
    f.publish_pending_first(&f.proposed(), || {}).unwrap();
    let receipt = fs::read(f.receipt()).unwrap();
    fs::remove_file(f.skill()).unwrap();
    assert_eq!(f.install(), Err(Reason::OwnedFileChanged));
    assert!(!f.skill().exists());
    assert_eq!(fs::read(f.receipt()).unwrap(), receipt);
}

#[test]
fn legacy_publication_proof_can_finalize_an_installed_skill_but_not_restore_it() {
    for removed in [false, true] {
        let f = Fixture::new();
        f.publish_pending_first(&f.proposed(), || {}).unwrap();
        let path = f.receipt();
        let mut saved = read_optional(&path).unwrap();
        let mut receipt: Receipt = serde_json::from_slice(saved.as_deref().unwrap()).unwrap();
        receipt.first_publication.as_mut().unwrap().stage = None;
        save(&path, &receipt, &mut saved).unwrap();
        if removed {
            fs::remove_file(f.skill()).unwrap();
            assert_eq!(f.install(), Err(Reason::OwnedFileChanged));
            assert!(!f.skill().exists());
        } else {
            f.install().unwrap();
            assert_eq!(fs::read(f.skill()).unwrap(), f.proposed());
        }
    }
}

#[test]
fn retry_does_not_publish_a_replaced_or_redirected_stage() {
    for redirect in [false, true] {
        let f = Fixture::new();
        let (stage, _) = f.stage_pending_first(&f.proposed());
        let replacement = f.home.join("other");
        fs::copy(&stage, &replacement).unwrap();
        fs::remove_file(&stage).unwrap();
        let expected = if redirect {
            symlink(&replacement, &stage).unwrap();
            Reason::OwnedFileChanged
        } else {
            fs::rename(&replacement, &stage).unwrap();
            Reason::OwnershipConflict
        };
        assert_eq!(f.install(), Err(expected));
        assert!(!f.skill().exists());
        assert!(stage.exists());
    }
}

#[test]
fn inherited_file_acl_blocks_publication_until_corrected() {
    let f = Fixture::new();
    let parent = f.home.join(".claude/skills");
    fs::create_dir_all(&parent).unwrap();
    let entry =
        "everyone allow read,readattr,readextattr,readsecurity,file_inherit,directory_inherit";
    let change_acl = |operation| {
        assert!(
            std::process::Command::new("/bin/chmod")
                .args([operation, entry])
                .arg(&parent)
                .status()
                .unwrap()
                .success()
        );
    };
    change_acl("+a");
    let result = f.install();
    change_acl("-a");
    assert_eq!(result, Err(Reason::SetupUnavailable));
    assert!(!f.skill().exists());
    assert!(
        fs::read_dir(f.skill().parent().unwrap())
            .unwrap()
            .next()
            .is_none()
    );
    let receipt: Receipt = serde_json::from_slice(&fs::read(f.receipt()).unwrap()).unwrap();
    assert!(receipt.first_publication.is_none());
    assert!(
        std::process::Command::new("/bin/chmod")
            .arg("-N")
            .arg(f.skill().parent().unwrap())
            .status()
            .unwrap()
            .success()
    );
    f.install().unwrap();
    assert_eq!(fs::read(f.skill()).unwrap(), f.proposed());
}

#[test]
fn only_release_distribution_launches_install_the_skill() {
    assert!(automatic_installation(true, false, RELEASE_IDENTIFIER));
    assert!(!automatic_installation(false, false, RELEASE_IDENTIFIER));
    assert!(!automatic_installation(true, true, RELEASE_IDENTIFIER));
    assert!(!automatic_installation(
        true,
        false,
        super::super::DEBUG_IDENTIFIER
    ));
}

#[test]
fn successful_startup_and_repair_reconcile_the_skill_under_the_runtime_lock() {
    let f = Fixture::new();
    let payload = f.home.join("cli-payload");
    ensure_directory(&payload).unwrap();
    let cli = antiburn_cli_runtime::Installation {
        root: f.data.join("cli"),
        terminal: f.home.join(".local/bin/antiburn-cli"),
        payload,
        target: antiburn_cli_runtime::TARGET.into(),
    };
    assert!(!f.home.join(".claude").exists());
    assert!(cli.reconcile_for_binding(true).is_err());
    assert!(!f.skill().exists());
    assert!(!f.receipt().exists());

    let bytes = b"synthetic CLI payload";
    let name = antiburn_cli_runtime::file_name();
    fs::write(cli.payload.join(name), bytes).unwrap();
    let manifest = antiburn_cli_runtime::Manifest {
        schema: 1,
        target: cli.target.clone(),
        executable: name.into(),
        version: "0.1.0".into(),
        sha256: Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        delivery_protocols: vec!["delivery-v1".into()],
    };
    fs::write(
        cli.payload.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let (lock, runtime) = cli.reconcile_for_binding(true).unwrap();
    assert_eq!(runtime.executable, f.runtime().executable);
    assert!(matches!(
        cli.reconcile_for_binding(false)
            .unwrap_err()
            .downcast_ref::<fs::TryLockError>(),
        Some(fs::TryLockError::WouldBlock)
    ));
    reconcile_bound(&f.data, &f.home, &runtime).unwrap();
    drop(lock);
    assert_eq!(fs::read(f.skill()).unwrap(), f.proposed());
    assert!(!f.home.join(".local").exists());
    let inode = fs::metadata(f.skill()).unwrap().ino();
    let receipt_inode = fs::metadata(f.receipt()).unwrap().ino();
    fs::remove_file(&runtime.executable).unwrap();
    let (_lock, repaired) = cli.reconcile_for_binding(false).unwrap();
    reconcile_bound(&f.data, &f.home, &repaired).unwrap();
    assert!(repaired.executable.is_file());
    assert_eq!(fs::metadata(f.skill()).unwrap().ino(), inode);
    assert_eq!(fs::metadata(f.receipt()).unwrap().ino(), receipt_inode);
}

#[test]
fn refuses_release_skill_binding_with_a_different_runtime_path() {
    let f = Fixture::new();
    let mut runtime = f.runtime();
    runtime.executable = f.home.join("antiburn");
    assert_eq!(
        reconcile_bound(&f.data, &f.home, &runtime),
        Err(Reason::SetupUnavailable)
    );
    assert!(!f.skill().exists());
    assert!(!f.receipt().exists());
}

#[test]
fn upgrade_replaces_only_receipt_owned_bytes() {
    let f = Fixture::new();
    let profile = f.home.join(".claude");
    ensure_directory(&profile).unwrap();
    ensure_directory(&profile.join("skills")).unwrap();
    let settings = profile.join("settings.json");
    fs::write(&settings, b"{}").unwrap();
    reconcile(&f.data, &f.home, b"previous canonical skill").unwrap();
    assert_eq!(fs::read(f.skill()).unwrap(), b"previous canonical skill");
    f.install().unwrap();
    assert_eq!(fs::read(f.skill()).unwrap(), f.proposed());
    assert_eq!(fs::read(settings).unwrap(), b"{}");

    fs::write(f.skill(), b"user edit").unwrap();
    let receipt = fs::read(f.receipt()).unwrap();
    assert_eq!(f.install(), Err(Reason::OwnedFileChanged));
    assert_eq!(fs::read(f.skill()).unwrap(), b"user edit");
    assert_eq!(fs::read(f.receipt()).unwrap(), receipt);
    fs::remove_file(f.skill()).unwrap();
    assert_eq!(f.install(), Err(Reason::OwnedFileChanged));
    assert!(!f.skill().exists());
}

#[test]
fn interrupted_publication_accepts_the_recorded_pending_bytes() {
    let f = Fixture::new();
    reconcile(&f.data, &f.home, b"previous canonical skill").unwrap();
    let receipt_path = f.receipt();
    let mut saved = read_optional(&receipt_path).unwrap();
    let mut receipt: Receipt = serde_json::from_slice(saved.as_deref().unwrap()).unwrap();
    receipt.skill.pending = Some(b"interrupted canonical skill".to_vec());
    save(&receipt_path, &receipt, &mut saved).unwrap();
    fs::write(f.skill(), b"interrupted canonical skill").unwrap();
    f.install().unwrap();
    assert_eq!(fs::read(f.skill()).unwrap(), f.proposed());
}

#[test]
fn first_publication_resumes_without_replacing_the_staged_file() {
    let f = Fixture::new();
    let proposed = f.proposed();
    f.publish_pending_first(&proposed, || {}).unwrap();
    let inode = fs::metadata(f.skill()).unwrap().ino();
    f.install().unwrap();
    assert_eq!(fs::metadata(f.skill()).unwrap().ino(), inode);
    let receipt: Receipt = serde_json::from_slice(&fs::read(f.receipt()).unwrap()).unwrap();
    assert_eq!(receipt.skill.active, Some(proposed));
    assert!(receipt.skill.pending.is_none());
    assert!(receipt.first_publication.is_none());
}

#[test]
fn first_publication_can_resume_on_a_new_release() {
    let f = Fixture::new();
    f.publish_pending_first(b"previous canonical skill", || {})
        .unwrap();
    f.install().unwrap();
    assert_eq!(fs::read(f.skill()).unwrap(), f.proposed());
}

#[test]
fn a_concurrent_matching_creation_does_not_gain_ownership() {
    let f = Fixture::new();
    let failure = f.publish_pending_first(SKILL.as_bytes(), || {
        fs::write(f.skill(), SKILL).unwrap();
    });
    assert_eq!(failure, Err(Reason::SetupUnavailable));
    let inode = fs::metadata(f.skill()).unwrap().ino();
    let receipt = fs::read(f.receipt()).unwrap();
    assert_eq!(f.install(), Err(Reason::OwnershipConflict));
    assert_eq!(fs::metadata(f.skill()).unwrap().ino(), inode);
    assert_eq!(fs::read(f.receipt()).unwrap(), receipt);
}

#[test]
fn a_matching_replacement_does_not_gain_ownership() {
    let f = Fixture::new();
    f.publish_pending_first(SKILL.as_bytes(), || {}).unwrap();
    let replacement = f.skill().with_extension("replacement");
    fs::copy(f.skill(), &replacement).unwrap();
    fs::rename(replacement, f.skill()).unwrap();
    let receipt = fs::read(f.receipt()).unwrap();
    assert_eq!(f.install(), Err(Reason::OwnershipConflict));
    assert_eq!(fs::read(f.skill()).unwrap(), SKILL.as_bytes());
    assert_eq!(fs::read(f.receipt()).unwrap(), receipt);
}

#[test]
fn a_changed_pending_first_file_is_not_replaced() {
    let f = Fixture::new();
    f.publish_pending_first(SKILL.as_bytes(), || {}).unwrap();
    fs::write(f.skill(), b"user edit").unwrap();
    assert_eq!(f.install(), Err(Reason::OwnershipConflict));
    assert_eq!(fs::read(f.skill()).unwrap(), b"user edit");
}

#[test]
fn existing_unmanaged_skill_is_not_adopted_even_when_bytes_match() {
    let f = Fixture::new();
    fs::create_dir_all(f.skill().parent().unwrap()).unwrap();
    fs::write(f.skill(), f.proposed()).unwrap();
    let inode = fs::metadata(f.skill()).unwrap().ino();
    assert_eq!(f.install(), Err(Reason::OwnershipConflict));
    assert_eq!(fs::metadata(f.skill()).unwrap().ino(), inode);
    assert!(!f.receipt().exists());
}

#[test]
fn pending_first_install_does_not_claim_a_matching_file() {
    let f = Fixture::new();
    let profile = f.home.join(".claude");
    ensure_directory(f.receipt().parent().unwrap()).unwrap();
    save(
        &f.receipt(),
        &Receipt {
            owner: OWNER.into(),
            profile,
            skill: FileReceipt {
                active: None,
                pending: Some(SKILL.as_bytes().to_vec()),
            },
            first_publication: None,
        },
        &mut None,
    )
    .unwrap();
    fs::create_dir_all(f.skill().parent().unwrap()).unwrap();
    fs::write(f.skill(), SKILL).unwrap();
    assert_eq!(f.install(), Err(Reason::OwnershipConflict));
    assert_eq!(fs::read(f.skill()).unwrap(), SKILL.as_bytes());
}

#[test]
fn symlinked_profile_paths_and_managed_skill_are_left_untouched() {
    for relative in [".claude", ".claude/skills", ".claude/skills/antiburn"] {
        let f = Fixture::new();
        let target = f.home.join(relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let outside = f.home.join("other");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, &target).unwrap();
        assert_eq!(f.install(), Err(Reason::ProfileUnavailable), "{relative}");
        assert_eq!(fs::read_link(target).unwrap(), outside);
        assert!(fs::read_dir(outside).unwrap().next().is_none());
    }

    let f = Fixture::new();
    f.install().unwrap();
    let outside = f.home.join("other");
    fs::write(&outside, SKILL.as_bytes()).unwrap();
    fs::remove_file(f.skill()).unwrap();
    symlink(&outside, f.skill()).unwrap();
    assert_eq!(f.install(), Err(Reason::OwnedFileChanged));
    assert_eq!(fs::read_link(f.skill()).unwrap(), outside);
    assert_eq!(fs::read(outside).unwrap(), SKILL.as_bytes());
}
