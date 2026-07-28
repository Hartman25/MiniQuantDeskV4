#!/usr/bin/env bash
# MULTI-STRATEGY-CONFLICT-POLICY-01 (Bundle 6) closure guard.
#
# Static, grep-based proofs that do not require a live daemon or DB
# connection. These complement (never replace) the real cargo/npm test
# suites — see docs/specs/multi_strategy_conflict_policy_01a_current_truth_and_contract.md.
#
# Usage:
#   bash scripts/guards/check_multi_strategy_conflict_policy_01.sh          # run the real checks
#   bash scripts/guards/check_multi_strategy_conflict_policy_01.sh --self-test
#     # proves each check actually fails against a deliberately mutated copy
#     # of the relevant file (mutation-negative fixtures)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

fail() {
  echo "[msc-guard] FAIL: $*" >&2
  exit 1
}
ok() {
  echo "[msc-guard] OK: $*"
}

LOOP_RUNNER="core-rs/crates/mqk-daemon/src/state/loop_runner.rs"
CONFLICT_MODE="core-rs/crates/mqk-daemon/src/runtime_strategy_conflict_mode.rs"
CONFLICT_RUNTIME="core-rs/crates/mqk-daemon/src/runtime_strategy_conflict.rs"
CONFLICT_ROUTE="core-rs/crates/mqk-daemon/src/routes/strategy_conflict.rs"
CONFLICT_PANEL="core-rs/mqk-gui/src/features/system/StrategyConflictPolicyPanel.tsx"
CONFLICT_POLICY_RS="core-rs/crates/mqk-portfolio/src/conflict_policy.rs"
NATIVE_STRATEGY_RS="core-rs/crates/mqk-runtime/src/native_strategy.rs"
MIGRATION_0056="core-rs/crates/mqk-db/migrations/0056_runtime_strategy_conflict_plans.sql"
# AUTHORITY-AND-EVIDENCE-REPAIR-01 additions.
CONFLICT_DB_STORE="core-rs/crates/mqk-db/src/runtime_strategy_conflict.rs"
CONFLICT_EVIDENCE_VALIDATION="core-rs/crates/mqk-daemon/src/conflict_evidence_validation.rs"
MIGRATION_0057="core-rs/crates/mqk-db/migrations/0057_runtime_strategy_conflict_evidence_provenance.sql"
# FINAL-IDENTITY-AND-READ-AUTHORITY-REPAIR-01 additions.
CONFLICT_TYPES_TS="core-rs/mqk-gui/src/features/system/types/strategyConflict.ts"

# ---------------------------------------------------------------------------
# Check functions — each takes the file path(s) to check, so the self-test
# below can point them at a mutated scratch copy instead of the real tree.
# Each prints "PASS"/"FAIL:<reason>" to stdout and never exits the process.
# ---------------------------------------------------------------------------

check_ordering() {
  local loop_runner="$1"
  [[ -f "$loop_runner" ]] || { echo "FAIL:missing $loop_runner"; return; }
  local conflict_line allocation_line cap6_line submit_line
  conflict_line="$(grep -n 'runtime_strategy_conflict::gather_and_resolve' "$loop_runner" | head -1 | cut -d: -f1 || true)"
  allocation_line="$(grep -n 'runtime_opportunity_allocation::gather_and_apply' "$loop_runner" | head -1 | cut -d: -f1 || true)"
  cap6_line="$(grep -n 'AppState::max_new_orders_per_tick_reason' "$loop_runner" | head -1 | cut -d: -f1 || true)"
  submit_line="$(grep -n 'crate::decision::submit_internal_strategy_decision' "$loop_runner" | head -1 | cut -d: -f1 || true)"
  if [[ -z "$conflict_line" || -z "$allocation_line" || -z "$cap6_line" || -z "$submit_line" ]]; then
    echo "FAIL:one or more required call sites missing"
    return
  fi
  if [[ "$conflict_line" -lt "$allocation_line" && "$allocation_line" -lt "$cap6_line" && "$cap6_line" -lt "$submit_line" ]]; then
    echo "PASS"
  else
    echo "FAIL:ordering violated (conflict=$conflict_line allocation=$allocation_line cap6=$cap6_line submit=$submit_line)"
  fi
}

check_default_off() {
  local mode_file="$1"
  [[ -f "$mode_file" ]] || { echo "FAIL:missing $mode_file"; return; }
  if grep -A3 'let Some(value) = trimmed else {' "$mode_file" | grep -q 'ConflictPolicyMode::Off'; then
    echo "PASS"
  else
    echo "FAIL:absent/blank env var does not default to Off"
  fi
}

check_live_lock() {
  local mode_file="$1"
  [[ -f "$mode_file" ]] || { echo "FAIL:missing $mode_file"; return; }
  if grep -q 'is_paper_alpaca' "$mode_file" && grep -q 'ConflictPolicyMode::Off' "$mode_file"; then
    echo "PASS"
  else
    echo "FAIL:live-lock predicate (paper+Alpaca) missing"
  fi
}

check_approved_for_live_false() {
  local route_file="$1" panel_file="$2"
  [[ -f "$route_file" ]] || { echo "FAIL:missing $route_file"; return; }
  [[ -f "$panel_file" ]] || { echo "FAIL:missing $panel_file"; return; }
  if grep -rn 'approved_for_live' "$route_file" "$panel_file" 2>/dev/null | grep -qvE 'false|approved_for_live: bool'; then
    echo "FAIL:approved_for_live is not a hardcoded false everywhere it appears"
  else
    echo "PASS"
  fi
}

check_get_only() {
  local routes_file="$1"
  [[ -f "$routes_file" ]] || { echo "FAIL:missing $routes_file"; return; }
  if grep -A2 '/api/v1/strategy/conflict/status"' "$routes_file" | grep -qE 'post\(|put\(|delete\(|patch\('; then
    echo "FAIL:conflict routes are not GET-only"
  else
    echo "PASS"
  fi
}

check_gui_read_only() {
  local panel_file="$1"
  [[ -f "$panel_file" ]] || { echo "FAIL:missing $panel_file"; return; }
  if grep -qE 'method:\s*["'"'"'](POST|PUT|DELETE|PATCH)' "$panel_file"; then
    echo "FAIL:GUI panel performs a mutation fetch"
    return
  fi
  if grep -q '<button' "$panel_file"; then
    echo "FAIL:GUI panel contains an interactive mutation control"
    return
  fi
  echo "PASS"
}

check_no_broker_outbox() {
  local f
  for f in "$@"; do
    [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
    if grep -qE 'outbox_enqueue\(|AlpacaBroker|reqwest::' "$f"; then
      echo "FAIL:$f calls outbox/broker directly"
      return
    fi
  done
  echo "PASS"
}

check_no_dynamic_selection() {
  local f
  for f in "$@"; do
    [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
    if grep -qiE 'dynamic.strategy.selection|alpha.score|expected.return|backtest.rank|score_candidate' "$f"; then
      echo "FAIL:$f references dynamic-selection or score-ranking"
      return
    fi
  done
  echo "PASS"
}

# ---------------------------------------------------------------------------
# AUTHORITY-AND-EVIDENCE-REPAIR-01 mutation-negative check functions.
# ---------------------------------------------------------------------------

# A sole valid BUY in a multi-candidate group must never pass alongside an
# invalid sibling -- proven by the ambiguous-invalid-competitor reason
# constant being both defined AND actually referenced (a definition with no
# reference would mean the dead-code path never runs).
check_ambiguous_buy_refused() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  local hits
  hits="$(grep -c 'REASON_AMBIGUOUS_INVALID_COMPETITOR_REFUSED' "$f" || true)"
  if [[ "$hits" -ge 2 ]]; then
    echo "PASS"
  else
    echo "FAIL:ambiguous-invalid-competitor reason not defined+referenced ($hits occurrence(s))"
  fi
}

# A sell candidate must require full bar provenance exactly like a buy
# (only the positive-close requirement differs) -- proven by the absence of
# the old Buy-only exemption gate and the presence of the unconditional
# per-side match inside the bar check.
check_sell_bar_provenance_required() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if grep -q 'if let Side::Buy = side {' "$f"; then
    echo "FAIL:sell candidates are exempted from the bar-provenance check (Buy-only gate present)"
    return
  fi
  if ! grep -q 'Side::Sell => true' "$f"; then
    echo "FAIL:sell branch of the bar-provenance close check is missing"
    return
  fi
  echo "PASS"
}

# Bar timeframe must be checked against each candidate's own timeframe_secs
# (via a pure secs->string table), never a single caller-supplied global
# timeframe string.
check_per_candidate_timeframe_authority() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -q 'fn canonical_timeframe_str' "$f"; then
    echo "FAIL:canonical_timeframe_str (timeframe_secs -> string) converter missing"
    return
  fi
  if ! grep -q 'canonical_timeframe_str(c.timeframe_secs)' "$f"; then
    echo "FAIL:bar-timeframe check is not driven by the candidate's own timeframe_secs"
    return
  fi
  if grep -qE '^\s*pub timeframe: String,' "$f"; then
    echo "FAIL:the removed caller-global 'timeframe' field is still present on ConflictCandidateInput"
    return
  fi
  echo "PASS"
}

