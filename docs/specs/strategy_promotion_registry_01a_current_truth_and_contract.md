# Strategy Promotion Registry 01A — Current Truth Audit & Contract Design

Patch group: `STRATEGY-PROMOTION-REGISTRY-AND-RUNTIME-ENFORCEMENT-01-COMBINED`
Patch: `STRATEGY-PROMOTION-REGISTRY-01A-CURRENT-TRUTH-AUDIT-AND-CONTRACT-DESIGN-01`

## 0. Starting state

Branch `main`, HEAD `ce09041b` (`docs: close strategy scanner promotion
review`), matching the mission's expected starting HEAD exactly. Working
tree clean except allowed untracked paths
(`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`, `smoke_logs/`). Next
free migration id confirmed from `core-rs/crates/mqk-db/migrations/manifest.json`:
**`0046`** (last applied entry is `0045_account_equity_baseline.sql`).

---

## 1. What are all current strategy-originated outbox write paths?

Exactly two, both already fail-closed on a strict gate sequence, neither
of which currently checks any promotion/approval truth beyond
`sys_strategy_registry.enabled`:

1. **Internal/native path**: `mqk_daemon::decision::submit_internal_strategy_decision`
   (`core-rs/crates/mqk-daemon/src/decision.rs:338`), called from exactly
   one production call site — `loop_runner.rs`'s execution-loop tick
   (`core-rs/crates/mqk-daemon/src/state/loop_runner.rs:1000-1030`), which
   first converts a `StrategyBarResult` to zero-or-more
   `InternalStrategyDecision`s via `bar_result_to_decisions`
   (`decision.rs:262-326`) and then calls
   `submit_internal_strategy_decision` per decision. Gate 7 of that
   function (`decision.rs:788-819`) is the sole outbox-write call
   (`mqk_db::outbox_enqueue`).
2. **External signal path**: `POST /api/v1/strategy/signal`
   (`strategy_signal` handler, `core-rs/crates/mqk-daemon/src/routes/strategy.rs:201-1274`),
   an authenticated operator route wired in `routes.rs:702`. Gate 7
   (`routes/strategy.rs:1188-1273`) is the sole outbox-write call.

