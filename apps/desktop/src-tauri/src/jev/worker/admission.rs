use super::{WorkerHandle, batch_request_identities, unix_now};
use crate::store::{BurnCheckInput, BurnCheckRequestAdmission, Store};
use antiburn_local::analysis::jev::capabilities::ModelCapabilities;
use antiburn_local::analysis::jev::{JevCheckPlan, JevError, JevRunProgress};

pub(crate) fn preflight_admission(admission: BurnCheckRequestAdmission) -> Result<(), JevError> {
    match admission {
        BurnCheckRequestAdmission::Admitted | BurnCheckRequestAdmission::Exhausted => Ok(()),
        BurnCheckRequestAdmission::Deferred | BurnCheckRequestAdmission::Stale => {
            Err(JevError::Cancelled)
        }
        BurnCheckRequestAdmission::Unresolved => Err(JevError::RequestOutcomeUnknown),
    }
}

pub(crate) fn dispatch_readiness<P>(
    store: &Store,
    handle: &WorkerHandle,
    input: &BurnCheckInput,
    capabilities: &ModelCapabilities,
    plan: &JevCheckPlan<P>,
    progress: &JevRunProgress,
) -> anyhow::Result<BurnCheckRequestAdmission> {
    let mut connection = handle.system_one_connection();
    connection
        .model_revision
        .clone_from(&capabilities.model_revision);
    let mut readiness = BurnCheckRequestAdmission::Admitted;
    for item in &plan.work_items {
        if progress.results.contains_key(&item.id) {
            continue;
        }
        let packed = plan.pack(std::slice::from_ref(item));
        if packed.batches.is_empty() {
            return Ok(BurnCheckRequestAdmission::Exhausted);
        }
        for batch in &packed.batches {
            let identities = batch_request_identities(&connection, input, batch);
            match store.burn_check_dispatch_readiness(&identities, unix_now())? {
                BurnCheckRequestAdmission::Admitted => {}
                BurnCheckRequestAdmission::Deferred => {
                    readiness = BurnCheckRequestAdmission::Deferred
                }
                blocked => return Ok(blocked),
            }
        }
    }
    if plan.work_items.is_empty() {
        return Ok(BurnCheckRequestAdmission::Exhausted);
    }
    Ok(readiness)
}

pub(crate) fn target_failure_is_terminal(
    error: &JevError,
    readiness: impl FnOnce() -> anyhow::Result<BurnCheckRequestAdmission>,
) -> anyhow::Result<bool> {
    if matches!(error, JevError::AuthenticationRejected) {
        return Ok(false);
    }
    if matches!(
        error,
        JevError::RequestTooLarge { .. } | JevError::RequestTokenLimitExceeded { .. }
    ) {
        return Ok(true);
    }
    Ok(matches!(
        readiness()?,
        BurnCheckRequestAdmission::Exhausted | BurnCheckRequestAdmission::Unresolved
    ))
}
