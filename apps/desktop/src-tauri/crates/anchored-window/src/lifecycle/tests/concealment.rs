use crate::lifecycle::{RequestTransition, ScheduledTask};
use crate::model::{AnchorRegion, AnchoredWindowRequest, RevealPolicy};

use super::fixtures::{lifecycle, region, retargeted};

#[test]
fn generation_checks_cover_concealment_and_retargeting() {
    let mut lifecycle = lifecycle();
    let (first, _) = retargeted(lifecycle.request(
        "first",
        region(),
        RevealPolicy::AfterPresentation,
        120.0,
        None,
    ));
    let conceal = lifecycle.conceal();

    assert!(!lifecycle.presented(first.generation));
    assert!(lifecycle.concealed(conceal.generation));
    assert!(!lifecycle.visible);

    let (second, _) = retargeted(lifecycle.request(
        "second",
        region(),
        RevealPolicy::AfterPresentation,
        120.0,
        None,
    ));
    let stale_conceal = lifecycle.conceal();
    let latest_region = AnchorRegion {
        top: 180.0,
        height: 64.0,
    };
    let (third, _) = retargeted(lifecycle.request(
        "third",
        latest_region,
        RevealPolicy::AfterPresentation,
        120.0,
        None,
    ));

    assert!(!lifecycle.fallback_is_current(stale_conceal.generation));
    assert!(!lifecycle.presented(second.generation));
    assert!(lifecycle.presented(third.generation));
    assert_eq!(lifecycle.state().target, Some("third"));
    assert_eq!(lifecycle.anchor_region, latest_region);
}

#[test]
fn forced_anchor_conceal_makes_the_next_immediate_request_a_cold_reveal() {
    let mut lifecycle = lifecycle();
    lifecycle.renderer_ready = true;
    let (first, reveal_now) = retargeted(lifecycle.request(
        "first",
        region(),
        RevealPolicy::ImmediatePlaceholder,
        320.0,
        None,
    ));
    assert!(reveal_now);
    assert!(lifecycle.presented(first.generation));
    lifecycle.height = 500.0;

    let conceal = lifecycle.conceal();
    lifecycle.force_hidden();
    assert!(!lifecycle.visible);
    assert!(lifecycle.awaiting_concealment);

    let (second, reveal_now) = retargeted(lifecycle.request(
        "second",
        region(),
        RevealPolicy::ImmediatePlaceholder,
        320.0,
        None,
    ));

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
    let (first, _) = retargeted(lifecycle.request(
        "target",
        region(),
        RevealPolicy::ImmediatePlaceholder,
        120.0,
        None,
    ));
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

    let retained = lifecycle.request(
        "target",
        next_region,
        RevealPolicy::ImmediatePlaceholder,
        120.0,
        None,
    );

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
