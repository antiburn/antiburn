use crate::lifecycle::{RequestTransition, ScheduledTask};
use crate::model::{AnchorRegion, AnchoredWindowRequest};

use super::fixtures::{lifecycle, region, retargeted};

#[test]
fn generation_checks_cover_concealment_and_retargeting() {
    let mut lifecycle = lifecycle();
    let (first, _) = retargeted(lifecycle.request("first", region(), 120.0, None));
    let conceal = lifecycle.conceal();

    assert!(!lifecycle.presented(first.generation));
    assert!(lifecycle.concealed(conceal.generation));
    assert!(!lifecycle.visible);

    let (second, _) = retargeted(lifecycle.request("second", region(), 120.0, None));
    let stale_conceal = lifecycle.conceal();
    let latest_region = AnchorRegion {
        top: 180.0,
        height: 64.0,
    };
    let (third, _) = retargeted(lifecycle.request("third", latest_region, 120.0, None));

    assert!(!lifecycle.fallback_is_current(stale_conceal.generation));
    assert!(!lifecycle.presented(second.generation));
    assert!(lifecycle.presented(third.generation));
    assert_eq!(lifecycle.state().target, Some("third"));
    assert_eq!(lifecycle.anchor_region, latest_region);
}

#[test]
fn renderer_retirement_requires_the_current_completed_concealment() {
    let mut lifecycle = lifecycle();
    lifecycle.renderer_ready = true;
    let (first, _) = retargeted(lifecycle.request("first", region(), 120.0, None));
    assert!(lifecycle.presented(first.generation));

    let conceal = lifecycle.conceal();
    assert!(!lifecycle.renderer_retirement_is_current(conceal.generation));
    assert!(lifecycle.concealed(conceal.generation));
    assert!(lifecycle.renderer_retirement_is_current(conceal.generation));

    let (second, _) = retargeted(lifecycle.request("second", region(), 120.0, None));
    assert!(!lifecycle.renderer_retirement_is_current(conceal.generation));
    assert!(!lifecycle.renderer_retirement_is_current(second.generation));
}

#[test]
fn late_renderer_destruction_preserves_a_new_target_for_rebuild() {
    let mut lifecycle = lifecycle();
    lifecycle.renderer_generation = 7;
    lifecycle.renderer_ready = true;
    let (first, _) = retargeted(lifecycle.request("first", region(), 120.0, None));
    assert!(lifecycle.presented(first.generation));
    let conceal = lifecycle.conceal();
    assert!(lifecycle.concealed(conceal.generation));

    let (second, _) = retargeted(lifecycle.request("second", region(), 120.0, None));
    assert!(!lifecycle.renderer_retirement_is_current(conceal.generation));

    lifecycle.renderer_destroyed();
    assert_eq!(lifecycle.state().target, Some("second"));
    assert!(lifecycle.awaiting_presentation);
    assert_eq!(lifecycle.renderer_ready(7), None);
    assert_eq!(lifecycle.renderer_ready(8), Some(false));
    let pending = lifecycle
        .pending_render_request()
        .expect("the new target needs the replacement renderer");
    assert_eq!(pending.generation, second.generation);
    assert_eq!(pending.target, Some("second"));
}

#[test]
fn anchor_teardown_retires_a_renderer_that_is_still_loading() {
    let mut lifecycle = lifecycle();
    lifecycle.renderer_generation = 7;
    let (request, _) = retargeted(lifecycle.request("target", region(), 120.0, None));
    assert!(lifecycle.awaiting_presentation);

    let conceal = lifecycle.conceal();
    lifecycle.force_hidden();
    assert!(lifecycle.concealed(conceal.generation));
    assert!(lifecycle.renderer_retirement_is_current(conceal.generation));

    lifecycle.renderer_destroyed();
    assert_eq!(lifecycle.state().target, None);
    assert!(!lifecycle.awaiting_presentation);
    assert_eq!(lifecycle.renderer_ready(7), None);
    assert!(!lifecycle.presented(request.generation));
}

#[test]
fn forced_anchor_conceal_makes_the_next_immediate_request_a_cold_reveal() {
    let mut lifecycle = lifecycle();
    lifecycle.renderer_ready = true;
    let (first, reveal_now) = retargeted(lifecycle.request("first", region(), 320.0, None));
    assert!(reveal_now);
    assert!(lifecycle.presented(first.generation));
    lifecycle.height = 500.0;

    let conceal = lifecycle.conceal();
    lifecycle.force_hidden();
    assert!(!lifecycle.visible);
    assert!(lifecycle.awaiting_concealment);

    let (second, reveal_now) = retargeted(lifecycle.request("second", region(), 320.0, None));

    assert_eq!(second.generation, conceal.generation + 1);
    assert!(reveal_now);
    assert_eq!(lifecycle.height, 320.0);
    assert!(lifecycle.presented(second.generation));
    assert!(lifecycle.visible);
}

#[test]
fn same_target_reentry_cancels_concealment_without_restarting_presentation() {
    let mut lifecycle = lifecycle();
    lifecycle.renderer_ready = true;
    let (first, _) = retargeted(lifecycle.request("target", region(), 120.0, None));
    assert!(lifecycle.presented(first.generation));
    lifecycle.height = 240.0;

    let task_token = lifecycle.reserve_task();
    lifecycle.task = Some(ScheduledTask {
        token: task_token,
        handle: tauri::async_runtime::spawn(std::future::pending()),
    });
    let next_region = AnchorRegion {
        top: 36.0,
        height: 52.0,
    };

    let retained = lifecycle.request("target", next_region, 120.0, None);

    assert_eq!(
        retained,
        RequestTransition::Retained {
            request: AnchoredWindowRequest {
                generation: first.generation,
                target: Some("target"),
                retarget_commit_required: false,
            },
            reposition: true,
        }
    );
    assert!(lifecycle.task.is_none());
    assert_eq!(lifecycle.generation, first.generation);
    assert_eq!(lifecycle.anchor_region, next_region);
    assert_eq!(lifecycle.height, 240.0);
    assert!(lifecycle.visible);
    assert!(!lifecycle.awaiting_presentation);
    assert!(!lifecycle.awaiting_concealment);
}
