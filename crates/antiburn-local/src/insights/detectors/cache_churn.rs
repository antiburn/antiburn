// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Cache Churn: cache writes paid after observed churn events — idle
//! expiry, model switching, or manual compaction.
//!
//! Provider-eviction estimates are not part of the rule: the
//! `provider_eviction` marker carries no estimate yet. The rule uses
//! only user-visible churn events the transcript proves.
//!
//! Partial-evidence rules:
//! - Partial cache evidence still permits a finding. An observed
//!   churn event next to paid cache writes proves presence.
//! - Partial cache evidence prevents clean. A missed record may hide
//!   churn.

use crate::analysis::SessionEvidence;

use super::{Observation, ReportCatalogs, observed};

pub(crate) fn evaluate(evidence: &SessionEvidence, catalogs: &ReportCatalogs) -> Observation {
    if let Some(cache) = observed(&evidence.cache) {
        let churn_event = !cache.model_transitions.is_empty()
            || (cache.longest_idle_gap_ms > 0
                && cache.longest_idle_gap_ms >= catalogs.cache_idle_expiry_ms)
            || cache.user_controlled_churn.manual_compactions > 0;
        if churn_event && cache.cache_creation_tokens > 0 {
            return Observation::Finding;
        }
    }
    Observation::NoFinding
}

#[cfg(test)]
mod tests {
    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::{CacheEvidence, CoverageReason, EvidenceValue, ModelTransition};

    fn edit_cache(
        evidence: &mut SessionEvidence,
        partial: bool,
        edit: impl FnOnce(&mut CacheEvidence),
    ) {
        let EvidenceValue::Complete(mut cache) = evidence.cache.clone() else {
            unreachable!()
        };
        edit(&mut cache);
        evidence.cache = if partial {
            EvidenceValue::Partial {
                observed: cache,
                reason: CoverageReason::MalformedRecord,
            }
        } else {
            EvidenceValue::Complete(cache)
        };
    }

    #[test]
    fn model_switch_with_paid_cache_writes_is_a_finding_even_from_partial_evidence() {
        for partial in [false, true] {
            let mut evidence = claude_evidence("churn");
            edit_cache(&mut evidence, partial, |cache| {
                cache.cache_creation_tokens = 5_000;
                cache.model_transitions.push(ModelTransition {
                    ts_ms: 10,
                    from_model: "model-a".to_owned(),
                    to_model: "model-b".to_owned(),
                });
            });

            assert_eq!(evaluate(&evidence, &ReportCatalogs::default()), {
                Observation::Finding
            });
        }
    }

    #[test]
    fn idle_gap_past_expiry_with_paid_cache_writes_is_a_finding() {
        let catalogs = ReportCatalogs::default();
        let mut evidence = claude_evidence("idle");
        edit_cache(&mut evidence, false, |cache| {
            cache.cache_creation_tokens = 1;
            cache.longest_idle_gap_ms = catalogs.cache_idle_expiry_ms;
        });

        assert_eq!(evaluate(&evidence, &catalogs), Observation::Finding);
    }

    #[test]
    fn churn_event_without_paid_cache_writes_is_no_finding() {
        let mut evidence = claude_evidence("free-churn");
        edit_cache(&mut evidence, false, |cache| {
            cache.user_controlled_churn.manual_compactions = 2;
        });

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::NoFinding
        );
    }

    #[test]
    fn quiet_cache_is_no_finding() {
        assert_eq!(
            evaluate(&claude_evidence("quiet"), &ReportCatalogs::default()),
            Observation::NoFinding
        );
    }
}
