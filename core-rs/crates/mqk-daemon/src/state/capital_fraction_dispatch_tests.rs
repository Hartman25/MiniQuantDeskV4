//! Durable capital-fraction hosts inside the daemon selected-host dispatch.
//!
//! DB-backed tests use the port-5434 disposable test database and skip when
//! `MQK_DATABASE_URL` is absent (this crate's `db_pool_or_skip` convention).
//! "Restart" drops the host pool and rebuilds it from the database only.
//! Nothing here activates Paper, touches a broker, or submits an order.

use super::*;
use crate::dynamic_selection_dispatch_authority::SelectedDispatchBinding;
use crate::dynamic_selection_host_pool::{
    DynamicSelectionHostPool, HostPoolBuildError, RuntimeSelectedStrategyHost,
};
use mqk_db::held_sizing_state::held_sizing_fetch;
use mqk_runtime::native_strategy::{
    resolve_capital_fraction_paper_binding, StrategyBootstrapInputs,
};
use sqlx::PgPool;

const USD: i64 = 1_000_000;
const DAY: i64 = 86_400;
const BASE_TS: i64 = 1_700_000_000;
const CF: &str = "fixed_initial_capital_fraction_v1";
const TREND: &str = "trend_sma50";
const DUAL: &str = "dual_sma_50_200_trend";

fn uniq(tag: &str) -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|t| t.as_nanos())
        .unwrap_or(0);
    format!("CF{tag}{}", n % 1_000_000_000)
}

fn cf_inputs(symbol: &str) -> StrategyBootstrapInputs {
    StrategyBootstrapInputs {
        symbol: symbol.to_string(),
        raw_sizing_policy: Some(CF.to_string()),
        raw_allocation_fraction_bps: Some("1000".to_string()),
        raw_allocated_capital_micros: Some("100000000000".to_string()),
        ..Default::default()
    }
}

fn fixed_inputs(symbol: &str) -> StrategyBootstrapInputs {
    StrategyBootstrapInputs {
        symbol: symbol.to_string(),
        raw_target_qty: Some("3".to_string()),
        ..Default::default()
    }
}

fn keys(pairs: &[(&str, &str)]) -> Vec<(String, String, i64)> {
    pairs
        .iter()
        .map(|(s, st)| (s.to_string(), st.to_string(), DAY))
        .collect()
}

fn binding(symbol: &str, strategy_id: &str) -> SelectedDispatchBinding {
    SelectedDispatchBinding {
        symbol: symbol.to_string(),
        strategy_id: strategy_id.to_string(),
        timeframe_secs: DAY,
        db_timeframe_label: "1D".to_string(),
        selection_reason_code: "test_fixture".to_string(),
        plan_id: Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"cfd.test.plan"),
    }
}

fn run_id() -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"cfd.test.run")
}

async fn db_or_skip(label: &str) -> Option<PgPool> {
    let Ok(url) = std::env::var("MQK_DATABASE_URL") else {
        eprintln!("{label}: MQK_DATABASE_URL not set; skipped");
        return None;
    };
    if !url.contains(":5434") {
        eprintln!("{label}: MQK_DATABASE_URL must be the port-5434 local test DB; skipped");
        return None;
    }
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
        .ok()?;
    mqk_db::migrate(&pool).await.ok()?;
    Some(pool)
}

fn end_ts(k: i64) -> i64 {
    BASE_TS + k * DAY
}

async fn seed_bar(pool: &PgPool, symbol: &str, k: i64, close_dollars: i64) {
    let c = close_dollars * USD;
    sqlx::query(
        "insert into md_bars (symbol, timeframe, end_ts, open_micros, high_micros, low_micros, \
         close_micros, volume, is_complete, provider_id, provider_source, provider_symbol, \
         ingest_mode, ingested_at) values ($1,'1D',$2,$3,$3,$3,$3,1000,true,'cfd_test','cfd_test',\
         $1,'historical_sync',now()) on conflict do nothing",
    )
    .bind(symbol)
    .bind(end_ts(k))
    .bind(c)
    .execute(pool)
    .await
    .expect("seed bar");
}

async fn seed_rising(pool: &PgPool, symbol: &str, n: i64) {
    for k in 0..n {
        seed_bar(pool, symbol, k, 100 + k).await;
    }
}

async fn cleanup(pool: &PgPool, symbols: &[&str]) {
    for s in symbols {
        let _ = sqlx::query("delete from md_bars where symbol = $1 and provider_id = 'cfd_test'")
            .bind(s)
            .execute(pool)
            .await;
    }
}

fn state_with_db(pool: &PgPool) -> Arc<AppState> {
    let mut state =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Paper);
    state.db = Some(pool.clone());
    state.set_per_symbol_bar_staleness_secs_for_test(Some(10_000_000_000));
    Arc::new(state)
}

async fn build(
    selected: &[(String, String, i64)],
    pool: &PgPool,
) -> Result<DynamicSelectionHostPool, HostPoolBuildError> {
    DynamicSelectionHostPool::build_with_durable_state_from(selected, Some(pool), |s: &str| {
        cf_inputs(s)
    })
    .await
}

type TickOut = Result<
    Vec<(
        SymbolStrategyAssignment,
        mqk_strategy::StrategyBarResult,
        Option<EvaluatedBarFacts>,
    )>,
    SelectedHostDispatchFault,
>;

