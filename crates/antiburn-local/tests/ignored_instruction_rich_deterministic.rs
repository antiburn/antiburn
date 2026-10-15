#[path = "support/rich_deterministic.rs"]
mod support;

use antiburn_local::analysis::ignored_instructions::IgnoredInstructionsCheck;
use antiburn_local::analysis::jev::{JevRunProgress, run_jev_check};
use support::{content, context, response, tool};

#[tokio::test]
async fn excluded_body_changes_preserve_accepted_answer_reuse() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let mut worker_output = tool("worker-output", 2, "Read", "worker result before");
    worker_output.kind = "tool_result".to_owned();
    worker_output.authority = "tool".to_owned();
    let raw = content(
        "Do not run Bash commands.",
        vec![
            tool("command", 1, "Bash", "cargo test --lib"),
            worker_output,
        ],
    );
    let initial_context = context(raw.clone());
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let initial = run_jev_check(
        &IgnoredInstructionsCheck,
        &initial_context,
        JevRunProgress::default(),
        move |batch| {
            counter.fetch_add(1, Ordering::SeqCst);
            async move { Ok(response(&batch)) }
        },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert!(initial.complete);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(!initial.progress.results.is_empty());

    let mut changed = raw;
    changed.actions[1].text = "WORKER_OUTPUT_SENTINEL: completed the task".to_owned();
    let changed_context = context(changed);
    assert_eq!(
        initial_context.input_revision,
        changed_context.input_revision
    );
    assert_eq!(initial_context.check_context, changed_context.check_context);

    let before_resume = calls.load(Ordering::SeqCst);
    let counter = Arc::clone(&calls);
    let resumed = run_jev_check(
        &IgnoredInstructionsCheck,
        &changed_context,
        initial.progress,
        move |batch| {
            counter.fetch_add(1, Ordering::SeqCst);
            async move { Ok(response(&batch)) }
        },
        |_| Ok(()),
    )
    .await
    .unwrap();

    assert!(resumed.complete);
    assert_eq!(calls.load(Ordering::SeqCst), before_resume);
    assert_eq!(resumed.result, initial.result);
}
