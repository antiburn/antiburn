use crate::lifecycle::{Lifecycle, RequestTransition};
use crate::model::{AnchorRegion, AnchoredWindowRequest};

pub(super) fn lifecycle() -> Lifecycle<&'static str, &'static str> {
    Lifecycle::new(120.0)
}

pub(super) fn region() -> AnchorRegion {
    AnchorRegion {
        top: 24.0,
        height: 48.0,
    }
}

pub(super) fn retargeted<T>(transition: RequestTransition<T>) -> (AnchoredWindowRequest<T>, bool) {
    let RequestTransition::Retargeted {
        request,
        reveal_now,
    } = transition
    else {
        panic!("the request retargets the lifecycle");
    };
    (request, reveal_now)
}