# Cycle identity must include the configured and effective mode -- the
# defect that let identical candidates in shadow vs paper_enforced collide
# on the same plan_id.
#
# FINAL-IDENTITY-AND-READ-AUTHORITY-REPAIR-01: the seed construction no
# longer uses a single interpolated format!() string with named
# `{configured}`/`{effective}` placeholders -- it builds a `top_fields`
# array (each element length-prefixed via `lp()`, Defect 3's unambiguous
# encoding) that includes `configured_mode.as_str().to_string()` and
# `effective_mode.as_str().to_string()` explicitly.
check_mode_in_cycle_identity() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -q 'configured_mode: ConflictPolicyMode' "$f"; then
    echo "FAIL:compute_conflict_cycle_id does not take a configured_mode parameter"
    return
  fi
  if ! grep -qF 'configured_mode.as_str().to_string()' "$f" \
      || ! grep -qF 'effective_mode.as_str().to_string()' "$f"; then
    echo "FAIL:the cycle-id seed string does not include both configured and effective mode"
    return
  fi
  echo "PASS"
}

# Cycle identity must include full bar provenance (explicit presence flag
# and close) -- not just bar_end_ts.
#
# FINAL-IDENTITY-AND-READ-AUTHORITY-REPAIR-01: `bar_present` and
# `close_micros` are now folded into the seed via the `fields` array inside
# `candidate_identity_seed` (length-prefixed, Defect 3), not a named
# `{bar_present}`/`{close_micros}` format placeholder.
check_bar_presence_in_cycle_identity() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -q 'let bar_present' "$f"; then
    echo "FAIL:cycle identity seed has no explicit bar-presence field"
    return
  fi
  if ! grep -qF 'bar_present.to_string(),' "$f"; then
    echo "FAIL:bar_present is computed but not folded into the identity seed fields"
    return
  fi
  if ! grep -qE '^\s*close_micros,\s*$' "$f"; then
    echo "FAIL:close_micros is not folded into the identity seed fields"
    return
  fi
  echo "PASS"
}

# A plan_id match must never be accepted as idempotent when the stored
# payload diverges from the replay -- proven by the PayloadCollision
# outcome variant existing and being reachable from the existence-check
# branch (not just declared and unused).
check_divergent_replay_rejected() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -q 'PayloadCollision' "$f"; then
    echo "FAIL:no PayloadCollision outcome -- a plan_id match may be accepted as idempotent unconditionally"
    return
  fi
  if ! grep -qE 'plans_match\s*&&\s*candidates_match' "$f"; then
    echo "FAIL:no canonical full-payload comparison gating the idempotent-replay outcome"
    return
  fi
  echo "PASS"
}

# The API must run the shared evidence validator before ever projecting a
# persisted row as active truth.
check_api_evidence_validation_present() {
  local route_file="$1" validator_file="$2"
  [[ -f "$route_file" ]] || { echo "FAIL:missing $route_file"; return; }
  [[ -f "$validator_file" ]] || { echo "FAIL:missing $validator_file"; return; }
  if ! grep -q 'fn validate_plan_with_candidates' "$validator_file"; then
    echo "FAIL:shared validator validate_plan_with_candidates is missing"
    return
  fi
  if ! grep -q 'validate_plan_with_candidates' "$route_file"; then
    echo "FAIL:routes/strategy_conflict.rs never calls the shared evidence validator"
    return
  fi
  if ! grep -q 'InvalidEvidence' "$route_file"; then
    echo "FAIL:routes/strategy_conflict.rs has no invalid_evidence truth_state"
    return
  fi
  echo "PASS"
}

# status must inspect only the single latest plan (limit=1) and return on a
# validation failure rather than looping/falling back to an older plan.
check_status_no_fallback_on_invalid_latest() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -qE 'fetch_recent_runtime_strategy_conflict_plans\(db, run\.run_id, 1\)' "$f"; then
    echo "FAIL:status no longer bounds itself to exactly the single latest plan (limit=1)"
    return
  fi
  if ! grep -q 'if !validation.valid {' "$f"; then
    echo "FAIL:status has no explicit validation-failure branch for the latest plan"
    return
  fi
  echo "PASS"
}

# ---------------------------------------------------------------------------
# FINAL-IDENTITY-AND-READ-AUTHORITY-REPAIR-01 mutation-negative check
# functions (Defects 1-8 final repair).
# ---------------------------------------------------------------------------

# Defect 1: no global first-assignment timeframe in Bundle 6 identity --
# `compute_conflict_cycle_id` must not take a global `timeframe: &str`
# parameter, and `loop_runner.rs` must not pass `dispatch_timeframe` into
# the `gather_and_resolve` call site.
check_no_global_timeframe_in_bundle6_identity() {
  local runtime_file="$1" loop_runner="$2"
  [[ -f "$runtime_file" ]] || { echo "FAIL:missing $runtime_file"; return; }
  [[ -f "$loop_runner" ]] || { echo "FAIL:missing $loop_runner"; return; }

  local sig
  sig="$(awk '/pub fn compute_conflict_cycle_id\(/,/-> String \{/' "$runtime_file")"
  if [[ -z "$sig" ]]; then
    echo "FAIL:compute_conflict_cycle_id function not found"
    return
  fi
  if echo "$sig" | grep -qE 'timeframe: &str'; then
    echo "FAIL:compute_conflict_cycle_id still takes a global timeframe parameter"
    return
  fi

  local call_block
  call_block="$(awk '/runtime_strategy_conflict::gather_and_resolve\(/,/\.await;/' "$loop_runner")"
  if [[ -z "$call_block" ]]; then
    echo "FAIL:gather_and_resolve call site not found in loop_runner"
    return
  fi
  if echo "$call_block" | grep -q 'dispatch_timeframe'; then
    echo "FAIL:loop_runner still passes dispatch_timeframe into gather_and_resolve"
    return
  fi
  echo "PASS"
}

# Defect 2: DB replay comparison must be ordinal-independent -- a canonical
# sorted Vec<CandidateSnapshot>, never a BTreeMap keyed by ordinal. Matches
# only an actual type position (`->` or `:` immediately before), so the
# module's own historical doc-comment describing the prior (fixed) defect
# never self-trips this check.
check_replay_ordinal_independent() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if grep -qE '(->|:)\s*BTreeMap<i32,\s*CandidateSnapshot>' "$f"; then
    echo "FAIL:candidate replay comparison is still keyed by ordinal (BTreeMap<i32, CandidateSnapshot>)"
    return
  fi
  if ! grep -qE 'Vec<CandidateSnapshot>' "$f"; then
    echo "FAIL:candidate replay comparison no longer produces a canonical sorted Vec<CandidateSnapshot>"
    return
  fi
  if ! grep -q 'snapshots.sort();' "$f"; then
    echo "FAIL:candidate snapshots are not canonically sorted before comparison"
    return
  fi
  echo "PASS"
}

# Defect 3: read-side validation must recompute the deterministic cycle id
# from reconstructed durable evidence and require it to equal the
# persisted plan_id/cycle_id.
check_read_validation_recomputes_cycle_id() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -q 'compute_conflict_cycle_id(' "$f"; then
    echo "FAIL:read-side validator no longer recomputes the cycle id"
    return
  fi
  if ! grep -q 'recomputed_cycle_id' "$f"; then
    echo "FAIL:read-side validator does not compare a recomputed cycle id against persisted evidence"
    return
  fi
  echo "PASS"
}

# Defect 4: read-side validation must re-run the pure resolver against the
# reconstructed candidate batch and compare every recomputed candidate
# outcome (selected/disposition/reason_code/proposed_target_qty) against
# persisted evidence.
check_read_validation_reruns_pure_resolver() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -q 'resolve_conflict_cycle(cycle_context, &inputs)' "$f"; then
    echo "FAIL:read-side validator no longer re-runs the pure resolver against reconstructed candidates"
    return
  fi
  if ! grep -q 'expected.selected != c.selected' "$f"; then
    echo "FAIL:recomputed candidate outcome is no longer compared against persisted selected/disposition/reason/target"
    return
  fi
  echo "PASS"
}

# Defect 4: a selected candidate with no proposed_target_qty must be
# rejected, never silently accepted.
check_selected_target_none_rejected() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -q 'c.selected && c.proposed_target_qty.is_none()' "$f"; then
    echo "FAIL:a selected candidate with no proposed_target_qty is no longer rejected"
    return
  fi
  echo "PASS"
}

# Defect 4: a persisted configured_mode/mode combination the runtime can
# never actually produce must be rejected (mode incoherence).
check_mode_coherence_enforced() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -q 'cm != plan.mode.as_str()' "$f"; then
    echo "FAIL:configured_mode/mode coherence is no longer enforced"
    return
  fi
  echo "PASS"
}

