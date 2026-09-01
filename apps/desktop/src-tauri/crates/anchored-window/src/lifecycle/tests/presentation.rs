use crate::lifecycle::RequestTransition;
use crate::model::{AnchorRegion, AnchoredWindowRequest, RevealPolicy};

use super::fixtures::{lifecycle, region, retargeted};

#[test]
fn visible_immediate_retarget_waits_for_the_renderer_commit() {
    let mut lifecycle = lifecycle();
    lifecycle.renderer_generation = 7;
    let (queued, reveal_before_ready) = retargeted(lifecycle.request(
        "target",
        region(),
        RevealPolicy::ImmediatePlaceholder,
        320.0,
        None,
    ));
    assert!(!reveal_before_ready);
    assert!(!queued.retarget_commit_required);

    let Some(reveal_ready) = lifecycle.renderer_ready(7, RevealPolicy::ImmediatePlaceholder) else {
        panic!("the renderer generation is current");
    };
    let delivered = lifecycle
        .pending_render_request()
        .expect("the current request needs delivery");

    assert!(reveal_ready);
    assert!(lifecycle.visible);
    assert_eq!(delivered.generation, queued.generation);
    assert_eq!(delivered.target, queued.target);
    assert_eq!(
        delivered.retarget_commit_required,
        queued.retarget_commit_required
    );
    assert_eq!(delivered.initial_presentation, None);
    lifecycle.height = 500.0;

    let (retargeted, reveal_now) = retargeted(lifecycle.request(
        "second",
        region(),
        RevealPolicy::ImmediatePlaceholder,
        320.0,
        Some("second presentation"),
    ));

    assert_eq!(retargeted.generation, queued.generation + 1);
    assert!(retargeted.retarget_commit_required);
    assert!(!reveal_now);
    assert!(lifecycle.visible);
    assert_eq!(lifecycle.height, 500.0);
    assert!(lifecycle.awaiting_retarget_commit);
    assert_eq!(
        lifecycle.render_request().initial_presentation,
        Some("second presentation")
    );
    assert!(!lifecycle.retarget_committed(queued.generation));
    assert!(lifecycle.retarget_committed(retargeted.generation));
    assert!(!lifecycle.awaiting_retarget_commit);
    assert!(!lifecycle.retarget_committed(retargeted.generation));
}

#[test]
fn cold_seeded_request_waits_for_presentation_before_reveal() {
    let mut lifecycle = lifecycle();
    lifecycle.renderer_generation = 7;
    let (queued, reveal_before_ready) = retargeted(lifecycle.request(
        "target",
        region(),
        RevealPolicy::ImmediatePlaceholder,
        320.0,
        Some("presentation"),
    ));

    assert!(!reveal_before_ready);
    let Some(reveal_ready) = lifecycle.renderer_ready(7, RevealPolicy::ImmediatePlaceholder) else {
        panic!("the renderer generation is current");
    };
    let delivered = lifecycle
        .pending_render_request()
        .expect("the current request needs delivery");

    assert!(!reveal_ready);
    assert!(!lifecycle.visible);
    assert!(lifecycle.awaiting_presentation);
    assert_eq!(delivered.initial_presentation, Some("presentation"));
    assert!(lifecycle.presented(queued.generation));
    assert!(lifecycle.visible);
}

#[test]
fn seeded_renderer_recovery_rearms_the_presentation_barrier() {
    let mut lifecycle = lifecycle();
    lifecycle.renderer_generation = 7;
    lifecycle.renderer_ready = true;
    let (request, reveal_now) = retargeted(lifecycle.request(
        "target",
        region(),
        RevealPolicy::ImmediatePlaceholder,
        320.0,
        Some("presentation"),
    ));
    assert!(!reveal_now);
    assert!(lifecycle.presented(request.generation));
    assert!(lifecycle.visible);

    lifecycle.renderer_destroyed();
    lifecycle.renderer_generation = 8;

    assert!(!lifecycle.visible);
    assert!(lifecycle.awaiting_presentation);
    let Some(reveal_ready) = lifecycle.renderer_ready(8, RevealPolicy::ImmediatePlaceholder) else {
        panic!("the replacement renderer generation is current");
    };
    let delivered = lifecycle
        .pending_render_request()
        .expect("the replacement renderer needs delivery");
    assert!(!reveal_ready);
    assert_eq!(delivered.generation, request.generation);
    assert_eq!(delivered.initial_presentation, Some("presentation"));
    assert!(lifecycle.presented(request.generation));
    assert!(lifecycle.visible);
}

