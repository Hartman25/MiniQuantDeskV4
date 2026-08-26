# Broad Research Universe — Current Truth Audit

**Mission:** DIRECT-RANK-AND-BROAD-UNIVERSE-RESEARCH-01, Patch C
(BROAD-RESEARCH-UNIVERSE-CURRENT-TRUTH-AUDIT-01)

**Scope:** read-only. No production behavior changes in this patch.

**Audited HEAD:** worktree `MiniQuantDeskV4-direct-rank-policy`, branch
`research-direct-rank-policy-01`, commit `830068ed` (Patch B tip).

**Method:** every count/list below was independently verified against the
committed file content at this HEAD (`config/instruments/equities.json`
parsed directly; source files read directly) — this document does not rely
on any prior audit, memory, or ledger claim. Where an older document (e.g.
the 2026-08-10 audit) reported the same number, that is noted as
corroboration, not as the source of truth.

---

## Universe concept definitions (do not collapse these)

| Concept | Definition | Current state |
|---|---|---|
| **CURRENT_DISCOVERY_UNIVERSE** | Symbols MiniQuantDesk could consider trading *right now*, if it queried a live provider for all tradable US symbols. | **Not implemented.** No such query exists anywhere in the repo (see Q11/Q12). |
| **CURRENT_REGISTRY_RESEARCH_SEED** | The current, committed `config/instruments/equities.json` snapshot, usable *today* as a Research development seed. | **Exists.** 88 enabled equity symbols (see Q1-Q6). Not point-in-time. |
| **HISTORICAL_POINT_IN_TIME_RESEARCH_UNIVERSE** | Symbols that were genuinely eligible/listed *as of each historical date* being backtested. | **Not implemented.** `POINT_IN_TIME_UNIVERSE_AVAILABLE=NO` (see Q13). |
| **ACTIVE_PAPER_UNIVERSE** | The small set actually authorized for live Paper trading in a session. | **Exists, currently 1 symbol** (AAPL, 5-minute bars — see Q6). Hard-capped at 5 by `MULTI_SYMBOL_HARD_CEILING` (see Q10). |

A large Research/discovery universe (88 symbols, or a future provider-driven
thousands) does **not** authorize expanding the Active Paper Universe. That
remains a separate, explicit operator decision gated by its own caps.

---

## Q1. `config/instruments/equities.json` — schema & count

**Total entries: 88** (verified: `len(json.load(...)) == 88`).

Every entry carries 10 required fields: `instrument_id`, `symbol`,
`asset_class`, `provider`, `provider_symbol`, `venue`, `currency`,
`enabled`, `timeframes` (array), `notes`. Three optional fields
(`instrument_kind`, `sector`, `category`) are present on a subset only.

Rust mirror type: `core-rs/crates/mqk-md/src/instrument_registry.rs:27-70`
(`struct TrackedInstrument`).

## Q2. Enabled + equity filter

Field names: `enabled` (bool), `asset_class` (string). **88/88 entries have
`enabled: true` and `asset_class: "equity"`** — verified directly; there is
no disabled or non-equity entry in the current file. Filter logic:
`instrument_registry.rs:134-141` (`enabled_equities`), pinned by an existing
test at `instrument_registry.rs:417-427`
(`reg_08_enabled_equity_count_matches_backfill_universe`).

## Q3. Exact enabled symbol list (88, sorted)

```
AAL, AAPL, ACHR, AFRM, AMD, AMZN, ARKK, BAC, BITO, CCL, CHPT, CSCO, CVX, DIA,
DKNG, EEM, EFA, F, FXI, GDX, GDXJ, GE, GLD, GOOGL, GS, HD, HIMS, HOOD, IBM,
ICLN, IEF, INTC, IONQ, IWM, JBLU, JNJ, JOBY, JPM, KGC, KO, KWEB, LCID, LYFT,
MARA, META, MSFT, NCLH, NEM, NFLX, NIO, NVDA, OPEN, ORCL, PEP, PFE, PLTR,
PLUG, QQQ, RBLX, RIOT, RIVN, RKLB, SHY, SLV, SMH, SOFI, SPY, T, TAN, TLT,
TSLA, UAL, UPST, VNQ, VTI, VZ, WFC, WMT, XLB, XLE, XLF, XLI, XLK, XLP, XLU,
XLV, XLY, XOM
```

## Q4. Venue distribution

3 distinct `venue` values, summing to 88: **NYSE: 30, NASDAQ: 29, NYSEARCA:
29** (verified via `Counter`).