async fn tick(
    state: &Arc<AppState>,
    bindings: &[SelectedDispatchBinding],
    host_pool: &mut DynamicSelectionHostPool,
    k: i64,
) -> TickOut {
    state
        .deposit_strategy_bar_input(StrategyBarInput {
            now_tick: k as u64 + 1,
            end_ts: end_ts(k),
            limit_price: None,
            qty: 0,
        })
        .await;
    state
        .tick_strategy_dispatch_selected_hosts_with_bar_facts(run_id(), bindings, host_pool)
        .await
}

fn qty_of(out: &TickOut, idx: usize) -> i64 {
    out.as_ref()
        .expect("tick result")
        .get(idx)
        .expect("result")
        .1
        .intents
        .output
        .targets[0]
        .qty
        .raw()
}

fn deployment_id(symbol: &str, strategy: &str) -> String {
    resolve_capital_fraction_paper_binding(&cf_inputs(symbol), strategy)
        .expect("binding resolves")
        .expect("capital-fraction contract")
        .scope
        .deployment_id
}

async fn rows(
    pool: &PgPool,
    symbol: &str,
    strategy: &str,
) -> Vec<mqk_db::held_sizing_state::HeldSizingRow> {
    held_sizing_fetch(pool, &deployment_id(symbol, strategy), strategy)
        .await
        .expect("fetch held sizing")
}

async fn seed_ohlc(pool: &PgPool, symbol: &str, k: i64, ohlc: (i64, i64, i64, i64)) {
    sqlx::query(
        "insert into md_bars (symbol, timeframe, end_ts, open_micros, high_micros, low_micros,          close_micros, volume, is_complete, provider_id, provider_source, provider_symbol,          ingest_mode, ingested_at) values ($1,'1D',$2,$3,$4,$5,$6,1000,true,'cfd_test','cfd_test',         $1,'historical_sync',now()) on conflict do nothing",
    )
    .bind(symbol)
    .bind(end_ts(k))
    .bind(ohlc.0)
    .bind(ohlc.1)
    .bind(ohlc.2)
    .bind(ohlc.3)
    .execute(pool)
    .await
    .expect("seed ohlc bar");
}

/// F03 fixture: flat 60 -> ramp to 100 -> flat 100 (true range 2), then a >1.5-ATR 3-day drop whose
/// decision bar is index 199 (close 96.5), then closes of 90. Every bar's open equals its close.
fn f03_bars(n: usize) -> Vec<(i64, i64, i64, i64)> {
    f03_bars_padded(n, USD)
}

fn f03_bars_padded(n: usize, flat_pad: i64) -> Vec<(i64, i64, i64, i64)> {
    const M: i64 = USD;
    let mut v: Vec<(i64, i64, i64, i64)> = Vec::new();
    let push = |prev: i64, c: i64, pad: i64, v: &mut Vec<(i64, i64, i64, i64)>| {
        v.push((c, prev.max(c) + pad, prev.min(c) - pad, c));
    };
    for _ in 0..=99 {
        push(60 * M, 60 * M, 0, &mut v);
    }
    for i in 100..=149i64 {
        let c = 60 * M + 40 * M * (i - 99) / 50;
        let prev = v.last().unwrap().3;
        push(prev, c, 0, &mut v);
    }
    for _ in 150..=196 {
        push(100 * M, 100 * M, flat_pad, &mut v);
    }
    for (c, pad) in [
        (98 * M, 0),
        (100 * M, 0),
        (96 * M + M / 2, 5 * M),
        (96 * M + M / 2, M),
        (90 * M, M),
    ] {
        let prev = v.last().unwrap().3;
        push(prev, c, pad, &mut v);
    }
    while v.len() < n {
        push(90 * M, 90 * M, M, &mut v);
    }
    v.truncate(n);
    v
}

const F03: &str = "trend_filtered_extreme_3d_atr_reversal_v1";

#[tokio::test]
async fn stateful_inner_strategy_hold_phase_survives_restart_at_every_boundary() {
    let Some(db) = db_or_skip("CFD-02").await else {
        return;
    };
    let sym = uniq("F03");
    let bars = f03_bars(206);
    for (k, b) in bars.iter().enumerate().take(200) {
        seed_ohlc(&db, &sym, k as i64, *b).await;
    }
    let state = state_with_db(&db);
    let sel = keys(&[(&sym, F03)]);
    let b = vec![binding(&sym, F03)];

    // Entry at the decision bar 199 (close $96.5): floor(10,000 / 96.5) = 103.
    let mut p = build(&sel, &db).await.expect("pool");
    assert_eq!(qty_of(&tick(&state, &b, &mut p, 199).await, 0), 103 * USD);
    // Restart before EVERY later bar. The closes are now $90 (re-resolving would give 111); a
    // reset hold phase would turn flat at bar 200.
    let mut seq = Vec::new();
    for k in 200..=204i64 {
        drop(p);
        seed_ohlc(&db, &sym, k, bars[k as usize]).await;
        p = build(&sel, &db).await.expect("recovered pool");
        seq.push(qty_of(&tick(&state, &b, &mut p, k).await, 0));
    }
    assert_eq!(
        seq,
        vec![103 * USD, 103 * USD, 103 * USD, 103 * USD, 0],
        "hold bars 2-5, then flat"
    );
    let r = rows(&db, &sym, F03).await;
    assert_eq!(
        (r[0].status.as_str(), r[0].entry_generation),
        ("released", 1)
    );
    cleanup(&db, &[&sym]).await;
}

