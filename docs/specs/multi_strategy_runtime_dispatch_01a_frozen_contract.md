# MULTI-STRATEGY-RUNTIME-DISPATCH-01 — frozen design contract

**Status:** FROZEN (operator-authorized, `V4-STAGE-B-M2-REPAIR-02`, R1A). Governs implementation
in R1B. Resolves the R1 `SPEC_DECISION_REQUIRED` recorded in
`docs/V4_CODE_COMPLETION_MANIFEST.md` § "Stage B M2 Repair Findings" (R1).

**Operator decision (frozen, do not reinterpret):** same-symbol economically-active strategies use
**explicit per-symbol authorization** — a new, versioned, additive artifact schema
(`watchlist-v3`) that lets an operator explicitly list more than one strategy identity per symbol.
This is a *separate, explicit* mechanism from Bundle 7's dynamic ranking selector
(`mqk-portfolio::dynamic_selection`), which remains frozen and unchanged (still "exactly one
selected candidate per symbol, or none — never more").

## 1. Why a new schema version, not a v2 reinterpretation

`watchlist-v2`'s `strategy_assignments` field is `symbol -> String` (one strategy id). Silently
reinterpreting the same field as `symbol -> Vec<String>` under the same `schema_version` string
would make every existing v2 artifact byte-ambiguous (is `"strategy_assignments": {"AAPL":
"intraday_scalper"}` a single string or a one-element list, to a consumer that expects a list?) and
would violate the frozen invariant that a `schema_version` string denotes one fixed JSON shape.
Per the operator's explicit instruction, this uses a new schema version instead:
**`watchlist-v3`**.

## 2. New schema: `watchlist-v3`

Backward compatible: v1 and v2 artifacts are parsed exactly as before, unchanged in every respect
(same validation order, same error codes, same `LoadedWatchlistArtifact` shape for those two
versions). v3 is additive — a new schema version value, a new validation branch, and a new,
separate loaded-artifact type. No existing test for v1/v2 changes behavior.

```json
{
  "schema_version": "watchlist-v3",
  "mode": "paper",
  "approved_for_live": false,
  "approved_for_autonomous_paper": true,
  "symbols": ["AAPL", "MSFT"],
  "strategy_assignments": {
    "AAPL": ["intraday_scalper", "intraday_short_scalper"],
    "MSFT": ["swing_momentum"]
  },
  "max_symbols_to_trade": 2,
  "max_concurrent_positions": 2
}
```

`strategy_assignments` in v3 is `symbol -> Vec<String>` (one or more strategy identities), never
empty for an admitted symbol. A singleton list (`["swing_momentum"]`) is the exact multi-strategy
generalization of a v2 single-assignment symbol — not a different case needing special-cased code.

### 2.1 New bound: `MAX_STRATEGIES_PER_SYMBOL`

A new hard ceiling, mirroring `MULTI_SYMBOL_HARD_CEILING`'s existing pattern (defense-in-depth,
bounds total isolated `StrategyHost` construction/dispatch cost per tick):

```rust
pub const MAX_STRATEGIES_PER_SYMBOL: u64 = 3;
```

`strategy_assignments[symbol].len()` must be in `1..=MAX_STRATEGIES_PER_SYMBOL` for every admitted
symbol — `0` is `watchlist_strategy_assignment_missing` (same error family as v2's missing-entry
case); `> MAX_STRATEGIES_PER_SYMBOL` is a new error, `watchlist_strategy_per_symbol_ceiling_exceeded`.

### 2.2 Validation contract (v3 branch, mirrors v2's steps exactly except where noted)