No other file in the workspace calls `mqk_db::outbox_enqueue` from a
strategy-originated context (confirmed by full-workspace grep for
`outbox_enqueue` callers — the only two call sites are the ones above;
all other production writers are non-strategy, e.g. manual order submit
in `routes/execution.rs`, which is operator-direct and out of this
patch's scope per the mission).

Both paths currently gate on `sys_strategy_registry.enabled` (internal:
Gate 3, `decision.rs:618-661`; external: implicitly required earlier in
the pipeline via the same registry, though the external route does not
re-check registry `enabled` directly today — it relies on suppression
state (Gate 6) and does not call `fetch_strategy_registry_entry` at all).
**This is itself a pre-existing gap**: the external signal path does not
verify `enabled=true` before Gate 7. This patch does not need to close
that specific gap to satisfy its mission (promotion enforcement is
strictly additive and stronger than registry `enabled` alone — an
unpromoted signal is refused regardless of registry state), but it is
noted here for completeness since Phase D touches this same route.

## 2. What exact identity is present at each path?

| Field | Internal (`InternalStrategyDecision`) | External (`StrategySignalRequest`) |
|---|---|---|
| `strategy_id` | present, `String` | present, `String` |
| `symbol` | present, `String` | present, `String` |
| `timeframe` | **absent on the struct**, but the caller (`loop_runner.rs:660`, `decision.rs:271`) has `bar_result.spec.timeframe_secs: i64` in scope at the exact call site that constructs the decision | **absent entirely** — `StrategySignalRequest` (`api_types.rs:2879-2908`) has no timeframe field of any kind |
| config/fingerprint | absent | absent |

`mqk_strategy::StrategySpec` (`core-rs/crates/mqk-strategy/src/types.rs:4-19`)
carries exactly `{ name: String, timeframe_secs: i64 }` — one timeframe
per strategy instance (Tier A single-timeframe constraint), which is
exactly the seconds-based canonical form this design adopts (see §4).

## 3. Can current runtime reproduce strategy configuration identity?

**No**, not beyond `strategy_id + symbol + timeframe`. Neither
`InternalStrategyDecision`, `StrategySignalRequest`, `StrategySpec`, nor
`SymbolStrategyAssignment` (used in `loop_runner.rs` for per-symbol
dispatch) carries a config hash, parameter fingerprint, or `git_hash` at
decision time. The only `git_hash` values in the whole promotion-adjacent
surface are **artifact-level**, captured at scan/review time
(`ReviewManifest.git_hash`, `strategy_scan_review.rs:378`) — these
describe what code produced the *evidence*, not what code is running
*now* when a decision is evaluated. There is no live "config fingerprint
helper" anywhere in `mqk-daemon`, `mqk-strategy`, or `mqk-runtime` that
both an approval-creation caller and a runtime-check caller could invoke
identically. Per the mission's bounded fallback: this patch implements
**identity-v1** (`strategy_id + symbol + timeframe_secs` only) and
stores `config_fingerprint` as a nullable column, always `NULL` in this
patch, with `config_identity_status = "unavailable_in_current_runtime"`
surfaced honestly on every read route. This is recorded as the single
`PARTIAL` item for this patch group in §12/closure.

## 4. How will timeframe normalization work?

Canonical runtime form: **`timeframe_secs: i64`** (matches
`StrategySpec.timeframe_secs` exactly — no conversion needed on the
runtime-check side for the internal path).

Scanner/review artifacts carry timeframe as a **string label**
(`StrategyScanCandidate.timeframe: String`, e.g. `"1D"`,
`"1H"`, `"5m"`; confirmed field name via `strategy_scan_review.rs:107`
`StrategyScanReviewDecision.timeframe: String`). A single pure,
deterministic conversion table — used **only** at evidence-validation
time (Phase C, when an operator creates or re-validates an approval from
a review artifact) — converts the label to seconds:

```text
"1m"  -> 60
"5m"  -> 300
"15m" -> 900
"30m" -> 1800
"1H"  -> 3600
"1D"  -> 86400
anything else -> None (fail closed: promotion_timeframe_unknown)
```

Because the canonical DB/runtime identity is always `timeframe_secs`,
the runtime-check side (internal and external) never needs to
re-derive a string label — it compares its own already-known
`timeframe_secs` directly against the stored promotion row. Only the
one-time evidence-to-approval step performs the label→seconds
conversion, so there is exactly one place this mapping can drift, and it
is exercised by DB-backed scenario tests in Phase B/C.

External signal path gap: `StrategySignalRequest` has no timeframe field
today. Phase D adds `pub timeframe_secs: Option<i64>` (smallest
backward-compatible addition — optional field, `#[serde(default)]`,
existing producers unaffected until promotion enforcement lands). Once
the Phase D gate is wired, a missing or non-positive `timeframe_secs` on
an external signal fails closed (`promotion_timeframe_unknown`) — the
route never guesses or defaults a timeframe.

## 5. What is the exact promotion state / capability matrix?

Six states, `snake_case`, matching the mission's required minimum set
exactly: `shadow_approved`, `paper_approved`, `active_paper`, `demoted`,
`retired`, `rejected`.

Capability matrix (identical to the mission's table; **only
`active_paper` may create a new paper outbox row**):

| State | Shadow/research eval | New paper outbox rows | Live trading |
|---|---:|---:|---:|
| no record | No authorization inferred | Denied | Denied |
| `shadow_approved` | Allowed (dry-run/shadow architecture only) | Denied | Denied |
| `paper_approved` | Allowed | Denied (activation required) | Denied |
| `active_paper` | Allowed | Allowed (subject to every other existing gate) | Denied |
| `demoted` | Inspect only | Denied | Denied |
| `retired` | No | Denied | Denied |
| `rejected` | No | Denied | Denied |
| expired approval | No authorization | Denied | Denied |
| DB/query unavailable | Unknown, fail closed | Denied | Denied |

`paper_approved` does not trade. No stronger existing project convention
was found requiring otherwise (checked `docs/specs/roadmap_completion_reconcile_01.md`
and both `strategy_scanner_promotion_01a`/`01e` docs — neither implies
any different capability mapping).

## 6. What is the exact legal transition graph?

An append-only transition-history table stores every transition; current
state is always derived by querying the latest row per identity — there
is no separate mutable "current state" row to fall out of sync.

```text
NULL            -> shadow_approved   (requires validated paper_candidate evidence)
shadow_approved -> paper_approved    (requires validated paper_candidate evidence)
shadow_approved -> rejected
shadow_approved -> retired
paper_approved  -> active_paper
paper_approved  -> demoted
paper_approved  -> retired
active_paper    -> demoted
active_paper    -> retired
demoted         -> shadow_approved   (requires a freshly re-validated evidence artifact —
                                       same evidence-validation path as NULL -> shadow_approved,
                                       run again; not a bare state flip)
demoted         -> retired
retired         -> (terminal; no further transition)
rejected        -> (terminal; no further transition)
```

This is a strict superset of every rule the mission lists as a minimum,
plus documented, bounded extensions (`paper_approved -> retired`,
`active_paper -> retired` directly, both needed so an operator can
retire a strategy without forcing it through `demoted` first). Enforced
in two layers: (a) a DB `CHECK` constraint on `(previous_state,
new_state)` pairs as a structural backstop, and (b) the daemon route
layer, which computes the actual current state first and only ever
constructs a transition row with the correct `previous_state` — a client
can never claim a false `previous_state`.

## 7. What immutable evidence fields will be stored?

Every transition row that requires evidence (`NULL -> shadow_approved`
and `demoted -> shadow_approved`) stores, verbatim from the
independently-read artifact (never from caller-supplied claims):

- `evidence_review_id` (from `ReviewManifest.review_id`)
- `evidence_scanner_scan_id` (from `ReviewManifest.scanner_scan_id`)
- `evidence_git_hash` (from `ReviewManifest.git_hash`)
- `evidence_artifact_path` (canonicalized `review_dir`, root-bounded)
- `evidence_fingerprint` (see §8)
- plus the transition's own `transition_id`, `operator_note`,
  `initiated_by`, `effective_at_utc`, `expires_at_utc`, `created_at_utc`
  — all caller-injected, never DB-generated.

Transitions that do not require new evidence (e.g. `paper_approved ->
active_paper`, any `-> demoted`, any `-> retired`) carry `NULL` in all
`evidence_*` columns and reference the fact that evidence was already
validated at the most recent evidence-bearing transition for that
identity (visible via history readback).

## 8. What is the exact evidence fingerprint input?

`sha2::Sha256` (already a workspace-vetted dependency — present in
`core-rs/crates/mqk-audit/Cargo.toml:11` and
`core-rs/crates/mqk-config/Cargo.toml:11`; no new hashing crate is
introduced, only reused in `mqk-daemon`) over the canonical JSON
serialization of the **matched `StrategyScanReviewDecision` row itself**
(not the whole artifact directory) — i.e. `serde_json::to_string` of the
single decision struct that exactly matched
`(strategy_id, symbol, timeframe_secs)` and had
`review_state == "paper_candidate"`. This ties the fingerprint to the
exact evidence content that justified the approval, independent of
surrounding file formatting, and is trivially reproducible by re-reading
the same artifact later for audit.

## 9. What migration shape is safest?

One additive migration, `0046_strategy_promotion_registry.sql`, adding a
single append-only table `sys_strategy_promotion_transitions` (see Phase
B). No existing table is altered. `sys_strategy_registry` is untouched —
`enabled` keeps its current, narrower meaning. No backfill: an empty
promotion registry after migration means zero approved strategies,
proven by a dedicated Phase B test.

## 10. Which routes are public and which require auth?

| Route | Method | Auth |
|---|---|---|
| `/api/v1/strategy/promotions` | GET | public (read-only truth) |
| `/api/v1/strategy/promotions/history` | GET | public (read-only truth) |
| `/api/v1/strategy/promotions/check` | GET | public (read-only truth) |
| `/api/v1/strategy/promotions/transition` | POST | **operator (Bearer token required)** |

Matches the existing convention exactly (compare `strategy_scans.rs`:
job/artifact reads are public, job submission is operator-only). The
mutation route is added to the existing `operator` sub-router in
`routes.rs` (wrapped by `token_auth_middleware`), not a new router.

## 11. Where will each runtime promotion gate be placed?

One shared evaluator, `mqk_daemon::promotion_gate::evaluate_paper_promotion`
(new module `core-rs/crates/mqk-daemon/src/promotion_gate.rs`), mirroring
the existing shared-gate pattern already used for sector risk
(`capital_policy::sector_risk_gate::evaluate_sector_risk_gate`, called
identically from both `decision.rs` Gate 1h and `routes/strategy.rs`
Gate 1i). The new evaluator:

- takes `(db: &PgPool, strategy_id, symbol, timeframe_secs, now_utc)`
- fetches the latest transition row via `mqk_db::strategy_promotion::fetch_current_promotion_state`
- returns a structured outcome `{ paper_tradable: bool, reason_code: PromotionReasonCode, blockers: Vec<String> }`
- never reads any artifact file — only durable DB truth

Call sites:
- **Internal path**: new Gate, placed immediately after Gate 3 (registry
  enabled check) and before Gate 4 (suppression), in `decision.rs`. This
  keeps registry-enabled as a distinct, still-necessary precondition
  while making promotion strictly additive and evaluated after it.
- **External path**: new Gate, placed after Gate 2 (DB present) and
  before Gate 3 (arm state), in `routes/strategy.rs`. Placed early
  (right after DB becomes available) so a promotion refusal never
  consumes WS-continuity/session/day-limit checks that ran earlier in
  the existing gate order — but strictly after DB-presence, since the
  gate cannot evaluate anything without DB. Gate numbering in the module
  doc comment will be updated to insert this as a new gate without
  renumbering the existing gates' semantic meaning (existing gates keep
  their existing labels; the new gate gets its own label, e.g. "Gate 2b:
  paper_promotion").

Both call sites require the caller to have already resolved a concrete
`timeframe_secs` (internal: from `StrategySpec`; external: from the new
optional request field) — resolution/validation of *that* happens before
the promotion gate runs, so the promotion gate itself never guesses.

## 12. What existing clients must be updated?

- No in-repo producer of `StrategySignalRequest` exists today besides
  the daemon's own route handler and its tests (confirmed by
  workspace-wide grep — no `research-py` or CLI caller constructs this
  JSON body). Phase D's addition of an optional `timeframe_secs` field
  is therefore backward compatible with zero required external updates;
  only in-repo daemon scenario tests that assert full gate-sequence
  behavior need a `timeframe_secs` value once the new gate is wired
  (tracked in Phase D).
