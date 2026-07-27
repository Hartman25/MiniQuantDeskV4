#!/usr/bin/env bash
# RUNTIME-OPPORTUNITY-ALLOCATION-01 final closure guard.
#
# Static, grep-based proofs that do not require a live daemon or DB
# connection. These complement (never replace) the real cargo/pytest/npm
# test suites — see docs/specs/runtime_opportunity_allocation_01f_bundle_5_closure.md
# for the full evidence matrix.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

fail() {
  echo "[roa-guard] FAIL: $*" >&2
  exit 1
}
ok() {
  echo "[roa-guard] OK: $*"
}

echo "[roa-guard] repo root: $REPO_ROOT"

# ---------------------------------------------------------------------------
# 1. mqk-portfolio (allocator + cycle model) stays a zero-dependency, pure
#    crate: no DB/network/broker crate, no random-number crate.
# ---------------------------------------------------------------------------
if grep -qE '^\s*(sqlx|reqwest|tokio|rand|mqk-db|mqk-execution|mqk-broker)' \
    core-rs/crates/mqk-portfolio/Cargo.toml; then
  fail "mqk-portfolio must remain a zero-IO, zero-dependency crate"
fi
ok "mqk-portfolio has no DB/network/broker/random dependency"

# ---------------------------------------------------------------------------
# 2. The pure cycle model and allocator never call the outbox or a broker
#    adapter directly.
# ---------------------------------------------------------------------------
if grep -qE 'outbox_enqueue|submit_internal_strategy_decision|AlpacaBroker|reqwest::' \
    core-rs/crates/mqk-portfolio/src/allocator.rs \
    core-rs/crates/mqk-portfolio/src/cycle.rs; then
  fail "allocator/cycle module must never call outbox/broker directly"
fi
ok "allocator and cycle model never call outbox/broker directly"

# ---------------------------------------------------------------------------
# 3. None of Bundle 5's own new modules call outbox_enqueue or a broker
#    adapter directly -- every allocation-adjusted decision must still flow
#    through the existing, unchanged decision.rs::submit_internal_strategy_decision
#    seam. (Pre-existing call sites in control_plane.rs/execution.rs/
#    strategy.rs/loop_runner.rs's own dispatch belong to Bundle 4 and earlier
#    signal paths and are out of scope for this check.)
# ---------------------------------------------------------------------------
bundle5_new_daemon_files=(
  core-rs/crates/mqk-daemon/src/runtime_opportunity_allocation.rs
  core-rs/crates/mqk-daemon/src/runtime_opportunity_artifact.rs
  core-rs/crates/mqk-daemon/src/runtime_opportunity_mode.rs
  core-rs/crates/mqk-daemon/src/routes/portfolio_allocation.rs
)
if grep -lE 'outbox_enqueue\(|AlpacaBroker|reqwest::' "${bundle5_new_daemon_files[@]}" 2>/dev/null; then
  fail "Bundle 5's new modules must never call outbox/broker directly"
fi
ok "Bundle 5's new modules never call outbox/broker directly"

# ---------------------------------------------------------------------------
# 4. Default mode is off.
# ---------------------------------------------------------------------------
grep -A3 'let Some(value) = trimmed else {' \
  core-rs/crates/mqk-daemon/src/runtime_opportunity_mode.rs | \
  grep -q 'RuntimeOpportunityAllocationMode::Off' || \
  fail "absent/blank env var must default to Off"
ok "default runtime mode is off"

# ---------------------------------------------------------------------------
# 5. Live hard lock: effective_mode must be able to force Off regardless of
#    configuration outside paper+Alpaca.
# ---------------------------------------------------------------------------
grep -q 'RuntimeOpportunityAllocationMode::Off' \
  core-rs/crates/mqk-daemon/src/runtime_opportunity_mode.rs || \
  fail "effective_mode must be able to force Off"
grep -q 'is_paper_alpaca' core-rs/crates/mqk-daemon/src/runtime_opportunity_mode.rs || \
  fail "live-lock predicate (paper+Alpaca) must exist"
ok "live hard-lock predicate present"

