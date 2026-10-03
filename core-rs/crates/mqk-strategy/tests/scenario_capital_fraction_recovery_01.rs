//! Restart recovery of the capital-fraction wrapper (pure, no IO): a wrapper
//! rebuilt from a validated durable snapshot keeps the held `Q` exactly, never
//! re-resolves it from a later bar, and refuses any record that is not proven
//! against the deployment contract.

use std::sync::{Arc, Mutex};

use mqk_execution::{QtyMicros, StrategyOutput, TargetPosition};
use mqk_strategy::{
    capital_fraction_semantic_fingerprint, BarStub, CapitalFractionSizedStrategy, HeldSizingRecord,
    HeldSizingRecoveryError, HeldSizingScope, HeldSizingStatus, HeldSizingTransition,
    PluginRegistry, RecentBarsWindow, RegistryError, RestartRecovery, SizingPolicy, Strategy,
    StrategyContext, StrategyMeta, StrategySpec, TargetSizing, HELD_SIZING_STATE_VERSION,
    SIZING_POLICY_FIXED_INITIAL_CAPITAL_FRACTION_V1,
};

const USD: i64 = 1_000_000;

type RecordMutation = Box<dyn Fn(&mut HeldSizingRecord)>;
const CAPITAL: i64 = 100_000 * USD;
const BPS: i64 = 1_000; // 10% -> $10,000 budget

/// Long exactly when `script[call]` is true.
struct Scripted {
    symbol: &'static str,
    script: Arc<Mutex<Vec<bool>>>,
    calls: usize,
}

impl Strategy for Scripted {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("scripted", 86_400)
    }
    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        let long = self
            .script
            .lock()
            .unwrap()
            .get(self.calls)
            .copied()
            .unwrap_or(false);
        self.calls += 1;
        StrategyOutput::new(vec![TargetPosition::new(
            self.symbol,
            if long {
                QtyMicros::from_whole_units(1).unwrap()
            } else {
                QtyMicros::ZERO
            },
        )])
    }
}

fn scope() -> HeldSizingScope {
    HeldSizingScope {
        deployment_id: "dep-1".into(),
        strategy_id: "scripted".into(),
    }
}

fn caps() -> TargetSizing {
    TargetSizing::equity_default()
}

fn ctx(end_ts: i64, close: i64) -> StrategyContext {
    StrategyContext::new(
        86_400,
        0,
        RecentBarsWindow::new(1, vec![BarStub::new(end_ts, true, close * USD, 1)]),
    )
}

fn build(
    symbol: &'static str,
    script: Vec<bool>,
    snapshot: Vec<HeldSizingRecord>,
) -> Result<
    (
        CapitalFractionSizedStrategy,
        mqk_strategy::SizingAuditHandle,
        mqk_strategy::SizingStateHandle,
    ),
    HeldSizingRecoveryError,
> {
    CapitalFractionSizedStrategy::new_recoverable(
        Box::new(Scripted {
            symbol,
            script: Arc::new(Mutex::new(script)),
            calls: 0,
        }),
        SizingPolicy::capital_fraction_v1(BPS).unwrap(),
        CAPITAL,
        caps(),
        scope(),
        snapshot,
    )
}

fn qty(out: &StrategyOutput) -> i64 {
    out.targets[0].qty.raw()
}

/// A record the resolver would actually produce: $10,000 / $125 = 80 shares.
fn active(symbol: &str) -> HeldSizingRecord {
    HeldSizingRecord {
        deployment_id: "dep-1".into(),
        strategy_id: "scripted".into(),
        symbol: symbol.into(),
        state_version: HELD_SIZING_STATE_VERSION,
        entry_generation: 1,
        status: HeldSizingStatus::Active,
        policy_id: SIZING_POLICY_FIXED_INITIAL_CAPITAL_FRACTION_V1.into(),
        allocation_fraction_bps: BPS,
        initial_allocated_capital_micros: CAPITAL,
        max_target_qty_micros: None,
        max_notional_usd: None,
        resolved_target_qty_micros: 80 * USD,
        reference_bar_end_ts: 10,
        reference_price_micros: 125 * USD,
    }
}

