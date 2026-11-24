use super::*;

fn transfer(status: &Status) -> serde_json::Value {
    serde_json::to_value(status).unwrap()["uploadProgress"]["transfer"].clone()
}

#[test]
fn cloud_scheduler_backfill_transfer_accumulates_sessions_after_spool_removal() {
    let mut fixture = Fixture::new(READY, 0);
    write_source(
        &fixture.root,
        &fixture.project,
        "second",
        "another synthetic session with different compressed bytes",
    );
    fixture.schedule_backfill();
    let mut observed = Vec::new();
    let mut plans = BTreeMap::new();
    for _ in 0..20 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
        for entry in fixture.host.spool().unwrap().entries().unwrap() {
            let progress = entry.progress().unwrap();
            if progress.negotiated {
                plans.insert(entry.identity.capture_id, progress.required_transfer_bytes);
            }
        }
        if serde_json::to_value(observed.last().unwrap()).unwrap()["uploadProgress"]["uploaded"]
            == 2
        {
            break;
        }
    }
    assert_eq!(plans.len(), 2);
    assert_eq!(
        transfer(&observed[0]),
        serde_json::json!({ "sentBytes": 0, "totalBytes": null })
    );
    let total: u64 = plans.values().sum();
    let completed = observed.last().unwrap();
    assert_eq!(
        serde_json::to_value(completed).unwrap()["uploadProgress"]["uploaded"],
        2
    );
    assert_eq!(
        transfer(completed),
        serde_json::json!({ "sentBytes": total, "totalBytes": total })
    );
    assert!(fixture.host.spool().unwrap().entries().unwrap().is_empty());
    for _ in 0..3 {
        assert_eq!(transfer(&fixture.published_status()), transfer(completed));
    }
}

#[test]
fn cloud_scheduler_backfill_transfer_survives_integrity_verification_and_retry() {
    let mut fixture = Fixture::new(VERIFY_PENDING, 0);
    fixture.schedule_backfill();
    let mut observed = Vec::new();
    for _ in 0..6 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
    }
    let entry = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(entry.state, CaptureState::AwaitingIntegrity);
    let total = entry.progress().unwrap().required_transfer_bytes;
    let expected = serde_json::json!({ "sentBytes": total, "totalBytes": total });
    let pending = fixture.published_status();
    assert_eq!(transfer(&pending), expected);
    assert!(
        pending
            .automatic_recovery_count
            .is_some_and(|count| count > 0)
    );
    assert_eq!(
        serde_json::to_value(&pending).unwrap()["uploadProgress"]["uploaded"],
        0
    );
    let grant_id = fixture.engine.session().unwrap().grant_id.clone();
    let retried = fixture
        .runner
        .retry_status(&BTreeSet::new(), &grant_id, |_, _| Ok(pending))
        .unwrap();
    assert_eq!(transfer(&retried), expected);
    fixture.service.mode.store(READY, Ordering::Release);
    fixture.runner.next_integrity_poll = None;
    fixture.turn_published(&mut observed);
    assert!(fixture.host.spool().unwrap().entries().unwrap().is_empty());
    assert_eq!(transfer(observed.last().unwrap()), expected);
}

#[test]
fn cloud_scheduler_backfill_transfer_keeps_both_acceptance_paths_before_discovery() {
    for (retained_acceptance, saved_receipt) in [(false, false), (true, false), (true, true)] {
        let mut fixture = Fixture::new(VERIFY_PENDING, 0);
        for _ in 0..6 {
            fixture.runner.next_integrity_poll = None;
            fixture.turn();
        }
        let entry = fixture.host.spool().unwrap().entries().unwrap().remove(0);
        assert_eq!(entry.state, CaptureState::AwaitingIntegrity);
        let total = entry.progress().unwrap().required_transfer_bytes;
        fixture.service.mode.store(READY, Ordering::Release);
        if retained_acceptance {
            let current = || true;
            let mut transport = upload::HttpTransport::new(
                &mut fixture.engine,
                &current,
                &current,
                &fixture.service.origin,
                None,
            );
            let step = fixture
                .host
                .spool()
                .unwrap()
                .resume_step(&entry.identity, &mut transport)
                .unwrap();
            assert_eq!(step.entry.state, CaptureState::Accepted);
            if saved_receipt {
                fixture.store.accept_cloud_sync(&step.entry).unwrap();
            }
        }
        fixture.schedule_backfill();
        fixture.runner.next_integrity_poll = None;
        let mut observed = Vec::new();
        for _ in 0..3 {
            fixture.turn_published(&mut observed);
        }
        let completed = observed.last().unwrap();
        assert_eq!(
            serde_json::to_value(completed).unwrap()["uploadProgress"]["uploaded"],
            1
        );
        assert_eq!(
            transfer(completed),
            serde_json::json!({
                "sentBytes": if saved_receipt { 0 } else { total },
                "totalBytes": if saved_receipt { 0 } else { total },
            })
        );
        assert!(fixture.host.spool().unwrap().entries().unwrap().is_empty());
        assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    }
}

#[test]
fn cloud_scheduler_backfill_transfer_new_scan_uses_zero_for_matching_receipts() {
    let mut fixture = Fixture::new(READY, 0);
    fixture.accept_first();
    fixture.schedule_backfill();
    let mut observed = Vec::new();
    for _ in 0..3 {
        fixture.turn_published(&mut observed);
    }
    assert_eq!(
        transfer(observed.last().unwrap()),
        serde_json::json!({ "sentBytes": 0, "totalBytes": 0 })
    );
    fixture.sign_in_again();
    assert!(fixture.published_status().upload_progress.is_none());
}

#[test]
fn cloud_scheduler_backfill_transfer_drops_credit_for_missing_unaccepted_entries() {
    let mut fixture = Fixture::new(READY, 0);
    fixture.schedule_backfill();
    let mut observed = Vec::new();
    fixture.turn_published(&mut observed);
    fixture.turn_published(&mut observed);
    let entry = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert!(entry.progress().unwrap().confirmed_upload_bytes > 0);
    fixture
        .host
        .spool()
        .unwrap()
        .cancel(&entry.identity)
        .unwrap();
    assert_eq!(
        transfer(&fixture.published_status()),
        serde_json::json!({ "sentBytes": 0, "totalBytes": null })
    );
}
