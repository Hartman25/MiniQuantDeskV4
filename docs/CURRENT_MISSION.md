# MiniQuantDeskV4 — Current Mission

Status: **ACTIVE TURNOVER / CURRENT TRUTH**

This file is intentionally short. It records current durable project state, not project history.

---

## -9. M5-M8 Deterministic Code-Completion Controller (2026-09-26, `V4-M5-M8-DETERMINISTIC-CODE-COMPLETION-01`)

Full record: `MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md` §G3 and the section of the same name at the end of `docs/V4_CODE_COMPLETION_MANIFEST.md`. Baseline `8e029b86` (main = origin/main); nine local commits, NOT pushed.

Current truth for M5-M8, superseding the §-8 census where they differ (that census predates the QtyMicros runtime cutover):

- Fractional quantity now survives reconcile drift explanation, paper accounting, the durable Paper snapshot (migration 0078), broker P&L, and fails closed (409, no panic) on four V1 read/repair seams. Alpaca crypto wire correctness (position symbol, TIF, REST fractional fills) and admission TIF are fixed. Contract-identity validators (expiry, whole contracts, pair legs) are fixed in registry-v2, the intent/spec models and the options permission classifier.
- Alpaca still does NOT advertise crypto capability (operator decision); M7 (IBKR) and M8 (options execution/lifecycle) are BLOCKED_HARD_STOP for design/dependency reasons; per-instrument session handling in the autonomous controller and the fractional backtest domain are BLOCKED_HARD_STOP (design / frozen contract).
- Not run: full `cargo test --workspace` (resource bounded), any Paper/Live/provider session, GitHub CI.

---

## -8. V4 Bulk Code Completion Wave B — M5-M8 Multi-Asset: Frozen Matrix + Bounded Census + First Code (2026-09-19, `V4-BULK-CODE-COMPLETION-STAGE-B-M2-01`)

### Frozen V4 asset matrix (operator-approved, recorded here as durable truth)

CRYPTO: canonical integration market BTC/USD; market data Kraken; execution
broker Alpaca; 24/7 spot semantics; fractional quantity required.
FUTURES: canonical integration contract MES; execution broker IBKR;
executable-contract identity must stay distinct from research continuous-
series identity. FX: canonical integration pair EUR/USD; execution broker
IBKR; base/quote, pip/tick, leverage/margin, financing/rollover semantics
required. OPTIONS: canonical underlying SPY; execution broker Alpaca.

Frozen options permission set — ALLOWED: long call, long put, covered call,
cash-secured put, defined-risk call vertical spread, defined-risk put
vertical spread. NOT ALLOWED: naked short calls, naked short puts,
unlimited-risk structures, arbitrary complex/multi-leg strategies outside
defined-risk verticals.

### Bounded M5-M8 census (evidence-grounded, not exhaustive)