# Defect 4: symbol_group_count must be reconciled against the candidates'
# actual canonical symbol groups, not merely trusted from the stored field.
check_symbol_group_count_recomputed() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -q 'canonical_symbol_groups' "$f"; then
    echo "FAIL:symbol_group_count is no longer reconciled against actual canonical symbol groups"
    return
  fi
  echo "PASS"
}

# Defect 5: a per-row detail/candidate query error while building the
# plans list must return query_failed with no active list projection --
# never folded into excluded_malformed_count under an "active" truth_state.
# A vanished row must remain distinguishable via its own counter.
check_list_query_failure_distinct() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -q 'excluded_vanished_count' "$f"; then
    echo "FAIL:vanished-row count is no longer distinguished from malformed count"
    return
  fi
  local err_block
  err_block="$(awk '/strategy_conflict_plans_row_detail_query_failed/,/into_response\(\);/' "$f")"
  if [[ -z "$err_block" ]]; then
    echo "FAIL:list route detail-query error arm not found"
    return
  fi
  if echo "$err_block" | grep -q 'excluded_malformed_count += 1'; then
    echo "FAIL:a per-row query failure is still folded into excluded_malformed_count"
    return
  fi
  if ! echo "$err_block" | grep -q 'ConflictTruthState::QueryFailed'; then
    echo "FAIL:a per-row query failure does not return truth_state=query_failed"
    return
  fi
  echo "PASS"
}

# Defect 6: deterministic latest/history ordering must keep the plan_id
# DESC tie-break alongside created_at_utc DESC.
check_latest_ordering_tie_break() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -qiE 'order by created_at_utc desc, plan_id desc' "$f"; then
    echo "FAIL:deterministic latest/history ordering lost the plan_id DESC tie-break"
    return
  fi
  echo "PASS"
}

# Defect 7: the API candidate row (and its GUI type mirror) must expose
# full 0057 provenance, not just bar_end_ts.
check_api_provenance_fields_present() {
  local route_file="$1" ts_types_file="$2"
  [[ -f "$route_file" ]] || { echo "FAIL:missing $route_file"; return; }
  [[ -f "$ts_types_file" ]] || { echo "FAIL:missing $ts_types_file"; return; }
  local field
  for field in order_type time_in_force limit_price bar_present bar_symbol bar_strategy_id bar_timeframe close_micros; do
    if ! grep -q "pub $field:" "$route_file"; then
      echo "FAIL:API candidate row is missing provenance field '$field'"
      return
    fi
    if ! grep -q "$field" "$ts_types_file"; then
      echo "FAIL:GUI candidate row type is missing provenance field '$field'"
      return
    fi
  done
  echo "PASS"
}

# Defect 8: total blockers and per-plan candidate count must both be
# bounded, with a truncation-truth message when exceeded.
check_blocker_bounds_enforced() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -q 'MAX_BLOCKERS' "$f" || ! grep -q 'MAX_BLOCKER_LEN' "$f" || ! grep -q 'MAX_CANDIDATES_PER_PLAN' "$f"; then
    echo "FAIL:blocker/candidate bounds (MAX_BLOCKERS/MAX_BLOCKER_LEN/MAX_CANDIDATES_PER_PLAN) are missing"
    return
  fi
  if ! grep -q 'fn bound_blockers(' "$f"; then
    echo "FAIL:bound_blockers truncation helper is missing"
    return
  fi
  echo "PASS"
}

# ---------------------------------------------------------------------------
# UTF8-AND-BOUNDED-READ-CLOSURE mutation-negative check functions.
# ---------------------------------------------------------------------------

# Defect 1: blocker truncation must use an explicit UTF-8 character-boundary
# helper, never blind byte-offset truncation.
check_utf8_safe_truncation_helper() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -q 'fn safe_truncate_utf8(' "$f"; then
    echo "FAIL:safe_truncate_utf8 UTF-8 char-boundary helper is missing"
    return
  fi
  if ! grep -q 'is_char_boundary' "$f"; then
    echo "FAIL:truncation helper does not use is_char_boundary"
    return
  fi
  echo "PASS"
}

# Defect 1: no direct String::truncate(MAX_BLOCKER_LEN) may remain in
# blocker-bounding code -- it panics on a non-char-boundary offset.
check_no_raw_truncate_in_blocker_bounding() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if grep -q 'truncate(MAX_BLOCKER_LEN)' "$f"; then
    echo "FAIL:direct truncate(MAX_BLOCKER_LEN) call present -- must use safe_truncate_utf8 instead"
    return
  fi
  if ! grep -q 'safe_truncate_utf8(' "$f"; then
    echo "FAIL:bound_blockers no longer calls safe_truncate_utf8"
    return
  fi
  echo "PASS"
}

# Defect 1: the truncation suffix's byte length must be reserved out of
# max_len before any content is kept, so content + suffix never exceeds it.
check_suffix_reserved_within_max() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -qE 'max_len\s*-\s*suffix\.len\(\)' "$f"; then
    echo "FAIL:truncation budget does not reserve the suffix length out of max_len"
    return
  fi
  echo "PASS"
}

# Defect 2: the bounded read-side fetch seam must use a parameterized SQL
# LIMIT of bound + 1, and must return an explicit CandidateLimitExceeded
# outcome rather than silently truncating.
check_sql_bounded_limit() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  local body
  body="$(awk '/pub async fn fetch_runtime_strategy_conflict_plan_for_read\(/,/^}/' "$f")"
  if [[ -z "$body" ]]; then
    echo "FAIL:fetch_runtime_strategy_conflict_plan_for_read is missing"
    return
  fi
  if ! echo "$body" | grep -qE 'limit \$2'; then
    echo "FAIL:bounded read seam's candidate query has no parameterized SQL LIMIT"
    return
  fi
  if ! echo "$body" | grep -qE 'saturating_add\(1\)|bound *\+ *1'; then
    echo "FAIL:bounded read seam does not fetch bound + 1 rows"
    return
  fi
  if ! echo "$body" | grep -q 'CandidateLimitExceeded'; then
    echo "FAIL:bounded read seam never returns CandidateLimitExceeded"
    return
  fi
  echo "PASS"
}

# Defect 2: status/list/detail routes must all call the bounded read seam,
# and none may call the unbounded full candidate fetch directly.
check_routes_use_bounded_seam() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  local hits
  hits="$(grep -c 'fetch_runtime_strategy_conflict_plan_for_read' "$f" || true)"
  if [[ "$hits" -lt 3 ]]; then
    echo "FAIL:status/list/detail routes do not all call the bounded read seam (found $hits call site(s), need >= 3)"
    return
  fi
  if grep -qE 'fetch_runtime_strategy_conflict_plan\(' "$f"; then
    echo "FAIL:a route still calls the unbounded fetch_runtime_strategy_conflict_plan directly"
    return
  fi
  echo "PASS"
}

# Defect 2: the unbounded full-payload fetch (reserved for internal DB
# replay/collision comparison) must never itself gain a LIMIT.
check_full_replay_path_unbounded() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  local body
  body="$(awk '/pub async fn fetch_runtime_strategy_conflict_plan\(/,/^}/' "$f")"
  if [[ -z "$body" ]]; then
    echo "FAIL:fetch_runtime_strategy_conflict_plan (unbounded replay fetch) is missing"
    return
  fi
  if echo "$body" | grep -qiE '\blimit\b'; then
    echo "FAIL:the unbounded full-payload fetch has been accidentally limited"
    return
  fi
  echo "PASS"
}

# Defect 2: status must fail closed to invalid_evidence for an over-limit
# latest plan -- never active, never query_failed.
check_status_over_limit_is_invalid_evidence() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -A6 'the durable plan exceeds the shared bounded-read' "$f" | grep -q 'ConflictTruthState::InvalidEvidence'; then
    echo "FAIL:status route no longer maps an over-limit latest plan to invalid_evidence"
    return
  fi
  echo "PASS"
}

# Defect 2: detail must never project partial candidate data for an
# over-limit plan.
check_detail_over_limit_no_partial_candidates() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -A10 'over-limit plan -- invalid_evidence, no partial' "$f" | grep -q 'candidates: vec!\[\],'; then
    echo "FAIL:detail route projects partial/non-empty candidate data for an over-limit plan"
    return
  fi
  echo "PASS"
}

# Defect 2: list must count a successfully-read over-limit plan as
# malformed, never silently drop it from that count.
check_list_over_limit_counts_as_malformed() {
  local f="$1"
  [[ -f "$f" ]] || { echo "FAIL:missing $f"; return; }
  if ! grep -A4 'a successfully-read over-limit plan is' "$f" | grep -q 'excluded_malformed_count += 1;'; then
    echo "FAIL:list route no longer counts a successfully-read over-limit plan as malformed"
    return
  fi
  echo "PASS"
}