## Q5. ETF-tagged vs untagged

No boolean `is_etf` field. The marker is the optional `instrument_kind`
field. **14/88 tagged `instrument_kind: "etf"`**: `SPY, QQQ, IWM, DIA, XLK,
XLF, XLE, XLI, XLP, XLU, TLT, IEF, SHY, GLD`. **74/88 carry no
`instrument_kind` field at all.**

Caveat: at least 16 more symbols in the untagged 74 are economically ETFs
but not formally marked (`ARKK, BITO, EEM, EFA, FXI, GDX, GDXJ, ICLN, KWEB,
SMH, TAN, VNQ, VTI, XLB, XLV, XLY`) — the registry deliberately keeps ETFs'
`asset_class` as `"equity"` (`instrument_registry.rs:33-39`), so
`instrument_kind` tagging is informational/partial (ETF-REGISTRY-01 scope),
not a complete ETF classification. Do not treat "14 tagged" as "14 total
ETFs in the registry."

## Q6. Timeframes per symbol

`timeframes` is a per-entry array. **87/88 entries carry `["1D"]`; exactly
1 entry — `AAPL` (provider `alpaca`) — carries `["5m"]` only.** AAPL's
`notes` field states this was a deliberate 2026-08-11 narrowing tied to a
live-Paper incident, and explicitly identifies AAPL/5m as *"the live
approved paper universe"* — i.e., today's entire Active Paper Universe is
one symbol.

## Q7. Hardcoded tiny universes

- `instrument_registry.rs:123-129` (`load_instrument_registry`) reads
  `config/instruments/equities.json` from disk dynamically — no hardcoded
  symbol list in the Rust loader itself.
- `instrument_registry_v2.rs` is an additive, **unwired** schema/loader
  seam — its own header states nothing consumes it and v1 remains the sole
  registry any daemon/CLI/ingestion/backtest/GUI path reads.
- `research-py/` never reads `config/instruments/equities.json` at all —
  zero references to that file or to `instrument_registry` under
  `research-py/`. The Rust registry and Research-py are currently fully
  decoupled.
- One hardcoded fixed universe exists:
  `research-py/experiments/short_01_etf_trend/run_experiment.py:157-161` —
  a literal 12-symbol ETF list (`SPY, QQQ, IWM, DIA, XLF, XLK, XLE, XLV,
  XLI, XLY, XLP, XLU`), explicitly commented as a frozen ex-ante universe
  for one specific experiment (SHORT-01). This is scoped to that one
  experiment, not a system-wide default, and is the same 12-ETF set the
  now-superseded DIRECT-SIGNED-RANK-RESEARCH-POLICY-01 controller would
  have reused for Wave-03 — this is precisely what this mission's Patch D/E
  replace with the broader 88-symbol seed.

## Q8. Scanner paths that already iterate registry symbols

`runtime_opportunity_allocation.rs`, `runtime_opportunity_artifact.rs`,
`runtime_opportunity_mode.rs`, `mqk-portfolio/dynamic_selection.rs`,
`conflict_policy.rs` do **not** loop the full registry — they operate on a
caller-supplied `eligible_symbols` slice capped at 5 (Q10).

The one true full-registry loop is
**`core-rs/crates/mqk-backtest/src/strategy_scanner.rs:654-711`**
(`execute_strategy_scan`):

```rust
let instruments = load_instrument_registry(Path::new(&req.registry_path))...;
let mut universe = enabled_equity_symbols(&instruments);
if let Some(limit) = req.limit_symbols { universe.truncate(limit); }
...
for symbol in &universe {
    // register strategies per symbol, load local bars, evaluate every
    // (symbol, strategy) pair
}
```

Wired into both the CLI (`mqk backtest scan-strategies`,
`mqk-cli/src/commands/bkt.rs`) and the daemon (`POST
/api/v1/strategy-scans/jobs`, `mqk-daemon/src/routes/strategy_scans.rs`).

## Q9. Runtime paths that rank/select strategy-symbol evidence

- Rust: `strategy_scanner.rs:447-473` (`rank_scan_candidates`) — sorts by
  truth-state group, then score desc, then symbol/timeframe/strategy_id.
- Rust: `mqk-portfolio/src/dynamic_selection.rs` (`compute_dynamic_selection_plan`)
  — per eligible symbol, picks the single best fingerprint-validated
  candidate strategy.
