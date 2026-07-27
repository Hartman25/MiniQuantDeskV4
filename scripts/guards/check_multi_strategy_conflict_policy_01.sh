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
