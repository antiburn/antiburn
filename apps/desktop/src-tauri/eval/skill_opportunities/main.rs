#[path = "../support/mod.rs"]
mod support;
pub(crate) use support::jev_cloudflare;
pub(crate) mod jev {
    pub(crate) use crate::support::config;
}

#[path = "../support/baseline.rs"]
mod baseline;
mod fixtures;
mod harness;
mod scoring;

#[tokio::test]
#[ignore = "Authorized live diagnostic through the selected production provider"]
async fn live() -> Result<(), String> {
    harness::run().await
}

#[tokio::test]
#[ignore = "Authorized live full-suite session benchmark through the selected production provider"]
async fn benchmark_session() -> Result<(), String> {
    harness::benchmark_session().await
}
