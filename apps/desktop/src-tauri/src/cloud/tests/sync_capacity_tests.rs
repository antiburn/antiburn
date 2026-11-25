use super::{Job, JobState, waiting_project_directories};
use antiburn_cloud_capture::CaptureIdentity;
use std::path::PathBuf;

#[test]
fn waiting_repository_jobs_use_the_saved_project_path() {
    let mut job = Job {
        identity: CaptureIdentity {
            capture_id: "capture".into(),
            account_id: "account".into(),
            destination_id: "workspace".into(),
            source_id: "scope/source".into(),
        },
        resource_origin: None,
        issuer_origin: None,
        team_grant_id: None,
        membership_generation: None,
        source_scope_id: "scope".into(),
        project_path: "/repo/child".into(),
        project_folders: Vec::new(),
        repository_origin: Some("github.com/team/repo".into()),
        recorded_project_paths: Some(vec!["/repo/child".into()]),
        observation_key: "observation".into(),
        state: JobState::WaitingForCapacity,
        reservation_retry_at: Some(1),
    };
    assert_eq!(
        waiting_project_directories(&job),
        [PathBuf::from("/repo/child")]
    );
    job.state = JobState::WaitingForLocalCapacity;
    assert_eq!(
        waiting_project_directories(&job),
        [PathBuf::from("/repo/child")]
    );

    job.repository_origin = None;
    job.project_folders = vec!["/repo".into(), "/repo".into()];
    assert_eq!(waiting_project_directories(&job), [PathBuf::from("/repo")]);
}
