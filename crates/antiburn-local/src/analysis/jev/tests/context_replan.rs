use super::tests::{item, response_for, tev1_capabilities};
use super::*;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn actual_context_rejection_replans_independent_items_once_without_clipping_evidence() {
    let items = vec![
        item("one", "first complete comparison"),
        item("two", "second complete comparison"),
    ];
    let limits = tev1_capabilities(2050);
    let batches = pack_work_items_with_capabilities(&items, &limits).batches;
    assert_eq!(batches.len(), 1);
    let original_bytes = batches[0].serialized_bytes;
    let calls = std::sync::Mutex::new(Vec::new());
    let mut settled = Vec::new();
    execute_batches_with_context_replan(
        &|batch: std::sync::Arc<JevRequestBatch>| {
            calls.lock().unwrap().push(batch.work_item_ids.clone());
            async move {
                if batch.work_item_ids.len() > 1 {
                    Err(JevError::ContextRejected)
                } else {
                    assert!(batch.serialized_bytes < original_bytes);
                    Ok(response_for(&batch.request))
                }
            }
        },
        batches,
        &limits,
        &items,
        None,
        true,
        |batch, response| {
            let response = response?;
            settled.extend(unpack_jev_response_with_capabilities(
                &batch, &response, &limits,
            )?);
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(calls.lock().unwrap().len(), 3);
    assert_eq!(settled.len(), 2);
    assert_eq!(settled[0].evidence, items[0].window.evidence);
    assert_eq!(settled[1].evidence, items[1].window.evidence);

    let calls = std::sync::atomic::AtomicUsize::new(0);
    let mut failures = 0;
    execute_batches_with_context_replan(
        &|_: std::sync::Arc<JevRequestBatch>| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(JevError::ContextRejected) }
        },
        pack_work_items_with_capabilities(&items, &limits).batches,
        &limits,
        &items,
        None,
        true,
        |_, result| {
            assert_eq!(result, Err(JevError::ContextRejected));
            failures += 1;
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(failures, 2);
}

#[tokio::test]
async fn local_admission_and_unknown_delivery_do_not_trigger_context_replans() {
    let items = vec![item("one", "first"), item("two", "second")];
    let limits = tev1_capabilities(2050);
    for error in [
        JevError::RequestOutcomeUnknown,
        JevError::InvalidRequestSchema,
        JevError::RequestTokenLimitExceeded {
            tokens: 2051,
            maximum: 2050,
        },
    ] {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        execute_batches_with_context_replan(
            &|_: std::sync::Arc<JevRequestBatch>| {
                calls.fetch_add(1, Ordering::SeqCst);
                let error = error.clone();
                async move { Err(error) }
            },
            pack_work_items_with_capabilities(&items, &limits).batches,
            &limits,
            &items,
            None,
            false,
            |_, result| {
                assert_eq!(result, Err(error.clone()));
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