#[test]
fn restored_active_entry_is_held_exactly_and_never_re_resolved_from_a_later_price() {
    let (mut w, audit, state) = build("SPY", vec![true, true], vec![active("SPY")]).unwrap();
    // Later close is $400: re-resolving would give 25 shares.
    let out = w.on_bar(&ctx(20, 400));
    assert_eq!(qty(&out), 80 * USD, "exactly the original Q");
    assert!(
        audit.snapshot().entries.is_empty(),
        "no new resolution happened"
    );
    assert!(
        state.drain_transitions().is_empty(),
        "no duplicate entry transition"
    );
    assert_eq!(
        qty(&w.on_bar(&ctx(30, 1))),
        80 * USD,
        "and never a one-share fallback"
    );
}

#[test]
fn restart_while_flat_resolves_a_fresh_entry_from_the_new_causal_close_and_initial_capital() {
    let (mut w, _a, state) = build("SPY", vec![true], vec![]).unwrap();
    let out = w.on_bar(&ctx(20, 400));
    assert_eq!(qty(&out), 25 * USD, "10_000 / 400 from INITIAL capital");
    let t = state.drain_transitions();
    assert_eq!(t.len(), 1);
    let HeldSizingTransition::Entered(r) = &t[0] else {
        panic!("entry expected")
    };
    assert_eq!(
        (
            r.entry_generation,
            r.resolved_target_qty_micros,
            r.reference_price_micros,
            r.reference_bar_end_ts
        ),
        (1, 25 * USD, 400 * USD, 20)
    );
    assert_eq!(
        (
            r.deployment_id.as_str(),
            r.strategy_id.as_str(),
            r.symbol.as_str()
        ),
        ("dep-1", "scripted", "SPY")
    );
}

#[test]
fn exit_releases_and_a_later_entry_is_the_next_generation_with_a_new_q() {
    let (mut w, audit, state) = build("SPY", vec![true, false, true], vec![active("SPY")]).unwrap();
    assert_eq!(qty(&w.on_bar(&ctx(20, 400))), 80 * USD);
    assert!(state.drain_transitions().is_empty());

    assert_eq!(qty(&w.on_bar(&ctx(30, 400))), 0, "inner flat => flat");
    let t = state.drain_transitions();
    assert_eq!(t.len(), 1);
    let HeldSizingTransition::Released(r) = &t[0] else {
        panic!("release expected")
    };
    assert_eq!(
        (r.status, r.entry_generation, r.resolved_target_qty_micros),
        (HeldSizingStatus::Released, 1, 80 * USD)
    );

    assert_eq!(
        qty(&w.on_bar(&ctx(40, 250))),
        40 * USD,
        "10_000 / 250, same initial capital"
    );
    let t = state.drain_transitions();
    let HeldSizingTransition::Entered(r) = &t[0] else {
        panic!("entry expected")
    };
    assert_eq!(
        (r.entry_generation, r.status),
        (2, HeldSizingStatus::Active)
    );
    assert_eq!(audit.snapshot().entries.len(), 1);
}

#[test]
fn released_snapshot_restart_stays_flat_then_enters_at_the_next_generation() {
    let mut released = active("SPY");
    released.status = HeldSizingStatus::Released;
    let (mut w, _a, state) = build("SPY", vec![false, true], vec![released]).unwrap();
    assert_eq!(qty(&w.on_bar(&ctx(20, 400))), 0, "flat stays flat");
    assert!(state.drain_transitions().is_empty());
    assert_eq!(qty(&w.on_bar(&ctx(30, 400))), 25 * USD);
    let HeldSizingTransition::Entered(r) = &state.drain_transitions()[0] else {
        panic!()
    };
    assert_eq!(
        r.entry_generation, 2,
        "generation floor survives the release"
    );
}

