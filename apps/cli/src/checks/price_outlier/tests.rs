use rusqlite::Connection;
use serde_json::json;

use super::{assessment, cohort, insert_analysis, insert_session, with_fixture};

#[test]
fn emits_a_finding_only_when_the_target_exceeds_the_cohort_threshold() {
    with_fixture(|connection, database_path, identity, config| {
        let finding = assessment(connection, database_path, identity, config).unwrap();
        assert_eq!(finding.check_id, "outlier_priced_session");
        assert_eq!(finding.status, "finding");
        assert_eq!(finding.result.as_ref().unwrap()["estimated_cost_usd"], 4.0);
        assert_eq!(finding.result.as_ref().unwrap()["threshold_usd"], 3.0);
        assert_eq!(finding.result.as_ref().unwrap()["priced_session_count"], 8);
    });
}

#[test]
fn a_target_equal_to_the_threshold_is_clean_and_over_the_floor_is_a_finding() {
    with_fixture(|connection, database_path, identity, config| {
        insert_analysis(connection, "native", "pi", "target", 3_000_000);
        let equal = assessment(connection, database_path, identity, config).unwrap();
        assert_eq!(equal.status, "clean");
        assert_eq!(equal.result.as_ref().unwrap()["threshold_usd"], 3.0);

        insert_analysis(connection, "native", "pi", "target", 2_100_000);
        for index in 0..7 {
            insert_analysis(
                connection,
                "native",
                "pi",
                &format!("neighbor-{index:02}"),
                100_000,
            );
        }
        let over_floor = assessment(connection, database_path, identity, config).unwrap();
        assert_eq!(over_floor.status, "finding");
        assert_eq!(over_floor.result.as_ref().unwrap()["threshold_usd"], 2.0);
    });
}

#[test]
fn neighbors_use_time_distance_activity_recency_and_inclusive_age_bounds() {
    with_fixture(|connection, _, identity, _| {
        clear_neighbors(connection);
        let target_epoch = 1_000_000;
        for (id, environment, agent, epoch) in [
            ("plus", "native", "pi", target_epoch + 10),
            ("minus", "native", "pi", target_epoch - 10),
            ("tie-a", "native", "pi", target_epoch + 20),
            ("tie-z", "native", "pi", target_epoch + 20),
            ("edge", "native", "pi", target_epoch + 72 * 60 * 60),
            ("outside", "native", "pi", target_epoch + 72 * 60 * 60 + 1),
            ("target", "wsl:ubuntu", "codex", target_epoch + 5),
        ] {
            insert_session(connection, environment, agent, id, Some(epoch));
            insert_analysis(connection, environment, agent, id, 1_000_000);
        }
        let neighbors =
            cohort::neighbors(connection, identity, target_epoch, 72 * 60 * 60, 10).unwrap();
        let ids = neighbors
            .iter()
            .map(|neighbor| neighbor.identity.session_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["target", "plus", "minus", "tie-z", "tie-a", "edge"]);
    });
}

#[test]
fn cap_is_applied_before_priceability_and_does_not_backfill() {
    with_fixture(|connection, database_path, identity, config| {
        clear_neighbors(connection);
        let target_epoch = 1_000_000;
        for index in 0_i64..14 {
            let id = format!("priced-{index:02}");
            insert_session(
                connection,
                "native",
                "pi",
                &id,
                Some(target_epoch + index + 1),
            );
            insert_analysis(connection, "native", "pi", &id, 1_000_000);
        }
        insert_session(
            connection,
            "native",
            "pi",
            "unpriced",
            Some(target_epoch + 15),
        );
        insert_analysis_json(connection, "native", "pi", "unpriced");
        insert_session(
            connection,
            "native",
            "pi",
            "backfill",
            Some(target_epoch + 16),
        );
        insert_analysis(connection, "native", "pi", "backfill", 100_000_000);

        let result = assessment(connection, database_path, identity, config).unwrap();
        assert_eq!(result.status, "finding");
        assert_eq!(result.result.as_ref().unwrap()["priced_session_count"], 15);
    });
}

#[test]
fn missing_time_or_price_data_is_not_assessed() {
    with_fixture(|connection, database_path, identity, config| {
        connection
            .execute(
                "UPDATE session SET updated_at_epoch = NULL
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
                rusqlite::params![
                    identity.environment_key,
                    identity.agent,
                    identity.session_id
                ],
            )
            .unwrap();
        let missing_time = assessment(connection, database_path, identity, config).unwrap();
        assert_eq!(missing_time.status, "not_assessed");
        assert_eq!(
            missing_time.result.as_ref().unwrap()["reason"],
            "missing_target_timestamp"
        );

        insert_session(
            connection,
            "native",
            "pi",
            "target-without-analysis",
            Some(1_000_000),
        );
        let no_analysis = assessment(
            connection,
            database_path,
            &super::target_identity("target-without-analysis"),
            config,
        );
        assert!(no_analysis.is_none());
    });
}

#[test]
fn missing_pricing_snapshot_does_not_block_other_check_lookup() {
    with_fixture(|connection, database_path, identity, config| {
        std::fs::remove_file(database_path.with_file_name("model-pricing.json")).unwrap();
        let unavailable = assessment(connection, database_path, identity, config).unwrap();
        assert_eq!(unavailable.status, "not_assessed");
        assert_eq!(
            unavailable.result.as_ref().unwrap()["reason"],
            "pricing_unavailable"
        );
    });
}

#[test]
fn fewer_than_eight_priceable_sessions_is_not_assessed() {
    with_fixture(|connection, database_path, identity, config| {
        connection
            .execute("DELETE FROM session WHERE session_id = 'neighbor-06'", [])
            .unwrap();
        let result = assessment(connection, database_path, identity, config).unwrap();
        assert_eq!(result.status, "not_assessed");
        assert_eq!(result.result.as_ref().unwrap()["priced_session_count"], 7);
    });
}

fn clear_neighbors(connection: &Connection) {
    connection
        .execute(
            "DELETE FROM session_analysis WHERE session_id LIKE 'neighbor-%'",
            [],
        )
        .unwrap();
    connection
        .execute("DELETE FROM session WHERE session_id LIKE 'neighbor-%'", [])
        .unwrap();
}

fn insert_analysis_json(connection: &Connection, environment: &str, agent: &str, session_id: &str) {
    let breakdown = json!({
        "unknown-model": {
            "input_tokens": 1_000_000,
            "output_tokens": 0,
            "cache_read_tokens": 0,
            "cache_creation_tokens": 0,
            "cache_creation_1h_tokens": 0
        }
    })
    .to_string();
    connection
        .execute(
            "INSERT INTO session_analysis (
                environment_key, agent, session_id, model_breakdown_json,
                pricing_breakdown_json
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![environment, agent, session_id, breakdown, breakdown],
        )
        .unwrap();
}
