# ASSET-CORE-01H — Instrument Registry V2 Consumption Boundary Decision

Patch ID: `ASSET-CORE-01H-INSTRUMENT-REGISTRY-V2-CONSUMPTION-BOUNDARY-DECISION-01`

Decision-only. No code, no wiring, no cutover. Written because
`ASSET-CORE-01F`'s completion audit
([asset_core_01f_instrument_registry_v2_completion_audit.md](asset_core_01f_instrument_registry_v2_completion_audit.md))
found `ASSET-CORE-01`'s foundation complete, with the only remaining gap
being that no production path consumes `InstrumentRegistryV2`. That gap
must not be silently left implicit — this doc names the boundary
explicitly so a future session does not treat "foundation complete" as
"production ready."

---

## 1. Is `ASSET-CORE-01` foundation-complete?

Yes. `01A`-`01E` (all `CLOSED_LOCAL`, committed) together deliver: an
exhaustive provider-asset-class mapping (`01A`), a schema/loader modeling
all seven asset categories — equity, ETF, crypto spot, future, option,
forex, rate/fixed income (`01B`, extended by `01E`), a validator CLI and
daemon status route (`01C`), and an operator-visible GUI/status surface
(`01D`). See `ASSET-CORE-01F`'s audit doc §2-§4 for the full evidence
table.

## 2. Is `InstrumentRegistryV2` production trading truth?

No. It is read only by read-only diagnostic surfaces (`mqk md
registry-v2-status`, the three `mqk-daemon` status routes, and the GUI
panels that render their output). None of those surfaces write to a
production file, gate an order, or feed any downstream execution
decision. `config/instruments/equities.json` (v1, `TrackedInstrument`) is
the only registry any trading path reads.

## 3. Which production paths still consume v1 registry or hardcoded assumptions?

| Path | What it reads today |
|---|---|
| Signal admission (Gate 0, `653730f6`) | `mqk_schemas::AssetClass` equity-only allowlist |
| Broker-submit routing guard (`MULTI-ASSET-ROUTING-GUARD-01`, `ff2ae59f`) | Same `AssetClass` enum, not `InstrumentRegistryV2` |
| Market-data ingestion (`mqk-md` provider factory) | `TrackedInstrument` (v1) from `equities.json`, keyed by bare symbol string |
| Backtest engine / portfolio accounting (`mqk-portfolio::accounting.rs`) | Whole-share, single-currency, multiplier-implicitly-1 semantics; no `ContractDefinitionV2` field read anywhere |
| Risk engine (`mqk-risk::engine.rs`) | Portfolio-level drawdown/daily-loss only; no per-instrument registry lookup at all, v1 or v2 |
| OMS / order lifecycle | No instrument-registry read in the lifecycle state machine itself |

All six read either v1 or nothing instrument-registry-shaped. Zero read
`InstrumentRegistryV2`.

## 4. Risks of a production cutover

- **Silent multiplier/currency errors.** `ContractDefinitionV2::Future`/
  `Option` carry `multiplier`/`tick_size_micros`, but `mqk-portfolio`'s
  fill math is `qty * price_delta` everywhere — a cutover that fed v2 data
  into today's accounting path without first landing
  `BACKTEST-MULTIPLIER-MARGIN-01` would silently mis-price any non-equity
  fill by the multiplier factor.
- **Symbol-format mismatch.** v1 `equities.json` and `md_bars` are keyed
  by bare ticker strings; v2's `instrument_id`
  (`"equity:US:AAPL"`)/pair-style `symbol` (`"BTC/USD"`) do not match
  existing lookups without an explicit translation layer.
- **Fail-closed regression.** Today's two asset-class gates (Gate 0,
  routing guard) key off `mqk_schemas::AssetClass`, not
  `InstrumentRegistryV2::asset_class`. A cutover must prove the v2 field
  and the gate's enum stay in lockstep, or a disabled-by-v1 asset class
  could become reachable through a v2 path the gates don't inspect.
