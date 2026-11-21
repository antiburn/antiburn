use super::*;

#[test]
fn alternating_provisional_receipts_yield_without_an_unbounded_resume_loop() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let id = identity("main", "alternating-receipts");
    let mut transport = FakeTransport {
        keep_only_latest_receipt: true,
        ..FakeTransport::default()
    };
    queue
        .capture(
            id.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    let entry = queue.resume(&id, &mut transport).unwrap();
    assert_eq!(entry.state, CaptureState::Queued);
    assert_eq!(
        transport.uploaded.len(),
        entry.upload_plan.as_ref().unwrap().chunks.len() + 1
    );
    assert_eq!(transport.complete_calls, 0);
    assert_eq!(transport.reserve_calls, 1);
}

#[test]
fn missing_confirmation_after_a_successful_part_is_transient_and_keeps_bytes() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let id = identity("main", "missing-confirmation");
    let mut transport = FakeTransport {
        lose_latest_receipt: true,
        ..FakeTransport::default()
    };
    let saved = queue
        .capture(
            id.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    let directory = temp.path().join("spool/missing-confirmation");
    let before = saved_chunks(&directory, saved.manifest.as_ref().unwrap());
    assert!(matches!(
        queue.resume_step(&id, &mut transport),
        Err(Error::Transport(TransportError::Offline))
    ));
    assert_eq!(
        saved_chunks(&directory, saved.manifest.as_ref().unwrap()),
        before
    );
    assert_eq!(queue.entries().unwrap()[0].identity, id);
    assert_eq!(transport.reserve_calls, 1);
}

#[test]
fn expired_parts_refill_the_same_capture_after_finalization_and_restart() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let id = identity("main", "expired-parts");
    let mut transport = FakeTransport::default();
    queue
        .capture(
            id.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    let awaiting = queue.resume(&id, &mut transport).unwrap();
    assert_eq!(awaiting.state, CaptureState::AwaitingIntegrity);
    assert!(awaiting.completion_submitted);
    let directory = temp.path().join("spool/expired-parts");
    let before = saved_chunks(&directory, awaiting.manifest.as_ref().unwrap());
    transport.confirmed.clear();
    drop(queue);
    let mut queue = spool(&temp);
    let step = queue.resume_step(&id, &mut transport).unwrap();
    assert_eq!(step.entry.identity, id);
    assert_eq!(step.entry.state, CaptureState::Queued);
    assert!(!step.entry.completion_submitted);
    assert_eq!(step.entry.manifest, awaiting.manifest);
    assert_eq!(step.entry.upload_plan, awaiting.upload_plan);
    assert_eq!(
        saved_chunks(&directory, step.entry.manifest.as_ref().unwrap()),
        before
    );
    assert_eq!(
        queue.resume(&id, &mut transport).unwrap().state,
        CaptureState::AwaitingIntegrity
    );
    assert_eq!(transport.complete_calls, 2);
    assert_eq!(transport.reserve_calls, 1);
    assert_eq!(transport.negotiate_calls, 1);
    transport.accepted = true;
    assert_eq!(
        queue.resume_step(&id, &mut transport).unwrap().entry.state,
        CaptureState::Accepted
    );
}
use std::os::unix::fs::MetadataExt;

#[test]
fn saved_plan_survives_restart_without_repeating_negotiation() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let id = identity("main", "saved-plan");
    let mut transport = FakeTransport::default();
    queue
        .capture(
            id.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    let first = queue.resume_step(&id, &mut transport).unwrap();
    let plan = first.entry.upload_plan.unwrap();
    assert_eq!(transport.negotiate_calls, 1);
    assert_eq!(transport.status_calls, 2);
    assert_eq!(transport.uploaded.len(), 1);
    drop(queue);

    let mut queue = spool(&temp);
    transport.bad_hash = true;
    transport.reuse_stream = Some("capture-metadata".into());
    let second = queue.resume_step(&id, &mut transport).unwrap();
    assert_eq!(second.entry.upload_plan.as_ref(), Some(&plan));
    assert_eq!(transport.negotiate_calls, 1);
    assert_eq!(transport.status_calls, 4);
    assert_eq!(transport.uploaded.len(), 2);
    assert_eq!(transport.complete_calls, 0);

    transport.accept_on_complete = true;
    let accepted = queue.resume_step(&id, &mut transport).unwrap();
    assert_eq!(accepted.entry.state, CaptureState::Accepted);
    assert_eq!(accepted.entry.upload_plan.as_ref(), Some(&plan));
    assert_eq!(transport.negotiate_calls, 1);
    assert_eq!(transport.status_calls, 6);
    assert_eq!(transport.complete_calls, 1);
}

