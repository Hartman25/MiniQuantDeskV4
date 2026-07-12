# Paper Daily P&L Baseline — 01D Capture Boundary Decision

Patch group: `PAPER-DAILY-PNL-BASELINE-01-COMBINED`, Phase D.

**Decision: baseline capture is deferred to a future patch.** This phase
adds no code — it records and confirms the boundary decision Phase A
already locked (`paper_daily_pnl_baseline_01a_current_truth_reconcile.md`
§8), now that Phases B and C are built and proven.

## 1. What exists after Phases A-C

- `sys_account_equity_baseline` table (migration `0045`), keyed by
  `trading_date`, with provenance columns (`captured_at_utc`,
  `captured_by`, `broker_snapshot_source`, `audit_event_id`).
- `upsert_account_equity_baseline` / `fetch_account_equity_baseline_for_date`
  DB helpers (`core-rs/crates/mqk-db/src/account_equity_baseline.rs`),
  proven by DB-backed tests against the real local test/paper Postgres.
- `GET /api/v1/portfolio/summary` now reports a real `daily_pnl` whenever a
  baseline row exists for the required prior trading day, and an honest
  `daily_pnl_truth_state` (`"active"` / `"baseline_unavailable"` /
  `"stale_baseline"` / `"no_snapshot"` / `"db_unavailable"`) in every other
  case — proven by 11 DB-backed route tests plus a pure weekend-skip unit
  test.

## 2. What does not exist after Phases A-C

**No mechanism writes a baseline row in production.** The only writer
proven in this patch group is the test suite itself, calling
`upsert_account_equity_baseline` directly. There is:

- no automatic capture triggered by daemon ticks, market-close detection,
  or session-boundary events;
- no CLI command (`mqk-cli` has no `capture-equity-baseline` subcommand);
- no HTTP write route.

This means `daily_pnl` will remain `"baseline_unavailable"` in a real
running daemon until a future patch adds a real capture mechanism (or an
operator/test manually seeds a row, which is explicitly not a production
capture path).

## 3. Why capture is not built in this patch

This restates and confirms Phase A's locked decision
(`paper_daily_pnl_baseline_01a_current_truth_reconcile.md` §8):

1. **The design doc's own conclusion.** `paper_daily_pnl_baseline_design_only_01.md`
   §10 already identified the capture mechanism — a market-session-timed
   trigger, provenance capture, and idempotent write path — as
   independently non-trivial, testable surface deserving its own patch
   group, separate from the schema/read-side work in this bundle.
2. **`mqk-cli` scope.** `core-rs/crates/mqk-cli/src/main.rs` uses a `clap`
   `Subcommand` enum; adding a `capture-equity-baseline` command correctly
   would require wiring a DB pool, a broker-snapshot source, and
   market-calendar trading-day validation into the CLI binary — real new
   scope, not a small addition, and untested by this patch group's proof
   suite.
3. **CLAUDE.md's one-patch-per-turn / minimal-scope discipline.** This
   patch group's honest, provable deliverable is "a real
   previous-session-close baseline schema, with correct fail-closed
   read-side visibility once a row exists." Bundling an unproven,
   untested write path onto the very invariant (fail-closed truth) this
   patch group exists to strengthen would be the kind of scope-widening
   CLAUDE.md's patch discipline exists to prevent.

## 4. Future patch

```text
PAPER-DAILY-PNL-BASELINE-CAPTURE-01-COMBINED
```

Recommended scope for that patch: a market-calendar-gated capture
mechanism (CLI-first, per the design doc's §10/Phase D guidance in the
original prompt) that:

- requires an active broker snapshot and a DB pool, failing closed on
  either's absence;
- validates the target date is an actual NYSE trading day via
  `NyseWeekdaysProvider`/`ExchangeSourcedCalendarProvider` before writing;
- is idempotent by `trading_date` (already guaranteed by the Phase B
  upsert helper — the future patch only needs to call it correctly);
- records deterministic provenance (`captured_by`, `broker_snapshot_source`,
  a caller-supplied `audit_event_id`) per `.claude/rules/audit_repo_truth_rules.md`.

## 5. Non-goals reaffirmed

- No fabricated, guessed, or inferred baseline in this phase or any prior
  phase of this patch group.
- No historical backfill.
- No provider, broker, or network call in this phase (docs-only; no code
  touched).
- No order submission, no live routing, no execution arming.
- No forced paper orders, no manually submitted paper orders.
- No strategy, gate, or config threshold changes.
