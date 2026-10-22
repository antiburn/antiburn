//! Real WSL transport acceptance with a synthetic service, not a producer smoke.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use antiburn_local::analysis::{
    NormalizedRecord, RawSource, RecordSink, SessionInput, SessionSummary, SourceFormat,
    reader_for_input,
};
use antiburn_local::discovery::SessionSource;
use antiburn_local::discovery::agents::opencode::discover_recent_in_wsl;
use antiburn_local::platform::environment::{discover_wsl_environments, wsl_to_windows_path};
use serde_json::Value;
use tokio::io::AsyncWriteExt;

const SERVER: &str = include_str!("fixtures/opencode_wsl_acceptance/server.py");

struct Service {
    home: PathBuf,
    child: tokio::process::Child,
}

impl Drop for Service {
    fn drop(&mut self) {
        // The Linux child also has a 120-second deadline if the host process stops.
        let _ = std::fs::write(self.home.join("stop"), b"");
        let _ = std::fs::remove_file(self.home.join(".local/state/opencode/service.json"));
    }
}

async fn wait_for_file(path: &Path, child: &mut tokio::process::Child) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "WSL service stopped early"
        );
        assert!(
            Instant::now() < deadline,
            "WSL service did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[derive(Default)]
struct Sink {
    native_results: usize,
    gaps: usize,
}

impl RecordSink for Sink {
    fn record(&mut self, record: NormalizedRecord) {
        self.native_results += usize::from(matches!(record, NormalizedRecord::ToolResult(_)));
    }

    fn tool_result_gap(&mut self) {
        self.gaps += 1;
    }

    fn finish(&mut self, _: SessionSummary) {}
}

#[tokio::test]
#[ignore = "requires a new isolated WSL distro; run scripts/run-opencode-wsl-acceptance.ps1"]
async fn production_bridge_reads_registered_wsl_service_without_starting_opencode() {
    assert_eq!(
        std::env::consts::OS,
        "windows",
        "This test requires native Windows"
    );
    let distribution = std::env::var("ANTIBURN_WSL_ACCEPTANCE_DISTRIBUTION")
        .expect("Set the isolated acceptance distribution explicitly");
    let suffix = distribution
        .strip_prefix("AntiburnAcceptance-")
        .expect("Refuse a distribution outside the acceptance namespace");
    assert!(!suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let home = wsl_to_windows_path(&distribution, "/home/antiburn").unwrap();
    assert!(!home.join(".local/state/opencode/service.json").exists());
    assert!(
        !home.join("ready.json").exists(),
        "Use a new acceptance profile"
    );

    let child = tokio::process::Command::new("wsl.exe")
        .args([
            "--distribution",
            &distribution,
            "--user",
            "antiburn",
            "--exec",
            "python3",
            "-u",
            "-",
            "/home/antiburn",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("Start only the isolated synthetic Python service");
    let mut service = Service { home, child };
    let mut stdin = service.child.stdin.take().unwrap();
    stdin.write_all(SERVER.as_bytes()).await.unwrap();
    stdin.shutdown().await.unwrap();
    drop(stdin);
    wait_for_file(&service.home.join("ready.json"), &mut service.child).await;

    let environments = discover_wsl_environments().await;
    let info = environments
        .iter()
        .find(|info| info.distribution.eq_ignore_ascii_case(&distribution))
        .expect("Production mounted-WSL discovery must find the isolated default user");
    assert_eq!(info.user, "antiburn");
    assert_eq!(info.context.platform_home, Path::new("/home/antiburn"));
    assert_eq!(info.context.home, service.home);
    let ready: Value =
        serde_json::from_slice(&std::fs::read(service.home.join("ready.json")).unwrap()).unwrap();
    let registration_path = service.home.join(".local/state/opencode/service.json");
    let registration_before = std::fs::read(&registration_path).unwrap();
    let started = Instant::now();
    let logs = tokio::time::timeout(
        Duration::from_secs(60),
        discover_recent_in_wsl(info, ready["updated"].as_i64().unwrap(), 60),
    )
    .await
    .expect("The production WSL discovery deadline must hold");
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].environment, info.context.environment);
    let SessionSource::Inline { label, content } = &logs[0].source else {
        panic!("The WSL bridge must return the characterized transfer source");
    };
    assert_eq!(label, "opencode-v2-export:ses_wsl_acceptance");
    let transfer: Value = serde_json::from_str(content).unwrap();
    assert_eq!(
        transfer["messages"][0]["text"],
        "synthetic pipe request: café 雪\nsecond line"
    );
    assert_eq!(
        transfer["messages"][1]["content"][0]["state"]["content"][0]["text"]
            .as_str()
            .unwrap(),
        "x".repeat(256 * 1024 + 17),
    );
    let input = SessionInput {
        agent: "opencode".into(),
        session_id: "ses_wsl_acceptance".into(),
        source_format: SourceFormat::OpenCodeV2SessionExport,
        source: RawSource::Jsonl(content.clone()),
        fork_parent_session_id: None,
    };
    let reader = reader_for_input(&input);
    assert_eq!(
        reader.capabilities(&input).source_format,
        input.source_format
    );
    let normalized = reader.normalize(&input).unwrap();
    assert_eq!(
        normalized
            .events
            .iter()
            .map(|event| event.tools.len())
            .sum::<usize>(),
        1
    );
    let mut sink = Sink::default();
    reader.visit(&input, &mut sink).unwrap();
    assert_eq!(
        sink.native_results, 0,
        "A stored transfer cannot grant live eligibility"
    );
    assert_eq!(sink.gaps, 1);
    assert!(
        std::fs::read(&registration_path).unwrap() == registration_before,
        "The bridge must not change registration bytes"
    );
    assert!(!service.home.join("unexpected-opencode-command").exists());

    let requests: Vec<Value> =
        serde_json::from_slice(&std::fs::read(service.home.join("requests.json")).unwrap())
            .unwrap();
    let paths = requests
        .iter()
        .map(|request| request["path"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        [
            "/api/info",
            "/api/session?limit=100&order=desc",
            "/api/experimental/session/ses_wsl_acceptance/export",
            "/api/info",
        ]
    );
    assert!(requests.iter().all(|request| request["method"] == "GET"
        && request["authorized"] == true
        && request["status"] == 200));
    let response_bytes = requests
        .iter()
        .map(|request| request["bytes"].as_u64().unwrap())
        .sum::<u64>();
    println!(
        "Real WSL bridge: 4 authenticated GETs, {response_bytes} response bytes, {:?}, 0 producer or model calls",
        started.elapsed()
    );

    std::fs::write(service.home.join("stop"), b"").unwrap();
    let status = tokio::time::timeout(Duration::from_secs(10), service.child.wait())
        .await
        .expect("The isolated Linux service must stop")
        .unwrap();
    assert!(status.success());
    assert!(
        !registration_path.exists(),
        "The child must remove its credential file"
    );
}
