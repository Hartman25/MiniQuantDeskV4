# Strategy Promotion Registry Closure Repair 01A — Reproduction Audit

Patch: `STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01-COMBINED`
Repairs: `STRATEGY-PROMOTION-REGISTRY-AND-RUNTIME-ENFORCEMENT-01-COMBINED`
(closed `CLOSED_LOCAL` at `6631be67`, per
`docs/specs/strategy_promotion_registry_01f_closure_decision.md`).

Starting HEAD: `6631be67` on `main`, matching the mission's expected
starting point exactly.

This document reproduces, by direct code inspection (not assumption),
each of the five defects named in the repair mission. All five are
confirmed present at HEAD `6631be67`.

## 1. Future-effective transitions become active prematurely

`mqk_db::evaluate_promotion_tradability`
(`core-rs/crates/mqk-db/src/strategy_promotion.rs:449-476`) branches on
`record.new_state` and, for `active_paper`, checks only
`expires_at_utc` — it never reads or compares `record.effective_at_utc`
against `now_utc` at all:

```rust
PROMOTION_STATE_ACTIVE_PAPER => {
    if let Some(expires_at) = record.expires_at_utc {
        if expires_at < now_utc {
            return (false, PromotionReasonCode::PromotionExpired);
        }
    }
    (true, PromotionReasonCode::PromotionActive)
}
```

Because `fetch_current_promotion_state`
(`strategy_promotion.rs:297-321`) selects the latest row ordered by
`effective_at_utc desc` with **no** `where effective_at_utc <= now`
filter, an operator-submitted `active_paper` transition with
`effective_at_utc` set an hour (or a year) in the future becomes both
"current" and immediately `paper_tradable = true` the instant it is
inserted — not at its stated effective time. This defeats the entire
purpose of a caller-supplied `effective_at_utc` field: it is currently
decorative metadata, not an enforced gate.

**Reproduction test** (new, Phase C target):
`future_effective_active_paper_not_yet_tradable` — insert an
`active_paper` transition with `effective_at_utc = now + 1 hour`, then
call `evaluate_promotion_tradability` at `now`. Current behavior:
`tradable = true`. Required behavior: `tradable = false`.

## 2. Concurrent transitions can branch history

`strategy_promotion_transition`
(`core-rs/crates/mqk-daemon/src/routes/strategy_promotions.rs:524-824`)
computes `current` (Gate 2, line 677-694) with one `SELECT`, then later
performs the `INSERT` (line 784) as a **separate** statement/connection
acquisition, with no transaction spanning the two, no row lock, and no
advisory lock. Two concurrent requests for the same identity (e.g. two
operators both promoting `paper_approved -> active_paper`, or one
promoting to `active_paper` while another demotes) can both:

1. read the same `previous_state` (Gate 2),
2. both pass `is_legal_transition` (Gate 3) against that same
   `previous_state`,
3. both `INSERT` successfully (`ON CONFLICT (transition_id) DO NOTHING`
   only de-duplicates an *identical* `transition_id` — two distinct
   requests produce two distinct deterministic ids, so both inserts
   succeed).

The `sys_strategy_promotion_transitions_legal_graph` `CHECK` constraint
only constrains `(previous_state, new_state)` pairs in isolation — it
has no way to see that two sibling rows now claim the same
`previous_state`, so the history branches: two children of one parent,
with no DB-level or application-level mechanism that prevents it or
even detects it after the fact. `fetch_current_promotion_state`'s tie-
break (`created_at_utc desc, transition_id desc`) will silently pick
one branch as "current," discarding the other's legitimacy without any
operator-visible conflict signal.

**Reproduction test** (new, Phase B target):
`concurrent_transitions_from_same_parent_cannot_both_be_accepted` —
race two inserts sharing the same `previous_state`/parent from separate
tokio tasks against the same identity; current behavior: both succeed
(2 children of 1 parent, confirmed by counting rows with
`previous_state = 'paper_approved'` for the same identity > 1).
Required: exactly one succeeds, the other is rejected with a stable
`transition_conflict` reason.

## 3. Evidence provenance is lost past `shadow_approved`

`InsertStrategyPromotionTransitionArgs.evidence_*` fields
(`strategy_promotion.rs:154-162`) are populated by the route handler
only when `transition_requires_evidence(previous_state, target_state)`
is true — i.e. only for `no-state -> shadow_approved` and
`demoted -> shadow_approved`
(`strategy_promotions.rs:718-761`). Every other transition
(`shadow_approved -> paper_approved`, `paper_approved -> active_paper`,
any `-> demoted`, any `-> retired`) is inserted with
`evidence_review_id = None`, `evidence_scanner_scan_id = None`,
`evidence_git_hash = None`, `evidence_artifact_path = None`,
`evidence_fingerprint = None` (`strategy_promotions.rs:772-776`).

Since `fetch_current_promotion_state` returns only the single latest
row, the **current** `active_paper` (or `paper_approved`) record for
any identity that has advanced past its initial `shadow_approved` row
carries `NULL` in every evidence column. `GET
/api/v1/strategy/promotions` and `GET .../promotions/check`
(`to_row`, `strategy_promotions.rs:57-87`) surface exactly what is on
that single row — so an operator inspecting a live `active_paper`
identity today sees no review ID, no scanner scan ID, no git hash, no
fingerprint, and no artifact path, even though real evidence exists
three rows back in history. The only way to find it today is to
manually walk `GET .../promotions/history` and locate the earliest
evidence-bearing row by hand — there is no durable, queryable link from
the current row back to it.

