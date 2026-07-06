# CRYPTO-REGISTRY-02 — Kraken Data Registry Cutover Decision

Patch ID: `CRYPTO-REGISTRY-02-KRAKEN-DATA-REGISTRY-CUTOVER-DECISION-01`

This is a decision/spec patch. It is **not** a registry-v2 config change, **not**
a scheduler, **not** a daemon ingest job, **not** a broker/execution/risk/OMS/
runtime change, **not** a DB migration, **not** a network call, **not** crypto
trading enablement. It decides what "production registry-v2 cutover" means for
Kraken-sourced crypto OHLCV data (`BTC/USD`, `ETH/USD`) after the fully proven
fixture/DB/sync/status/GUI lane closed by:

```text
5762cf88 md: prove Kraken provider ingest path
9cd2093a md: add Kraken incremental sync proof
2f5d1840 md: add Kraken content-diff sync
8962a649 daemon: expose Kraken OHLC evidence status
82d14e31 gui: show Kraken OHLC sync status
```

Decided at HEAD `82d14e31`.

---

## 1. Executive Decision

**Do not flip any registry or provider enablement flag in this patch.**
`config/providers/providers.json`'s `kraken` entry stays `enabled: false`.
`config/instruments/instruments_v2.crypto_local_marks.example.json`'s `BTC/USD`
and `ETH/USD` rows stay `enabled: false`, `paper_trading_enabled: false`,
`live_trading_enabled: false`, unchanged.

Instead, this decision defines **data readiness** as a concept that is
independent of the `enabled` field and that a read-only CLI/validator surface
(`CRYPTO-REGISTRY-03`, next) can prove without touching any config flag:
Kraken is **data-path-ready** (parser, fixture ingest, incremental sync,
content-diff sync, evidence status route, GUI panel all proven end-to-end),
but **not production-default**, **not scheduled**, and **not tradable**. A
future "registry-v2 cutover" — if ever pursued — is scoped narrowly to
*flipping `enabled=true` so the row is tracked/visible by default*, and even
that is explicitly gated behind invariants this decision records (§7) but does
not satisfy. Trading enablement (`paper_trading_enabled`/`live_trading_enabled`)
is a separate, later decision with its own, stricter invariant list (§8) that
this patch does not begin to satisfy either.

---

## 2. Current Repo Facts

Grounded by direct file reads at HEAD `82d14e31`, answering the mission's nine
pre-flight questions in order.

### Q1 — What does `enabled` mean in the current registry-v2 crypto fixture?

`core-rs/crates/mqk-md/src/instrument_registry_v2.rs::InstrumentDefinitionV2.enabled`
is documented in the struct itself: *"Whether this instrument is tracked at
all (ingestion/backtest/GUI scope)."* It is a **tracking/visibility** flag, not
a trading flag. The struct also carries `paper_trading_enabled` and
`live_trading_enabled` as separate fields, each doc-commented *"Independent of
`enabled`."*

### Q2 — Is there any code path today that treats `enabled=true` as tradable?

No. The module's own header comment states plainly: *"Nothing in this module
is wired into any consumer. `InstrumentRegistryV2` is parsed and validated
only by the tests in this file"* (module docs, lines 1–15) — with one narrow,
already-audited exception: the `ASSET-CORE-04F` daemon route
(`?registry_source=v2` on `/api/v1/portfolio/economics/status`) reads this
schema to compute a **read-only economics/valuation summary**, not to route,
gate, or authorize any order. No broker, risk, OMS, or runtime code reads
`InstrumentRegistryV2`, `enabled`, `paper_trading_enabled`, or
`live_trading_enabled` at all. `enabled=true` triggers zero trading side
effect anywhere in the current repo.

### Q3 — Are `paper_trading_enabled` and `live_trading_enabled` parsed anywhere today?

Yes, as plain `bool` struct fields (`#[serde(default)]`, so absent ⇒ `false`).
They are also counted, read-only, by
`summarize_instrument_registry_v2_status` (`paper_trading_enabled_count`,
`live_trading_enabled_count`) for the `ASSET-CORE-01D` registry-v2-source
status route. No code path branches on their value to permit or block an
order — there is no order path that reads this schema at all (Q2).