# ---------------------------------------------------------------------------
# 6. approved_for_live is hardcoded false everywhere it appears in the new
#    API/GUI/artifact/DB surfaces -- never a variable, never caller-supplied.
# ---------------------------------------------------------------------------
if grep -rn 'approved_for_live' \
    core-rs/crates/mqk-daemon/src/routes/portfolio_allocation.rs \
    core-rs/mqk-gui/src/features/system/types/allocation.ts \
  | grep -qvE 'false|approved_for_live: bool'; then
  fail "approved_for_live must always be a hardcoded false in Bundle 5 surfaces"
fi
ok "approved_for_live is hardcoded false in API/GUI surfaces"

# ---------------------------------------------------------------------------
# 7. GET-only API: the three allocation routes must only be registered with
#    axum's get().
# ---------------------------------------------------------------------------
if grep -A2 '/api/v1/portfolio/allocation/status"' core-rs/crates/mqk-daemon/src/routes.rs \
  | grep -qE 'post\(|put\(|delete\(|patch\('; then
  fail "allocation routes must be GET-only"
fi
ok "allocation routes registered GET-only"

# ---------------------------------------------------------------------------
# 8. GUI panel has no mutation control (no fetch/axios POST/PUT/DELETE, no
#    <button> wired to a mode-change or write action) in the panel component.
# ---------------------------------------------------------------------------
if grep -qE 'method:\s*["'"'"'](POST|PUT|DELETE|PATCH)' \
    core-rs/mqk-gui/src/features/system/RuntimeOpportunityAllocationPanel.tsx 2>/dev/null; then
  fail "allocation GUI panel must not perform any mutation"
fi
if grep -q '<button' core-rs/mqk-gui/src/features/system/RuntimeOpportunityAllocationPanel.tsx; then
  fail "allocation GUI panel must not contain interactive mutation controls"
fi
ok "allocation GUI panel is read-only (no mutation calls, no buttons)"

# ---------------------------------------------------------------------------
# 9. Durable evidence tables are additive and distinct from portfolio/P&L
#    tables -- migration 0055 must not ALTER any existing table.
# ---------------------------------------------------------------------------
if grep -qiE '^\s*ALTER TABLE' \
    core-rs/crates/mqk-db/migrations/0055_runtime_opportunity_allocation_plans.sql; then
  fail "migration 0055 must be additive only (no ALTER TABLE on existing tables)"
fi
if grep -v '^\s*--' \
    core-rs/crates/mqk-db/migrations/0055_runtime_opportunity_allocation_plans.sql \
  | grep -qiE 'DEFAULT\s+now\(\)|DEFAULT\s+gen_random_uuid\(\)'; then
  fail "migration 0055 must not use DEFAULT now()/gen_random_uuid()"
fi
ok "migration 0055 is additive, no implicit time/uuid defaults"

# ---------------------------------------------------------------------------
# 10. No AI/ML framework or provider reference anywhere in Bundle 5's new
#     files.
# ---------------------------------------------------------------------------
bundle5_files="$(git diff --name-only main...HEAD -- \
  core-rs/crates/mqk-portfolio \
  core-rs/crates/mqk-daemon \
  core-rs/crates/mqk-db \
  core-rs/mqk-gui \
  research-py/src/mqk_research/scanner/runtime_opportunity_artifact.py \
  research-py/tests/test_runtime_opportunity_artifact.py \
  2>/dev/null || true)"
if [[ -n "$bundle5_files" ]]; then
  ai_hits="$(echo "$bundle5_files" | xargs -I{} grep -lIE \
    'openai|anthropic\.com|torch|tensorflow|onnxruntime|langchain' {} 2>/dev/null || true)"
  if [[ -n "$ai_hits" ]]; then
    echo "$ai_hits" >&2
    fail "no AI/ML framework or provider reference is permitted in Bundle 5 files"
  fi
fi
ok "no AI/ML reference found in Bundle 5 files"

# ---------------------------------------------------------------------------
# 11. Bundle 6 (multi-strategy conflict policy) is not started -- no file
#     path referencing it exists yet.
# ---------------------------------------------------------------------------
if git ls-files | grep -qiE 'multi.strategy.conflict|bundle.?6'; then
  fail "Bundle 6 (multi-strategy conflict policy) must not be started"
fi
ok "Bundle 6 not started"

echo "[roa-guard] ALL CHECKS PASSED"
