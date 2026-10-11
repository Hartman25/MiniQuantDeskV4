//! Unit tests for `account_entitlement` (pure evaluator + evidence cell).

use chrono::{DateTime, Duration, Utc};
use mqk_execution::{AccountEntitlementRefusal, AssetClass};
use serde_json::{json, Value};

use crate::account_entitlement::*;

fn active_account() -> Value {
    json!({
        "id": "904837e3-3b76-47ec-b432-046db621571b",
        "status": "ACTIVE",
        "crypto_status": "ACTIVE",
        "trading_blocked": false,
        "account_blocked": false,
        "trade_suspended_by_user": false,
        "options_trading_level": 2,
        "shorting_enabled": true,
        "currency": "USD"
    })
}

const ACCOUNT_ID: &str = "904837e3-3b76-47ec-b432-046db621571b";

fn ev(v: &Value) -> AccountEntitlementEvidence {
    AccountEntitlementEvidence::from_account_json(v)
}

fn code(r: Result<(), AccountEntitlementRefusal>) -> String {
    r.expect_err("expected refusal").code
}

fn t0() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-10T14:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

#[test]
fn active_account_is_admitted_for_equity_crypto_option_and_base() {
    let e = ev(&active_account());
    for class in [
        None,
        Some(AssetClass::Equity),
        Some(AssetClass::Crypto),
        Some(AssetClass::Option),
    ] {
        assert!(evaluate_account_entitlement(&e, class).is_ok(), "{class:?}");
    }
    assert_eq!(
        e.provider_account_id.as_deref(),
        Some("904837e3-3b76-47ec-b432-046db621571b")
    );
}

/// Every denial/unknown shape refuses with its own stable code.
#[test]
fn each_blocked_or_unknown_field_refuses_with_its_code() {
    let cases: Vec<(&str, Value, &str)> = vec![
        ("trading_blocked", json!(true), "account_trading_blocked"),
        ("account_blocked", json!(true), "account_blocked"),
        (
            "trade_suspended_by_user",
            json!(true),
            "account_trade_suspended_by_user",
        ),
        (
            "status",
            json!("ACCOUNT_UPDATED"),
            "account_status_not_active",
        ),
        ("status", json!("PAPER_ONLY"), "account_status_not_active"),
        ("status", json!("active"), "account_status_not_active"),
        (
            "trading_blocked",
            json!("false"),
            "account_entitlement_field_unavailable",
        ),
        (
            "account_blocked",
            json!(0),
            "account_entitlement_field_unavailable",
        ),
        (
            "trade_suspended_by_user",
            Value::Null,
            "account_entitlement_field_unavailable",
        ),
        ("status", json!(7), "account_entitlement_field_unavailable"),
        ("id", json!(""), "account_identity_unavailable"),
        ("id", json!(12), "account_identity_unavailable"),
    ];
    for (field, value, want) in cases {
        let mut a = active_account();
        a[field] = value.clone();
        assert_eq!(
            code(evaluate_account_entitlement(
                &ev(&a),
                Some(AssetClass::Equity)
            )),
            want,
            "{field}={value}"
        );
    }
    for field in [
        "trading_blocked",
        "account_blocked",
        "trade_suspended_by_user",
        "status",
        "id",
    ] {
        let mut a = active_account();
        a.as_object_mut().unwrap().remove(field);
        let c = code(evaluate_account_entitlement(
            &ev(&a),
            Some(AssetClass::Equity),
        ));
        assert!(
            c == "account_entitlement_field_unavailable" || c == "account_identity_unavailable",
            "missing {field} must refuse, got {c}"
        );
    }
}

#[test]
fn class_specific_entitlement_is_independent_of_equity_entitlement() {
    let mut a = active_account();
    a["crypto_status"] = json!("INACTIVE");
    a["options_trading_level"] = json!(0);
    let e = ev(&a);
    assert!(evaluate_account_entitlement(&e, Some(AssetClass::Equity)).is_ok());
    assert_eq!(
        code(evaluate_account_entitlement(&e, Some(AssetClass::Crypto))),
        "account_crypto_not_active"
    );
    assert_eq!(
        code(evaluate_account_entitlement(&e, Some(AssetClass::Option))),
        "account_options_not_enabled"
    );
    let mut missing = active_account();
    missing.as_object_mut().unwrap().remove("crypto_status");
    missing
        .as_object_mut()
        .unwrap()
        .remove("options_trading_level");
    let e = ev(&missing);
    for class in [AssetClass::Crypto, AssetClass::Option] {
        assert_eq!(
            code(evaluate_account_entitlement(&e, Some(class))),
            "account_entitlement_field_unavailable"
        );
    }
    for class in [AssetClass::Future, AssetClass::Forex] {
        assert_eq!(
            code(evaluate_account_entitlement(
                &ev(&active_account()),
                Some(class)
            )),
            "account_entitlement_unsupported_asset_class"
        );
    }
}