Same steps as v2 (schema_version recognition, mode=paper, live-lock, approved_for_autonomous_paper
bool, symbols array, max_symbols_to_trade/max_concurrent_positions bounds, cap #1
truncate-and-surface), with `strategy_assignments` parsed as `symbol -> Vec<String>` instead of
`symbol -> String`, and one new check (§2.1) per admitted symbol. Truncation (cap #1) behaves
identically: a symbol dropped to `dropped_symbols` is exempt from nothing — its (list-valued)
assignment must still be present and valid before truncation is applied, exactly mirroring v2's
existing "malformed dropped-tail symbol must still fail the whole artifact" rule.

### 2.3 New loaded-artifact type

```rust
pub struct LoadedWatchlistArtifactV3 {
    pub schema_version: String,       // always "watchlist-v3"
    pub symbols: Vec<String>,
    pub top_symbol: Option<String>,
    pub strategy_assignments: HashMap<String, Vec<String>>,
    pub max_symbols_to_trade: u64,
    pub max_concurrent_positions: u64,
    pub approved_for_autonomous_paper: bool,
    pub dropped_symbols: Vec<String>,
}
```

A separate type, not a widened `LoadedWatchlistArtifact` — v1/v2 callers that only understand
`symbol -> String` must not be handed a `Vec` they don't expect; the type system enforces this
rather than a runtime check. `WatchlistIntakeOutcome` gains two new variants
(`LoadedApprovedV3`/`LoadedNotApprovedV3`) alongside the existing five; existing variants and their
exhaustive-match call sites are unchanged in meaning, only in enum arity (a new match arm is
additive — every existing arm's existing behavior is preserved verbatim; this is a source-breaking,
not behavior-breaking, extension of the enum, and every existing call site must be updated to
handle the new arms explicitly, never via a wildcard that would silently misclassify a v3 outcome
as V1/v2 absence).

## 3. Identity tuple

Every economically-active binding this schema authorizes is identified by the same triple Bundle 7
already uses: **`(symbol, strategy_id, timeframe_secs)`**. v3 does not introduce per-symbol
per-strategy timeframe overrides (Tier A precedent: timeframe stays global per artifact, same as
v2) — every binding a v3 artifact authorizes shares the artifact's one `default_timeframe`/
`MQK_STRATEGY_MD_TIMEFRAME`-equivalent value, exactly as v2's `SymbolStrategyAssignment` already
does. A future patch may add per-binding timeframe if a real requirement emerges; this patch does
not invent it speculatively (`CLAUDE.md` §12).

## 4. Promotion / readiness requirements per binding

Every `(symbol, strategy_id, timeframe_secs)` binding a v3 artifact authorizes must **independently**
pass, before it may dispatch:

1. durable registry enabled truth (`sys_strategy_registry.enabled = true` for `strategy_id`);
2. exact `active_paper` promotion authority for the exact triple (`promotion_evidence_validation` /
   `evaluate_paper_promotion_gate`'s existing Gate 3b path — unchanged, reused as-is);
3. promotion/config semantic identity verification (`config_identity_verified`, existing Bundle 7
   contract, unchanged);
4. strategy/timeframe compatibility (`StrategySpec.timeframe_secs == timeframe_secs`);
5. data readiness for `(symbol, timeframe)` (existing Bundle 2 readiness gate, unchanged);
6. existing risk/capital gates (`per_symbol_max_position_qty` and friends — **account/symbol**
   authority, see §6).

A promoted sibling never authorizes an unpromoted one: each binding's promotion check is
independent, keyed by its own exact `(symbol, strategy_id, timeframe_secs)` — there is no
"symbol-level" promotion shortcut. This mirrors Bundle 7's existing per-candidate promotion check
exactly (`dynamic_selection_plan_builder.rs:329`), extended to the explicit-authorization path.

## 5. Deterministic ordering

Within one symbol, authorized strategies are processed in the artifact's own list order (never
`HashMap` iteration order — the parsed `Vec<String>` preserves JSON array order). Across symbols,
the existing `MultiSymbolRuntimeConfig.symbols` ordering (artifact order) is preserved. The
resulting flattened `(symbol, strategy_id, timeframe_secs)` binding set fed to
`DynamicSelectionHostPool` is sorted into the pool's existing `BTreeMap<HostPoolKey, StrategyHost>`
key order (`(symbol, strategy_id, timeframe_secs)` lexicographic) — identical determinism guarantee
Bundle 7 already provides today (`input_order_does_not_change_the_resulting_pool_keys`), unchanged
by this extension.

## 6. Relationship to Bundle 6 and Bundle 5

- **One isolated `StrategyHost` per `(symbol, strategy_id, timeframe_secs)` binding**, exactly the
  existing `DynamicSelectionHostPool` pattern — reused as-is, not reimplemented.
  `StrategyHost::register`'s `MultiStrategyNotAllowed` invariant is **not weakened**: it continues
  to mean "one strategy per host instance," and multiple hosts (one per binding) is how multiple
  strategies coexist, exactly as Bundle 7 already does for multiple *symbols*.
- Every binding's genuine, independently-produced proposal (from its own isolated host's `on_bar`)
  is gathered into the same per-tick `all_decisions` vector every other binding's proposal already
  reaches — **before** Bundle 6. No new conflict resolver. Bundle 6
  (`runtime_strategy_conflict::apply_conflict_policy`) remains the **one** conflict authority; two
  same-symbol proposals from two authorized strategies are exactly the "genuine multiple candidates"
  input Bundle 6 was already built and tested to resolve correctly (see
  `docs/specs/multi_strategy_conflict_policy_01a_current_truth_and_contract.md` Q4 — this is the
  production step that document explicitly deferred, now supplied).
