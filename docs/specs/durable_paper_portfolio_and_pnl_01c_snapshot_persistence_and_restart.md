# DURABLE-PAPER-PORTFOLIO-AND-PNL-01C — Snapshot Persistence and Restart

Patch ID: `DURABLE-PAPER-PORTFOLIO-AND-PNL-01C-SNAPSHOT-PERSISTENCE-AND-RESTART`
Wires B4-B's durable store into the one canonical acceptance seam identified
by B4-A. No route-side persistence anywhere in this patch.

## Canonical acceptance seam

`state::snapshot::accept_external_broker_snapshot(state, snapshot, run_id,
operation_id)` is now the sole function that writes `AppState.broker_snapshot`
for the `External` (real Alpaca) source. It:

1. Writes the in-memory cache exactly as before (`*state.broker_snapshot
   .write().await = Some(snapshot.clone())`) — every existing reader keeps
   working unchanged.
2. Additively calls `persist_external_broker_snapshot_best_effort`, which
   derives a deterministic `snapshot_id` (UUIDv5, per the B4-A contract's
   seed convention) and calls B4-B's
   `insert_or_confirm_paper_portfolio_snapshot`.

Three call sites were refactored to go through it instead of writing the
`RwLock` directly:

- `state/orchestrator_build.rs`'s run-start cold-fetch (the primary seam —
  `run_id` is directly in scope, so this call is a plain `.await`).
- `state/loop_runner.rs`'s periodic ~60s External-source refresh (`state_arc`
  is directly in scope, same plain `.await`).
- `state/orchestrator_build.rs`'s terminal-fill-expiry-refresher. This one
  closure is **sync** (invoked synchronously from inside the orchestrator
  tick, not `.await`-able directly), so only the in-memory write stays
  inline; the durable-persist half is captured as owned, `'static`-safe
  pieces (`Option<PgPool>`, `DeploymentMode`, `Option<BrokerKind>` — all
  `Clone`/`Copy`) and spawned via `tokio::spawn`, matching the existing
  fire-and-forget pattern this same file already uses for the Discord alert
  sink. This is still best-effort and additive; the existing fail-closed
  halt/reconcile behavior driven by this closure's return value is
  completely unchanged.

The `Synthetic` branch (in-memory synthesis for the local paper-fill engine)
and the dev-only `/routes/trading.rs` injection/clear routes never call this
seam and never produce durable authoritative truth — proven by
`synthesized_source_never_masquerades_as_authoritative`.

## Source authority enforcement

`persist_external_broker_snapshot_best_effort` is fail-closed on its own,
independent of caller discipline: it only persists when `deployment_mode ==
Paper` **and** `broker_kind == Some(Alpaca)`; any other combination is a
silent no-op (the in-memory write above it still happens, since acceptance
and persistence are independent halves of the same function).

## Failure handling

Every persistence failure mode (`no_db_pool_configured`, unparseable
account/position decimal fields, a DB error, a typed `Conflict` from B4-B)
is logged via `tracing::warn!` and swallowed — never propagated, never
panics, never blocks or reverts the in-memory acceptance that already
happened. Successful persistence is not itself a license to trade: it
touches no reconcile, risk, or order-submission code path.

## Test seam

`AppState::accept_external_broker_snapshot_for_test` (a thin, one-line
pass-through to the real `accept_external_broker_snapshot`) lets integration
tests exercise the actual production seam with an injected `BrokerSnapshot`,
without needing a live orchestrator, run, or broker fetch — following the
same "Test seam for ..." convention already used elsewhere in `state.rs`
(e.g. `dispatch_native_strategy_for_symbol_with_loaded_bars_for_test`).

## Tests (`scenario_durable_paper_portfolio_snapshot_persistence_01.rs`, 9 tests)