**Reproduction test** (new, Phase D target):
`active_paper_current_state_loses_evidence_lineage` — walk
`shadow_approved -> paper_approved -> active_paper`, then read the
current record for the identity; current behavior:
`evidence_review_id/scanner_scan_id/git_hash/fingerprint/artifact_path`
are all `None` even though the `shadow_approved` row three steps back
has them populated. Required: current state exposes the exact
evidence-bearing transition (id, review id, scanner scan id, git hash,
fingerprint, artifact path).

## 4. No explicit paper-only runtime authorization boundary

`promotion_gate::evaluate_paper_promotion_gate`
(`core-rs/crates/mqk-daemon/src/promotion_gate.rs:55-92`) takes exactly
`(db, strategy_id, symbol, timeframe_secs)` — **no run-mode or
deployment-mode parameter of any kind**. Both call sites —
`decision.rs:690-696` (`submit_internal_strategy_decision`, Gate 3b) and
`routes/strategy.rs:967-973` (`strategy_signal`, Gate 2b) — call it
without reading `state.deployment_mode()` / `st.deployment_mode()`
first, even though both files already import and use
`DeploymentMode`/`deployment_mode()` extensively elsewhere in the same
file (confirmed by grep: `deployment_mode()` appears at
`decision.rs:445,518,589` and dozens of times in
`routes/strategy.rs`, always for Discord notification labeling, never
as a promotion-gate precondition).

The current design doc
(`docs/specs/strategy_promotion_registry_01a_current_truth_and_contract.md`
§14) explicitly rationalizes this as safe **"by construction, not by a
runtime flag"** — i.e. the gate is safe today only because neither call
site happens to be reachable from a LIVE-mode run in the current
architecture. That is a structural assumption about the rest of the
codebase, not an enforced boundary at the gate itself: nothing stops a
future LIVE-mode call site (or a refactor that reuses
`submit_internal_strategy_decision`/`strategy_signal` from a
live-routing context) from silently inheriting paper-only authorization
with no compile-time or run-time signal. `PromotionReasonCode::
PromotionLiveNotAuthorized` already exists in the enum
(`strategy_promotion.rs:417`) specifically for this purpose but is
**never constructed by any code path** — it is dead documentation, not
enforcement.

**Reproduction test** (new, Phase E target):
`live_mode_promotion_gate_call_has_no_denial_path` — the gate function
signature itself has no mode parameter to pass `Live`/`Unknown` into;
this is a structural/compile-time gap, demonstrated by the fact that no
test in `scenario_strategy_promotion_runtime_gate_01.rs` (the existing
"Shadow / live boundaries" section, lines 950-996) exercises a
LIVE-mode caller at all — the closest existing test,
`paper_promotion_gate_never_authorizes_live`, only asserts that an
**already-paper-tradable** outcome's reason code isn't the live one; it
never calls the gate from a LIVE deployment-mode context, because doing
so is not currently possible without modifying the gate's own
signature.

## 5. Incorrect `CLOSED_LOCAL` disposition while configuration identity is unbound

`docs/specs/strategy_promotion_registry_01f_closure_decision.md` §1
states plainly:

> **Yes — `CLOSED_LOCAL`.**

for the entire
`STRATEGY-PROMOTION-REGISTRY-AND-RUNTIME-ENFORCEMENT-01-COMBINED` patch
group, while §5 and §9 of the *same document* state that configuration
fingerprint identity binding is `**PARTIAL**` and
`config_identity_status` remains permanently
`"unavailable_in_current_runtime"` for this patch. `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s
`STRATEGY-PROMOTION-REGISTRY-AND-RUNTIME-ENFORCEMENT-01-COMBINED` entry
carries the same top-level `CLOSED_LOCAL` disposition inherited from
this closure document.

Per this repo's own audit rules
(`.claude/rules/audit_repo_truth_rules.md`, "Honest status vocabulary"):
`CLOSED_LOCAL`/`CLOSED` requires the full contract to be satisfied;
`PARTIAL` is required whenever any required piece — here, the identity
boundary the mission's own Phase A design doc calls "binding for Phases
B-F" — remains open. A patch group cannot be `CLOSED_LOCAL` overall
while one of its own stated binding contract elements is honestly
reported as `PARTIAL` in the same breath. This is the exact "Incorrect
`CLOSED_LOCAL` disposition while configuration identity remains
unbound" defect named in this repair mission's item 5.

**Fix (Phase F):** re-audit whether a reproducible fingerprint can now
be added (see Phase F below); if not, correct the top-level disposition
to `PARTIAL` in both the closure document and the ledger, and record an
explicit open ledger item,
`STRATEGY-PROMOTION-CONFIG-IDENTITY-BINDING-01`.

## Summary of confirmed defects

| # | Defect | Location | Status |
|---|---|---|---|
| 1 | Future `effective_at_utc` ignored by tradability check | `mqk-db/src/strategy_promotion.rs:449-476`, `fetch_current_promotion_state` (no time filter) | Confirmed |
| 2 | No transactional/lock-based serialization of transitions | `mqk-daemon/src/routes/strategy_promotions.rs` (Gates 2-6, read-then-write, no lock) | Confirmed |
| 3 | Evidence fields not carried/linked forward past first evidence-bearing row | `strategy_promotions.rs:763-782` (`InsertStrategyPromotionTransitionArgs` construction) | Confirmed |
| 4 | Promotion gate has no run-mode parameter / no LIVE denial path | `mqk-daemon/src/promotion_gate.rs:55-92` and both call sites | Confirmed |
| 5 | Bundle disposition `CLOSED_LOCAL` despite `PARTIAL` identity binding | `docs/specs/strategy_promotion_registry_01f_closure_decision.md` §1 vs §5/§9 | Confirmed |

All five are repaired in Phases B–F of this patch, each with its own
commit and its own DB-backed proof test.