### Q4 — Can BTC/USD and ETH/USD become data-enabled while still non-tradable, or does the schema lack that distinction?

The schema **does** carry the distinction: `enabled`,
`paper_trading_enabled`, and `live_trading_enabled` are three independent
boolean fields, and nothing downstream conflates them (Q2, Q3). However, the
schema's own validator, `validate_registry_v2`, fail-closed **blocks** setting
`enabled=true` on any non-equity instrument (crypto included) unless the
test/fixture-only escape hatch `allow_enabled_non_equity_for_testing=true` is
also set — and that flag's own doc comment says outright: *"nothing in any
production daemon, CLI, ingestion, backtest, or GUI path reads
`InstrumentRegistryV2` at all, so this flag has no trading effect. It exists
solely so a test fixture can prove the fail-closed rule is an explicit,
deliberate gate rather than an accidental omission."* In practice: the
independent-boolean distinction exists in the *shape* of the schema, but there
is currently no *production-sanctioned* way to set `enabled=true` for BTC/USD
or ETH/USD without also setting a flag documented as test-only. This patch
does not introduce that production-sanctioned path; §5–§6 record why.

### Q5 — Should production data-only readiness use a separate candidate file, a docs/spec artifact, or the existing disabled fixture?

**The existing disabled fixture, read as-is, with no field changed.** Q4
establishes that flipping `enabled=true` on the current fixture requires the
test-only `allow_enabled_non_equity_for_testing` flag — an unsafe,
mislabeling move for a production-facing readiness surface. A brand-new
candidate registry file would duplicate the existing `provider_symbols`
aliases and risk drift between two copies of the same BTC/USD/ETH/USD
metadata. Instead, `CRYPTO-REGISTRY-03`'s readiness CLI reads the current,
unmodified fixture and providers config, and classifies readiness
(`data_ready_manual_only`, not `production_default`) from the *values already
present* — `enabled=false` is treated as an expected, correct state for this
phase, not a failure.

### Q6 — Should the existing disabled fixture remain unchanged?

Yes. See §1 and Q5. No field in
`config/instruments/instruments_v2.crypto_local_marks.example.json` or
`config/providers/providers.json` changes in this patch or in the
`CRYPTO-REGISTRY-03`/`04` patches planned after it.

### Q7 — What exact invariants must a registry candidate satisfy before recurring Kraken sync could be considered later?

Recorded in §7 below; none are satisfied by this patch or claimed to be.

### Q8 — What exact invariants must a registry candidate satisfy before crypto risk/execution/strategy could be considered later?

Recorded in §8 below; none are satisfied by this patch or claimed to be.

### Q9 — Which exact files are sufficient for each phase?

- **Phase A (this patch):** this decision doc, its JSON artifact, its
  validator script, plus ledger/audit/runbook updates. No source code.
- **Phase B (`CRYPTO-REGISTRY-03`, next):** one new CLI subcommand in
  `mqk-cli` reading the existing, unmodified `providers.json` and
  `instruments_v2.crypto_local_marks.example.json` files — no config file
  edited.
- **Phase C (`CRYPTO-REGISTRY-04`, conditional):** a read-only daemon route
  plus GUI panel reusing the same pure readiness-check logic, if Phase B
  closes cleanly and the addition stays small.

---

## 3. What "Production Registry-v2 Cutover" Means Here

For this decision, "registry-v2 cutover" is narrowly scoped to: **the moment
an operator would be safe to flip `BTC/USD`/`ETH/USD`'s `enabled` field to
`true` in a real (non-test, non-fixture) registry-v2 document, so the
instrument is tracked/visible by default in registry-v2-consuming surfaces
(currently only the `ASSET-CORE-04F` economics-status bridge).** It explicitly
does **not** mean:

- Crypto becoming tradable (paper or live) — that is a separate, later
  decision (§8), unrelated to `enabled`.
- Any scheduler or recurring sync existing — that is a separate, later
  decision (§7).
- Any daemon background job — none exists and none is added here.
- Any change to which registry (v1 vs v2) any production execution path
  reads — v1 (`config/instruments/equities.json`, equities-only) remains the
  sole trading-authoritative registry, unchanged.

