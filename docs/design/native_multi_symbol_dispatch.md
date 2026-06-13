# Native Multi-Symbol Dispatch — Design

**Status:** Design-only / audit-only. No production code, schema, or test changes are made by this
patch. This document records the current single-symbol architecture, the target multi-symbol
architecture, and an ordered, dependency-checked sequence of follow-on patches. Each follow-on
patch remains subject to the one-patch-per-turn rule and its own scenario-test proof.

**Patch ID:** NATIVE-MULTI-SYMBOL-DISPATCH-DESIGN-01

## 0. Scope and Method

### In scope
- Documenting how the current daemon/runtime dispatches a single symbol end-to-end (bar ingestion
  → strategy → decision → outbox → broker → portfolio).
- Documenting the current watchlist/promotion artifact contract and where its `max_symbols=1`
  assumptions live.
- Documenting current risk/gating surfaces and confirming which are already account-wide
  (and therefore symbol-count-independent) vs. which would need new per-symbol variants.
- Proposing new types/components, config schema additions, API/evidence surfaces, GUI surfaces,
  a multi-symbol paper-smoke runner, and an ordered implementation patch sequence.

### Out of scope (explicit non-goals)
- Any change to `approved_for_live` semantics. It remains hard-locked `false` everywhere it
  appears today, and every new artifact/response type introduced by this design preserves that
  lock.
- Heterogeneous strategies-per-symbol (multiple distinct `Strategy` implementations active in one
  run). Tier A's `StrategyHost::register()` single-registration constraint
  (`MultiStrategyNotAllowed`) is preserved. This design supports **one strategy assigned to
  multiple symbols**, not multiple strategies.
