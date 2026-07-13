# Strategy Promotion Registry 01F — Closure Decision

Patch group: `STRATEGY-PROMOTION-REGISTRY-AND-RUNTIME-ENFORCEMENT-01-COMBINED`
Patch: `STRATEGY-PROMOTION-REGISTRY-01F-CLOSURE-AND-LEDGER-RECONCILE-01`

> **Disposition corrected by `STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01-COMBINED`.**
> §1 below originally reported `CLOSED_LOCAL` while §5/§9 of this same
> document already honestly reported configuration-fingerprint identity
> binding as `PARTIAL` — a patch group cannot be `CLOSED_LOCAL` while one
> of its own binding contract elements is `PARTIAL` in the same breath
> (see `docs/specs/strategy_promotion_registry_closure_repair_01a_audit.md`
> item 5). The corrected bundle disposition is **`PARTIAL`**. Everything
> else in this document is preserved unedited as the historical record of
> what Phases A–F actually built and proved; the repair patch separately
> fixed five runtime/data-correctness defects found in that work
> (future-effective activation, concurrent-transition branching, evidence
> lineage loss, missing paper-only mode boundary, and this disposition
> itself) — see that patch's own audit and the ledger entries
> `STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01-COMBINED` and
> `STRATEGY-PROMOTION-CONFIG-IDENTITY-BINDING-01`.

## 1. Is `STRATEGY-PROMOTION-REGISTRY-AND-RUNTIME-ENFORCEMENT-01-COMBINED` closed?

**Corrected: `PARTIAL`, not `CLOSED_LOCAL`** (see notice above). At the
time this section was originally written: A durable, append-only strategy paper-promotion
registry now exists, is independently evidence-validated at approval
time, exposes an operator-authenticated transition surface plus
read-only truth routes, and is enforced as a hard runtime gate on both
strategy-originated outbox write paths (the internal decision seam and
the external signal route) via one shared evaluator. `registered +
enabled` in `sys_strategy_registry` is proven — by structural design and
by DB-backed test — to never be sufficient for paper trading; only an
exact-identity, unexpired `active_paper` promotion is. No live
authorization exists anywhere in this patch.

## 2. What durable model was added?

`core-rs/crates/mqk-db/migrations/0046_strategy_promotion_registry.sql`:
`sys_strategy_promotion_transitions`, an append-only history table keyed
by caller-injected `transition_id` (deterministic UUIDv5, never
`Uuid::new_v4()`). Identity is `(strategy_id, symbol, timeframe_secs)`.
Six canonical states (`shadow_approved`, `paper_approved`,
`active_paper`, `demoted`, `retired`, `rejected`), a `CHECK`-constrained
legal transition graph (implemented as `CASE`/`WHEN`, not an OR-chain —
see §9 below for why), evidence-provenance columns, and expiry support.
There is no separate mutable "current state" row — current state is
always a deterministic query over history
(`mqk_db::fetch_current_promotion_state` / `fetch_all_current_promotions`
/ `fetch_promotion_history`, `core-rs/crates/mqk-db/src/strategy_promotion.rs`).
No backfill: the table is empty after migration, which is authoritative
"zero approved strategies", not "unavailable".

## 3. What daemon control surface was added?

`core-rs/crates/mqk-daemon/src/routes/strategy_promotions.rs`:
- `GET /api/v1/strategy/promotions` — current state, all identities (public).
- `GET /api/v1/strategy/promotions/history` — full history, one identity (public).
- `GET /api/v1/strategy/promotions/check` — tradability check, one identity (public).
- `POST /api/v1/strategy/promotions/transition` — operator-authenticated
  mutation (Bearer token required via the existing `token_auth_middleware`).

The mutation route never trusts a caller's claim that a candidate is
`paper_candidate`: for evidence-requiring transitions (`no state ->
shadow_approved`, `demoted -> shadow_approved`), it independently
canonicalizes and root-bounds `review_dir` inside
`MQK_STRATEGY_REVIEW_ARTIFACT_ROOT` (the exact same pattern as the
sibling `GET /api/v1/strategy-scans/review-artifact` route), reads and
schema-validates `manifest.json`/`review_decisions.json`, requires
exactly one matching decision row, requires `review_state ==
paper_candidate`, and computes a SHA-256 evidence fingerprint from the
matched decision's own serialized content — never from request fields.