- **No live non-equity provider.** Even with a correct v2-reading
  execution path, no market-data provider has been live-network-verified
  for any non-equity symbol beyond the existing read-only CoinLore/Kraken
  checks (`CRYPTO-DATA-01*`) — a cutover without this would trade on
  synthetic/fixture data.

## 5. What must be true before a production cutover patch?

1. `BACKTEST-MULTIPLIER-MARGIN-01` closed — multiplier-aware P&L exists
   and is tested, so a `Future`/`Option` fill cannot silently mis-price.
2. An explicit `instrument_id`/`symbol` translation or lookup path between
   `InstrumentRegistryV2` and every existing symbol-string-keyed table
   (`md_bars`, outbox rows, portfolio positions) — proven idempotent and
   collision-free against the current 88-row equity universe.
3. Gate 0 and the broker-submit routing guard re-verified (or migrated) to
   read the same asset-class truth as `InstrumentRegistryV2`, with
   regression tests proving no previously-disabled asset class becomes
   reachable.
4. At least one non-equity market-data provider live-network-verified
   (not just fixture/CSV-proven) end-to-end into `md_bars`.
5. An explicit operator decision to enable `enabled=true` for a specific,
   named non-equity instrument — never inferred from schema presence
   alone.
6. A scenario-test suite proving idempotency/restart-safety for the new
   read path, per `audit_repo_truth_rules.md`'s closure standard.

## 6. What is the next safe boundary-crossing patch?

`REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` — itself a decision/design
patch (not code) that names the exact first production reader (most
likely the ingestion/provider-factory layer, since it is the shallowest
consumer and already provider-symbol-aware) and the exact acceptance
criteria from §5 above that must be met before that patch is allowed to
touch code.

Independent of that boundary, `ASSET-CORE-05` (market-calendar/session
generalization, already `PARTIAL`) and `BACKTEST-MULTIPLIER-MARGIN-01`
(prerequisite #1 above) are safe to pursue now without crossing this
boundary.

## 7. Which files are forbidden until that patch?

Until `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` (or an equivalent
explicitly-scoped follow-up) is authorized:

```text
core-rs/crates/mqk-runtime/*
core-rs/crates/mqk-execution/*
core-rs/crates/mqk-risk/*
core-rs/crates/mqk-broker-alpaca/*
core-rs/crates/mqk-db/* (migrations)
core-rs/crates/mqk-portfolio/src/accounting.rs
config/instruments/equities.json (replacement/repointing)
```

No patch should make any of these read `InstrumentRegistryV2` as a
production input without that follow-up's explicit scope review.

## 8. Which tests must exist for that patch?

- Idempotent-write / restart-safety scenario tests for any new
  registry-v2-backed read path (per `audit_repo_truth_rules.md`).
- A multiplier-correctness regression test proving a `Future`/`Option`
  fill's P&L matches `qty * price_delta * multiplier`, not
  `qty * price_delta`.
- A gate-parity test proving Gate 0 and the routing guard reject the same
  set of asset classes whether keyed off `mqk_schemas::AssetClass` or
  `InstrumentRegistryV2::asset_class`.
- A symbol-translation round-trip test (v1 symbol -> v2 `instrument_id` ->
  back) with zero collisions across the full production instrument set.
- A live (or documented live-once, fixture-thereafter) non-equity
  provider-to-`md_bars` proof, mirroring the existing
  `CRYPTO-DATA-01A`/`01B` proof pattern.

---

## 9. Closure decision

**`ASSET-CORE-01` is `CLOSED_LOCAL / FOUNDATION-COMPLETE`.** Production
consumption remains a separate boundary-crossing patch. Do not mark
`ASSET-CORE-02`, `ASSET-CORE-04` (beyond its already-closed `04A`-`04F`
metadata-bridge slices), `CRYPTO-REGISTRY-01`, `CRYPTO-DATA-01`,
`CRYPTO-RISK-01`, `CRYPTO-EXEC-01`, or `CRYPTO-STRAT-01` closed on the
strength of this decision — none of them are touched by it.

**Recommended next patch:** `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01`.