#[test]
fn cell_without_observation_is_unavailable_never_admitting() {
    let cell = AccountEvidenceCell::new();
    let r = cell.admit(t0(), Duration::seconds(61), Some(AssetClass::Equity));
    assert_eq!(code(r), "account_evidence_unavailable");
    assert_eq!(
        cell.readiness(t0(), Duration::seconds(61), AssetClass::Equity)
            .state,
        "not_observed"
    );
}

#[test]
fn stale_and_future_observations_refuse() {
    let cell = AccountEvidenceCell::new();
    cell.pin_provider_account_id(ACCOUNT_ID);
    cell.observe(ev(&active_account()), t0());
    let bound = Duration::seconds(61);
    assert!(cell
        .admit(t0() + Duration::seconds(61), bound, None)
        .is_ok());
    for now in [t0() + Duration::seconds(62), t0() - Duration::seconds(1)] {
        assert_eq!(code(cell.admit(now, bound, None)), "account_evidence_stale");
    }
    assert_eq!(
        cell.readiness(t0() + Duration::seconds(300), bound, AssetClass::Equity)
            .state,
        "stale"
    );
}

#[test]
fn status_change_after_valid_snapshot_refuses_on_next_observation() {
    let cell = AccountEvidenceCell::new();
    cell.pin_provider_account_id(ACCOUNT_ID);
    let bound = Duration::seconds(61);
    cell.observe(ev(&active_account()), t0());
    assert!(cell.admit(t0(), bound, Some(AssetClass::Equity)).is_ok());
    let mut blocked = active_account();
    blocked["trading_blocked"] = json!(true);
    cell.observe(ev(&blocked), t0() + Duration::seconds(5));
    let now = t0() + Duration::seconds(6);
    assert_eq!(
        code(cell.admit(now, bound, Some(AssetClass::Equity))),
        "account_trading_blocked"
    );
    let r = cell.readiness(now, bound, AssetClass::Equity);
    assert_eq!(r.state, "denied");
    assert_eq!(r.code.as_deref(), Some("account_trading_blocked"));
}

#[test]
fn pinned_account_identity_drift_refuses() {
    let cell = AccountEvidenceCell::new();
    let bound = Duration::seconds(61);
    cell.pin_provider_account_id("904837E3-3B76-47EC-B432-046DB621571B");
    cell.observe(ev(&active_account()), t0());
    assert!(
        cell.admit(t0(), bound, None).is_ok(),
        "pin is case-insensitive and matches the bound account"
    );
    let mut other = active_account();
    other["id"] = json!("11111111-2222-3333-4444-555555555555");
    cell.observe(ev(&other), t0());
    assert_eq!(
        code(cell.admit(t0(), bound, None)),
        "account_identity_drift"
    );
    assert_eq!(
        cell.readiness(t0(), bound, AssetClass::Equity).state,
        "denied"
    );
}

#[test]
fn unknown_fields_report_unknown_not_denied() {
    let cell = AccountEvidenceCell::new();
    cell.pin_provider_account_id(ACCOUNT_ID);
    let mut a = active_account();
    a.as_object_mut().unwrap().remove("trading_blocked");
    cell.observe(ev(&a), t0());
    let r = cell.readiness(t0(), Duration::seconds(61), AssetClass::Equity);
    assert_eq!(r.state, "unknown");
    assert!(!is_provider_denial_code(r.code.as_deref().unwrap()));
}

// ---------------------------------------------------------------------------
// Adapter-level: the real AlpacaBrokerAdapter records evidence from its own
// GET /v2/account fetches and its BrokerAdapter impl admits only against it.
// The HTTP body here is an in-process mock of the provider wire shape: it
// proves adapter plumbing, NOT that any real Alpaca account is entitled.
// ---------------------------------------------------------------------------
mod adapter {
    use super::*;
    use crate::{AlpacaBrokerAdapter, AlpacaConfig};
    use httpmock::prelude::*;
    use mqk_execution::BrokerAdapter;