# Shared bound authority: the daemon's validator bound must reference the
# same mqk-db constant the SQL fetch uses, never its own hardcoded literal --
# proves the two bounds cannot silently drift apart.
check_shared_bound_no_drift() {
  local validator_file="$1" db_file="$2"
  [[ -f "$validator_file" ]] || { echo "FAIL:missing $validator_file"; return; }
  [[ -f "$db_file" ]] || { echo "FAIL:missing $db_file"; return; }
  if ! grep -q 'pub const RUNTIME_STRATEGY_CONFLICT_CANDIDATE_READ_BOUND' "$db_file"; then
    echo "FAIL:shared candidate-read bound constant is missing from mqk-db"
    return
  fi
  if ! grep -qE 'MAX_CANDIDATES_PER_PLAN: usize = mqk_db::RUNTIME_STRATEGY_CONFLICT_CANDIDATE_READ_BOUND' "$validator_file"; then
    echo "FAIL:daemon validator's MAX_CANDIDATES_PER_PLAN no longer references the shared mqk-db bound -- SQL fetch bound and validator bound can silently drift apart"
    return
  fi
  echo "PASS"
}

# ---------------------------------------------------------------------------
# Real-tree run
# ---------------------------------------------------------------------------

run_real_checks() {
  echo "[msc-guard] repo root: $REPO_ROOT"

  # 1. mqk-portfolio (conflict_policy model) stays a zero-dependency, pure
  #    crate: no DB/network/broker/random-number crate.
  if grep -qE '^\s*(sqlx|reqwest|tokio|rand|mqk-db|mqk-execution|mqk-broker)' \
      core-rs/crates/mqk-portfolio/Cargo.toml; then
    fail "mqk-portfolio must remain a zero-IO, zero-dependency crate"
  fi
  ok "mqk-portfolio has no DB/network/broker/random dependency"

  # 2. The pure conflict_policy model never calls the outbox or a broker
  #    adapter directly, and is never itself a ranking/scoring engine.
  local r2 r2b
  r2="$(check_no_broker_outbox "$CONFLICT_POLICY_RS")"
  [[ "$r2" == PASS ]] || fail "${r2#FAIL:}"
  ok "conflict_policy.rs never calls outbox/broker directly"
  r2b="$(check_no_dynamic_selection "$CONFLICT_POLICY_RS")"
  [[ "$r2b" == PASS ]] || fail "${r2b#FAIL:}"
  ok "conflict_policy.rs contains no dynamic-selection/score-ranking reference"

  # 3. Bundle 6's daemon-side modules never call outbox_enqueue or a broker
  #    adapter directly -- every surviving decision must still flow through
  #    the unchanged Bundle 5 -> submit_internal_strategy_decision seam.
  local r3
  r3="$(check_no_broker_outbox "$CONFLICT_MODE" "$CONFLICT_RUNTIME" "$CONFLICT_ROUTE")"
  [[ "$r3" == PASS ]] || fail "${r3#FAIL:}"
  ok "Bundle 6's daemon modules never call outbox/broker directly"

  # 4. Default mode is off.
  local r4
  r4="$(check_default_off "$CONFLICT_MODE")"
  [[ "$r4" == PASS ]] || fail "${r4#FAIL:}"
  ok "default conflict-policy mode is off"

  # 5. Live hard lock present.
  local r5
  r5="$(check_live_lock "$CONFLICT_MODE")"
  [[ "$r5" == PASS ]] || fail "${r5#FAIL:}"
  ok "live hard-lock predicate present"

  # 6. approved_for_live hardcoded false in API/GUI.
  local r6
  r6="$(check_approved_for_live_false "$CONFLICT_ROUTE" "$CONFLICT_PANEL")"
  [[ "$r6" == PASS ]] || fail "${r6#FAIL:}"
  ok "approved_for_live is hardcoded false in API/GUI surfaces"

  # 7. GET-only API.
  local r7
  r7="$(check_get_only core-rs/crates/mqk-daemon/src/routes.rs)"
  [[ "$r7" == PASS ]] || fail "${r7#FAIL:}"
  ok "conflict routes registered GET-only"

  # 8. GUI panel is read-only.
  local r8
  r8="$(check_gui_read_only "$CONFLICT_PANEL")"
  [[ "$r8" == PASS ]] || fail "${r8#FAIL:}"
  ok "conflict GUI panel is read-only (no mutation calls, no buttons)"

  # 9. Migration additive, no implicit time/uuid defaults.
  if grep -qiE '^\s*ALTER TABLE' "$MIGRATION_0056"; then
    fail "migration 0056 must be additive only (no ALTER TABLE on existing tables)"
  fi
  if grep -v '^\s*--' "$MIGRATION_0056" | grep -qiE 'DEFAULT\s+now\(\)|DEFAULT\s+gen_random_uuid\(\)'; then
    fail "migration 0056 must not use DEFAULT now()/gen_random_uuid()"
  fi
  ok "migration 0056 is additive, no implicit time/uuid defaults"

  # 10. Insertion point ordering: conflict resolution before Bundle 5 before
  #     cap #6 before canonical submission.
  local r10
  r10="$(check_ordering "$LOOP_RUNNER")"
  [[ "$r10" == PASS ]] || fail "${r10#FAIL:}"
  ok "insertion point ordering intact: conflict -> allocation -> cap#6 -> submit"

  # 11. NativeStrategyBootstrap still consumes only the source-proven
  #     current fleet behavior -- no multi-host runtime introduced. Proven
  #     by the exact "consume only the first fleet entry" single-strategy
  #     marker remaining present and MultiStrategyNotAllowed still being the
  #     second-registration outcome (mqk-strategy::host.rs is upstream of
  #     this crate and out of Bundle 6's file scope, so this check is
  #     narrowly scoped to the bootstrap seam Bundle 6's own docs rely on).
  if [[ -f "$NATIVE_STRATEGY_RS" ]] && grep -q 'Single-strategy Tier A policy: consume only the first fleet entry' "$NATIVE_STRATEGY_RS"; then
    ok "NativeStrategyBootstrap single-strategy-fleet behavior unchanged"
  else
    fail "NativeStrategyBootstrap's single-strategy-fleet marker is missing or changed"
  fi

  # 12. Watchlist one-strategy-per-symbol schema not widened: each symbol
  #     still maps to exactly one strategy_id via a HashMap<String,String>,
  #     not a Vec/multi-valued map.
  if grep -q 'strategy_assignments' core-rs/crates/mqk-daemon/src/watchlist_intake.rs 2>/dev/null \
      && ! grep -qE 'strategy_assignments:\s*(HashMap|BTreeMap)<String,\s*Vec<' core-rs/crates/mqk-daemon/src/watchlist_intake.rs; then
    ok "watchlist strategy_assignments remains one-strategy-per-symbol"
  else
    fail "watchlist strategy_assignments schema may have been widened to multi-strategy"
  fi

  # 13. Bundle 7 (dynamic strategy-symbol selection) not started.
  if git ls-files | grep -qiE 'dynamic.strategy.symbol|bundle.?7'; then
    fail "Bundle 7 (dynamic strategy-symbol selection) must not be started"
  fi
  ok "Bundle 7 not started"

  # 14. No AI/ML framework or provider reference anywhere in Bundle 6's new
  #     files.
  local bundle6_files
  bundle6_files="$(git diff --name-only a0852c2f6ffd2343c9b6740728abd5b7889bcb15...HEAD -- \
    core-rs/crates/mqk-portfolio/src/conflict_policy.rs \
    core-rs/crates/mqk-daemon/src/runtime_strategy_conflict.rs \
    core-rs/crates/mqk-daemon/src/runtime_strategy_conflict_mode.rs \
    core-rs/crates/mqk-daemon/src/routes/strategy_conflict.rs \
    core-rs/crates/mqk-db/src/runtime_strategy_conflict.rs \
    core-rs/mqk-gui/src/features/system/StrategyConflictPolicyPanel.tsx \
    core-rs/mqk-gui/src/features/system/strategyConflict.ts \
    core-rs/mqk-gui/src/features/system/types/strategyConflict.ts \
    2>/dev/null || true)"
  if [[ -n "$bundle6_files" ]]; then
    local ai_hits
    ai_hits="$(echo "$bundle6_files" | xargs -I{} grep -lIE \
      'openai|anthropic\.com|torch|tensorflow|onnxruntime|langchain' {} 2>/dev/null || true)"
    if [[ -n "$ai_hits" ]]; then
      echo "$ai_hits" >&2
      fail "no AI/ML framework or provider reference is permitted in Bundle 6 files"
    fi
  fi
  ok "no AI/ML reference found in Bundle 6 files"

  # ---------------------------------------------------------------------
  # AUTHORITY-AND-EVIDENCE-REPAIR-01 checks (15-22).
  # ---------------------------------------------------------------------

  # 15. Sole valid BUY refused alongside an invalid sibling.
  local r15
  r15="$(check_ambiguous_buy_refused "$CONFLICT_POLICY_RS")"
  [[ "$r15" == PASS ]] || fail "${r15#FAIL:}"
  ok "sole valid buy is refused (not authorized) alongside an invalid sibling"

  # 16. Sell candidates require full bar provenance, same as buys.
  local r16
  r16="$(check_sell_bar_provenance_required "$CONFLICT_POLICY_RS")"
  [[ "$r16" == PASS ]] || fail "${r16#FAIL:}"
  ok "sell candidates require bar provenance (not exempted)"

  # 17. Per-candidate timeframe authority (own timeframe_secs, not a global).
  local r17
  r17="$(check_per_candidate_timeframe_authority "$CONFLICT_POLICY_RS")"
  [[ "$r17" == PASS ]] || fail "${r17#FAIL:}"
  ok "bar-timeframe check is driven by each candidate's own timeframe_secs"

  # 18. Mode is part of cycle economic identity.
  local r18
  r18="$(check_mode_in_cycle_identity "$CONFLICT_RUNTIME")"
  [[ "$r18" == PASS ]] || fail "${r18#FAIL:}"
  ok "configured and effective mode are part of cycle identity"

  # 19. Bar presence/close are part of cycle economic identity.
  local r19
  r19="$(check_bar_presence_in_cycle_identity "$CONFLICT_RUNTIME")"
  [[ "$r19" == PASS ]] || fail "${r19#FAIL:}"
  ok "bar presence and close are part of cycle identity"

  # 20. Divergent same-plan_id payload is never accepted as idempotent.
  local r20
  r20="$(check_divergent_replay_rejected "$CONFLICT_DB_STORE")"
  [[ "$r20" == PASS ]] || fail "${r20#FAIL:}"
  ok "a divergent same-plan_id payload is rejected, not treated as idempotent replay"

  # 21. API runs the shared evidence validator before projecting active truth.
  local r21
  r21="$(check_api_evidence_validation_present "$CONFLICT_ROUTE" "$CONFLICT_EVIDENCE_VALIDATION")"
  [[ "$r21" == PASS ]] || fail "${r21#FAIL:}"
  ok "API validates durable evidence before projecting active truth"

  # 22. status never falls back from a malformed latest plan to an older one.
  local r22
  r22="$(check_status_no_fallback_on_invalid_latest "$CONFLICT_ROUTE")"
  [[ "$r22" == PASS ]] || fail "${r22#FAIL:}"
  ok "status inspects only the single latest plan and never falls back on malformed evidence"

  # 23. Migration 0057 additive, no implicit time/uuid defaults.
  if grep -qiE '^\s*DROP |^\s*ALTER TABLE .* DROP ' "$MIGRATION_0057"; then
    fail "migration 0057 must be additive only (no DROP)"
  fi
  if grep -v '^\s*--' "$MIGRATION_0057" | grep -qiE 'DEFAULT\s+now\(\)|DEFAULT\s+gen_random_uuid\(\)'; then
    fail "migration 0057 must not use DEFAULT now()/gen_random_uuid()"
  fi
  ok "migration 0057 is additive, no implicit time/uuid defaults"

  # ---------------------------------------------------------------------
  # FINAL-IDENTITY-AND-READ-AUTHORITY-REPAIR-01 checks (24-34).
  # ---------------------------------------------------------------------

  # 24. No global first-assignment timeframe in Bundle 6 identity.
  local r24
  r24="$(check_no_global_timeframe_in_bundle6_identity "$CONFLICT_RUNTIME" "$LOOP_RUNNER")"
  [[ "$r24" == PASS ]] || fail "${r24#FAIL:}"
  ok "Bundle 6 cycle identity has no global first-assignment timeframe"

  # 25. DB replay comparison is ordinal-independent.
  local r25
  r25="$(check_replay_ordinal_independent "$CONFLICT_DB_STORE")"
  [[ "$r25" == PASS ]] || fail "${r25#FAIL:}"
  ok "DB replay comparison is ordinal-independent (canonical sorted Vec)"

  # 26. Read validation recomputes the cycle id.
  local r26
  r26="$(check_read_validation_recomputes_cycle_id "$CONFLICT_EVIDENCE_VALIDATION")"
  [[ "$r26" == PASS ]] || fail "${r26#FAIL:}"
  ok "read-side validation recomputes and compares the cycle id"

  # 27. Read validation re-runs the pure resolver.
  local r27
  r27="$(check_read_validation_reruns_pure_resolver "$CONFLICT_EVIDENCE_VALIDATION")"
  [[ "$r27" == PASS ]] || fail "${r27#FAIL:}"
  ok "read-side validation re-runs the pure resolver and compares its outcome"

  # 28. Selected candidate with no target is rejected.
  local r28
  r28="$(check_selected_target_none_rejected "$CONFLICT_EVIDENCE_VALIDATION")"
  [[ "$r28" == PASS ]] || fail "${r28#FAIL:}"
  ok "a selected candidate with no proposed_target_qty is rejected"

  # 29. configured_mode/mode coherence is enforced.
  local r29
  r29="$(check_mode_coherence_enforced "$CONFLICT_EVIDENCE_VALIDATION")"
  [[ "$r29" == PASS ]] || fail "${r29#FAIL:}"
  ok "configured_mode/mode incoherence is rejected"

  # 30. symbol_group_count is reconciled.
  local r30
  r30="$(check_symbol_group_count_recomputed "$CONFLICT_EVIDENCE_VALIDATION")"
  [[ "$r30" == PASS ]] || fail "${r30#FAIL:}"
  ok "symbol_group_count is reconciled against actual canonical symbol groups"

  # 31. List query failure is distinct from malformed/active.
  local r31
  r31="$(check_list_query_failure_distinct "$CONFLICT_ROUTE")"
  [[ "$r31" == PASS ]] || fail "${r31#FAIL:}"
  ok "list route distinguishes query failure from malformed/vanished rows"

  # 32. Deterministic latest ordering keeps the plan_id tie-break.
  local r32
  r32="$(check_latest_ordering_tie_break "$CONFLICT_DB_STORE")"
  [[ "$r32" == PASS ]] || fail "${r32#FAIL:}"
  ok "deterministic latest/history ordering keeps the plan_id DESC tie-break"

  # 33. API/GUI provenance fields present.
  local r33
  r33="$(check_api_provenance_fields_present "$CONFLICT_ROUTE" "$CONFLICT_TYPES_TS")"
  [[ "$r33" == PASS ]] || fail "${r33#FAIL:}"
  ok "API/GUI expose full 0057 candidate provenance"

  # 34. Blocker/candidate bounds enforced.
  local r34
  r34="$(check_blocker_bounds_enforced "$CONFLICT_EVIDENCE_VALIDATION")"
  [[ "$r34" == PASS ]] || fail "${r34#FAIL:}"
  ok "blocker count/length and per-plan candidate count are bounded"

  # ---------------------------------------------------------------------
  # UTF8-AND-BOUNDED-READ-CLOSURE checks (35-43).
  # ---------------------------------------------------------------------

  # 35. UTF-8-safe truncation helper present and uses is_char_boundary.
  local r35
  r35="$(check_utf8_safe_truncation_helper "$CONFLICT_EVIDENCE_VALIDATION")"
  [[ "$r35" == PASS ]] || fail "${r35#FAIL:}"
  ok "blocker truncation uses an explicit UTF-8 character-boundary helper"

  # 36. No direct String::truncate(MAX_BLOCKER_LEN) in blocker bounding.
  local r36
  r36="$(check_no_raw_truncate_in_blocker_bounding "$CONFLICT_EVIDENCE_VALIDATION")"
  [[ "$r36" == PASS ]] || fail "${r36#FAIL:}"
  ok "no direct String::truncate(MAX_BLOCKER_LEN) remains in blocker-bounding code"

  # 37. Truncation suffix length is reserved within the maximum.
  local r37
  r37="$(check_suffix_reserved_within_max "$CONFLICT_EVIDENCE_VALIDATION")"
  [[ "$r37" == PASS ]] || fail "${r37#FAIL:}"
  ok "truncation suffix length is reserved out of the configured maximum"

  # 38. Bounded DB read seam uses a parameterized SQL LIMIT of bound + 1.
  local r38
  r38="$(check_sql_bounded_limit "$CONFLICT_DB_STORE")"
  [[ "$r38" == PASS ]] || fail "${r38#FAIL:}"
  ok "bounded read seam fetches at most bound + 1 candidate rows via SQL LIMIT"

  # 39. status/list/detail all use the bounded seam, never the unbounded fetch.
  local r39
  r39="$(check_routes_use_bounded_seam "$CONFLICT_ROUTE")"
  [[ "$r39" == PASS ]] || fail "${r39#FAIL:}"
  ok "status/list/detail routes all use the bounded read seam, never the unbounded fetch"

  # 40. Unbounded full-payload fetch (replay path) never gains a LIMIT.
  local r40
  r40="$(check_full_replay_path_unbounded "$CONFLICT_DB_STORE")"
  [[ "$r40" == PASS ]] || fail "${r40#FAIL:}"
  ok "the unbounded full-payload fetch used by internal DB replay remains unlimited"

  # 41. status maps an over-limit latest plan to invalid_evidence.
  local r41
  r41="$(check_status_over_limit_is_invalid_evidence "$CONFLICT_ROUTE")"
  [[ "$r41" == PASS ]] || fail "${r41#FAIL:}"
  ok "status maps an over-limit latest plan to invalid_evidence, never active/query_failed"

  # 42. detail projects no candidates for an over-limit plan.
  local r42
  r42="$(check_detail_over_limit_no_partial_candidates "$CONFLICT_ROUTE")"
  [[ "$r42" == PASS ]] || fail "${r42#FAIL:}"
  ok "detail never projects partial candidate data for an over-limit plan"

  # 43. list counts a successfully-read over-limit plan as malformed.
  local r43
  r43="$(check_list_over_limit_counts_as_malformed "$CONFLICT_ROUTE")"
  [[ "$r43" == PASS ]] || fail "${r43#FAIL:}"
  ok "list counts a successfully-read over-limit plan as malformed evidence"

  # 44. Shared bound authority: no drift between SQL fetch bound and
  #     validator bound.
  local r44
  r44="$(check_shared_bound_no_drift "$CONFLICT_EVIDENCE_VALIDATION" "$CONFLICT_DB_STORE")"
  [[ "$r44" == PASS ]] || fail "${r44#FAIL:}"
  ok "daemon validator bound and SQL fetch bound share one authoritative constant"

  echo "[msc-guard] ALL CHECKS PASSED"
}

