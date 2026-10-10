//! MULTIASSET-RUNTIME-INTEGRATION-01 / correction: a flatten close is an
//! Equity-shaped order, so it is enqueued only when trusted instrument
//! evidence POSITIVELY proves the position is an Equity and no configured
//! authoritative source contradicts it. Absence of a refusal is never
//! permission.
//!
//! Evidence sources: the canonical legacy Equity registry (v1,
//! `MQK_INSTRUMENT_REGISTRY_PATH`) is the positive proof; the trading
//! registry-v2 (`MQK_TRADING_INSTRUMENT_REGISTRY_V2_PATH`), when configured,
//! must be readable, valid and non-contradictory.
//!
//! | Test  | Case |
//! |-------|------|
//! | FP-01 | proven canonical Equity, registry-v2 not configured -> closed |
//! | FP-02 | proven Equity, valid non-contradictory registry-v2 -> closed |
//! | FP-03 | known Crypto in registry-v2 -> refused |
//! | FP-04 | OCC option unknown to both registries -> refused |
//! | FP-05 | Crypto position, registry-v2 absent -> refused |
//! | FP-06 | registry-v2 configured but unreadable / malformed -> refused |
//! | FP-07 | conflicting or invalid registry-v2 content -> refused |
//! | FP-08 | mixed portfolio -> only proven Equities enqueued, refusals reported |
//! | FP-09 | unlisted ticker -> refused with an actionable warning |
//! | FP-10 | automatic pre-event flatten: no Equity row for any unproven position |
//! | FP-11 | canonical-registry contradictions (disabled, non-Equity, duplicate, unreadable) never prove an Equity |
//!
//! DB-backed (`#[ignore]`): run with
//! `MQK_DATABASE_URL=postgres://.../mqk_test cargo test -p mqk-daemon \
//!  --test scenario_multiasset_flatten_equity_proof_01 -- --include-ignored --test-threads=1`

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Method, Request, StatusCode};
use mqk_daemon::{
    routes::build_router,
    state::{AppState, ExecutionDomain, OperatorAuthMode},
};
use mqk_execution::QtyMicros;
use mqk_md::instrument_registry_v2::{
    ContractDefinitionV2, InstrumentDefinitionV2, InstrumentEconomicsMetadataV2,
    InstrumentMetadataV2, InstrumentRegistryV2, SESSION_PROFILE_CRYPTO_24_7,
};
use tower::ServiceExt;
use uuid::Uuid;

static DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const OCC_OPTION: &str = "AAPL260116C00150000";

fn definition(symbol: &str, asset_class: &str) -> InstrumentDefinitionV2 {
    let crypto = asset_class == "crypto";
    InstrumentDefinitionV2 {
        instrument_id: format!("{asset_class}:GLOBAL:{symbol}"),
        symbol: symbol.to_string(),
        asset_class: asset_class.to_string(),
        instrument_kind: None,
        venue: Some("GLOBAL".to_string()),
        currency: "USD".to_string(),
        quote_currency: crypto.then(|| "USD".to_string()),
        provider_symbols: if crypto {
            BTreeMap::new()
        } else {
            BTreeMap::from([("twelvedata".to_string(), symbol.to_string())])
        },
        broker_symbols: if crypto {
            BTreeMap::from([("alpaca".to_string(), symbol.to_string())])
        } else {
            BTreeMap::new()
        },
        enabled: !crypto,
        paper_trading_enabled: true,
        live_trading_enabled: false,
        timeframes: vec!["5m".to_string()],
        contract: crypto.then(|| ContractDefinitionV2::CryptoPair {
            base: "BTC".to_string(),
            quote: "USD".to_string(),
        }),
        metadata: InstrumentMetadataV2::default(),
        notes: Some("flatten equity-proof fixture".to_string()),
        allow_enabled_non_equity_for_testing: false,
        economics: crypto.then(|| InstrumentEconomicsMetadataV2 {
            contract_multiplier: None,
            initial_margin_micros: None,
            maintenance_margin_micros: None,
            quantity_increment_micros: Some(100),
            min_trade_qty_micros: Some(100),
            price_tick_micros: Some(1_000_000),
            session_profile: Some(SESSION_PROFILE_CRYPTO_24_7.to_string()),
        }),
    }
}

/// How the trading registry-v2 is configured for a scenario.
enum V2 {
    NotConfigured,
    /// A configured path with no file behind it.
    Unreadable,
    /// A readable file with exactly these bytes.
    Bytes(Vec<u8>),
}