#[test]
fn held_state_is_per_symbol_and_does_not_contaminate_other_symbols() {
    // A record for QQQ must not hold a quantity for SPY.
    let (mut w, audit, _s) = build("SPY", vec![true], vec![active("QQQ")]).unwrap();
    assert_eq!(
        qty(&w.on_bar(&ctx(20, 400))),
        25 * USD,
        "SPY resolves its own entry"
    );
    assert_eq!(audit.snapshot().entries.len(), 1);
}

#[test]
fn recovery_refuses_every_foreign_stale_malformed_or_mismatched_record() {
    let cases: Vec<(&str, RecordMutation)> = vec![
        (
            "wrong deployment id",
            Box::new(|r| r.deployment_id = "dep-2".into()),
        ),
        (
            "wrong strategy id",
            Box::new(|r| r.strategy_id = "other".into()),
        ),
        ("blank symbol", Box::new(|r| r.symbol = " ".into())),
        ("padded symbol", Box::new(|r| r.symbol = " SPY".into())),
        (
            "wrong policy",
            Box::new(|r| r.policy_id = "fixed_quantity_v1".into()),
        ),
        (
            "wrong fraction",
            Box::new(|r| r.allocation_fraction_bps = 2_000),
        ),
        (
            "wrong capital",
            Box::new(|r| r.initial_allocated_capital_micros = 50_000 * USD),
        ),
        (
            "record cap not in contract",
            Box::new(|r| r.max_target_qty_micros = Some(10 * USD)),
        ),
        (
            "record notional cap not in contract",
            Box::new(|r| r.max_notional_usd = Some(5_000)),
        ),
        (
            "tampered quantity",
            Box::new(|r| r.resolved_target_qty_micros = 81 * USD),
        ),
        (
            "one-share fallback quantity",
            Box::new(|r| r.resolved_target_qty_micros = USD),
        ),
        (
            "zero quantity",
            Box::new(|r| r.resolved_target_qty_micros = 0),
        ),
        (
            "fractional quantity",
            Box::new(|r| r.resolved_target_qty_micros = 80 * USD + 1),
        ),
        (
            "zero price (partial state)",
            Box::new(|r| r.reference_price_micros = 0),
        ),
        (
            "zero bar ts (partial state)",
            Box::new(|r| r.reference_bar_end_ts = 0),
        ),
        ("generation zero", Box::new(|r| r.entry_generation = 0)),
        ("future state version", Box::new(|r| r.state_version = 2)),
    ];
    for (name, mutate) in cases {
        let mut rec = active("SPY");
        mutate(&mut rec);
        assert!(
            build("SPY", vec![], vec![rec]).is_err(),
            "{name} must fail closed"
        );
    }
    // Duplicate symbol rows are ambiguous.
    assert!(matches!(
        build("SPY", vec![], vec![active("SPY"), active("SPY")]),
        Err(HeldSizingRecoveryError::DuplicateSymbol { .. })
    ));
    // The valid record is the control: it is accepted.
    assert!(build("SPY", vec![], vec![active("SPY")]).is_ok());
}

#[test]
fn recovery_refuses_a_blank_scope_and_a_non_capital_fraction_policy() {
    let blank = |d: &str, s: &str| {
        CapitalFractionSizedStrategy::new_recoverable(
            Box::new(Scripted {
                symbol: "SPY",
                script: Arc::default(),
                calls: 0,
            }),
            SizingPolicy::capital_fraction_v1(BPS).unwrap(),
            CAPITAL,
            caps(),
            HeldSizingScope {
                deployment_id: d.into(),
                strategy_id: s.into(),
            },
            vec![],
        )
        .is_err()
    };
    assert!(blank("", "scripted") && blank("dep-1", " "));
    assert!(CapitalFractionSizedStrategy::new_recoverable(
        Box::new(Scripted {
            symbol: "SPY",
            script: Arc::default(),
            calls: 0
        }),
        SizingPolicy::FixedQuantityV1,
        CAPITAL,
        caps(),
        scope(),
        vec![],
    )
    .is_err());
}