#[test]
fn unseeded_renderer_recovery_waits_for_replacement_presentation() {
    let mut lifecycle = lifecycle();
    lifecycle.renderer_generation = 7;
    lifecycle.renderer_ready = true;
    let (request, reveal_now) = retargeted(lifecycle.request(
        "target",
        region(),
        RevealPolicy::ImmediatePlaceholder,
        320.0,
        None,
    ));
    assert!(reveal_now);
    assert!(lifecycle.presented(request.generation));
    assert!(lifecycle.visible);

    lifecycle.renderer_destroyed();
    lifecycle.renderer_generation = 8;

    assert!(!lifecycle.visible);
    assert!(lifecycle.awaiting_presentation);
    assert_eq!(
        lifecycle.renderer_ready(8, RevealPolicy::ImmediatePlaceholder),
        Some(false)
    );
    let delivered = lifecycle
        .pending_render_request()
        .expect("the replacement renderer needs delivery");
    assert_eq!(delivered.generation, request.generation);
    assert_eq!(delivered.initial_presentation, None);
    assert!(lifecycle.presented(request.generation));
    assert!(lifecycle.visible);
}

#[test]
fn failed_delivery_remains_retryable_for_the_same_target() {
    let mut lifecycle = lifecycle();
    lifecycle.renderer_ready = true;
    let (request, _) = retargeted(lifecycle.request(
        "target",
        region(),
        RevealPolicy::AfterPresentation,
        120.0,
        Some("presentation"),
    ));

    assert_eq!(
        lifecycle
            .pending_render_request()
            .expect("the new target needs delivery")
            .generation,
        request.generation
    );
    let retained = lifecycle.request(
        "target",
        region(),
        RevealPolicy::AfterPresentation,
        120.0,
        Some("ignored replacement"),
    );
    assert!(matches!(retained, RequestTransition::Retained { .. }));
    assert!(lifecycle.pending_render_request().is_some());
    assert!(lifecycle.mark_delivered(request.generation));
    assert!(lifecycle.pending_render_request().is_none());
    lifecycle.force_hidden();
    assert!(lifecycle.pending_render_request().is_some());

    lifecycle.renderer_destroyed();
    lifecycle.renderer_generation = 9;
    assert_eq!(
        lifecycle.renderer_ready(9, RevealPolicy::AfterPresentation),
        Some(false)
    );
    assert!(lifecycle.pending_render_request().is_some());
}

#[test]
fn same_target_reentry_does_not_bypass_a_pending_retarget_commit() {
    let mut lifecycle = lifecycle();
    lifecycle.renderer_ready = true;
    let (first, _) = retargeted(lifecycle.request(
        "first",
        region(),
        RevealPolicy::ImmediatePlaceholder,
        120.0,
        None,
    ));
    assert!(lifecycle.presented(first.generation));

    let next_region = AnchorRegion {
        top: 80.0,
        height: 52.0,
    };
    let (second, _) = retargeted(lifecycle.request(
        "second",
        next_region,
        RevealPolicy::ImmediatePlaceholder,
        120.0,
        Some("second presentation"),
    ));
    assert!(second.retarget_commit_required);

    assert_eq!(
        lifecycle.request(
            "second",
            next_region,
            RevealPolicy::ImmediatePlaceholder,
            120.0,
            Some("replacement presentation"),
        ),
        RequestTransition::Retained {
            request: AnchoredWindowRequest {
                generation: second.generation,
                target: second.target,
                retarget_commit_required: second.retarget_commit_required,
            },
            reposition: false,
        }
    );
    assert!(lifecycle.awaiting_retarget_commit);
    assert_eq!(
        lifecycle.render_request().initial_presentation,
        Some("second presentation")
    );
    assert!(lifecycle.presented(second.generation));
    assert!(!lifecycle.awaiting_retarget_commit);
    assert!(!lifecycle.retarget_committed(second.generation));
}
