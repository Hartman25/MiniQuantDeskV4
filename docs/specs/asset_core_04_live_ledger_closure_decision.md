# ASSET-CORE-04 — Live Ledger Section Closure Decision

**Bundle:** `ASSET-CORE-04-LIVE-LEDGER-SECTION-CLOSURE-01-COMBINED`
**Phases completed:** A (`b99e49b9`), B (`e21bbbaf`), C (skipped), D (skipped), E (this document).

---

## 1. Is `ASSET-CORE-04` closed?

**No — remains `PARTIAL / LIVE-ACCOUNTING-PRODUCTION-CONSUMPTION-OPEN`.**

This bundle closed the *audit-and-safe-gap* scope it was authorized for.
It did not, and per its own hard safety rules could not, close the
production-consumption scope — wiring the economics scaffold into live
accounting, risk enforcement, or order routing is a production-behavior
change, explicitly out of scope for a safe-closure sweep.

## 2. What exact sub-scope closed in this bundle?

- **Phase A** (`ASSET-CORE-04A-CURRENT-LIVE-LEDGER-BOUNDARY-AUDIT-01`,
  `b99e49b9`): grounded the live-accounting/economics-scaffold boundary in
  current-repo evidence (caller-map grep, not doc-comment trust), and
  identified that the Phase C/D surfaces this bundle was authorized to
  build already exist.
- **Phase B** (`ASSET-CORE-04B-LIVE-ACCOUNTING-INVARIANT-PROOF-01`,
  `e21bbbaf`): added the one cross-module numeric proof that did not
  already exist — live FIFO accounting/NAV and the economics scaffold's
  equity special case agree exactly, for six scenarios plus two
  separation-of-call-graph checks (8 tests total,
  `scenario_asset_core_04_live_ledger_invariants.rs`).
- **Phase C, Phase D:** skipped — see §11.
- **Phase E** (this document): honest closure verdict and ledger reconcile.

No file outside `docs/specs/`, `docs/audits/` (see §13), and
`core-rs/crates/mqk-portfolio/tests/` was touched by this bundle.

## 3. Does live accounting consume `InstrumentEconomics`?

No. Confirmed by direct source read and caller-map grep across
`mqk-execution`, `mqk-risk`, `mqk-runtime`, and the GUI
(`docs/specs/asset_core_04_live_ledger_boundary_audit.md` §2/§4): zero
production callers of `InstrumentEconomics`/`value_position_economics`/
`PortfolioEconomics`/`aggregate_portfolio_economics` anywhere. Phase B's
new tests reinforce this by construction — they call both paths
side-by-side from test code, never from `accounting.rs` calling into
`instrument_economics.rs` or vice versa.

## 4. Does live accounting apply contract multipliers?

No. `metrics::compute_equity_micros`/`compute_exposure_micros` compute
plain `qty * mark`; there is no multiplier field or term anywhere in
`accounting.rs`, `metrics.rs`, or `valuation.rs`.

## 5. Does live accounting support fractional position quantities?

No. `Fill.qty`, `Lot.qty_signed`, `PositionSnapshot.net_qty`, and every
relevant DB column are `i64`/`bigint`, whole-unit only. The one
fractional-capable model type in the workspace,
`OrderIntentV2.qty: QtyMicros`, is never referenced by
`mqk-execution::gateway` — unreachable from any submission path.

## 6. Does live accounting support margin?

No. No `margin` symbol exists anywhere in `mqk-portfolio`,
`mqk-execution`, or `mqk-risk` production source. The only `margin`
symbols in the workspace are `AssetRiskPolicy.requires_margin_model`
(a static per-asset-class readiness flag, `ASSET-CORE-03B`) and
`mqk-backtest`'s independent, backtest-only economics module.

## 7. Does live accounting support currency conversion?

No. No `currency` field exists in the live accounting path at all; the
one place a currency assumption is made explicit (`04D`/`04F`'s
`PORTFOLIO_ECONOMICS_ACCOUNT_CURRENCY = "USD"` constant) is a read-only
diagnostic route, not a conversion — and the economics scaffold itself
always *refuses* rather than converts
(`InstrumentEconomicsTruthState::CurrencyConversionUnsupported`).

## 8. Does risk enforcement consume live NAV/margin/multiplier economics?

No. Confirmed by grep: zero references to any economics-scaffold symbol
in `mqk-risk`. `mqk_execution::asset_risk_policy` (`ASSET-CORE-03B`) is a
**static**, model-only policy table — it does not read live portfolio
state, NAV, or any economics-scaffold output; it only classifies each
asset class's *readiness* (`requires_margin_model`, etc.) independent of
any specific position.