/// Same close path as the F03 entry fixture, but every flat session has a true range of 10 (ATR
/// ~9.2), so the 3.5 drop is NOT an event. Close-only bars (range 0, ATR ~0.2) would enter.
#[tokio::test]
async fn daemon_bar_window_carries_true_ohlc_to_the_atr_engine() {
    let Some(db) = db_or_skip("CFD-OHLC").await else {
        return;
    };
    let sym = uniq("OHL");
    let bars = f03_bars_padded(200, 5 * USD);
    for (k, b) in bars.iter().enumerate() {
        seed_ohlc(&db, &sym, k as i64, *b).await;
    }
    let state = state_with_db(&db);
    let mut p = build(&keys(&[(&sym, F03)]), &db).await.expect("pool");
    let out = tick(&state, &[binding(&sym, F03)], &mut p, 199).await;
    assert_eq!(qty_of(&out, 0), 0, "wide true ranges: no entry");
    assert!(rows(&db, &sym, F03).await.is_empty(), "nothing held");
    cleanup(&db, &[&sym]).await;
}

#[tokio::test]
async fn restart_keeps_exact_q_exit_releases_and_reentry_resolves_anew() {
    let Some(db) = db_or_skip("CFD-01").await else {
        return;
    };
    let sym = uniq("RST");
    seed_rising(&db, &sym, 60).await;
    let state = state_with_db(&db);
    let sel = keys(&[(&sym, TREND)]);
    let b = vec![binding(&sym, TREND)];

    // Flat->long entry at the causal completed close ($159): 10,000/159 = 62.
    let mut p = build(&sel, &db).await.expect("pool");
    assert_eq!(qty_of(&tick(&state, &b, &mut p, 59).await, 0), 62 * USD);
    let r = rows(&db, &sym, TREND).await;
    assert_eq!(r.len(), 1);
    assert_eq!(
        (
            r[0].status.as_str(),
            r[0].entry_generation,
            r[0].resolved_target_qty_micros
        ),
        ("active", 1, 62 * USD)
    );
    let before = r[0].clone();

    // RESTART with a much higher close: re-resolving would give 33, a reset 1.
    drop(p);
    seed_bar(&db, &sym, 60, 300).await;
    let mut p = build(&sel, &db).await.expect("recovered pool");
    assert_eq!(qty_of(&tick(&state, &b, &mut p, 60).await, 0), 62 * USD);
    assert_eq!(rows(&db, &sym, TREND).await, vec![before]);

    // Exit releases generation 1 durably; restart while flat stays flat.
    drop(p);
    seed_bar(&db, &sym, 61, 1).await;
    let mut p = build(&sel, &db).await.expect("pool");
    assert_eq!(qty_of(&tick(&state, &b, &mut p, 61).await, 0), 0);
    assert_eq!(rows(&db, &sym, TREND).await[0].status, "released");
    drop(p);
    seed_bar(&db, &sym, 62, 1).await;
    let mut p = build(&sel, &db).await.expect("pool");
    assert_eq!(qty_of(&tick(&state, &b, &mut p, 62).await, 0), 0);

    // New entry: same initial capital, new causal close $400 -> 25, generation 2.
    seed_bar(&db, &sym, 63, 400).await;
    assert_eq!(qty_of(&tick(&state, &b, &mut p, 63).await, 0), 25 * USD);
    let r = rows(&db, &sym, TREND).await;
    assert_eq!(
        (
            r[0].status.as_str(),
            r[0].entry_generation,
            r[0].resolved_target_qty_micros
        ),
        ("active", 2, 25 * USD)
    );
    cleanup(&db, &[&sym]).await;
}

#[tokio::test]
async fn bindings_are_isolated_by_symbol_and_by_strategy() {
    let Some(db) = db_or_skip("CFD-02").await else {
        return;
    };
    let (s1, s2) = (uniq("ISA"), uniq("ISB"));
    seed_rising(&db, &s1, 260).await;
    for k in 0..260 {
        seed_bar(&db, &s2, k, 200 + k).await;
    }
    let state = state_with_db(&db);
    let sel = keys(&[(&s1, TREND), (&s2, TREND), (&s1, DUAL)]);
    let b = vec![binding(&s1, TREND), binding(&s2, TREND), binding(&s1, DUAL)];
    let mut p = build(&sel, &db).await.expect("pool");
    let out = tick(&state, &b, &mut p, 259).await;
    let q_s1 = qty_of(&out, 0);
    let q_s2 = qty_of(&out, 1);
    assert_eq!(q_s1, 10_000 / 359 * USD);
    assert_eq!(q_s2, 10_000 / 459 * USD);
    assert_ne!(q_s1, q_s2);

    let ids = [
        deployment_id(&s1, TREND),
        deployment_id(&s2, TREND),
        deployment_id(&s1, DUAL),
    ];
    assert_eq!(
        ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
        3,
        "no two bindings share a deployment identity"
    );
    for (sym, st, q) in [(&s1, TREND, q_s1), (&s2, TREND, q_s2), (&s1, DUAL, q_s1)] {
        let r = rows(&db, sym, st).await;
        assert_eq!(r.len(), 1, "{sym}/{st}");
        assert_eq!(r[0].resolved_target_qty_micros, q);
    }
    // Restart: every binding recovers its own Q only.
    drop(p);
    let mut p = build(&sel, &db).await.expect("pool");
    seed_bar(&db, &s1, 260, 900).await;
    seed_bar(&db, &s2, 260, 900).await;
    let out = tick(&state, &b, &mut p, 260).await;
    assert_eq!(
        (qty_of(&out, 0), qty_of(&out, 1), qty_of(&out, 2)),
        (q_s1, q_s2, q_s1)
    );
    cleanup(&db, &[&s1, &s2]).await;
}