#[test]
fn lost_negotiation_response_retries_only_until_the_plan_is_durable() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let id = identity("main", "lost-negotiation");
    let mut transport = FakeTransport {
        fail_negotiate_after_registration: true,
        ..FakeTransport::default()
    };
    let entry = queue
        .capture(
            id.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    let directory = temp.path().join("spool/lost-negotiation");
    let manifest = entry.manifest.unwrap();
    let chunks = saved_chunks(&directory, &manifest);
    assert!(matches!(
        queue.resume_step(&id, &mut transport),
        Err(Error::Transport(TransportError::Offline))
    ));
    assert_eq!(transport.manifest.as_ref(), Some(&manifest));
    assert_eq!(transport.negotiate_calls, 1);
    assert_eq!(transport.status_calls, 0);
    assert!(queue.entries().unwrap()[0].upload_plan.is_none());
    drop(queue);

    let mut queue = spool(&temp);
    queue.resume_step(&id, &mut transport).unwrap();
    assert_eq!(transport.negotiate_calls, 2);
    assert!(queue.entries().unwrap()[0].upload_plan.is_some());
    drop(queue);

    let mut queue = spool(&temp);
    queue.resume_step(&id, &mut transport).unwrap();
    assert_eq!(transport.negotiate_calls, 2);
    assert_eq!(transport.status_calls, 4);
    assert_eq!(transport.reserve_calls, 1);
    assert_eq!(saved_chunks(&directory, &manifest), chunks);
}

#[test]
fn failed_plan_save_blocks_status_and_upload_until_negotiation_is_durable() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let id = identity("main", "plan-save-failure");
    let mut transport = FakeTransport::default();
    let saved = queue
        .capture(
            id.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    let directory = temp.path().join("spool/plan-save-failure");
    let chunks = saved_chunks(&directory, saved.manifest.as_ref().unwrap());
    let next = directory.join("record.next");
    fs::create_dir(&next).unwrap();

    assert!(matches!(
        queue.resume_step(&id, &mut transport),
        Err(Error::Io(_))
    ));
    assert_eq!(transport.negotiate_calls, 1);
    assert_eq!(transport.status_calls, 0);
    assert!(transport.uploaded.is_empty());
    assert_eq!(queue.entries().unwrap(), vec![saved.clone()]);
    assert_eq!(
        saved_chunks(&directory, saved.manifest.as_ref().unwrap()),
        chunks
    );
    fs::remove_dir(next).unwrap();
    drop(queue);

    let mut queue = spool(&temp);
    queue.resume_step(&id, &mut transport).unwrap();
    assert_eq!(transport.negotiate_calls, 2);
    assert_eq!(transport.reserve_calls, 1);
    assert!(queue.entries().unwrap()[0].upload_plan.is_some());
}

#[test]
fn saved_plan_rejects_changed_status_bindings_and_receipts_without_writes() {
    for failure in 0..5 {
        let temp = temporary();
        let mut queue = spool(&temp);
        let id = identity("main", "status-binding");
        let mut transport = FakeTransport::default();
        queue
            .capture(
                id.clone(),
                &candidate("main"),
                BTreeMap::new(),
                &mut transport,
            )
            .unwrap();
        let saved = queue.resume_step(&id, &mut transport).unwrap().entry;
        let directory = temp.path().join("spool/status-binding");
        let record = fs::File::open(directory.join("record.json")).unwrap();
        let inode = record.metadata().unwrap().ino();
        let chunks = saved_chunks(&directory, saved.manifest.as_ref().unwrap());
        match failure {
            0 => transport.bad_identity = true,
            1 => transport.bad_manifest_digest = true,
            2 => transport.change_plan = true,
            3 => transport.bad_receipt_hash = true,
            4 => transport.duplicate_receipt = true,
            _ => unreachable!(),
        }
        assert!(matches!(
            queue.resume_step(&id, &mut transport),
            Err(Error::InvalidServerStatus)
        ));
        assert_eq!(transport.negotiate_calls, 1);
        assert_eq!(transport.status_calls, 3);
        assert_eq!(transport.uploaded.len(), 1);
        assert_eq!(transport.complete_calls, 0);
        assert_eq!(queue.entries().unwrap(), vec![saved.clone()]);
        assert_eq!(
            fs::metadata(directory.join("record.json")).unwrap().ino(),
            inode
        );
        assert_eq!(
            saved_chunks(&directory, saved.manifest.as_ref().unwrap()),
            chunks
        );
    }
}

