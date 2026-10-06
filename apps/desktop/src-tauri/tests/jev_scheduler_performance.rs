use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use antiburn_local::analysis::jev::{
    JevAnswer, JevCheck, JevCheckPlan, JevCheckRevisions, JevCoverage, JevError, JevEvidenceStore,
    JevInputWindow, JevQuestion, JevResponse, JevRunProgress, JevSessionContext, JevUsage,
    JevWorkItem, JevWorkItemResult, PINNED_MODEL, run_jev_check,
};
use serde_json::{Value, json};

struct CountingAllocator;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
            ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let pointer = unsafe { System.realloc(pointer, layout, size) };
        if !pointer.is_null() {
            if size >= layout.size() {
                let live =
                    LIVE.fetch_add(size - layout.size(), Ordering::Relaxed) + size - layout.size();
                PEAK.fetch_max(live, Ordering::Relaxed);
            } else {
                LIVE.fetch_sub(layout.size() - size, Ordering::Relaxed);
            }
            ALLOCATED.fetch_add(size, Ordering::Relaxed);
        }
        pointer
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct SyntheticCheck {
    id: &'static str,
    items: usize,
}

impl JevCheck for SyntheticCheck {
    type Prepared = ();
    type Result = usize;

    fn id(&self) -> &'static str {
        self.id
    }

    fn revisions(&self) -> JevCheckRevisions {
        JevCheckRevisions {
            projection: 1,
            chunking: 1,
            questions: 1,
            reducer: 1,
        }
    }

    fn prepare(&self, context: &JevSessionContext) -> Result<JevCheckPlan<()>, JevError> {
        Ok(JevCheckPlan {
            check_id: self.id.to_owned(),
            input_revision: context.input_revision.clone(),
            revisions: self.revisions(),
            work_items: (0..self.items)
                .map(|index| JevWorkItem {
                    id: format!("item-{index}"),
                    window: JevInputWindow {
                        fields: json!({"text": "x".repeat(8192)}),
                        evidence: Vec::new(),
                    },
                    questions: BTreeMap::from([(
                        "decision".to_owned(),
                        JevQuestion::Noul {
                            instructions: json!("Is the selected evidence sufficient?"),
                            criteria: None,
                        },
                    )]),
                })
                .collect(),
            skipped_item_ids: Vec::new(),
            coverage: JevCoverage::default(),
            prepared: (),
        })
    }

    fn reduce(
        &self,
        _plan: &JevCheckPlan<()>,
        results: &[JevWorkItemResult],
        complete: bool,
    ) -> Result<usize, JevError> {
        if !complete || results.len() != self.items {
            return Err(JevError::InvalidCheckPlan);
        }
        Ok(results.len())
    }
}

#[test]
#[ignore = "offline release scheduler allocation benchmark"]
fn measures_two_checks_with_per_response_checkpoints() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let context = JevSessionContext {
        session_identity: "synthetic-session".to_owned(),
        input_revision: "revision".to_owned(),
        check_context: Value::Null,
        limitations: Vec::new(),
        reference_snapshots: Vec::new(),
        evidence_store: JevEvidenceStore::default(),
    };
    for items in [8, 128] {
        let first = SyntheticCheck {
            id: "first-check",
            items,
        };
        let second = SyntheticCheck {
            id: "second-check",
            items,
        };
        for sample in 0..8 {
            let before = LIVE.load(Ordering::Relaxed);
            PEAK.store(before, Ordering::Relaxed);
            ALLOCATED.store(0, Ordering::Relaxed);
            let started = Instant::now();
            let mut checkpoints = 0;
            let mut first_checkpoint_us = None;
            let execute = |batch: std::sync::Arc<
                antiburn_local::analysis::jev::JevRequestBatch,
            >| async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(JevResponse {
                    model: PINNED_MODEL.to_owned(),
                    answers: batch
                        .request
                        .questions
                        .keys()
                        .map(|id| (id.clone(), JevAnswer::Noul { noul: 0.9 }))
                        .collect(),
                    usage: JevUsage {
                        input_tokens: 100,
                        output_tokens: 1,
                    },
                })
            };
            let checkpoint = |_: &JevRunProgress| {
                checkpoints += 1;
                first_checkpoint_us.get_or_insert_with(|| started.elapsed().as_micros());
                Ok(())
            };
            let (one, two) = runtime.block_on(async {
                let one = run_jev_check(
                    &first,
                    &context,
                    JevRunProgress::default(),
                    &execute,
                    checkpoint,
                );
                let two = run_jev_check(
                    &second,
                    &context,
                    JevRunProgress::default(),
                    &execute,
                    |_| Ok(()),
                );
                tokio::join!(one, two)
            });
            let one = one.unwrap();
            let two = two.unwrap();
            assert_eq!((one.result, two.result), (items, items));
            assert_eq!(checkpoints, one.progress.request_count);
            let elapsed_us = started.elapsed().as_micros();
            let peak = PEAK.load(Ordering::Relaxed).saturating_sub(before);
            let allocated = ALLOCATED.load(Ordering::Relaxed);
            let requests = one.progress.request_count + two.progress.request_count;
            drop((one, two));
            let retained = LIVE.load(Ordering::Relaxed).saturating_sub(before);
            println!(
                "scheduler checks=2 items_per_check={items} sample={sample} warmup={} elapsed_us={elapsed_us} first_checkpoint_us={} requests={requests} checkpoints_first_check={checkpoints} peak_live_bytes={peak} allocated_bytes={allocated} retained_bytes={retained}",
                sample == 0,
                first_checkpoint_us.unwrap()
            );
        }
    }
}