    fn account_body(extra: Value) -> Value {
        let mut a = json!({
            "id": "904837e3-3b76-47ec-b432-046db621571b",
            "status": "ACTIVE",
            "crypto_status": "ACTIVE",
            "trading_blocked": false,
            "account_blocked": false,
            "trade_suspended_by_user": false,
            "equity": "100000",
            "cash": "100000",
            "currency": "USD",
            "buying_power": "200000"
        });
        for (k, v) in extra.as_object().unwrap() {
            a[k] = v.clone();
        }
        a
    }

    fn adapter(server: &MockServer, cell: &AccountEvidenceCell) -> AlpacaBrokerAdapter {
        AlpacaBrokerAdapter::new(AlpacaConfig {
            base_url: server.base_url(),
            api_key_id: "k".into(),
            api_secret_key: "s".into(),
            crypto_capability_enabled: false,
            options_mleg_capability_enabled: false,
        })
        .with_account_evidence(cell.clone(), Duration::seconds(61))
    }

    /// Bind the run to the fixture account (what the run-start step does after
    /// its own probe proved the account).
    fn bound(cell: &AccountEvidenceCell) {
        cell.pin_provider_account_id("904837e3-3b76-47ec-b432-046db621571b");
    }

    fn serve(server: &MockServer, body: Value) {
        server.mock(|when, then| {
            when.method(GET).path("/v2/account");
            then.status(200).json_body(body);
        });
        server.mock(|when, then| {
            when.method(GET).path("/v2/positions");
            then.status(200).json_body(json!([]));
        });
        server.mock(|when, then| {
            when.method(GET).path("/v2/orders");
            then.status(200).json_body(json!([]));
        });
    }

    #[test]
    fn unbound_adapter_never_admits() {
        let a = AlpacaBrokerAdapter::new(AlpacaConfig {
            base_url: "http://127.0.0.1:1".into(),
            api_key_id: "k".into(),
            api_secret_key: "s".into(),
            crypto_capability_enabled: false,
            options_mleg_capability_enabled: false,
        });
        assert_eq!(
            a.admit_account_entitlement(Some(AssetClass::Equity))
                .unwrap_err()
                .code,
            "account_evidence_not_bound"
        );
    }

    #[test]
    fn bound_adapter_without_any_fetch_refuses_as_unavailable() {
        let server = MockServer::start();
        let cell = AccountEvidenceCell::new();
        let a = adapter(&server, &cell);
        assert_eq!(
            a.admit_account_entitlement(Some(AssetClass::Equity))
                .unwrap_err()
                .code,
            "account_evidence_unavailable"
        );
    }

    #[test]
    fn snapshot_fetch_records_evidence_and_admits_equity_only_for_equity_account() {
        let server = MockServer::start();
        serve(&server, account_body(json!({"crypto_status": "INACTIVE"})));
        let cell = AccountEvidenceCell::new();
        let a = adapter(&server, &cell);
        bound(&cell);
        a.fetch_broker_snapshot(Utc::now()).expect("snapshot");
        assert!(a
            .admit_account_entitlement(Some(AssetClass::Equity))
            .is_ok());
        assert_eq!(
            a.admit_account_entitlement(Some(AssetClass::Crypto))
                .unwrap_err()
                .code,
            "account_crypto_not_active"
        );
    }

    #[test]
    fn blocked_account_is_refused_after_probe_even_though_snapshot_parses() {
        let server = MockServer::start();
        serve(&server, account_body(json!({"trading_blocked": true})));
        let cell = AccountEvidenceCell::new();
        let a = adapter(&server, &cell);
        bound(&cell);
        a.fetch_broker_snapshot(Utc::now())
            .expect("a blocked account still yields a snapshot");
        assert_eq!(
            a.admit_account_entitlement(Some(AssetClass::Equity))
                .unwrap_err()
                .code,
            "account_trading_blocked"
        );
    }

    #[test]
    fn stale_observation_cannot_admit() {
        let server = MockServer::start();
        serve(&server, account_body(json!({})));
        let cell = AccountEvidenceCell::new();
        let a = adapter(&server, &cell);
        bound(&cell);
        a.fetch_account_entitlement(Utc::now() - Duration::seconds(600))
            .expect("probe");
        assert_eq!(
            a.admit_account_entitlement(Some(AssetClass::Equity))
                .unwrap_err()
                .code,
            "account_evidence_stale"
        );
    }

