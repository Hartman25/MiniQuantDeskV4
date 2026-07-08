# REGISTRY-V2-INSTRUMENT-ENABLEMENT-01-BTC-USD-DECISION-01

Patch ID: `REGISTRY-V2-INSTRUMENT-ENABLEMENT-01-BTC-USD-DECISION-01`

Decision-only review. No code, no config change, no network call, no DB
access, no trading enablement, no order routing, no broker/risk/runtime
change. Written after the operator explicitly authorized:

> I explicitly authorize a decision-only review for enabling BTC/USD as
> the first named non-equity instrument. Do not enable trading, do not
> route orders, do not change broker/risk/runtime behavior, and stop
> after the enablement decision evidence.

This addresses `ASSET-CORE-01H` §5 prerequisite #5:

> An explicit operator decision to enable `enabled=true` for a specific,
> named non-equity instrument — never inferred from schema presence
> alone.

---

## 1. Current state of BTC/USD

`config/instruments/instruments_v2.crypto_local_marks.example.json`,
instrument `crypto:GLOBAL:BTCUSD` / symbol `BTC/USD`:

```json
"enabled": false,
"paper_trading_enabled": false,
"live_trading_enabled": false
```

No `allow_enabled_non_equity_for_testing` field is present (defaults to
`false`). This has not changed since `REGISTRY-V2-KRAKEN-LIVE-PROVIDER-PROOF-01`
proved a live Kraken OHLC network call reaches `md_bars` for this symbol
(720 real completed bars, in the isolated `mqk_test` database only —
`docs/specs/registry_v2_kraken_live_provider_proof_01_closure_decision.md`).

## 2. What `enabled=true` does and does not gate today

Direct source read of `core-rs/crates/mqk-md/src/instrument_registry_v2.rs`
(module doc comment, lines 1-15):

> This module is a **model + loader seam, not a production cutover**...
> Nothing in this module is wired into any consumer... there is no
> production enablement path through this schema.

Confirmed, independently, by `ASSET-CORE-01H` §3's evidence table (still
accurate — unchanged by any patch in this session): Gate 0, the
broker-submit routing guard, market-data ingestion's provider factory,
the backtest engine/portfolio accounting, the risk engine, and the OMS
lifecycle state machine all read either `mqk_schemas::AssetClass` or
nothing instrument-registry-shaped — **zero** read `InstrumentRegistryV2`.

Concretely, setting `BTC/USD.enabled = true` today would:

- **Not** change what Gate 0 (`mqk-daemon::routes::strategy::validate_strategy_signal`)
  accepts or rejects — it reads only `StrategySignalRequest.asset_class: Option<String>`.
- **Not** change what the broker-submit routing guard
  (`mqk-execution::gateway::BrokerGateway::enforce_gates`) accepts or
  rejects — it reads only `BrokerSubmitRequest.asset_class: mqk_schemas::AssetClass`,
  a five-variant enum with no `Crypto`-via-v2 path and no way to construct
  a request from `InstrumentRegistryV2` at all today.
- **Not** change market-data ingestion, the backtest engine, portfolio
  accounting, or the risk engine — none of them load or read
  `InstrumentRegistryV2`.
- **Not** enable any order to be routed, any broker call to be made, or
  any runtime/strategy behavior to change.
- **Only** become visible in read-only diagnostic surfaces: `mqk md
  registry-v2-status`/`registry-v2-translation-check`/`crypto-registry-readiness`
  CLI output, the corresponding `mqk-daemon` status routes, and the GUI
  panels that render them (`enabled_count` in the summary; per-instrument
  `enabled` field in per-symbol output).

## 3. A schema constraint this review surfaced

`validate_registry_v2` (`core-rs/crates/mqk-md/src/instrument_registry_v2.rs:328-337`)
fail-closed **rejects loading the entire registry file** if any non-equity
instrument has `enabled=true` without also having
`allow_enabled_non_equity_for_testing=true`:

```rust
if inst.enabled
    && inst.asset_class != "equity"
    && !inst.allow_enabled_non_equity_for_testing
{
    anyhow::bail!(
        "instrument_registry_v2: enabled non-equity instrument symbol={} asset_class={} requires allow_enabled_non_equity_for_testing=true (test/fixture only; no production path reads this schema)",
        inst.symbol, inst.asset_class
    );
}
```

This means there is **no way today** to set `BTC/USD.enabled=true` and
have the registry file still load, other than also setting
`allow_enabled_non_equity_for_testing=true` — a flag whose own name and
bail-message wording (`"test/fixture only; no production path reads this
schema"`) exist specifically to prevent this flag combination from being
mistaken for production enablement. Separately, the same file's
`paper_trading_enabled`/`live_trading_enabled` fields are read by
`mqk-cli`/`mqk-daemon` read-only surfaces and fail closed with
`truth_state="unsafe_trading_enabled"` if either is ever `true`
(`core-rs/crates/mqk-cli/src/commands/md.rs:1828-1832`,
`core-rs/crates/mqk-daemon/src/routes/transport_quality.rs:1677-1682`).

