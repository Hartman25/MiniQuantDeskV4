# ASSET-CORE-04O — Cutover Go/No-Go Decision

**Patch:** `ASSET-CORE-04O-CUTOVER-GO-NOGO-DECISION-AND-LEDGER-RECONCILE-01`
(Phase D of `ASSET-CORE-04-PRODUCTION-CUTOVER-DESIGN-ONLY-01-COMBINED`)

**Bundle:** `ASSET-CORE-04-PRODUCTION-CUTOVER-DESIGN-ONLY-01-COMBINED`
**Phases completed:** A (`ASSET-CORE-04L`, commit `eca81772`), B
(`ASSET-CORE-04M`, commit `77129dd2`), C (`ASSET-CORE-04N`, commit
`68294bd4`), D (this document).

---

## 1. Is a production cutover authorized by this patch?

**No.** This bundle is design/decision-only per its own mission and hard
safety rules. Nothing in `04L`, `04M`, or `04N` flips any config flag,
adds any DB migration, or changes any production `.rs` source file's
behavior. Authorizing a cutover is explicitly out of scope for a
design-only bundle.

## 2. Is `ASSET-CORE-04` fully closed?

**No.**

```text
ASSET-CORE-04: PARTIAL / LIVE-ACCOUNTING-PRODUCTION-CONSUMPTION-OPEN
ASSET-CORE-04-PRODUCTION-CUTOVER-DESIGN-ONLY-01: CLOSED_LOCAL
ASSET-CORE-04 parent: PARTIAL / PRODUCTION-CUTOVER-DESIGNED-NOT-AUTHORIZED
```