Real-source (Paper+Alpaca) accepted snapshot persists; synthesized
(Paper-broker) source never masquerades as authoritative even though the
in-memory cache still updates; missing DB is bounded (no panic, in-memory
acceptance still succeeds); an unreachable DB (a self-contained lazy pool
against an unroutable address, never touching the shared test pool) is
bounded the same way; exact replay is idempotent; restart reconstruction (a
dropped `AppState` + dropped pool, replaced by a brand-new pair, reads back
the durable row); position ordering round-trips; multiple distinct
snapshots form an independently-readable durable history; zero
`oms_outbox`/`oms_inbox` writes and — structurally, not just by
observation — zero broker/provider calls (this seam's signature takes an
already-constructed `BrokerSnapshot`; no provider or broker handle exists
anywhere in the test file).

All 9 pass, stable across five consecutive default-parallel runs. One
correctness finding surfaced and fixed during this patch: the first draft
used `fetch_latest_paper_portfolio_snapshot` (global across all runs
sharing deployment_mode+source) to locate a just-persisted row, which is
inherently racy under `cargo test`'s default parallelism once multiple
tests in this file persist concurrently — a different test's later
`captured_at_utc` can legitimately "win" the global-latest query at the
exact moment another test checks it. Fixed by looking rows up via their
exact deterministic `snapshot_id` (mirroring the production seed formula)
instead, eliminating the race entirely — the same class of fix B4-0 applied
to the completed-bar-driver scenario.

## Verification

- `cargo check -p mqk-daemon`: clean.
- `cargo clippy -p mqk-daemon --lib -- -D warnings`: clean.
- `cargo clippy -p mqk-daemon --test scenario_durable_paper_portfolio_snapshot_persistence_01 -- -D warnings`: clean.
  (A pre-existing, unrelated `await_holding_lock` clippy debt in
  `state/session_controller.rs`'s own test module surfaces only under a
  blanket `--tests` compile of the whole crate; it predates this patch,
  touches no file this patch changed, and is out of scope here.)
- `cargo fmt --check` on the touched files: no diff.
- `cargo test -p mqk-daemon --test scenario_durable_paper_portfolio_snapshot_persistence_01`: 9/9 pass, five consecutive runs.
- `cargo test -p mqk-daemon --test scenario_autonomous_completed_bar_driver_01`: still 56/56 (regression check — this patch touches `orchestrator_build.rs`/`loop_runner.rs`, which that scenario exercises indirectly via the same module tree).
- `cargo test -p mqk-daemon --test scenario_paper_daily_pnl_baseline_01`: still 11/11 (existing daily-P&L baseline untouched).
- `cargo test -p mqk-daemon --test scenario_paper_order_lifecycle_visibility_01`: still 5/5 passing, 8 ignored (unchanged).

## FINAL-RUN-SCOPING-ACCOUNTING-AND-CLOSURE-REPAIR addendum

Patch ID: `DURABLE-PAPER-PORTFOLIO-AND-PNL-01-FINAL-RUN-SCOPING-ACCOUNTING-AND-CLOSURE-REPAIR`

`persist_external_broker_snapshot_best_effort` now returns a typed
`ExternalSnapshotPersistOutcome` (`Confirmed { snapshot_id, newly_inserted }`
/ `SkippedUnsupported` / `Unavailable` / `Conflict` / `DatabaseFailure` /
`InvalidSnapshot`) instead of `()`. `accept_external_broker_snapshot` calls
the accounting refresh (B4-D) only when the persist outcome is `Confirmed`,
passing its `snapshot_id` through as the accounting row's provenance —
closing the gap where a best-effort persistence failure or conflict could
previously still trigger an accounting refresh with no confirmed snapshot
backing it. The same gating was applied to the one call site that could not
await the persist call directly (`orchestrator_build.rs`'s
`terminal_fill_expiry_refresher`, a spawned `tokio::spawn` task) — the prior
version of that closure only spawned the persist half and never chained the
accounting refresh at all.

Verification: `cargo test -p mqk-daemon --test
scenario_durable_paper_portfolio_snapshot_persistence_01 -- --include-ignored
--test-threads=1`: 9/9 pass, unchanged. `cargo clippy -p mqk-db -p mqk-daemon
--tests -- -D warnings`: clean on every file this repair touched (one
pre-existing `large_enum_variant` lint on the new `RunResolution` type,
introduced and then fixed in the same patch by boxing the `Found` variant).