## 4. What is the exact runtime tradability rule?

```text
only active_paper, exact identity match, not expired -> may create a new paper outbox row
```

Enforced by one shared evaluator,
`mqk_daemon::promotion_gate::evaluate_paper_promotion_gate`
(`core-rs/crates/mqk-daemon/src/promotion_gate.rs`), called identically
from:
- `decision::submit_internal_strategy_decision` — new **Gate 3b**,
  placed after Gate 3 (registry enabled) and before Gate 4 (suppression).
- `routes::strategy::strategy_signal` — new **Gate 2b**, placed after
  Gate 2 (DB present) and before Gate 3 (arm state).

Both call sites use the identical evaluator, so promotion enforcement
cannot drift between an internally-generated order and an externally
submitted signal — the same pattern already established in this
codebase for sector risk
(`capital_policy::sector_risk_gate::evaluate_sector_risk_gate`).

## 5. Identity boundary — what exactly was bound, and what remains open?

- **Strategy**: `strategy_id`, trimmed.
- **Symbol**: uppercased.
- **Timeframe**: canonical `timeframe_secs: i64`. The internal path
  reads this natively from `StrategySpec.timeframe_secs` (added to
  `InternalStrategyDecision`, populated in `bar_result_to_decisions`).
  The external path required a new field,
  `StrategySignalRequest.timeframe_secs: Option<i64>` — optional at the
  JSON/type level (backward compatible) but required and fail-closed
  (`promotion_timeframe_unknown`) at the promotion gate itself. A
  scanner-label-to-seconds conversion table
  (`mqk_db::scanner_timeframe_label_to_secs`) exists only at
  evidence-validation time, converting a review artifact's string label
  (`"1D"`, `"1H"`, `"5m"`, …) to the canonical seconds form once, at
  approval creation — never re-derived at decision time.
- **Configuration fingerprint**: **`PARTIAL`.** No runtime code path
  (internal decision, external signal) carries a config hash or
  parameter fingerprint reproducible both at approval time and decision
  time. `config_fingerprint` is a nullable column, always `NULL` in this
  patch; `config_identity_status` is always
  `"unavailable_in_current_runtime"`, surfaced truthfully on every read
  route. This is the sole remaining `PARTIAL` item for the whole patch
  group — identity-v1 (`strategy_id + symbol + timeframe_secs`) is what
  is actually enforced.

## 6. What is the exact legal transition graph, and was a real bug found in it?

```text
NULL            -> shadow_approved   (requires evidence)
shadow_approved -> paper_approved | rejected | retired
paper_approved  -> active_paper | demoted | retired
active_paper    -> demoted | retired
demoted         -> shadow_approved (requires fresh evidence) | retired
retired         -> (terminal)
rejected        -> (terminal)
```

**Yes — a real bug was found and fixed during Phase B's DB-backed
proof, not just claimed fixed.** The first implementation encoded this
graph as an OR-chain of `previous_state = 'x'` equality comparisons in
the migration's `CHECK` constraint. In SQL three-valued logic,
`NULL = 'x'` evaluates to `NULL` (not `FALSE`), and a `CHECK` constraint
**passes** when its expression evaluates to `NULL`/`UNKNOWN` — so a row
with `previous_state IS NULL` and *any* `new_state` (including the
illegal `no-state -> active_paper`) silently passed the constraint. The
DB-backed `illegal_transition_rejected` test caught this immediately
(an insert that should have errored, didn't). Fixed by rewriting the
constraint as `CASE`/`WHEN` with an explicit `previous_state is null`
test as the first branch (never an equality comparison against `NULL`),
re-verified by dropping the table, clearing the stale `_sqlx_migrations`
checksum row, and re-running the full 11-test Phase B suite green.

## 7. What other real bugs did DB-backed testing catch in later phases?

- **Phase C**: the original `transition_id` seed included
  server-computed `previous_state`. On an exact request replay (the same
  client request sent twice), the *second* call's freshly-computed
  `previous_state` differed from the first call's (since the first call
  had already advanced state), producing a *different* `transition_id`
  and defeating idempotency — the replay was misclassified as
  `illegal_transition` instead of `duplicate`. Fixed by deriving
  `transition_id` only from client-supplied, replay-stable request
  fields (never a server-computed value), plus an explicit idempotency
  pre-check (search recorded history for a matching `transition_id`)
  before the legality/evidence gates run.
