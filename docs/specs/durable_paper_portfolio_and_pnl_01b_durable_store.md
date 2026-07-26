# DURABLE-PAPER-PORTFOLIO-AND-PNL-01B — Durable Schema and DB Store

Patch ID: `DURABLE-PAPER-PORTFOLIO-AND-PNL-01B-DURABLE-STORE`
Implements the B4-A contract's schema decision. No daemon wiring — pure DB
layer, zero callers outside this patch's own tests.

## Migration

`0053_paper_portfolio_durable_store.sql` (next sequential ID after `0052`,
discovered fresh from `manifest.json`, registered there). Additive only, no
existing table modified. No `DEFAULT now()` / `DEFAULT gen_random_uuid()` —
every timestamp and identity is caller-supplied.

Three tables:

- `sys_paper_portfolio_snapshots` (PK `snapshot_id`) — one row per accepted
  authoritative account/position snapshot. `source` is constrained to
  `('external_alpaca', 'synthetic_diagnostic')` — the source-authority
  distinction the B4-A contract requires is enforced at the schema level,
  not just by convention. Nullable `run_id` (FK to `runs`) and
  `operation_id`.
- `sys_paper_portfolio_snapshot_positions` (PK `(snapshot_id, symbol)`) —
  child rows, FK to the snapshot.
- `sys_paper_portfolio_accounting_state` (PK `run_id`, FK to `runs`) — one
  durable row per run caching a replay of that run's `oms_inbox` applied
  fills through `mqk-portfolio`'s FIFO engine. `accounting_epoch`
  constrained to `('complete', 'incomplete')`. `last_applied_inbox_id`
  constrained `>= 0`. **No fill content is stored here** — this is not a
  second fill ledger (see B4-A §1.2/§1.6); it is a persisted cache of a
  computation, keyed by the watermark that makes re-application idempotent.

## DB helpers (`mqk-db/src/paper_portfolio.rs`)

Snapshots:
- `insert_or_confirm_paper_portfolio_snapshot` — atomic (single transaction)
  insert of the snapshot row plus all position rows, or idempotent
  confirmation. Three-way outcome: `Inserted`, `AlreadyExists` (identical
  content, order-independent position comparison), `Conflict` (same
  `snapshot_id`, different content — the existing row is never
  overwritten, proven by `conflicting_replay_is_rejected`).
- `fetch_paper_portfolio_snapshot_by_id`
- `fetch_latest_paper_portfolio_snapshot(pool, deployment_mode, source)`
- `fetch_recent_paper_portfolio_snapshots(pool, deployment_mode, source, limit)`

Accounting state:
- `upsert_paper_portfolio_accounting_state` — four-way outcome: `Inserted`
  (first write for a run), `Updated` (watermark strictly advances),
  `AlreadyCurrent` (watermark exactly matches — idempotent no-op, returns
  the *stored* row, not the caller's freshly-recomputed input, since a
  deterministic replay of the same inbox rows always yields the same result
  by construction), `Rejected` (caller supplied a watermark *lower* than
  what's stored — fail closed, the row is never regressed; this is the
  "conflicting replay" case for this table).
- `fetch_paper_portfolio_accounting_state`

Neither insert path accepts `DEFAULT now()`/random-UUID authority from the
database — every timestamp and identity comes from the caller, per B4-A's
UUIDv5 convention for `snapshot_id`, and per `runs.run_id` (an existing
stable identity) for the accounting-state key.

## Why no separate "accounting event" table

The B4-A contract (§1.2, §1.6) established that `oms_inbox` already provides
a complete, ordered (`inbox_id asc`), deduped, replayable fill stream via
`inbox_load_all_applied_for_run`. Introducing a second table that stores
fill symbol/side/qty/price again would create a second source of truth that
could drift from the first, in violation of `db_rules.md`'s "no write paths
outside the established outbox/inbox/run seams without proof." The
`last_applied_inbox_id` watermark plus the computed cash/realized-P&L/fees
summary is the smallest durable surface that makes replay idempotent
without duplicating fill content — exactly what the master mission's own
"do not force this shape when the audit proves a smaller existing durable
source already suffices" allows for.