This decision concludes crypto data **can** be "data-ready" (§4) without being
"trading-enabled" — the schema supports the distinction (Q4) — but that the
*safe, non-test path* to exercise that distinction for `enabled` specifically
does not exist yet in this schema/validator, and building it is out of scope
here.

---

## 4. Data-Ready vs. Trading-Enabled

| Concept | Field(s) | Current BTC/USD & ETH/USD state | This patch's classification |
|---|---|---|---|
| Data-path proven | N/A (proven by commits `5762cf88`→`82d14e31`) | Parser, fixture ingest, incremental sync, content-diff sync, evidence status route, GUI panel all closed and tested | `kraken_fixture_db_sync_status_gui_proven` |
| Tracked/visible by default | `enabled` | `false` (both rows) | `data_ready_manual_only` (correct disabled state; not a failure) |
| Paper-tradable | `paper_trading_enabled` | `false` (both rows) | `disabled` |
| Live-tradable | `live_trading_enabled` | `false` (both rows) | `disabled` |
| Recurring/scheduled sync | N/A — no scheduler exists | absent | `absent` |
| Registry-v2 cutover (this decision's scope, §3) | `enabled` | not flipped by this patch | `decision_only` |

`data_ready_manual_only` means: an operator can run the existing explicit CLI
commands (`mqk md kraken-ohlc-dry-run` / `kraken-ohlc-ingest` /
`kraken-ohlc-sync`) against the current fixture today, on demand, and inspect
the results via the evidence status route/GUI panel — but nothing runs
automatically, nothing is enabled by default, and nothing is tradable.

---

## 5. Why Not Flip `enabled=true` Now

1. **Validator gate.** `validate_registry_v2` fail-closed rejects
   `enabled=true` for non-equity instruments without
   `allow_enabled_non_equity_for_testing=true` — a flag explicitly documented
   as test/fixture-only with "no production enablement path through this
   schema" (Q4).
2. **No consumer needs it yet.** The only production-facing consumer of this
   schema, `ASSET-CORE-04F`'s `?registry_source=v2` economics-status bridge,
   is itself default-off (query-param opt-in) and read-only. There is no
   route, job, or screen today whose correctness depends on `enabled=true`
   for BTC/USD/ETH/USD.
3. **Blast-radius discipline.** Flipping `enabled` on a schema this codebase
   has repeatedly documented as "not a production cutover" (module header,
   `instrument_registry_v2.rs:1-15`) for the sake of a data-visibility CLI
   readiness check would be scope creep relative to what
   `CRYPTO-REGISTRY-03`/`04` actually need — those patches can prove readiness
   by reading the *current* `enabled=false` state truthfully, no flag change
   required.

---

## 6. Registry Candidate Shape Decision

**Keep the single existing fixture file.** No new
`instruments_v2.crypto_registry_candidate.*.json` file is created. Rationale:
Q5. If a future patch ever does need a distinct "candidate" registry (e.g. to
model a genuinely different set of fields for a real cutover), that is a new,
separately-authorized decision — not implied or pre-approved by this one.

---

## 7. Required Invariants Before a Future Scheduler Can Be Considered

None of the following are satisfied by this patch. Recorded so a future
scheduler-design patch inherits an explicit list rather than assuming any of
these away:

1. Kraken's public rate limit must move from "no numeric limit established"
   (current `providers.json.kraken.rate_limit_notes`) to a documented,
   verified numeric limit before any recurring call is designed.
2. A recurring-call design must reuse the existing
   `MarketDataProviderRateLimits` capability surface
   (`calls_per_minute`/`calls_per_day`/`remaining_calls`/`notes`) — unchanged
   guardrail carried forward from `CRYPTO-DATA-01H`/`01C`.
3. A scheduler must be a separately authorized patch with its own decision
   document — this patch does not authorize one.
4. A scheduler must default to disabled/opt-in, mirroring every other
   provider-enablement default in this repo (`kraken.enabled=false`,
   `twelvedata`/`alpaca` require explicit config).
5. A scheduler must not share a lock, cursor, or state slot with any existing
   equity ingestion scheduler (`CRYPTO-DATA-01C`'s scheduler-design decision
   already recorded this as an open question; still open).
6. A scheduler must not be Windows Task Scheduler-registered without a
   separate, explicit operator decision — `CRYPTO-DATA-01E`'s existing local
   crypto mark task registration is the only precedent, and it targets the
   local-CSV lane, not Kraken.

---

## 8. Required Invariants Before Crypto Risk/Execution/Strategy Can Be Considered

None of the following are satisfied by this patch:

1. A crypto-specific risk policy (asset-class-aware position/exposure limits)
   must exist and be tested — `mqk-execution/src/asset_risk_policy.rs`
   currently has no crypto-specific gate wired to any live path.
2. A crypto broker/paper-execution adapter must exist — none does; no
   crypto order has ever been constructed, submitted, or acknowledged by this
   repo.
3. A crypto strategy must exist and be backtested — none does.
4. Crypto 24/7 session-calendar semantics (`ASSET-CORE-05`'s
   session-profile scaffold) must graduate from a model-layer concept to an
   authoritative runtime source — unchanged open question carried forward
   from `ASSET-CORE-04E`/`CRYPTO-DATA-01H`.
5. Account-currency generalization must be resolved —
   `ASSET-CORE-04D`/`04F`'s route still hardcodes `account_currency = "USD"`;
   unchanged open question.
6. An explicit, separately authorized operator decision must set
   `paper_trading_enabled=true` for a specific instrument before any paper
   order is even attempted — this patch does not make or recommend that
   decision.
7. `live_trading_enabled=true` requires all of the above plus a live-broker
   integration decision — entirely out of scope for the foreseeable
   data-lane roadmap.

---

## 9. What This Patch Does Not Change

This patch (`CRYPTO-REGISTRY-02`) adds only: this decision document, its
machine-readable JSON artifact, a validator script, and
ledger/runbook/audit updates. It does not touch, and makes no behavior change
to: `config/providers/providers.json`, `config/instruments/*`, any Rust source
file in `core-rs/crates/mqk-md/src`, `mqk-daemon`, `mqk-runtime`,
`mqk-execution`, `mqk-broker-alpaca`, `mqk-broker-paper`, `mqk-risk`,
`mqk-portfolio`, any file under `core-rs/mqk-gui`, any DB migration,
`.env.local`, or any strategy/OMS/outbox/scheduler code. No daemon runtime was
started. No provider or network call was made. No API credits were spent. No
DB was mutated.

---

## 10. Safety Boundaries

Unconditionally true of this decision and must remain true of
`CRYPTO-REGISTRY-03`/`04`:

- `kraken.enabled` stays `false` in `config/providers/providers.json`.
- `BTC/USD.enabled` and `ETH/USD.enabled` stay `false` in
  `instruments_v2.crypto_local_marks.example.json`.
- `paper_trading_enabled` and `live_trading_enabled` stay `false` for both
  rows.
- No scheduler, Windows Task Scheduler registration, or daemon background job
  is added.
- No network call. No API credits spent. No credential read.
- No DB migration. No DB write except an isolated test using an already
  established cleanup pattern, if strictly necessary (not needed by this
  patch).
- No change to risk, OMS, broker, runtime, or strategy code.
- No claim of crypto trading readiness or production trading enablement.

---

## 11. Recommended Next Patches

1. **`CRYPTO-REGISTRY-03-KRAKEN-DATA-ONLY-REGISTRY-READINESS-CLI-01`** — a
   read-only operator CLI proving the current, unmodified registry/provider
   configs are ready for data-only Kraken OHLCV operations without touching
   any config flag.
2. **`CRYPTO-REGISTRY-04-KRAKEN-DATA-ONLY-REGISTRY-STATUS-SURFACE-01`**
   (conditional on `03` closing cleanly) — expose the same read-only truth
   through a daemon route and GUI panel.

---

## 12. Remaining Gaps (Unchanged by This Decision)

- No recurring/scheduled Kraken sync of any kind.
- No production registry-v2 cutover (`enabled` stays `false`).
- No crypto risk policy activation.
- No crypto paper or live execution.
- No crypto strategy.
- `CRYPTO-DATA-01`, `CRYPTO-REGISTRY-01`, `ASSET-CORE-04` remain `PARTIAL`,
  not `CLOSED`.