#[tokio::test]
async fn persistence_failure_returns_no_result_and_poisons_the_host() {
    let Some(db) = db_or_skip("CFD-03").await else {
        return;
    };
    let sym = uniq("PRF");
    seed_rising(&db, &sym, 60).await;
    let state = state_with_db(&db);
    let sel = keys(&[(&sym, TREND)]);
    let b = vec![binding(&sym, TREND)];
    let dep = deployment_id(&sym, TREND);
    let fname = format!("cfd_fail_{}", sym.to_lowercase());
    sqlx::query(&format!(
        "create function {fname}() returns trigger language plpgsql as $$ begin \
         if new.deployment_id = '{dep}' then raise exception 'forced persistence failure'; end if; \
         return new; end $$"
    ))
    .execute(&db)
    .await
    .expect("create fault fn");
    sqlx::query(&format!(
        "create trigger {fname} before insert or update on sys_strategy_held_sizing_state \
         for each row execute function {fname}()"
    ))
    .execute(&db)
    .await
    .expect("create fault trigger");

    let mut p = build(&sel, &db).await.expect("pool");
    let out = tick(&state, &b, &mut p, 59).await;
    assert!(
        matches!(out, Err(SelectedHostDispatchFault::HostOnBarError { .. })),
        "a persistence error must surface as a whole-tick fault with no result: {out:?}"
    );
    assert!(rows(&db, &sym, TREND).await.is_empty());
    let host = p.get_mut(&sym, TREND, DAY).expect("host");
    let RuntimeSelectedStrategyHost::DurableCapitalFraction(h) = host else {
        panic!("capital-fraction binding must own a durable host");
    };
    assert!(h.is_poisoned());

    sqlx::query(&format!(
        "drop trigger {fname} on sys_strategy_held_sizing_state"
    ))
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(&format!("drop function {fname}()"))
        .execute(&db)
        .await
        .unwrap();
    assert!(
        tick(&state, &b, &mut p, 59).await.is_err(),
        "a poisoned host refuses further bars even once the DB is healthy; it is never silently rebuilt"
    );
    assert!(rows(&db, &sym, TREND).await.is_empty());
    drop(p);
    let mut p = build(&sel, &db).await.expect("pool");
    assert_eq!(qty_of(&tick(&state, &b, &mut p, 59).await, 0), 62 * USD);
    cleanup(&db, &[&sym]).await;
}

fn host_is_poisoned(p: &mut DynamicSelectionHostPool, sym: &str, strategy: &str) -> bool {
    let RuntimeSelectedStrategyHost::DurableCapitalFraction(h) =
        p.get_mut(sym, strategy, DAY).expect("host")
    else {
        panic!("capital-fraction binding must own a durable host");
    };
    h.is_poisoned()
}

/// IR-CF-01: a later binding's whole-tick fault must not leave an earlier
/// binding's held sizing advanced.
#[tokio::test]
async fn whole_tick_fault_in_a_later_binding_commits_no_earlier_durable_transition() {
    let Some(db) = db_or_skip("CFD-ATOM-FAULT").await else {
        return;
    };
    let (a, b_sym) = (uniq("ATA"), uniq("ATB"));
    seed_rising(&db, &a, 60).await;
    seed_rising(&db, &b_sym, 60).await;
    let state = state_with_db(&db);
    let sel = keys(&[(&a, TREND), (&b_sym, TREND)]);
    let bindings = vec![binding(&a, TREND), binding(&b_sym, TREND)];

    let mut p = build(&sel, &db).await.expect("pool");
    state
        .set_panic_on_symbol_for_test(Some(b_sym.clone()))
        .await;
    let out = tick(&state, &bindings, &mut p, 59).await;
    assert!(
        matches!(
            out,
            Err(SelectedHostDispatchFault::HostOnBarPanicked { .. })
        ),
        "binding B fails after binding A evaluated: {out:?}"
    );
    assert!(
        rows(&db, &a, TREND).await.is_empty(),
        "A's entry must not be durable when the tick failed closed"
    );
    assert!(rows(&db, &b_sym, TREND).await.is_empty());
    assert!(
        host_is_poisoned(&mut p, &a, TREND),
        "prepared A is invalidated"
    );
    assert!(
        !host_is_poisoned(&mut p, &b_sym, TREND),
        "B faulted before it evaluated, so it holds no un-persisted state"
    );
    state.set_panic_on_symbol_for_test(None).await;
    assert!(
        tick(&state, &bindings, &mut p, 59).await.is_err(),
        "an aborted prepared host is never reused without recovery"
    );

    // Recovery starts from the old durable state, not the aborted tick's memory.
    drop(p);
    let mut p = build(&sel, &db).await.expect("recovered pool");
    let out = tick(&state, &bindings, &mut p, 59).await;
    assert_eq!((qty_of(&out, 0), qty_of(&out, 1)), (62 * USD, 62 * USD));
    assert_eq!(rows(&db, &a, TREND).await[0].entry_generation, 1);
    cleanup(&db, &[&a, &b_sym]).await;
}