This repository already has a deliberate, multi-month-old architectural
pattern for exactly this problem: build asset-neutral **model-only, zero-
production-caller** contracts first (`ASSET-CORE-01` through `04`), prove
them with focused tests, and defer the live production cutover until a
concrete consumer requires it (`instrument_registry_v2.rs` module docs:
"a model + loader seam, not a production cutover... nothing in this module
is wired into any consumer"). This census inventories that existing layer
against the frozen M6-M8 assets rather than assuming it doesn't exist.

**Already real (model layer, zero production callers unless noted):**
- `mqk_schemas::{AssetClass, ContractSpec, Instrument, QtyMicros, OrderSpec}`
  (`core-rs/crates/mqk-schemas/src/lib.rs:119-230`) — `QtyMicros` is an
  explicit fixed-point fractional-quantity type ("future assets (crypto...)
  require fractional quantities"); `ContractSpec` already models
  `Option`/`Future`/`Crypto`. `AssetClass` here is canonical: it is the type
  actually checked by the live broker-submit gate.
- `mqk_md::instrument_registry_v2` (2,661 lines) — additive instrument
  schema modeling equity/option/future/crypto/forex/rate identity,
  contract shape, and (`InstrumentEconomicsMetadataV2`) multiplier/margin
  metadata. No production JSON file exists for this schema yet; nothing
  reads it in any daemon/CLI/ingest/backtest/GUI path.
- `mqk_execution::types::{OrderIntentV2, IntentV2Contract, BracketLegs}` —
  asset-neutral order intent with fractional `QtyMicros` qty, per-asset
  contract validation (currency pair, future multiplier/tick, option
  strike/multiplier), and a TP/SL bracket model. Explicitly documented as
  not wired into `BrokerGateway`/OMS/broker adapters (`lib.rs`: "RESEARCH-
  NON-EQ-01... NOT wired into canonical MAIN execution path").
- `mqk_execution::asset_risk_policy` (ASSET-CORE-03) — static per-asset
  policy table with two hard global kill switches,
  `ASSET_RISK_PRODUCTION_ENFORCEMENT_ENABLED = false` and
  `ASSET_RISK_NON_EQUITY_ROUTING_ENABLED = false`. Its own `crypto_policy`/
  `future_policy`/`option_policy`/`forex_policy` functions self-document
  the exact remaining gap per asset class (quoted verbatim below).
- `mqk_portfolio::{instrument_economics, portfolio_economics}`
  (ASSET-CORE-04A-F) — multiplier/currency-aware single-position valuation,
  integer-checked (`i128`), zero production callers; explicitly notes the
  still-missing bridge from `instrument_registry_v2`'s raw `multiplier: i64`
  to this crate's micros-scaled `contract_multiplier_micros` (`ASSET-CORE-
  04B... deferred`).
- `mqk_md::providers::kraken` (1,708 lines) + its `provider_registry`
  factory entry — a real, non-stub Kraken market-data provider.
- **New this session** (commit `2b9387e4`):
  `mqk_execution::option_strategy_permission` — the M8 frozen options-
  permission-set structural classifier (long call/put, covered call,
  cash-secured put, defined-risk call/put vertical spread; every other
  shape, especially a naked short, refused by name). 17 focused tests.
  Same model-only precedent as the rest of this layer.

**Confirmed CODE_MISSING / WIRING_MISSING (concrete, cited):**

- **M5 — fractional quantity does not reach the live execution boundary.**
  `mqk_execution::order_router::BrokerSubmitRequest.quantity: i64` and
  `mqk_execution::types::{TargetPosition,OrderIntent,ExecutionIntent}.qty:
  i64` are the types the real orchestrator/OMS/broker-adapter path actually
  uses; none of them is `QtyMicros`. `QtyMicros` exists only in the V2
  model scaffold. Threading fractional quantity through the live execution
  boundary is real, deterministically scoped work, but touches an already-
  audited, safety-critical path (OMS state machine, outbox/inbox,
  portfolio accounting, every existing equity call site) with no dedicated
  verification budget available this session — not attempted here; recorded
  as the single largest concrete M5/M6 blocker.
- **M5 — `MULTI-ASSET-ROUTING-GUARD-01` remains a hard equity-only gate.**
  `mqk_execution::gateway::BrokerGateway::submit_with_context`
  (`gateway.rs:388`) refuses every `AssetClass != Equity` before any broker
  adapter is invoked. Correct and intentional today; loosening it to a
  real per-`(asset_class, broker)` capability check is required before any
  of M6-M8 can submit a live/paper order, and was deliberately not
  attempted this session for the same reason as above (safety-critical,
  no dedicated verification budget).
- **M6 — crypto cannot flow through the production data-freshness
  controller.** `mqk-daemon/src/state/required_market_data_autofresh.rs`
  (the daemon's real, scheduled required-universe controller) fail-closed
  rejects any `instrument.asset_class != "equity"` (line 316) and any
  provider not declaring `supports_asset_class("equity")` (line 364); its
  top-level trading-day gate (`schedule.is_trading_day`, line ~1075) is a
  single NYSE-calendar check with no per-asset 24/7 override. Kraken's own
  scheduler has a read-only status route (`CRYPTO-DATA-03C`, per repo
  memory) but no task registration — Kraken ingestion is CLI-invoked only,
  never automatic. This exactly matches the mission's own description of
  the M6 data gap.
- **M7 — no IBKR integration exists at any level.** Verified directly:
  `grep -i ibkr` across the repo's non-test source returns zero real
  references; the only "interactive-brokers" string anywhere is a
  deliberate negative-control test fixture
  (`mqk-daemon/src/state.rs::unknown_broker_adapter_string_is_fail_closed`)
  proving an *unrecognized* adapter string fails closed — there is no
  `BrokerKind::InteractiveBrokers` variant, no adapter crate, no account/
  order/position/fill interface. `asset_risk_policy::future_policy()`/
  `forex_policy()` already name the remaining gap precisely: futures need
  "margin model, contract multiplier, expiry handling, and futures session
  calendar"; FX needs "pair registry, pip/lot sizing, leverage, currency
  conversion, and 24x5 session model." No roll/expiry logic exists.
- **M8 — no options chain/lifecycle/Alpaca-options capability.**
  `option_policy()` already self-documents: "options require chain
  metadata, contract multiplier, Greeks, assignment, and margin risk model
  before routing." `mqk-broker-alpaca` has no options-specific code path
  (grep for `option` in that crate returns only unrelated `Option<T>`
  Rust-syntax matches and one pricing-tick-size doc comment). Contract
  discovery, liquidity/spread data, expiry/exercise/assignment lifecycle,
  and Alpaca options order/position/fill handling are all absent.

**Why a full live production cutover was not attempted this session:**
every one of the four gaps above requires modifying either (a) the exact
safety-critical, already-audited execution/OMS/risk choke point this
repository's own `CLAUDE.md` and `execution_rules.md` single out for
extreme care (`BrokerGateway`, `OMS` state machine, outbox/inbox, the
`i64` quantity type threaded through dozens of already-tested call sites),
or (b) a brand-new broker wire-protocol integration (IBKR) with zero
existing scaffolding, real account/session/order lifecycle semantics, and
no committed design doc. Rushing either without a dedicated verification
budget is exactly the failure mode this repository's own prior sessions
have repeatedly identified and declined to rush (see the R2B/R2C
completed-bar-driver deferral and the M5 registry v1->v2 cutover
deferral, both above in this file). This session instead: (1) froze the
asset matrix as durable repo truth, (2) performed this grounded citation-
backed census, and (3) implemented the one concrete, safely-scoped,
zero-blast-radius M8 gap the census identified (the options-permission
classifier). **M5-M8 CODE_MISSING/WIRING_MISSING are NOT zero** — the wave
exit gate is not met. No Paper/Live/runtime state modified; no broker
network call made; no push.

---

## -7. V4 Bulk Code Completion Wave A — M2 C4/C5/C6 Closure + M3 Bounded Census (2026-09-19, `V4-BULK-CODE-COMPLETION-STAGE-B-M2-01`)

Continuation of §-6a's C1-C3 baseline. Scope: close the remaining M2 gap
(C4/C5/C6, the R2B/R2C multi-binding completed-bar driver work §-5 explicitly
deferred), then begin M3/M4/M5 CODE_MISSING/WIRING_MISSING implementation
under an explicit CODE-COMPLETION-FIRST mandate (exhaustive
regression/acceptance/operational proof deferred to a later verification
wave). Commit `f93b3660`.

**IR-3 — CORRECTED (2026-09-19, same-day follow-up).** The disposition
recorded immediately below ("ALREADY SAFE + PROVEN") and §-6a's original
record of it were both **incomplete, not merely stale**: they relied solely
on the read-side validator rejecting an already-wrapped negative
`scanner_rank` — real, but not a substitute for refusing the write itself.
The write path (`build_new_explicit_multi_strategy_authority`) still
performed an unchecked `r as i32` and would deliberately persist a
bit-reinterpreted value for any `scanner_rank` above `i32::MAX`. Fixed
(commit `9515b6f5`): the builder now returns
`Result<_, ScannerRankOverflow>` via `i32::try_from`; the one production
caller (`state/lifecycle.rs`'s explicit-v3 start path) propagates the
refusal through its existing `RuntimeLifecycleError::forbidden(...)`
fail-closed construction path — the durable authority is never built, and
the DB insert is never reached, for an overflowing value. Read-side
rejection is unchanged and still applies as a second, independent backstop.
Four focused tests (normal value, `i32::MAX`, `i32::MAX+1`, `u32::MAX`).
**IR-3 is CODE_CLOSED with a checked write, not merely a read-side catch.**

**C4 (multi-binding resolution) — CORRECTED (2026-09-19, same-day
follow-up).** The claim below that "genuine multi-*strategy* concurrent
dispatch through this specific autonomous path... remains achievable only
through the separate `DynamicSelectionHostPool` the interactive execution
loop already uses" was wrong against the authoritative M2 contract, which
requires same-symbol competing strategies (e.g. `AAPL/strategy_A` +
`AAPL/strategy_B`) to progress independently through production code, not
merely through one specific runtime path. Fixed (commit `e37d10c6`):
`resolve_effective_bindings` now checks a configured assignment against
both the legacy engine *and* the current run's real host-pool selection
(`AppState::dynamic_selection_runtime_snapshot`) before ever rejecting it.
A binding matching the host pool's own selected pairs resolves
`BindingDispatchRoute::HostPool` and dispatches through the existing
`pending_strategy_bar_input` mailbox hand-off (never reaching into the
execution loop's exclusively-owned host-pool object), confirmed via the
durable `strategy_signal_evaluations` row the loop's own dispatch writes.
Migration 0075 adds a strategy_id-scoped claim table
(`sys_autonomous_daily_binding_bar_dispatches`) since two strategies can
share one `(symbol, timeframe)` and migration 0050's identity cannot
distinguish them. The Tier-A single-engine policy remains frozen and
unchanged for the *legacy* bootstrap itself — it was never actually a
ceiling on what this driver may resolve or dispatch, and is no longer
treated as one. Proof:
`same_symbol_multi_strategy_resolves_via_host_pool_never_narrowed_or_refused`
resolves all three of AAPL/strategy_A, AAPL/strategy_B, MSFT/strategy_C via
the host pool, none narrowed or refused. **C4 now supports genuine
same-symbol multi-strategy production dispatch, not only multi-symbol
single-shared-strategy.**

**C5 (fault isolation).** `tick_autonomous_completed_bar_driver_multi`
classifies every binding's tick outcome via the frozen hybrid isolation
policy (migration 0071) into healthy/local-fault/global-critical
(`classify_binding_outcome_for_isolation`, exhaustive match, no wildcard),
writing `sys_autonomous_daily_binding_state` per binding. Migration 0074
adds one new closed reason (`binding_strategy_engine_not_active`) that
0071's original five did not honestly cover.

**C6 (wiring).** `autonomous_completed_bar_task.rs` routes a >1-symbol
assignment config through the new multi-binding entrypoint (each binding
resolving its own `provider_id` from the instrument registry); an
exactly-one-symbol config keeps the original single-binding call
byte-for-byte unchanged.

**Proof:** `scenario_autonomous_completed_bar_driver_01` (58/58, including
the same-symbol multi-strategy proof) and `scenario_autonomous_completed_bar_task_01`
(47/47, 2 DB-only ignored) pass; `cargo check -p mqk-db -p mqk-daemon --lib
--tests` clean; `check_migration_governance.sh` all 3 checks pass. DB-backed
`sys_autonomous_daily_binding_state`/`sys_autonomous_daily_binding_bar_dispatches`
scenario suites, an end-to-end dispatch-through-a-real-host-pool integration
test, and full daemon/db regression remain deferred to the verification wave
per this mission's explicit mandate — this wave's proof is at the resolution
layer (routing is correct, never narrowed/refused) plus reuse of
already-tested claim/completion primitives, not a live host-pool dispatch
run.

**M2 status: CODE_MISSING = 0, WIRING_MISSING = 0**, against the M2.1-M2.9
requirement list (§-5/§-6a's census) *and* including same-symbol
multi-strategy production dispatch (commits `f93b3660`, `9515b6f5`,
`e37d10c6`). M2 operational acceptance is still not claimed.

**M3 bounded census (Concurrent Paper + Live Execution Domains) — audit
before build, per this mission's own instruction.** Read
`mode_transition.rs` (canonical, restart-only mode-transition state machine;
hot switching is architecturally unsupported — every `DeploymentMode` is
fixed for a daemon process's whole lifetime), `state/env.rs`
(`deployment_mode_readiness`, per-mode/per-broker readiness, including an
explicit `(LiveCapital, Paper-broker)` block), `state/lifecycle.rs`'s
`LiveCapital` trust-chain gate (`TV-03`, fail-closed until parity proof
completes — confirmed still fail-closed at this HEAD), and confirmed
`deployment_mode` is already durable identity on
`sys_autonomous_daily_operations` and `sys_paper_portfolio_snapshots` with
explicit mismatch rejection (`paper_portfolio.rs`: "expected 'paper'").
Combined with Paper and Live using physically separate databases
(operationally enforced — Paper 5440 / Live 5432, per this mission's own
protected-resource list) and separate broker base URLs per mode
(`alpaca_base_url_for_mode`), the negative controls M3's contract requires
("Paper cannot submit to Live," "Live cannot submit to Paper," "duplicate/
replayed deployment identity cannot cross domains") are structurally
satisfied by already-existing, already-committed architecture — not a
dormant or partial seam requiring new code. No `CODE_MISSING`/
`WIRING_MISSING` gap was found against the M3 finish-line contract's stated
requirements. **No new production code was written for M3 in this session**
— per the same principle §-5's M2 census applied ("if code is already
complete, prove it; do not invent changes").

**M3 status: CODE_MISSING = 0, WIRING_MISSING = 0** (bounded census, not an
exhaustive audit — see caveat below). **TEST_MISSING:** a dedicated
integrated negative-control proof (two daemon processes, Paper-mode and
Live-shadow-mode, running concurrently against their real separate
databases, with an explicit attempt to cross-submit/replay identity between
them and observe the fail-closed refusal) has never been run and is not
attempted here — this mission explicitly forbids enabling Live or any
broker mutation this session, and exhaustive acceptance testing is deferred
to the verification wave by mandate. This census is bounded (mirrors the
Stage A/M2 historical-correction precedent: items not independently
re-verified end-to-end should be treated as citation-unverified until that
proof exists). M3 operational acceptance is not claimed.

**M4 bounded census (US Equity/ETF Live Production).** Read
`parity_evidence.rs` (parses/validates an external `parity_evidence.json`
TV-03 artifact; `live_trust_complete` is surfaced honestly, never
fabricated — every current build's artifact hardcodes it `false` because no
real shadow-execution cycle has ever completed), `state/lifecycle.rs`'s
LiveCapital-transition gate (fail-closed on `!live_trust_complete`,
confirmed unchanged at this HEAD), `state/broker.rs::build_daemon_broker`
(refuses `BrokerKind::Paper`/`LockedPaperBroker` outright as "not the
canonical paper-trading execution path" — fail-closed against a broker that
would accept orders with no real fills; `BrokerKind::Alpaca` selects
`ALPACA_API_KEY_PAPER`/`ALPACA_API_SECRET_PAPER` for `Paper` vs
`ALPACA_API_KEY_LIVE`/`ALPACA_API_SECRET_LIVE` for
`LiveShadow`/`LiveCapital` — separate credential identity per domain, not a
shared secret gated only by a flag), and `alpaca_base_url_for_mode`
(`paper-api.alpaca.markets` vs `api.alpaca.markets`, refuses `Backtest`
outright). The reconciliation/outbox/inbox/OMS-state-machine/restart-safety
machinery this gate sits in front of is deployment-mode-agnostic by
construction (the same Alpaca adapter code already runs continuously
against the Paper endpoint) — M4's required proof surface (order/fill
lifecycle, reconciliation, cancel, restart/recovery, disconnect/reconnect)
is therefore already exercised in production against Alpaca Paper today;
what remains before it can be *claimed* for Live is exclusively the
economic/operational chain the finish line itself names as sequential and
human-gated (accumulate real shadow-execution evidence -> `live_trust_complete=true`
-> explicit operator Live authorization -> tiny-capital Live validation),
never a missing code seam.

**M4 status: CODE_MISSING = 0, WIRING_MISSING = 0** against everything this
census actually read (bounded, not exhaustive). **OPERATIONAL_ONLY /
BLOCKED (correctly, not fabricated):** `live_trust_complete=true` itself —
requires a completed real shadow-execution cycle this session cannot and
must not produce — and every step downstream of it (operator Live
authorization, tiny-capital order, real Live fill/reconciliation proof).
No code was written or changed for M4 in this session; no Live credential,
broker, or capital action was taken or simulated.

**M5 bounded census (Asset-Neutral Production Contracts).** `git log
--grep=asset-core -i` shows the most recent asset-core/registry-v2 work
(portfolio economics v2, multi-asset NAV aggregation, registry v2 status
surfacing) predates this bulk-completion wave by roughly three months, with
no work in between. Per this repository's own memory record, that lineage
already closed CODE_LOCAL but left an explicit, *recorded operator
decision* rather than an oversight: registry v1 remains the sole trading
identity source of truth; registry v2 (and the portfolio-economics/NAV
seams built on it) has zero production callers by design, pending a
deliberate v1->v2 cutover this repository's own prior sessions declined to
attempt without a dedicated verification budget of its own — the same
category of risk this wave's own C4/C5/C6 investigation (§ above)
independently rediscovered for a different subsystem. M5's own finish line
explicitly warns against exactly the alternative: "the contract is proven
sufficient by concrete later-asset requirements rather than speculative
abstraction" — and no M6 (crypto) work has started, so no concrete
non-equity requirement yet exists to prove any further generalization
against. Building more asset-neutral surface now, with no live M6 consumer
and no committed decision to cut equities over to it, would be exactly the
speculative framework the frozen contract forbids.

**M5 status: PARTIAL, unchanged from repo truth. Not a new
`CODE_MISSING` finding** — the registry v1->v2 cutover is a known,
previously-recorded `BLOCKED_OPERATOR_DECISION` (deferred pending its own
dedicated session, not an omission of this wave), and no other concrete
M5 gap was identified against the finish-line contract without inventing
speculative scope. No code was written or changed for M5 in this session.

No Paper/Live/runtime state modified; no push; `smoke_logs/` untouched.

---

## -6a. Stage B M2 — C1/C2/C3 Consolidated Surgical Correction (2026-09-19, `V4-STAGE-B-M2-C1-C3-CORRECTION-02`)

Independent review of §-6's C1/C2/C3 completion bundle found two deterministic,
CI-breaking repo-truth defects that §-6's `CODE_CLOSED` claims did not account
for, plus one risk finding requiring disposition. All three are now closed.
This does not retract §-6's `CODE_CLOSED` verdicts (the production behavior
they describe was already correct); it corrects the repo-inventory/gitattributes
governance gap that would have failed CI, and adds proof for a risk finding.

- **IR-1 (migration manifest drift).** Migration `0073_explicit_multi_strategy_
  authority_full_evidence.sql` was added on disk in `b0e957b5` but never added
  to `migrations/manifest.json`. `scripts/guards/check_migration_governance.sh`
  (wired into CI at `.github/workflows/ci.yml`) already enforces exact
  manifest/filesystem parity and was failing deterministically at HEAD `6c8e1d16`
  (`FAIL: manifest drift detected` — confirmed by running the guard before the
  fix). Added the missing manifest entry; guard now passes.
- **IR-2 (`.gitattributes` LF-pin omission).** The same commit range explicitly
  pinned migrations 0068-0072 to `eol=lf` in `.gitattributes` (byte-sensitive
  SQLx checksum identity) but omitted 0073. Guard 3 of the same script was
  failing (`eol: unspecified` for 0073, confirmed pre-fix). Added the missing
  `.gitattributes` line; guard now passes for all three checks.
- **IR-3 (scanner_rank narrowing-cast risk).** **CORRECTED 2026-09-19, see
  §-7 above — this disposition was incomplete: it never fixed the write
  itself, only the read-side backstop. The write is now a checked
  conversion (commit `9515b6f5`); the "ALREADY SAFE + PROVEN, not fixed"
  text immediately below is superseded and kept only as the historical
  record of what this session actually found and concluded at the time.**
  The write path persists
  `SelectionCandidateEvidence.scanner_rank` (`Option<u32>`) as `Option<i32>`
  via `.map(|r| r as i32)`. Disposition (superseded): **ALREADY SAFE +
  PROVEN**, not fixed.
  `u32 as i32` is a same-width bit-reinterpreting cast: every value above
  `i32::MAX` has its top bit set and therefore always becomes negative — a
  two's-complement identity, not a coincidence of typical rank values — and
  the existing read-side validator (`explicit_multi_strategy_evidence_validator
  ::stored_binding_to_evaluation`) already rejects any negative stored value
  as `NegativeScannerRank`, failing closed rather than silently accepting a
  wrapped value. Added a boundary/mutation test
  (`scanner_rank_overflow_is_always_caught_never_silently_accepted`) proving
  i32::MAX round-trips exactly and every overflowing u32 value is rejected;
  verified red (weakening the `r >= 0` guard fails the test) then green
  (restoring it passes).
- Census of analogous narrowing casts in the same seam (`binding_count`,
  `authorized_count`, `ordinal`, all `usize`/`.len()`-derived `as i32`)
  found no code-backed defect: all are bounded by an in-memory parsed
  watchlist-v3 artifact's realistic size (never remotely approaching
  `i32::MAX`), and even in a hypothetical wrap, the validator's exact-equality
  comparisons (never merely "is present") would still fail closed rather than
  coincidentally match. No dedicated test added for these — no plausible
  failure mode exists to prove against, unlike IR-3's explicit two's-complement
  boundary.
- C1 dormancy-gate call sites re-traced via the repo graph (`graft_find_all`
  for `strategy_bootstrap_dormant|is_dormant()`): only two production gate
  sites exist (`lifecycle.rs::start_execution_runtime`,
  `autonomous_runtime_context::resolve_autonomous_runtime_context_from_fleet`),
  both already consuming the one shared `explicit_watchlist_v3_authority_pending`
  predicate per §-6's own `0af79883` — no residual parallel interpretation
  found. `autonomous_completed_bar_driver.rs`'s dormancy check was re-confirmed
  out of scope (drives the separate legacy single-binding path, never the
  explicit-v3 host pool) — same conclusion §-6 already recorded.
- C3 activation ordering re-verified directly in
  `build_explicit_multi_strategy_start_snapshot`: write -> read-back -> validate
  -> (only then) `build_explicit_multi_strategy_dispatch_authority`/host-pool
  construction. Traced every production caller of `DynamicSelectionHostPool::
  build`; the only caller in the explicit-v3 authority path is this function.
  No alternate activation entry point exists.

**Acceptance (corrected numbers):** `cargo test -p mqk-daemon --lib` 998
passed / 0 failed / 22 ignored (one more than §-6's 997 — the new IR-3 proof
test); `cargo test -p mqk-db --lib` 79 passed / 0 failed / 24 ignored;
`cargo test -p mqk-db --test scenario_explicit_multi_strategy_authority_01 --
--ignored` 5 passed against `mqk_test`; `scripts/guards/check_migration_
governance.sh` passes (all 3 checks); `git diff --check` clean; `smoke_logs/`
untouched; no Paper/Live/broker mutation.

**Status: C1 = CODE_CLOSED (unchanged). C2 = CODE_CLOSED (unchanged; migration
inventory gap repaired). C3 = CODE_CLOSED (unchanged).** No new deterministic
C1/C2/C3 defect remains. C4/C5/C6/M3 not started, not authorized. No push.

---

## -6. Stage B M2 — C1/C2/C3 Final Completion (2026-09-19, `V4-STAGE-B-M2-C1-C3-FINAL-01`)

Reconciles §-5 below, whose draft text (written mid-`V4-STAGE-B-M2-REPAIR-03`)
was never updated to reflect three further commits that landed later the same
day under a follow-on controller (`V4-STAGE-B-M2-C1-C3-REPAIR-04`, visible in
those commits' own code comments): `97a9af5c` (config: align watchlist-v3
with frozen runtime authority), `b0e957b5` (authority: bind complete explicit
strategy evidence identity — Patch D2, migration 0073's full-evidence
columns), and `ac7b400d` (runtime: true read-side validation — Patch D3,
`explicit_multi_strategy_evidence_validator.rs`, plus lifecycle.rs's own
`explicit_v3_authority_pending` STRATEGY-DORMANCY-01 bypass). §-5's "R2B/R2C
Still Open" framing undersold what REPAIR-04 had already closed; this section
is the accurate final record.

**This controller's own two commits**, starting from HEAD `ac7b400d`:

- `0af79883` (C1): `resolve_autonomous_runtime_context_from_fleet`
  (`state/autonomous_runtime_context.rs`) — the one seam every autonomous
  daily-coordinator/completed-bar-task/operator-retry caller uses — still
  carried the pre-REPAIR-04 STRATEGY-DORMANCY-01 interpretation verbatim:
  Paper+Alpaca with no `MQK_STRATEGY_IDS` refused unconditionally even with
  an approved watchlist-v3 fleet configured, even though `lifecycle.rs`'s own
  `start_execution_runtime` gate had already been repaired to bypass this.
  Extracted the bypass predicate into one shared, pure
  `dynamic_selection_mode::explicit_watchlist_v3_authority_pending` helper
  consumed by both call sites, closing the one remaining parallel
  interpretation the frozen-contract review had flagged. Proof: three new
  focused tests (`a11`/`a12`/`a13` in
  `scenario_autonomous_daily_coordinator_policy_01.rs`) prove the bypass
  fires only for an approved v3 artifact under `paper_enforced`, never for
  `LoadedNotApprovedV3` or outside that mode.
- `420c0690` (C2): adversarial field-by-field audit of
  `derive_explicit_multi_strategy_authority_id` found all 33
  `SelectionCandidateEvidence` fields structurally bound into `authority_id`,
  but 9 of them had no mutation-test proof the binding actually holds
  (`promotion_query_ok`, `promotion_state`, `config_identity_verified`,
  `durable_config_fingerprint`, `current_config_fingerprint`,
  `registry_enabled`, `data_ready`, `promotion_transition_id`,
  `evidence_transition_id`). Added the missing mutators. Also replaced the
  DB round-trip test's handful of spot-checks with an exhaustive per-field
  comparison of all 40 persisted columns against the value written, run for
  real against `postgres://postgres:postgres@127.0.0.1:5434/mqk_test`.

**C3**: adversarially reviewed `explicit_multi_strategy_evidence_validator.rs`
(D3) and `build_explicit_multi_strategy_start_snapshot`'s call ordering
directly — the validator recomputes `authority_id` from durable stored facts
(never trusts the stored column), checks every header identity field, the
exact binding set (missing/extra/duplicate), and is called strictly before
host-pool construction (`validate_...` at `lifecycle.rs:1181`, host pool
`build_explicit_multi_strategy_dispatch_authority` at `lifecycle.rs:1210`+).
The status route (`routes/dynamic_selection_evidence.rs`) truthfully
distinguishes `explicit_watchlist_v3_multi_strategy` and never fabricates a
Bundle-7 `committed_plan_id`/`committed_source_kind` for it (already covered
by its own dedicated test). No defect found; no additional patch needed.
Ran the integrated DB-backed proof
(`c3_01_real_registry_promotion_and_evaluate_candidate_drive_durable_authority`,
real registry + real research/backtest/promotion-to-`active_paper` chain)
against the real test DB this session — passes.

**Known, deliberately out-of-scope finding (not part of C1/C2/C3, not
touched)**: `autonomous_completed_bar_driver.rs`'s `prove_running_dispatch_eligibility`
treats a `Dormant` native-strategy bootstrap as unconditionally
"not ready" for per-bar dispatch — a second dormancy interpretation on paper,
but this driver only supports the single-effective-binding (legacy
`MQK_STRATEGY_IDS`) dispatch path; it is not on the explicit-v3 runtime path
at all (that path dispatches through `state/loop_runner.rs`'s host pool, per
the frozen contract §9). This is the already-identified, already-deferred
R2B/R2C multi-binding completed-bar driver gap (§-5 below), requiring its
own dedicated verification budget per that prior session's explicit decision
— confirmed still accurate, not re-attempted here (mission scope: C1/C2/C3
only, no C4/C5/C6).

**Acceptance**: `cargo test -p mqk-daemon --lib` 997 passed / 0 failed / 22
ignored; `cargo test -p mqk-db --lib` 79 passed / 0 failed / 24 ignored;
plus the DB-backed integration suites for both crates' explicit-multi-
strategy-authority mechanisms run directly against `mqk_test`, all passing.

**Final status: C1 = CODE_CLOSED. C2 = CODE_CLOSED. C3 = CODE_CLOSED.**
Independent review of the diff is still required before any of the three is
treated as accepted contract. **M2 overall remains NOT CLOSED** — C4/C5/C6
(the R2B/R2C multi-binding completed-bar driver/aggregation rewrite among
them) are the remaining Stage B work, out of scope for this controller. M1
operational status is unchanged; alpha discovery remains deferred; no
Paper/Live/runtime state was modified; no push; no M3.

---

## -5. Stage B M2 — R1B Live Activation Closed; R2B/R2C Still Open (2026-09-19, `V4-STAGE-B-M2-REPAIR-03`)

Independent review found R1A/R1B were not fully coherent (§-4 below): the
frozen contract required watchlist-v3 wired into the *canonical* intake
contract (`WatchlistIntakeOutcome` itself), but R1B instead built a wholly
separate `WatchlistIntakeOutcomeV3` type the canonical evaluator never
recognized. This controller repaired that mismatch and closed R1B's
previously-`WIRING_MISSING` live-activation gap.

**Patch C1 (commit `7db0a18b`) — canonical intake repair.**
`WatchlistIntakeOutcome` now has `LoadedApprovedV3`/`LoadedNotApprovedV3`
variants; `evaluate_watchlist_intake` recognizes `watchlist-v3` directly
(v1/v2 parsing byte-for-byte unchanged); `state/multi_symbol_config.rs`
gained the frozen contract's v3-aware config source. The V16 test that
previously asserted the *mismatch* as correct behavior is replaced with
proof of the fix.

**Patch C2 (commit `9fc2f912`) — durable explicit-authority evidence.**
New additive tables (migration 0072) `sys_explicit_multi_strategy_authority`
+ `_bindings`, deliberately separate from `sys_dynamic_selection_plans`
(Bundle 7's own ranking-plan schema — never conflated). `authority_id` is a
deterministic identity binding every result-affecting input (run_id, source
artifact hash, config fingerprint, market_date, every binding's own
evidence) so a changed artifact or a changed promotion/config/readiness
fact can never reuse a stale authority. Idempotent-insert-or-payload-
collision, mirroring `insert_dynamic_selection_plan`'s established pattern.

**Patch C3 (commit `644cfde7`) — real daemon-start activation. R1B's
WIRING_MISSING gap is now CODE_CLOSED.** `state/lifecycle.rs`'s real
`build_dynamic_selection_start_snapshot` now routes a configured
`watchlist-v3` artifact under `PaperEnforced` to a new
`build_explicit_multi_strategy_start_snapshot`, which runs the full real
sequence (canonical v3 validation -> real `evaluate_candidate` DB I/O ->
build+persist+read-validate the durable authority -> construct the isolated
host pool -> return committed runtime truth) and fails start closed on any
step. A new `RuntimeStrategyAuthorityKind` (`Legacy` /
`Bundle7DynamicSelection` / `ExplicitWatchlistV3MultiStrategy`) lets a
status surface distinguish which mechanism produced a given start's
authority without inferring it from `plan`'s presence; this mechanism never
fabricates a Bundle-7 plan. Proven with real registry + real
research/backtest/promotion-to-`active_paper` evidence (not hand-built
`SelectionCandidateEvidence`): the durable evidence persists even when the
overall start is honestly refused (data readiness genuinely fails for a
symbol with no real instrument-registry entry — the same wall R1B's own
heaviest existing fixture, `full_evidence_chain_passes_refused_only_on_data_readiness`,
also stops at); an unregistered sibling refuses independently; a genuinely-
promoted identity is still excluded when dry-run-flagged; tampering the
persisted row then re-running fails closed as a payload collision; two
independently-authorized same-symbol identities build two real hosts; `Off`
and the live-lock are untouched. Full regression: 983 mqk-daemon lib tests
green.

**R2B/R2C (multi-binding completed-bar driver + hybrid aggregation) — still
NOT IMPLEMENTED.** Investigated: `resolve_single_effective_binding`
(`state/autonomous_completed_bar_driver.rs`) is the exact-one-binding
restriction R2 must replace, embedded through ~2,600 lines of already-
audited, safety-critical claim/dispatch machinery across
`autonomous_completed_bar_driver.rs`, `autonomous_completed_bar_task.rs`,
and `autonomous_daily_coordinator.rs` — the live autonomous-trading
heartbeat. Not attempted this session: a correct rewrite requires its own
dedicated verification budget (idempotency/restart-safety proofs, per-
binding negative controls) that this session's remaining time could not
responsibly provide without risking exactly the kind of under-verified
change this repair mission exists to prevent (CLAUDE.md's correctness-first
priority). C6 (integrated M2 finish-line proof) is consequently also not
attempted — it is blocked on C4/C5 existing as real production paths.

**Doc truth corrections (this section):** the R1B section below (§-4)
claimed "All 10 of R1B's mission-required focused proofs... each calling a
real production function" for the *whole* proof set; several of those
proofs (the Bundle-6-conflict-resolution ones) call
`compute_dynamic_selection_plan` directly with hand-built
`SelectionCandidateEvidence` — genuine, valid proofs of the pure
selector/conflict-resolution logic, but not DB evidence-gate proofs. Only
the promotion/registry/dry-run-facing proofs (`r1b_06`, `r1b_08`) exercise
real DB-backed evidence. This session's own C3 tests are the first to
exercise the real `evaluate_candidate` DB path end to end for this
mechanism. A separately-referenced "V3 report" commit-count typo could not
be located anywhere in this repository or in `Downloads/`; it may refer to
an external/prior-session artifact not present on this machine and was not
corrected.

**Corrected M2 totals (this session):** `CODE_CLOSED 7/9` (M2.2, M2.4,
M2.5, M2.6, M2.8, M2.9, plus **M2.1 and M2.3 upgraded from
WIRING_MISSING to CODE_CLOSED** by C1-C3), `WIRING_MISSING 1/9` (M2.7 —
durable per-binding schema CODE_CLOSED via R2A, driver/aggregation still
not wired, R2B/R2C). **M2 CODE COMPLETION remains NOT CLOSED** (one
requirement, M2.7, still open) and **M2 operational acceptance remains
NOT CLAIMED** regardless. No Paper/Live/runtime state modified; no push;
no M3; no alpha discovery.

---

## -4. Stage B M2 Completion After Operator Decisions (2026-09-18, `V4-STAGE-B-M2-REPAIR-02`)

The operator resolved both `SPEC_DECISION_REQUIRED` items from §-3 below
with frozen decisions. This controller implemented against both. Full
evidence: `docs/V4_CODE_COMPLETION_MANIFEST.md` § "Stage B M2 Completion
After Operator Decisions".

**R1 frozen decision:** explicit per-symbol multi-strategy authorization via
a new, additive `watchlist-v3` schema (`symbol -> Vec<strategy_id>`), not
implicit fleet-wide activation and not a relaxation of Bundle 7's frozen
selector. Design frozen in
`docs/specs/multi_strategy_runtime_dispatch_01a_frozen_contract.md`
(commit `7f78cc0d`).

**R1B result — CODE_CLOSED (pipeline) / WIRING_MISSING (live activation).**
The same-symbol multi-strategy dispatch/conflict/config pipeline is
implemented and proven with 10 real production-function-level proofs (69
tests green: commit `a00f9238`). Two real strategies on one symbol
genuinely dispatch with distinct identity; both reach Bundle 6 and resolve
deterministically regardless of input order; a promoted sibling never
authorizes an unpromoted one; same-symbol provenance swap fails closed;
dry-run identity is excluded; no cross-contamination; existing v1/v2
config is unaffected (43/43 regression). **Not yet spliced into the live
daemon start sequence** — that requires extending Bundle 7's
plan/evidence-persistence type shape, a new observability surface R1A's
own contract explicitly deferred out of R1B's scope. This is a narrow,
well-understood follow-up integration patch, not an open design question.

**R2 frozen decision:** hybrid per-binding fault isolation. A binding-local
failure (symbol-specific no-new-bar, missing/stale data, readiness
failure, unsupported symbol/timeframe, isolated provider failure)
quarantines only that binding. A global-critical failure (evidence-lineage
corruption, dispatch-claim ambiguity, runtime-ownership/leadership
failure, etc.) remains operation-wide fail-closed via the existing state
machine, unchanged.

**R2A result — CODE_CLOSED.** New additive table
`sys_autonomous_daily_binding_state` (migration 0071, commit `9fd7df12`)
durably tracks each binding's `active`/`locally_blocked` health with a
closed reason-code vocabulary. 8/8 DB-backed tests: schema/constraint
enforcement, restart persistence, cross-binding isolation.

**R2B/R2C result — NOT IMPLEMENTED, WIRING_MISSING.** The multi-binding
completed-bar driver rewrite and operation-state aggregation
(~2,600 lines across `autonomous_completed_bar_driver.rs`,
`autonomous_completed_bar_task.rs`, `autonomous_daily_coordinator.rs` —
the actual live autonomous-trading heartbeat) were investigated in full
but not attempted this session: the safety-critical nature of this exact
code path, and the fact that this whole repair mission exists because a
prior session rushed a closure claim on this same subsystem class, made a
rushed rewrite in remaining session time the wrong tradeoff. R2A's durable
foundation is ready for a focused follow-up patch with its own dedicated
verification budget.

**R3:** reconfirmed unchanged, 16/16 still passing, no code modified.

**R4:** still `BLOCKED` — needs R1B's live-activation splice and R2B/R2C,
neither of which exist as production paths yet.

**Corrected M2 totals:** `CODE_CLOSED 6/9` (M2.2, M2.4, M2.5, M2.6, M2.8,
M2.9), `WIRING_MISSING 3/9` (M2.1, M2.3, M2.7 — each has a real, tested
underlying capability; each is missing only its final production-
activation splice). `SPEC_DECISION_REQUIRED: 0` (both resolved this
session). **M2 code completion is NOT closed** (per this controller's own
rule: only claim closed if the real integrated proof, R4, passes — it does
not yet). M2 operational acceptance remains unclaimed. M1 operational
blocker, the `intraday_scalper` rejection, and alpha-discovery deferral
are unchanged.

---

## -3. Stage B M2 Repair Outcome — R1/R2 stopped as SPEC_DECISION_REQUIRED (2026-09-18, `V4-STAGE-B-M2-REPAIR-01`)

Independent review rejected the original Stage B 9/9 CODE_CLOSED conclusion
(see §-2 below). This controller investigated both confirmed gaps in full
and, for each, found a genuine unresolved operator decision — not merely
missing code — blocking further implementation. **No production/test code
was changed by this controller.** Full evidence and cited line numbers:
`docs/V4_CODE_COMPLETION_MANIFEST.md` § "Stage B M2 Repair Findings".

**R1 (`MULTI-STRATEGY-RUNTIME-DISPATCH-01`, blocks M2.1/M2.3) — `SPEC_DECISION_REQUIRED`.**
Both Bundle 6's and Bundle 7's own committed design docs explicitly defer
"how multiple strategies become simultaneously economically active on the
same symbol" to this not-yet-designed follow-up patch; no committed
contract answers it. Operator must choose one of:
- (a) multi-entry `MQK_STRATEGY_IDS` = fleet-wide multi-strategy;
- (b) watchlist-v2 `strategy_assignments` becomes `symbol -> Vec<strategy_id>` (per-symbol explicit);
- (c) relax `dynamic_selection.rs`'s frozen one-per-symbol contract to ranked top-N under a new mode;
- (d) something else.

Each has different capital-caps/promotion-authority consequences that this
controller will not invent unilaterally (`CLAUDE.md` §19).

**R2 (multi-symbol completed-bar driver, blocks M2.7) — `SPEC_DECISION_REQUIRED`.**
Concrete, code-proven conflict found: `select_driver_mode_for_state`
(`state/autonomous_completed_bar_task.rs:154-164`) returns `None` (halts
**all** automated driver invocation, for every configured symbol) for
`controller_degraded`/`evidence_degraded` — so today's single-binding
"any critical fault degrades the operation" behavior, naively extended to
N bindings, would let *one* symbol's non-remediable blocker silently stop
dispatch for *every* symbol. This directly violates R2's own requirements
("a claimed bar for symbol A cannot suppress symbol B"; "one symbol lacking
a new bar must not cause another symbol's real new bar to be lost").
Operator must choose:
- (a) keep coarse operation-wide degrade (simplest, but knowingly violates
  the isolation requirement the moment >1 symbol is configured);
- (b) add genuine per-binding fault state and change the state-machine
  gating so operation-level degrade reflects "no binding can progress," not
  "any binding failed" (correct, but a real schema + state-machine change to
  an already-audited subsystem);
- (c) a hybrid (only certain fault classes — e.g. evidence/claim-integrity —
  degrade globally; per-symbol readiness/config faults isolate).

**R3 (retry/restart idempotency proof) — `CLOSED`.** Ran
`scenario_strategy_decision_idempotency_01` in full against the real test
Postgres: 16/16 passed (4 DB-backed, including restart/replay and
multi-symbol cross-contamination negative controls). No code changed — M2.6
now has direct load-bearing proof from this session, not just a cited seam.

**R4 (integrated M2 finish-line scenario) — `BLOCKED`** on R1/R2; cannot be
honestly built without reimplementing the missing production logic inside
the test, which would prove nothing real.

**Corrected M2 totals:** `CODE_CLOSED 6/9` (M2.2, M2.4, M2.5, M2.6, M2.8,
M2.9), `WIRING_MISSING 2/9` (M2.3, M2.7 — both blocked on the above),
`PARTIAL/WIRING_MISSING 1/9` (M2.1). **M2 code completion is NOT closed.**
Two operator decisions (R1, R2) are required before remaining M2 code can
proceed. M1 operational blocker, `intraday_scalper` rejection, and alpha
discovery deferral are unchanged.

---

## -2. Governance Decision — Bulk Code Completion Resumes (Stage B / M2), M1 Operational Status Unchanged (2026-09-18, `V4-BULK-CODE-COMPLETION-STAGE-B-M2-02`)

Branch: `v4-bulk-code-completion-stage-b-m2-01`. Baseline HEAD (ancestor):
`28fff6952a65791cd489b88d3673561ab8c12a03`.

The operator has explicitly changed near-term execution priority. This is a
sequencing decision only; it does not redefine milestone acceptance.

**Two status dimensions are now tracked separately and must not be
conflated:**

- **A. CODE COMPLETION** — may continue into Stage B / M2 now.
- **B. OPERATIONAL ACCEPTANCE** — unaffected by this decision; M1 remains
  `BLOCKED` per the M1.9 correction below.

**Durable truth carried forward unchanged by this decision:**

- Stage A / M1 code-completion: `INDEPENDENTLY ACCEPTED`, `PUSHED-VERIFIED`.
  M1 code census: `CODE_MISSING=0`, `WIRING_MISSING=0`, `TEST_MISSING=0`.
- Deployed strategy identity: `intraday_scalper` / `AAPL` / `300s`.
- A genuine real-data promotion-evidence attempt was performed once and was
  **REJECTED**: `review_state=rejected`, `reason_code=negative_total_return`.
  This rejection is durable truth. It is not waived or converted to a pass.
  `intraday_scalper` must not be promoted from that evidence, and the
  rejected attempt must not be retried/tuned merely to obtain a passing
  result.
- Formal M1 operational acceptance remains **BLOCKED** (deployment authority
  gap, see §-1 below — unchanged by this decision).
- M1.10 formal soak remains `OPERATOR-WAIVED`. WAIVED != PASSED, WAIVED !=
  OPEN BLOCKER.
- Additional alpha discovery is now `OPERATOR-DEFERRED`.

**What this decision authorizes:**

- Stage B / M2 CODE COMPLETION work may proceed now, independent of the open
  M1 operational-only gate, using the canonical M2 requirement set in
  `MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md`.
- Later-milestone code may be implemented and tested without that implying
  the corresponding milestone is operationally accepted.

**What this decision does NOT authorize:**

- M1 must not be called formally `CLOSED`.
- Paper must not be promoted or forced ready.
- Live must not be enabled.
- No alpha discovery, no retry/tune of the rejected `intraday_scalper`
  evidence, no Paper/Live operational ceremony under this controller.

### Stage B / M2 result (2026-09-18, CORRECTED 2026-09-18, `V4-STAGE-B-M2-REPAIR-01`)

**The original 9/9 CODE_CLOSED conclusion below was REJECTED by
independent review and is superseded.** It is preserved as audit history
in `docs/V4_CODE_COMPLETION_MANIFEST.md` (struck through in place, not
deleted).

~~Bounded census against canonical M2 found all 9 required capabilities
already CODE_CLOSED... 137 targeted tests... no production/test code was
changed.~~

**Confirmed gaps (independent review, 2026-09-18):**

1. **M2.3 = WIRING_MISSING.** Bundle 6 (`runtime_strategy_conflict.rs`) is
   real and correctly resolves conflicting same-symbol inputs, but no
   production producer can ever hand it two genuine same-symbol
   economically-active strategy decisions in one cycle:
   `mqk-portfolio/src/dynamic_selection.rs:29` selects "exactly one
   candidate per symbol, or none — never more"; `StrategyHost` refuses a
   second concurrent strategy registration
   (`StrategyHostError::MultiStrategyNotAllowed`,
   `scenario_parallel_long_short_strategy_01.rs::p11_strategy_host_enforces_single_strategy`).
   Remaining work: `MULTI-STRATEGY-RUNTIME-DISPATCH-01`.
2. **M2.7 = WIRING_MISSING.** The authoritative autonomous completed-bar
   production driver, `resolve_single_effective_binding`
   (`state/autonomous_completed_bar_driver.rs:329-333`), only ever resolves
   exactly one binding and explicitly documents multi-symbol assignment as
   unsupported (fails closed). The durable per-assignment claim identity
   foundation already exists (`sys_autonomous_daily_bar_dispatches`, keyed
   by `(operation_id, local_symbol, timeframe, bar_end_ts)`) and must be
   reused, not replaced.
3. **M2.1 = PARTIAL / WIRING_MISSING** (downstream of #1 — multiple
   *symbols* with distinct single strategies works; multiple
   economically-active *strategies on the same symbol* does not).

All other requirements (M2.2, M2.4, M2.5, M2.6, M2.8, M2.9, and the five
additional invariants) are unaffected by this correction and retain
`CODE_CLOSED`.

```text
M2 CODE COMPLETION:        CODE_CLOSED 6/9, WIRING_MISSING 2/9 (M2.3, M2.7),
                            PARTIAL/WIRING_MISSING 1/9 (M2.1)
M2 OPERATIONAL ACCEPTANCE:  NOT CLAIMED (unchanged)
```

Remaining work tracked under `V4-STAGE-B-M2-REPAIR-01`:
- **R1** — `MULTI-STRATEGY-RUNTIME-DISPATCH-01`: wire genuine same-symbol
  multi-strategy dispatch into the existing Bundle 6 conflict authority.
- **R2** — extend the autonomous completed-bar driver to support multiple
  configured symbol/timeframe assignments, reusing
  `sys_autonomous_daily_bar_dispatches` identity.
- **R3** — run the existing `scenario_strategy_decision_idempotency_01`
  restart/replay proof (M2.6 load-bearing check not run in the original
  census).
- **R4** — one integrated M2 finish-line scenario combining R1+R2 with
  retry/restart and cross-contamination negative controls.

Full detail and citations: `docs/V4_CODE_COMPLETION_MANIFEST.md`
§ "M2 Code Completion Manifest" (original census + correction note +
per-requirement corrections).

---

## -1. M1.9 Deployed-State Verification Result (2026-09-19, READ-ONLY; CORRECTED 2026-09-19)

A bounded, read-only M1.9 verification was performed against the actual
deployed Paper system (daemon PID 16680, `mqk-paper-postgres` /
`miniquantdesk_paper` on port 5440, live status API on 127.0.0.1:8899).
Full evidence: `C:\Users\Zacha\Downloads\MQD_M1_9_DEPLOYED_STATE_REVIEW\`.

Seven of eight sub-items are truthfully verified and internally
consistent: correct Paper DB, correct deployment mode/adapter
(`paper`/`alpaca`), `live_routing_enabled=false`, provider data freshness
(AAPL 5m latest completed bar `2026-09-17T16:20:00Z`, stale only because
the runtime has been disarmed/halted since then — not a separate
provider defect), scheduler registration (task `Ready`, correct
action/arguments), and risk/arm/reconcile truth
(`sys_arm_state.state=DISARMED` reason `DeadmanSupervisorFailure` since
`2026-09-17T16:33:50Z`, `reconcile_status=ok`, 0 mismatches, no active
risk block). A halted/disarmed state is treated as truthful, not as an
M1.9 failure, per the mission's own acceptance rule.

### CORRECTION (M1-DEPLOYED-PROMOTION-AUTHORITY-01, 2026-09-19)

The **deployed-universe/promotion-authorization** sub-item was
previously recorded as `UNKNOWN_NEEDS_PROOF` on the theory that
native/built-in strategies might be authorized through a path other
than `sys_strategy_promotion_transitions`. An independent review found
this classification error: current production code makes the outcome
deterministic, and that theory is contradicted by the code itself.

Full evidence: `C:\Users\Zacha\Downloads\MQD_M1_9_PROMOTION_AUTHORITY_REVIEW\`.

Five invariants were inspected and cited against current HEAD (`05_code_authority.txt`), all CONFIRMED:
1. `submit_internal_strategy_decision` Gate 3b unconditionally invokes
   `evaluate_paper_promotion_gate` for every strategy_id
   (`mqk-daemon/src/decision.rs:801-828`).
2. `registered + enabled` in `sys_strategy_registry` is explicitly
   documented and enforced as insufficient
   (`mqk-daemon/src/promotion_gate.rs:11-16`; Gate 3 and Gate 3b are
   separate, sequential gates).
3. Only an exact `(strategy_id, symbol, timeframe_secs)` match with
   current state `active_paper` (not expired, already effective)
   authorizes trading (`mqk-db/src/strategy_promotion.rs:989-1019`).
4. Absence of a promotion record returns `paper_tradable=false`,
   `reason_code=promotion_missing`
   (`mqk-db/src/strategy_promotion.rs:993-995`) — confirmed live.
5. There is **no** special native/built-in bypass for `intraday_scalper`
   or any `kind=native` strategy; the registry's `kind` field is never
   read by the promotion gate.

The exact deployed identity was resolved read-only:
`strategy_id=intraday_scalper`, `symbol=AAPL`, `timeframe_secs=300`
(`01_runtime_identity.txt`; `configured_fleet_size=1`,
`runtime_execution_mode=single_strategy` — this is the ONLY runtime
fleet member).

The read-only truth surface `GET /api/v1/strategy/promotions/check` was
queried for that exact identity (no POST transition route called):
`tradable_paper=false`, `reason_code=promotion_missing`,
`current_state=null` (`02_promotion_check.txt`). `GET
/api/v1/strategy/promotions` additionally confirms **zero** promotion
rows exist for any identity system-wide.

Promotion-evidence availability was checked against the configured
review-artifact root (`exports/strategy_reviews/`, default path): the
only artifact present is for a different strategy (`swing_momentum`),
scored 0 `paper_candidate` results out of 88, and has no entry for
`intraday_scalper`/`AAPL` at all (`03_existing_promotion_evidence.txt`).
Classification: **NO_VALID_PROMOTION_EVIDENCE**.

The 40 non-`intraday_scalper` `sys_strategy_registry` rows were
bounded-classified: all 40 trace to exact test-fixture call sites
(`unique_id(...)` in `scenario_internal_strategy_decision.rs`,
`scenario_suppress_strategy.rs`, and
`scenario_sector_risk_gate_etf_risk_closure_01.rs`), carry zero
promotion/signal-eval references, and are not runtime fleet members.
Classification: **CONFIRMED_TEST_RESIDUE** for all 40
(`04_registry_residue_classification.txt`).

**Corrected result:**
- **CODE DEFECT:** none established by this finding — the promotion
  gate enforces its documented invariant correctly and identically on
  both the write path (Gate 3b) and the read-only observability path.
- **DEPLOYMENT AUTHORITY GAP:** **CONFIRMED**. The deployed
  `intraday_scalper`/`AAPL`/300 identity cannot create a new Paper
  outbox order through the canonical internal decision path without an
  `active_paper` promotion, no such promotion exists, and no valid
  promotion evidence exists to create one through the normal transition
  path. `intraday_scalper` was seeded directly into the enabled runtime
  fleet ("Seeded for autonomous paper trading startup") without ever
  being run through the promotion pipeline the code requires before it
  may actually trade.
- **M1.9:** corrected from `UNKNOWN_NEEDS_PROOF` to verified-sufficient
  to establish the deployed mismatch — not a residual unknown, a
  confirmed gap.

Also observed (informational, not diagnosed further — out of read-only
scope): today's (`2026-09-18`) autonomous daily operation
(`352ea5d7...`) never started a run (`start_attempt_count=0`) and was
closed to `evidence_degraded` at end-of-day rollover, consistent with
the runtime remaining disarmed for the entire session window rather
than a separate scheduling defect.

**Formal M1 closure status: BLOCKED.** Independent of the Stage A
code-completion acceptance recorded in §0 below, formal M1 is blocked
until the deployed Paper strategy has truthful promotion authority (an
`active_paper` promotion for `intraday_scalper`/`AAPL`/300, created
through a validated evidence bundle via
`POST /api/v1/strategy/promotions/transition`) or is removed from the
deployed trading universe by an explicit, valid operator decision. M2
remains **NOT AUTHORIZED** by this document.

M1.10 remains `OPERATOR-WAIVED`. WAIVED != PASSED. WAIVED != OPEN
BLOCKER. This is unrelated to and unaffected by the M1.9 correction
above.

No Paper/Live/runtime state was modified during this verification or
this correction: no re-arm, no halt clear, no daemon restart, no
scheduled-task mutation, no Discord test, no order submission, no
`.env.local` edit, no `smoke_logs/` access, no promotion-transition
endpoint call.

---

## 0. Stage A M1 Code-Completion Census — Current Status (2026-09-18)

Branch:

```text
v4-bulk-code-completion-stage-a-m1-01
```

Baseline HEAD (ancestor):

```text
f0e16651da74cc4a26726ae02321315618855a22
```

**Stage A bounded census completed locally.** Gap census result (bounded, requirement-driven inspection of M1-critical seams — not an exhaustive repo audit):

```text
CODE_MISSING:      0
WIRING_MISSING:    0
TEST_MISSING:      0
OPERATIONAL_ONLY:  1
  - Actual deployed Paper DB/config/provider/universe/scheduler/risk-state verified
```

Genuine Paper trade lifecycle and genuine no-trade lifecycle were both
originally listed here as still-unproven OPERATIONAL_ONLY items. They are
not: both have already been observed end to end against a real Alpaca
Paper broker during live market hours and are recorded as CLOSED_LOCAL in
committed closure decisions (`docs/specs/paper_trade_lifecycle_proof_02_fast_market_hours_retry.md`,
`docs/specs/paper_daily_pnl_capture_01e_closure_decision.md`,
`docs/specs/paper_order_lifecycle_visibility_01e_closure_decision.md`,
`docs/specs/auton_no_trade_02c_market_hours_closure_decision.md`,
`docs/specs/market_hours_proof_sweep_01e_closure_decision.md`). See
`docs/V4_CODE_COMPLETION_MANIFEST.md` M1.7/M1.8 for the full evidence
chain. They are not re-demanded here absent a new deterministic
contradiction.

Full detail: `docs/V4_CODE_COMPLETION_MANIFEST.md`. Its citations were
independently spot-checked (`V4-STAGE-A-M1-CLOSEOUT-02`) against several
of the original census's table names, file names, and line numbers that
did not match the repo; the manifest body has since been rewritten to
cite the verified current locations directly (`V4-STAGE-A-M1-DOC-TRUTH-REPAIR-01`).
No classification changed as a result of that citation repair.

Formal M1 soak requirement:

```text
OPERATOR-WAIVED
```

WAIVED does not mean PASSED (no claim of 10/10 or 5/5 sessions is made),
and it does not mean OPEN BLOCKER either (no further multi-day soak is
required before advancing).

**This status explicitly is NOT:**
- independent acceptance of Stage A;
- formal M1 closure (M1 is not formally closed merely because
  code-completion gaps are zero — the one remaining OPERATIONAL_ONLY item,
  actual deployed Paper state verification, is open against full M1
  closure; the operator-waived soak is neither passed nor an open
  blocker);
- authorization to push to origin (push requires independent review);
- authorization to begin M2 (M2 is not authorized by this controller).

Discord-notification provenance investigation (read-only, V4-STAGE-A-M1-CLOSEOUT-02): see the Stage A review bundle (`C:\Users\Zacha\Downloads\MQD_STAGE_A_M1_REVIEW\07_discord_provenance.md`). No deterministic M1 defect was found; verdict and detail are in that file. No Paper/Live/runtime state was modified during that investigation.

---

## 1. Prior Checkpoint — M1-LINEAGE-SAME-RUN-RECOVERY-01 (context, not current HEAD)

The section below predates the Stage A M1 census and was written against `main` at the lineage-repair commit. It is retained as prior-state context; section 0 above is current truth for the active branch.

Local branch (at the time this section was written):

```text
main
```

Local HEAD after the latest lineage repair:

```text
f0e16651da74cc4a26726ae02321315618855a22
```

Commit subject:

```text
fix(autonomous): preserve lineage across same-run recovery
```

Remote `origin/main` was still at:

```text
554faa20d770325e22d12ed08ecbd98f63d1122e
```

at the time this checkpoint was written.

Latest patch status:

```text
M1-LINEAGE-SAME-RUN-RECOVERY-01
STATUS: LOCALLY COMPLETE
PUSH: NO
INDEPENDENT ACCEPTANCE: PENDING
```

---


## 2. Current Machine / Runtime State

**HISTORICAL (superseded) — shutdown snapshot as previously recorded, no longer current:**

```text
MiniQuantDesk-Paper-Preopen-Startup:  STOPPED + DISABLED
mqk-daemon:                           STOPPED
mqk-cli/cargo/rustc:                  NONE RUNNING
MQD GUI/node helpers:                 STOPPED
MQD Docker containers:                mqk-test-postgres / mqk-live-postgres / mqk-paper-postgres STOPPED
HEAD (at time of that snapshot):      f0e16651 (main)
```

**Fresh read-only capture, 2026-09-19T00:36:35Z** (this repair turn;
read-only inspection only — no daemon start/stop/restart, no re-arm, no
halt clear, no scheduled-task modification, no Discord test, no
Paper/Live state mutation):

```text
MiniQuantDesk-Paper-Preopen-Startup:
  PRESENT, State=Ready (Get-ScheduledTask) — this is the enabled/idle
  state, not Disabled. This contradicts the historical snapshot above.
  Get-ScheduledTaskInfo (last-run/next-run detail) errored on this box
  and could not be read this turn — treat last/next run time as
  UNAVAILABLE, not absent.

mqk-daemon:
  PRESENT / RUNNING — PID 16680, image
  C:\Users\Zacha\Desktop\MiniQuantDeskV4\core-rs\target\release\mqk-daemon.exe,
  StartTime 2026-09-15T11:42:29 (local), listening on 127.0.0.1:8899
  (TCP connect succeeds). This contradicts the historical "STOPPED" entry
  above.

mqk-daemon read-only API (GET /api/v1/system/status on :8899):
  UNAVAILABLE — TCP connects but the HTTP request did not return within
  15s (timed out). Runtime/halt/kill-switch/reconcile/session-window
  truth could not be read this turn; do not assume any value for it.

mqk-cli/cargo/rustc:
  NONE RUNNING (checked by name; consistent with historical snapshot).

MQD GUI/node helpers:
  PRESENT — 14 `node` processes running (oldest since 2026-09-15, newest
  since 2026-09-18T14:30). This contradicts the historical "STOPPED"
  entry above. Not identified further this turn (which dev server/task
  owns each PID is UNAVAILABLE without additional read-only inspection).

MQD Docker containers:
  UNAVAILABLE — Docker Desktop application processes are present
  (`Docker Desktop`, `com.docker.backend`) but `docker ps` itself fails
  ("Docker Desktop is unable to start"). Container status for
  mqk-test-postgres / mqk-live-postgres / mqk-paper-postgres could not be
  read this turn; do not assume STOPPED or RUNNING for any of them.

Git index lock:
  NONE (Test-Path on .git/index.lock = False).

Git worktree (this branch, this turn):
  Two intentional modifications from this doc-repair patch
  (docs/CURRENT_MISSION.md, docs/V4_CODE_COMPLETION_MANIFEST.md); no
  other changes.

HEAD:
  c2ed45cb (v4-bulk-code-completion-stage-a-m1-01) — unchanged by this
  read-only capture.
```

This turn's inspection did **not**:
- touch `smoke_logs/`;
- run `git clean`, `git reset`, or `git stash`;
- delete evidence/artifacts or Docker containers;
- start, stop, or restart the daemon;
- re-arm, clear a halt, or modify any scheduled task;
- send a Discord test;
- mutate any Paper/Live state.

Operator: the daemon-process and scheduled-task findings above
contradict the previously recorded shutdown state. Do not assume Paper
auto-start is currently disabled based on the historical block — verify
directly (e.g. via a working `docker ps` and a responsive daemon status
route) before relying on either the historical or this turn's partial
capture for an operational decision.

---

## 3. Operator Decision — M1 Soak

The formal M1 soak requirement is:

```text
OPERATOR-WAIVED
```

Do not claim:
- 10/10 countable sessions;
- 5/5 clean sessions;
- soak passed.

Do not require another 10-day or 30-day soak before advancing.

The replacement exit criterion is a bounded proof that the Paper core path can stay alive and trading-capable through the normal production path.

---

## 4. Confirmed Production Defect That Was Just Patched

2026-09-15 Paper operation:

```text
operation_id:
91626a1d-7c85-50ac-b749-66f33c124d25

run_id:
79a49ab4-a6ae-5332-95bf-92f7541beeec
```

Authoritative event history showed:

```text
seq 46:
start_retrying -> running(A)

seq 47:
running -> controller_degraded
reason=interior_gap

seq 48:
controller_degraded -> running(A)
same live runtime, same run_id
```

Later finalization failed:

```text
stopping -> evidence_degraded
reason=unknown_run_lineage_unavailable
```

Root cause:

The coordinator intentionally allowed `controller_degraded -> running` recovery with the same still-live `run_id`, while the lineage validator treated any repeated `run_id` among `to_state='running'` events as `DuplicateRunId`.

That was an internal contract contradiction.

Patch `f0e16651...` changes lineage semantics so same-live-run controller recovery does not manufacture a second logical runtime-generation member.

---

## 5. Patch Proof Status

Agent-reported focused result:

```text
14 pure lineage tests passed
```

Required negative controls reported covered:

1. initial start + same-run controller recovery => lineage `[A]`;
2. real recovery to new run => lineage `[A,B]`;
3. true duplicate runtime-generation run ID remains rejected;
4. current-run mismatch remains rejected;
5. production same-run recovery no longer fails solely because the same run was restored to running.

`rustfmt --check`:

```text
PASS
```

`git diff --check`:

```text
PASS
```

No push performed.

---

## 6. Known Test-Environment Blocker

The local test Postgres at:

```text
127.0.0.1:5434 / mqk_test
```

currently reports migration checksum drift:

```text
migration 6 was previously applied but has been modified
```

This blocks the DB-backed tests in the focused lineage scenario, including the new DB-backed `h03` proof.

Agent reported:
- 26 non-DB tests passed;
- 22 DB-backed tests failed identically on the migration-checksum drift;
- the DB-backed regression test compiles but did not execute successfully.

Do not claim DB-backed proof passed.

Do not modify historical migrations merely to make the tests green.

The test-DB environment issue is separate from the same-run lineage code invariant.

---

## 7. Current Paper Runtime Truth Before Recovery

Last captured Paper truth:

```text
environment=paper
adapter_id=alpaca
live_routing_enabled=false

runtime_status=halted
kill_switch_active=true
integrity_halt_active=true

reconcile_status=ok
mismatched_positions=0
mismatched_orders=0
mismatched_fills=0
unmatched_broker_events=0

current run status=HALTED
OMS outbox rows=0
OMS inbox rows=0
```

The canonical ops catalog reported:

```text
clear-halted-run = enabled
```

The current operation was:

```text
state=evidence_degraded
reason=unknown_run_lineage_unavailable
```

Market data itself was fresh when last checked:

```text
AAPL / 5m
completed_rows=9916
latest completed bar=2026-09-15T14:45:00Z
freshness=OK
```

Required-universe had shown short oscillations around new 5-minute boundaries:

```text
ready
-> expected_latest_bar_missing
-> ready
```

These may be provider-publication timing or a separate scheduler-timing issue. Do not weaken freshness gates without proof.

---

## 8. Session Window Truth

Observed current Paper environment:

```text
MQK_SESSION_START_HH_MM=13:30
MQK_SESSION_STOP_HH_MM=20:00
session_window_source=fixed_window_override
```

Do not silently edit `.env.local`.

If these overrides are unintended, prove that before changing them.

---

## 9. Immediate Next Mission

### Mission: independently review and operationally prove `f0e16651...`

Next actions, in order:

1. independently inspect the exact diff for commit `f0e16651...`;
2. confirm it implements only the intended same-run-lineage invariant;
3. decide whether the blocked DB-backed `h03` proof is required before acceptance or whether existing production RED + focused pure regression proof is sufficient for local acceptance;
4. if accepted, push only after explicit operator authorization;
5. deploy/restart the repaired daemon only as required;
6. use canonical Paper recovery:
   - inspect halt/run/reconcile/OMS truth;
   - `clear-halted-run` only if still enabled and safe;
   - re-arm;
   - allow canonical autonomous start/recovery;
7. return immediately to Paper monitoring.

No new soak campaign.

No new worktree.

No broad test campaign.

---

## 10. Paper Stability Exit Criteria

M1 exit readiness no longer requires 10 days.

Success means the core Paper path stays healthy continuously for at least 20 minutes, or until market close if less than 20 minutes remain, with:

```text
daemon reachable
mode=paper
adapter=alpaca
live_routing_enabled=false

kill_switch_active=false
integrity_halt_active=false
reconcile=ok

operation not manual_intervention_required
operation not controller_degraded
operation not evidence_degraded

runtime RUNNING while in session window
completed-bar task in applicable running-dispatch mode
completed-bar progress advances across completed 5m bars
strategy evaluation count increases

required-universe ready or only bounded self-healing provider-publication waits

no unresolved OMS authority issue
no unexpected task death
no deterministic blocker remaining
```

Orders/fills are **not required** for this stability proof if the strategy truthfully emits no trade.

A truthful no-trade decision is not an infrastructure failure.

---

## 11. After M1 Exit

Once the Paper core path is stable:

1. stop adding infrastructure;
2. close M1 with the soak recorded as operator-waived;
3. expand the Paper universe;
4. move to real strategy evaluation / alpha work;
5. validate actual Paper trading behavior.

Frozen first expanded Paper-universe target discussed:

```text
SPY
QQQ
NVDA
TSLA
AAPL
```

Do not encode that as a comma-separated `MQK_STRATEGY_SYMBOL`; the current legacy env path is single-symbol.

Use the repo's approved multi-symbol/watchlist path only after the remaining production wiring/registry restrictions are repaired.

---

## 12. Current Workflow Rules

Follow:

```text
ONE WRITER
ONE BLOCKER
ONE PATCH
ONE TARGETED PROOF
ONE COMMIT
RETURN TO PAPER
```

Production failures count as RED evidence.

Do not:
- create another worktree by default;
- run redundant standalone builds;
- run `cargo test --workspace` during blocker repair;
- broaden into GUI/infrastructure work;
- start another multi-day soak;
- touch `smoke_logs/`;
- enable Live.

---

## 13. Next Response Should Start With

```text
VERIFIED HEAD:
CURRENT BLOCKER:
NEXT ACTION:
```

Then act on the smallest load-bearing next step.