This is new information relevant to the operator's decision: literally
flipping `enabled=true` on `BTC/USD` today requires pairing it with a
flag the schema's own author explicitly documented as non-production, and
would still change zero trading/execution/risk/broker/OMS behavior either
way, since nothing production-facing reads this schema yet.

## 4. Decision

**BTC/USD is named as the first candidate non-equity instrument for
eventual `enabled=true` status**, consistent with:

- `REGISTRY-V2-LIVE-PROVIDER-01A`'s audit selecting Kraken/`BTC/USD`/`ETH/USD`
  as the safest, most complete non-equity data lane.
- `REGISTRY-V2-KRAKEN-LIVE-PROVIDER-PROOF-01`'s live-network proof that
  real `BTC/USD` bars reach `md_bars` end-to-end.
- `BTC/USD` being the more liquid/higher-priority of the two proven
  symbols (operator naming, this decision).

**This decision does not, by itself, flip `BTC/USD.enabled` in
`config/instruments/instruments_v2.crypto_local_marks.example.json`.**
Per the operator's explicit instruction, this patch stops after recording
the enablement decision as evidence — it does not implement it. Given §3's
finding, actually flipping the flag would require either (a) pairing it
with the `allow_enabled_non_equity_for_testing` escape hatch, which the
schema's own documentation states is not a production path, or (b)
waiting until a real production consumption path exists
(`REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` and its follow-on work,
still not started). Implementing the flag flip is deliberately treated as
a separate, distinct, future action requiring its own explicit
authorization — not bundled into this decision-only review.

## 5. What remains false/unchanged

- `config/instruments/instruments_v2.crypto_local_marks.example.json` —
  byte-identical to its pre-review state; `BTC/USD.enabled`,
  `paper_trading_enabled`, and `live_trading_enabled` all remain `false`;
  `allow_enabled_non_equity_for_testing` remains absent/`false`.
- `config/providers/providers.json`'s `kraken.enabled` — unchanged,
  still `false`.
- No order was routed, no broker call was made, no risk/runtime/strategy
  behavior changed.
- No trading was enabled, paper or live.
- No network call, no DB access.

## 6. What "closes" and what remains open

`ASSET-CORE-01H` §5 prerequisite #5 asks for "an explicit operator
decision to enable `enabled=true` for a specific, named non-equity
instrument — never inferred from schema presence alone." That decision
now exists and is recorded here: **BTC/USD**, explicitly named by the
operator, not inferred. The *decision* is made. The *implementation* (the
actual `enabled=true`/`allow_enabled_non_equity_for_testing=true` config
change) is a distinct, separate action this patch deliberately does not
take, per the operator's "stop after the enablement decision evidence"
instruction.

`REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` itself remains not written.
All five `ASSET-CORE-01H` §5 prerequisites now have either a closed
implementation (#1-#4) or an explicit operator decision (#5) — but #5's
decision has not yet been *implemented* as a config change, and no
production code path reads `InstrumentRegistryV2` as trading truth yet
regardless. A production-cutover decision patch would still need to
address that gap; it is not automatically unblocked by this decision
alone.

## 7. What was deliberately not done

- `BTC/USD.enabled` was not set to `true`.
- `allow_enabled_non_equity_for_testing` was not set on any instrument.
- No trading (paper or live) was enabled.
- No order was routed.
- No broker/risk/runtime/strategy/OMS/portfolio code was touched.
- No network call, no DB access.
- `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` was not written.

---

## Decision record

```text
Operator decision (ASSET-CORE-01H prerequisite #5):
BTC/USD is named as the first non-equity instrument for eventual
enabled=true status.

Implementation status: NOT YET IMPLEMENTED. config/instruments/
instruments_v2.crypto_local_marks.example.json remains unchanged
(enabled=false, paper_trading_enabled=false, live_trading_enabled=false
for both BTC/USD and ETH/USD). Flipping enabled=true requires either the
test-only allow_enabled_non_equity_for_testing escape hatch (schema's own
documented non-production path) or a real production consumption path
(not yet built). Either requires a separate, explicit follow-up
authorization.

No trading enabled. No orders routed. No broker/risk/runtime behavior
changed. No network call. No DB access.
```

**Recommended next step:** if the operator wants the config flag actually
flipped, a separate, explicit authorization naming exactly which flags to
set (`enabled`, and necessarily `allow_enabled_non_equity_for_testing`
given §3's finding) is needed — distinct from this decision-only review.
Absent that, no further action is recommended on this thread.