## 9. Did any live/paper trading behavior change?

No. Every file this bundle touched is either a doc
(`docs/specs/*.md`), a PowerShell guard script
(`scripts/guards/validate_asset_core_04_live_ledger_audit.ps1`), or a new
test file that calls existing, unmodified library functions
(`core-rs/crates/mqk-portfolio/tests/scenario_asset_core_04_live_ledger_invariants.rs`).
No production `.rs` file under `src/` in any crate was edited.

## 10. Were any non-equity classes enabled?

No. `AssetRiskPolicy` state for `crypto`/`future`/`option`/`forex` remains
`Disabled` (`rates_fixed_income` remains `ResearchOnly`); module constants
`ASSET_RISK_PRODUCTION_ENFORCEMENT_ENABLED` and
`ASSET_RISK_NON_EQUITY_ROUTING_ENABLED` remain `false`. Unchanged by this
bundle.

## 11. What remains before `ASSET-CORE-03` can enforce graduated live per-asset risk?

A real production-consumption cutover: wiring live portfolio NAV, a real
margin model, contract-multiplier application, and currency conversion
into an actual enforcement path (`mqk-risk` and/or
`mqk-execution::gateway`). Per this bundle's own hard safety rules, that
is production-behavior-changing and explicitly out of scope here — see
§12 for the recommended next patch.

**Phase C/D disposition (required detail):**

- **Phase C** (`ASSET-CORE-04C-READONLY-ACCOUNTING-BOUNDARY-SURFACE-IF-SAFE-01`)
  was skipped. Its mission was to add explicit
  `production_consumption_enabled=false`-style honesty fields to a
  read-only economics status route. `GET /api/v1/portfolio/economics/status`
  (`04D`/`04F`, already committed) already carries exactly this class of
  field: `model_only: true`, `trading_uses_portfolio_economics: false`,
  `runtime_uses_portfolio_economics: false`,
  `risk_uses_portfolio_economics: false`,
  `order_path_uses_portfolio_economics: false`. Adding a second,
  differently-named set of the same booleans on a new route would create
  two sources of the same truth that could silently drift. No files were
  changed for Phase C.
- **Phase D** (`ASSET-CORE-04D-RISK-ENFORCEMENT-READINESS-CLASSIFIER-IF-SAFE-01`)
  was skipped. Its mission was a pure readiness classifier
  (`AssetCore04RiskEnforcementReadiness`) reporting
  `requires_margin_model`/`requires_contract_multiplier`/
  `requires_currency_conversion`/etc. per asset class.
  `GET /api/v1/system/asset-risk-policy/status` (`ASSET-CORE-03B`, already
  committed, `mqk-daemon/src/routes/system.rs:1432`) already reports
  exactly this shape, live from `mqk_execution::default_asset_risk_policies()`
  — not a hardcoded string table. A second classifier would either
  duplicate this route's live truth or hardcode roadmap-status strings,
  the exact anti-pattern `ASSET-CORE-02-03E`'s closure doc already
  rejected for the same reason. No files were changed for Phase D.

## 12. Recommended next partial-roadmap bundle

`ASSET-CORE-04-PRODUCTION-CUTOVER-DESIGN-ONLY-01-COMBINED` — a
design/decision-only patch (no code) that specifies exactly what a real
production cutover would require: which live-accounting call sites would
need to change, what DB migration (if any) fractional/multi-currency
positions would require, and which of `mqk-risk`/`mqk-execution::gateway`
would need a new, explicitly-authorized production-behavior-changing
patch to consume it. This bundle's audit (§2-§10 above,
`docs/specs/asset_core_04_live_ledger_boundary_audit.md` §2-§6) already
supplies the current-state grounding that design patch would build on —
no further audit-only work is needed first.

An alternative, independent next step (not blocked by the above) is
`CRYPTO-REGISTRY-DATA-COMPLETION-SWEEP-01-COMBINED`: `ASSET-CORE-04B`'s
bridge already proves crypto instruments bridge cleanly to the economics
model (model-only, zero enablement), and `CRYPTO-DATA-01`/`CRYPTO-REGISTRY-01`
remain the cheaper, non-margin, non-multiplier non-equity path per
`docs/audits/multi_asset_completion_audit.md`'s own row-level
difficulty/dependency notes.

---

## Safety confirmation

No network call. No provider/broker call. No paper or live order
submitted or attempted. No config flag changed. No gate weakened. No
strategy threshold changed. No fabricated data. No generated evidence,
smoke log, export, or `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`
staged by this bundle. `.env.local` never read or modified.
