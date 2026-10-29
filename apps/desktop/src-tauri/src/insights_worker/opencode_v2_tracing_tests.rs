use super::*;
use crate::store::burn_checks::trace_capture::LogCapture;

#[test]
fn capture_failure_trace_reports_the_same_outage_again_after_a_healthy_attachment() {
    let trace = CaptureFailureTrace::default();
    let error = anyhow::Error::new(CaptureFailure(OpenCodeV2SourceStatus::ServiceUnavailable));
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        for retry in 0..100 {
            trace.failed(&error, retry);
        }
        trace.healthy();
        for retry in 0..100 {
            trace.failed(&error, retry);
        }
    });
    let events = captured.events("opencode_v2_capture_stopped");
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| event.level == tracing::Level::WARN)
    );
    assert_eq!(events[0].fields["status"], events[1].fields["status"]);
}

#[test]
fn source_limit_trace_keeps_fixed_causes_counts_and_health_transitions_without_private_input() {
    let trace = CaptureFailureTrace::default();
    let sessions = source_limit_error(
        CaptureLimitKind::CandidateSessions,
        Some(CANDIDATE_SESSION_LIMIT),
        Some(CANDIDATE_SESSION_LIMIT),
    )
    .context("PRIVATE_SESSION_ID /PRIVATE_PATH event=PRIVATE_EVENT_BODY");
    let feed = anyhow::Error::new(opencode_live::stream::EventStreamError::CapacityExceeded)
        .context("PRIVATE_FEED_BODY token=PRIVATE_KEY");
    let recovery = snapshot_limit_context(
        anyhow::Error::new(antiburn_local::analysis::OpenCodeV2SnapshotError::SourceLimit)
            .context("PRIVATE_SNAPSHOT_BODY"),
        CaptureLimitKind::RecoverySnapshot,
    );
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        for retry in 0..100 {
            trace.failed(&sessions, retry);
        }
        for retry in 0..100 {
            trace.failed(&feed, retry);
        }
        for retry in 0..100 {
            trace.failed(&recovery, retry);
        }
        trace.healthy();
        trace.failed(&sessions, 0);
    });
    let events = captured.events("opencode_v2_capture_stopped");
    assert_eq!(events.len(), 4);
    for event in &events {
        assert_eq!(event.level, tracing::Level::WARN);
        assert_eq!(
            event.fields["status"],
            format!("{:?}", OpenCodeV2SourceStatus::SourceLimit.code())
        );
    }
    assert_eq!(events[0].fields["phase"], "\"source_selection\"");
    assert_eq!(events[0].fields["reason"], "\"candidate_session_limit\"");
    assert_eq!(
        events[0].fields["observed"],
        CANDIDATE_SESSION_LIMIT.to_string()
    );
    assert_eq!(
        events[0].fields["limit"],
        CANDIDATE_SESSION_LIMIT.to_string()
    );
    assert_eq!(events[1].fields["phase"], "\"event_feed\"");
    assert_eq!(events[1].fields["reason"], "\"feed_buffer_limit\"");
    assert_eq!(events[2].fields["phase"], "\"recovery\"");
    assert_eq!(events[2].fields["reason"], "\"snapshot_budget\"");
    assert!(!events[1].fields.contains_key("observed"));
    assert!(!events[2].fields.contains_key("limit"));
    assert!(!captured.text().contains("PRIVATE_"));
}

#[test]
fn capture_failure_trace_distinguishes_feed_and_boundary_causes_with_the_same_source_status() {
    use opencode_live::stream::EventStreamError;
    let trace = CaptureFailureTrace::default();
    let captured = LogCapture::default();
    let errors = [
        anyhow::Error::new(EventStreamError::IdleTimeout),
        anyhow::Error::new(EventStreamError::Sse(
            opencode_live::SseError::MalformedJson,
        )),
        anyhow::Error::new(EventStreamError::Body),
        capture_gap_error(CaptureGap::InvalidBoundary),
        capture_gap_error(CaptureGap::InvalidEvent),
    ];
    tracing::subscriber::with_default(captured.clone(), || {
        for error in errors {
            let error = error.context("PRIVATE_FEED_BODY /PRIVATE_PATH");
            for retry in 0..100 {
                trace.failed(&error, retry);
            }
        }
    });
    let events = captured.events("opencode_v2_capture_stopped");
    assert_eq!(events.len(), 5);
    for (event, (phase, reason)) in events.iter().zip([
        ("event_feed", "heartbeat_timeout"),
        ("event_feed", "malformed_json"),
        ("event_feed", "body_read_failed"),
        ("capture_boundary", "boundary_invalid"),
        ("capture_boundary", "event_invalid"),
    ]) {
        assert_eq!(event.level, tracing::Level::WARN);
        assert_eq!(event.fields["phase"], format!("{phase:?}"));
        assert_eq!(event.fields["reason"], format!("{reason:?}"));
        assert_eq!(event.fields["status"], "\"capture-gap\"");
    }
    assert!(!captured.text().contains("PRIVATE_"));
}