- Per-symbol reconcile-drift halting, per-symbol deadman timers, or per-symbol kill-switches.
  Account-wide halt/kill-switch/reconcile/deadman semantics are preserved unchanged and are
  called out explicitly where relevant (Phase 5, Q6–Q8 and Phase 6, caps #7/#10/#11).
- Limit-order support. B1C decisions remain `order_type="market"` always; one new cap
  (per-symbol notional, Phase 6 cap #3) is therefore documented as currently unverifiable in
  practice and that gap is stated honestly rather than hidden.

### Research method note
Two of four research agents spawned for this design hit a session limit and returned no output
(the watchlist/promotion-artifact agent and the GUI/API-surface agent). Their scope was covered
by direct `Read`/`Grep` research in this session instead — full-file reads of
`watchlist_intake.rs`, `routes/watchlist.rs`, `watchlist_promotion.py`,
`truthRendering.ts`, and `decision.rs`, plus targeted greps across `api_types.rs` and
`loop_runner.rs`. All facts below reflect that direct research against the repo as of this
session; no content was carried over from the failed agents.

Two identifiers referenced informally before this design phase do not exist verbatim in the repo
(confirmed via grep, zero matches in both cases):

- **`GUI-EXECUTION-SCREEN-RENDER-GUARD-01`** — there is no patch or symbol with this exact name.
  The behavior it informally refers to is the existing
  `if (truthState !== null) return <TruthStateNotice state={truthState} />` hard-block at the top
  of every live-data screen (driven by `panelTruthRenderState()` /
  `isTruthHardBlock()` in `truthRendering.ts`). This design treats that existing pattern as the
  GUI contract any multi-symbol screen extension must sit *behind*, not *replace*.
- **`scripts/guards/run_all_script_guards.ps1`** — no such aggregator script exists. The guard
  scripts that exist are: `check_unsafe_patterns.ps1` / `.sh`, `check_migration_governance.sh`,
  `check_ignored_load_bearing_proofs.sh`, `check_workspace_dep_inheritance.sh`. Phase 11
  (validation for this patch) runs the guards that exist; it does not invoke a nonexistent
  aggregator.

---

## 1. Current Single-Symbol Architecture (Baseline)

### 1.1 Bar input and dispatch entry point

`StrategyBarInput` (`core-rs/crates/mqk-daemon/src/state.rs:425`):

```rust
pub struct StrategyBarInput {
    pub now_tick: u64,
    pub end_ts: i64,
    pub limit_price: Option<i64>,
    pub qty: i64,
}
```

No `symbol` field — the type is implicitly single-symbol. It is stored in a single-slot mutex,
`pending_strategy_bar_input: Arc<Mutex<Option<StrategyBarInput>>>`, and consumed via `.take()`
(at most one pending bar input at a time, for the one configured symbol).

`RecentBarsWindow` (`core-rs/crates/mqk-strategy/src/types.rs:82`):

```rust
pub struct RecentBarsWindow {
    pub max_len: usize,
    pub bars: Vec<BarStub>,
}
```

`fetch_recent_completed_bars_for_strategy` (`core-rs/crates/mqk-db/src/md.rs:1338`):

```rust
pub async fn fetch_recent_completed_bars_for_strategy(
    pool: &PgPool,
    symbol: &str,
    timeframe: &str,
    limit: i64,
) -> Result<Vec<MdBarRow>>
```

This already takes `symbol: &str` — `SELECT ... FROM md_bars WHERE symbol=$1 AND timeframe=$2 AND
is_complete=true ORDER BY end_ts DESC LIMIT $3`, returned oldest-first. **The DB read path is
already symbol-parameterized; nothing in `mqk-db` needs to change for multi-symbol.**

`tick_strategy_dispatch()` (`core-rs/crates/mqk-daemon/src/state.rs:1525`, called from
`loop_runner.rs:591`):

- Returns `Option<mqk_strategy::StrategyBarResult>`.
- Takes the pending bar input from the single-slot mutex via `.take()`.
- Reads `MQK_STRATEGY_SYMBOL` (trimmed) and `MQK_STRATEGY_MD_TIMEFRAME` (constant
  `STRATEGY_MD_TIMEFRAME_ENV`, defined at `state.rs:114`).
- Uses `STRATEGY_CONTEXT_LOAD_LIMIT` as the DB fetch limit.
- If both symbol and timeframe are non-empty and `self.db` is present, fetches bars via
  `fetch_recent_completed_bars_for_strategy`. On success with non-empty rows, builds
  `RecentBarsWindow::new(bars_loaded.max(1), stubs)` and calls
  `invoke_native_strategy_on_bar_from_window`.
- On empty rows or DB error, falls back to a stub invocation with a default window (existing,
  intentional degraded-but-not-halted path).

### 1.2 Strategy types

`StrategyBarResult` (`core-rs/crates/mqk-strategy/src/types.rs:178`):

```rust
pub struct StrategyBarResult {
    pub spec: StrategySpec,
    pub intents: StrategyIntents,
}
```

`StrategySpec` (`core-rs/crates/mqk-strategy/src/types.rs:5`):

```rust
pub struct StrategySpec {
    pub name: String,
    pub timeframe_secs: i64, // Tier A: exactly one timeframe for the strategy
}
```

`StrategyOutput` / `TargetPosition` (`core-rs/crates/mqk-execution/src/types.rs:8-17`):

```rust
pub struct StrategyOutput {
    pub targets: Vec<TargetPosition>,
}

pub struct TargetPosition {
    pub symbol: String,
    pub qty: i64, // signed target portfolio state, +long / -short
}
```

**`StrategyOutput.targets` is already a `Vec<TargetPosition>` keyed by `symbol` — the strategy
output type is structurally multi-symbol-ready today.** The bottleneck is entirely upstream
(bar input is single-slot/single-symbol) and in fleet/config wiring (below), not in this type.

`ShadowMode` / `IntentMode` (`core-rs/crates/mqk-strategy/src/types.rs:151`):

```rust
pub enum ShadowMode { Off, On }
pub enum IntentMode { Live, Shadow }

pub struct StrategyIntents {
    pub mode: IntentMode,
    pub output: StrategyOutput,
}

impl StrategyIntents {
    pub fn should_execute(&self) -> bool { self.mode == IntentMode::Live }
}
```

Mode mapping (`state.rs:72`): `ShadowMode::Off => IntentMode::Live`,
`ShadowMode::On => IntentMode::Shadow`. Currently hardcoded
`StrategyHost::new(ShadowMode::Off)` (`native_strategy.rs:121`) — under the B1C policy, dispatch
is always `Live`.

### 1.3 Strategy fleet / single-strategy selection

`StrategyFleetEntry` (`core-rs/crates/mqk-daemon/src/state/broker.rs:197`):

```rust
pub struct StrategyFleetEntry {
    pub strategy_id: String,
}
```

Derived from `MQK_STRATEGY_IDS` (`state.rs:792`):

```rust
let strategy_fleet = std::env::var("MQK_STRATEGY_IDS").ok().map(|ids| {
    ids.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|id| StrategyFleetEntry { strategy_id: id.to_string() })
        .collect::<Vec<_>>()
});
```

Only `ids[0]` is ever instantiated (`native_strategy.rs:115`):

```rust
// Single-strategy Tier A policy: consume only the first fleet entry.
let strategy_id = ids[0].clone();
```

`build_daemon_plugin_registry()` (`core-rs/crates/mqk-runtime/src/native_strategy.rs:302`):

```rust
pub fn build_daemon_plugin_registry() -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    let symbol = std::env::var("MQK_STRATEGY_SYMBOL").unwrap_or_default();
    mqk_strategy::engines::register_builtin_strategies(&mut registry, symbol)
        .expect("daemon built-in strategy registration must not fail: duplicate names are a programming error");
    registry
}
```

`PluginRegistry` (`mqk-strategy/src/plugin_registry.rs:177`) exposes `register`, `contains`,
`len`, `list`, `lookup`, `instantiate_verified`.

`StrategyHost` (`mqk-strategy/src/host.rs:10`) has fields `strategy: Option<Box<dyn Strategy>>`,
`spec: Option<StrategySpec>`, `shadow: ShadowMode`; `register()` errors
`MultiStrategyNotAllowed` if a strategy is already registered (Tier A single-registration
constraint — preserved by this design, see §0 non-goals).

### 1.4 Env var usage map (single-symbol coupling points)

| Env var | Read sites |
|---|---|
| `MQK_STRATEGY_SYMBOL` | `state.rs:1529` (`tick_strategy_dispatch`), `native_strategy.rs:305` (`build_daemon_plugin_registry`), `routes/autonomous_paper_status.rs:210`, `routes/system.rs:443,864,897,938,968`, `state/lifecycle.rs:605` |
| `MQK_STRATEGY_MD_TIMEFRAME` (const `STRATEGY_MD_TIMEFRAME_ENV`) | `state.rs:114` (def) + `state.rs:1532` (read), `routes/system.rs:446,867,900,941,971`, `state/lifecycle.rs:608` |
| `MQK_STRATEGY_IDS` | `state.rs:792` (fleet derivation), `native_strategy.rs:115` (`ids[0]` only), `routes/system.rs:965` |

Every one of these is a **single scalar value**, read in multiple places, each implicitly
assuming "the one symbol this run trades."

### 1.5 Per-tick dispatch loop (`loop_runner.rs`)

`loop_runner.rs` lines 495–734 contain two consecutive blocks, both already iterating
`positions_to_check`/`targets` collections — i.e. the *shape* of a per-symbol loop already
exists for flatten and for decision construction, just not for *bar dispatch*.

**Pre-event flatten block** (`EVENT-RISK-FLATTEN-WIRE-01`, lines 503–572) — already a per-symbol
loop template:

```rust
if let Some(ref pool) = db {
    let positions_to_check: Vec<(String, i64)> = {
        let snap = snapshot_cache.read().await;
        snap.as_ref()
            .map(|s| {
                s.portfolio.positions.iter()
                    .filter(|p| p.net_qty != 0)
                    .map(|p| (p.symbol.clone(), p.net_qty))
                    .collect()
            })
            .unwrap_or_default()
    };
    for (symbol, net_qty) in &positions_to_check {
        let ts_secs = Utc::now().timestamp();
        let outcome = crate::pre_event_flatten::evaluate_flatten_trigger_from_env(
            symbol, ts_secs, crate::pre_event_flatten::DEFAULT_FLATTEN_LEAD_SECS,
        );
        if outcome.is_flatten_required() || outcome.is_unavailable() {
            let (key, order_json) = crate::pre_event_flatten::build_flatten_close_order_json(
                symbol, *net_qty, ts_secs, run_id,
            );
            match mqk_db::outbox_enqueue(pool, run_id, &key, order_json).await {
                Ok(true) => { /* pre_event_flatten_close_enqueued */ }
                Ok(false) => { /* pre_event_flatten_close_already_pending */ }
                Err(err) => { /* pre_event_flatten_close_enqueue_failed */ }
            }
        }
    }
}
```

**B1C dispatch block** (lines 574–732) — currently single-symbol at the bar-fetch step, but
already multi-target at the decision step:

```rust
if let Some(bar_result) = state_arc.tick_strategy_dispatch().await {
    let raw_signal_qty: i64 = bar_result.intents.output.targets.iter().map(|t| t.qty).sum();
    state_arc.record_bar_tick_outcome(raw_signal_qty);

    let now_micros = Utc::now().timestamp_micros();
    let current_positions: Option<BTreeMap<String, i64>> = {
        let snap = snapshot_cache.read().await;
        snap.as_ref().map(|s| {
            s.portfolio.positions.iter().map(|p| (p.symbol.clone(), p.net_qty)).collect()
        })
    };
    let Some(current_positions) = current_positions else {
        tracing::warn!(run_id = %run_id, "b1c_skip_no_snapshot: ...");
        continue;
    };

    for t in &bar_result.intents.output.targets {
        let current = current_positions.get(&t.symbol).copied().unwrap_or(0);
        let delta = t.qty - current;
        let no_order_reason = if delta == 0 {
            "already_at_target"
        } else if delta < 0 && (current <= 0 || (-delta) > current) {
            "b5_short_sale_guard"
        } else {
            "order_will_be_submitted"
        };
        tracing::info!(..., "b1c_position_delta_diagnostic");
        if no_order_reason == "b5_short_sale_guard" && state_arc.try_claim_b5_alert(&t.symbol).await {
            // Discord alert via notify_trade_event, stage "signal.blocked"
        }
    }

    let decisions = crate::decision::bar_result_to_decisions(&bar_result, run_id, now_micros, &current_positions);
    if decisions.is_empty() {
        tracing::info!(..., "b1c_bar_tick_no_decisions");
    }
    for decision in decisions {
        let outcome = crate::decision::submit_internal_strategy_decision(&state_arc, decision).await;
        if outcome.accepted {
            tracing::info!(..., "b1c_native_decision_accepted");
        } else {
            tracing::warn!(..., "b1c_native_decision_not_accepted");
        }
    }
}
```

The `for t in &bar_result.intents.output.targets` loop and `bar_result_to_decisions` are **already
multi-target** — they only ever see one target today because `tick_strategy_dispatch()` only ever
produces one symbol's bar result per tick.

### 1.6 Decision pipeline (`decision.rs`, full file read — 643 lines)

```rust
#[derive(Debug, Clone)]
pub struct InternalStrategyDecision {
    pub decision_id: String,    // UUIDv5, idempotency key
    pub strategy_id: String,
    pub symbol: String,         // already symbol-aware
    pub side: String,            // "buy" | "sell"
    pub qty: i64,
    pub order_type: String,      // "market" | "limit"
    pub time_in_force: String,   // "day" | "gtc" | "ioc" | "fok"
    pub limit_price: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct InternalDecisionOutcome {
    pub accepted: bool,
    pub disposition: String, // "accepted"|"duplicate"|"rejected"|"unavailable"|"suppressed"
                              // |"day_limit_reached"|"budget_denied"|"policy_invalid"
    pub decision_id: String,
    pub strategy_id: String,
    pub active_run_id: Option<Uuid>,
    pub blockers: Vec<String>,
}
```

`bar_result_to_decisions(result, run_id, now_micros, current_positions: &BTreeMap<String,i64>) ->
Vec<InternalStrategyDecision>`:

- Returns empty if `!result.intents.should_execute()` (shadow mode).
- `strategy_id = result.spec.name.clone()`.
- Iterates `result.intents.output.targets` (already multi-symbol capable structurally).
- `delta = t.qty - current_positions.get(&t.symbol).copied().unwrap_or(0)`.
- `delta == 0` → skip (`already_at_target`).
- `delta > 0` → `side="buy"`, `qty=delta`.
- `delta < 0` → B5 short-sale guard: if `current <= 0 || qty_to_sell > current` → drop (return
  `None`); else `side="sell"`, `qty=qty_to_sell`.
- `decision_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, format!("{run_id}:{strategy_id}:{symbol}:{side}:{qty}:{now_micros}"))`
  — **`symbol` is already part of the UUIDv5 namespace string**, so two symbols producing
  decisions in the same tick at the same `now_micros` produce different `decision_id`s. No
  collision risk for multi-symbol (verified by full-file read this session).
- `order_type="market"`, `time_in_force="day"`, `limit_price=None` always (B1C policy).

`submit_internal_strategy_decision(state, decision) -> InternalDecisionOutcome` — gate sequence:

| Gate | Check | Failure disposition | Scope |
|---|---|---|---|
| 0 | field validation (non-empty ids/symbol, side∈{buy,sell}, `0<qty<=i32::MAX`, order_type∈{market,limit}, time_in_force∈{day,gtc,ioc,fok}, limit_price required iff limit) | `rejected` | per-decision |
| 1 | `state.day_signal_limit_exceeded()` (PT-AUTO-02) | `day_limit_reached` | **account-wide** |
| 1e | `evaluate_strategy_budget_from_env(&sid)` (B6/TV-04B, `capital_policy`) | `budget_denied` / `policy_invalid` (fires Discord alert via `notify_trade_event`, stage `"signal.blocked"`) | per-strategy |
| 2 | `state.db.as_ref()` is `Some` | `unavailable` | account-wide |
| 3 | `mqk_db::fetch_strategy_registry_entry(db,&sid)` is `Some(record) if record.enabled` | `rejected` | per-strategy |
| 4 | `mqk_db::fetch_active_suppression_for_strategy(db,&sid)` is `Ok(None)` | `suppressed` | per-strategy |
| 5 | `mqk_db::load_arm_state(db)` is `("ARMED", _)` | `rejected` (reason from `durable_arm_reason`) | account-wide |
| 6 | `state.current_status_snapshot()` has `active_run_id.is_some()` and `status.state=="running"` | `unavailable` | account-wide |
| 7 | `mqk_db::outbox_enqueue(db, active_run_id, &did, order_json)` | `Ok(true)`→`accepted` (increments `day_signal_count`), `Ok(false)`→`duplicate`, `Err`→`unavailable` | per-decision |

`build_order_json`:

```rust
serde_json::json!({
    "symbol": d.symbol.trim(),
    "side": d.side.trim().to_ascii_lowercase(),
    "qty": d.qty,
    "order_type": d.order_type.trim().to_ascii_lowercase(),
    "time_in_force": d.time_in_force.trim().to_ascii_lowercase(),
    "limit_price": d.limit_price,
    "strategy_id": d.strategy_id.trim(),
    "signal_source": "internal_strategy_decision",
})
```

### 1.7 Per-symbol dedup primitives (already exist)

- `try_claim_b5_alert(&self, symbol: &str) -> bool` —
  `core-rs/crates/mqk-daemon/src/state/signal_intake.rs:62` — **already per-symbol-keyed**.
- `try_claim_day_limit_alert(&self) -> bool` — `signal_intake.rs:72-73` — account-wide
  (`day_limit_alert_fired: AtomicBool`).
- `try_claim_gap_escalation(&self) -> bool` — `state.rs:1300-1304` — account-wide
  (`gap_escalation_pending: AtomicBool`, atomic swap).

---

## 2. Watchlist / Promotion Artifact Contract (Current State)

### 2.1 `watchlist_intake.rs` (full file read, 494 lines — `PAPER-HANDOFF-READONLY-01`)

- `ENV_PAPER_WATCHLIST_PATH = "MQK_PAPER_WATCHLIST_PATH"`.
- `WATCHLIST_SCHEMA_VERSION = "watchlist-v1"` (const).
- `REQUIRED_MAX_SYMBOLS: u64 = 1`, `REQUIRED_MAX_CONCURRENT: u64 = 1` — **hard constraints baked
  into v1 validation**.
- `LoadedWatchlistArtifact { symbols: Vec<String>, top_symbol: Option<String>,
  strategy_assignments: HashMap<String,String>, max_symbols_to_trade: u64,
  max_concurrent_positions: u64, approved_for_autonomous_paper: bool }`.
- `WatchlistIntakeOutcome` enum: `NotConfigured | Missing{configured_path} | Invalid{failure_reasons}
  | LoadedNotApproved{artifact} | LoadedApproved{artifact}`.
- `approved_for_live()` always returns `false` (hard invariant, type-level).
- `evaluate_watchlist_intake(path) -> WatchlistIntakeOutcome` — pure. Validates: `schema_version
  == "watchlist-v1"`, `mode == "paper"`, `approved_for_live != true` (hard live lock),
  `approved_for_autonomous_paper` is bool, `symbols` is array, `strategy_assignments` is object,
  `max_symbols_to_trade == 1`, `max_concurrent_positions == 1`, and (if approved) `symbols`
  non-empty. Accumulates **all** failures before returning `Invalid`.
- `evaluate_watchlist_intake_from_env()` reads `MQK_PAPER_WATCHLIST_PATH`.
- `WatchlistAdmissionReason` enum: `WatchlistNotConfigured | WatchlistMissing | WatchlistInvalid |
  WatchlistNotApproved | SymbolNotApproved | StrategyNotAssigned | Allowed`.
- `WatchlistSignalAdmission { allowed: bool, reason: WatchlistAdmissionReason }`.
- `evaluate_watchlist_signal_admission(outcome, symbol, strategy_id) ->
  WatchlistSignalAdmission` — pure, **not wired** into the live signal path (explicit seam for
  `PAPER-HANDOFF-ENFORCE-01`). Checks `symbol ∈ artifact.symbols` AND
  `artifact.strategy_assignments.get(symbol) == Some(strategy_id)`.

### 2.2 `routes/watchlist.rs` (full file read, 198 lines)

- `GET /api/v1/watchlist/status` → `build_watchlist_status_response(outcome, configured_path,
  checked_at_utc) -> WatchlistStatusResponse`. Sets `approved_for_live: false` unconditionally.
- `GET /api/v1/watchlist/admission-check?symbol=X&strategy_id=Y` →
  `build_watchlist_admission_check_response(...) -> WatchlistAdmissionCheckResponse`. `note` is
  always `"dry_run_only_not_enforced"`. Not wired into `POST /api/v1/strategy/signal`.

### 2.3 `watchlist_promotion.py` (full file read, 308 lines — `WATCHLIST-PROMO-01`)

- `SCHEMA_VERSION_WATCHLIST = "watchlist-v1"`.
- 9 promotion gates, evaluated in order, accumulating failures:
  1. `watchlist_schema_valid` — `schema_version == "watchlist-v1"`
  2. `watchlist_mode_paper` — `mode == "paper"`
  3. `watchlist_live_locked` — `approved_for_live != True` in input
  4. `has_ranked_candidates` — at least one symbol present
  5. `strategy_fit_present` — strategy-fit artifact exists for the top symbol; symbol/strategy_id
     match the watchlist's top-symbol assignment
  6. `strategy_fit_recommended` — `recommended_for_paper == True`
  7. `risk_simulation_passed` — `config.risk_simulation_passed == True` (or derived from
     `risk_simulation_result["passed"]`)
  8. `operator_review_approved` — `config.operator_review_approved == True`
  9. `premarket_revalidation` — `config.premarket_revalidation_required == False` (or derived
     from `premarket_revalidation_result["passed"]`)
- Failure-reason constants: `REASON_WATCHLIST_SCHEMA_INVALID`, `REASON_WATCHLIST_MODE_NOT_PAPER`,
  `REASON_WATCHLIST_LIVE_NOT_LOCKED`, `REASON_NO_RANKED_CANDIDATES`,
  `REASON_STRATEGY_FIT_MISSING`, `REASON_STRATEGY_FIT_SYMBOL_MISMATCH`,
  `REASON_STRATEGY_FIT_STRATEGY_MISMATCH`, `REASON_STRATEGY_FIT_NOT_RECOMMENDED_FOR_PAPER`,
  `REASON_RISK_SIMULATION_REQUIRED`, `REASON_OPERATOR_REVIEW_REQUIRED`,
  `REASON_PREMARKET_REVALIDATION_REQUIRED`, `REASON_LIVE_APPROVAL_FORBIDDEN`.
- `WatchlistPromotionConfig`: `operator_review_approved: bool=False`,
  `risk_simulation_passed: bool=False`, `premarket_revalidation_required: bool=True`,
  `max_symbols_to_trade: int=1`, `max_concurrent_positions: int=1`,
  `approved_for_live: bool=False` (forced `False` in `__post_init__`).
- `PromotionInput { watchlist: dict, strategy_fit_artifacts: dict[str, dict] }` (symbol →
  strategy-fit-v1).
- `PromotionDecision { approved_for_autonomous_paper: bool, approved_for_live: bool (always
  False), passed: bool, failure_reasons: list[str], approved_symbols: list[str],
  strategy_assignments: dict[str,str], notes: str }`.
- `evaluate_watchlist_promotion(...)`: **"Only the top-ranked symbol (`symbols[0]`) is considered
  for approval in v1."** `approved_symbols = [top_symbol]` only if `passed`.
- `apply_watchlist_promotion(watchlist, decision, config) -> dict` forces
  `approved_for_live=False`, `max_symbols_to_trade=1`, `max_concurrent_positions=1`,
  `symbols=decision.approved_symbols`, `strategy_assignments=decision.strategy_assignments`, and
  adds `selection_reason` + `promotion_decision`.
- `write_promoted_watchlist(watchlist, path) -> Path`.

### 2.4 `symbol_inputs.py` (partial read, lines 240–310 — `SYMBOL-INPUTS-PRODUCER-01`)

`build_symbol_inputs(specs, *, trade_date, source, ...) -> dict`:

```python
return {
    "schema_version": SCHEMA_VERSION,
    "generated_at_utc": generated,
    "trade_date": trade_date,
    "source": source,
    "symbols": symbols,  # dict[symbol -> record] — already multi-symbol
    "approved_for_live": False,
    "notes": notes,
}
```

`write_symbol_inputs_artifact` forces `approved_for_live=False` defensively even if the input
dict was tampered with.

**Key finding:** `symbol_inputs-v1` is *already* a multi-symbol dict keyed by symbol. The
single-symbol bottleneck is specifically in `watchlist_promotion.py`'s `max_symbols_to_trade=1` /
`max_concurrent_positions=1` hard-force and its "only `symbols[0]`" approval-selection logic —
not in the upstream symbol-input or strategy-fit artifacts.

Premarket revalidation (`premarket_revalidation.py`) gate 1 is also
`watchlist_schema_valid — schema_version == "watchlist-v1"` (line 8); it has
`PremarketRevalidationConfig`, `PremarketRevalidationResult` (with its own `schema_version`,
line ~113), and `evaluate_premarket_watchlist()` (line 159) which checks
`watchlist.get("schema_version") != _WATCHLIST_SCHEMA` (line 179).

`risk_simulation.py` has `RiskSimulationConfig`, `RiskSimulationResult` (`schema_version` field,
line 113), `evaluate_watchlist_risk()` (line 158).

---

## 3. Risk / Gating Surfaces (Current State — Account-Wide)

This phase establishes the baseline against which Phase 6's new per-symbol caps are layered.
**Every enforcement mechanism below is account-wide today, and this design does not change that
for the mechanisms marked "preserved unchanged."**

### 3.1 `RiskConfig` / `RiskState` (`mqk-risk/src/types.rs:6-35`)

- `daily_loss_limit_micros: i64` (default `0` = disabled)
- `max_drawdown_limit_micros: i64` (default `0` = disabled)
- `reject_storm_max_rejects_in_window: u32` (default `10`)
- `pdt_auto_enabled: bool` (default `true`)
- `missing_protective_stop_flattens: bool` (default `true`)

`RiskState`: `day_id, day_start_equity_micros, peak_equity_micros, halted, disarmed,
reject_window_id, reject_count_in_window`.

### 3.2 Enforcement (`mqk-risk/src/engine.rs::evaluate()`, lines 76-257)

| Check | Lines | Trigger | Effect | Scope |
|---|---|---|---|---|
| Daily loss limit | 146-184 | `equity <= day_start_equity - daily_loss_limit` | halt | account-wide |
| Max drawdown | 190-228 | `equity <= peak_equity - max_drawdown_limit` | **flatten AND halt** | account-wide |
| Reject storm | 231-249 | `reject_count_in_window >= reject_storm_max_rejects_in_window` | halt | account-wide |
| PDT auto | 131-141 | `pdt_auto_enabled && !pdt_ok` | reject non-risk-reducing requests | account-wide |

**No per-symbol caps exist anywhere in `mqk-risk` today** (confirmed via grep across the crate,
zero matches for any per-symbol risk type).

### 3.3 Deadman TTL (`state.rs:104`, `state/deadman.rs:27-80`)

`DEADMAN_TTL_SECONDS: i64 = 120`. `deadman_truth_for_run()` calls
`mqk_db::enforce_deadman_or_halt()` pre-tick. On expiry, calls
`mqk_db::persist_arm_state_canonical(..., DisarmReason::DeadmanExpired)`, which sets **both**
`integrity.disarmed=true` **and** `integrity.halted=true`. There is no "disarm only" path. This
is a single, run-scoped timer — see Phase 5 Q8 for why it stays run-scoped under multi-symbol.

### 3.4 ReconcileDrift (`mqk-reconcile/src/engine.rs:94-184`)

Triggers: `UnknownBrokerFill`, `UnknownBrokerOrder`, `LocalOrderMissingAtBroker`, `OrderDrift`,
`PositionMismatch`, `UnknownBrokerPosition`. **No configurable threshold** — any mismatch (even
`qty=1` on any single symbol) halts the **entire account**. Enforced via
`ReconcileFreshnessGuard::is_clean()` (`mqk-execution/src/reconcile_guard.rs:29-85`). See Phase 5
Q7 for why this stays account-wide under multi-symbol.

### 3.5 `BrokerSnapshotTruthSource` (`state/types.rs:320-344`)

`Synthetic` (paper, synthesized from local OMS+portfolio) vs. `External` (Alpaca REST).
`EXTERNAL_SNAPSHOT_REFRESH_TICKS: u32 = 60` (`state.rs:109`) = 60 seconds, Alpaca-only.
`external_snapshot_refresher: Arc<RwLock<Option<Arc<AlpacaBrokerAdapter>>>>` (`state.rs:282`).
This refresh is account/run-scoped (one snapshot covers all symbols' positions) — multi-symbol
does not change its cadence or scope.

### 3.6 Halt gate (`mqk-execution/src/gateway.rs:268-288`)

```rust
fn enforce_gates(&self) -> Result<(), GateRefusal> {
    if !self.integrity.is_armed() {
        return Err(GateRefusal::IntegrityDisarmed);
    }
    // RiskGate::evaluate_gate() -> GateRefusal::RiskBlocked(denial)
    // ReconcileGate::is_clean() -> GateRefusal::ReconcileNotClean
}
```

`IntegrityState.is_execution_blocked()` (`mqk-integrity/src/types.rs:145-147`):
`self.disarmed || self.halted` (fields at lines 115-116). This gate is checked **once per tick,
before any dispatch** (CLAUDE.md execution rules: "If the halt flag is set, tick must refuse
before any dispatch"). See Phase 5 Q6.

### 3.7 Position sizing (`capital_policy/position_sizing.rs:37-65, 158-265`)

`PositionSizingOutcome::SizingAuthorized { strategy_id, implied_notional_usd,
max_position_notional_usd }` — cap is **per-strategy** (not per-symbol), sourced from
`capital_allocation_policy.json`'s `per_strategy_budgets[].max_position_notional_usd`. Only
applies to **limit orders**; market orders → `SizingUnverifiable` (B1C only emits market orders,
so this check is currently unverifiable in practice for B1C-originated decisions — an existing,
honest gap). Check: `qty * (limit_price_micros / 1_000_000) <= max_position_notional_usd`
(line 249). Portfolio-level `max_portfolio_notional_usd` (`capital_policy/mod.rs:24-44`) is
account-wide, with no per-symbol breakdown.

### 3.8 PDT (`mqk-risk/src/pdt.rs:30-92`)

`PDT_DAY_TRADE_THRESHOLD: u32 = 4`, `PDT_MIN_EQUITY_MICROS: i64 = 25_000 * 1_000_000` ($25k),
`PDT_DEFAULT_WINDOW_DAYS: u32 = 5`. `PdtPolicy { enabled: bool (true), window_days: u32 (5),
max_day_trades_in_window: u32 (3), min_equity_micros: i64 }`.
`PdtPolicy::finra_defaults()` (lines 73-81). The rolling-window day-trade count (lines 186-199)
is account-wide — correct, since PDT is a FINRA *account*-level rule. See Phase 6 cap #13.

### 3.9 Summary

No `MultiSymbolRiskCaps`-shaped type exists anywhere in the repo (confirmed via grep, zero
matches). Every enforcement mechanism above operates on account-wide aggregates that are already
correct under multi-symbol *by construction* (sums/aggregates don't care how many symbols
contributed to them). What's missing is purely **additive**: new, optional, narrower caps that
sit *inside* the existing account-wide envelope — Phase 6.

---

## 4. Target Architecture — New Components

Nine new components close the gap between today's single-symbol wiring and a multi-symbol
dispatch loop, while preserving every account-wide invariant from Phase 3 and the
`approved_for_live=false` lock from Phase 2.

### 4.1 `MultiSymbolRuntimeConfig`

**Purpose:** the central config object, built once at orchestrator/loop_runner startup, that
replaces the implicit single-symbol assumption baked into `MQK_STRATEGY_SYMBOL` /
`MQK_STRATEGY_IDS[0]`.

**Crate/location:** `mqk-daemon` (new module, e.g. `state/multi_symbol.rs`), consumed by
`mqk-runtime`'s `loop_runner`.

```rust
pub struct MultiSymbolRuntimeConfig {
    pub schema_version: String, // "multi-symbol-runtime-config-v1"
    pub symbols: Vec<SymbolStrategyAssignment>, // ordered; symbols[0] remains "primary" for back-compat telemetry
    pub max_concurrent_symbols: usize,
    pub source: MultiSymbolConfigSource,
}

pub enum MultiSymbolConfigSource {
    EnvSingleSymbolFallback, // legacy MQK_STRATEGY_SYMBOL / MQK_STRATEGY_IDS[0]
    WatchlistArtifactV2 { path: String },
}
```

**Source of truth:** either (a) the legacy env vars (back-compat, exactly one entry) or (b) a
`watchlist-v2` artifact loaded via `MQK_PAPER_WATCHLIST_PATH` (§4.2).

**Validation:** `symbols.len() <= max_concurrent_symbols`, and
`max_concurrent_symbols <= MULTI_SYMBOL_HARD_CEILING` (Phase 6 cap #12). Each symbol's
`strategy_id` must appear in the `StrategyFleetEntry` list (§1.3) — Tier A still restricts to a
single *strategy*, but that one strategy may be assigned to multiple *symbols*.

**Failure modes:** empty `symbols` → orchestrator refuses to start the dispatch loop (fail-closed,
same posture as `PT-TRUTH-01`). Invalid `watchlist-v2` artifact → falls back to
`EnvSingleSymbolFallback` (back-compat), logged, not an error.

**Tests:** pure construction tests for both sources; ceiling-enforcement test; empty-symbols
refuses-start test.

**API/GUI:** surfaced read-only via the extended `WatchlistStatusResponse` (Phase 7).

**Implementation status (`MULTI-SYMBOL-RUNTIME-CONFIG-01`):** the type and both source variants
above are implemented as written, in
`core-rs/crates/mqk-daemon/src/state/multi_symbol_config.rs` (not `state/multi_symbol.rs`).
`build_legacy_single_symbol_config[_from_env]` covers (a); `build_multi_symbol_config_from_watchlist_artifact`
covers (b), re-validating the ceiling and per-symbol assignments against the `watchlist-v2`
artifact at config-build time. `build_multi_symbol_runtime_config_from_env_and_watchlist`
implements the failure-mode language above verbatim: an invalid/ineligible `watchlist-v2`
artifact falls back to `EnvSingleSymbolFallback`, not an error; only when both sources fail does
the combined builder return `Err` (8 `multi_symbol_config_*` reason strings, §6 cap #1 plus the
7 other validation failures). 18 proof tests (`M01`-`M18`) cover both sources, the selection
fallback, the ceiling, and the `approved_for_live` absence invariant (Q9). This is a
config-construction seam only: registered in `state.rs` via `mod`/`pub use` but invoked by
nothing — `loop_runner.rs` and `routes/strategy.rs` are untouched (Patches 3/4 wire dispatch).
The `WatchlistStatusResponse` surface (API/GUI) remains open for a later patch.

### 4.2 `ApprovedPaperWatchlist` v2 (schema evolution of `LoadedWatchlistArtifact`)

**Purpose:** evolves the watchlist artifact from `watchlist-v1` to `watchlist-v2`, adding
explicit multi-symbol fields while preserving every v1 hard invariant.

```rust
pub struct LoadedWatchlistArtifactV2 {
    pub schema_version: String, // "watchlist-v2"
    pub symbols: Vec<String>, // up to MULTI_SYMBOL_HARD_CEILING
    pub strategy_assignments: HashMap<String, String>, // symbol -> strategy_id, REQUIRED for every symbol
    pub max_symbols_to_trade: u64, // may be > 1, still <= MULTI_SYMBOL_HARD_CEILING
    pub max_concurrent_positions: u64, // may be > 1, still <= max_symbols_to_trade
    pub approved_for_autonomous_paper: bool,
    pub approved_for_live: bool, // hard-locked false, validated same as v1
}
```

**Implementation status (`WATCHLIST-V2-SCHEMA-01`):** the schema/validation rules in this
section are implemented in `watchlist_intake.rs` as an extension of the existing
`LoadedWatchlistArtifact` (a `schema_version: String` field was added; no separate
`LoadedWatchlistArtifactV2` type was introduced). `WATCHLIST_SCHEMA_VERSION_V1` /
`WATCHLIST_SCHEMA_VERSION_V2` / `MULTI_SYMBOL_HARD_CEILING` (= 5) constants are defined, and
`WatchlistStatusResponse.schema_version: Option<String>` was added additively. This is
schema/validation only: no runtime multi-symbol dispatch, no `loop_runner.rs` or `state.rs`
changes, and the dry-run admission contract (§22 of the scanner spec) remains unwired. The
`excluded_symbols` field and the promotion-side `watchlist_promotion.py` v2 gate (Patch 11)
remain open.

**Validation (extends `evaluate_watchlist_intake`):**

- `schema_version ∈ {"watchlist-v1", "watchlist-v2"}` (both remain readable; v1 always implies
  `max_symbols_to_trade=1`/`max_concurrent_positions=1`, unchanged).
- v2: every symbol in `symbols` must have an entry in `strategy_assignments`.
- v2: `max_symbols_to_trade <= MULTI_SYMBOL_HARD_CEILING` and
  `max_concurrent_positions <= max_symbols_to_trade`.
- v2: `symbols.len() <= max_symbols_to_trade`.
- `approved_for_live != true` (unchanged hard lock — `Invalid` otherwise, same as v1).

**Failure modes:** same `WatchlistIntakeOutcome` enum; `Invalid` carries all `failure_reasons`,
unchanged shape.

**Promotion-side change** (`watchlist_promotion.py`): the "only `symbols[0]`" gate
(`approved_symbols = [top_symbol]`) becomes `approved_symbols =
symbols[:config.max_symbols_to_trade]` **only when** `schema_version == "watchlist-v2"`. v1
artifacts keep today's `symbols[0]`-only behavior unchanged (back-compat).

**Tests:** schema-migration tests — v1 still valid and behaves as before; v2 with N symbols all
assigned passes; v2 with an unassigned symbol → `Invalid`; v2 exceeding the ceiling → `Invalid`.

**API/GUI:** `WatchlistStatusResponse` gains `schema_version` and `excluded_symbols: Vec<String>`
(symbols present in the artifact beyond `max_symbols_to_trade`, for operator visibility).

### 4.3 `SymbolStrategyAssignment`

**Purpose:** the atomic unit mapping one traded symbol to one `strategy_id` plus its market-data
timeframe; used by `MultiSymbolRuntimeConfig` and `PerSymbolBarWindow`.

```rust
pub struct SymbolStrategyAssignment {
    pub symbol: String,
    pub strategy_id: String, // must be in StrategyFleetEntry list (MQK_STRATEGY_IDS)
    pub timeframe: String, // e.g. "1Min", "1H" — mirrors STRATEGY_MD_TIMEFRAME_ENV per symbol
}
```

**Source of truth:** derived from `LoadedWatchlistArtifactV2.symbols` × `strategy_assignments`,
cross-referenced against `StrategyFleetEntry` at config-build time. Timeframe is currently global
via `MQK_STRATEGY_MD_TIMEFRAME` (Tier A); v2 allows an optional per-symbol override
(`timeframe_overrides: HashMap<String,String>` in the watchlist-v2 artifact), defaulting to the
global env value when absent.

**Validation:** `strategy_id` must exist in the `StrategyFleetEntry` list. An unknown
`strategy_id` causes that symbol to be **excluded** from `MultiSymbolRuntimeConfig.symbols` with
a logged warning — fail-closed, never silently substituted.

**Tier A constraint note:** `StrategyHost::register()` still errors `MultiStrategyNotAllowed` if
more than one *distinct strategy instance* would be registered. A single `strategy_id` assigned
to multiple symbols does **not** violate this — one `StrategyHost` instance processes bars for
multiple symbols sequentially (Phase 5 Q1). Heterogeneous `strategy_id`s per symbol remain
explicitly out of scope (§0).

**Implementation status (`MULTI-SYMBOL-RUNTIME-CONFIG-01`):** the `symbol`/`strategy_id`/
`timeframe` struct above is implemented exactly as written in `multi_symbol_config.rs`. The
per-symbol `timeframe_overrides: HashMap<String,String>` described above is **not**
implemented in this patch — every symbol produced by
`build_multi_symbol_config_from_watchlist_artifact` shares one caller-supplied
`default_timeframe` (sourced from `MQK_STRATEGY_MD_TIMEFRAME`), consistent with Tier A's single
global timeframe. The `strategy_id`-exists-in-fleet cross-check is also not implemented here —
`build_multi_symbol_config_from_watchlist_artifact` only requires that every symbol have *some*
`strategy_assignments` entry (`multi_symbol_config_missing_assignment` if absent); validating
that entry against `StrategyFleetEntry` remains open for the dispatch-loop patch (3/4).

### 4.4 `PerSymbolBarWindow`

**Purpose:** generalizes the single-slot `pending_strategy_bar_input: Arc<Mutex<Option<StrategyBarInput>>>`
+ `RecentBarsWindow` machinery to a per-symbol keyed structure.

```rust
pub struct PerSymbolBarWindow {
    pub windows: HashMap<String, RecentBarsWindow>, // keyed by symbol
}

// state.rs equivalent of pending_strategy_bar_input becomes:
pending_strategy_bar_inputs: Arc<Mutex<HashMap<String, StrategyBarInput>>>, // keyed by symbol, .remove(symbol) instead of .take()
```

`StrategyBarInput` (§1.1) gains a `symbol: String` field — additive; existing single-symbol
callers populate it from `MQK_STRATEGY_SYMBOL` for back-compat.

`tick_strategy_dispatch()` becomes `tick_strategy_dispatch_for_symbol(symbol: &str) ->
Option<StrategyBarResult>`, called once per entry in `MultiSymbolRuntimeConfig.symbols` inside the
per-symbol loop (Phase 5). `fetch_recent_completed_bars_for_strategy` (§1.1) already takes
`symbol: &str` — **no change needed in `mqk-db`**; only the daemon-side caching/dispatch wrapper
becomes symbol-keyed.

**Failure modes:** missing bar data for one symbol (empty `md_bars` rows) → that symbol's
dispatch falls back to the existing stub-invocation path and does **not** block other symbols'
dispatch in the same tick.

**Tests:** per-symbol independence (symbol A has bars, symbol B doesn't → both produce results, B
uses stub fallback); concurrent-key isolation (writing symbol A's pending bar input doesn't
affect symbol B's slot).

### 4.5 `PerSymbolStrategyDecision` (no new type — documentation of an existing-correct seam)

**Purpose:** `InternalStrategyDecision` (§1.6) is **already symbol-aware**
(`pub symbol: String`). This component records that **no structural change** is needed to
`InternalStrategyDecision`, `InternalDecisionOutcome`, `bar_result_to_decisions`, or
`submit_internal_strategy_decision` — they are already correct for multi-symbol. The only change
is at the call site: `bar_result_to_decisions` is called once per symbol's
`StrategyBarResult` (from `PerSymbolBarWindow` dispatch), inside the per-symbol loop, with the
**same** `current_positions: BTreeMap<String,i64>` snapshot shared across all symbols in a tick
(Phase 5 Q2).

`decision_id` UUIDv5 derivation already includes `symbol` in its namespace string — no collision
risk across symbols (Phase 5 Q3, confirmed via full-file read of `decision.rs`).

Gates 0, 2, 3, 4, 7 in `submit_internal_strategy_decision` are already correctly scoped
(per-decision or per-strategy). Gates 1, 5, 6 remain account-wide by design (§3). The **only new
gate** this design proposes is **Gate 1f** (per-symbol day order count, Phase 6 cap #4), inserted
between Gate 1 and Gate 1e.

### 4.6 `PerSymbolTargetState`

**Purpose:** makes the B1C diagnostic loop's per-target computation (`current_qty`, `target_qty`,
`delta`, `no_order_reason` — currently only `tracing::info!`'d at
`loop_runner.rs` ~593-615, never retained) queryable for `MultiSymbolDispatchSummary` (§4.8) and
the GUI (Phase 8).

```rust
pub struct PerSymbolTargetState {
    pub symbol: String,
    pub strategy_id: String,
    pub current_qty: i64,
    pub target_qty: i64,
    pub delta: i64,
    pub no_order_reason: String, // "already_at_target" | "b5_short_sale_guard" | "order_will_be_submitted"
                                  // | "bar_data_stale" | "max_new_orders_per_tick_reached" (new, Phase 6)
    pub last_decision_id: Option<String>,
    pub last_decision_disposition: Option<String>, // InternalDecisionOutcome.disposition
    pub updated_at_utc: String,
}
```

**Storage:** in-memory `HashMap<String, PerSymbolTargetState>` on daemon state
(`Arc<RwLock<...>>`, mirroring `external_snapshot_refresher`), updated at the end of each
per-symbol dispatch iteration. **Not persisted to DB** — observability only, rebuilt every tick;
loss on restart is acceptable (same posture as other in-memory snapshot caches).

**API/GUI:** feeds `MultiSymbolDispatchSummary.per_symbol` (§4.8) and
`OmsOverviewResponse.per_symbol_status` (Phase 7).

### 4.7 `MultiSymbolRiskCaps`

**Purpose:** the new, optional config block holding the per-symbol and aggregate caps from
Phase 6, distinct from the existing account-wide `RiskConfig` (§3.1).

```rust
pub struct MultiSymbolRiskCaps {
    pub schema_version: String, // "multi-symbol-risk-caps-v1"
    pub max_concurrent_symbols: usize, // cap #1, mirrors MultiSymbolRuntimeConfig; defaults to 1
    pub per_symbol_max_position_qty: Option<i64>, // cap #2
    pub per_symbol_max_notional_usd: Option<f64>, // cap #3
    pub per_symbol_day_order_count_limit: Option<u32>, // cap #4
    pub aggregate_gross_exposure_cap_usd: Option<f64>, // cap #5 (extends max_portfolio_notional_usd)
    pub max_new_orders_per_tick: Option<u32>, // cap #6
    pub per_symbol_bar_staleness_secs: Option<i64>, // cap #9
}
```

**Source of truth:** loaded from `capital_allocation_policy.json` (extends
`capital_policy/mod.rs`) as an **optional** new top-level key `multi_symbol_risk_caps`. Absence
of the key ⇒ all `Option` fields `None` ⇒ those caps disabled, and `max_concurrent_symbols`
defaults to `1` (preserving today's Tier A behavior exactly). This is acceptable fail-open *only*
because every account-wide cap from Phase 3 remains fully enforced regardless — removing this
optional layer removes nothing that exists today.

**Validation:** every populated numeric field must be `> 0`; `1 <= max_concurrent_symbols <=
MULTI_SYMBOL_HARD_CEILING`.

**Tests:** schema parse tests (absent key → defaults; present key → values loaded); validation
rejection tests (zero/negative → `PolicyInvalid`, same disposition as existing capital-policy
errors).

**API/GUI:** read-only via new `MetricsDashboardResponse` per-symbol panel fields and
`RiskScreen` (Phase 8).

### 4.8 `MultiSymbolDispatchSummary`

**Purpose:** the primary new evidence surface — `GET /api/v1/strategy/multi-symbol-dispatch-summary`.

```rust
pub struct MultiSymbolDispatchSummaryResponse {
    pub canonical_route: String, // "/api/v1/strategy/multi-symbol-dispatch-summary"
    pub backend: String, // "daemon.runtime_state" | "daemon.runtime_state+postgres"
    pub truth_state: String, // "no_snapshot" | "active" | "legacy_single_symbol"
    pub runtime_execution_mode: String, // mirrors StrategySummaryResponse: "single_symbol" | "multi_symbol"
    pub configured_symbol_count: usize,
    pub per_symbol: Vec<PerSymbolDispatchRow>,
}

pub struct PerSymbolDispatchRow {
    pub symbol: String,
    pub strategy_id: String,
    pub current_qty: i64,
    pub target_qty: i64,
    pub delta: i64,
    pub no_order_reason: String,
    pub last_decision_id: Option<String>,
    pub last_decision_disposition: Option<String>,
    pub day_order_count: u32, // cap #4 visibility
    pub day_order_limit: Option<u32>,
    pub bar_staleness_secs: Option<i64>, // cap #9
}
```

`truth_state = "legacy_single_symbol"` when `MultiSymbolConfigSource::EnvSingleSymbolFallback` —
existing single-symbol deployments see this value, **not** `"active"`, per gui_rules.md (a
distinct state, not a synonym for "active"). `truth_state = "no_snapshot"` before the first tick
completes (mirrors `OmsOverviewResponse`'s existing pattern).

### 4.9 `MultiSymbolEvidenceSnapshot`

**Purpose:** the evidence-capture extension — adds a `multi_symbol_dispatch_summary.json` file
(raw `GET` of §4.8's route) to each `Capture-PaperSmokeEvidence.ps1` run, plus a per-symbol
breakdown section in `Review-PaperSmokeEvidence.ps1`'s output schema.

```json
{
  "schema_version": "multi-symbol-evidence-v1",
  "captured_at_utc": "...",
  "dispatch_summary": { "...": "MultiSymbolDispatchSummaryResponse" },
  "watchlist_status": { "...": "WatchlistStatusResponse (v2)" }
}
```

No new Rust/Python types beyond what's captured — purely a PowerShell evidence-capture/review
extension, consistent with the existing `Capture-PaperSmokeEvidence.ps1` pattern (GET existing
routes, write JSON to the evidence dir).

---

## 5. Dispatch Loop Design — Nine Questions

### Q1. One `StrategyHost` instance, or one per symbol?

Tier A's `StrategyHost::register()` single-registration constraint limits us to **one** strategy
implementation instance. Design: **one `StrategyHost` instance, called sequentially once per
assigned symbol within a tick**, each call passing that symbol's `PerSymbolBarWindow` entry.
`on_bar(ctx)` is stateless w.r.t. symbol identity at the `StrategyHost` level.

**Caveat (explicit, not silently assumed):** if a strategy implementation is *internally*
stateful across bars (e.g. holds an EMA), per-symbol state isolation inside that implementation
is a separate concern this design does not address. The currently-registered Tier A strategies
are assumed stateless or are out of scope until verified — a follow-on patch must verify this
before any stateful strategy is used multi-symbol.

### Q2. One snapshot read per tick, or one per symbol?

**One read per tick, shared across all symbols.** `current_positions: BTreeMap<String,i64>` is
built once, before the per-symbol loop begins — mirroring the existing B1C block's single read
(`loop_runner.rs` ~580).

**Rationale:** avoids a torn-snapshot race where symbol A sees pre-fill positions and symbol B
(processed milliseconds later in the same tick) sees post-fill positions from a fill that
arrived mid-tick. All symbols in a tick reason about the *same* point-in-time portfolio state.

**Consequence:** a fill for symbol A completing mid-tick is not reflected in symbol B's delta
until the *next* tick. This is the existing single-symbol behavior (ticks are the unit of
consistency) and is preserved unchanged.

### Q3. `decision_id` collision risk across symbols at identical `now_micros`?

**None.** `decision_id = UUIDv5("{run_id}:{strategy_id}:{symbol}:{side}:{qty}:{now_micros}")`
already includes `symbol` in the namespace string (`decision.rs`, full file read this session).
Two symbols producing decisions in the same tick (same `now_micros`) produce different
`decision_id`s because `symbol` differs. **No code change needed.**

### Q4. Cross-symbol decision conflicts within one tick?

The only cross-symbol coupling points are (a) `current_positions` (Q2, frozen for the tick) and
(b) the new per-tick caps (cap #6 `max_new_orders_per_tick`, cap #1 `max_concurrent_symbols`).
Otherwise, decisions for different symbols are independent — the account-wide gates (ARMED,
running, day-limit, budget) apply uniformly per-decision regardless of order, **except** Gate 1
(`day_signal_limit`) and the new cap #6, which are *cumulative counters* incremented as each
decision in the tick is processed — so **processing order within a tick determines which symbols
hit these caps first**.

**Design decision:** iterate `MultiSymbolRuntimeConfig.symbols` in **artifact order** (the order
symbols appear in the `watchlist-v2` `symbols` array). This gives the operator direct,
documented control over priority ordering via how they author the watchlist artifact — must be
called out in the watchlist-v2 schema docs (operator-facing).

### Q5. Partial-symbol failure handling

If `tick_strategy_dispatch_for_symbol(B)` errors, it does **not** abort the whole tick (including
symbol A's already-submitted decisions). Each symbol's dispatch+decision submission is wrapped in
its own error boundary — the existing `if let Some(bar_result) = ...` short-circuit pattern
extends per-symbol: errors for symbol B are logged (`tracing::warn!` with a `symbol` field) and
the loop `continue`s to symbol B+1.

Symbol A's decisions, already submitted via `outbox_enqueue` before B's error, are **not** rolled
back — `outbox_enqueue` is its own atomic DB transaction per db_rules.md ("writes that must be
atomic must happen inside a single transaction"), and each symbol's decision submission is
already such an atomic, independent unit.

### Q6. Halt gate — once per tick, or per symbol?

**Once per tick, before the per-symbol loop, unchanged.** The halt gate
(`enforce_gates`/`is_execution_blocked()`, §3.6) is account-wide by definition — execution_rules.md:
"If the halt flag is set, tick must refuse before any dispatch." The underlying risks it guards
(capital loss limits, drawdown, reject storms) are account-wide regardless of symbol count. If
halted, the **entire** per-symbol loop is skipped — tick refuses before any dispatch, exactly as
today.

### Q7. Per-symbol reconcile-drift halting?

**No — explicit non-goal.** `ReconcileDrift` triggers (`PositionMismatch`,
`UnknownBrokerPosition`, etc., §3.4) remain **account-wide halts**. A mismatch on *any* symbol —
including ones outside the configured multi-symbol set, e.g. a stray manual order — halts the
**entire** account, exactly as today.

Cap #7 ("per-symbol reconcile drift visibility") is **observability only** — surfacing which
symbol's mismatch triggered the existing account-wide halt, in
`MultiSymbolDispatchSummary`/`ReconcileScreen`. It does **not** introduce a narrower halt scope.
Per-symbol reconcile granularity (halting only the affected symbol while others continue) would
weaken the existing "any mismatch halts everything" fail-closed posture and is explicitly out of
scope — it would require its own risk review and design.

### Q8. Deadman TTL — per-symbol, or account-wide?

**Account-wide, unchanged.** `DEADMAN_TTL_SECONDS=120` and `enforce_deadman_or_halt()` operate on
the *run*, not on individual symbols. One daemon process drives the per-symbol loop within each
tick, so one deadman heartbeat per tick (covering all symbols' dispatch within that tick) is
correct and sufficient. **No per-symbol deadman concept is introduced** (see also Phase 6
cap #10 — a documentation-only entry to prevent future fragmentation of this invariant).

### Q9. `approved_for_live` / live-routing invariant preservation

**No new path.** `evaluate_watchlist_intake` (extended for v2, §4.2) retains the identical
hard-lock check (`approved_for_live != true` → `Invalid`) for both `watchlist-v1` and
`watchlist-v2`. `apply_watchlist_promotion` continues to force `approved_for_live=False`
regardless of how many symbols are approved. The deployment-mode gate
(`deployment_mode_readiness`, `PT-TRUTH-01`) and the WS-continuity gate (`BRK-00R-04`) are
evaluated **once** at orchestrator startup and are entirely independent of symbol count — adding
symbols touches neither gate.

**Conclusion:** the live-lock invariant is preserved by construction; no new test is strictly
required to *prove* this (it follows from the gates being symbol-count-independent), but Phase 10
includes one regression test per modified file (`watchlist_intake.rs`,
`watchlist_promotion.py`) asserting `approved_for_live` stays `false` with N>1 symbols, for
defense-in-depth.

---

## 6. Risk Caps Design — Thirteen Caps

Each cap is additive and optional unless stated otherwise. Caps #7, #10, #11, #13 introduce **no
new enforcement** — they exist in this list to make explicit design decisions that a future
implementer must *not* silently overturn (e.g. by "helpfully" adding per-symbol deadman timers).

### Cap #1 — `max_concurrent_symbols`

- **Config:** `MultiSymbolRiskCaps.max_concurrent_symbols`, default `1` (preserves Tier A).
- **Enforcement:** at `MultiSymbolRuntimeConfig` construction (§4.1) — if
  `symbols.len() > max_concurrent_symbols`, construction truncates to the first
  `max_concurrent_symbols` entries (artifact order, Q4) and logs a warning; excluded symbols are
  surfaced, not silently dropped.
- **Test:** construction with N=3 symbols, `max_concurrent_symbols=1` → only `symbols[0]` used,
  warning logged.
- **GUI:** `WatchlistStatusResponse.excluded_symbols` (§4.2), `StrategyScreen`.

### Cap #2 — `per_symbol_max_position_qty`

- **Config:** `MultiSymbolRiskCaps.per_symbol_max_position_qty: Option<i64>`, default `None`.
- **Enforcement:** new check between bar-result and decision construction — if
  `target_qty.abs() > cap`, clamp `target_qty` to `cap` (preserving sign) **before** delta
  computation, and log `b1c_target_qty_clamped_per_symbol_cap`. Because clamping overrides the
  strategy's intended signal, this must be **loud**: fire a Discord alert (mirroring the B5
  short-sale guard's `notify_trade_event` pattern), gated by a new per-symbol-per-day dedup claim
  (same shape as `try_claim_b5_alert`) to avoid alert storms.
- **Test:** `target_qty=1000`, `cap=500` → clamped to `500`, alert fired once per symbol per day.
- **GUI:** `RiskScreen` per-symbol cap-utilization row.

### Cap #3 — `per_symbol_max_notional_usd`

- **Config:** `MultiSymbolRiskCaps.per_symbol_max_notional_usd: Option<f64>`, default `None`.
- **Enforcement:** extends `position_sizing.rs` (§3.7) — currently per-strategy
  (`max_position_notional_usd`); add a per-**symbol** check alongside it, extending
  `PositionSizingOutcome` with a new `SizingDeniedPerSymbolCap { symbol,
  implied_notional_usd, cap_usd }` variant.
- **Honest gap:** same limitation as the existing per-strategy check — only applies to **limit**
  orders. B1C only emits `order_type="market"` (§1.6), so **this cap is currently unverifiable in
  practice for all B1C-originated decisions** until limit-order support exists. This is stated
  here explicitly rather than silently implied to "just work."
- **Test:** limit-order decision with notional > cap → `SizingDeniedPerSymbolCap` →
  `InternalDecisionOutcome.disposition = "rejected"` with a blocker naming the cap.
- **GUI:** `RiskScreen`, `MetricsScreen` per-symbol notional panel.

### Cap #4 — `per_symbol_day_order_count_limit` (new Gate 1f)

- **Config:** `MultiSymbolRiskCaps.per_symbol_day_order_count_limit: Option<u32>`, default `None`.
- **Enforcement:** new **Gate 1f** in `submit_internal_strategy_decision`, between Gate 1
  (account-wide `day_signal_limit`) and Gate 1e (per-strategy budget). Requires a new per-symbol
  counter `day_signal_count_by_symbol: HashMap<String,u32>`, reset at the **same** day-rollover
  boundary as the existing account-wide `day_signal_count` (reuse existing rollover detection —
  do not introduce a second clock source, per CLAUDE.md determinism). On exceed: new
  `InternalDecisionOutcome.disposition = "symbol_day_limit_reached"` (additive — all existing
  dispositions unchanged).
- **Test:** symbol A hits its per-symbol limit while the account-wide limit is not yet reached →
  symbol A's further decisions return `"symbol_day_limit_reached"`; symbol B's decisions in the
  same tick still return `"accepted"` (proves per-symbol granularity).
- **GUI:** `MultiSymbolDispatchSummary.per_symbol[].day_order_count` / `day_order_limit` (§4.8),
  `AlertsScreen`.

### Cap #5 — `aggregate_gross_exposure_cap_usd`

- **Config:** `MultiSymbolRiskCaps.aggregate_gross_exposure_cap_usd: Option<f64>`, default
  `None`. **Distinct** from the existing account-wide `max_portfolio_notional_usd`
  (`capital_policy/mod.rs:24-44`, §3.7), which already exists and is unaffected by absence of this
  cap.
- **Enforcement:** same `position_sizing.rs` path as `max_portfolio_notional_usd` — extend the
  comparison to `min(max_portfolio_notional_usd, aggregate_gross_exposure_cap_usd.unwrap_or(f64::MAX))`
  (fail-closed: the more restrictive of the two wins).
- **Test:** both set, aggregate cap lower → aggregate cap wins; only
  `max_portfolio_notional_usd` set → unchanged existing behavior (back-compat).
- **GUI:** `MetricsScreen` risk panel, `RiskScreen`.

### Cap #6 — `max_new_orders_per_tick`

- **Config:** `MultiSymbolRiskCaps.max_new_orders_per_tick: Option<u32>`, default `None`
  (unbounded — matches today's implicit behavior, where the per-symbol loop processes every
  configured symbol every tick).
- **Enforcement:** a per-tick counter `new_orders_this_tick: u32`, incremented each time
  `submit_internal_strategy_decision` returns `accepted=true`. If
  `new_orders_this_tick >= cap` before processing the next symbol, remaining symbols in this tick
  are skipped with `no_order_reason = "max_new_orders_per_tick_reached"` (new value, additive to
  `PerSymbolTargetState.no_order_reason`, §4.6). Skipped symbols are **not lost** — their
  decisions are re-evaluated fresh next tick from then-current bar/position state, so no queuing
  mechanism is needed.
- **Test:** 3 symbols all produce accepted decisions, `cap=1` → `symbols[0]` accepted,
  `symbols[1:]` show `"max_new_orders_per_tick_reached"`.
- **GUI:** `MultiSymbolDispatchSummary`, `ExecutionScreen` ("Dispatching" stat card gains
  per-tick-cap context).

### Cap #7 — `per_symbol_reconcile_drift_visibility` (observability only)

- **Config:** none — always on, pure observability.
- **Enforcement:** **none** (no halt-scope change, per Q7). The existing reconcile engine output
  (mismatches list) is already symbol-keyed (`PositionMismatch { symbol, ... }` etc., per
  `mqk-reconcile` types). This cap surfaces that existing per-symbol detail in
  `MultiSymbolDispatchSummary`/`ReconcileScreen`, rather than only the account-wide "reconcile
  dirty" boolean. **No new halt path, no new config.**
- **Test:** existing reconcile scenario tests extended with an assertion that per-symbol
  mismatch detail is queryable from the new response type, not just the boolean dirty flag.
- **GUI:** `ReconcileScreen` mismatches-by-symbol table.

### Cap #8 — `b5_short_sale_guard_per_symbol` (already correct — test-only addition)

- **Config:** none — `try_claim_b5_alert(symbol)` is already per-symbol-keyed
  (`signal_intake.rs:62`, §1.7).
- **Enforcement:** **none new** — `bar_result_to_decisions`'s B5 guard
  (`current <= 0 || qty_to_sell > current` → drop) already operates per-target since it iterates
  `result.intents.output.targets` and checks `current_positions.get(&t.symbol)`.
- **Test (the missing proof):** new multi-symbol scenario test — symbol A flat (`current=0`)
  attempts sell → dropped + alert; symbol B long 100 attempts sell 50 → allowed, no alert; both in
  the **same tick** → proves independence. Currently only single-symbol B5 tests exist.
- **GUI:** `AlertsScreen` gains a `symbol` column (B5 alerts already carry `symbol` in their
  Discord payload per the loop_runner B1C block — just not yet in `AlertsActiveResponse` rows).

### Cap #9 — `per_symbol_bar_staleness_guard`

- **Config:** `MultiSymbolRiskCaps.per_symbol_bar_staleness_secs: Option<i64>`, default `None`
  (disabled — matches today; no staleness check exists for the single-symbol path either).
- **Enforcement:** in `PerSymbolBarWindow` dispatch (§4.4) — after
  `fetch_recent_completed_bars_for_strategy`, compute `now - latest_bar.end_ts`. If `> cap`, that
  symbol's dispatch for this tick uses the **existing stub-fallback path** (unchanged code path —
  same as the empty-`md_bars` case) instead of the DB-backed window, and
  `no_order_reason = "bar_data_stale"` (new value) is recorded. This **does not halt anything** —
  it degrades that symbol to stub-strategy behavior for the tick.
- **Test:** symbol with `latest_bar.end_ts` older than cap → stub fallback used +
  `"bar_data_stale"` recorded; symbol with a fresh bar → DB window used normally.
- **GUI:** `MultiSymbolDispatchSummary.per_symbol[].bar_staleness_secs` (§4.8), `MarketDataScreen`
  per-symbol freshness.

### Cap #10 — `deadman_ttl_unchanged` (documentation-only, no code change)

- **Config:** none — `DEADMAN_TTL_SECONDS=120` unchanged (§3.3).
- This entry exists solely to make explicit (per the "no stale snapshots in living docs" rule)
  that multi-symbol dispatch does **not** introduce N deadman timers — there is exactly **one**,
  account/run-wide, as today (Q8). No code change, no test change. Its purpose is to prevent a
  future implementer from "helpfully" adding per-symbol deadman timers that would fragment the
  restart-safety story.

### Cap #11 — `kill_switch_propagation` (documentation-only, no code change)

- **Config:** none — existing `kill_switch_active: bool` on `IntegrityState`, account-wide.
- **Enforcement:** checked **once**, before the per-symbol loop begins (same point as the halt
  gate, Q6) — if `kill_switch_active`, the **entire** per-symbol loop is skipped, identical to
  halt. This entry makes explicit that kill-switch is **not** symbol-scoped — an operator cannot
  kill-switch "just AAPL." Kill-switch remains a blunt, account-wide instrument by design:
  partial kill-switches would create ambiguity about which symbols are "safe," violating
  fail-closed.
- **Test:** existing kill-switch tests extended with N>1 symbols configured → kill-switch active
  → **zero** symbols dispatch (not just `symbols[0]`).

### Cap #12 — `max_symbols_hard_ceiling`

- **Config:** compile-time const `MULTI_SYMBOL_HARD_CEILING: usize` (proposed value: **5** for
  Tier A+ — chosen as the smallest number that materially exercises "multi" beyond 2 while
  keeping operator review burden manageable; **not** derived from capacity/load testing — a
  policy ceiling, raisable later via a const change + version bump, not a technical limit).
- **Enforcement:** `LoadedWatchlistArtifactV2` validation (§4.2) rejects (`Invalid`) any artifact
  where `symbols.len() > MULTI_SYMBOL_HARD_CEILING` OR
  `max_symbols_to_trade > MULTI_SYMBOL_HARD_CEILING`, **regardless** of `max_concurrent_symbols`
  (cap #1) — this is a hard, non-configurable ceiling, distinguishing it from cap #1 (which is
  configurable and must itself be `<= MULTI_SYMBOL_HARD_CEILING`).
- **Test:** artifact with 6 symbols, ceiling=5 → `Invalid` with
  `REASON_MULTI_SYMBOL_CEILING_EXCEEDED` (new failure-reason constant).

### Cap #13 — `pdt_round_trip_awareness` (test-only addition)

- **Config:** none — existing `PdtPolicy`/`PDT_DAY_TRADE_THRESHOLD=4`/rolling-window day-trade
  count (§3.8) remain account-wide and **already** aggregate correctly across all symbols. PDT is
  a FINRA *account*-level rule by definition — correctly account-wide, must **not** be
  per-symbolized.
- **Enforcement:** **none new**.
- **Test (the missing proof):** new multi-symbol scenario test — 2 day-trades on symbol A + 2 on
  symbol B in the same rolling window = 4 total → PDT threshold correctly triggers from the
  **sum** across symbols, not from each symbol's individual count of 2 (each "under threshold"
  alone). Today's PDT tests are single-symbol, so this cross-symbol summation has never been
  exercised with N>1.

---

## 7. API / Evidence Surface Additions

All additions below are additive fields or new routes with explicit `truth_state`, per
gui_rules.md ("Every response type that carries snapshot data must have an explicit
`truth_state` field" / "Do not substitute a zero value, empty list, or 'healthy' label when the
backend has not confirmed that state"). None of these change the shape or meaning of an existing
field — existing single-symbol consumers see identical responses plus new optional/empty
collections.

### 7.1 `WatchlistStatusResponse` (`api_types.rs:3598-3624`) — extend

New fields:

```rust
pub schema_version: String, // "watchlist-v1" | "watchlist-v2"
pub excluded_symbols: Vec<String>, // symbols present in artifact beyond max_symbols_to_trade (cap #1)
```

`approved_for_live: bool` remains, unconditionally `false`, as today.

### 7.2 `StrategySummaryResponse` / `StrategySummaryRow` (`api_types.rs:1053-1070`, `:953+`) — extend

`runtime_execution_mode` gains a new possible value: `"multi_symbol"` (alongside existing
`"single_strategy"|"fleet_not_configured"|"fleet"|"unknown"`), set when
`MultiSymbolRuntimeConfig.source == WatchlistArtifactV2` and `symbols.len() > 1`.

`StrategySummaryRow` gains an optional field:

```rust
pub symbol_assignments: Option<Vec<String>>, // symbols this strategy_id is assigned to; None for legacy single-symbol rows
```

`None` (not `Some(vec![])`) for legacy single-symbol configs — distinguishing "not derivable
under this config shape" from "assigned to zero symbols," per gui_rules.md's
unavailable/empty/present distinction.

### 7.3 New route: `GET /api/v1/strategy/multi-symbol-dispatch-summary`

Returns `MultiSymbolDispatchSummaryResponse` (§4.8). `truth_state` values:
`"no_snapshot"` (before first tick) | `"active"` (multi-symbol config loaded and at least one
tick completed) | `"legacy_single_symbol"` (env-fallback config, §4.1). Backend:
`"daemon.runtime_state"` (in-memory `PerSymbolTargetState`, §4.6) plus, when day-order-count
fields are populated, `"+postgres"` (day-rollover counters persisted alongside the existing
account-wide `day_signal_count`).

### 7.4 `OmsOverviewResponse` (`api_types.rs:2070+`) — extend

New field:

```rust
pub per_symbol_status: Vec<PerSymbolStatusRow>, // empty for legacy single-symbol configs (back-compat)

pub struct PerSymbolStatusRow {
    pub symbol: String,
    pub position_qty: i64,
    pub open_order_count: usize,
    pub fill_count: usize,
    pub reconcile_mismatch: bool, // cap #7 visibility — does NOT imply per-symbol halt scope (Q7)
}
```

Empty `Vec` for legacy single-symbol configs is correct here (not a `truth_state` violation) —
the *response itself* still carries its existing `truth_state` (`"no_snapshot"|"active"`); the
empty per-symbol vec simply means "this run has one implicit symbol, already represented by the
existing top-level fields."

### 7.5 `MetricsDashboardResponse` (`api_types.rs:1993+`) — extend

New optional per-symbol panel:

```rust
pub per_symbol_exposure: Vec<PerSymbolExposureRow>, // empty for legacy single-symbol configs

pub struct PerSymbolExposureRow {
    pub symbol: String,
    pub market_value: Option<f64>,
    pub notional_usd: Option<f64>, // None if unverifiable (cap #3 honest gap — market orders)
    pub per_symbol_notional_cap_usd: Option<f64>, // cap #3 config value, for utilization display
    pub gross_exposure_contribution_usd: Option<f64>, // contribution to cap #5
}
```

`daily_pnl`, `drawdown_pct`, `loss_limit_utilization_pct` remain `Option<f64>` always-`None` as
documented in `CC-05` — multi-symbol does not create a new derivation source for these, so they
stay honest.

### 7.6 `AlertsActiveResponse` — extend rows

Existing alert rows (B5 short-sale guard, day-limit, gap-escalation, and the two new alerts from
caps #2/#4) gain an optional field:

```rust
pub symbol: Option<String>, // Some(symbol) for symbol-scoped alerts (B5, cap #2 clamp, cap #4 symbol-day-limit); None for account-wide alerts (day-limit, gap-escalation, kill-switch)
```

`None` here is the *correct* terminal value for genuinely account-wide alerts — not a "not yet
wired" placeholder. This distinction must be preserved in the GUI (§8.7).

### 7.7 `ReconcileScreen`-backing response — extend

Whatever response currently backs `ReconcileScreen` (`/reconcile/status`, `/reconcile/mismatches`
per `truthRendering.ts` hints) gains, on each mismatch row, the `symbol` field already present in
the underlying `mqk-reconcile` mismatch types (§6, cap #7) — purely exposing existing internal
detail, no new computation.

---

## 8. GUI Surface Additions

Every addition below sits **behind** the existing hard-block pattern
(`if (truthState !== null) return <TruthStateNotice state={truthState} />`, driven by
`panelTruthRenderState()` / `isTruthHardBlock()` in `truthRendering.ts`, §0). No new screen is
added; all additions are new sections/columns within existing screens, gated by the same
`PANEL_TRUTH_REQUIREMENTS` entries those screens already use. For legacy single-symbol configs
(`truth_state="legacy_single_symbol"` on §7.3, or empty per-symbol arrays on §7.4/§7.5), the new
sections render nothing extra — existing single-symbol views are pixel-identical to today.

### 8.1 `StrategyScreen`

New "Symbol Assignments" table, populated from `StrategySummaryRow.symbol_assignments` (§7.2).
One row per `SymbolStrategyAssignment` (§4.3): symbol, strategy_id, timeframe. Gated by the
existing `strategy` panel's `PANEL_TRUTH_REQUIREMENTS` entry (`hints: ["/strategy/summary"]`) —
this screen already hard-blocks on `truth_state === "not_wired"`; the new table inherits that
block with no separate gate.

### 8.2 `ExecutionScreen`

New "Per-Symbol Status" table, populated from `OmsOverviewResponse.per_symbol_status` (§7.4):
symbol, position qty, open orders, fills, reconcile-mismatch indicator (cap #7). Sits below the
existing StatCards row (Active Orders, Dispatching, Rejects Today, Stuck OMS Orders). The
"Dispatching" stat card gains a tooltip noting `max_new_orders_per_tick` context (cap #6) when
that cap is configured. Gated by the existing `execution` panel hard-block
(`hints: ["/execution/orders"]`, plus `EXTERNAL_BROKER_GATED_PANELS` continuity-gap check) — this
is the pattern informally referred to as "GUI-EXECUTION-SCREEN-RENDER-GUARD-01" (§0); the new
table sits entirely inside the existing `if (truthState !== null) return ...` block, i.e. it
never renders unless the existing guard already permits the rest of the screen to render.

### 8.3 `PortfolioScreen`

**No change.** Positions are already keyed by `symbol` (`p.symbol`, per `current_positions`
construction in `loop_runner.rs`, §1.5) — this screen is already correct for multi-symbol.
Recorded here explicitly so a future implementer does not "fix" something that isn't broken.

### 8.4 `RiskScreen`

New "Per-Symbol Caps" section, populated from `MultiSymbolRiskCaps` config values (§4.7) ×
`MetricsDashboardResponse.per_symbol_exposure` (§7.5): for each configured symbol, show
`per_symbol_max_position_qty` utilization (cap #2), `per_symbol_max_notional_usd` utilization
(cap #3, with an explicit "unverifiable — market orders only" note per the honest gap), and
contribution to `aggregate_gross_exposure_cap_usd` (cap #5). Gated by the existing `risk` panel
(`hints: ["/risk/denials"]`).

### 8.5 `MetricsScreen`

New per-symbol notional/exposure breakdown table from `MetricsDashboardResponse.per_symbol_exposure`
(§7.5). Existing always-`None` fields (`daily_pnl`, `drawdown_pct`,
`loss_limit_utilization_pct`) remain always-`None` at the per-symbol level too — no new
derivation source is created by this design, so per-symbol PnL/drawdown are *not* shown (showing
`None` per-symbol would be redundant with the existing top-level `None` and could misleadingly
imply per-symbol PnL tracking exists). Gated by the existing `metrics` panel
(`hints: ["/metrics/dashboards"]`).

### 8.6 `ReconcileScreen`

New "Mismatches by Symbol" table from §7.7's extended mismatch rows (cap #7). Purely additive
detail under the existing account-wide "reconcile dirty" indicator — the screen's existing
hard-block (`hints: ["/reconcile/status", "/reconcile/mismatches"], missingMode: "all"`) and its
account-wide halt semantics (§3.4, Q7) are unchanged; this table answers "which symbol caused
the halt," not "which symbol is halted" (there is no such thing).

### 8.7 `AlertsScreen`

New `Symbol` column on the alerts table, from `AlertsActiveResponse` rows' new `symbol: Option<String>`
field (§7.6). Account-wide alerts (day-limit, gap-escalation, kill-switch) render this column as
an explicit `"account-wide"` label — **not** blank/dash — to make the
unavailable/empty/present distinction visible to the operator (a blank cell could be
misread as "symbol unknown" rather than "this alert is intentionally not symbol-scoped"). Gated
by the existing `alerts` panel (`hints: ["/alerts/active"]`).

### 8.8 `OpsScreen`

No new data fields. Existing halt/kill-switch controls gain a tooltip/help-text clarification:
"Halt and kill-switch apply to the entire account across all configured symbols — there is no
per-symbol halt" (caps #10/#11, Q6-Q8). This is a documentation/UX clarification only, addressing
a likely operator misconception once the GUI shows multiple symbols, not a functional change.
Gated by the existing `ops` panel (`hints: ["/system/status"]`).

---

## 9. `MULTI-SYMBOL-PAPER-SMOKE-RUNNER-01` Design

Extends the existing `Start-PaperTradingSmoke.ps1` / `Capture-PaperSmokeEvidence.ps1` /
`Review-PaperSmokeEvidence.ps1` pattern (`scripts/windows/`). No new script architecture — this
is additive capability within the existing three-script pattern plus
`Send-PaperSmokeReviewDiscordAlert.ps1`.

### 9.1 Pre-flight

Requires a `watchlist-v2` artifact (§4.2) with `symbols.len() > 1`,
`approved_for_autonomous_paper=true`, `approved_for_live=false` (existing hard lock, unchanged).
Reuses `evaluate_watchlist_intake` (extended for v2) via `GET /api/v1/watchlist/status` —
`Start-PaperTradingSmoke.ps1` refuses to start if `schema_version != "watchlist-v2"` or
`symbols.len() <= 1` (this is *the multi-symbol smoke runner*; a single-symbol artifact should
use the existing single-symbol smoke path unchanged).

### 9.2 Capture

`Capture-PaperSmokeEvidence.ps1` additionally calls
`GET /api/v1/strategy/multi-symbol-dispatch-summary` (§7.3) at each existing snapshot interval,
writing `multi_symbol_dispatch_summary.json` per `MultiSymbolEvidenceSnapshot` (§4.9) alongside
the existing evidence files in the run's evidence directory.

### 9.3 Stop conditions

| # | Condition | Source | Action | Scope |
|---|---|---|---|---|
| 1 | Account-wide halt (`system/status.halted=true`) | existing | stop | account-wide, unchanged |
| 2 | Any reconcile mismatch (`reconcile/status` dirty) | existing | stop | account-wide, unchanged (Q7) |
| 3 | `day_signal_limit` reached (account-wide) | existing | stop | account-wide, unchanged |
| 4 | `max_new_orders_per_tick` (cap #6) hit on >50% of ticks in a rolling window | **new** | **warn, continue** | advisory — signals caps may be too tight for this strategy's signal rate; operator decides |
| 5 | `per_symbol_bar_staleness_secs` (cap #9) fired for >50% of configured symbols simultaneously | **new** | **stop** | hard stop — broader market-data feed problem, not a single-symbol issue |
| 6 | Manual operator stop | existing | stop | unchanged |

Stop condition #4 is the only "soft" one — it is a *configuration* signal (caps too tight), not
a *safety* signal, so it does not halt the run; conditions #1, #2, #3, #5, #6 are hard stops.

### 9.4 Review

`Review-PaperSmokeEvidence.ps1` gains a per-symbol breakdown section in its output, sourced from
the captured `multi_symbol_dispatch_summary.json` snapshots over the run — one row per symbol per
snapshot, showing `delta`/`no_order_reason`/`last_decision_disposition` over time. This is an
additive section on top of the existing `review-v2` schema (per `EVIDENCE-CAPTURE-TRADE-FLOW-01`)
— exact versioning (`review-v2` field addition vs. `review-v3`) is an implementation-time
decision for the patch that builds this (Patch 11, §10).

---

## 10. Implementation Patch Sequence (Dependency-Ordered)

Each patch below is sized for the one-patch-per-turn rule, scoped to a single file/seam, and
proven by its own scenario tests. Patch 1 (`WATCHLIST-V2-SCHEMA-01`) has been implemented as a
schema/validation layer in `watchlist_intake.rs` (see §4.2) — it introduces no runtime
multi-symbol dispatch. Patch 2 (`MULTI-SYMBOL-RUNTIME-CONFIG-01`) has also been implemented, as a
pure config-construction layer in `multi_symbol_config.rs` (see §4.1, §4.3) — registered in
`state.rs` but not invoked, and `loop_runner.rs`/`routes/strategy.rs` are untouched. Patches 3-11
remain `OPEN`; none have been started. The dependency graph determines minimum ordering; patches
with no dependency on each other within a "tier" may be reordered relative to each other but not
across tiers.

| # | Patch ID | Depends on | Closes |
|---|---|---|---|
| 1 | `WATCHLIST-V2-SCHEMA-01` | — | §4.2 (`watchlist-v2` schema in `watchlist_intake.rs`, `MULTI_SYMBOL_HARD_CEILING` const, cap #12) |
| 2 | `MULTI-SYMBOL-RUNTIME-CONFIG-01` | 1 | §4.1, §4.3 (`MultiSymbolRuntimeConfig`, `SymbolStrategyAssignment`, `MultiSymbolConfigSource`, cap #1) |
| 3 | `PER-SYMBOL-BAR-WINDOW-01` | 2 | §4.4 (`StrategyBarInput.symbol`, keyed `pending_strategy_bar_inputs`, `tick_strategy_dispatch_for_symbol`, cap #9 staleness) |
| 4 | `MULTI-SYMBOL-DISPATCH-LOOP-01` | 2, 3 | §5 (per-symbol loop in `loop_runner.rs`, Q1/Q2/Q4/Q5/Q6/Q11 wiring) |
| 5 | `MULTI-SYMBOL-DAY-ORDER-CAP-01` | 4 | §6 cap #4 (Gate 1f, `day_signal_count_by_symbol`, `"symbol_day_limit_reached"`) |
| 6 | `MULTI-SYMBOL-CAPITAL-CAPS-01` | 4 | §6 caps #2/#3/#5 (clamp+alert, `position_sizing.rs` per-symbol/aggregate checks) |
| 7 | `MULTI-SYMBOL-TICK-ORDER-CAP-01` | 4 | §6 cap #6 (`max_new_orders_per_tick`, `"max_new_orders_per_tick_reached"`) |
| 8 | `PER-SYMBOL-TARGET-STATE-01` | 4 | §4.6 (`PerSymbolTargetState` in-memory map) |
| 9 | `MULTI-SYMBOL-DISPATCH-SUMMARY-01` | 5, 6, 7, 8 | §4.8, §7.3 (`MultiSymbolDispatchSummaryResponse`, new route) |
| 10 | `MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01` | 9 | §7.4-§7.7, §8.1-§8.8 (`OmsOverviewResponse.per_symbol_status`, `MetricsDashboardResponse.per_symbol_exposure`, `AlertsActiveResponse.symbol`, all 8 GUI screens) |
| 11 | `WATCHLIST-PROMO-V2-MULTI-SYMBOL-AND-SMOKE-01` | 1, 9, 10 | §4.2 promotion-side (`watchlist_promotion.py` v2 gate logic, caps #8/#13 missing-proof tests), §9 (`MULTI-SYMBOL-PAPER-SMOKE-RUNNER-01`) |

### Dependency notes

- **Patch 1** (implemented) is foundational — touches `watchlist_intake.rs` (adds
  `WATCHLIST_SCHEMA_VERSION_V2` and the `MULTI_SYMBOL_HARD_CEILING` const), plus additive
  surfaces in `api_types.rs` / `routes/watchlist.rs` (`schema_version` on
  `WatchlistStatusResponse`). No runtime/dispatch code changes. Lowest risk, smallest blast
  radius, good first patch.
- **Patch 2** depends on 1 because `MultiSymbolRuntimeConfig` is built from the v2 artifact
  schema; it can still ship with only `EnvSingleSymbolFallback` exercised in tests if patch 1's
  v2 path isn't yet integration-tested end-to-end — but the type and both source variants should
  exist together to avoid a half-built config object.
- **Patch 3** depends on 2 for `MultiSymbolRuntimeConfig.symbols` (the list of symbols to build
  `pending_strategy_bar_inputs` keys for) but does not depend on 4 — the keyed map and
  `tick_strategy_dispatch_for_symbol` can exist and be unit-tested before the loop calls them.
- **Patch 4** is the highest-risk patch — it changes `loop_runner.rs`'s B1C block, the most
  sensitive per-tick dispatch code. It should land with Q1-Q9 each individually covered by a
  scenario test (9 new tests minimum, one per question), per Phase 5.
- **Patches 5, 6, 7** are mutually independent (each adds an isolated gate/check inside the loop
  patch 4 establishes) and could be reordered among themselves — listed in this order because
  cap #4 (day-order) reuses the most existing infrastructure (mirrors account-wide
  `day_signal_count`) and is therefore lowest-risk of the three.
- **Patch 8** depends on 4 (needs per-symbol loop iterations to populate from) but is
  independent of 5/6/7 — it could land in parallel with them. Listed after 5-7 only because the
  dispatch-summary route (9) benefits from all of 5/6/7/8 being present so it doesn't need a
  second migration.
- **Patch 9** is the first new *route* — depends on 5, 6, 7, 8 all existing so the response shape
  is final on first ship (avoids a second additive-field patch to the same response type within
  the same feature).
- **Patch 10** is GUI-only (TypeScript/React) — depends on 9 for the data it renders, plus the
  smaller additive changes to `OmsOverviewResponse`/`MetricsDashboardResponse`/`AlertsActiveResponse`
  (§7.4-§7.6) which could technically be split out, but are bundled here because they're each
  small (one field) and all consumed by this patch's GUI work.
- **Patch 11** is last — it's the integration/validation patch. The promotion-side `v2` gate
  logic (§4.2) only depends on patch 1 (schema), but the smoke-runner (§9) needs the
  dispatch-summary route (9) and GUI (10) to be meaningful, so bundling them as one
  "everything is wired, prove it end-to-end" patch is intentional. This patch also carries the
  three missing-proof tests called out earlier (Q9's `approved_for_live` regression test, cap
  #8's multi-symbol B5 independence test, cap #13's PDT cross-symbol summation test) since they
  are integration-level proofs that only make sense once the full chain exists.

---

## 11. Validation Performed for This Patch

This patch is documentation-only — no Rust, Python, TypeScript, migration, or config file is
added, removed, or modified other than this new file. Validation is scoped accordingly:

1. `git status --porcelain=v1` confirmed, before writing, that the only repo change introduced
   by this session is the new `docs/design/native_multi_symbol_dispatch.md` file (plus
   pre-existing untracked evidence directories from earlier sessions, unrelated to this patch).
2. The guard scripts that actually exist in `scripts/guards/` —
   `check_unsafe_patterns.ps1`/`.sh`, `check_migration_governance.sh`,
   `check_ignored_load_bearing_proofs.sh`, `check_workspace_dep_inheritance.sh` — scan Rust
   source, migrations, and `Cargo.toml` dependency declarations. A new markdown file under
   `docs/design/` is outside all of their scopes; they are unaffected by this patch and continue
   to reflect whatever state they were in before this patch (no regression possible from a
   docs-only addition). No aggregator script named `run_all_script_guards.ps1` exists (§0) — none
   is invoked.
3. No scenario-test harness run is required or meaningful for a design document — per
   `audit_repo_truth_rules.md`, "Scenario test file presence alone is not closure" and "DONE
   means code committed, test committed, and tests passing in CI" apply to *implementation*
   patches (the 11 in §10), not to this design record itself.

---

## 12. Summary and Status Ledger

| Item | Status | Notes |
|---|---|---|
| This design document (`NATIVE-MULTI-SYMBOL-DISPATCH-DESIGN-01`) | **CLOSED** once committed | docs-only; see §11 |
| Component: `MultiSymbolRuntimeConfig` (§4.1) | **CLOSED (config-construction only)** | Patch 2 (`multi_symbol_config.rs`); not wired into `loop_runner.rs`/`state.rs` dispatch (Patches 3/4) |
| Component: `ApprovedPaperWatchlist` v2 (§4.2) | OPEN | delivered by Patch 1 (schema) + Patch 11 (promotion-side) |
| Component: `SymbolStrategyAssignment` (§4.3) | **CLOSED (no per-symbol timeframe override)** | Patch 2; `timeframe_overrides` deferred to Patch 3/4 |
| Component: `PerSymbolBarWindow` (§4.4) | OPEN | delivered by Patch 3 |
| Component: `PerSymbolStrategyDecision` seam (§4.5) | OPEN | no new types; call-site change in Patch 4 |
| Component: `PerSymbolTargetState` (§4.6) | OPEN | delivered by Patch 8 |
| Component: `MultiSymbolRiskCaps` (§4.7) | OPEN | delivered across Patches 5/6/7 |
| Component: `MultiSymbolDispatchSummary` (§4.8) | OPEN | delivered by Patch 9 |
| Component: `MultiSymbolEvidenceSnapshot` (§4.9) | OPEN | delivered by Patch 11 |
| Cap #1 `max_concurrent_symbols` | **CLOSED (construction-time only)** | Patch 2; `MultiSymbolRuntimeConfig.max_concurrent_symbols` validated against `MULTI_SYMBOL_HARD_CEILING` and `symbols.len()` at config-build time, not yet enforced at dispatch time (Patch 4) |
| Cap #2 `per_symbol_max_position_qty` | OPEN | Patch 6 |
| Cap #3 `per_symbol_max_notional_usd` | OPEN | Patch 6 — **honest gap:** unverifiable for market orders (B1C is market-only today) |
| Cap #4 `per_symbol_day_order_count_limit` (Gate 1f) | OPEN | Patch 5 |
| Cap #5 `aggregate_gross_exposure_cap_usd` | OPEN | Patch 6 |
| Cap #6 `max_new_orders_per_tick` | OPEN | Patch 7 |
| Cap #7 reconcile drift visibility | OPEN | Patch 9/10 — observability only, no halt-scope change |
| Cap #8 B5 short-sale guard, multi-symbol proof | OPEN | proof in Patch 11; enforcement already correct |
| Cap #9 `per_symbol_bar_staleness_secs` | OPEN | Patch 3 |
| Cap #10 deadman TTL (documentation-only) | **PARKED by design** | account-wide by construction; no patch will change this |
| Cap #11 kill-switch propagation (documentation-only) | **PARKED by design** | account-wide by construction; no patch will change this |
| Cap #12 `MULTI_SYMBOL_HARD_CEILING` | OPEN | Patch 1 |
| Cap #13 PDT cross-symbol summation, proof | OPEN | proof in Patch 11; enforcement already correct |
| Patches 1-11 (§10) | all OPEN | none started; this design patch adds zero production code |

### Honest gaps and discrepancies surfaced by this design

- **`GUI-EXECUTION-SCREEN-RENDER-GUARD-01`** does not exist as a named patch/symbol anywhere in
  the repo (grep, zero matches). This design maps the informal reference to the existing
  `truthState !== null` hard-block pattern in `ExecutionScreen.tsx` / `truthRendering.ts` and
  requires all new multi-symbol GUI surfaces to sit behind that existing pattern (§8.2).
- **`scripts/guards/run_all_script_guards.ps1`** does not exist (grep, zero matches). §11's
  validation runs the five guard scripts that do exist; no aggregator was invoked or assumed.
- **Cap #3 (`per_symbol_max_notional_usd`) is currently unverifiable in practice** because B1C
  decisions are always `order_type="market"` (§1.6), and the existing per-strategy notional cap
  in `position_sizing.rs` already has this same limitation (§3.7) — this is a pre-existing gap,
  not introduced by this design, but it means cap #3's enforcement code path (Patch 6) will be
  correct-but-dormant until limit-order support exists. This is stated explicitly so Patch 6's
  scenario tests must construct synthetic limit-order decisions to exercise the check, and so
  that "Patch 6 is CLOSED" does not get conflated with "cap #3 is enforced in production paper
  trading today."
- **Heterogeneous strategies-per-symbol** (different `strategy_id` per symbol in the same run)
  is explicitly out of scope (§0) — this design only supports one strategy assigned to up to
  `MULTI_SYMBOL_HARD_CEILING` symbols. Lifting the Tier A `StrategyHost` single-registration
  constraint is a separate, larger design.
- **Stateful strategy implementations** are not verified safe for multi-symbol sequential
  dispatch (Q1 caveat) — Patch 4 must not be used with a stateful strategy until that is
  separately verified.

