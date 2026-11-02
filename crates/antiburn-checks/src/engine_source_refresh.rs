use super::*;

impl<S: CheckStore, E: DecisionEvaluator> CheckEngine<S, E> {
    /// Reloads a checkpoint after an atomic host-owned startup seed.
    pub fn refresh_source_progress(
        &self,
        activation: &EffectiveActivation,
    ) -> Result<(), StoreError> {
        let mut selectors = self
            .selectors
            .lock()
            .expect("selector mutex is not poisoned");
        let progress = self.store.load_source_progress(activation)?;
        selectors.insert(activation.id.clone(), progress);
        Ok(())
    }
}
