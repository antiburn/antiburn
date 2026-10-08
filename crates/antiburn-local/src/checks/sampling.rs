//! Pure incremental sampling data for one main Smart Burn Check worker.
//!
//! Checks supply candidates in their domain ranking order. The caller persists
//! progress after selection and outcomes, and starts a run only when its scheduler
//! permits one. Selection consumes a judgment slot, not an HTTP request slot.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A bounded identity for a check, candidate, semantic context, or answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct StableId([u8; 32]);

impl From<StableId> for String {
    fn from(id: StableId) -> Self {
        id.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

impl TryFrom<String> for StableId {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 64 || !value.is_ascii() {
            return Err("invalid sampling identity");
        }
        let mut bytes = [0; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
                .map_err(|_| "invalid sampling identity")?;
        }
        Ok(Self(bytes))
    }
}

impl StableId {
    /// Hash length-delimited identity parts. Include a check-owned revision.
    pub fn new(domain: &str, parts: &[&[u8]]) -> Self {
        let mut hash = Sha256::new();
        for part in std::iter::once(domain.as_bytes()).chain(parts.iter().copied()) {
            hash.update((part.len() as u64).to_be_bytes());
            hash.update(part);
        }
        Self(hash.finalize().into())
    }
}

/// One required question on one required evidence window.
///
/// Include question revision and selected evidence in this identity. Exclude
/// transport batch IDs, packing order, and request size limits.
pub type AnswerId = StableId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: StableId,
    pub required_answers: Vec<AnswerId>,
}