- **Phase D**: (a) a test-fixture bug in a hand-rolled "expired"
  promotion seed inserted a non-expired row with a *later*
  `effective_at_utc` than the intentionally-expired row, so the
  non-expired row won as "current" — the DB-backed
  `internal_expired_active_paper_denied` test caught the resulting
  `accepted` outcome where `promotion_expired` was expected. (b) a test
  relying on "arm state not set" silently inherited a **globally ARMED**
  `sys_arm_state` singleton row left over from an unrelated earlier test
  run against the same shared test DB, producing a `409` instead of the
  expected `403` — fixed with an explicit `DELETE FROM sys_arm_state`
  reset, matching the existing convention already used elsewhere in this
  test suite (e.g. `scenario_internal_strategy_decision.rs`).

Every one of these was caught by *running* the DB-backed tests against
the real isolated test database, not by static review — consistent with
this repo's proof-discipline rule that scenario tests are the standard,
not optimistic implementation claims.

## 8. Regression scope — did runtime enforcement break existing tests?

**Yes, and every break was found, diagnosed, and fixed with real DB
proof, not assumed away.** Adding Gate 3b (internal) and Gate 2b
(external) is fail-closed by construction, so *every* pre-existing
DB-backed test that registered a strategy as `enabled=true` and expected
to reach a gate past the registry check now required an explicit
`active_paper` promotion seed to keep reaching that gate. An initial
static-analysis pass (an `Explore` subagent scoped to "does this test
assert `accepted == true`") found 7 tests needing seeding; **running**
the full affected-file suite found 9 more (tests asserting things like
"must reach Gate 4, not Gate 3" or "must not be `suppressed`" — which
also silently regress once a new gate is inserted ahead of the one under
test, even without asserting acceptance). All 16 were fixed by adding a
small `seed_active_paper_promotion` helper (duplicated per test file,
matching this suite's existing per-file helper-duplication convention)
that walks the real legal transition graph to `active_paper` before the
gate under test runs.

A full-suite run (`cargo test -p mqk-daemon --no-fail-fast --include-ignored`)
found 13 additional failing targets unrelated to strategy
signals/decisions at all (missing `ALPACA_API_KEY_LIVE`, deployment-mode
configuration requiring `MQK_DAEMON_ADAPTER_ID=alpaca`, and one
pre-existing `sys_incidents` backend-string assertion). These were
**not** assumed pre-existing — verified via `git stash` (running the
identical failing tests against the last-committed pre-Phase-D HEAD,
same shared test DB) that they reproduce identically with zero Phase D
code present, and via grep confirming the remaining unverified ones
never reference `submit_internal_strategy_decision` or
`StrategySignalRequest` anywhere in their source.

## 9. Config identity status — why `PARTIAL`, and is that safe to close on?

The mission's own bounded-fallback clause is explicit: "If configuration
fingerprinting cannot be safely completed but exact strategy/symbol/
timeframe enforcement is complete, report `PARTIAL`… Do not call it
closed merely because the table and routes exist." Exact
strategy/symbol/timeframe enforcement **is** complete and DB-proven
(§4-§6, §11). Config fingerprinting is not — no reproducible
configuration hash exists anywhere in the current strategy runtime for
either path to read. `config_identity_status =
"unavailable_in_current_runtime"` is surfaced truthfully on every read
route (`GET .../promotions`, `.../history`, `.../check`) and in the
closure record here — never silently defaulted or hidden. This is a
scoped, honestly-reported `PARTIAL` within an otherwise `CLOSED_LOCAL`
patch group, exactly as the mission's own closure standard permits.

## 10. What GUI surface was added?

Bounded to the existing `StrategyScannerScreen.tsx` (no new screen
registration) — a `PromotionControlPanel`: identity fields, "Check
current state" / "Load history" reads, and a transition form. Safety
properties enforced in the component logic (not just documentation) and
proven both by static source assertions
(`__tests__/screenSource.test.ts`) and live browser verification against
the Vite dev server:
- Initial approval (`shadow_approved` from no prior state) is disabled
  until a `paper_candidate` row has been selected from the
  review-artifact panel above and its identity exactly matches the form
  — verified live: with no selection, the Submit button reports
  `disabled: true` and an explicit "No matching selection loaded" notice
  renders.
- `active_paper` / `demoted` / `retired` transitions require an explicit
  confirmation checkbox before Submit enables — verified live: selecting
  `active_paper` renders the checkbox; Submit stays `disabled: true`
  with all other fields filled until the checkbox is checked
  (`cb.click()` → `checkboxChecked: true, buttonDisabled: false`).
- A standing warning — "Paper promotion never grants live authorization.
  tradable_live is always false on every route in this panel." — is
  always rendered.
- `GET /api/v1/strategy/promotions/check` was observed on the real
  network tab with the exact expected query string
  (`strategy_id=swing_momentum&symbol=AAPL&timeframe_secs=86400`), and
  the panel surfaced `"Failed to fetch"` gracefully (no crash, no
  fabricated success state) when the daemon was unreachable.
- No order/broker/run-start/arm route is referenced anywhere in the
  panel or its API module — enforced by a source-text negative
  assertion (`screenSource.test.ts`) inherited unchanged from the prior
  scanner-promotion bundle plus new assertions for this panel.

`npm run build` (tsc + vite) succeeds with zero type errors; `npm test`
— 732/732 pass (19 in `strategyScanner`, all others pre-existing and
unaffected).

## 11. Real bounded closure proof — what was proven, end to end?

`core-rs/crates/mqk-daemon/tests/scenario_strategy_promotion_closure_proof_01f.rs`,
one consolidated DB-backed test run against the isolated local test
database (port 5434), driving the **real daemon router** in-process
(`axum`/`tower` `.oneshot()` — no daemon process started, no broker call):

1. Registered a fresh test strategy in `sys_strategy_registry`.
2. Wrote a real `paper_candidate` review-artifact fixture via the exact
   `write_review_artifacts` function the CLI's `review-scan` command
   calls (not a hand-rolled JSON blob).
3. `POST .../transition` no-state → `shadow_approved` (evidence
   independently validated from the fixture) → readback confirmed
   `current_state = "shadow_approved"`, `tradable_paper = false`.
4. `POST .../transition` → `paper_approved`.
5. Confirmed **zero** outbox rows can be created at `paper_approved`
   (`submit_internal_strategy_decision` → `disposition =
   "promotion_not_active"`, `outbox_row_count = 0`).
6. `POST .../transition` → `active_paper` → readback confirmed
   `current_state = "active_paper"`, `tradable_paper = true`,
   `tradable_live = false`; history readback showed all 3 transitions
   so far, newest first.
7. Confirmed **exactly one** synthetic outbox row is created once
   `active_paper` is reached and every other existing gate (registered +
   enabled, armed, active running run) is satisfied.
8. `POST .../transition` → `demoted`.
9. Confirmed a **new** decision (different `decision_id`) is refused
   post-demotion (`disposition = "promotion_demoted"`,
   `outbox_row_count = 0`); final history readback showed all 4
   transitions, newest first.
10. Cleaned up only this test's own rows
    (`sys_strategy_promotion_transitions`, `sys_strategy_registry`,
    `oms_outbox`, `runs`, and the temp fixture directory) — verified
    directly against the DB after the test run: 0 leftover rows in both
    `sys_strategy_promotion_transitions` and `sys_strategy_registry` for
    the test's `strategy_id` prefix.

## 12. Were any real or forced paper/live orders submitted?

**No.** Every outbox row created anywhere in this patch group (Phase B
DB tests, Phase C/D route tests, Phase F closure proof) is a synthetic
row inserted via the same `oms_outbox` write path already proven durable
by prior patches, inside an isolated test database. No broker adapter
(`mqk-broker-alpaca`, `mqk-broker-paper`) is imported by any file this
patch touched. No daemon process was started at any phase — every proof
uses in-process `axum::Router` + `tower::ServiceExt::oneshot()`. No
network call occurs anywhere in the promotion gate, the promotion
routes, or the GUI panel except the panel's own `fetch()` to the
daemon's own configured URL (observed failing gracefully against an
unreachable daemon during live GUI verification).

## 13. Was the paper database (port 5440) touched?

**No.** All migration application and all DB-backed proof in every
phase ran exclusively against the isolated local test database (`mqk
-test-postgres`, port 5434, `mqk_test`). `docker exec mqk-paper-postgres`
was used only for the read-only pre-flight schema inspection specified
at the start of this patch group — no write command was ever issued
against it.

## 14. Full patch-group commit chain

Phase A `6ff1f39c` (design) → Phase B `ea57f098` (durable DB
foundation) → Phase C `e3758bae` (daemon control surface) → Phase D
`436ae2b4` (runtime enforcement) → Phase E `6f3233fd` (GUI) → Phase F
(this entry).

## 15. Were any forbidden files touched?

**No.** Confirmed via `git status --short` before every phase commit:
no `.env.local`, no `exports/`, no `smoke_logs/`, no
`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` staged at any phase.
No file under `mqk-broker-alpaca/`, `mqk-broker-paper/`, `mqk-risk/`,
`mqk-reconcile/`, `mqk-portfolio/`, `mqk-execution/src/gateway.rs`, or
any strategy-engine/provider-adapter/crypto/futures/options/forex path
was ever touched at any phase — these paths were never touched, per
`git diff --stat` against every phase's commit.

## 16. Known skipped validation (reported honestly, not silently omitted)

- Whole-crate `cargo fmt --check` / `cargo clippy --all-targets` were
  not run for `mqk-daemon` in every phase — the crate carries
  pre-existing, unrelated formatting/lint drift (confirmed via `git
  diff --stat` showing zero changes to the affected files) in files this
  patch never touches. The narrowest available proof
  (`cargo clippy -p mqk-daemon --lib`, plus targeted per-file
  `rustfmt --check`) was used instead at every phase, and is clean
  except the one pre-flagged `manual_range_contains` finding at
  `routes/strategy_scans.rs:87`.
- The 13 full-suite test-target failures named in §8 remain unfixed —
  correctly out of this patch's scope (missing Alpaca credentials /
  deployment-mode environment configuration, and one pre-existing
  `sys_incidents` assertion, none related to strategy promotion).