- Python: `research-py/src/mqk_research/scanner/selector.py:150-201`
  (`select_ranked_candidates`) — filters, sorts by `(-total_score,
  -liquidity_score, -regime_score, symbol)`, truncates to `max_candidates`.
- Python: `research-py/src/mqk_research/universe/build.py:66-74`
  (`build_universe_swing_v1`) — scores `ret_60d + trend_proxy`, truncates
  to `top_k`. See "existing universe module" note below — this is a
  rank/filter stage over an already-assembled features frame, not a
  symbol-source.

## Q10. Hard symbol/candidate caps

| Constant | Value | Location |
|---|---|---|
| `MAX_ELIGIBLE_SYMBOLS` | 5 | `mqk-portfolio/src/dynamic_selection.rs:462` |
| `MAX_STRATEGY_UNIVERSE` | 5 | `dynamic_selection.rs:470` |
| `MAX_CANDIDATE_PAIRS` | 25 (5×5) | `dynamic_selection.rs:474` |
| `MULTI_SYMBOL_HARD_CEILING` | 5 | `mqk-daemon/src/watchlist_intake.rs:86` |
| `REQUIRED_MAX_SYMBOLS` (watchlist v1) | 1 | `watchlist_intake.rs:89` |
| `REQUIRED_MAX_CONCURRENT` (watchlist v1) | 1 | `watchlist_intake.rs:92` |
| `SelectorConfig.max_symbols_to_trade` (research-py v1) | 1 (forced) | `research-py/.../scanner/selector.py:42,270` |

The live/Paper runtime is hard-capped at 5 concurrently eligible symbols
(watchlist-v2 architecture), and today's actual live-Paper path is capped
at 1 (currently AAPL — Q6).

## Q11. Can the runtime scanner discover symbols outside the registry?

**No.** The only Alpaca asset endpoint used anywhere in `core-rs` is a
**per-symbol** lookup (`fetch_asset`, `GET /v2/assets/{symbol}`,
`mqk-broker-alpaca/src/lib.rs:458-463`), called only for shortable
preflight on an already-known symbol. No `GET /v2/assets` (bulk,
no-symbol-filter) call exists anywhere.

## Q12. Official provider asset-list/universe ingestion seam?

**Confirmed absent.** No file in `core-rs` or `research-py` calls a
broker/provider full asset-list endpoint. Nothing comparable exists on the
Research side either — every symbol list in `research-py` today is
caller/artifact-supplied (Q7, Q13).

## Q13. Point-in-time universe membership in research-py?

**Not implemented, and explicitly declared as such in code.**
`research-py/src/mqk_research/data/bars_provenance.py:206-215` defines
`UNIVERSE_MODE_FIXED_EX_ANTE` and `UNIVERSE_MODE_POINT_IN_TIME` but
restricts `_SUPPORTED_UNIVERSE_MODES` to `{fixed_ex_ante}` only, with an
explicit comment: *"Point-in-time dynamic universe semantics are NOT
implemented — claiming that mode here would be false, so it is named but
deliberately unsupported."* This is fail-closed enforced at
`bars_provenance.py:916-922` — any manifest claiming a non-`fixed_ex_ante`
universe mode raises `BarsProvenanceUnverifiable` and blocks official
registered economic evaluation.

A future patch ID already exists for this gap,
`RESEARCH-POINT-IN-TIME-UNIVERSE-01`, status **DEFERRED / CONDITIONAL**
per `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md:2161-2165`.

**`POINT_IN_TIME_UNIVERSE_AVAILABLE=NO`**

## Q14. Delisted-symbol history?

**None found.** No `delisted`/`delisting` field, table, or code anywhere in
`config/`, `core-rs/crates/mqk-db`, or any migration. Corroborated by
`docs/research/Research_Backtest_V1_Closeout_Audit.md:2209,2405`, which
already lists "no delist/rename mapping" and "no corporate-action... or
delisting handling exists anywhere in research-py" as known open gaps.

**`DELISTED_HISTORY_AVAILABLE=NO`**

## Q15. Survivorship/look-ahead risk of applying today's registry to 2016-2024

Given Q13/Q14, using the current 88-symbol registry snapshot as a
backward-applied universe over 2016-2024 carries real, currently
undocumented-if-unlabeled survivorship bias:

- Every symbol present today survived to 2026-08-23 by construction — any
  equity delisted, merged, or renamed 2016-2024 (failed SPACs, acquired
  names, bankruptcies) is invisible, which tends to inflate apparent
  historical returns/Sharpe versus a true point-in-time universe.