/// IR-CF-01: a DB failure in the batch rolls back every binding's transition.
#[tokio::test]
async fn batch_commit_failure_rolls_back_every_binding_and_poisons_all() {
    let Some(db) = db_or_skip("CFD-ATOM-DB").await else {
        return;
    };
    let (a, b_sym) = (uniq("ADA"), uniq("ADB"));
    seed_rising(&db, &a, 60).await;
    seed_rising(&db, &b_sym, 60).await;
    let state = state_with_db(&db);
    let sel = keys(&[(&a, TREND), (&b_sym, TREND)]);
    let bindings = vec![binding(&a, TREND), binding(&b_sym, TREND)];
    let dep_b = deployment_id(&b_sym, TREND);
    let fname = format!("cfd_atom_{}", b_sym.to_lowercase());
    sqlx::query(&format!(
        "create function {fname}() returns trigger language plpgsql as $$ begin \
         if new.deployment_id = '{dep_b}' then raise exception 'forced batch failure'; end if; \
         return new; end $$"
    ))
    .execute(&db)
    .await
    .expect("create fault fn");
    sqlx::query(&format!(
        "create trigger {fname} before insert or update on sys_strategy_held_sizing_state \
         for each row execute function {fname}()"
    ))
    .execute(&db)
    .await
    .expect("create fault trigger");

    let mut p = build(&sel, &db).await.expect("pool");
    let out = tick(&state, &bindings, &mut p, 59).await;
    assert!(
        matches!(out, Err(SelectedHostDispatchFault::HostOnBarError { .. })),
        "{out:?}"
    );
    assert!(
        rows(&db, &a, TREND).await.is_empty(),
        "A was applied before B in the same transaction and must roll back"
    );
    assert!(rows(&db, &b_sym, TREND).await.is_empty());
    assert!(host_is_poisoned(&mut p, &a, TREND));
    assert!(host_is_poisoned(&mut p, &b_sym, TREND));

    sqlx::query(&format!(
        "drop trigger {fname} on sys_strategy_held_sizing_state"
    ))
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(&format!("drop function {fname}()"))
        .execute(&db)
        .await
        .unwrap();
    drop(p);
    let mut p = build(&sel, &db).await.expect("recovered pool");
    let out = tick(&state, &bindings, &mut p, 59).await;
    assert_eq!((qty_of(&out, 0), qty_of(&out, 1)), (62 * USD, 62 * USD));
    cleanup(&db, &[&a, &b_sym]).await;
}

/// IR-CF-01: two bindings with transitions commit together and release results.
#[tokio::test]
async fn two_durable_bindings_commit_one_batch_and_release_both_results() {
    let Some(db) = db_or_skip("CFD-ATOM-OK").await else {
        return;
    };
    let (a, b_sym) = (uniq("AOA"), uniq("AOB"));
    seed_rising(&db, &a, 60).await;
    seed_rising(&db, &b_sym, 60).await;
    let state = state_with_db(&db);
    let sel = keys(&[(&a, TREND), (&b_sym, TREND)]);
    let bindings = vec![binding(&a, TREND), binding(&b_sym, TREND)];
    let mut p = build(&sel, &db).await.expect("pool");
    let out = tick(&state, &bindings, &mut p, 59).await;
    assert_eq!((qty_of(&out, 0), qty_of(&out, 1)), (62 * USD, 62 * USD));
    for s in [&a, &b_sym] {
        let r = rows(&db, s, TREND).await;
        assert_eq!(r.len(), 1);
        assert_eq!(
            (r[0].status.as_str(), r[0].resolved_target_qty_micros),
            ("active", 62 * USD)
        );
    }
    assert!(!host_is_poisoned(&mut p, &a, TREND));
    // Finalized hosts evaluate the next bar normally.
    seed_bar(&db, &a, 60, 300).await;
    seed_bar(&db, &b_sym, 60, 300).await;
    let out = tick(&state, &bindings, &mut p, 60).await;
    assert_eq!((qty_of(&out, 0), qty_of(&out, 1)), (62 * USD, 62 * USD));
    cleanup(&db, &[&a, &b_sym]).await;
}

/// IR-CF-02: durable Q is the strategy-contract-capped target; the runtime
/// per-symbol position cap is a separate downstream clamp on the executable
/// target and never mutates, re-resolves or is baked into stored Q.
#[tokio::test]
async fn runtime_position_cap_clamps_the_executable_target_but_never_the_durable_q() {
    let Some(db) = db_or_skip("CFD-CAPQ").await else {
        return;
    };
    let sym = uniq("CPQ");
    seed_rising(&db, &sym, 60).await;
    let state = state_with_db(&db);
    state.set_per_symbol_max_position_qty_for_test(Some(10));
    let sel = keys(&[(&sym, TREND)]);
    let b = vec![binding(&sym, TREND)];

    let mut p = build(&sel, &db).await.expect("pool");
    let mut out = tick(&state, &b, &mut p, 59).await.expect("tick");
    assert_eq!(
        out[0].1.intents.output.targets[0].qty.raw(),
        62 * USD,
        "the dispatcher returns the raw durable Q, independent of the runtime cap"
    );
    let stored = rows(&db, &sym, TREND).await;
    assert_eq!(stored[0].resolved_target_qty_micros, 62 * USD);
    let clamped = AppState::clamp_targets_to_per_symbol_position_cap(
        &mut out[0].1.intents.output.targets,
        10,
    );
    assert_eq!(clamped.len(), 1);
    assert_eq!(out[0].1.intents.output.targets[0].qty.raw(), 10 * USD);

    // Restart: stored Q is unchanged and the downstream clamp re-applies identically.
    drop(p);
    let mut p = build(&sel, &db).await.expect("recovered pool");
    seed_bar(&db, &sym, 60, 300).await;
    let mut out = tick(&state, &b, &mut p, 60).await.expect("tick");
    assert_eq!(out[0].1.intents.output.targets[0].qty.raw(), 62 * USD);
    AppState::clamp_targets_to_per_symbol_position_cap(&mut out[0].1.intents.output.targets, 10);
    assert_eq!(out[0].1.intents.output.targets[0].qty.raw(), 10 * USD);

    // Removing or loosening the runtime cap cannot change stored Q.
    state.set_per_symbol_max_position_qty_for_test(None);
    let mut loose = out;
    loose[0].1.intents.output.targets[0].qty =
        mqk_schemas::QtyMicros::from_whole_units(62).unwrap();
    assert!(AppState::clamp_targets_to_per_symbol_position_cap(
        &mut loose[0].1.intents.output.targets,
        100
    )
    .is_empty());
    assert_eq!(rows(&db, &sym, TREND).await, stored);
    cleanup(&db, &[&sym]).await;
}