- `bar_result_to_decisions` (`decision.rs:262`) gains one new field on
  its output (`InternalStrategyDecision.timeframe_secs`, populated from
  `result.spec.timeframe_secs`) — every existing direct constructor of
  `InternalStrategyDecision` in `mqk-daemon/tests/*.rs` (the scenario
  files listed in the mission's producer search) must add this field.
  Enumerated for Phase D: `scenario_internal_strategy_decision.rs`,
  `scenario_fleet_enable_disable_interaction_cc02d.rs`,
  `scenario_multi_symbol_capital_caps_01.rs`,
  `scenario_multi_symbol_day_order_cap_01.rs`,
  `scenario_native_strategy_b6_budget_gate.rs`,
  `scenario_native_strategy_bridge_b1c.rs`,
  `scenario_sector_risk_gate_etf_risk_closure_01.rs`,
  `scenario_signal_to_outbox_unit_proof_01.rs`,
  `scenario_stressed_recovery_lo02.rs`.

## 13. Exact behavior for no DB, empty registry, expired promotion, query failure

- **No DB**: evaluator never runs a query; returns `paper_tradable=false,
  reason_code=promotion_db_unavailable` immediately, mirroring the
  existing `Gate 2: db_present` pattern already used by both paths.
- **Empty registry** (table exists, zero rows for this identity):
  `fetch_current_promotion_state` returns `Ok(None)` — authoritative
  "no record", never synthesized as anything else. Evaluator returns
  `paper_tradable=false, reason_code=promotion_missing`.
- **Expired promotion** (`active_paper` row with `expires_at_utc <
  now_utc`): evaluator checks expiry after confirming state is
  `active_paper`; if expired, returns `paper_tradable=false,
  reason_code=promotion_expired` — never silently treated as still
  active.
- **Query failure** (DB present but the query itself errors, e.g.
  connection drop mid-call): evaluator returns `paper_tradable=false,
  reason_code=promotion_query_failed`, distinct from both `_missing` and
  `_db_unavailable` so an operator can tell "never approved" apart from
  "truth temporarily unreachable."

## 14. How is paper approval prevented from authorizing LIVE?

By construction, not by a runtime flag: the promotion gate is wired only
into the two paper-outbox call sites named in the mission
(`submit_internal_strategy_decision`, `POST /api/v1/strategy/signal`),
neither of which is reachable from a LIVE-mode run in the current
architecture (LIVE routing lives entirely outside this patch's touched
files — `mqk-broker-alpaca`, `mqk-execution/src/gateway.rs` — both on the
explicit do-not-modify list). The evaluator's own return type has no
"live" variant; `PromotionReasonCode::PromotionLiveNotAuthorized` is
defined for forward documentation/consistency with the mission's
reason-code vocabulary but is never produced by any code path in this
patch, since this patch adds no live-authorization check of any kind —
there is nothing for it to gate. Every read route additionally hard-codes
`tradable_live: false` in its response with no code path that can flip
it, so an operator reading `GET /api/v1/strategy/promotions` can never
observe live authorization being granted by a paper transition.

## 15. Can the GUI fit safely in this bundle?

Yes, bounded to the existing `core-rs/mqk-gui/src/features/strategyScanner/`
feature (no new screen registration). The existing screen already reads
review artifacts (`getStrategyScanReviewArtifact` in
`strategyScanner/api.ts`) and displays a `paper_candidate` table — Phase
E adds a promotion panel alongside it, following the exact same
`fetchJsonCandidate`/`postJson({ privileged: true })` pattern already
used by `submitStrategyScanJob`. If Phase E is put at risk by the
runtime-gate work overrunning, it is reported `PARTIAL` per the mission's
explicit permission to do so — Phase D (DB/API/runtime enforcement) is
the load-bearing safety work and will not be compromised to finish GUI
polish.

---

## Design summary (contract, binding for Phases B–F)

- **States**: `shadow_approved`, `paper_approved`, `active_paper`,
  `demoted`, `retired`, `rejected` (snake_case, exactly as specified).
- **Identity**: `strategy_id` (trimmed) + `symbol` (uppercased) +
  `timeframe_secs` (canonical `i64`, converted from scanner string
  labels only at evidence-validation time). No wildcards accepted
  anywhere (`symbol` / a literal `*` string is rejected as invalid
  input, never stored).
- **Config fingerprint**: identity-v1 bounded fallback. Column exists,
  always `NULL` this patch, `config_identity_status =
  "unavailable_in_current_runtime"` surfaced on every read. Recorded as
  the sole `PARTIAL` item.
- **Storage**: one append-only table, `sys_strategy_promotion_transitions`,
  migration `0046`. Current state is always a deterministic query over
  history, never a separately mutable row.
- **Runtime rule**: only `active_paper`, unexpired, exact identity match,
  may create a new paper outbox row. Every other state, no record,
  expired approval, or unavailable truth denies.
- **Evidence trust boundary**: initial/re-approval transitions
  independently read and validate the review artifact (canonicalize +
  root-bounded inside `MQK_STRATEGY_REVIEW_ARTIFACT_ROOT`, exact
  identity match, `review_state == paper_candidate` required); caller
  claims about evidence state are never trusted.
- **Live boundary**: a paper promotion state must never authorize a LIVE run or live-routing path.
  No live-authorization code path exists in
  this patch. Read routes hard-code `tradable_live: false`.
- **No automatic promotion**: registry `enabled=true` never implies any
  promotion state; migration performs no backfill; scanner rank/score
  alone can never produce a transition row — only an operator-authenticated
  `POST .../transition` with independently-validated `paper_candidate`
  evidence can create the first `shadow_approved`/`paper_approved` row.

This design does not authorize automatic promotion, live trading,
wildcard identity, backfill from `sys_strategy_registry.enabled`, or
scanner-rank-only approval — enforced structurally (no code path exists
for any of these) and checked mechanically by
`scripts/guards/validate_strategy_promotion_registry_01a_audit.ps1`.
