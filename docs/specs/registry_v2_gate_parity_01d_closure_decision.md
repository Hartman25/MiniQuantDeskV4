# REGISTRY-V2-GATE-PARITY-01D — Closure Decision

Patch ID: `REGISTRY-V2-GATE-PARITY-01D-CLOSURE-AND-ROADMAP-RECONCILE-01`

Decision-only. No code changed by this patch. Written after `01A` (current
gate audit), `01B` (pure `registry_v2_gate_asset_class` helper), and `01C`
(20 regression tests proving parity) to decide whether `ASSET-CORE-01H`'s
production-cutover prerequisite #3 is closed and to reconcile the roadmap
accordingly.

---

## 1. Is prerequisite #3 of `ASSET-CORE-01H` closed?

**Yes, for the re-verification scope it names.** Prerequisite #3 reads:

> Gate 0 and the broker-submit routing guard re-verified (or migrated) to
> read the same asset-class truth as `InstrumentRegistryV2`, with
> regression tests proving no previously-disabled asset class becomes
> reachable.

This bundle chose **re-verification**, not migration: neither gate was
modified. `01B` built a pure, fail-closed helper
(`registry_v2_gate_asset_class`) that classifies any
`InstrumentRegistryV2.asset_class` string against the exact same
allow-equity/reject-everything-else contract both gates already enforce.
`01C` then proved, with 12 regression tests
(`scenario_registry_v2_gate0_parity_01c.rs`) plus 8 more
(`scenario_registry_v2_routing_guard_parity_01c.rs`), that the helper's
decision matches each gate's actual, already-tested behavior for every
`CANONICAL_ASSET_CLASSES_V2` value and for malformed/unknown input. No
previously-disabled asset class became reachable — the opposite was
proven: the helper is at least as strict as both gates, and one class
(`"rate"`) has no way to even reach the routing guard at all (see §3).

## 2. What Gate 0 parity was proven?

`scenario_registry_v2_gate0_parity_01c.rs`, `GD-01`..`GD-04`, against the
live `POST /api/v1/strategy/signal` route
(`mqk-daemon::routes::strategy::validate_strategy_signal`):

- `GD-01`: every one of the six `CANONICAL_ASSET_CLASSES_V2` strings
  (`equity`, `option`, `future`, `crypto`, `forex`, `rate`) — the helper's
  Equity/NonEquity classification agrees with Gate 0's actual pass/reject
  decision for the identical string sent as `asset_class`.
- `GD-02`: malformed/unknown strings (`""`, `"stock"`, `"perpetual_swap"`,
  `"etf"`, `"futures"`, `"options"`) are rejected by both the helper
  (`Err`) and Gate 0 (400/`disposition: "rejected"`).
- `GD-03`: case/whitespace-normalized equity (`"EQUITY"`, `" equity "`)
  passes both sides.
- `GD-04`: exhaustive — no `CANONICAL_ASSET_CLASSES_V2` value other than
  `"equity"` passes Gate 0.

Gate 0 itself is unmodified — it still reads only
`StrategySignalRequest.asset_class: Option<String>` and never
`InstrumentRegistryV2`.

## 3. What broker-submit routing-guard parity was proven?

`scenario_registry_v2_routing_guard_parity_01c.rs`, `RG-01`..`RG-08`,
against the live `BrokerGateway::submit`
(`mqk-execution::gateway::enforce_gates`):

- `RG-01`..`RG-05`: `equity`/`crypto`/`future`/`option`/`forex` — the
  helper's classification matches the routing guard's actual
  allow/`AssetClassDisabled`-reject decision for the corresponding
  `mqk_schemas::AssetClass` variant, with a `PanicBroker` test double
  proving the broker adapter is never invoked for the four rejected
  classes.
- `RG-06`: `"rate"` has **no** `mqk_schemas::AssetClass` counterpart — the
  enum has exactly five variants (`Equity`/`Option`/`Future`/`Crypto`/
  `Forex`), so a `BrokerSubmitRequest` carrying a `"rate"`-derived asset
  class can never be *constructed* in the first place. This is a stronger
  guarantee than a runtime rejection: `"rate"` is unreachable through the
  routing guard's closed enum, not merely blocked by it.
- `RG-07`: exhaustive parity check confirming exactly 5 of the 6 canonical
  v2 classes have a schema counterpart (the `"rate"` gap is the sole,
  documented exception).
