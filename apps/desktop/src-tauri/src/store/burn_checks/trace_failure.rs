#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct FailureCause {
    pub category: &'static str,
    pub sqlite_code: Option<i32>,
    pub io_kind: Option<std::io::ErrorKind>,
}

impl FailureCause {
    pub(crate) fn classify(error: &anyhow::Error) -> Self {
        error
            .chain()
            .find_map(Self::from_cause)
            .unwrap_or_else(Self::invariant)
    }

    fn invariant() -> Self {
        Self {
            category: "runtime_invariant",
            sqlite_code: None,
            io_kind: None,
        }
    }

    fn from_cause(cause: &(dyn std::error::Error + 'static)) -> Option<Self> {
        let mut failure = Self::invariant();
        if let Some(sqlite) = cause.downcast_ref::<rusqlite::Error>() {
            failure.category = "sqlite";
            if let rusqlite::Error::SqliteFailure(code, _) = sqlite {
                failure.sqlite_code = Some(code.extended_code);
            }
            return Some(failure);
        }
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            failure.category = "io";
            failure.io_kind = Some(io.kind());
            return Some(failure);
        }
        if cause.is::<serde_json::Error>() {
            failure.category = "saved_data_invalid";
            return Some(failure);
        }
        if cause.is::<antiburn_checks::check_store::StoreError>() {
            failure.category = "store_contract";
            return Some(failure);
        }
        None
    }

    pub(crate) fn classify_value<T: std::any::Any>(error: &T) -> Self {
        let value = error as &dyn std::any::Any;
        if let Some(error) = value.downcast_ref::<anyhow::Error>() {
            return Self::classify(error);
        }
        let cause: Option<&(dyn std::error::Error + 'static)> = value
            .downcast_ref::<rusqlite::Error>()
            .map(|error| error as &_)
            .or_else(|| {
                value
                    .downcast_ref::<std::io::Error>()
                    .map(|error| error as &_)
            })
            .or_else(|| {
                value
                    .downcast_ref::<serde_json::Error>()
                    .map(|error| error as &_)
            })
            .or_else(|| {
                value
                    .downcast_ref::<antiburn_checks::check_store::StoreError>()
                    .map(|error| error as &_)
            });
        cause
            .and_then(Self::from_cause)
            .unwrap_or_else(Self::invariant)
    }
}

#[derive(Default)]
pub(crate) struct FailureLog(
    std::sync::Mutex<std::collections::BTreeMap<(String, &'static str), FailureCause>>,
);

impl FailureLog {
    pub(crate) fn changed(
        &self,
        reference: String,
        stage: &'static str,
        cause: Option<FailureCause>,
    ) -> bool {
        let mut previous = self.0.lock().expect("failure trace lock");
        let key = (reference, stage);
        let Some(cause) = cause else {
            previous.remove(&key);
            return false;
        };
        if previous.get(&key) == Some(&cause)
            || previous.len() >= 128 && !previous.contains_key(&key)
        {
            return false;
        }
        previous.insert(key, cause);
        true
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.lock().expect("failure trace lock").is_empty()
    }
}
