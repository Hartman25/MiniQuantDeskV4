# RUNTIME-OPPORTUNITY-ALLOCATION-01 — Phase B/C: Artifact and Allocator

Status: implemented and tested. This document freezes what was built and
where, for future readers — it is not a stale ledger snapshot and carries no
patch-status table (see `.claude/rules/audit_repo_truth_rules.md`).

## Artifact: `runtime-opportunity-set-v1`

Producer (Python, read-only from the daemon's perspective):
[research-py/src/mqk_research/scanner/runtime_opportunity_artifact.py](../../research-py/src/mqk_research/scanner/runtime_opportunity_artifact.py)

Consumer (Rust, daemon-side loader/validator, mirrors
`watchlist_intake.rs`'s pattern):
[core-rs/crates/mqk-daemon/src/runtime_opportunity_artifact.rs](../../core-rs/crates/mqk-daemon/src/runtime_opportunity_artifact.rs)

### Schema

Top level: `schema_version` ("runtime-opportunity-set-v1"), `artifact_id`,
`generated_at_utc`, `expires_at_utc`, `market_date`, `mode` ("paper"),
`approved_for_autonomous_paper` (must be `true`), `approved_for_live` (always
`false`), `source_watchlist_hash`, `source_candidate_artifact_hashes`,
`scanner_lineage` (`scanner_id`, `code_version`, `scoring_config_hash`),
`timeframe`, `max_symbols_to_trade`, `max_concurrent_positions`, `candidates`.

Each candidate: `symbol`, `strategy_id`, `score` (canonical decimal string,
exactly 6 fractional digits, e.g. `"0.550000"`), `score_source`
(`"scanner_total_score"`, the only permitted value), `candidate_artifact_id`,
`candidate_generated_at_utc`, `eligible_for_paper` (must be `true`),
`eligible_for_live` (must be `false`).

### Score is never a binary float on the wire

`score` is a string, not a JSON number, validated against
`CANONICAL_SCORE_RE = r"^(0\.\d{6}|[1-9]\d*\.\d{6})$"` (Python) /
`parse_canonical_score` (Rust, byte-identical grammar). This rejects
scientific notation, a bare sign, leading zeros beyond a single `0`, and any
representation with a different fractional-digit count. Zero and negative
values are syntactically canonical but semantically rejected separately
(`nonpositive`/`noncanonical` are distinct failure reasons).

### Watchlist lineage binding

`source_watchlist_hash` binds the artifact to the exact approved
watchlist-v2 it was built from. The hash is a **canonical delimited string**
(`schema_version=...|generated_at_utc=...|max_symbols_to_trade=...|
max_concurrent_positions=...|symbols=...|strategy_assignments=sym:strat,...`),
SHA-256 hex — deliberately not JSON serialization, because JSON map-key
ordering and number formatting are library/feature-flag dependent (e.g.
serde_json's `preserve_order` feature) and would make a byte-identical
cross-language hash unsafe to rely on. `strategy_assignments` entries are
sorted by symbol before hashing so both languages produce the same digest
regardless of the source dict/map's iteration order. The Rust consumer
recomputes this hash from the raw watchlist JSON
(`watchlist_lineage_hash` in `runtime_opportunity_artifact.rs`) and rejects
a mismatch (`runtime_opportunity_watchlist_hash_mismatch`).

### Validation (Rust consumer, `evaluate_runtime_opportunity_intake`)

Every one of the task's required rejections is implemented and tested (25
unit tests in `runtime_opportunity_artifact.rs`): stale/expired, wrong
market date, non-paper mode, live approval, missing lineage, blank/duplicate
symbol, symbol/strategy mismatch against the bound watchlist, missing score,
noncanonical score, nonpositive score, wrong score source, candidate not
paper-eligible, candidate live-eligible, symbol count above
`MULTI_SYMBOL_HARD_CEILING`, and watchlist-hash mismatch. Multiple failures
accumulate rather than short-circuiting (except a missing/unreadable file or
malformed top-level JSON).

## Allocator hardening (Phase C)

[core-rs/crates/mqk-portfolio/src/allocator.rs](../../core-rs/crates/mqk-portfolio/src/allocator.rs)

The existing `Allocator::allocate` (generic, short-capable, used by research
callers) is unchanged in its short-selling capability but gained two
determinism fixes that apply to **both** lanes:

- **Duplicate symbols** are rejected (`AllocationError::DuplicateSymbol`)
  instead of being silently resolved by last-write-wins into the internal
  `BTreeMap`.
- **Tied `|score|` candidates** break ties by symbol ascending instead of
  falling back to input order via a stable sort — output is now fully
  independent of candidate input order in every case, not just the
  no-ties case.

`Allocator::allocate_long_only(equity_micros, candidates, runtime_ceiling)`
is the new runtime seam:

- `AllocationConstraints::validate()` rejects non-finite, non-positive, or
  out-of-range (`(0.0, 10.0]`) weight constraints, and internally
  inconsistent ones (`max_single_weight > max_gross_weight`,
  `max_net_weight > max_gross_weight`).
- Negative scores are rejected (`NegativeScoreOnLongOnlyLane`) — this lane
  never produces a short.
- `max_positions` (explicit or, when unconstrained, the effective ceiling
  applied) must be positive and must not exceed `runtime_ceiling`
  (`MaxPositionsExceedsCeiling`).
- All other behavior (normalisation, clipping, gross/net trimming, zero-weight
  pruning) is identical to `allocate`.

24 unit tests cover every one of the above plus determinism-across-input-order
proofs for both the generic and long-only entry points.

## What Bundle 5 does NOT touch

- `Allocator::allocate`'s short-capable behavior is unchanged — research
  callers using it directly are unaffected.
- No change to `constraints.rs`'s post-allocation checks or
  `evaluate_sector_risk` (ETF-RISK-CLOSURE-01) — unrelated gate, untouched.