fn v2_rows(rows: Vec<InstrumentDefinitionV2>) -> V2 {
    V2::Bytes(
        serde_json::to_vec(&InstrumentRegistryV2 {
            schema_version: 1,
            instruments: rows,
        })
        .expect("serialize"),
    )
}

fn v2_path(v2: V2) -> Option<String> {
    let dir = std::env::temp_dir().join(format!("mqk_fp01_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("fixture dir");
    let path = dir.join("trading_registry_v2.json");
    match v2 {
        V2::NotConfigured => None,
        V2::Unreadable => Some(path.to_string_lossy().into_owned()),
        V2::Bytes(bytes) => {
            std::fs::write(&path, bytes).expect("write");
            Some(path.to_string_lossy().into_owned())
        }
    }
}

async fn db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).expect("DB tests require MQK_DATABASE_URL");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect to test DB");
    mqk_db::migrate(&pool).await.expect("run migrations");
    for stmt in [
        "DELETE FROM oms_outbox WHERE run_id IN (SELECT run_id FROM runs WHERE engine_id = 'mqk-daemon' AND mode = 'PAPER')",
        "DELETE FROM runs WHERE engine_id = 'mqk-daemon' AND mode = 'PAPER'",
        "DELETE FROM sys_arm_state WHERE sentinel_id = 1",
        "DELETE FROM sys_reconcile_status_state",
    ] {
        sqlx::query(stmt).execute(&pool).await.expect("reset");
    }
    pool
}

fn qty(units: i64) -> QtyMicros {
    QtyMicros::from_whole_units(units).unwrap()
}

struct Outcome {
    status: StatusCode,
    json: serde_json::Value,
    outbox_symbols: Vec<String>,
    /// `payload.runtime_transition` of the durable audit event the route wrote.
    audit_transition: Option<String>,
}

/// Run the operator `flatten-paper-positions` route against a running Equity
/// run holding `positions`, with the canonical v1 registry anchored and the
/// trading registry-v2 configured as `v2`.
async fn operator_flatten(v2: V2, positions: Vec<(&str, QtyMicros)>) -> Outcome {
    operator_flatten_with_v1(None, v2, positions).await
}

/// As [`operator_flatten`], with the canonical (v1) registry path overridden.
async fn operator_flatten_with_v1(
    v1_path: Option<String>,
    v2: V2,
    positions: Vec<(&str, QtyMicros)>,
) -> Outcome {
    let pool = db_pool().await;
    let mut st = common::with_canonical_equity_registry(AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    ));
    if let Some(v1) = v1_path {
        st.instrument_registry_path = v1;
    }
    st.trading_instrument_registry_v2_path = v2_path(v2);
    let st = Arc::new(st);

    mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Armed, None)
        .await
        .expect("persist arm state");
    mqk_db::persist_reconcile_status_state(
        &pool,
        &mqk_db::PersistReconcileStatusState {
            status: "ok",
            last_run_at_utc: Some(chrono::Utc::now()),
            snapshot_watermark_ms: None,
            mismatched_positions: 0,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: None,
            updated_at_utc: chrono::Utc::now(),
        },
    )
    .await
    .expect("persist reconcile ok");
    let run_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now,
            git_hash: "fp01-test".to_string(),
            config_hash: "fp01-config".to_string(),
            config_json: serde_json::json!({"source": "scenario_multiasset_flatten_equity_proof_01"}),
            host_fingerprint: "fp01-host".to_string(),
        },
    )
    .await
    .expect("insert run");
    mqk_db::arm_run(&pool, run_id).await.expect("arm run");
    mqk_db::begin_run(&pool, run_id).await.expect("begin run");
    mqk_db::heartbeat_run(&pool, run_id, now)
        .await
        .expect("heartbeat run");
    st.inject_running_loop_for_test(ExecutionDomain::EquityNyse, run_id)
        .await;
    *st.execution_snapshot.write().await = Some(mqk_runtime::observability::ExecutionSnapshot {
        run_id: Some(run_id),
        active_orders: vec![],
        pending_outbox: vec![],
        recent_inbox_events: vec![],
        portfolio: mqk_runtime::observability::PortfolioSnapshot {
            cash_micros: 0,
            realized_pnl_micros: 0,
            positions: positions
                .into_iter()
                .map(
                    |(symbol, net_qty)| mqk_runtime::observability::PositionSnapshot {
                        symbol: symbol.to_string(),
                        net_qty,
                    },
                )
                .collect(),
        },
        system_block_state: None,
        recent_risk_denials: vec![],
        snapshot_at_utc: chrono::Utc::now(),
        has_recent_terminal_fill: false,
        risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::json!({
                "action_key": "flatten-paper-positions",
                "reason": "fp01-test-flatten"
            })
            .to_string(),
        ))
        .unwrap();
    let resp = build_router(Arc::clone(&st)).oneshot(req).await.unwrap();
    let status = resp.status();
    let json: serde_json::Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap())
            .expect("json");
    let outbox_symbols = sqlx::query_scalar::<_, String>(
        "SELECT order_json->>'symbol' FROM oms_outbox WHERE run_id = $1 ORDER BY 1",
    )
    .bind(run_id)
    .fetch_all(&pool)
    .await
    .expect("outbox symbols");
    let audit_transition = match json["audit"]["audit_event_id"].as_str() {
        Some(id) => sqlx::query_scalar::<_, String>(
            "SELECT payload->>'runtime_transition' FROM audit_events WHERE event_id = $1::uuid",
        )
        .bind(id)
        .fetch_optional(&pool)
        .await
        .expect("audit event"),
        None => None,
    };
    Outcome {
        status,
        json,
        outbox_symbols,
        audit_transition,
    }
}