/// IR-CF-02: the strategy-contract caps are applied BEFORE the durable commit
/// and are bound to the stored record, so changing one refuses recovery.
#[tokio::test]
async fn strategy_contract_cap_is_applied_before_commit_and_bound_to_the_stored_record() {
    let Some(db) = db_or_skip("CFD-CAPID").await else {
        return;
    };
    let sym = uniq("CPI");
    seed_rising(&db, &sym, 60).await;
    let state = state_with_db(&db);
    let sel = keys(&[(&sym, TREND)]);
    let b = vec![binding(&sym, TREND)];
    let capped = |cap: &'static str| {
        move |s: &str| {
            let mut i = cf_inputs(s);
            i.raw_max_target_qty = Some(cap.to_string());
            i
        }
    };
    let build_capped = |cap: &'static str| {
        let (sel, db) = (sel.clone(), db.clone());
        async move {
            DynamicSelectionHostPool::build_with_durable_state_from(&sel, Some(&db), capped(cap))
                .await
        }
    };
    let dep = |cap: &'static str| {
        resolve_capital_fraction_paper_binding(&capped(cap)(&sym), TREND)
            .unwrap()
            .unwrap()
            .scope
            .deployment_id
    };
    assert_ne!(
        dep("30"),
        dep("40"),
        "a strategy-contract cap is part of identity"
    );
    assert_ne!(dep("30"), deployment_id(&sym, TREND));

    let mut p = build_capped("30").await.expect("pool");
    let out = tick(&state, &b, &mut p, 59).await;
    assert_eq!(
        qty_of(&out, 0),
        30 * USD,
        "62 shares capped to 30 before commit"
    );
    let stored = held_sizing_fetch(&db, &dep("30"), TREND).await.unwrap();
    assert_eq!(stored[0].resolved_target_qty_micros, 30 * USD);
    assert_eq!(stored[0].max_target_qty_micros, Some(30 * USD));
    cleanup(&db, &[&sym]).await;
}

#[tokio::test]
async fn recovery_failure_fails_the_pool_build_closed() {
    let Some(db) = db_or_skip("CFD-04").await else {
        return;
    };
    let sym = uniq("RCF");
    seed_rising(&db, &sym, 60).await;
    let state = state_with_db(&db);
    let sel = keys(&[(&sym, TREND)]);
    let b = vec![binding(&sym, TREND)];
    let mut p = build(&sel, &db).await.expect("pool");
    tick(&state, &b, &mut p, 59).await.expect("entry");
    drop(p);
    sqlx::query(
        "update sys_strategy_held_sizing_state set resolved_target_qty_micros = 63000000 \
         where deployment_id = $1",
    )
    .bind(deployment_id(&sym, TREND))
    .execute(&db)
    .await
    .unwrap();
    let err = build(&sel, &db)
        .await
        .err()
        .expect("tampered state must refuse");
    assert_eq!(err.code(), "host_pool_durable_recovery_failed");
    cleanup(&db, &[&sym]).await;
}

