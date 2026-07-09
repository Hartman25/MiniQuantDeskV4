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
