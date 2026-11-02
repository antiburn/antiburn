use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    BeforeActivationRegistration,
    BeforeCancellation,
    BeforeAnswer,
    AfterAnswer,
    BeforeAcceptance,
    AfterRecoveryPage,
    AfterRecoveryProgress,
}

pub(crate) type Hook = Arc<dyn Fn(Stage) + Send + Sync>;
pub(crate) type Hooks = Arc<Mutex<Option<Hook>>>;

impl crate::store::Store {
    pub(crate) fn set_burn_check_test_hook(&self, hook: Option<Hook>) {
        *self.burn_check_test_hooks.lock().unwrap() = hook;
    }

    pub(crate) fn burn_check_test_stage(&self, stage: Stage) {
        let hook = self.burn_check_test_hooks.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(stage);
        }
    }
}