#[tokio::test]
async fn pool_build_never_writes_held_state_and_fixed_quantity_stays_stateless() {
    let Some(db) = db_or_skip("CFD-05").await else {
        return;
    };
    let (cf_sym, fx_sym) = (uniq("NWR"), uniq("FXQ"));
    let sel = keys(&[(&cf_sym, TREND), (&fx_sym, TREND)]);
    let mixed = |s: &str| {
        if s == cf_sym {
            cf_inputs(s)
        } else {
            fixed_inputs(s)
        }
    };
    let mut p = DynamicSelectionHostPool::build_with_durable_state_from(&sel, Some(&db), mixed)
        .await
        .expect("mixed pool");
    assert!(p.get_mut(&cf_sym, TREND, DAY).unwrap().is_durable());
    assert!(
        !p.get_mut(&fx_sym, TREND, DAY).unwrap().is_durable(),
        "fixed-quantity bindings keep the accepted stateless host"
    );
    // Read-only identity reads (start gate, dry-run, status) never mutate held state.
    for _ in 0..2 {
        for s in [&cf_sym, &fx_sym] {
            p.get_mut(s, TREND, DAY)
                .unwrap()
                .semantic_fingerprint()
                .unwrap();
        }
    }
    assert!(rows(&db, &cf_sym, TREND).await.is_empty());
    let sql = "select count(*) from sys_strategy_held_sizing_state where symbol = $1";
    let n: i64 = sqlx::query_scalar(sql)
        .bind(&fx_sym)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn capital_fraction_binding_without_a_database_is_refused() {
    let sym = "CFNODB";
    let sel = keys(&[(sym, TREND)]);
    let err =
        DynamicSelectionHostPool::build_with_durable_state_from(&sel, None, |s: &str| cf_inputs(s))
            .await
            .err()
            .expect("no db must refuse");
    assert_eq!(err.code(), "host_pool_durable_state_unavailable");
    // The fixed-quantity contract needs no database and builds stateless.
    let p = DynamicSelectionHostPool::build_with_durable_state_from(&sel, None, |s: &str| {
        fixed_inputs(s)
    })
    .await
    .expect("fixed quantity builds without db");
    assert_eq!(p.len(), 1);
}

#[tokio::test]
async fn partial_unknown_or_malformed_capital_fraction_config_is_refused_without_defaults() {
    let sym = "CFBAD";
    let sel = keys(&[(sym, TREND)]);
    type Mutation = Box<dyn Fn(&mut StrategyBootstrapInputs)>;
    let mutate: Vec<(&str, Mutation)> = vec![
        (
            "missing bps",
            Box::new(|i| i.raw_allocation_fraction_bps = None),
        ),
        (
            "missing capital",
            Box::new(|i| i.raw_allocated_capital_micros = None),
        ),
        (
            "unknown policy",
            Box::new(|i| i.raw_sizing_policy = Some("fixed_initial_capital_fraction_v9".into())),
        ),
        (
            "zero bps",
            Box::new(|i| i.raw_allocation_fraction_bps = Some("0".into())),
        ),
        (
            "over 100%",
            Box::new(|i| i.raw_allocation_fraction_bps = Some("10001".into())),
        ),
        (
            "malformed bps",
            Box::new(|i| i.raw_allocation_fraction_bps = Some("10x".into())),
        ),
        (
            "zero capital",
            Box::new(|i| i.raw_allocated_capital_micros = Some("0".into())),
        ),
        (
            "negative capital",
            Box::new(|i| i.raw_allocated_capital_micros = Some("-5".into())),
        ),
        (
            "fixed qty with fraction",
            Box::new(|i| i.raw_target_qty = Some("2".into())),
        ),
        (
            "bps without policy",
            Box::new(|i| i.raw_sizing_policy = None),
        ),
    ];
    for (label, m) in mutate {
        let mut inputs = cf_inputs(sym);
        m(&mut inputs);
        let r = DynamicSelectionHostPool::build_with_durable_state_from(&sel, None, |_: &str| {
            inputs.clone()
        })
        .await;
        let err = r
            .err()
            .unwrap_or_else(|| panic!("{label}: must be refused"));
        assert_eq!(
            err.code(),
            "host_pool_capital_fraction_contract_refused",
            "{label}"
        );
    }
}

#[test]
fn deployment_identity_is_stable_and_distinct_across_every_dimension() {
    let base = deployment_id("SPY", TREND);
    assert_eq!(base, deployment_id("SPY", TREND), "stable across restarts");
    assert_ne!(base, deployment_id("QQQ", TREND));
    assert_ne!(base, deployment_id("SPY", DUAL));
    let mut other_bps = cf_inputs("SPY");
    other_bps.raw_allocation_fraction_bps = Some("500".to_string());
    let other = resolve_capital_fraction_paper_binding(&other_bps, TREND)
        .unwrap()
        .unwrap()
        .scope
        .deployment_id;
    assert_ne!(base, other, "a contract change is a different deployment");
    assert!(
        resolve_capital_fraction_paper_binding(&fixed_inputs("SPY"), TREND)
            .unwrap()
            .is_none()
    );
}

async fn seed_promotion(
    pool: &PgPool,
    strategy: &str,
    symbol: &str,
    timeframe_secs: i64,
    fingerprint: &str,
    chain: &[(&str, &str)],
) {
    for (i, (prev, new)) in chain.iter().enumerate() {
        mqk_db::insert_strategy_promotion_transition(
            pool,
            &mqk_db::InsertStrategyPromotionTransitionArgs {
                transition_id: Uuid::new_v5(
                    &Uuid::NAMESPACE_URL,
                    format!("cfd-seed:{strategy}:{symbol}:{timeframe_secs}:{i}").as_bytes(),
                ),
                strategy_id: strategy.to_string(),
                symbol: symbol.to_string(),
                timeframe_secs,
                config_fingerprint: Some(fingerprint.to_string()),
                config_identity_status: "verified_v1".to_string(),
                previous_state: Some((*prev).to_string()),
                new_state: (*new).to_string(),
                parent_transition_id: None,
                evidence_transition_id: None,
                evidence_review_id: None,
                evidence_scanner_scan_id: None,
                evidence_git_hash: None,
                evidence_artifact_path: None,
                evidence_fingerprint: None,
                evidence_fingerprint_v2: None,
                effective_at_utc: Utc::now() + chrono::Duration::seconds(i as i64),
                expires_at_utc: None,
                initiated_by: "cfd-test-seed".to_string(),
                reason: "test seed".to_string(),
                created_at_utc: Utc::now() + chrono::Duration::seconds(i as i64),
            },
        )
        .await
        .expect("seed promotion transition");
    }
}

const ACTIVE: &[(&str, &str)] = &[("paper_approved", "active_paper")];

#[tokio::test]
async fn durable_decision_is_promotion_gated_on_exact_strategy_symbol_timeframe_fingerprint() {
    let Some(db) = db_or_skip("CFD-PROMO").await else {
        return;
    };
    let cases = [
        "exact",
        "unpromoted",
        "otherfp",
        "othersymbol",
        "otherstrategy",
        "othertimeframe",
        "demoted",
    ];
    let syms: Vec<String> = cases.iter().map(|c| uniq(&format!("P{c}"))).collect();
    let registry_path = std::env::temp_dir().join(format!("{}_registry.json", uniq("REG")));
    let entries: Vec<serde_json::Value> = syms
        .iter()
        .map(|s| {
            serde_json::json!({
                "instrument_id": format!("equity:US:{s}"), "symbol": s, "asset_class": "equity",
                "provider": "twelvedata", "provider_symbol": s, "venue": "NASDAQ",
                "currency": "USD", "enabled": true, "timeframes": ["1D"], "notes": "cfd test"
            })
        })
        .collect();
    std::fs::write(&registry_path, serde_json::to_vec(&entries).unwrap()).unwrap();

    mqk_db::persist_arm_state(&db, "ARMED", None)
        .await
        .expect("arm");
    let ts = Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        &db,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: TREND.to_string(),
            display_name: "cfd test".to_string(),
            enabled: true,
            kind: String::new(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: String::new(),
        },
    )
    .await
    .expect("registry");
    let mut st =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Paper);
    st.db = Some(db.clone());
    st.instrument_registry_path = registry_path.to_string_lossy().into_owned();
    st.set_per_symbol_bar_staleness_secs_for_test(Some(10_000_000_000));
    let state = Arc::new(st);
    let live_run = Uuid::new_v4();
    mqk_db::insert_run(
        &db,
        &mqk_db::NewRun {
            run_id: live_run,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: ts,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({"source": "capital_fraction_dispatch_tests"}),
            host_fingerprint: "test-host".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::arm_run(&db, live_run).await.expect("arm_run");
    mqk_db::begin_run(&db, live_run).await.expect("begin_run");
    mqk_db::heartbeat_run(&db, live_run, ts)
        .await
        .expect("heartbeat");
    state
        .inject_running_loop_for_test(ExecutionDomain::EquityNyse, live_run)
        .await;

    let dual_fp = resolve_capital_fraction_paper_binding(&cf_inputs(&syms[0]), DUAL)
        .unwrap()
        .unwrap()
        .semantic_fingerprint;
    for (case, sym) in cases.iter().zip(&syms) {
        seed_rising(&db, sym, 60).await;
        let mut p = build(&keys(&[(sym, TREND)]), &db).await.expect("pool");
        let out = tick(&state, &[binding(sym, TREND)], &mut p, 59).await;
        let result = &out
            .as_ref()
            .expect("durable tick")
            .first()
            .expect("result")
            .1;
        let fp = result.semantic_fingerprint.clone();
        assert_eq!(
            fp,
            resolve_capital_fraction_paper_binding(&cf_inputs(sym), TREND)
                .unwrap()
                .unwrap()
                .semantic_fingerprint,
            "{case}: durable result carries the deployment's semantic fingerprint"
        );
        let decisions = crate::decision::bar_result_to_decisions(
            result,
            live_run,
            end_ts(59),
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(decisions.len(), 1, "{case}");
        assert_eq!(
            decisions[0].qty.raw(),
            62 * USD,
            "{case}: held Q reaches the decision"
        );
        assert_eq!(decisions[0].strategy_semantic_fingerprint, fp, "{case}");

        match *case {
            "exact" => seed_promotion(&db, TREND, sym, DAY, &fp, ACTIVE).await,
            "otherfp" => seed_promotion(&db, TREND, sym, DAY, &dual_fp, ACTIVE).await,
            "othersymbol" => seed_promotion(&db, TREND, &format!("{sym}X"), DAY, &fp, ACTIVE).await,
            "otherstrategy" => seed_promotion(&db, DUAL, sym, DAY, &fp, ACTIVE).await,
            "othertimeframe" => seed_promotion(&db, TREND, sym, 3_600, &fp, ACTIVE).await,
            "demoted" => {
                seed_promotion(
                    &db,
                    TREND,
                    sym,
                    DAY,
                    &fp,
                    &[
                        ("paper_approved", "active_paper"),
                        ("active_paper", "demoted"),
                    ],
                )
                .await
            }
            _ => {}
        }
        let did = decisions[0].decision_id.clone();
        let outcome =
            crate::decision::submit_internal_strategy_decision(&state, decisions[0].clone()).await;
        let n: (i64,) =
            sqlx::query_as("select count(*) from oms_outbox where idempotency_key = $1")
                .bind(&did)
                .fetch_one(&db)
                .await
                .unwrap();
        if *case == "exact" {
            assert!(outcome.accepted, "{case}: {outcome:?}");
            assert_eq!(n.0, 1);
        } else {
            assert!(!outcome.accepted, "{case}: must be refused: {outcome:?}");
            assert_eq!(n.0, 0, "{case}: no outbox row without exact promotion");
        }
    }
    let _ = std::fs::remove_file(&registry_path);
    for s in &syms {
        cleanup(&db, &[s]).await;
    }
}

#[tokio::test]
async fn unaffordable_entry_resolves_to_zero_never_one_share_and_holds_nothing() {
    let Some(db) = db_or_skip("CFD-ZERO").await else {
        return;
    };
    let sym = uniq("ZRO");
    // $20,000+ per share against a $10,000 allocation: floor(10,000 / price) = 0.
    seed_rising(&db, &sym, 60).await;
    sqlx::query("update md_bars set open_micros = open_micros * 200, high_micros = high_micros * 200, low_micros = low_micros * 200, close_micros = close_micros * 200 where symbol = $1 and provider_id = 'cfd_test'")
        .bind(&sym)
        .execute(&db)
        .await
        .unwrap();
    let state = state_with_db(&db);
    let sel = keys(&[(&sym, TREND)]);
    let b = vec![binding(&sym, TREND)];
    let mut p = build(&sel, &db).await.expect("pool");
    let out = tick(&state, &b, &mut p, 59).await;
    assert_eq!(
        out.as_ref()
            .map(|v| v[0].1.intents.output.targets[0].qty.raw())
            .ok(),
        Some(0),
        "an unaffordable entry resolves to exactly zero, never a one-share fallback"
    );
    assert!(
        rows(&db, &sym, TREND).await.is_empty(),
        "nothing is held for an unaffordable entry"
    );
    cleanup(&db, &[&sym]).await;
}