fn warnings(json: &serde_json::Value) -> Vec<String> {
    json["warnings"]
        .as_array()
        .map(|w| {
            w.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn refusal_warning(json: &serde_json::Value, symbol: &str) -> Option<String> {
    let needle = format!("unsupported_position: symbol={symbol} ");
    warnings(json).into_iter().find(|w| w.starts_with(&needle))
}

/// A refused position must never be described as enqueued or pending.
fn assert_not_claimed_closed(json: &serde_json::Value, symbol: &str) {
    for w in warnings(json) {
        assert!(
            !(w.starts_with("enqueued:") || w.starts_with("already_pending:"))
                || !w.contains(&format!("symbol={symbol} ")),
            "refused {symbol} reported as closed: {w}"
        );
    }
}

fn assert_refused_only(out: &Outcome, symbol: &str) {
    assert_eq!(
        out.audit_transition.as_deref(),
        Some("FLATTEN_NOT_SUBMITTED"),
        "the audit trail must not record a submission that did not happen: {}",
        out.json
    );
    assert!(
        out.outbox_symbols.is_empty(),
        "an unproven position must produce zero outbox rows: {:?} / {}",
        out.outbox_symbols,
        out.json
    );
    assert_eq!(
        out.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "{}",
        out.json
    );
    assert_eq!(out.json["accepted"], false, "{}", out.json);
    assert_eq!(
        out.json["disposition"], "all_enqueue_failed",
        "{}",
        out.json
    );
    assert!(
        refusal_warning(&out.json, symbol).is_some(),
        "the refusal of {symbol} must be reported: {}",
        out.json
    );
    assert_not_claimed_closed(&out.json, symbol);
}

/// FP-01
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fp01_proven_canonical_equity_without_registry_v2_is_closed() {
    let _guard = DB_LOCK.lock().await;
    let out = operator_flatten(V2::NotConfigured, vec![("AAPL", qty(10))]).await;
    assert_eq!(out.status, StatusCode::OK, "{}", out.json);
    assert_eq!(out.json["disposition"], "enqueued", "{}", out.json);
    assert_eq!(out.audit_transition.as_deref(), Some("FLATTEN_SUBMITTED"));
    assert_eq!(out.outbox_symbols, vec!["AAPL".to_string()]);
}

/// FP-02
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fp02_proven_equity_with_valid_non_contradictory_registry_v2_is_closed() {
    let _guard = DB_LOCK.lock().await;
    for rows in [
        // v2 lists the same symbol as an Equity.
        vec![definition("AAPL", "equity")],
        // v2 lists only other instruments.
        vec![definition("BTC/USD", "crypto")],
    ] {
        let out = operator_flatten(v2_rows(rows), vec![("AAPL", qty(10))]).await;
        assert_eq!(out.status, StatusCode::OK, "{}", out.json);
        assert_eq!(out.json["disposition"], "enqueued", "{}", out.json);
        assert_eq!(out.outbox_symbols, vec!["AAPL".to_string()]);
    }
}

/// FP-03
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fp03_known_crypto_in_registry_v2_is_refused() {
    let _guard = DB_LOCK.lock().await;
    let out = operator_flatten(
        v2_rows(vec![definition("BTC/USD", "crypto")]),
        vec![("BTC/USD", QtyMicros::new(500_000))],
    )
    .await;
    assert_refused_only(&out, "BTC/USD");
}

/// FP-04
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fp04_occ_option_unknown_to_both_registries_is_refused() {
    let _guard = DB_LOCK.lock().await;
    for v2 in [
        V2::NotConfigured,
        v2_rows(vec![definition("BTC/USD", "crypto")]),
    ] {
        let out = operator_flatten(v2, vec![(OCC_OPTION, qty(1))]).await;
        assert_refused_only(&out, OCC_OPTION);
    }
}

/// FP-05
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fp05_crypto_position_with_registry_v2_absent_is_refused() {
    let _guard = DB_LOCK.lock().await;
    let out = operator_flatten(
        V2::NotConfigured,
        vec![("BTC/USD", QtyMicros::new(500_000))],
    )
    .await;
    assert_refused_only(&out, "BTC/USD");
}

/// FP-06
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fp06_configured_registry_v2_unreadable_or_malformed_refuses_even_an_equity() {
    let _guard = DB_LOCK.lock().await;
    for v2 in [
        V2::Unreadable,
        V2::Bytes(b"{ this is not json".to_vec()),
        V2::Bytes(Vec::new()),
    ] {
        let out = operator_flatten(v2, vec![("AAPL", qty(10))]).await;
        assert_refused_only(&out, "AAPL");
    }
}

/// FP-07
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fp07_conflicting_or_invalid_registry_v2_refuses_an_equity() {
    let _guard = DB_LOCK.lock().await;
    let mut case_conflict = definition("aapl", "crypto");
    case_conflict.instrument_id = "crypto:GLOBAL:aapl".to_string();
    let mut bogus_class = definition("MSFT", "equity");
    bogus_class.asset_class = "not_a_class".to_string();
    let unsupported_schema = V2::Bytes(
        serde_json::to_vec(&InstrumentRegistryV2 {
            schema_version: 9999,
            instruments: vec![definition("AAPL", "equity")],
        })
        .unwrap(),
    );
    let cases = vec![
        // Same symbol listed twice, Equity first: first-match must not win.
        v2_rows(vec![definition("AAPL", "equity"), {
            let mut d = definition("AAPL", "crypto");
            d.instrument_id = "crypto:GLOBAL:AAPL-dup".to_string();
            d
        }]),
        // Same symbol differing only by case, classified non-Equity.
        v2_rows(vec![definition("AAPL", "equity"), case_conflict]),
        // An unrelated invalid row poisons the whole configured source.
        v2_rows(vec![definition("AAPL", "equity"), bogus_class]),
        unsupported_schema,
    ];
    for v2 in cases {
        let out = operator_flatten(v2, vec![("AAPL", qty(10))]).await;
        assert_refused_only(&out, "AAPL");
    }
}

/// FP-08
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fp08_mixed_portfolio_closes_only_proven_equities_and_reports_the_rest() {
    let _guard = DB_LOCK.lock().await;
    let out = operator_flatten(
        v2_rows(vec![definition("BTC/USD", "crypto")]),
        vec![
            ("AAPL", qty(10)),
            ("SPY", qty(5)),
            ("BTC/USD", QtyMicros::new(500_000)),
            (OCC_OPTION, qty(2)),
            // An ordinary-looking ticker no registry lists.
            ("ZZZZ", qty(3)),
        ],
    )
    .await;

    assert_eq!(out.status, StatusCode::OK, "{}", out.json);
    assert_eq!(out.json["accepted"], true, "{}", out.json);
    assert_eq!(
        out.json["disposition"], "partial_enqueue_failed",
        "{}",
        out.json
    );
    assert_eq!(
        out.outbox_symbols,
        vec!["AAPL".to_string(), "SPY".to_string()],
        "only positively proven Equities may be enqueued"
    );
    for refused in ["BTC/USD", OCC_OPTION, "ZZZZ"] {
        let warning = refusal_warning(&out.json, refused)
            .unwrap_or_else(|| panic!("{refused} refusal not reported: {}", out.json));
        assert!(
            warning.contains("not closed") || warning.contains("refused"),
            "{warning}"
        );
        assert_not_claimed_closed(&out.json, refused);
    }
    let blockers = out.json["blockers"].to_string();
    let listed: std::collections::BTreeSet<&str> = blockers
        .split("NOT closed): ")
        .nth(1)
        .expect("refusal blocker present")
        .trim_end_matches("\"]")
        .split(", ")
        .collect();
    assert_eq!(
        listed,
        ["BTC/USD", OCC_OPTION, "ZZZZ"].into_iter().collect(),
        "exactly the unproven positions are reported refused: {blockers}"
    );
}

/// FP-09
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fp09_unlisted_equity_ticker_is_refused_with_an_actionable_warning() {
    let _guard = DB_LOCK.lock().await;
    let out = operator_flatten(V2::NotConfigured, vec![("ZZZZ", qty(3))]).await;
    assert_refused_only(&out, "ZZZZ");
    let warning = refusal_warning(&out.json, "ZZZZ").unwrap();
    assert!(
        warning.contains("registry") && warning.contains("broker"),
        "the warning must tell the operator what to do: {warning}"
    );
}

/// Run the automatic pre-event flatten (`enqueue_pre_event_flatten_closes`)
/// for a Paper run holding `positions`, with the blackout source unreadable so
/// the trigger is `Unavailable` (fail-closed flatten-required) for every
/// position. Returns the enqueued count and the outbox symbols.
async fn pre_event_flatten(v2: V2, positions: Vec<(&str, QtyMicros)>) -> (usize, Vec<String>) {
    let pool = db_pool().await;
    // Sole env writer in this test binary.
    std::env::set_var(
        mqk_daemon::event_risk_blackout::ENV_BLACKOUT_PATH,
        "/nonexistent/fp01-blackout.json",
    );
    std::env::remove_var(mqk_daemon::earnings_calendar::ENV_EARNINGS_CALENDAR_PATH);
    let mut st = common::with_canonical_equity_registry(AppState::new_for_test_with_mode(
        mqk_daemon::state::DeploymentMode::Paper,
    ));
    st.trading_instrument_registry_v2_path = v2_path(v2);

    let run_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now,
            git_hash: "fp01-test".to_string(),
            config_hash: "fp01-config".to_string(),
            config_json: serde_json::json!({"source": "scenario_multiasset_flatten_equity_proof_01"}),
            host_fingerprint: "fp01-host".to_string(),
        },
    )
    .await
    .expect("insert run");
    mqk_db::arm_run(&pool, run_id).await.expect("arm run");
    mqk_db::begin_run(&pool, run_id).await.expect("begin run");
    mqk_db::heartbeat_run(&pool, run_id, now)
        .await
        .expect("heartbeat run");

    let positions: Vec<(String, QtyMicros)> = positions
        .into_iter()
        .map(|(symbol, qty)| (symbol.to_string(), qty))
        .collect();
    let n = mqk_daemon::pre_event_flatten::enqueue_pre_event_flatten_closes(
        &st,
        mqk_daemon::state::DeploymentMode::Paper,
        &pool,
        run_id,
        &positions,
    )
    .await;
    let symbols = sqlx::query_scalar::<_, String>(
        "SELECT order_json->>'symbol' FROM oms_outbox WHERE run_id = $1 ORDER BY 1",
    )
    .bind(run_id)
    .fetch_all(&pool)
    .await
    .expect("outbox symbols");
    (n, symbols)
}

/// FP-10: the automatic flatten closes a proven Equity and never writes an
/// Equity row for an unproven position, whatever the evidence failure is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fp10_pre_event_flatten_never_writes_an_equity_row_for_an_unproven_position() {
    let _guard = DB_LOCK.lock().await;

    // Controls: canonical Equity proof, with and without a valid registry-v2.
    let (n, rows) = pre_event_flatten(V2::NotConfigured, vec![("AAPL", qty(10))]).await;
    assert_eq!((n, rows), (1, vec!["AAPL".to_string()]));
    let (n, rows) = pre_event_flatten(
        v2_rows(vec![definition("BTC/USD", "crypto")]),
        vec![("AAPL", qty(10))],
    )
    .await;
    assert_eq!((n, rows), (1, vec!["AAPL".to_string()]));

    // Unknown OCC option, unlisted ticker, and a crypto pair with registry-v2
    // absent: no row.
    let (n, rows) = pre_event_flatten(
        V2::NotConfigured,
        vec![
            (OCC_OPTION, qty(1)),
            ("ZZZZ", qty(3)),
            ("BTC/USD", QtyMicros::new(500_000)),
        ],
    )
    .await;
    assert_eq!((n, rows), (0, Vec::<String>::new()));

    // A known Crypto listing is refused.
    let (n, rows) = pre_event_flatten(
        v2_rows(vec![definition("BTC/USD", "crypto")]),
        vec![("BTC/USD", QtyMicros::new(500_000))],
    )
    .await;
    assert_eq!((n, rows), (0, Vec::<String>::new()));

    // Unreadable, malformed, or contradictory registry-v2 refuses even AAPL.
    let mut duplicate = definition("AAPL", "crypto");
    duplicate.instrument_id = "crypto:GLOBAL:AAPL-dup".to_string();
    for v2 in [
        V2::Unreadable,
        V2::Bytes(b"not json".to_vec()),
        v2_rows(vec![definition("AAPL", "equity"), duplicate]),
    ] {
        let (n, rows) = pre_event_flatten(v2, vec![("AAPL", qty(10))]).await;
        assert_eq!((n, rows), (0, Vec::<String>::new()));
    }

    // Mixed portfolio: only the proven Equities.
    let (n, rows) = pre_event_flatten(
        v2_rows(vec![definition("BTC/USD", "crypto")]),
        vec![
            ("AAPL", qty(10)),
            ("SPY", qty(5)),
            ("BTC/USD", QtyMicros::new(500_000)),
            (OCC_OPTION, qty(2)),
            ("ZZZZ", qty(3)),
        ],
    )
    .await;
    assert_eq!((n, rows), (2, vec!["AAPL".to_string(), "SPY".to_string()]));
}