#[test]
fn recoverable_and_plain_wrappers_have_one_semantic_identity() {
    let inner = Scripted {
        symbol: "SPY",
        script: Arc::default(),
        calls: 0,
    };
    let inner_fp = inner.semantic_fingerprint();
    let (plain, _) = CapitalFractionSizedStrategy::new(
        Box::new(Scripted {
            symbol: "SPY",
            script: Arc::default(),
            calls: 0,
        }),
        SizingPolicy::capital_fraction_v1(BPS).unwrap(),
        CAPITAL,
        caps(),
    )
    .unwrap();
    let (recoverable, _, _) = build("SPY", vec![], vec![]).unwrap();
    let pure = capital_fraction_semantic_fingerprint(&inner_fp, BPS, CAPITAL, &caps());
    assert_eq!(plain.semantic_fingerprint(), pure);
    assert_eq!(
        recoverable.semantic_fingerprint(),
        pure,
        "scope is not part of identity"
    );
    assert_ne!(pure, inner_fp);
}

#[test]
fn plain_wrapper_records_no_durable_transitions() {
    // Backtest/Research behavior is unchanged: no state handle exists.
    let (mut plain, _) = CapitalFractionSizedStrategy::new(
        Box::new(Scripted {
            symbol: "SPY",
            script: Arc::new(Mutex::new(vec![true, false])),
            calls: 0,
        }),
        SizingPolicy::capital_fraction_v1(BPS).unwrap(),
        CAPITAL,
        caps(),
    )
    .unwrap();
    assert_eq!(qty(&plain.on_bar(&ctx(20, 400))), 25 * USD);
    assert_eq!(qty(&plain.on_bar(&ctx(30, 400))), 0);
}

#[test]
fn registry_stateless_seam_refuses_durable_state_entries_but_identity_seam_admits_them() {
    let mut reg = PluginRegistry::new();
    let meta = |n: &str| StrategyMeta::new(n, "1", 86_400, "t");
    for (name, recovery) in [
        ("bounded", RestartRecovery::BoundedHistoryReconstructible),
        ("durable", RestartRecovery::DurableStateRequired),
        ("unrecoverable", RestartRecovery::NotRecoverable),
    ] {
        reg.register(meta(name).with_restart_recovery(recovery), || {
            Box::new(Scripted {
                symbol: "SPY",
                script: Arc::default(),
                calls: 0,
            })
        })
        .unwrap();
    }
    assert!(reg.instantiate_verified("bounded").is_ok());
    assert!(matches!(
        reg.instantiate_verified("durable"),
        Err(RegistryError::DurableStateRequired { .. })
    ));
    assert!(matches!(
        reg.instantiate_verified("unrecoverable"),
        Err(RegistryError::NotRestartRecoverable { .. })
    ));
    let (_, kind) = reg.instantiate_for_identity("durable").unwrap();
    assert_eq!(kind, RestartRecovery::DurableStateRequired);
    assert!(reg.instantiate_for_identity("bounded").is_ok());
    assert!(matches!(
        reg.instantiate_for_identity("unrecoverable"),
        Err(RegistryError::NotRestartRecoverable { .. })
    ));
    // Marking converts only the bounded entry; the unrecoverable one stays refused.
    let marked = {
        let mut r = PluginRegistry::new();
        for (name, recovery) in [
            ("bounded", RestartRecovery::BoundedHistoryReconstructible),
            ("unrecoverable", RestartRecovery::NotRecoverable),
        ] {
            r.register(meta(name).with_restart_recovery(recovery), || {
                Box::new(Scripted {
                    symbol: "SPY",
                    script: Arc::default(),
                    calls: 0,
                })
            })
            .unwrap();
        }
        r.with_durable_state_required()
    };
    assert!(matches!(
        marked.instantiate_verified("bounded"),
        Err(RegistryError::DurableStateRequired { .. })
    ));
    assert!(matches!(
        marked.instantiate_verified("unrecoverable"),
        Err(RegistryError::NotRestartRecoverable { .. })
    ));
}
