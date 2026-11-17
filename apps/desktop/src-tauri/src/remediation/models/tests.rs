use super::*;

fn known_status_json(amount: &str) -> String {
    format!(
        r#"{{"method":"oldModelPriceDifference","methodRevision":1,"pricingRevision":"pricing","apiEquivalentCostAvoidedUsd":{amount},"measuredThroughMs":2000,"recurrenceMs":null,"status":"known"}}"#
    )
}

#[test]
fn legacy_aggregate_savings_default_to_pending() {
    let savings: AggregateSavings = serde_json::from_value(serde_json::json!({
        "version": 1,
        "apiEquivalentCostAvoidedUsd": 0.25,
    }))
    .unwrap();

    assert_eq!(
        savings.status,
        SavingsStatus::Pending {
            method_revision: None
        }
    );
    assert_eq!(savings.api_equivalent_cost_avoided_usd, Some(0.25));
}

#[test]
fn explicit_aggregate_savings_status_is_preserved() {
    let savings: AggregateSavings = serde_json::from_value(serde_json::json!({
        "version": 1,
        "status": {"status": "unavailable"},
    }))
    .unwrap();

    assert_eq!(savings.status, SavingsStatus::Unavailable);
}

#[test]
fn unavailable_aggregate_savings_cannot_have_an_avoided_cost() {
    let result = serde_json::from_value::<AggregateSavings>(serde_json::json!({
        "version": 1,
        "status": {"status": "unavailable"},
        "apiEquivalentCostAvoidedUsd": 0.25,
    }));

    assert!(result.is_err());
}