/// Copy of the canonical v1 registry with `edit` applied to its JSON rows.
fn v1_registry_file(edit: impl FnOnce(&mut Vec<serde_json::Value>)) -> String {
    let mut rows: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(common::canonical_equity_registry_path()).unwrap())
            .expect("canonical registry parses");
    edit(&mut rows);
    let dir = std::env::temp_dir().join(format!("mqk_fp11_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("fixture dir");
    let path = dir.join("equities.json");
    std::fs::write(&path, serde_json::to_vec(&rows).unwrap()).expect("write");
    path.to_string_lossy().into_owned()
}

fn aapl_row(rows: &[serde_json::Value]) -> serde_json::Value {
    rows.iter()
        .find(|r| r["symbol"] == "AAPL")
        .expect("AAPL in canonical registry")
        .clone()
}

/// FP-11: the canonical registry is the positive proof, so a listing that
/// is disabled, non-Equity, duplicated (even by case) or unreadable proves
/// nothing, and a contradictory second row cannot hide behind a first row
/// that says Equity.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn fp11_canonical_registry_contradictions_never_prove_an_equity() {
    let _guard = DB_LOCK.lock().await;

    let disabled = v1_registry_file(|rows| {
        for r in rows.iter_mut().filter(|r| r["symbol"] == "AAPL") {
            r["enabled"] = serde_json::Value::Bool(false);
        }
    });
    let non_equity = v1_registry_file(|rows| {
        for r in rows.iter_mut().filter(|r| r["symbol"] == "AAPL") {
            r["asset_class"] = "crypto".into();
        }
    });
    let case_duplicate = v1_registry_file(|rows| {
        let mut dup = aapl_row(rows);
        dup["symbol"] = "aapl".into();
        dup["instrument_id"] = "crypto:US:aapl".into();
        dup["provider_symbol"] = "aapl".into();
        dup["asset_class"] = "crypto".into();
        rows.push(dup);
    });
    let exact_duplicate = v1_registry_file(|rows| {
        let mut dup = aapl_row(rows);
        dup["instrument_id"] = "equity:US:AAPL-dup".into();
        rows.push(dup);
    });
    let unreadable = std::env::temp_dir()
        .join(format!("mqk_fp11_missing_{}.json", Uuid::new_v4()))
        .to_string_lossy()
        .into_owned();

    for v1 in [
        disabled,
        non_equity,
        case_duplicate,
        exact_duplicate,
        unreadable,
    ] {
        let out =
            operator_flatten_with_v1(Some(v1.clone()), V2::NotConfigured, vec![("AAPL", qty(10))])
                .await;
        assert_refused_only(&out, "AAPL");
        let _ = v1;
    }

    // Control: the unmodified canonical registry still proves AAPL.
    let out = operator_flatten(V2::NotConfigured, vec![("AAPL", qty(10))]).await;
    assert_eq!(out.outbox_symbols, vec!["AAPL".to_string()]);
}