- Configuration-fingerprint identity binding remains `PARTIAL` (§9).

## 17. Recommended next market-hours prompt

`AUTON-NO-TRADE-02A-MARKET-HOURS-PREFLIGHT-AUDIT-01`'s successor —
unchanged from the prior scanner-promotion bundle; this patch group adds
no new market-hours-dependent surface.

## 18. Recommended next off-market prompt

`DAILY-DATA-READINESS-AND-FRESHNESS-01-COMBINED`, as specified by this
patch group's own mission.

---

## Final status

```text
STRATEGY-PROMOTION-REGISTRY-AND-RUNTIME-ENFORCEMENT-01-COMBINED: PARTIAL
```

**Corrected by `STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01-COMBINED`**:
this line originally read `CLOSED_LOCAL`, which was inconsistent with
this same document's own §5/§9 `PARTIAL` identity-boundary finding
directly below. See the notice at the top of this document.

**Identity boundary:** `strategy_id + symbol + timeframe_secs` fully
enforced and DB-proven; configuration fingerprint `PARTIAL`
(`config_identity_status = "unavailable_in_current_runtime"`, truthfully
surfaced everywhere, never defaulted) — tracked as open ledger item
`STRATEGY-PROMOTION-CONFIG-IDENTITY-BINDING-01`.

**Safety confirmation (whole bundle):** no real or forced paper/live
orders; no broker/provider/network call from any promotion-gate, route,
or GUI code path; no daemon runtime process started at any phase; no
execution armed; paper DB (port 5440) never migrated or mutated; no
strategy/risk/session/reconcile logic weakened; no generated
artifact/evidence/export staged at any phase; no secret touched.