- The registry was seeded opportunistically ("seeded from existing daily
  backfill universe") and patched incident-by-incident (the AAPL 5m note),
  not curated for historical-membership honesty.
- The repo already names and anticipates this exact risk
  (`UNIVERSE_MODE_POINT_IN_TIME`, `RESEARCH-POINT-IN-TIME-UNIVERSE-01`) but
  has not implemented mitigation. `docs/research/ALPHA_DISCOVERY_01_REPORT.md`
  lists survivorship/delisting as an explicit open gap on prior research.
- The one existing safeguard is procedural, not corrective: forcing every
  manifest to self-label `fixed_ex_ante` prevents the system from *lying*
  about point-in-time status, but does not remove the underlying bias.

## Q16. Safest broad development universe available today

**The current enabled-equity registry snapshot (all 88 symbols from
`config/instruments/equities.json`) is the safest available *non-*
point-in-time seed**, because:

- It is the only place in the repo with more than a handful of symbols
  assembled deliberately as a "universe" — Research-py's own artifacts are
  always caller-supplied small/single-symbol lists, and the live runtime
  caps at 5 (currently 1).
- It is schema-validated (`validate_registry`,
  `instrument_registry.rs:163-251`) and provider/venue-attributed.
- It **must** be labeled non-point-in-time (`fixed_ex_ante` /
  `CURRENT_REGISTRY_SNAPSHOT_NOT_POINT_IN_TIME`, see Patch D) — never
  claimed as historical membership truth — and any 2016-2024 backtest over
  it must carry an explicit survivorship-bias caveat, consistent with
  `docs/specs/backtest_policy.md:35-37`: *"Broader universes require
  survivorship membership datasets."*

## Q17. What's required to grow beyond ~88 to hundreds/thousands of current US symbols

Given Q11/Q12 (no list-assets seam exists anywhere), this requires new
build work, not a wiring change:

1. A provider integration that calls a **bulk** asset-list endpoint — e.g.
   Alpaca `GET /v2/assets?status=active&asset_class=us_equity` (today only
   the per-symbol `GET /v2/assets/{symbol}` exists) — or an equivalent bulk
   call from another configured provider (TwelveData has no bulk-symbol
   call either today).
2. A registry-ingestion path turning that bulk response into either (a)
   new `config/instruments/equities.json` entries following the existing
   `TrackedInstrument` schema, or (b) a separate Research-py-only universe
   artifact, since Research-py currently has zero coupling to the Rust
   registry (Q7).
3. Liquidity/data-quality gating on top of the raw bulk list before it's
   usable as a discovery universe (this document does not scope that
   work — see "What this controller does not do" in the mission).

This is explicitly **out of scope** for the current mission — see "WHAT
THIS CONTROLLER DOES NOT DO" in the mission brief. This document records
what would be needed, without doing it.

---

## Existing Research-py universe module check

**`research-py/src/mqk_research/universe/build.py` already exists** (only
file in that package directory). It is a **rank-and-filter** module
(`build_universe_swing_v1`), not a symbol-source/seed module — it takes an
already-assembled `features: pd.DataFrame` (caller must already have
populated a `symbol` column) plus a policy dict, applies deterministic
price/ADV/earnings filters, scores, and truncates to `top_k`. It does not
read `config/instruments/equities.json`, does not call any provider, and
has no notion of point-in-time membership.

**Conclusion: Patch D's new universe-snapshot/seed module is not
duplicative of `build.py`.** `build.py` is a downstream ranking/filtering
stage that would consume a snapshot module's symbol list as (part of) its
input; it is not a competing universe-seed. Patch D adds a new,
distinctly-named module in the same package (`universe/snapshot.py`) to
avoid any naming collision with the existing, tested `build.py` public
surface.

---

## Classifications (per mission)

- `CURRENT_DISCOVERY_UNIVERSE` = not implemented (Q11/Q12).
- `CURRENT_REGISTRY_RESEARCH_SEED` = the 88-symbol `equities.json` snapshot
  (Q1-Q6), usable today as a Research development seed, not as Live/Paper
  authorization.
- `HISTORICAL_POINT_IN_TIME_RESEARCH_UNIVERSE` = not available.
  **`POINT_IN_TIME_UNIVERSE_AVAILABLE=NO`**
- `ACTIVE_PAPER_UNIVERSE` = 1 symbol today (AAPL/5m), hard-capped at 5.

This document changes no behavior. Patch D builds the narrow Research
universe-snapshot/identity seam this audit shows does not yet exist.