    #[test]
    fn config_flag_alone_does_not_manufacture_provider_permission() {
        // crypto flag on + Paper host: adapter SUPPORTS crypto, but the
        // provider account reports crypto inactive => admission refuses.
        let server = MockServer::start();
        serve(&server, account_body(json!({"crypto_status": "INACTIVE"})));
        let cell = AccountEvidenceCell::new();
        let a = AlpacaBrokerAdapter::new(AlpacaConfig {
            base_url: "https://paper-api.alpaca.markets".into(),
            api_key_id: "k".into(),
            api_secret_key: "s".into(),
            crypto_capability_enabled: true,
            options_mleg_capability_enabled: false,
        })
        .with_account_evidence(cell.clone(), Duration::seconds(61));
        assert!(a.supports_asset_class(AssetClass::Crypto));
        bound(&cell);
        cell.observe(
            ev(&account_body(json!({"crypto_status": "INACTIVE"}))),
            Utc::now(),
        );
        assert_eq!(
            a.admit_account_entitlement(Some(AssetClass::Crypto))
                .unwrap_err()
                .code,
            "account_crypto_not_active"
        );
        drop(server);
    }

    #[test]
    fn provider_entitlement_present_but_adapter_capability_off_still_refuses_class() {
        // Entitlement is necessary, never sufficient: crypto_status ACTIVE
        // does not make a default (flag-off) adapter support crypto.
        let server = MockServer::start();
        let cell = AccountEvidenceCell::new();
        let a = adapter(&server, &cell);
        bound(&cell);
        cell.observe(ev(&account_body(json!({}))), Utc::now());
        assert!(a
            .admit_account_entitlement(Some(AssetClass::Crypto))
            .is_ok());
        assert!(!a.supports_asset_class(AssetClass::Crypto));
    }
}

// ---------------------------------------------------------------------------
// Positive run/account binding is REQUIRED: fresh, entitled evidence observed
// by anyone (e.g. an unrelated snapshot refresher) never authorizes orders.
// ---------------------------------------------------------------------------

#[test]
fn fresh_entitled_evidence_without_a_binding_is_refused() {
    let cell = AccountEvidenceCell::new();
    let bound = Duration::seconds(61);
    cell.observe(ev(&active_account()), t0());
    for class in [None, Some(AssetClass::Equity)] {
        assert_eq!(
            code(cell.admit(t0(), bound, class)),
            "account_binding_absent"
        );
    }
    let r = cell.readiness(t0(), bound, AssetClass::Equity);
    assert_eq!(r.state, "unbound");
    assert_eq!(r.code.as_deref(), Some("account_binding_absent"));
    assert!(!is_provider_denial_code("account_binding_absent"));
}

#[test]
fn binding_is_the_only_thing_that_turns_evidence_into_admission() {
    let cell = AccountEvidenceCell::new();
    let bound = Duration::seconds(61);
    cell.observe(ev(&active_account()), t0());
    assert!(cell.admit(t0(), bound, None).is_err());
    cell.pin_provider_account_id(ACCOUNT_ID);
    assert!(cell.admit(t0(), bound, None).is_ok());
    // a blank id is not a binding; clearing removes admission again
    cell.pin_provider_account_id("  ");
    assert_eq!(
        code(cell.admit(t0(), bound, None)),
        "account_binding_absent"
    );
    cell.pin_provider_account_id(ACCOUNT_ID);
    cell.clear_run_binding();
    assert_eq!(
        code(cell.admit(t0(), bound, None)),
        "account_binding_absent"
    );
    // a later independent observation does not re-establish it
    cell.observe(ev(&active_account()), t0() + Duration::seconds(5));
    assert_eq!(
        code(cell.admit(t0() + Duration::seconds(6), bound, None)),
        "account_binding_absent"
    );
}

#[test]
fn a_clone_shares_the_binding_state_but_a_new_cell_inherits_nothing() {
    let cell = AccountEvidenceCell::new();
    cell.pin_provider_account_id(ACCOUNT_ID);
    cell.observe(ev(&active_account()), t0());
    let shared = cell.clone();
    assert!(shared.admit(t0(), Duration::seconds(61), None).is_ok());
    // "restart": a fresh process-local cell has neither evidence nor binding
    let fresh = AccountEvidenceCell::new();
    assert!(fresh.latest().is_none());
    assert!(fresh.pinned_provider_account_id().is_none());
    assert_eq!(
        code(fresh.admit(t0(), Duration::seconds(61), None)),
        "account_evidence_unavailable"
    );
}