#[test]
fn known_savings_json_preserves_numeric_values_and_wire_shape() {
    for (number, expected) in [
        ("0.25", 0.25),
        ("0", 0.0),
        ("-0.25", -0.25),
        ("2.5e-3", 0.0025),
        ("1.7976931348623157e308", f64::MAX),
    ] {
        let status = known_status_json(number);
        let expected_status = SavingsStatus::Known {
            method: SavingsMethod::OldModelPriceDifference,
            method_revision: 1,
            pricing_revision: "pricing".into(),
            api_equivalent_cost_avoided_usd: expected,
            measured_through_ms: 2_000,
            recurrence_ms: None,
        };
        let from_string: SavingsStatus = serde_json::from_str(&status).unwrap();
        assert_eq!(from_string, expected_status);
        assert_eq!(
            serde_json::from_slice::<SavingsStatus>(status.as_bytes()).unwrap(),
            expected_status
        );
        let serialized = serde_json::to_value(&from_string).unwrap();
        assert_eq!(
            serialized,
            serde_json::json!({
                "status": "known",
                "method": "oldModelPriceDifference",
                "methodRevision": 1,
                "pricingRevision": "pricing",
                "apiEquivalentCostAvoidedUsd": expected,
                "measuredThroughMs": 2000,
                "recurrenceMs": null,
            })
        );
        assert_eq!(
            serde_json::from_value::<SavingsStatus>(serialized).unwrap(),
            expected_status
        );

        let aggregate =
            format!(r#"{{"version":1,"status":{status},"apiEquivalentCostAvoidedUsd":{number}}}"#);
        let savings: AggregateSavings = serde_json::from_str(&aggregate).unwrap();
        assert_eq!(savings.status, expected_status);
        assert_eq!(savings.api_equivalent_cost_avoided_usd, Some(expected));
        assert_eq!(
            serde_json::from_str::<AggregateSavings>(&serde_json::to_string(&savings).unwrap())
                .unwrap(),
            savings
        );
    }
}

#[test]
fn known_savings_json_rejects_non_numeric_and_out_of_range_costs() {
    for number in [
        r#"{"$serde_json::private::Number":"0.25"}"#,
        r#"{"$serde_json::private::RawValue":"0.25"}"#,
        r#""0.25""#,
        "null",
        "true",
        "[]",
        "{}",
        "1e309",
        "-1e309",
    ] {
        let status = known_status_json(number);
        assert!(serde_json::from_str::<SavingsStatus>(&status).is_err());
        let aggregate = format!(r#"{{"version":1,"status":{status}}}"#);
        assert!(serde_json::from_str::<AggregateSavings>(&aggregate).is_err());
    }
}

#[test]
fn savings_json_preserves_variant_field_rules() {
    for (json, expected) in [
        (
            r#"{"status":"pending"}"#,
            SavingsStatus::Pending {
                method_revision: None,
            },
        ),
        (
            r#"{"methodRevision":null,"status":"pending"}"#,
            SavingsStatus::Pending {
                method_revision: None,
            },
        ),
        (
            r#"{"methodRevision":2,"status":"pending"}"#,
            SavingsStatus::Pending {
                method_revision: Some(2),
            },
        ),
        (r#"{"status":"unavailable"}"#, SavingsStatus::Unavailable),
        (
            r#"{"reason":"missingRates","methodRevision":1,"status":"unknown"}"#,
            SavingsStatus::Unknown {
                reason: SavingsUnknownReason::MissingRates,
                method_revision: 1,
            },
        ),
        (
            r#"{"status":"pending","reason":{},"extra":true}"#,
            SavingsStatus::Pending {
                method_revision: None,
            },
        ),
        (
            r#"{"methodRevision":"ignored","status":"unavailable"}"#,
            SavingsStatus::Unavailable,
        ),
    ] {
        assert_eq!(
            serde_json::from_str::<SavingsStatus>(json).unwrap(),
            expected
        );
    }

    let known = known_status_json("0.25");
    let tag_first = known.replace(r#","status":"known"}"#, "}");
    let tag_first = tag_first.replacen('{', r#"{"status":"known","#, 1);
    assert_eq!(
        serde_json::from_str::<SavingsStatus>(&known).unwrap(),
        serde_json::from_str::<SavingsStatus>(&tag_first).unwrap()
    );

    for json in [
        "{}".to_owned(),
        r#"{"status":"invalid"}"#.to_owned(),
        r#"{"status":null}"#.to_owned(),
        r#"{"status":"pending","status":"pending"}"#.to_owned(),
        r#"{"status":"pending","methodRevision":null,"methodRevision":1}"#.to_owned(),
        r#"{"status":"unknown","methodRevision":1}"#.to_owned(),
        r#"{"status":"unknown","reason":"missingRates"}"#.to_owned(),
        known.replace(r#""methodRevision":1,"#, ""),
        known.replace(r#""methodRevision":1"#, r#""methodRevision":4294967296"#),
        known.replace(
            r#""method":"oldModelPriceDifference""#,
            r#""method":"fastTierPricePremium""#,
        ),
        known.replace(
            r#""pricingRevision":"pricing""#,
            r#""pricingRevision":null"#,
        ),
        known.replace(
            r#""recurrenceMs":null"#,
            r#""recurrenceMs":null,"recurrenceMs":null"#,
        ),
        known.replace(r#""measuredThroughMs":2000"#, r#""measuredThroughMs":1.5"#),
    ] {
        assert!(serde_json::from_str::<SavingsStatus>(&json).is_err());
    }

    let unknown_aggregate = format!(r#"{{"version":1,"status":{known},"extra":true}}"#);
    assert!(serde_json::from_str::<AggregateSavings>(&unknown_aggregate).is_err());
}

#[test]
fn savings_json_preserves_sequence_field_order() {
    for (json, expected) in [
        (
            r#"["pending"]"#,
            SavingsStatus::Pending {
                method_revision: None,
            },
        ),
        (
            r#"["pending",null]"#,
            SavingsStatus::Pending {
                method_revision: None,
            },
        ),
        (
            r#"["pending",2]"#,
            SavingsStatus::Pending {
                method_revision: Some(2),
            },
        ),
        (r#"["unavailable"]"#, SavingsStatus::Unavailable),
        (
            r#"["unknown","missingRates",1]"#,
            SavingsStatus::Unknown {
                reason: SavingsUnknownReason::MissingRates,
                method_revision: 1,
            },
        ),
        (
            r#"["known","oldModelPriceDifference",1,"pricing",0.25,2000,null]"#,
            serde_json::from_str(&known_status_json("0.25")).unwrap(),
        ),
    ] {
        assert_eq!(
            serde_json::from_str::<SavingsStatus>(json).unwrap(),
            expected
        );
    }
    for json in [
        "[]",
        r#"["unavailable",null]"#,
        r#"["pending",1,2]"#,
        r#"["unknown","missingRates"]"#,
        r#"["known","oldModelPriceDifference",1,"pricing",0.25,2000]"#,
        r#"["known","oldModelPriceDifference",1,"pricing",{"$serde_json::private::Number":"0.25"},2000,null]"#,
    ] {
        assert!(serde_json::from_str::<SavingsStatus>(json).is_err());
    }
}