#[test]
fn invalid_saved_plan_blocks_network_and_keeps_saved_chunks() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let id = identity("main", "invalid-saved-plan");
    let mut transport = FakeTransport::default();
    queue
        .capture(
            id.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    let saved = queue.resume_step(&id, &mut transport).unwrap().entry;
    let directory = temp.path().join("spool/invalid-saved-plan");
    let chunks = saved_chunks(&directory, saved.manifest.as_ref().unwrap());
    let path = directory.join("record.json");
    let mut record = serde_json::to_value(&saved).unwrap();
    record["upload_plan"]["manifest_sha256"] = "0".repeat(64).into();
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(&path, &bytes).unwrap();

    assert!(matches!(
        queue.resume_step(&id, &mut transport),
        Err(Error::CorruptSpool)
    ));
    assert_eq!(transport.negotiate_calls, 1);
    assert_eq!(transport.status_calls, 2);
    assert_eq!(transport.uploaded.len(), 1);
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert_eq!(
        saved_chunks(&directory, saved.manifest.as_ref().unwrap()),
        chunks
    );
}

#[test]
fn unchanged_integrity_polls_keep_the_record_until_acceptance_is_durable() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let id = identity("main", "unchanged-status");
    let mut transport = FakeTransport {
        reuse_all: true,
        ..FakeTransport::default()
    };
    queue
        .capture(
            id.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    let pending = queue.resume_step(&id, &mut transport).unwrap().entry;
    assert_eq!(pending.state, CaptureState::AwaitingIntegrity);
    assert!(pending.completion_submitted);
    let directory = temp.path().join("spool/unchanged-status");
    let path = directory.join("record.json");
    let record = fs::File::open(&path).unwrap();
    let inode = record.metadata().unwrap().ino();
    let bytes = fs::read(&path).unwrap();
    let chunks = saved_chunks(&directory, pending.manifest.as_ref().unwrap());

    for _ in 0..3 {
        assert_eq!(
            queue.resume_step(&id, &mut transport).unwrap().entry,
            pending
        );
        assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    }
    drop(queue);
    let mut queue = spool(&temp);
    assert_eq!(
        queue.resume_step(&id, &mut transport).unwrap().entry,
        pending
    );
    assert_eq!(transport.negotiate_calls, 1);
    assert_eq!(transport.status_calls, 6);
    assert_eq!(transport.complete_calls, 1);
    assert!(transport.uploaded.is_empty());
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(
        saved_chunks(&directory, pending.manifest.as_ref().unwrap()),
        chunks
    );

    transport.accepted = true;
    let accepted = queue.resume_step(&id, &mut transport).unwrap().entry;
    assert_eq!(accepted.state, CaptureState::Accepted);
    assert_ne!(fs::metadata(&path).unwrap().ino(), inode);
    let durable: QueueEntry = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(durable, accepted);
    assert!(accepted.server_status.as_ref().unwrap().integrity_accepted);
    for key in chunks.keys() {
        assert!(
            !directory
                .join(format!("chunk-{}-{:05}.gz", key.stream_id, key.index))
                .exists()
        );
    }
    assert!(matches!(queue.cancel(&id), Err(Error::AlreadyAccepted)));
    drop(queue);
    let mut queue = spool(&temp);
    assert_eq!(queue.entries().unwrap(), vec![accepted]);
    queue.acknowledge_accepted(&id).unwrap();
    assert!(!directory.exists());
}

#[test]
fn failed_acceptance_save_keeps_pending_state_and_every_saved_chunk() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let id = identity("main", "acceptance-save-failure");
    let mut transport = FakeTransport {
        reuse_all: true,
        ..FakeTransport::default()
    };
    queue
        .capture(
            id.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    let pending = queue.resume_step(&id, &mut transport).unwrap().entry;
    let directory = temp.path().join("spool/acceptance-save-failure");
    let chunks = saved_chunks(&directory, pending.manifest.as_ref().unwrap());
    let path = directory.join("record.json");
    let bytes = fs::read(&path).unwrap();
    let next = directory.join("record.next");
    fs::create_dir(&next).unwrap();
    transport.accepted = true;

    assert!(matches!(
        queue.resume_step(&id, &mut transport),
        Err(Error::Io(_))
    ));
    assert_eq!(queue.entries().unwrap(), vec![pending.clone()]);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(
        saved_chunks(&directory, pending.manifest.as_ref().unwrap()),
        chunks
    );
    assert_eq!(transport.negotiate_calls, 1);
    assert_eq!(transport.complete_calls, 1);
    fs::remove_dir(next).unwrap();
    drop(queue);

    let mut queue = spool(&temp);
    assert_eq!(queue.entries().unwrap(), vec![pending]);
    let accepted = queue.resume_step(&id, &mut transport).unwrap().entry;
    assert_eq!(accepted.state, CaptureState::Accepted);
    assert_eq!(
        serde_json::from_slice::<QueueEntry>(&fs::read(path).unwrap()).unwrap(),
        accepted
    );
    assert_eq!(transport.negotiate_calls, 1);
    assert_eq!(transport.complete_calls, 1);
}
