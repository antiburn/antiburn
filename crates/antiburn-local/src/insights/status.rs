/// Identifies one exclusive coverage bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageBucket {
    UnknownStart,
    Pending,
    Processing,
    Failed,
    Unsupported,
    Stale,
    Ready,
}