#[test]
fn capture_limit_trace_preserves_each_local_limit_and_numeric_bound() {
    let captured = LogCapture::default();
    let trace = CaptureFailureTrace::default();
    let limits = [
        (
            CaptureLimitKind::CandidateSessions,
            CANDIDATE_SESSION_LIMIT,
            CANDIDATE_SESSION_LIMIT,
        ),
        (
            CaptureLimitKind::BoundSessions,
            CAPTURE_SESSION_LIMIT + 1,
            CAPTURE_SESSION_LIMIT,
        ),
        (
            CaptureLimitKind::Databases,
            CAPTURE_DATABASE_LIMIT + 1,
            CAPTURE_DATABASE_LIMIT,
        ),
        (
            CaptureLimitKind::RecoverySessions,
            CAPTURE_SESSION_LIMIT + 1,
            CAPTURE_SESSION_LIMIT,
        ),
        (
            CaptureLimitKind::RetainedSessions,
            CAPTURE_SESSION_LIMIT + 1,
            CAPTURE_SESSION_LIMIT,
        ),
    ];
    tracing::subscriber::with_default(captured.clone(), || {
        for (kind, observed, limit) in limits {
            let error = source_limit_error(kind, Some(observed), Some(limit));
            for retry in 0..100 {
                trace.failed(&error, retry);
            }
        }
    });
    let events = captured.events("opencode_v2_capture_stopped");
    assert_eq!(events.len(), limits.len());
    for (event, (kind, observed, limit)) in events.iter().zip(limits) {
        let (phase, reason) = kind.codes();
        assert_eq!(event.fields["phase"], format!("{phase:?}"));
        assert_eq!(event.fields["reason"], format!("{reason:?}"));
        assert_eq!(event.fields["observed"], observed.to_string());
        assert_eq!(event.fields["limit"], limit.to_string());
    }
}

#[test]
fn capture_limit_trace_recovers_typed_limits_through_error_wrappers() {
    use opencode_live::stream::EventStreamError;
    let errors = [
        (
            anyhow::Error::new(EventStreamError::CapacityExceeded),
            CaptureLimitKind::FeedBuffer,
        ),
        (
            anyhow::Error::new(EventStreamError::Sse(
                opencode_live::SseError::FrameTooLarge,
            )),
            CaptureLimitKind::SseFrame,
        ),
        (
            anyhow::Error::new(EventStreamError::Registration(
                opencode_live::RegistrationError::TooLarge,
            )),
            CaptureLimitKind::Registration,
        ),
        (
            anyhow::Error::new(antiburn_local::analysis::OpenCodeV2SnapshotError::SourceLimit),
            CaptureLimitKind::Snapshot,
        ),
        (
            capture_gap_error(CaptureGap::CapacityExceeded),
            CaptureLimitKind::CaptureBuffer,
        ),
    ];
    let trace = CaptureFailureTrace::default();
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        for (error, kind) in errors {
            let error = error.context("PRIVATE_LIMIT_BODY /PRIVATE_PATH");
            let context = capture_limit_context(&error).unwrap();
            assert!(context.kind == kind);
            assert!(context.observed.is_none());
            assert!(context.limit.is_none());
            for retry in 0..100 {
                trace.failed(&error, retry);
            }
        }
    });
    let events = captured.events("opencode_v2_capture_stopped");
    assert_eq!(events.len(), 5);
    assert!(
        events
            .iter()
            .all(|event| event.fields["status"] == "\"source-limit\"")
    );
    assert!(!captured.text().contains("PRIVATE_"));
}

#[test]
fn recovery_limit_trace_keeps_local_failures_without_claiming_the_feed_stopped() {
    let trace = CaptureFailureTrace::default();
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        for kind in [
            CaptureLimitKind::RecoverySnapshot,
            CaptureLimitKind::RecoveryBuffer,
            CaptureLimitKind::RecoverySessions,
        ] {
            let error = source_limit_error(kind, None, None).context("PRIVATE_RECOVERY_BODY");
            for _ in 0..100 {
                trace.recovery_limited(&error);
            }
        }
        trace.recovered();
        trace.recovery_limited(&source_limit_error(
            CaptureLimitKind::RecoverySessions,
            None,
            None,
        ));
    });
    let events = captured.events("opencode_v2_recovery_limited");
    assert_eq!(events.len(), 4);
    assert!(
        events
            .iter()
            .all(|event| event.level == tracing::Level::WARN)
    );
    assert_eq!(events[0].fields["reason"], "\"snapshot_budget\"");
    assert_eq!(events[1].fields["reason"], "\"capture_buffer_limit\"");
    assert_eq!(events[2].fields["reason"], "\"session_limit\"");
    assert!(captured.events("opencode_v2_capture_stopped").is_empty());
    assert!(!captured.text().contains("PRIVATE_"));
}
