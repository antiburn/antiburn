use serde_json::Value;

#[derive(Default)]
pub(crate) struct RunUsage {
    pub(crate) requests: u64,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) calls: Vec<Value>,
}

pub(crate) struct LiveRunGuard(std::path::PathBuf);

impl LiveRunGuard {
    pub(crate) fn acquire() -> Self {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../.agent-artifacts/reviews/jev-live-evaluation.lock");
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("another live evaluation owns the lock; a stale lock requires review");
        Self(path)
    }
}

impl Drop for LiveRunGuard {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.0) {
            eprintln!("Cannot release live evaluation lock: {error}");
        }
    }
}
