#[path = "../support/mod.rs"]
mod support;
pub(crate) use support::jev_cloudflare;
pub(crate) mod jev {
    pub(crate) use crate::support::config;
}

#[path = "../support/baseline.rs"]
mod baseline;
mod controls;
mod development;
mod development_extents;
mod fixtures;
mod harness;
mod native;
mod scoring;

#[tokio::test]
#[ignore = "Authorized live diagnostic through the selected production provider"]
async fn live() -> Result<(), String> {
    harness::run().await
}
