use super::*;
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
        reconcile(&self.data, &self.home, SKILL.as_bytes())
    }
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
fn clean_home_gets_canonical_bytes_without_a_cli_and_relaunch_keeps_the_files() {
    let f = Fixture::new();
    assert!(!f.home.join(".claude").exists());
    assert!(!f.data.join("cli").exists());
    f.install().unwrap();
    assert_eq!(fs::read(f.skill()).unwrap(), SKILL.as_bytes());
    assert!(!f.data.join("cli").exists());
    let inode = fs::metadata(f.skill()).unwrap().ino();
    let receipt_inode = fs::metadata(f.receipt()).unwrap().ino();
    f.install().unwrap();
    assert_eq!(fs::metadata(f.skill()).unwrap().ino(), inode);
    assert_eq!(fs::metadata(f.receipt()).unwrap().ino(), receipt_inode);
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
    assert_eq!(fs::read(f.skill()).unwrap(), SKILL.as_bytes());
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
    assert_eq!(fs::read(f.skill()).unwrap(), SKILL.as_bytes());
}

#[test]
fn existing_unmanaged_skill_is_not_adopted_even_when_bytes_match() {
    let f = Fixture::new();
    fs::create_dir_all(f.skill().parent().unwrap()).unwrap();
    fs::write(f.skill(), SKILL.as_bytes()).unwrap();
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