# ---------------------------------------------------------------------------
# Self-test: mutation-negative fixtures
# ---------------------------------------------------------------------------

run_self_test() {
  local scratch
  scratch="$(mktemp -d)"
  # Double-quoted so $scratch expands now, at trap-set time -- a
  # single-quoted trap would defer expansion until the trap fires, by which
  # point this function-local variable has already fallen out of scope
  # under `set -u`.
  trap "rm -rf '$scratch'" EXIT

  local failures=0

  assert_now_fails() {
    local label="$1" result="$2"
    if [[ "$result" == PASS ]]; then
      echo "[msc-guard-selftest] FAIL: mutation '$label' was NOT caught (check still reports PASS)" >&2
      failures=$((failures + 1))
    else
      echo "[msc-guard-selftest] OK: mutation '$label' correctly caught (${result})"
    fi
  }

  # MUT-A: conflict resolution call site removed (simulates it being moved
  # or bypassed) -- the mutation must break the exact grep substring, not
  # just prefix it (a prefix would still contain the original substring and
  # silently pass).
  cp "$LOOP_RUNNER" "$scratch/loop_runner_mut_a.rs"
  sed -i 's/runtime_strategy_conflict::gather_and_resolve/runtime_strategy_conflict::XXXgather_and_resolve/' "$scratch/loop_runner_mut_a.rs"
  assert_now_fails "MUT-A conflict resolution call removed/moved" "$(check_ordering "$scratch/loop_runner_mut_a.rs")"

  # MUT-B: canonical submission call removed (simulates it moving before
  # cap #6 / being bypassed).
  cp "$LOOP_RUNNER" "$scratch/loop_runner_mut_b.rs"
  sed -i 's/crate::decision::submit_internal_strategy_decision/MUTATED_submit_internal_strategy_decision/' "$scratch/loop_runner_mut_b.rs"
  assert_now_fails "MUT-B canonical submission call renamed/removed" "$(check_ordering "$scratch/loop_runner_mut_b.rs")"

  # MUT-C: default mode becomes enforced (Off -> PaperEnforced on the
  # absent/blank branch).
  cp "$CONFLICT_MODE" "$scratch/mode_mut_c.rs"
  python - "$scratch/mode_mut_c.rs" <<'PY'
import sys
path = sys.argv[1]
text = open(path, encoding="utf-8").read()
anchor = "let Some(value) = trimmed else {"
idx = text.find(anchor)
end = text.find("};", idx)
block = text[idx:end]
mutated = block.replace("ConflictPolicyMode::Off", "ConflictPolicyMode::PaperEnforced")
text = text[:idx] + mutated + text[end:]
open(path, "w", encoding="utf-8").write(text)
PY
  assert_now_fails "MUT-C default mode changed to paper_enforced" "$(check_default_off "$scratch/mode_mut_c.rs")"

  # MUT-D: live hard lock removed (is_paper_alpaca predicate deleted). The
  # replacement must not contain "is_paper_alpaca" as a substring (a prefix
  # or suffix rename would still match the grep and silently pass).
  cp "$CONFLICT_MODE" "$scratch/mode_mut_d.rs"
  sed -i 's/is_paper_alpaca/isPaperAlpacaCheck/g' "$scratch/mode_mut_d.rs"
  assert_now_fails "MUT-D live hard-lock predicate removed" "$(check_live_lock "$scratch/mode_mut_d.rs")"

  # MUT-E: a broker/outbox call introduced into the pure conflict_policy
  # model.
  cp "$CONFLICT_POLICY_RS" "$scratch/conflict_policy_mut_e.rs"
  printf '\nfn _mut_e() { let _ = "outbox_enqueue("; }\n' >> "$scratch/conflict_policy_mut_e.rs"
  assert_now_fails "MUT-E broker/outbox call introduced" "$(check_no_broker_outbox "$scratch/conflict_policy_mut_e.rs")"

  # MUT-F: GUI gains a mutation control (<button>).
  cp "$CONFLICT_PANEL" "$scratch/panel_mut_f.tsx"
  printf '\n<button onClick={() => {}}>Enforce</button>\n' >> "$scratch/panel_mut_f.tsx"
  assert_now_fails "MUT-F GUI mutation button introduced" "$(check_gui_read_only "$scratch/panel_mut_f.tsx")"

  # MUT-G: dynamic-selection/score-ranking code introduced into the pure
  # model.
  cp "$CONFLICT_POLICY_RS" "$scratch/conflict_policy_mut_g.rs"
  printf '\n// score_candidate placeholder\n' >> "$scratch/conflict_policy_mut_g.rs"
  assert_now_fails "MUT-G dynamic-selection/score-ranking reference introduced" "$(check_no_dynamic_selection "$scratch/conflict_policy_mut_g.rs")"

  # MUT-H: the ambiguous-invalid-competitor refusal path is deleted/renamed
  # (simulates the sole-valid-buy branch going back to unconditional
  # passthrough regardless of invalid siblings).
  cp "$CONFLICT_POLICY_RS" "$scratch/conflict_policy_mut_h.rs"
  sed -i 's/REASON_AMBIGUOUS_INVALID_COMPETITOR_REFUSED/REASON_XXX_REMOVED/g' "$scratch/conflict_policy_mut_h.rs"
  assert_now_fails "MUT-H ambiguous-invalid-competitor refusal removed" "$(check_ambiguous_buy_refused "$scratch/conflict_policy_mut_h.rs")"

  # MUT-I: sell bar provenance is made optional again (Buy-only gate
  # reintroduced).
  cp "$CONFLICT_POLICY_RS" "$scratch/conflict_policy_mut_i.rs"
  printf '\nfn _mut_i_marker(side: &Side) { if let Side::Buy = side { let _ = 1; } }\n' >> "$scratch/conflict_policy_mut_i.rs"
  assert_now_fails "MUT-I sell bar provenance made optional" "$(check_sell_bar_provenance_required "$scratch/conflict_policy_mut_i.rs")"

  # MUT-J: per-candidate timeframe authority removed (bar-timeframe check no
  # longer driven by the candidate's own timeframe_secs).
  cp "$CONFLICT_POLICY_RS" "$scratch/conflict_policy_mut_j.rs"
  sed -i 's/canonical_timeframe_str(c\.timeframe_secs)/GLOBAL_TIMEFRAME_STR/g' "$scratch/conflict_policy_mut_j.rs"
  assert_now_fails "MUT-J per-candidate timeframe authority removed" "$(check_per_candidate_timeframe_authority "$scratch/conflict_policy_mut_j.rs")"

  # MUT-K: mode removed from cycle identity (seed fields array no longer
  # folds in configured/effective mode).
  cp "$CONFLICT_RUNTIME" "$scratch/conflict_runtime_mut_k.rs"
  sed -i '/configured_mode\.as_str()\.to_string(),/d' "$scratch/conflict_runtime_mut_k.rs"
  assert_now_fails "MUT-K mode removed from cycle identity seed" "$(check_mode_in_cycle_identity "$scratch/conflict_runtime_mut_k.rs")"

  # MUT-L: close/bar-presence removed from cycle identity seed fields.
  cp "$CONFLICT_RUNTIME" "$scratch/conflict_runtime_mut_l.rs"
  sed -i '/bar_present\.to_string(),/d; /^\s*close_micros,\s*$/d' "$scratch/conflict_runtime_mut_l.rs"
  assert_now_fails "MUT-L bar_present/close removed from cycle identity seed" "$(check_bar_presence_in_cycle_identity "$scratch/conflict_runtime_mut_l.rs")"

  # MUT-M: divergent same-ID replay silently accepted as idempotent
  # (PayloadCollision handling deleted).
  cp "$CONFLICT_DB_STORE" "$scratch/db_store_mut_m.rs"
  sed -i 's/PayloadCollision/AlreadyExistsRenamed/g; s/plans_match && candidates_match/true/' "$scratch/db_store_mut_m.rs"
  assert_now_fails "MUT-M divergent same-ID replay accepted as idempotent" "$(check_divergent_replay_rejected "$scratch/db_store_mut_m.rs")"

  # MUT-N: API malformed-evidence validation bypassed (route no longer calls
  # the shared validator).
  cp "$CONFLICT_ROUTE" "$scratch/route_mut_n.rs"
  sed -i 's/validate_plan_with_candidates/validateXplanXwithXcandidates/g' "$scratch/route_mut_n.rs"
  assert_now_fails "MUT-N API evidence validation bypassed" "$(check_api_evidence_validation_present "$scratch/route_mut_n.rs" "$CONFLICT_EVIDENCE_VALIDATION")"

  # MUT-O: status falls back from a malformed latest plan to an older one
  # (limit widened past 1, or the validation-failure branch removed).
  cp "$CONFLICT_ROUTE" "$scratch/route_mut_o.rs"
  sed -i 's/fetch_recent_runtime_strategy_conflict_plans(db, run\.run_id, 1)/fetch_recent_runtime_strategy_conflict_plans(db, run.run_id, 5)/' "$scratch/route_mut_o.rs"
  assert_now_fails "MUT-O status falls back past the single latest plan" "$(check_status_no_fallback_on_invalid_latest "$scratch/route_mut_o.rs")"

  # ---------------------------------------------------------------------
  # FINAL-IDENTITY-AND-READ-AUTHORITY-REPAIR-01 mutation-negative fixtures.
  # ---------------------------------------------------------------------

  # MUT-P: global first-assignment timeframe reintroduced into
  # compute_conflict_cycle_id's own signature.
  cp "$CONFLICT_RUNTIME" "$scratch/conflict_runtime_mut_p.rs"
  sed -i 's/pub fn compute_conflict_cycle_id(/pub fn compute_conflict_cycle_id(\n    timeframe: \&str,/' "$scratch/conflict_runtime_mut_p.rs"
  assert_now_fails "MUT-P global timeframe reintroduced into cycle identity signature" "$(check_no_global_timeframe_in_bundle6_identity "$scratch/conflict_runtime_mut_p.rs" "$LOOP_RUNNER")"

  # MUT-P2: loop_runner regresses to passing dispatch_timeframe back into
  # the gather_and_resolve call site.
  cp "$LOOP_RUNNER" "$scratch/loop_runner_mut_p2.rs"
  python - "$scratch/loop_runner_mut_p2.rs" <<'PY'
import sys
path = sys.argv[1]
text = open(path, encoding="utf-8").read()
anchor = "runtime_strategy_conflict::gather_and_resolve("
idx = text.find(anchor)
end = text.find(".await;", idx)
block = text[idx:end]
mutated = block.replace("all_decisions,", "all_decisions,\n                            dispatch_timeframe.clone(),", 1)
text = text[:idx] + mutated + text[end:]
open(path, "w", encoding="utf-8").write(text)
PY
  assert_now_fails "MUT-P2 dispatch_timeframe reintroduced into gather_and_resolve call" "$(check_no_global_timeframe_in_bundle6_identity "$CONFLICT_RUNTIME" "$scratch/loop_runner_mut_p2.rs")"

  # MUT-Q: DB replay comparison reverted to an ordinal-keyed map.
  cp "$CONFLICT_DB_STORE" "$scratch/db_store_mut_q.rs"
  sed -i 's/Vec<CandidateSnapshot>/BTreeMap<i32, CandidateSnapshot>/g; s/snapshots\.sort();/let _ = 0;/g' "$scratch/db_store_mut_q.rs"
  assert_now_fails "MUT-Q replay comparison reverted to ordinal-keyed map" "$(check_replay_ordinal_independent "$scratch/db_store_mut_q.rs")"

  # MUT-R: cycle-id recomputation removed from read-side validation.
  cp "$CONFLICT_EVIDENCE_VALIDATION" "$scratch/evidence_validation_mut_r.rs"
  sed -i 's/recomputed_cycle_id/xxx_removed_xxx/g' "$scratch/evidence_validation_mut_r.rs"
  assert_now_fails "MUT-R cycle-id recomputation removed from read validation" "$(check_read_validation_recomputes_cycle_id "$scratch/evidence_validation_mut_r.rs")"

  # MUT-S: recomputed-outcome comparison against persisted evidence removed
  # (read validation stops re-running the pure resolver in any meaningful
  # way).
  cp "$CONFLICT_EVIDENCE_VALIDATION" "$scratch/evidence_validation_mut_s.rs"
  sed -i 's/expected\.selected != c\.selected/false/g' "$scratch/evidence_validation_mut_s.rs"
  assert_now_fails "MUT-S recomputed-outcome comparison removed" "$(check_read_validation_reruns_pure_resolver "$scratch/evidence_validation_mut_s.rs")"

  # MUT-T: a selected candidate with no proposed_target_qty is silently
  # accepted again.
  cp "$CONFLICT_EVIDENCE_VALIDATION" "$scratch/evidence_validation_mut_t.rs"
  sed -i 's/c\.selected && c\.proposed_target_qty\.is_none()/false/g' "$scratch/evidence_validation_mut_t.rs"
  assert_now_fails "MUT-T selected-with-none-target check removed" "$(check_selected_target_none_rejected "$scratch/evidence_validation_mut_t.rs")"

  # MUT-U: configured_mode/mode incoherence is silently accepted again.
  cp "$CONFLICT_EVIDENCE_VALIDATION" "$scratch/evidence_validation_mut_u.rs"
  sed -i 's/cm != plan\.mode\.as_str()/false/g' "$scratch/evidence_validation_mut_u.rs"
  assert_now_fails "MUT-U configured_mode/mode coherence check removed" "$(check_mode_coherence_enforced "$scratch/evidence_validation_mut_u.rs")"

  # MUT-V: symbol_group_count reconciliation removed.
  cp "$CONFLICT_EVIDENCE_VALIDATION" "$scratch/evidence_validation_mut_v.rs"
  sed -i 's/canonical_symbol_groups/xxx_removed_xxx/g' "$scratch/evidence_validation_mut_v.rs"
  assert_now_fails "MUT-V symbol_group_count reconciliation removed" "$(check_symbol_group_count_recomputed "$scratch/evidence_validation_mut_v.rs")"

  # MUT-W: vanished-row count removed/merged back into malformed count.
  cp "$CONFLICT_ROUTE" "$scratch/route_mut_w.rs"
  sed -i 's/excluded_vanished_count/excluded_malformed_count_renamed/g' "$scratch/route_mut_w.rs"
  assert_now_fails "MUT-W vanished-row count removed/merged" "$(check_list_query_failure_distinct "$scratch/route_mut_w.rs")"

  # MUT-W2: a per-row query failure is folded back into an active/
  # malformed projection instead of failing the whole list closed.
  cp "$CONFLICT_ROUTE" "$scratch/route_mut_w2.rs"
  python - "$scratch/route_mut_w2.rs" <<'PY'
import sys
path = sys.argv[1]
text = open(path, encoding="utf-8").read()
anchor = "strategy_conflict_plans_row_detail_query_failed"
idx = text.find(anchor)
end = text.find("into_response();", idx) + len("into_response();")
block = text[idx:end]
mutated = block.replace("ConflictTruthState::QueryFailed", "ConflictTruthState::Active")
text = text[:idx] + mutated + text[end:]
open(path, "w", encoding="utf-8").write(text)
PY
  assert_now_fails "MUT-W2 per-row query failure folded back into active projection" "$(check_list_query_failure_distinct "$scratch/route_mut_w2.rs")"

  # MUT-X: deterministic latest ordering loses the plan_id DESC tie-break.
  cp "$CONFLICT_DB_STORE" "$scratch/db_store_mut_x.rs"
  sed -i 's/order by created_at_utc desc, plan_id desc/order by created_at_utc desc/' "$scratch/db_store_mut_x.rs"
  assert_now_fails "MUT-X plan_id DESC tie-break removed" "$(check_latest_ordering_tie_break "$scratch/db_store_mut_x.rs")"

  # MUT-Y: an API provenance field is removed/renamed.
  cp "$CONFLICT_ROUTE" "$scratch/route_mut_y.rs"
  sed -i 's/pub bar_timeframe:/pub bar_timeframe_removed:/' "$scratch/route_mut_y.rs"
  assert_now_fails "MUT-Y API provenance field removed" "$(check_api_provenance_fields_present "$scratch/route_mut_y.rs" "$CONFLICT_TYPES_TS")"

  # MUT-Z: blocker-bounds truncation helper removed.
  cp "$CONFLICT_EVIDENCE_VALIDATION" "$scratch/evidence_validation_mut_z.rs"
  sed -i 's/fn bound_blockers/fn bound_blockers_renamed/' "$scratch/evidence_validation_mut_z.rs"
  assert_now_fails "MUT-Z blocker-bounds truncation helper removed" "$(check_blocker_bounds_enforced "$scratch/evidence_validation_mut_z.rs")"

  # ---------------------------------------------------------------------
  # UTF8-AND-BOUNDED-READ-CLOSURE mutation-negative fixtures.
  # ---------------------------------------------------------------------

  # MUT-AA: UTF-8-safe truncation helper removed/renamed.
  cp "$CONFLICT_EVIDENCE_VALIDATION" "$scratch/evidence_validation_mut_aa.rs"
  sed -i 's/fn safe_truncate_utf8/fn safe_truncate_utf8_renamed/' "$scratch/evidence_validation_mut_aa.rs"
  assert_now_fails "MUT-AA UTF-8-safe truncation helper removed/renamed" "$(check_utf8_safe_truncation_helper "$scratch/evidence_validation_mut_aa.rs")"

  # MUT-BB: safe truncation reverted to direct String::truncate(MAX_BLOCKER_LEN).
  cp "$CONFLICT_EVIDENCE_VALIDATION" "$scratch/evidence_validation_mut_bb.rs"
  sed -i 's/\*b = safe_truncate_utf8(b, MAX_BLOCKER_LEN);/b.truncate(MAX_BLOCKER_LEN);/' "$scratch/evidence_validation_mut_bb.rs"
  assert_now_fails "MUT-BB safe truncation reverted to direct String::truncate" "$(check_no_raw_truncate_in_blocker_bounding "$scratch/evidence_validation_mut_bb.rs")"

  # MUT-CC: suffix length no longer reserved out of max_len (suffix could be
  # appended outside the configured maximum).
  cp "$CONFLICT_EVIDENCE_VALIDATION" "$scratch/evidence_validation_mut_cc.rs"
  sed -i 's/max_len - suffix\.len()/max_len/' "$scratch/evidence_validation_mut_cc.rs"
  assert_now_fails "MUT-CC suffix length no longer reserved from max_len" "$(check_suffix_reserved_within_max "$scratch/evidence_validation_mut_cc.rs")"

  # MUT-DD: bounded read seam's SQL LIMIT removed from the candidate query.
  cp "$CONFLICT_DB_STORE" "$scratch/db_store_mut_dd.rs"
  python - "$scratch/db_store_mut_dd.rs" <<'PY'
import sys
path = sys.argv[1]
text = open(path, encoding="utf-8").read()
anchor = "pub async fn fetch_runtime_strategy_conflict_plan_for_read("
idx = text.find(anchor)
end = text.find("\n}\n", idx)
block = text[idx:end]
lines = [l for l in block.split("\n") if "limit $2" not in l]
mutated = "\n".join(lines)
text = text[:idx] + mutated + text[end:]
open(path, "w", encoding="utf-8").write(text)
PY
  assert_now_fails "MUT-DD bounded SQL LIMIT removed from candidate query" "$(check_sql_bounded_limit "$scratch/db_store_mut_dd.rs")"

  # MUT-EE: routes revert to the unbounded candidate fetch.
  cp "$CONFLICT_ROUTE" "$scratch/route_mut_ee.rs"
  sed -i 's/fetch_runtime_strategy_conflict_plan_for_read/fetch_runtime_strategy_conflict_plan/g' "$scratch/route_mut_ee.rs"
  assert_now_fails "MUT-EE routes revert to the unbounded candidate fetch" "$(check_routes_use_bounded_seam "$scratch/route_mut_ee.rs")"

  # MUT-FF: the unbounded full-payload replay fetch is accidentally limited.
  cp "$CONFLICT_DB_STORE" "$scratch/db_store_mut_ff.rs"
  python - "$scratch/db_store_mut_ff.rs" <<'PY'
import sys
path = sys.argv[1]
text = open(path, encoding="utf-8").read()
anchor = "pub async fn fetch_runtime_strategy_conflict_plan("
idx = text.find(anchor)
end = text.find("\n}\n", idx)
block = text[idx:end]
mutated = block.replace(
    "from sys_runtime_strategy_conflict_candidates where plan_id = $1 order by ordinal\",",
    "from sys_runtime_strategy_conflict_candidates where plan_id = $1 order by ordinal limit 65\",",
)
text = text[:idx] + mutated + text[end:]
open(path, "w", encoding="utf-8").write(text)
PY
  assert_now_fails "MUT-FF unbounded full-payload replay fetch accidentally limited" "$(check_full_replay_path_unbounded "$scratch/db_store_mut_ff.rs")"

  # MUT-GG: status stops mapping an over-limit latest plan to invalid_evidence.
  cp "$CONFLICT_ROUTE" "$scratch/route_mut_gg.rs"
  python - "$scratch/route_mut_gg.rs" <<'PY'
import sys
path = sys.argv[1]
text = open(path, encoding="utf-8").read()
anchor = "the durable plan exceeds the shared bounded-read"
idx = text.find(anchor)
end = text.find("}", idx) + 1
block = text[idx:end]
mutated = block.replace("ConflictTruthState::InvalidEvidence", "ConflictTruthState::Active")
text = text[:idx] + mutated + text[end:]
open(path, "w", encoding="utf-8").write(text)
PY
  assert_now_fails "MUT-GG status over-limit plan no longer maps to invalid_evidence" "$(check_status_over_limit_is_invalid_evidence "$scratch/route_mut_gg.rs")"

  # MUT-HH: detail projects candidate data for an over-limit plan.
  cp "$CONFLICT_ROUTE" "$scratch/route_mut_hh.rs"
  python - "$scratch/route_mut_hh.rs" <<'PY'
import sys
path = sys.argv[1]
text = open(path, encoding="utf-8").read()
anchor = "over-limit plan -- invalid_evidence, no partial"
idx = text.find(anchor)
end = text.find("into_response()", idx) + len("into_response()")
block = text[idx:end]
mutated = block.replace("candidates: vec![],", "candidates: some_partial_candidates(),", 1)
text = text[:idx] + mutated + text[end:]
open(path, "w", encoding="utf-8").write(text)
PY
  assert_now_fails "MUT-HH detail route projects candidate data for an over-limit plan" "$(check_detail_over_limit_no_partial_candidates "$scratch/route_mut_hh.rs")"

  # MUT-II: list stops counting a successfully-read over-limit plan as malformed.
  cp "$CONFLICT_ROUTE" "$scratch/route_mut_ii.rs"
  python - "$scratch/route_mut_ii.rs" <<'PY'
import sys
path = sys.argv[1]
text = open(path, encoding="utf-8").read()
anchor = "a successfully-read over-limit plan is"
idx = text.find(anchor)
end = text.find("}", idx) + 1
block = text[idx:end]
mutated = block.replace("excluded_malformed_count += 1;", "// dropped")
text = text[:idx] + mutated + text[end:]
open(path, "w", encoding="utf-8").write(text)
PY
  assert_now_fails "MUT-II list route stops counting an over-limit plan as malformed" "$(check_list_over_limit_counts_as_malformed "$scratch/route_mut_ii.rs")"

  # MUT-JJ: daemon validator bound reverts to a hardcoded literal, no longer
  # referencing the shared mqk-db constant (drift risk reintroduced).
  cp "$CONFLICT_EVIDENCE_VALIDATION" "$scratch/evidence_validation_mut_jj.rs"
  sed -i 's/pub const MAX_CANDIDATES_PER_PLAN: usize = mqk_db::RUNTIME_STRATEGY_CONFLICT_CANDIDATE_READ_BOUND;/pub const MAX_CANDIDATES_PER_PLAN: usize = 64;/' "$scratch/evidence_validation_mut_jj.rs"
  assert_now_fails "MUT-JJ validator bound reverts to a hardcoded literal (drift risk)" "$(check_shared_bound_no_drift "$scratch/evidence_validation_mut_jj.rs" "$CONFLICT_DB_STORE")"

  if [[ "$failures" -gt 0 ]]; then
    echo "[msc-guard-selftest] FAIL: $failures mutation(s) were not caught" >&2
    exit 1
  fi
  echo "[msc-guard-selftest] ALL MUTATION-NEGATIVE FIXTURES CAUGHT"
}

if [[ "${1:-}" == "--self-test" ]]; then
  run_self_test
else
  run_real_checks
fi
