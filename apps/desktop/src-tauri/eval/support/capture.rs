use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub(crate) fn repository() -> PathBuf {
    std::env::var_os("ANTIBURN_EVAL_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."))
}

pub(crate) fn directory(check: &str, suite: &str) -> PathBuf {
    std::env::var_os("ANTIBURN_EVAL_OUTPUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository().join("target/eval-captures"))
        .join(check)
        .join(suite)
}

pub(crate) fn report(check: &str, suite: &str, value: &Value) -> PathBuf {
    let directory = directory(check, suite);
    std::fs::create_dir_all(&directory).expect("Create evaluation output directory");
    let path = directory.join(format!(
        "{}.json",
        time::OffsetDateTime::now_utc().unix_timestamp_nanos()
    ));
    write_new(&path, value).expect("Write a new report and preserve earlier captures");
    println!("Report: {}", path.display());
    path
}

fn write_new(path: &Path, value: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).expect("capture serializes");
    assert!(bytes.len() <= 16 * 1024 * 1024, "Report exceeds byte limit");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()
}