Live accounting still does not consume `InstrumentEconomics`, does not
apply contract multipliers, does not support fractional position
quantities, does not support margin, and does not support currency
conversion (all re-confirmed at HEAD by `04L`'s callsite audit, not
assumed from any prior session's claim). Risk enforcement still does not
consume live NAV/margin/multiplier economics. Non-equity trading remains
disabled (`ASSET_RISK_NON_EQUITY_ROUTING_ENABLED = false`,
`ASSET_RISK_PRODUCTION_ENFORCEMENT_ENABLED = false`, unchanged).

## 3. What design-only scope closed?

- **`04L`** — a grounded, current-repo-evidence callsite audit naming
  every live-accounting, runtime-snapshot, risk-enforcement, and
  broker/order-routing symbol a future cutover would touch, plus the exact
  DB tables/columns that are whole-unit-only today.
- **`04M`** — a target architecture: an additive (not replacing)
  economics-aware valuation layer, reusing existing `ASSET-CORE-04A`-`04C`
  scaffold and the existing unreferenced `QtyMicros` fixed-point type
  rather than inventing new ones; explicit multiplier/margin/currency/NAV/
  risk/routing/rollout design; a 6-step proposed future patch sequence.
- **`04N`** — the mandatory test list and DB migration/decision package
  that must exist before any future behavior-changing cutover patch may
  start, plus an explicit Go/No-Go checklist.
- **`04O`** (this document) — the honest closure verdict and ledger
  reconcile.

No production `.rs` file under any crate's `src/` was edited by any phase
of this bundle. No file outside `docs/specs/`, `scripts/guards/`, and the
three roadmap-tracking docs (`MiniQuantDesk_Master_Patch_Ledger_v2.md`,
`docs/audits/multi_asset_completion_audit.md`,
`docs/specs/roadmap_completion_reconcile_01.md`) was touched.

## 4. What exact future patch is first?

Per `04M` §12 and `04N` §8's Go/No-Go checklist, the first future patch is
a **DB/schema design patch**: a new, additive-only migration (new
column(s) or new table(s), never modifying `0001`-`0044`) adding
fractional-quantity and multiplier/currency/margin-metadata capacity,
alongside a round-trip test proving pre-existing equity rows are
unaffected (`NULL` in the new column, no backfill). This must land and be
proven before the fixed-point quantity model patch, which must land before
any shadow-mode accounting patch.

## 5. Is a DB migration required before production consumption?

**Yes.** Every quantity column across all 44 migrations is `bigint`
(`04L` §7); no fixed-point/fractional column exists anywhere. Per
`db_rules.md`, this must be a new, additive migration, not a modification
of any committed one.

## 6. Is a fixed-point quantity model required?

**Yes**, for any fractional (crypto) or contract-multiplier
(futures/options) position — equity stays `i64` (`04M` §2). The workspace
already has an unreferenced candidate (`mqk_schemas::QtyMicros`, used only
by `OrderIntentV2.qty` today) that the design recommends reusing rather
than duplicating.

## 7. Is a live shadow-mode route/status required before paper cutover?

**Yes.** `04M` §10's rollout design and `04N` §5's scenario-test list both
require a read-only shadow-mode comparison (new economics-aware valuation
computed and compared against live equity accounting, exposed via a status
route) to be proven in production before any paper-only enablement, and
paper-only proven before any live consideration.

## 8. Is risk enforcement allowed to consume the new accounting model yet?

**No.** Confirmed unchanged by `04L`'s audit: `mqk-risk` has zero
references to any economics-scaffold symbol, and
`mqk_execution::asset_risk_policy` remains a static, model-only per-class
readiness table that does not read live portfolio state. This bundle adds
no caller anywhere in `mqk-risk` or `mqk-execution`'s submit path.

## 9. Are broker/order-routing changes allowed yet?

**No.** `mqk-execution::gateway` still has zero references to
`OrderIntentV2`/`QtyMicros` (`04L` §6, re-confirmed at this HEAD); no
broker adapter (`mqk-broker-alpaca` or any future adapter) was touched or
proposed as ready by this bundle. Per `broker_rules.md`, any broker
adapter change is its own separate patch, gated on accounting and risk
cutover being proven first.

## 10. What exact operator authorization will be required before a behavior-changing patch?

Each future patch in `04M` §12's sequence requires its own **separate,
explicit** operator authorization — one authorization does not carry
forward to the next slice, per `04N` §8's Go/No-Go checklist. The required
phrasing for the first patch (`04L` §4 above, the DB/schema design patch)
is:

```text
AUTHORIZE ASSET-CORE-04 DB SCHEMA MIGRATION FOR FRACTIONAL/MULTIPLIER/CURRENCY METADATA
```

Later slices (fixed-point model, shadow-mode accounting, risk shadow-mode,
paper-only cutover, broker-specific non-equity patch) each require their
own equivalent explicit phrase naming that exact slice, issued only after
the prior slice's Go/No-Go checklist items are all evidenced — not issued
in advance or as a blanket approval for the whole sequence.

## 11. What next patch should be queued?

Two independent options exist, neither blocked by the other:

- **`ASSET-CORE-04P-FIXED-POINT-LIVE-QUANTITY-SHADOW-MODE-01-COMBINED`** —
  if the operator wants to proceed toward the `ASSET-CORE-04` production
  cutover, this is the first implementation patch (the DB/schema design
  patch named in §4/§10 above), gated on the exact authorization phrase in
  §10.
- **`CRYPTO-REGISTRY-DATA-COMPLETION-SWEEP-01-COMBINED`** — an independent,
  cheaper, non-margin, non-multiplier, non-equity data-pipeline path
  (per `docs/audits/multi_asset_completion_audit.md`'s own row-level
  difficulty/dependency notes) that does not require any `ASSET-CORE-04`
  production cutover to make progress.

This document does not choose between them — that is an operator decision,
not one this design-only bundle is authorized to make.

---

## Safety confirmation

No live or paper order was submitted. No broker, provider, or network call
was made. No DB migration was added or applied. No config flag was
changed. No gate was weakened. No strategy threshold was changed. No
production accounting math was changed. No non-equity asset class was
enabled. No generated evidence, smoke log, export, or
`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` staged by this bundle.
`.env.local` never read or modified.