/// Retained state cannot exceed these limits. A full ledger requires explicit
/// inventory removal; completed entries are never silently evicted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SamplingLimits {
    pub checks: usize,
    pub candidates_per_check: usize,
    pub answers_per_candidate: usize,
    pub judgments_per_run: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplingError {
    InvalidLimits,
    Capacity,
    DuplicateIdentity,
    EmptyRequirements,
    StaleJob,
    UnexpectedAnswer,
    IncompleteCandidate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CandidateProgress {
    required: BTreeSet<AnswerId>,
    reduced: BTreeSet<AnswerId>,
    attempted: bool,
    introduced_run: u64,
    active_attempt: Option<u64>,
    complete: bool,
    terminal: bool,
    last_service: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CheckProgress {
    epoch: StableId,
    order: Vec<StableId>,
    chronology: Vec<StableId>,
    candidates: BTreeMap<StableId, CandidateProgress>,
    served: BTreeSet<StableId>,
    prefer_unchecked: bool,
    diverse_stratum: usize,
}

/// Persist this value with the existing engine progress record. It contains
/// identities and completion state, not evidence text or provider answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SamplingProgress {
    limits: SamplingLimits,
    checks: BTreeMap<StableId, CheckProgress>,
    check_after: Option<StableId>,
    run: u64,
    remaining: usize,
    last_attempt: u64,
}

/// A selection token with semantic identity and a unique lifecycle attempt.
/// Repacking keeps this token; a new selection gets a new attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SamplingJob {
    pub check: StableId,
    pub candidate: StableId,
    pub epoch: StableId,
    run: u64,
    requirements: StableId,
    attempt: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamplingCoverage {
    pub eligible: usize,
    pub completed: usize,
    pub remaining: usize,
}

impl SamplingProgress {
    pub fn new(limits: SamplingLimits) -> Result<Self, SamplingError> {
        if limits.checks == 0
            || limits.candidates_per_check == 0
            || limits.answers_per_candidate == 0
            || limits.judgments_per_run == 0
        {
            return Err(SamplingError::InvalidLimits);
        }
        Ok(Self {
            limits,
            checks: BTreeMap::new(),
            check_after: None,
            run: 0,
            remaining: 0,
            last_attempt: 0,
        })
    }

    /// Replace a check's bounded inventory in check-owned rank order. Keep IDs
    /// stable across appends. Change the epoch when semantic context changes.
    /// Removed candidates leave the ledger; reintroduced IDs need assessment.
    pub fn synchronize(
        &mut self,
        check: StableId,
        epoch: StableId,
        candidates: &[Candidate],
    ) -> Result<(), SamplingError> {
        let chronology: Vec<_> = candidates.iter().map(|candidate| candidate.id).collect();
        self.synchronize_ordered(check, epoch, candidates, &chronology)
    }

    /// Supply risk order separately from source chronology. Both lists contain
    /// the same inventory. Stable answer IDs bind the supplied decision context.
    pub fn synchronize_ordered(
        &mut self,
        check: StableId,
        epoch: StableId,
        candidates: &[Candidate],
        chronology: &[StableId],
    ) -> Result<(), SamplingError> {
        if candidates.len() > self.limits.candidates_per_check
            || (!self.checks.contains_key(&check) && self.checks.len() >= self.limits.checks)
        {
            return Err(SamplingError::Capacity);
        }
        let mut inventory = BTreeMap::new();
        for candidate in candidates {
            if candidate.required_answers.is_empty() {
                return Err(SamplingError::EmptyRequirements);
            }
            if candidate.required_answers.len() > self.limits.answers_per_candidate {
                return Err(SamplingError::Capacity);
            }
            let required: BTreeSet<_> = candidate.required_answers.iter().copied().collect();
            if required.len() != candidate.required_answers.len() {
                return Err(SamplingError::DuplicateIdentity);
            }
            if inventory
                .insert(
                    candidate.id,
                    CandidateProgress {
                        required,
                        reduced: BTreeSet::new(),
                        attempted: false,
                        introduced_run: self.run.saturating_add(1),
                        active_attempt: None,
                        complete: false,
                        terminal: false,
                        last_service: 0,
                    },
                )
                .is_some()
            {
                return Err(SamplingError::DuplicateIdentity);
            }
        }
        if chronology.len() != inventory.len()
            || chronology.iter().copied().collect::<BTreeSet<_>>()
                != inventory.keys().copied().collect()
        {
            return Err(SamplingError::DuplicateIdentity);
        }
        let progress = self.checks.entry(check).or_insert_with(|| CheckProgress {
            epoch,
            order: Vec::new(),
            chronology: Vec::new(),
            candidates: BTreeMap::new(),
            served: BTreeSet::new(),
            prefer_unchecked: false,
            diverse_stratum: 0,
        });
        for (id, candidate) in &mut inventory {
            if let Some(previous) = progress.candidates.get(id) {
                candidate.attempted = previous.attempted;
                candidate.introduced_run = previous.introduced_run;
                candidate.last_service = previous.last_service;
                if progress.epoch == epoch && previous.required == candidate.required {
                    candidate.reduced.clone_from(&previous.reduced);
                    candidate.complete = previous.complete;
                    candidate.terminal = previous.terminal;
                    candidate.active_attempt = previous.active_attempt;
                }
            }
        }
        // Context changes invalidate answers, but do not replenish this run.
        progress.epoch = epoch;
        progress.served.retain(|id| inventory.contains_key(id));
        progress.order = candidates.iter().map(|candidate| candidate.id).collect();
        progress.chronology = chronology.to_vec();
        progress.candidates = inventory;
        Ok(())
    }

    /// Call once per externally scheduled run. A restart resumes the saved run;
    /// it must not call this method just because the process restarted.
    pub fn begin_run(&mut self) {
        self.run = self.run.saturating_add(1);
        self.remaining = self.limits.judgments_per_run;
        for progress in self.checks.values_mut() {
            progress.served.clear();
            for candidate in progress.candidates.values_mut() {
                candidate.active_attempt = None;
            }
        }
    }

    /// Round-robin checks and alternate risk order with temporal diversity.
    /// A canceled job still consumes its slot.
    /// None means the main worker must wait for the next scheduler event.
    /// Counter exhaustion stops selection permanently; attempt IDs never wrap.
    pub fn choose_job(&mut self) -> Option<SamplingJob> {
        if self.remaining == 0 {
            return None;
        }
        let attempt = self.last_attempt.checked_add(1)?;
        let keys: Vec<_> = self.checks.keys().copied().collect();
        let start = self
            .check_after
            .and_then(|id| keys.iter().position(|key| *key == id))
            .map_or(0, |position| (position + 1) % keys.len());
        for offset in 0..keys.len() {
            let check = keys[(start + offset) % keys.len()];
            let progress = self.checks.get_mut(&check)?;
            if let Some(candidate) = progress.choose_candidate(self.run, attempt) {
                self.check_after = Some(check);
                self.remaining -= 1;
                self.last_attempt = attempt;
                return Some(SamplingJob {
                    check,
                    candidate,
                    epoch: progress.epoch,
                    run: self.run,
                    requirements: progress.candidates[&candidate].requirements_id(),
                    attempt,
                });
            }
        }
        None
    }

    /// Record only an accepted, reduced answer. Dispatch, provider errors, and
    /// unknown outcomes must not call this method.
    pub fn record_reduced_answer(
        &mut self,
        job: &SamplingJob,
        answer: AnswerId,
    ) -> Result<(), SamplingError> {
        let candidate = self.job_candidate(job)?;
        if !candidate.required.contains(&answer) {
            return Err(SamplingError::UnexpectedAnswer);
        }
        candidate.reduced.insert(answer);
        Ok(())
    }

    /// Commit completion only after the check's candidate reducer succeeds.
    pub fn complete_candidate(&mut self, job: &SamplingJob) -> Result<(), SamplingError> {
        let candidate = self.job_candidate(job)?;
        if candidate.required != candidate.reduced {
            return Err(SamplingError::IncompleteCandidate);
        }
        candidate.complete = true;
        candidate.active_attempt = None;
        Ok(())
    }

    /// Stop a canceled or interrupted candidate without completing it. Keep
    /// accepted partial answers, but reject later outcomes from this token.
    pub fn interrupt_candidate(&mut self, job: &SamplingJob) -> Result<(), SamplingError> {
        self.job_candidate(job)?.active_attempt = None;
        Ok(())
    }

    /// Stop unavailable work for this context without counting a model review.
    pub fn terminate_candidate(&mut self, job: &SamplingJob) -> Result<(), SamplingError> {
        let candidate = self.job_candidate(job)?;
        candidate.terminal = true;
        candidate.active_attempt = None;
        Ok(())
    }

    pub fn has_runnable_candidates(&self, check: StableId) -> bool {
        self.runnable_count(check) > 0
    }

    pub fn runnable_count(&self, check: StableId) -> usize {
        self.checks.get(&check).map_or(0, |progress| {
            progress
                .candidates
                .values()
                .filter(|candidate| !candidate.complete && !candidate.terminal)
                .count()
        })
    }

    /// Reuse only these semantic answers when constructing transport batches.
    pub fn reduced_answers(
        &self,
        check: StableId,
        candidate: StableId,
    ) -> Option<&BTreeSet<AnswerId>> {
        Some(&self.checks.get(&check)?.candidates.get(&candidate)?.reduced)
    }

    pub fn completed_ids(&self, check: StableId) -> BTreeSet<StableId> {
        self.checks
            .get(&check)
            .map_or_else(BTreeSet::new, |progress| {
                progress
                    .candidates
                    .iter()
                    .filter_map(|(id, candidate)| candidate.complete.then_some(*id))
                    .collect()
            })
    }

    pub fn coverage(&self, check: StableId) -> Option<SamplingCoverage> {
        let progress = self.checks.get(&check)?;
        let eligible = progress.candidates.len();
        let completed = progress
            .candidates
            .values()
            .filter(|candidate| candidate.complete)
            .count();
        Some(SamplingCoverage {
            eligible,
            completed,
            remaining: eligible - completed,
        })
    }

    fn job_candidate(
        &mut self,
        job: &SamplingJob,
    ) -> Result<&mut CandidateProgress, SamplingError> {
        let progress = self
            .checks
            .get_mut(&job.check)
            .ok_or(SamplingError::StaleJob)?;
        if job.run != self.run
            || job.epoch != progress.epoch
            || !progress.served.contains(&job.candidate)
        {
            return Err(SamplingError::StaleJob);
        }
        let candidate = progress
            .candidates
            .get_mut(&job.candidate)
            .ok_or(SamplingError::StaleJob)?;
        if candidate.active_attempt != Some(job.attempt)
            || candidate.requirements_id() != job.requirements
        {
            return Err(SamplingError::StaleJob);
        }
        Ok(candidate)
    }
}

impl CheckProgress {
    fn choose_candidate(&mut self, run: u64, attempt: u64) -> Option<StableId> {
        let eligible = |id: &&StableId| {
            !self.served.contains(id)
                && !self.candidates[*id].complete
                && !self.candidates[*id].terminal
        };
        let new = self
            .order
            .iter()
            .filter(eligible)
            .find(|id| self.candidates[id].is_new(run))
            .copied();
        let risk = new
            .or_else(|| {
                self.order
                    .iter()
                    .filter(eligible)
                    .find(|id| !self.candidates[id].attempted)
                    .copied()
            })
            .or_else(|| self.order.iter().find(eligible).copied());
        let diverse = (0..4)
            .filter_map(|stratum| {
                let items: Vec<_> = self
                    .chronology
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| index * 4 / self.chronology.len() == stratum)
                    .map(|(_, id)| id)
                    .collect();
                let reviewed = items
                    .iter()
                    .filter(|id| self.candidates[id].complete)
                    .count();
                let candidate = items
                    .into_iter()
                    .filter(eligible)
                    .min_by_key(|id| (self.candidates[id].last_service, **id))
                    .copied()?;
                Some((
                    reviewed,
                    (stratum + 4 - self.diverse_stratum) % 4,
                    stratum,
                    candidate,
                ))
            })
            .min()
            .map(|(_, _, stratum, id)| (stratum, id));
        let selected = if self.prefer_unchecked {
            if let Some((stratum, id)) = diverse {
                self.diverse_stratum = (stratum + 1) % 4;
                Some(id)
            } else {
                risk
            }
        } else {
            risk.or(diverse.map(|(_, id)| id))
        }?;
        self.prefer_unchecked = !self.prefer_unchecked;
        self.candidates.get_mut(&selected)?.attempted = true;
        self.candidates.get_mut(&selected)?.last_service = attempt;
        self.candidates.get_mut(&selected)?.active_attempt = Some(attempt);
        self.served.insert(selected);
        Some(selected)
    }
}