- Bundle 5 (`runtime_opportunity_allocation::gather_and_apply`) runs unchanged, immediately after
  Bundle 6, over whatever Bundle 6 leaves per symbol (at most one survivor).

## 7. Per-symbol position/capital caps remain account/symbol authority

`MULTI-SYMBOL-CAPITAL-CAPS-01`'s `per_symbol_max_position_qty` (and any future per-symbol capital
cap) applies to the **symbol's** net position, computed from the account's actual held quantity —
never a per-strategy sub-budget. Strategies propose targets against the shared account/symbol
state; they do not receive independent, hidden, per-strategy capital or position ownership. This
patch introduces no per-strategy capital carve-out — that would be a new, separate economic policy
this contract does not authorize (consistent with the R1 operator decision's explicit text:
"Strategies propose targets; they do not receive independent hidden account ownership").

## 8. No dry-run economic authority; no Live authority change

- `MQK_DRY_RUN_STRATEGY_IDS` identities remain structurally incapable of economic submission
  (`state/dry_run_strategy.rs`, unchanged) — a strategy id appearing in a v3
  `strategy_assignments` list does **not** by itself grant economic authority; it still must pass
  every check in §4, and dry-run identities are never fed into `DynamicSelectionHostPool`
  construction from the v3 path (mirrors the existing exclusion for the v2/legacy paths).
- `approved_for_live` remains hard-locked `false` for every v3 artifact, identically to v1/v2 (§2.2
  — the live-lock check is schema-version-independent and unchanged). No new Live authority is
  introduced anywhere by this contract.

## 9. What R1B must implement (scope boundary for this contract)

- `watchlist_intake.rs`: `WATCHLIST_SCHEMA_VERSION_V3`, `MAX_STRATEGIES_PER_SYMBOL`,
  `LoadedWatchlistArtifactV3`, two new `WatchlistIntakeOutcome` variants, v3 validation branch.
- `state/multi_symbol_config.rs`: a v3-aware config source
  (`MultiSymbolConfigSource::WatchlistArtifactV3`) producing a flattened
  `Vec<(symbol, strategy_id, timeframe_secs)>` binding set (not the existing single-strategy-per-
  symbol `SymbolStrategyAssignment` shape, which cannot represent >1 strategy per symbol) —
  additive, v1/v2 paths byte-for-byte unchanged.
- A v3-aware `RuntimeStrategyDispatchAuthority` construction path that builds a
  `DynamicSelectionHostPool` from the flattened v3 binding set (reusing the pool's existing `build`
  function and `HostPoolKey` type verbatim), independent of Bundle 7's ranking selector.
- Wiring into `state/loop_runner.rs`'s existing dispatch-authority `match` (§6 above) — genuinely
  same-symbol multi-strategy `on_bar` evaluation, decisions gathered pre-Bundle-6, unchanged
  Bundle 6/5/promotion/risk/decision/outbox path downstream.
- The ten focused proofs enumerated in the R1B mission text.

Not in scope for R1B: GUI surfaces, new API routes, Python scanner/promotion-pipeline changes to
*emit* v3 artifacts (operators may hand-author v3 artifacts for this patch's proof; producing them
from the scanner pipeline is a separate, later patch if the operator wants it).