- `RG-08`: malformed/unknown v2 strings fail closed in the helper and have
  no schema counterpart either — they can never be encoded into a real
  `BrokerSubmitRequest.asset_class` at all.

The routing guard itself is unmodified — it still reads only
`BrokerSubmitRequest.asset_class: mqk_schemas::AssetClass` and never
`InstrumentRegistryV2`.

## 4. Did any production path start consuming v2?

**No.** `registry_v2_gate_asset_class`/`registry_v2_gate_allows_asset_class`
(`core-rs/crates/mqk-md/src/instrument_registry_v2.rs`) have exactly two
kinds of callers in the current repo: their own unit tests (`01B`, 12
tests) and the two new parity test files (`01C`, 20 tests). Neither Gate 0
nor the routing guard was edited to read them. `mqk-execution`'s new
`mqk-md` dependency is a `[dev-dependencies]`-only addition — `mqk-md`
still does not appear in `mqk-execution`'s `[dependencies]`, so the
production build graph is unchanged. `config/instruments/equities.json`
(v1) remains the only registry any trading/execution/risk/OMS/ingestion
path reads.

## 5. Were all non-equity v2 asset classes still rejected?

Yes, exhaustively, for both gates:

- Gate 0: `GD-04` proves every `CANONICAL_ASSET_CLASSES_V2` value except
  `"equity"` is rejected (400/`disposition: "rejected"`).
- Routing guard: `RG-02`..`RG-05` prove `crypto`/`future`/`option`/`forex`
  are each rejected with `GateRefusal::AssetClassDisabled` before any
  broker adapter call; `RG-06` proves `"rate"` cannot even be constructed
  as a request the guard would need to reject.

## 6. Was equity still allowed?

Yes: `GD-01`/`GD-03` (Gate 0, including case/whitespace variants) and
`RG-01` (routing guard, end-to-end through `EchoBroker`) both confirm
`"equity"` passes unchanged.

## 7. What remains before production cutover?

Prerequisite #3 (this bundle) is now closed. Two prerequisites from
`ASSET-CORE-01H` §5 remain open, unchanged by this bundle:

1. ~~`BACKTEST-MULTIPLIER-MARGIN-01` closed~~ — already satisfied.
2. ~~Symbol/`instrument_id` translation layer~~ — already satisfied
   (`REGISTRY-V2-TRANSLATION-01A`-`01D`).
3. ~~Gate 0 / broker-submit routing-guard parity re-verification~~ —
   **now satisfied by this bundle (`01A`-`01D`).**
4. At least one non-equity market-data provider live-network-verified
   end-to-end into `md_bars` — still open (requires a live network call,
   forbidden by this session's hard safety rules).
5. An explicit operator decision to enable `enabled=true` for a specific,
   named non-equity instrument — still open.

`REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` remains blocked on
prerequisites #4-#5 and is **not** recommended next.

## 8. What next patch is recommended?

**`REGISTRY-V2-LIVE-PROVIDER-PROOF-BOUNDARY-DECISION-01`** — a
decision/design patch (not code) that names the exact first non-equity
market-data provider and network-call boundary prerequisite #4 requires,
and the exact operator authorization needed before any live network call
is made. This is recommended over attempting #4 directly in this session,
since every hard safety rule in this bundle (and every prior session's)
forbids live network calls without that explicit boundary decision first.
Prerequisite #5 (explicit operator enablement) is **not** recommended
before #4 is settled — enabling a specific instrument before its data
source is live-network-proven would invert the checklist's own ordering.

---

## Closure decision

```text
Prerequisite #3 of ASSET-CORE-01H's production-cutover checklist is
CLOSED_LOCAL for Gate 0 / broker-submit routing-guard parity proof.
No production cutover occurred. Existing production gates
(mqk-daemon::routes::strategy::validate_strategy_signal Gate 0,
mqk-execution::gateway::BrokerGateway::enforce_gates routing guard) remain
unchanged. REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01 is still blocked
until a live-network-verified non-equity provider proof (#4) and an
explicit operator enablement decision (#5) are addressed.
```

**Recommended next patch:** `REGISTRY-V2-LIVE-PROVIDER-PROOF-BOUNDARY-DECISION-01`.