impl CandidateProgress {
    fn is_new(&self, run: u64) -> bool {
        !self.attempted && self.introduced_run >= run
    }
    fn requirements_id(&self) -> StableId {
        let parts: Vec<_> = self.required.iter().map(|id| id.0.as_slice()).collect();
        StableId::new("sampling-requirements", &parts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> StableId {
        StableId::new("test", &[value.as_bytes()])
    }

    fn candidate(value: &str) -> Candidate {
        Candidate {
            id: id(value),
            required_answers: vec![
                id("window1-question1"),
                id("window1-question2"),
                id("window2-question1"),
            ],
        }
    }

    fn progress(budget: usize) -> SamplingProgress {
        SamplingProgress::new(SamplingLimits {
            checks: 4,
            candidates_per_check: 16,
            answers_per_candidate: 4,
            judgments_per_run: budget,
        })
        .unwrap()
    }

    fn synchronize(progress: &mut SamplingProgress, candidates: &[Candidate]) {
        progress
            .synchronize(id("check"), id("epoch"), candidates)
            .unwrap();
    }

    fn finish(progress: &mut SamplingProgress, job: &SamplingJob, candidate: &Candidate) {
        for answer in &candidate.required_answers {
            progress.record_reduced_answer(job, *answer).unwrap();
        }
        progress.complete_candidate(job).unwrap();
    }

    #[test]
    fn identities_are_stable_and_length_delimited() {
        assert_eq!(id("candidate"), id("candidate"));
        assert_ne!(StableId::new("a", &[b"bc"]), StableId::new("ab", &[b"c"]));
        assert_ne!(
            StableId::new("a", &[b"b", b"c"]),
            StableId::new("a", &[b"bc"])
        );
    }

    #[test]
    fn completion_requires_all_windows_questions_and_final_reduction() {
        let mut progress = progress(2);
        let candidate = candidate("one");
        synchronize(&mut progress, std::slice::from_ref(&candidate));
        progress.begin_run();
        let job = progress.choose_job().unwrap();
        assert_eq!(
            progress.complete_candidate(&job),
            Err(SamplingError::IncompleteCandidate)
        );
        for answer in &candidate.required_answers[..2] {
            progress.record_reduced_answer(&job, *answer).unwrap();
        }
        assert_eq!(
            progress.complete_candidate(&job),
            Err(SamplingError::IncompleteCandidate)
        );
        assert_eq!(progress.coverage(job.check).unwrap().completed, 0);
        progress
            .record_reduced_answer(&job, candidate.required_answers[2])
            .unwrap();
        assert!(progress.completed_ids(job.check).is_empty());
        progress.complete_candidate(&job).unwrap();
        assert_eq!(
            progress.completed_ids(job.check),
            BTreeSet::from([candidate.id])
        );
        assert_eq!(
            progress.coverage(job.check),
            Some(SamplingCoverage {
                eligible: 1,
                completed: 1,
                remaining: 0
            })
        );
        assert!(progress.choose_job().is_none());
    }

    #[test]
    fn restart_preserves_partial_answers_budget_and_completed_ledger() {
        let mut progress = progress(2);
        let first = candidate("first");
        let second = candidate("second");
        synchronize(&mut progress, &[first.clone(), second.clone()]);
        progress.begin_run();
        let job = progress.choose_job().unwrap();
        progress
            .record_reduced_answer(&job, first.required_answers[0])
            .unwrap();
        let encoded = serde_json::to_vec(&progress).unwrap();
        let mut restarted: SamplingProgress = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(restarted, progress);
        let second_job = restarted.choose_job().unwrap();
        assert_eq!(second_job.candidate, second.id);
        finish(&mut restarted, &second_job, &second);
        assert!(restarted.choose_job().is_none());
        restarted.begin_run();
        assert_eq!(
            restarted.record_reduced_answer(&job, first.required_answers[1]),
            Err(SamplingError::StaleJob)
        );
        let retry = restarted.choose_job().unwrap();
        assert_eq!(retry.candidate, first.id);
        assert_eq!(
            restarted
                .reduced_answers(retry.check, first.id)
                .unwrap()
                .len(),
            1
        );
        finish(&mut restarted, &retry, &first);
        let saved = postcard::to_stdvec(&restarted).unwrap();
        let mut restarted: SamplingProgress = postcard::from_bytes(&saved).unwrap();
        restarted.begin_run();
        assert!(restarted.choose_job().is_none());
        assert_eq!(restarted.coverage(id("check")).unwrap().completed, 2);
    }

    #[test]
    fn cancellation_cannot_hot_loop_or_complete_on_dispatch() {
        let mut progress = progress(10);
        synchronize(&mut progress, &[candidate("first")]);
        progress.begin_run();
        let job = progress.choose_job().unwrap();
        progress.interrupt_candidate(&job).unwrap();
        assert_eq!(
            progress.record_reduced_answer(&job, id("window1-question1")),
            Err(SamplingError::StaleJob)
        );
        for _ in 0..100 {
            assert!(progress.choose_job().is_none());
        }
        assert_eq!(progress.coverage(job.check).unwrap().remaining, 1);
        progress.begin_run();
        assert_eq!(progress.choose_job().unwrap().candidate, job.candidate);
    }

    #[test]
    fn removed_reintroduced_candidate_rejects_old_attempt_in_same_run() {
        let mut progress = progress(3);
        let candidate = candidate("one");
        synchronize(&mut progress, std::slice::from_ref(&candidate));
        progress.begin_run();
        let old_job = progress.choose_job().unwrap();
        progress.interrupt_candidate(&old_job).unwrap();
        synchronize(&mut progress, &[]);
        synchronize(&mut progress, std::slice::from_ref(&candidate));
        let new_job = progress.choose_job().unwrap();
        assert_eq!(old_job.run, new_job.run);
        assert_eq!(old_job.epoch, new_job.epoch);
        assert_eq!(old_job.requirements, new_job.requirements);
        assert!(new_job.attempt > old_job.attempt);

        let before = progress.clone();
        assert_eq!(
            progress.record_reduced_answer(&old_job, candidate.required_answers[0]),
            Err(SamplingError::StaleJob)
        );
        assert_eq!(
            progress.interrupt_candidate(&old_job),
            Err(SamplingError::StaleJob)
        );
        assert_eq!(progress, before);
        for answer in &candidate.required_answers {
            progress.record_reduced_answer(&new_job, *answer).unwrap();
        }
        assert_eq!(
            progress.complete_candidate(&old_job),
            Err(SamplingError::StaleJob)
        );
        assert!(progress.completed_ids(old_job.check).is_empty());
        progress.complete_candidate(&new_job).unwrap();
    }

    #[test]
    fn serde_resume_preserves_global_attempt_counter_and_active_token() {
        let mut progress = progress(4);
        let candidate = candidate("one");
        synchronize(&mut progress, std::slice::from_ref(&candidate));
        progress.begin_run();
        let old_job = progress.choose_job().unwrap();
        progress.interrupt_candidate(&old_job).unwrap();
        synchronize(&mut progress, &[]);
        let saved = postcard::to_stdvec(&progress).unwrap();
        let mut resumed: SamplingProgress = postcard::from_bytes(&saved).unwrap();
        synchronize(&mut resumed, std::slice::from_ref(&candidate));
        let new_job = resumed.choose_job().unwrap();
        assert_eq!(new_job.attempt, old_job.attempt + 1);
        let saved = serde_json::to_vec(&resumed).unwrap();
        let mut resumed: SamplingProgress = serde_json::from_slice(&saved).unwrap();
        let saved_job = serde_json::to_vec(&new_job).unwrap();
        let new_job: SamplingJob = serde_json::from_slice(&saved_job).unwrap();
        assert_eq!(
            resumed.complete_candidate(&old_job),
            Err(SamplingError::StaleJob)
        );
        finish(&mut resumed, &new_job, &candidate);

        resumed
            .synchronize(
                id("second-check"),
                id("epoch"),
                std::slice::from_ref(&candidate),
            )
            .unwrap();
        resumed.begin_run();
        let other_job = resumed.choose_job().unwrap();
        assert_eq!(other_job.check, id("second-check"));
        assert_eq!(other_job.attempt, new_job.attempt + 1);
    }

    #[test]
    fn attempt_counter_exhaustion_never_wraps_or_mutates_selection_state() {
        let mut progress = progress(3);
        let candidate = candidate("one");
        synchronize(&mut progress, std::slice::from_ref(&candidate));
        progress.last_attempt = u64::MAX - 1;
        progress.begin_run();
        let final_job = progress.choose_job().unwrap();
        assert_eq!(final_job.attempt, u64::MAX);
        progress.interrupt_candidate(&final_job).unwrap();
        synchronize(&mut progress, &[]);
        synchronize(&mut progress, std::slice::from_ref(&candidate));
        let before = progress.clone();
        assert!(progress.choose_job().is_none());
        assert_eq!(progress, before);
        let saved = serde_json::to_vec(&progress).unwrap();
        let mut resumed: SamplingProgress = serde_json::from_slice(&saved).unwrap();
        resumed.begin_run();
        let before = resumed.clone();
        assert!(resumed.choose_job().is_none());
        assert_eq!(resumed, before);
        assert_eq!(resumed.last_attempt, u64::MAX);
        assert_eq!(
            resumed.complete_candidate(&final_job),
            Err(SamplingError::StaleJob)
        );
    }

    #[test]
    fn context_changes_requeue_inventory_and_reject_stale_answers() {
        let mut progress = progress(4);
        let candidate = candidate("one");
        synchronize(&mut progress, std::slice::from_ref(&candidate));
        progress.begin_run();
        let old_job = progress.choose_job().unwrap();
        finish(&mut progress, &old_job, &candidate);
        progress
            .synchronize(
                id("check"),
                id("new-scope"),
                std::slice::from_ref(&candidate),
            )
            .unwrap();
        assert_eq!(progress.coverage(old_job.check).unwrap().remaining, 1);
        assert!(
            progress
                .reduced_answers(old_job.check, candidate.id)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            progress.record_reduced_answer(&old_job, candidate.required_answers[0]),
            Err(SamplingError::StaleJob)
        );
        assert!(progress.choose_job().is_none());
        progress.begin_run();
        assert_eq!(progress.choose_job().unwrap().candidate, candidate.id);
    }

    #[test]
    fn unchanged_semantics_reuse_answers_across_transport_repacking() {
        let mut progress = progress(1);
        let candidate = candidate("one");
        synchronize(&mut progress, std::slice::from_ref(&candidate));
        progress.begin_run();
        let job = progress.choose_job().unwrap();
        progress
            .record_reduced_answer(&job, candidate.required_answers[0])
            .unwrap();
        // Packing order belongs to the caller and does not enter semantic IDs.
        let mut repacked = candidate.clone();
        repacked.required_answers.reverse();
        synchronize(&mut progress, &[repacked]);
        assert_eq!(
            progress.reduced_answers(job.check, candidate.id).unwrap(),
            &BTreeSet::from([candidate.required_answers[0]])
        );
        finish(&mut progress, &job, &candidate);
        synchronize(&mut progress, &[candidate]);
        assert_eq!(progress.coverage(job.check).unwrap().completed, 1);
    }

    #[test]
    fn changed_evidence_or_questions_invalidate_candidate_answers() {
        let mut progress = progress(1);
        let mut candidate = candidate("one");
        synchronize(&mut progress, std::slice::from_ref(&candidate));
        progress.begin_run();
        let job = progress.choose_job().unwrap();
        finish(&mut progress, &job, &candidate);
        candidate.required_answers[0] = id("changed-evidence-question");
        synchronize(&mut progress, std::slice::from_ref(&candidate));
        assert!(progress.completed_ids(job.check).is_empty());
        assert!(
            progress
                .reduced_answers(job.check, candidate.id)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            progress.record_reduced_answer(&job, candidate.required_answers[1]),
            Err(SamplingError::StaleJob)
        );
    }

    #[test]
    fn four_checks_share_one_bounded_main_run_without_starvation() {
        let mut progress = progress(1);
        let checks = [
            id("ignored-instructions"),
            id("over-exploring"),
            id("scope-creep"),
            id("skill-opportunities"),
        ];
        for check in checks {
            progress
                .synchronize(check, id("epoch"), &[candidate("one"), candidate("two")])
                .unwrap();
        }
        let mut first_cycle = BTreeSet::new();
        let mut second_cycle = BTreeSet::new();
        for index in 0..8 {
            progress.begin_run();
            let job = progress.choose_job().unwrap();
            if index < 4 {
                first_cycle.insert(job.check);
            } else {
                second_cycle.insert(job.check);
            }
            assert!(progress.choose_job().is_none());
            let saved = serde_json::to_vec(&progress).unwrap();
            progress = serde_json::from_slice(&saved).unwrap();
        }
        assert_eq!(first_cycle, BTreeSet::from(checks));
        assert_eq!(second_cycle, first_cycle);
    }

    #[test]
    fn new_work_cannot_starve_older_unchecked_candidates() {
        let mut progress = progress(1);
        let old = [candidate("old1"), candidate("old2"), candidate("old3")];
        synchronize(&mut progress, &old);
        for _ in 0..3 {
            progress.begin_run();
            progress.choose_job().unwrap();
        }
        let mut selected_old = BTreeSet::new();
        for run in 0..8 {
            let mut inventory = vec![candidate(&format!("new-{run}"))];
            inventory.extend(old.clone());
            synchronize(&mut progress, &inventory);
            progress.begin_run();
            let job = progress.choose_job().unwrap();
            if old.iter().any(|candidate| candidate.id == job.candidate) {
                selected_old.insert(job.candidate);
            }
        }
        assert_eq!(
            selected_old,
            old.iter().map(|candidate| candidate.id).collect()
        );
    }

    #[test]
    fn risk_order_alternates_with_time_strata() {
        let mut progress = progress(3);
        let chronological: Vec<_> = (0..8)
            .map(|index| candidate(&format!("time-{index}")))
            .collect();
        let ranked: Vec<_> = chronological.iter().rev().cloned().collect();
        progress
            .synchronize_ordered(
                id("check"),
                id("epoch"),
                &ranked,
                &chronological
                    .iter()
                    .map(|candidate| candidate.id)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        progress.begin_run();
        let first = progress.choose_job().unwrap();
        assert_eq!(first.candidate, ranked[0].id);
        finish(&mut progress, &first, &ranked[0]);
        let diverse = progress.choose_job().unwrap();
        assert!(
            !chronological[6..]
                .iter()
                .any(|candidate| candidate.id == diverse.candidate)
        );
        assert_eq!(progress.choose_job().unwrap().candidate, ranked[1].id);
    }

    #[test]
    fn terminal_unavailable_is_not_reviewed_and_reopens_only_on_context_change() {
        let mut progress = progress(2);
        let candidate = candidate("one");
        synchronize(&mut progress, std::slice::from_ref(&candidate));
        progress.begin_run();
        let job = progress.choose_job().unwrap();
        progress.terminate_candidate(&job).unwrap();
        let encoded = postcard::to_stdvec(&progress).unwrap();
        let mut progress: SamplingProgress = postcard::from_bytes(&encoded).unwrap();
        synchronize(&mut progress, std::slice::from_ref(&candidate));
        progress.begin_run();
        assert!(progress.choose_job().is_none());
        assert!(!progress.has_runnable_candidates(job.check));
        assert_eq!(progress.coverage(job.check).unwrap().completed, 0);
        assert_eq!(progress.coverage(job.check).unwrap().remaining, 1);
        progress
            .synchronize(job.check, id("changed-context"), &[candidate])
            .unwrap();
        assert!(progress.has_runnable_candidates(job.check));
        assert!(progress.choose_job().is_some());
    }

    #[test]
    fn invalid_chronology_does_not_replace_inventory() {
        let mut progress = progress(2);
        let inventory = [candidate("one"), candidate("two")];
        synchronize(&mut progress, &inventory);
        let before = progress.clone();
        assert_eq!(
            progress.synchronize_ordered(
                id("check"),
                id("epoch"),
                &inventory,
                &[inventory[0].id, inventory[0].id]
            ),
            Err(SamplingError::DuplicateIdentity)
        );
        assert_eq!(progress, before);
    }

    #[test]
    fn appended_inventory_preserves_completion_and_prioritizes_new_work() {
        let mut progress = progress(1);
        let completed = candidate("completed");
        let unchecked = candidate("unchecked");
        synchronize(&mut progress, &[completed.clone(), unchecked.clone()]);
        progress.begin_run();
        let job = progress.choose_job().unwrap();
        finish(&mut progress, &job, &completed);
        let new = candidate("new");
        synchronize(&mut progress, &[completed.clone(), unchecked, new.clone()]);
        progress.begin_run();
        // The reserved older-work turn must survive the append.
        let older_job = progress.choose_job().unwrap();
        assert_ne!(older_job.candidate, new.id);
        progress.interrupt_candidate(&older_job).unwrap();
        progress.begin_run();
        // The previously new candidate is now unchecked and still progresses.
        assert_eq!(progress.choose_job().unwrap().candidate, new.id);
        assert_eq!(
            progress.completed_ids(job.check),
            BTreeSet::from([completed.id])
        );
    }

    #[test]
    fn never_dispatched_old_work_progresses_despite_new_arrivals() {
        let mut progress = progress(1);
        let old = [candidate("old1"), candidate("old2"), candidate("old3")];
        synchronize(&mut progress, &old);
        progress.begin_run();
        progress.choose_job().unwrap();
        let mut selected_old = BTreeSet::new();
        for run in 0..8 {
            let mut inventory = vec![candidate(&format!("new-{run}"))];
            inventory.extend(old.clone());
            synchronize(&mut progress, &inventory);
            progress.begin_run();
            let job = progress.choose_job().unwrap();
            if old.iter().any(|candidate| candidate.id == job.candidate) {
                selected_old.insert(job.candidate);
            }
        }
        assert_eq!(
            selected_old,
            old.iter().map(|candidate| candidate.id).collect()
        );
    }

    #[test]
    fn inventory_limits_are_transactional_and_serialized_state_is_bounded() {
        let mut progress = progress(16);
        let inventory: Vec<_> = (0..16)
            .map(|index| candidate(&format!("candidate-{index}")))
            .collect();
        for check in ["a", "b", "c", "d"] {
            progress
                .synchronize(id(check), id("epoch"), &inventory)
                .unwrap();
        }
        let before = progress.clone();
        assert_eq!(
            progress.synchronize(id("fifth"), id("epoch"), &inventory),
            Err(SamplingError::Capacity)
        );
        let mut too_many = inventory.clone();
        too_many.push(candidate("overflow"));
        assert_eq!(
            progress.synchronize(id("a"), id("epoch"), &too_many),
            Err(SamplingError::Capacity)
        );
        assert_eq!(progress, before);
        let initial_size = serde_json::to_vec(&progress).unwrap().len();
        for _ in 0..20 {
            progress.begin_run();
            while let Some(job) = progress.choose_job() {
                let candidate = inventory
                    .iter()
                    .find(|candidate| candidate.id == job.candidate)
                    .unwrap();
                finish(&mut progress, &job, candidate);
            }
        }
        assert!(serde_json::to_vec(&progress).unwrap().len() < initial_size + 30_000);
        assert!(serde_json::to_vec(&progress).unwrap().len() < 80_000);
    }

    #[test]
    fn invalid_requirements_and_unexpected_answers_do_not_change_progress() {
        let mut progress = progress(1);
        let candidate = candidate("one");
        synchronize(&mut progress, std::slice::from_ref(&candidate));
        let before = progress.clone();
        for (required_answers, error) in [
            (vec![], SamplingError::EmptyRequirements),
            (vec![id("same"); 2], SamplingError::DuplicateIdentity),
            (vec![id("many"); 5], SamplingError::Capacity),
        ] {
            assert_eq!(
                progress.synchronize(
                    id("check"),
                    id("epoch"),
                    &[Candidate {
                        id: candidate.id,
                        required_answers
                    }]
                ),
                Err(error)
            );
            assert_eq!(progress, before);
        }
        assert_eq!(
            progress.synchronize(id("check"), id("epoch"), &[candidate.clone(), candidate]),
            Err(SamplingError::DuplicateIdentity)
        );
        progress.begin_run();
        let job = progress.choose_job().unwrap();
        assert_eq!(
            progress.record_reduced_answer(&job, id("unexpected")),
            Err(SamplingError::UnexpectedAnswer)
        );
        assert!(
            progress
                .reduced_answers(job.check, job.candidate)
                .unwrap()
                .is_empty()
        );
    }
}