The actual replay computation — reading `oms_inbox` and driving
`mqk_portfolio::apply_fill`/`recompute_from_ledger` — is deliberately **not**
implemented in this patch. That logic already lives in mqk-daemon
(`broker_event_to_portfolio_fill`, `state/snapshot.rs`) and belongs in B4-D
("durable fill accounting and P&L truth"), which will call
`upsert_paper_portfolio_accounting_state` with its computed result. B4-B is
persistence only.

## Tests (`mqk-db/tests/scenario_paper_portfolio_store_01.rs`, 14 tests)

Empty store; first snapshot insert with a non-flat position; exact
idempotent replay (zero duplicate rows, including position children);
conflicting replay rejected (original row provably unchanged); multiple
snapshots ordered deterministically (both `fetch_recent_...` and
`fetch_latest_...`); flat account (zero position rows); synthetic-source
snapshot never appears when querying for `external_alpaca`; invalid
`source` rejected pre-write; first accounting-state write inserts;
watermark idempotency/advance/rejection in one sequential proof; incomplete
accounting epoch round-trips its reason string; invalid `accounting_epoch`
rejected pre-write; restart reconstruction (a dropped pool + fresh pool
reads back exactly what was written, for both tables); zero writes to
`oms_outbox`/`oms_inbox` from any of this module's functions.

All 14 pass against the isolated port-5434 test DB, `--test-threads=1` and
default parallelism alike (no shared global state — every test uses its own
deterministic `run_id`/`snapshot_id` derived from its own test name).

## Verification

- `cargo test -p mqk-db --test scenario_paper_portfolio_store_01 -- --include-ignored`: 14/14 pass.
- `cargo clippy -p mqk-db --tests -- -D warnings`: clean.
- `cargo fmt --check` on the two new files: no diff.
- Zero provider/broker/network calls anywhere in this patch.
- No production wiring — `mqk-daemon` is untouched by this patch.

## FINAL-RUN-SCOPING-ACCOUNTING-AND-CLOSURE-REPAIR addendum

Patch ID: `DURABLE-PAPER-PORTFOLIO-AND-PNL-01-FINAL-RUN-SCOPING-ACCOUNTING-AND-CLOSURE-REPAIR`

Two additive changes on top of this phase's original schema/store, closing
gaps found in the Bundle 4 final closure review:

- `fetch_latest_paper_portfolio_snapshot_for_run(pool, deployment_mode,
  source, run_id)` — the run-scoped counterpart of
  `fetch_latest_paper_portfolio_snapshot`. `WHERE ... AND run_id = $3`
  means a snapshot whose `run_id` is `NULL` can never satisfy a run-scoped
  query, and a different run's newer snapshot can never be returned in its
  place. The global function is unchanged and still used by the
  `durable-snapshots` history route, which is explicitly not run-scoped.
- Migration `0054_paper_portfolio_accounting_snapshot_provenance.sql`
  (additive, does not modify `0053`) adds nullable
  `source_snapshot_id uuid REFERENCES sys_paper_portfolio_snapshots(snapshot_id)`
  to `sys_paper_portfolio_accounting_state`, so a durable accounting row
  can be traced back to the exact confirmed snapshot whose positions
  produced its `accounting_epoch`. `UpsertPaperPortfolioAccountingStateArgs`
  now requires a `source_snapshot_id`; `UpsertPaperPortfolioAccountingStateOutcome`
  gained `UpdatedForSnapshot` (same watermark, same fill-derived values, new
  snapshot — only snapshot-dependent fields advance) and `Conflict` (same
  watermark, differing fill-derived values — fail closed, zero writes) to
  replace the prior "same watermark is always `AlreadyCurrent`, no content
  comparison" behavior, which could leave a stale `accounting_epoch`/reason
  in place after a new broker snapshot arrived with no new inbox row.

New tests in `scenario_paper_portfolio_store_01.rs`:
`run_scoped_snapshot_never_crosses_runs` (cross-run isolation + null-run_id
exclusion + no-snapshot-for-run resolves `None`) and an extended
`accounting_state_watermark_idempotency_and_ordering` proving the full
same-watermark matrix (`AlreadyCurrent` / `Conflict` / `UpdatedForSnapshot`
/ `Updated` / `Rejected`). All 15 tests in the file pass against the
port-5434 test DB, `--test-threads=1`.
