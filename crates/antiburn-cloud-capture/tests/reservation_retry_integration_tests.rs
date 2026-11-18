#![cfg(all(feature = "cloud", unix))]

use antiburn_cloud_capture::*;
use std::{fs, io::Read, os::unix::fs::PermissionsExt, path::PathBuf, result::Result};

struct Reservation {
    refusal: Option<TransportError>,
    identities: Vec<CaptureIdentity>,
}

impl Transport for Reservation {
    fn reserve(&mut self, identity: &CaptureIdentity, _: u64) -> Result<String, TransportError> {
        self.identities.push(identity.clone());
        match self.refusal.take() {
            Some(error) => Err(error),
            None => Ok("reservation".into()),
        }
    }

    fn negotiate(&mut self, _: &str, _: &CaptureManifest) -> Result<UploadPlan, TransportError> {
        Err(TransportError::Rejected)
    }

    fn status(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
    ) -> Result<ServerStatus, TransportError> {
        Err(TransportError::Rejected)
    }

    fn upload_part(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
        _: &ChunkKey,
        _: &Chunk,
        _: &mut dyn Read,
    ) -> Result<(), TransportError> {
        Err(TransportError::Rejected)
    }

    fn complete(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
    ) -> Result<(), TransportError> {
        Err(TransportError::Rejected)
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    spool: Spool,
    path: PathBuf,
    candidate: Candidate,
    identity: CaptureIdentity,
    transport: Reservation,
}

impl Fixture {
    fn new(refusal: Option<TransportError>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().canonicalize().unwrap();
        let source = base.join("source");
        let project = base.join("project");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&project).unwrap();
        fs::write(
            source.join("session.jsonl"),
            format!(
                "{}\n",
                serde_json::json!({
                    "type":"user", "sessionId":"session", "cwd":project,
                    "message":{"role":"user","content":"synthetic"}
                })
            ),
        )
        .unwrap();
        let candidate = discover_claude(&source, &Limits::default())
            .unwrap()
            .remove(0)
            .in_source_scope("scope")
            .unwrap();
        let candidate = ProjectSelection::new(vec![project])
            .unwrap()
            .admit(&candidate)
            .unwrap()
            .unwrap();
        let identity = CaptureIdentity {
            capture_id: "capture".into(),
            account_id: "account".into(),
            destination_id: "personal".into(),
            source_id: candidate.source_id().into(),
        };
        let path = base.join("spool");
        Self {
            spool: Spool::open(&path, Limits::default()).unwrap(),
            path,
            candidate,
            identity,
            transport: Reservation {
                refusal,
                identities: Vec::new(),
            },
            _directory: directory,
        }
    }

    fn capture(&mut self) -> antiburn_cloud_capture::Result<QueueEntry> {
        self.spool.capture(
            self.identity.clone(),
            &self.candidate,
            MetadataSections::new(),
            &mut self.transport,
        )
    }

    fn retry(&mut self) -> antiburn_cloud_capture::Result<QueueEntry> {
        self.spool.retry_reservation(
            self.identity.clone(),
            &self.candidate,
            MetadataSections::new(),
            &mut self.transport,
        )
    }

    fn record(&self) -> PathBuf {
        self.path
            .join(&self.identity.capture_id)
            .join("record.json")
    }
}

#[test]
fn reservation_retry_keeps_the_identity_and_entry_after_a_proved_capacity_refusal() {
    let mut fixture = Fixture::new(Some(TransportError::Backpressure));
    assert!(matches!(
        fixture.capture(),
        Err(Error::Transport(TransportError::Backpressure))
    ));
    let before = fixture.spool.entries().unwrap().remove(0);
    assert_eq!(before.state, CaptureState::Preparing);
    assert!(before.reservation_id.is_none());
    let queued = fixture.retry().unwrap();
    assert_eq!(queued.state, CaptureState::Queued);
    assert_eq!(queued.identity, before.identity);
    assert_eq!(
        fixture.transport.identities,
        vec![fixture.identity.clone(), fixture.identity.clone()]
    );
    assert_eq!(fixture.spool.entries().unwrap().len(), 1);
}

#[test]
fn ordinary_capture_does_not_infer_retry_permission_from_an_old_empty_record() {
    let mut fixture = Fixture::new(Some(TransportError::Offline));
    assert!(fixture.capture().is_err());
    let record = fs::read(fixture.record()).unwrap();
    assert_eq!(fixture.capture().unwrap().state, CaptureState::Preparing);
    assert_eq!(fixture.transport.identities.len(), 1);
    assert_eq!(fs::read(fixture.record()).unwrap(), record);
}

#[test]
fn reservation_retry_refuses_reserved_and_partial_entries_without_changing_saved_bytes() {
    let mut fixture = Fixture::new(None);
    fixture.capture().unwrap();
    let record = fs::read(fixture.record()).unwrap();
    assert!(fixture.retry().is_err());
    assert_eq!(fixture.transport.identities.len(), 1);
    assert_eq!(fs::read(fixture.record()).unwrap(), record);

    let mut reserved: serde_json::Value = serde_json::from_slice(&record).unwrap();
    reserved["state"] = "preparing".into();
    reserved["manifest"] = serde_json::Value::Null;
    fs::write(fixture.record(), serde_json::to_vec(&reserved).unwrap()).unwrap();
    let before = fs::read(fixture.record()).unwrap();
    assert!(fixture.retry().is_err());
    assert_eq!(fixture.transport.identities.len(), 1);
    assert_eq!(fs::read(fixture.record()).unwrap(), before);

    let mut empty = Fixture::new(Some(TransportError::Backpressure));
    assert!(empty.capture().is_err());
    let partial = empty.record().with_file_name("chunk-transcript-0.gz");
    fs::write(&partial, b"synthetic retained bytes").unwrap();
    fs::set_permissions(&partial, fs::Permissions::from_mode(0o600)).unwrap();
    let record = fs::read(empty.record()).unwrap();
    assert!(empty.retry().is_err());
    assert_eq!(empty.transport.identities.len(), 1);
    assert_eq!(fs::read(&partial).unwrap(), b"synthetic retained bytes");
    assert_eq!(fs::read(empty.record()).unwrap(), record);
}

#[test]
fn reservation_retry_refuses_missing_quarantined_or_rebound_entries() {
    let mut fixture = Fixture::new(Some(TransportError::Backpressure));
    assert!(fixture.capture().is_err());
    let record = fs::read(fixture.record()).unwrap();
    let mut other = fixture.identity.clone();
    other.account_id = "another-account".into();
    assert!(matches!(
        fixture.spool.retry_reservation(
            other,
            &fixture.candidate,
            MetadataSections::new(),
            &mut fixture.transport,
        ),
        Err(Error::IdentityMismatch)
    ));
    let mut other_source = fixture.identity.clone();
    other_source.source_id = "scope/another-source".into();
    assert!(matches!(
        fixture.spool.retry_reservation(
            other_source,
            &fixture.candidate,
            MetadataSections::new(),
            &mut fixture.transport,
        ),
        Err(Error::IdentityMismatch)
    ));
    let quarantine = fixture.path.with_file_name("quarantine");
    fs::rename(fixture.record().parent().unwrap(), &quarantine).unwrap();
    assert!(fixture.retry().is_err());
    assert!(!fixture.record().exists());
    assert_eq!(fs::read(quarantine.join("record.json")).unwrap(), record);
    assert_eq!(fixture.transport.identities.len(), 1);
}
