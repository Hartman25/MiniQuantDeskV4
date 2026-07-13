# Roadmap Completion Reconcile — 01

Patch ID: `ROADMAP-COMPLETION-RECONCILE-01`

Docs-only. Reconciles the multi-asset roadmap
(`docs/audits/multi_asset_completion_audit.md`) after this session's
`BACKTEST-MULTIPLIER-MARGIN-01` audit/closure work
(`BACKTEST-MULTIPLIER-MARGIN-01-COMPLETION-AUDIT-01` →
`-SAFE-GAP-CLOSURE-01` → `-CLOSURE-OR-BOUNDARY-DECISION-01`). No code
changed by this document. Percentages below are carried from
`multi_asset_completion_audit.md`'s existing per-item table except where
this session found direct repo evidence of drift, in which case the
evidence is cited inline — this doc does not re-derive a fresh audit for
items outside this session's scope.

---

## 1. Status categories used below

- **Closed foundation** — the item's own stated scope (schema/model/validator/docs, not production consumption) is done.
- **Backtest-complete** — multiplier/margin economics work in the backtest lane only; live/paper accounting is explicitly untouched.
- **Production-consumption-open** — the foundation exists but no trading/execution/risk/OMS/ingestion path reads it as truth.
- **Missing execution/risk/strategy** — no execution, risk, or strategy code exists for this asset class at all.

## 2. Per-item status

| Item | Status | Category | Exact blocker |
|---|---|---|---|
| `ASSET-CORE-01` | `CLOSED_LOCAL / FOUNDATION-COMPLETE` | Closed foundation | None for its own scope. Production consumption is a separate, explicitly-named boundary (`ASSET-CORE-01H`) — no trading/execution/risk/OMS/ingestion path reads `InstrumentRegistryV2`; v1 `equities.json` remains sole trading truth. |
| `ASSET-CORE-02` (Multi-Asset Order Intent Model) | `CLOSED_LOCAL / PARTIAL` (per `ASSET-CORE-02-ORDER-INTENT-V2-FOUNDATION-01-COMBINED`; see `docs/specs/asset_core_02_03a_current_completion_audit.md` for current-session re-audit) | Production-consumption-open | `OrderIntentV2`/`ExecutionIntentV2` are hardened and validated but explicitly unwired; zero production caller anywhere. Confirmed zero bracket/OCO representation as of this re-audit (see §8/§14 of the linked audit) — the one concrete remaining model gap. |
| `ASSET-CORE-03` (Asset-Aware Risk Router) | `CLOSED_LOCAL / POLICY-BOUNDARY-COMPLETE` (per `ASSET-CORE-03-RISK-ROUTER-FOUNDATION-01-COMBINED` + `ASSET-CORE-03B-ASSET-RISK-POLICY-SAFE-GAP-CLOSURE-01`; see `docs/specs/asset_core_02_03a_current_completion_audit.md` and `docs/specs/asset_core_02_03e_closure_decision.md`) | Production-consumption-open, `ASSET-CORE-04`-dependent for graduated live enforcement | Two tested fail-closed gates exist (Gate 0, routing guard) **plus** a graduated per-class static policy model (`mqk_execution::asset_risk_policy`) — this corrects the "zero graduated per-class policy behind them" claim previously recorded on this line. The policy model is now also operator-surfaced read-only via `GET /api/v1/system/asset-risk-policy` (`ASSET-CORE-03B`). True live enforcement (margin/multiplier/NAV blocking real orders) still requires `ASSET-CORE-04` live accounting, not yet wired. |
| `ASSET-CORE-04` (Multi-Asset Portfolio Ledger) | `PARTIAL` — dual-axis: live ledger ~20% (unchanged), read-only economics scaffold (`04A`-`04F`) substantially more complete | Production-consumption-open | The live-path ledger itself (`mqk-portfolio::accounting.rs`) is still single-currency, FIFO-P&L-multiplier-naive, and `PositionSnapshot.net_qty` is a whole-unit `i64` — fractional crypto quantities are not constructible through any route. Separately, an additive, zero-live-caller economics model chain (`ASSET-CORE-04A` instrument economics model → `04B` registry-v2 bridge → `04C` multi-asset NAV aggregation → `04D` read-only status route → `04F` registry-v2 seam) is built, tested, and daemon-exposed, but none of it is wired into `mqk-portfolio`'s actual accounting path or any live/paper order flow. This session confirms (via the `BACKTEST-MULTIPLIER-MARGIN-01` closure decision, §5) that `mqk-portfolio` remains untouched by the backtest-economics lineage too — the two economics scaffolds (backtest and portfolio-status) are parallel and neither reaches live accounting. Not touched this session — `mqk-portfolio/*` was forbidden. |
| `ASSET-CORE-05` (Market Calendar & Session Provider) | `PARTIAL` (~38%, up from ~35%) | Production-consumption-open | `MarketCalendarProvider` trait + fail-closed fallback + read-only session-profile diagnostics (`equity_us_regular`/`crypto_continuous`/`futures_globex`/`forex_24x5`, daemon route + GUI panel) exist and are tested. `ASSET-CORE-05-PER-INSTRUMENT-SESSION-ROUTING-01-COMBINED` (`05G`-`05J`) closed the remaining safe model/parity gap: a composed, reusable, pure `route_instrument_session_for_metadata` helper now exists (closing the prior duplication between `resolve_session_profile_for_instrument_metadata` and the per-profile classifier), the `"rate"` asset class is now explicitly routed instead of silently falling through to `Unknown`, and every canonical v2 asset class is covered by a dedicated parity test proving equity/ETF matches real production truth and non-equity always reports model-only. A true production per-instrument admission decision point, authoritative non-equity calendars, and any use of non-equity profiles in trading/admission remain unwired — see `docs/specs/asset_core_05j_session_routing_closure_decision.md` §8. |
| `BACKTEST-MULTIPLIER-MARGIN-01` | `CLOSED_LOCAL / BACKTEST-COMPLETE` (closed this session) | Backtest-complete | None for backtest economics. Margin enforcement and a real non-equity production registry-v2 data source are explicitly deferred, separate items — not blockers to this label's own closed scope. See `docs/specs/backtest_multiplier_margin_01_closure_decision.md`. |
| `CRYPTO-REGISTRY-01` (Crypto asset registry) | `PARTIAL` (~28%, unchanged this session) | Production-consumption-open | Two disabled registry-v2 fixture rows (`BTC/USD`, `ETH/USD`) exist, validated, bridged through `ASSET-CORE-04B`; zero production registry-v2 callers anywhere. Not touched this session. |
| `CRYPTO-DATA-01` (24/7 market ingestion) | `PARTIAL` (~32%, unchanged this session) | Production-consumption-open | Local-CSV and DB-backed local-mark ingestion proven for `BTC/USD`/`ETH/USD`; a fixture-first Kraken OHLCV parser/adapter/CLI/sync/scheduler-readiness chain is built and read-only-status-surfaced, but the `kraken` provider remains disabled by default, no recurring ingestion or scheduler registration exists, and `sync-provider`/`ingest-provider` have no Kraken path. Not touched this session (would require live network calls, forbidden by this session's hard safety rules). |
| `CRYPTO-RISK-01` | `MISSING` (0%, unchanged) | Missing execution/risk/strategy | No spread-gate, no counterparty-risk model. Not touched — `mqk-risk/*` forbidden this session. |
| `CRYPTO-EXEC-01` | `MISSING` (0%, unchanged) | Missing execution/risk/strategy | Alpaca adapter never calls `/v2/crypto/*` (confirmed by direct source read, `mqk-broker-alpaca/src/lib.rs`). Not touched — `mqk-broker-*` forbidden this session. |
| `CRYPTO-STRAT-01` | `MISSING` (0%, unchanged) | Missing execution/risk/strategy | Depends on `CRYPTO-EXEC-01`; no strategy code exists. Not touched this session. |
| `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` | Not started (decision-only patch, not yet written) | Production-consumption-open (this *is* the boundary-crossing decision point) | Blocked on two of five `ASSET-CORE-01H` §5 prerequisites: (1) `BACKTEST-MULTIPLIER-MARGIN-01` closed — **satisfied**; (2) symbol/`instrument_id` translation layer — **satisfied** by `REGISTRY-V2-TRANSLATION-01A`-`01D` (pure fail-closed `RegistryV2SymbolTranslationIndex`, proven collision-free and round-trippable across the full 88-row equity universe, zero production callers); (3) Gate 0 / broker-submit routing-guard parity re-verification against `InstrumentRegistryV2::asset_class` — **satisfied** by `REGISTRY-V2-GATE-PARITY-01A`-`01D` (pure fail-closed `registry_v2_gate_asset_class` helper, 20 regression tests proving Gate 0 and the routing guard reject the same asset classes whether keyed off `mqk_schemas::AssetClass` or `InstrumentRegistryV2::asset_class`, zero production callers, neither gate modified); (4) a live-network-verified non-equity market-data provider end-to-end into `md_bars` — **satisfied** by `REGISTRY-V2-KRAKEN-LIVE-PROVIDER-PROOF-01`: after explicit operator authorization, `mqk md kraken-ohlc-ingest` made one live Kraken OHLC network call each for `BTC/USD`/`ETH/USD` (`--timeframe 1D`) and wrote 720 real completed bars each into `md_bars` in the isolated `mqk_test` proof database, confirmed via evidence JSON and direct `psql` query, with zero rows in the paper database and no config/enablement change; (5) an explicit operator enablement decision for a named non-equity instrument — **decision now made** by `REGISTRY-V2-INSTRUMENT-ENABLEMENT-01-BTC-USD-DECISION-01` (`BTC/USD` explicitly named), but **not yet implemented**: the config flag itself remains `enabled=false`, since flipping it would require the schema's test-only `allow_enabled_non_equity_for_testing` escape hatch (discovered during this review to carry no production path) and zero production code reads `InstrumentRegistryV2` regardless; a separate, explicit authorization is needed to actually implement the flag change. |

## 3. Next best patch

**Update (`REGISTRY-V2-TRANSLATION-01A`-`01D`, this session's later work):**
prerequisite #2 — the symbol/`instrument_id` translation layer between
`InstrumentRegistryV2` and the existing symbol-string-keyed tables
(`md_bars`, outbox rows, portfolio positions) — is now **satisfied**. A
pure, fail-closed `RegistryV2SymbolTranslationIndex` was built
(`core-rs/crates/mqk-md/src/instrument_registry_v2.rs`) and proven
collision-free and round-trippable across the full 88-row production
equity universe via a read-only CLI (`mqk md registry-v2-translation-check`);
see `docs/specs/registry_v2_translation_01d_closure_decision.md` for the
full closure decision. Zero production paths consume it.

**Update (`REGISTRY-V2-GATE-PARITY-01A`-`01D`, this session's later work):**
prerequisite #3 — Gate 0 / broker-submit routing-guard parity
re-verification against `InstrumentRegistryV2::asset_class` — is now
**satisfied**. A pure, fail-closed `registry_v2_gate_asset_class` helper was
built (`core-rs/crates/mqk-md/src/instrument_registry_v2.rs`) and 20
regression tests (`scenario_registry_v2_gate0_parity_01c.rs`,
`scenario_registry_v2_routing_guard_parity_01c.rs`) proved it classifies
every `InstrumentRegistryV2.asset_class` string identically to Gate 0's and
the routing guard's actual, already-tested behavior — equity allowed,
every other canonical class rejected, malformed/unknown input fails
closed, and `"rate"` (no `mqk_schemas::AssetClass` counterpart) is
unconstructable through the routing guard's closed enum. Neither gate was
modified; zero production paths consume the helper. See
`docs/specs/registry_v2_gate_parity_01d_closure_decision.md` for the full
closure decision.

**Update (`REGISTRY-V2-LIVE-PROVIDER-01A`-`01D`, this session's later work):**
prerequisite #4's *boundary decision* — not prerequisite #4 itself — is now
**satisfied**. `01A` audited every non-equity provider candidate and
selected Kraken (`BTC/USD`/`ETH/USD`, `1D`, via the existing, tested
`mqk md kraken-ohlc-ingest` CLI command) as the safest first live-proof
candidate: no credentials required, a network call already fail-closed
behind `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1`, and an existing DB-writing path
into `md_bars`. `01B` named the exact allowed future command, the required
isolated proof/test database target (never paper/live), the exact
verbatim operator authorization phrase required before any future network
call, and the exact evidence fields required to close prerequisite #4.
`01C` added a purely local, no-network, no-DB preflight guard
(`scripts/guards/validate_registry_v2_live_provider_01c_preflight.ps1`)
proving `01B`'s boundary decision is complete. `01D` decided the boundary
is `CLOSED_LOCAL` while explicitly keeping prerequisite #4 itself `OPEN`
until a future, separately-authorized live proof actually runs. See
`docs/specs/registry_v2_live_provider_01d_boundary_closure_decision.md`
for the full closure decision. Zero network calls, zero DB access, and no
trading enablement occurred in any of `01A`-`01D`.

**Update (`REGISTRY-V2-KRAKEN-LIVE-PROVIDER-PROOF-01`, this session's later
work):** prerequisite #4 itself is now **satisfied**. After the operator
gave the exact authorization phrase named in `01B`/`01D`, `mqk md
kraken-ohlc-ingest --timeframe 1D` was run once each for `BTC/USD`/
`ETH/USD` with `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1` against the isolated
local `mqk_test` database (port 5434, confirmed distinct from the paper
database at port 5440). Both runs made one live HTTP GET each to Kraken's
public, credential-free endpoint and wrote 720 real completed bars each to
`md_bars` with truthful `provider_id="kraken"` metadata — confirmed via
evidence JSON and independent `psql` query. Post-proof checks confirmed
zero rows in the paper database, byte-identical `providers.json`/registry
fixture, and no scheduled task. See
`docs/specs/registry_v2_kraken_live_provider_proof_01_closure_decision.md`
for the full closure decision.

Only prerequisite #5 (explicit operator enablement of a specific,
named non-equity instrument) now remains before
`REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` can be written. That
prerequisite was **not** attempted, decided, or inferred by this proof —
`kraken.enabled` and both crypto fixture rows' enablement flags remain
`false`.

**Do not** recommend `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` itself
next — prerequisite #5 remains open and is an explicit operator decision,
not something this session may infer or take on the operator's behalf;
recommending the cutover-decision patch now would be the exact "stale
recommendation pointing to already-closed work" pattern this document is
required to avoid the opposite of (recommending unready work as if it were
ready).

**Update (`REGISTRY-V2-INSTRUMENT-ENABLEMENT-01-BTC-USD-DECISION-01`, this
session's later work):** prerequisite #5's *decision* — not its
*implementation* — is now made. After explicit operator authorization for
a decision-only review, `BTC/USD` was named as the first non-equity
instrument for eventual `enabled=true` status. This review confirmed (by
direct source read) that zero production paths read `InstrumentRegistryV2`
today, so the flag change would affect no trading behavior regardless, and
surfaced that `validate_registry_v2` fail-closed requires pairing
`enabled=true` on any non-equity instrument with the test-only
`allow_enabled_non_equity_for_testing` escape hatch or the whole registry
file fails to load — a flag explicitly documented as carrying no
production path. Per the operator's "stop after the enablement decision
evidence" instruction, the config flag itself was **not** flipped;
`BTC/USD.enabled` remains `false`. See
`docs/specs/registry_v2_instrument_enablement_01_btc_usd_decision.md` for
the full decision record.

**Still not recommended:** `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` —
prerequisite #5's *implementation* (the actual config change) remains a
separate, distinct action requiring its own explicit authorization, and no
production consumption path for `InstrumentRegistryV2` exists regardless
of that flag's value. Recommending the cutover-decision patch now would
still be premature.

Independent of the registry-v2 boundary entirely, `ASSET-CORE-05`'s
remaining per-instrument session-routing gap is the next-closest-to-done
independent item (~35%, cheapest structurally per the original audit's own
assessment), and remains available as a lower-risk parallel track that does
not touch the registry-v2 boundary at all.

---

## 4. What this reconciliation changed vs. what it left alone

**Changed (this session, with direct repo evidence):**
- `BACKTEST-MULTIPLIER-MARGIN-01`: `MISSING/0%` → `CLOSED_LOCAL / BACKTEST-COMPLETE` (Phase A/B/C this session).
- `ASSET-CORE-05`: percentage note updated (~30% → ~35%) citing the already-committed `ASSET-CORE-05-MARKET-CALENDAR-GENERALIZE-01-COMBINED` entry, which predates this session but was not yet reflected in the roadmap table's percentage.
- `ASSET-CORE-04`: evidence column expanded to name the `04A`-`04F` additive economics scaffold explicitly (already-committed work, not previously distinguished in the roadmap table from the live-ledger gap it sits beside).

**Left alone (no new evidence gathered this session, forbidden files not touched):**
- `ASSET-CORE-02`, `ASSET-CORE-03` — percentages carried forward unchanged; `mqk-execution/*` and `mqk-risk/*` were outside this session's allowed file list.
- `CRYPTO-REGISTRY-01`, `CRYPTO-DATA-01`, `CRYPTO-RISK-01`, `CRYPTO-EXEC-01`, `CRYPTO-STRAT-01` — unchanged; closing any of these further requires either a live network call (forbidden) or broker/risk code changes (forbidden) this session.
- `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` — not started; correctly not recommended as the immediate next patch (§3).

No config flag was changed, no trading was enabled, no network or DB call
was made, and no broker/execution/risk/OMS/runtime/strategy/portfolio file
was touched by this reconciliation or by any patch in this session.

## 5. Unrelated session work (not part of this multi-asset roadmap)

A separate, later session closed `AUTON-NO-TRADE-OFFHOURS-01` (non-market-
hours durable no-trade explanation observability — ledger §13). It does not
touch any `ASSET-CORE-*`/`CRYPTO-*`/`REGISTRY-V2-*` item above and does not
change any percentage or status in this table. See
`docs/specs/auton_no_trade_offhours_01e_closure_decision.md` for that
bundle's own closure record.

A further, later market-hours session (`MARKET-HOURS-PROOF-SWEEP-01`) closed
the remaining market-hours half via `AUTON-NO-TRADE-02B`/`02C` — see
`docs/specs/auton_no_trade_02c_market_hours_closure_decision.md`. Parent
`AUTON-NO-TRADE-01` is now `CLOSED_LOCAL`. This does not touch any
`ASSET-CORE-*`/`CRYPTO-*`/`REGISTRY-V2-*` item or percentage in this table.

That same session also ran `ASSET-CORE-05K-V2-EQUITY-ACTIVE-MARKET-HOURS-PROOF-01`
— a real market-hours wall-clock confirmation that
`MQK_RUNTIME_SESSION_SOURCE=v2_equity_active` matches legacy session-open
behavior (`candidate_v2_parity_state=matched`). This *does* touch
`ASSET-CORE-05` (row above) but does not change its status or percentage:
`05K` proves wall-clock behavioral parity only, not a production cutover,
not an authoritative non-equity calendar, and not per-instrument admission
logic — the row's `PARTIAL (~38%)` / production-consumption-open verdict
stands unchanged. See
`docs/specs/asset_core_05k_v2_equity_active_market_hours_proof.md`.

## 6. Later session: `ASSET-CORE-04-LIVE-LEDGER-SECTION-CLOSURE-01-COMBINED`

A later session ran the `ASSET-CORE-04` live-ledger boundary bundle
(Phase A audit, Phase B cross-module test proof, Phase C/D both skipped as
duplicative of already-committed surfaces, Phase E closure decision — see
`docs/specs/asset_core_04_live_ledger_boundary_audit.md` and
`docs/specs/asset_core_04_live_ledger_closure_decision.md`). This *does*
touch the `ASSET-CORE-04` row above (§2, row 32) but does not change its
status, percentage, or dual-axis characterization: the live ledger remains
~20%/unchanged, and the `04A`-`04F` economics scaffold remains additive
with zero production callers — this bundle only deepened the evidence for
both halves of that existing verdict (a workspace-wide caller-map grep in
place of trusting module doc comments, and a numeric cross-module
equivalence test in place of the scaffold's isolated unit tests alone).
The row's earlier "Not touched this session — `mqk-portfolio/*` was
forbidden" note refers specifically to the session that wrote §2/§4 above,
not to this later session, which was authorized to add exactly one
test-only file under `mqk-portfolio/tests/`.

## 7. Later session: `ASSET-CORE-04-PRODUCTION-CUTOVER-DESIGN-ONLY-01-COMBINED`

A later session ran the `ASSET-CORE-04` production-cutover design bundle
(Phase A `04L` callsite audit, Phase B `04M` design spec, Phase C `04N`
test/migration plan, Phase D `04O` go/no-go decision — see
`docs/specs/asset_core_04l_production_cutover_callsite_audit.md` through
`docs/specs/asset_core_04o_cutover_go_nogo_decision.md`). This bundle is
design/decision-only: no code, no DB migration, no config flag change. It
*does* touch the `ASSET-CORE-04` row above (§2, row 32) in the sense that
it specifies exactly what a future implementation patch would need, but it
does not change the row's status, percentage, or dual-axis
characterization — the live ledger remains ~20%/unchanged, and the
`04A`-`04F` economics scaffold remains additive with zero production
callers.

**Final verdict from this bundle:**

```text
ASSET-CORE-04: PARTIAL / LIVE-ACCOUNTING-PRODUCTION-CONSUMPTION-OPEN
ASSET-CORE-04-PRODUCTION-CUTOVER-DESIGN-ONLY-01: CLOSED_LOCAL
ASSET-CORE-04 parent: PARTIAL / PRODUCTION-CUTOVER-DESIGNED-NOT-AUTHORIZED
```

No production cutover is authorized. The first recommended future patch is
`ASSET-CORE-04P-FIXED-POINT-LIVE-QUANTITY-SHADOW-MODE-01-COMBINED` (a
DB/schema design patch), gated on an explicit, per-slice operator
authorization phrase — not a blanket approval for the whole proposed
sequence. Independent alternative:
`CRYPTO-REGISTRY-DATA-COMPLETION-SWEEP-01-COMBINED`.

## 8. Later session: `INTRADAY-PROVIDER-CLOCK-SKEW-OPERATOR-GUARD-01-COMBINED`

A later session ran the intraday provider freshness-headroom operator-guard
bundle (Phase A audit, Phase B pure headroom classifier wired into
`GET /api/v1/market-data/intraday-refresh/status`, Phase C skipped as
duplicative of Phase B, Phase D `Start-PaperTradingSmoke.ps1` preflight
guard, Phase E this closure — see
`docs/specs/intraday_provider_clock_skew_01a_current_truth_audit.md` and
`docs/specs/intraday_provider_clock_skew_01e_closure_decision.md`). This
bundle does **not** touch any row in §2 above: it is not an asset-class
completion item at all, but a diagnostic/operator-visibility layer on top
of the already-existing `DATA-FRESHNESS-READINESS-GATE-01` (a paper-trade
lifecycle / market-data-operations concern, not multi-asset roadmap scope).
No row's status, percentage, or category changes as a result of this
bundle. `docs/audits/multi_asset_completion_audit.md` is correspondingly
not updated by this bundle.

A subsequent repair patch, `INTRADAY-PROVIDER-CLOCK-SKEW-01F-LIVE-EFFECTIVE-AGE-RECOMPUTE-01`
(see `docs/specs/intraday_provider_clock_skew_01f_effective_age_recompute_audit.md`
and `docs/specs/intraday_provider_clock_skew_01f_effective_age_closure_decision.md`),
fixed a gap in the Phase B classifier (it used the evidence snapshot bar age
only, not wall-clock-elapsed effective age). Same scope classification
applies: not an asset-class completion item, no §2 row changes, no
multi-asset audit update.

## 9. Later session: `PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01-COMBINED`

A later session closed the P&L operator-visibility seam
`PAPER-TRADE-LIFECYCLE-PROOF-02` exposed (a real filled paper position,
`AAPL qty=3 avg_price=314.81`, had `mark_price`/`unrealized_pnl`/
`daily_pnl` all `null` on the primary operator routes). See
`docs/specs/paper_pnl_operator_visibility_01a_current_truth_audit.md`
through `..._01e_closure_decision.md`. `mqk_portfolio::unrealized_pnl_micros`
(Phase B) was wired into `/api/v1/portfolio/positions` and
`/api/v1/portfolio/summary` (Phase C) against the same latest-completed-
`md_bars`-close mark source `/api/v1/portfolio/live-weights` already uses.
`daily_pnl` remains permanently unavailable — no day-start/previous-close
equity baseline exists anywhere in this repo's schema (confirmed by
repo-wide grep in the Phase A audit).

This bundle does **not** touch any row in §2 above: it is a paper-trade
operator-visibility/accounting-display concern (equities-only, existing
broker-snapshot layer), not an asset-class completion or production-cutover
item. No row's status, percentage, or category changes.
`docs/audits/multi_asset_completion_audit.md` is correspondingly not
updated by this bundle.

## 10. Later session: `PAPER-PNL-OFFMARKET-COMPLETION-01-COMBINED`

A follow-up off-market session closed the timeframe gap Phase D of
`PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01-COMBINED` (§9 above) identified:
the real proof-02 `AAPL qty=3 avg_price=314.81` position's completed
`md_bars` only exist at `timeframe="5m"` (6111 rows), never `"1D"`, so the
hardcoded `"1D"` default made `mark_price`/`unrealized_pnl` truthfully but
unhelpfully report `mark_unavailable`. Added an optional `timeframe` query
param to `/api/v1/portfolio/positions` and `/api/v1/portfolio/summary`,
mirroring the existing `/api/v1/portfolio/live-weights` pattern; default
`"1D"` behavior is unchanged. Proved via DB-backed route tests
(`PPV-10`..`PPV-14`) that `?timeframe=5m` resolves the real position shape
to `mark_price=$314.86`, `unrealized_pnl≈$0.15`. See
`docs/specs/paper_pnl_offmarket_01a_timeframe_gap_audit.md` through
`..._01e_closure_decision.md`.

`daily_pnl` remains permanently unavailable, unchanged from §9 — this
bundle's Phase D
(`docs/specs/paper_daily_pnl_baseline_design_only_01.md`) is a
**design-only** proposal for a future day-start/previous-close equity
baseline mechanism (recommended next patch:
`PAPER-DAILY-PNL-BASELINE-01-COMBINED`); no schema migration or
baseline-capture code was implemented in this bundle.

This bundle does **not** touch any row in §2 above, for the same reason
§9 does not: paper-trade operator-visibility/accounting-display concern
only, equities-only, existing broker-snapshot layer. No row's status,
percentage, or category changes.
`docs/audits/multi_asset_completion_audit.md` is correspondingly not
updated by this bundle.

## 11. Later session: `PAPER-DAILY-PNL-BASELINE-01-COMBINED`

A follow-up off-market session implemented the design §10 above proposed
(design-only doc `docs/specs/paper_daily_pnl_baseline_design_only_01.md`):
a new `sys_account_equity_baseline` table (migration `0045`, one
provenance-tagged row per `trading_date`), DB helpers
(`upsert_account_equity_baseline`/`fetch_account_equity_baseline_for_date`
in `mqk-db`), and read-side wiring on
`GET /api/v1/portfolio/summary` so `daily_pnl` becomes computable
(`daily_pnl_truth_state = "active"`) whenever a real baseline row exists
for the required prior NYSE trading day (found via the existing
`NyseWeekdaysProvider` seam), and stays honestly unavailable
(`"no_snapshot"` / `"db_unavailable"` / `"baseline_unavailable"` /
`"stale_baseline"`) otherwise. See
`docs/specs/paper_daily_pnl_baseline_01a_current_truth_reconcile.md`
through `..._01e_closure_decision.md`.

**Baseline capture was explicitly deferred** — no automatic, CLI, or route
write-path populates `sys_account_equity_baseline` in production; the
table is confirmed empty (0 rows) in the real local paper Postgres as of
this session. Recommended next patch:
`PAPER-DAILY-PNL-BASELINE-CAPTURE-01-COMBINED`. Final status:

```text
PAPER-DAILY-PNL-BASELINE-01-COMBINED: PARTIAL / BASELINE-SCHEMA-AND-READ-SIDE-CLOSED-CAPTURE-SEAM-OPEN
```

This bundle does **not** touch any row in §2 above, for the same reason
§9/§10 do not: paper-trade operator-visibility/accounting-display concern
only, equities-only, existing broker-snapshot layer. No row's status,
percentage, or category changes.
`docs/audits/multi_asset_completion_audit.md` is correspondingly not
updated by this bundle.

## 12. Later session: `PAPER-DAILY-PNL-BASELINE-CAPTURE-AND-OPERATOR-CLOSURE-01-COMBINED`

A follow-up off-market session closed the capture seam §11 left open: a
new authenticated `POST /api/v1/ops/action
{"action_key":"capture-account-equity-baseline"}` arm
(`core-rs/crates/mqk-daemon/src/routes/control_plane.rs`) reads the
daemon's real in-memory `broker_snapshot`, validates a caller-supplied
`trading_date` against the existing `NyseWeekdaysProvider` calendar seam,
and writes exactly one `sys_account_equity_baseline` row via the
already-existing `upsert_account_equity_baseline` helper, with a
deterministic `Uuid::new_v5` audit ID. A companion read-only route (`GET
/api/v1/portfolio/account-equity-baseline?trading_date=...`) lets an
operator confirm captured provenance directly. Proven end-to-end by 22
DB-backed scenario tests
(`core-rs/crates/mqk-daemon/tests/scenario_paper_daily_pnl_baseline_capture_01.rs`):
`GET /api/v1/portfolio/summary.daily_pnl` now reaches
`daily_pnl_truth_state = "active"` once a real capture has run, and stays
honestly `"baseline_unavailable"` without one. See
`docs/specs/paper_daily_pnl_capture_01a_current_truth_action_design.md`
through `..._01e_closure_decision.md`. Final status:

```text
PAPER-DAILY-PNL-BASELINE-CAPTURE-AND-OPERATOR-CLOSURE-01-COMBINED: CLOSED_LOCAL
PAPER-DAILY-PNL-BASELINE-01-COMBINED: CAPTURE-SEAM-CLOSED-BY-CAPTURE-01
```

This bundle does **not** touch any row in §2 above, for the same reason
§9/§10/§11 do not: paper-trade operator-visibility/accounting-display
concern only, equities-only, existing broker-snapshot layer. No row's
status, percentage, or category changes.
`docs/audits/multi_asset_completion_audit.md` is correspondingly not
updated by this bundle.

## 13. Later session: `PAPER-ORDER-LIFECYCLE-PERSISTENT-VISIBILITY-AUDIT-AND-CLOSURE-01-COMBINED`

A follow-up off-market session closed the remaining durable-visibility
gap in the OMS lifecycle chain: no existing route could reconstruct a
completed paper run's `signal evaluation -> no-trade diagnostics ->
outbox -> inbox` chain after the run stopped, without already knowing its
`run_id` — `GET /api/v1/execution/flow` covers only
outbox+lifecycle-events+fills and resolves "no `run_id`" via in-memory
active-run state (`ARMED`/`RUNNING` only). New route
`GET /api/v1/execution/paper-lifecycle?run_id=<uuid>`
(`core-rs/crates/mqk-daemon/src/routes/paper_lifecycle.rs`) resolves the
target run via `mqk_db::fetch_run` (explicit) or
`mqk_db::fetch_latest_run_for_engine` (durable latest-PAPER-run
resolution, independent of ARMED/RUNNING status) and joins two new
run-scoped DB helpers
(`fetch_strategy_signal_evaluations_for_run`,
`fetch_autonomous_no_trade_diagnostics_for_run`,
`core-rs/crates/mqk-db/src/strategy.rs`) with the existing
outbox/inbox run-scoped helpers. No migration. 26 new tests (5 mqk-db, 13
mqk-daemon DB-backed/in-process, 6 pure classifier unit tests) plus a
Phase D real-paper-DB hand-trace confirming the classifier's output
matches a real, complete AAPL signal->outbox->fill chain for the latest
(`STOPPED`) PAPER run. Portfolio/P&L visibility is explicitly reported as
`"in_memory_only_not_restart_surviving"` rather than reconstructed — no
durable portfolio/position table exists anywhere in the repo today. See
`docs/specs/paper_order_lifecycle_visibility_01a_current_truth_audit.md`
through `..._01e_closure_decision.md`. Final status:

```text
PAPER-ORDER-LIFECYCLE-PERSISTENT-VISIBILITY-AUDIT-AND-CLOSURE-01-COMBINED: CLOSED_LOCAL
PAPER-TRADE-LIFECYCLE-PROOF-02: LIFECYCLE-PERSISTENT-VISIBILITY-CLOSED
```

This bundle does **not** touch any row in §2 above, for the same reason
§9–§12 do not: paper-trade operator-visibility concern only,
equities-only, existing OMS/DB layer. No row's status, percentage, or
category changes. `docs/audits/multi_asset_completion_audit.md` is
correspondingly not updated by this bundle.

## 14. Later session: `STRATEGY-LAB-COMPLETION-AND-SCANNER-FOUNDATION-01-COMBINED`

A follow-up off-market session built the first local-data-only strategy/
symbol scanner: a pure scanner core
(`core-rs/crates/mqk-backtest/src/strategy_scanner.rs`,
`evaluate_scan_candidate` + `rank_scan_candidates`, no IO) plus a CLI
runner (`mqk backtest scan-strategies`,
`core-rs/crates/mqk-cli/src/commands/bkt.rs::run_strategy_scan`) that
resolves the enabled-equity registry universe, reads local
`exports/md_backup/{timeframe}/{symbol}_{timeframe}.csv` bar files,
reuses the existing `BacktestEngine`/`sweep_row_from_report` to score
each `(symbol, strategy_id)` candidate, and writes a deterministic
`manifest.json`/`candidates.json`/`candidates.csv`/`summary.json`
artifact tree under `exports/strategy_scans/{scan_id}/` (`scan_id` a
UUIDv5, never `Uuid::new_v4()`). Proven against real local data: the
full 88-symbol registry scanned with `swing_momentum` on `1D` ranked
88/88 candidates; the same universe scanned with `intraday_scalper` on
the real (empty) `5m/` directory honestly reported 10/10 as
`data_missing` with zero crashes. This is a **research-ranking**
foundation only — it does not feed any promotion/admission gate, does
not touch `oms_outbox`/`oms_inbox`, and made no provider/broker/network
call at any phase. See
`docs/specs/strategy_lab_scanner_01a_current_truth_audit.md` through
`..._01e_closure_decision.md`. Final status:

```text
STRATEGY-LAB-COMPLETION-AND-SCANNER-FOUNDATION-01-COMBINED: CLOSED_LOCAL
```

This bundle does **not** touch any row in §2 above, for the same reason
§9–§13 do not: off-market research/backtest-tooling concern only,
equities-only, existing strategy-engine layer. No row's status,
percentage, or category changes. `docs/audits/multi_asset_completion_audit.md`
is correspondingly not updated by this bundle.

## 15. Later session: `STRATEGY-SCANNER-DAEMON-JOBS-AND-GUI-REVIEW-01-COMBINED`

A follow-up off-market session turned the CLI-only scanner (§14) into an
operator-reviewable workflow: a daemon job API
(`POST`/`GET /api/v1/strategy-scans/jobs`,
`GET /api/v1/strategy-scans/jobs/:job_id`) that runs the identical
scanner core the CLI uses (the scan-execution and artifact-writing logic
was moved out of `mqk-cli` into `mqk_backtest::{execute_strategy_scan,
write_scan_artifacts}` so both callers share one implementation), a
read-only artifact readback route
(`GET /api/v1/strategy-scans/artifact`) with root-confinement path
validation, and a new `strategyScanner` GUI screen that submits a
bounded scan, polls job status, and reviews the resulting artifact —
every surface carrying a fixed "research evidence only, not autonomous
trading approval" warning and no trade/promote/approve control anywhere.
Jobs are in-memory (process-lifetime), matching the existing
`backtest_jobs` daemon-job precedent; no DB migration. Proven via 14
daemon scenario tests (submit/list/status/artifact, including path-
escape rejection) and a manual browser verification of the GUI screen
against a real dev build; a live HTTP replay against a running daemon
*process* did not happen, per the mission's own hard "do not start the
daemon runtime" rule — see
`docs/specs/strategy_scanner_jobs_gui_01e_closure_decision.md` §15/§16
for the full reasoning. Final status:

```text
STRATEGY-SCANNER-DAEMON-JOBS-AND-GUI-REVIEW-01-COMBINED: CLOSED_LOCAL
```

This bundle does **not** touch any row in §2 above, for the same reason
§9–§14 do not: off-market operator-tooling concern only, equities-only,
existing strategy-engine/scanner layer. No row's status, percentage, or
category changes. `docs/audits/multi_asset_completion_audit.md` is
correspondingly not updated by this bundle.

## 16. Later session: `STRATEGY-SCANNER-PROMOTION-GATES-AND-RESEARCH-QUEUE-01-COMBINED`

A follow-up off-market session added a research-review/promotion-gate
layer over the scanner's output (§14/§15): a pure classifier
(`mqk_backtest::strategy_scan_review`) that sorts already-ranked scanner
candidates into `blocked` / `needs_review` / `watchlist_candidate` /
`paper_candidate` / `rejected`, gating on absolute `total_return_pct`
(not rank or alpha alone) so a candidate can never reach
`paper_candidate` while it is losing money in absolute terms; a CLI
command (`mqk backtest review-scan`) that reads a scanner artifact and
writes a review artifact (`manifest.json`/`review_decisions.json`/
`review_decisions.csv`/`summary.json`); a read-only daemon route
(`GET /api/v1/strategy-scans/review-artifact`) with the same root-
confinement path validation as the scanner artifact route; and a
display-only review panel added to the existing `strategyScanner` GUI
screen. File-artifact only — no DB migration. Proven against the real
1D bar data already in this repo: all 88 real candidates in this
session's scan had negative absolute returns and all 88 were correctly
classified `rejected`, zero reaching `paper_candidate` — see
`docs/specs/strategy_scanner_promotion_01e_closure_decision.md` §12 for
the full evidence. `paper_candidate` carries no trading meaning; nothing
in this repo consumes it to submit, route, or admit an order. Final
status:

```text
STRATEGY-SCANNER-PROMOTION-GATES-AND-RESEARCH-QUEUE-01-COMBINED: CLOSED_LOCAL
```

This bundle does **not** touch any row in §2 above, for the same reason
§9–§15 do not: off-market research-governance tooling concern only,
equities-only, existing strategy-engine/scanner layer. No row's status,
percentage, or category changes. `docs/audits/multi_asset_completion_audit.md`
is correspondingly not updated by this bundle.

## 17. Later session: `STRATEGY-PROMOTION-REGISTRY-AND-RUNTIME-ENFORCEMENT-01-COMBINED`

A follow-up off-market session closed the gap §16 explicitly left open:
`paper_candidate` carried no trading meaning, and nothing consumed it to
submit, route, or admit an order. This bundle adds the durable
promotion registry §16 anticipated — `sys_strategy_promotion_transitions`
(migration `0046`, append-only, six states
`shadow_approved`/`paper_approved`/`active_paper`/`demoted`/`retired`/
`rejected`), an operator-authenticated transition surface
(`POST /api/v1/strategy/promotions/transition`) that independently
validates `paper_candidate` evidence from a review artifact (never
trusting a caller's claim), and — the load-bearing difference from
§16 — a hard **runtime enforcement gate** wired into both
strategy-originated outbox write paths (`decision.rs` Gate 3b,
`routes/strategy.rs` Gate 2b via one shared evaluator,
`promotion_gate::evaluate_paper_promotion_gate`). `registered +
enabled` in `sys_strategy_registry` is now proven — by DB-backed test,
including a real end-to-end proof through the actual daemon router — to
never be sufficient for paper trading; only an exact-identity,
unexpired `active_paper` promotion is. No live authorization exists
anywhere in this patch. Configuration-fingerprint identity binding
remains `PARTIAL` (`config_identity_status =
"unavailable_in_current_runtime"`, truthfully surfaced, never
defaulted) — see
`docs/specs/strategy_promotion_registry_01f_closure_decision.md` §5/§9
for the full identity-boundary record. Final status:

```text
STRATEGY-PROMOTION-REGISTRY-AND-RUNTIME-ENFORCEMENT-01-COMBINED: CLOSED_LOCAL
```

This bundle does **not** touch any row in §2 above, for the same reason
§9–§16 do not: off-market strategy-admission-governance tooling
concern only, equities-only, existing strategy-engine/scanner layer. No
row's status, percentage, or category changes.
`docs/audits/multi_asset_completion_audit.md` is correspondingly not
updated by this bundle.
