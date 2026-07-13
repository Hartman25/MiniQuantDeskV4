# DAILY-DATA-READINESS-AND-FRESHNESS-01-COMBINED — Phase A: Current-Truth Audit and Final Contract

Status: **Phase A — design/audit only. No runtime code changed in this phase.**
Starting HEAD: `6dcf8f88` (branch `main`).

This document is the required Phase A deliverable: the current-truth audit (20
questions) and the final contract the remaining phases (B–E) must build
against. It intentionally does not modify any Rust/TS source.

---

## 0. Correction to the mission brief

The mission brief names `core-rs/crates/mqk-daemon/src/native_strategy.rs` as
an inspection target. **That path does not exist.** The actual strategy
bootstrap/dispatch module lives at
`core-rs/crates/mqk-runtime/src/native_strategy.rs`
(`NativeStrategyBootstrap::bootstrap`, `build_daemon_plugin_registry`). All
citations below use the real path. Phase B/C work that touches this module
must target `mqk-runtime`, not `mqk-daemon`.

---

## 1. Which runtime paths consume `md_bars`?

Only one live, wired path: the Paper+Alpaca native-strategy bar-dispatch loop,
gated by `AppState::strategy_market_data_source() ==
StrategyMarketDataSource::ExternalSignalIngestion`. That flag is set **only**
when `deployment_mode() == Paper && broker_kind == Some(BrokerKind::Alpaca)`
(`core-rs/crates/mqk-daemon/src/state.rs:1174-1181`).

A second module, `core-rs/crates/mqk-daemon/src/state/multi_symbol_config.rs`,
builds a richer `SymbolStrategyAssignment { symbol, strategy_id, timeframe }`
config from the same watchlist-v2/legacy sources, but its own module doc
(`multi_symbol_config.rs:14-18`) states it is **not called from
`tick_strategy_dispatch`, `loop_runner.rs`, or any route** — it is inert
scaffolding for a future `MULTI-SYMBOL-DISPATCH-LOOP-01`. It must not be
treated as "the" runtime assignment source; today there is exactly one wired
consumer of `md_bars` for trading decisions.

Backtests and research (`mqk-backtest`) also read `md_bars`, but are
explicitly out of scope per the mission (`SCOPE` section) and must not be
gated by this bundle.

## 2. Which of those are autonomous PAPER paths?

The single wired consumer above IS the autonomous PAPER path
(`start_execution_runtime` → `PREMARKET-DATA-READINESS-GATE-01` gate →
native-strategy bar ticks). There is no second autonomous PAPER path today.

## 3. What is the exact canonical assignment source?

Symbols + timeframe: `market_data_freshness::required_symbols_with_source_from_env()`
(`core-rs/crates/mqk-daemon/src/market_data_freshness.rs:606-657`). Preference
order (unchanged, to be reused, not reinvented):
1. Approved `watchlist-v2` artifact (`MQK_PAPER_WATCHLIST_PATH`) → every
   `artifact.symbols` entry paired with the single global
   `MQK_STRATEGY_MD_TIMEFRAME`.
2. Legacy `MQK_STRATEGY_SYMBOL` + `MQK_STRATEGY_MD_TIMEFRAME` single pair.
3. Empty (`SYMBOL_SOURCE_NONE`).

Strategy id: **Tier A is one global strategy for the whole deployment.**
`AppState.strategy_fleet` is populated from `MQK_STRATEGY_IDS`
(`state.rs:1185-1193`, comma-separated → `Vec<StrategyFleetEntry{strategy_id}>`,
no symbol/timeframe field), and
`NativeStrategyBootstrap::bootstrap` (`mqk-runtime/src/native_strategy.rs:115`)
takes only `ids[0]` — the first configured strategy id. There is currently no
per-symbol strategy assignment wired anywhere in the runtime.

**Required identity for this bundle, given current wiring:** for each
`(symbol, timeframe)` pair returned by `required_symbols_with_source_from_env()`,
the applicable `strategy_id` is the single active fleet entry
(`strategy_fleet_snapshot()` first entry). This is not a design compromise —
it is the literal, only-possible mapping until `MULTI-SYMBOL-DISPATCH-LOOP-01`
wires per-symbol strategy assignment. The evaluator must not invent a second,
independent strategy-assignment parser.

## 4. Can ingest plan and runtime assignment ever disagree?

No. `GET /api/v1/market-data/ingest-plan` (`routes/ingest.rs:2845`) calls the
exact same `required_symbols_with_source_from_env()` function as the runtime
gate (`lifecycle.rs:612`, via the `required_symbols_for_freshness_gate_from_env`
thin wrapper). Existing test `pmr_e04_single_symbol_path_is_byte_identical_to_legacy_evaluator`
and `ip12_ingest_plan_and_preflight_market_data_readiness_agree_on_required_symbols`
already prove this identity end-to-end. The new evaluator must call the same
function — not a copy — to preserve this guarantee.

## 5. What history does each active strategy require?

No `StrategyDataRequirements`/`minimum_completed_bars`-shaped construct exists
anywhere today. Each engine hardcodes a private `LOOKBACK` constant invisible
outside its own module (not exported via `StrategyMeta`, which only carries
`name`/`version`/`timeframe_secs`/`description`). Audited values:

| strategy_id | file:line | base lookback | extra bars | **minimum_completed_bars** |
|---|---|---|---|---|
| `swing_momentum` | `engines/swing_momentum.rs:5,8` | 20 | 0 | **20** |
| `mean_reversion` | `engines/mean_reversion.rs:5,8` | 20 | 0 | **20** |
| `volatility_breakout` | `engines/volatility_breakout.rs:5,8,32` | 20 | +1 (separate current-bar comparison, line 32/41) | **21** |
| `intraday_scalper` | `engines/intraday_scalper.rs:215,221` | 5 | 0 | **5** |
| `intraday_short_scalper` | `engines/intraday_scalper.rs:218,221` (shares `signal_from_recent`) | 5 | 0 | **5** |

Dispatch is registry-driven, not a hardcoded match: `engines/mod.rs`'s
`register_builtin_strategies` registers each engine's `meta()`; lookup is
`PluginRegistry::instantiate_verified` doing string-equality search
(`mqk-strategy/src/plugin_registry.rs:277-298`). These 5 ids are the complete,
currently-registered, dispatchable set — no dead/legacy ids exist.

`MD_FRESHNESS_MIN_BARS = 5` (`market_data_freshness.rs:22`) is a **flat,
strategy-agnostic** global constant — it is provably insufficient for
`swing_momentum`/`mean_reversion` (need 20) and `volatility_breakout` (needs
21): it would currently pass readiness with only 5 bars loaded even though
those strategies' own internal gate (`if recent.len() < LOOKBACK { return 0 }`)
would silently no-op every dispatch. This is the exact gap
`STRATEGY-DATA-REQUIREMENTS` metadata (Phase B) closes. No ledger or code
comment anywhere frames `5` as an intentional placeholder pending a
per-strategy value — this bundle establishes that concept from scratch.

**Contract decision:** add a small, additive, non-behavioral metadata seam
(new file `core-rs/crates/mqk-strategy/src/data_requirements.rs` or a field on
`StrategyMeta`) exposing:
```rust
pub struct StrategyDataRequirements { pub minimum_completed_bars: usize }
```
populated by literally re-exporting each engine's existing `LOOKBACK`
(+1 for `volatility_breakout`) constant — zero change to indicator math,
thresholds, or signals. Unknown strategy id (not found in the registry) →
fail closed with reason code `strategy_requirement_unknown`, never a silent
default of 5.

## 6. What timestamp convention does each enabled provider use?

**`end_ts` is the bar START timestamp for every provider, despite its name.**
Proven directly in code comments:
- `alpaca_provider.rs:85-91` (`end_ts_from_alpaca_t`): *"Store Alpaca
  start-of-bar `t` as `end_ts` to match TwelveData convention already in DB."*
- `mqk-md/src/lib.rs:202-208` (`filter_completed_provider_bars` doc): *"the
  stored timestamp convention in this repo is the bar period timestamp...a bar
  is treated as completed only after `end_ts + timeframe_secs <= now_ts`."*

So "has today's daily bar landed" can never be tested as
`end_ts == market_date_epoch`; it must be tested as
`end_ts + timeframe_secs <= now_ts` (the same rule
`filter_completed_provider_bars` already enforces, `lib.rs:209-255`). The
`ProviderBar.end_ts` / `RawBar.end_ts` doc comments ("Bar end timestamp") are
stale/misleading relative to actual semantics — the new evaluator's own doc
comments must state the true convention explicitly so this is not
re-misunderstood a third time.

Existing reusable pure helpers (`mqk-md/src/lib.rs:146-168`):
`latest_closed_bar_end_ts(timeframe, now_ts)` and `next_poll_time_ts(...)`
compute cadence boundaries `<= now_ts`, but do **not** themselves subtract
`timeframe_secs` to translate "cadence boundary" into "expected `end_ts` of
the last complete stored row." The new evaluator must reuse
`Timeframe::duration_secs()` and this cadence math, but apply the
start-vs-end correction explicitly (subtract `timeframe_secs`, or reuse
`filter_completed_provider_bars`'s cutoff arithmetic directly) rather than
assume `latest_closed_bar_end_ts`'s return value is the expected stored
`end_ts`.

## 7. What is the exact expected-bar rule for premarket/regular/postmarket/weekend/holiday/early-close?

No existing function computes this end-to-end; it must be built new in Phase
B on top of the existing calendar seam
(`state/market_calendar.rs::MarketCalendarProvider` /
`NyseWeekdaysProvider`, DST/holiday/early-close-aware via
`mqk_integrity::CalendarSpec::NyseWeekdays`, coverage 2023–2028). Contract
(Phase B implements; see §"Calendar contract" below):
- **Premarket / before first session boundary**: latest *required* bar is the
  **previous completed trading session's** final applicable bar (daily: prior
  session's daily bar; intraday: prior session's last completed interval).
  Weekend/holiday previous sessions are found by walking backward through
  `MarketCalendarProvider` classifications, not by a fixed day-count.
- **Regular session, intraday**: after `bar_boundary + grace_period`, require
  the interval that just closed. Before the first interval closes, the prior
  session's final bar remains acceptable.
- **After close (postmarket)**: require the final regular-session bar for
  that trading date, after grace.
- **Daily bars**: require the latest completed trading session per the same
  calendar walk — never a fixed "4 calendar days" threshold.
- **Calendar unknown** (`MarketSessionState::Unknown` from the provider):
  fail closed — `calendar_unavailable`, `start_allowed=false`.

## 8. What publication grace is safe?

No existing "ingestion grace" constant exists distinct from the intraday
staleness cap. **Contract decision:** new constant
`MQK_DATA_READINESS_GRACE_SECS`, default **900** (15 minutes) — chosen to
match the existing `DEFAULT_INTRADAY_BAR_MAX_AGE_SECS = 900`
(`market_data_freshness.rs:36`) so operators do not have to reason about a
second, differently-tuned timing knob. Invalid/negative env value falls back
fail-closed to the 900s default (mirrors
`intraday_bar_max_age_secs_from_env`'s existing pattern,
`market_data_freshness.rs:58-64`). Surfaced in the readiness response as
`ingestion_grace_seconds`.

## 9. What future-bar skew is safe?

No existing constant. **Contract decision:** new constant
`MQK_DATA_READINESS_FUTURE_SKEW_SECS`, default **300** (5 minutes). A stored
bar whose `end_ts > now_ts + tolerance` is rejected as `latest_bar_future`
regardless of `is_complete`. Same fail-closed-on-invalid-env pattern as §8.

## 10. How will bounded continuity be calculated?

Query bound = `strictest_required_history_bars_for_assignment + small fixed
buffer` (contract: buffer = **2** bars, documented, not derived from data) —
never a full-lifetime `md_bars` scan. Within that bounded window: verify row
count, strict ascending `end_ts` ordering, no duplicate `end_ts`, no bar
beyond the future-skew tolerance (§9), and — for daily timeframes only —
compare the set of actual bar dates against the calendar's expected trading
dates over the same window (interior gap on a real trading day → blocks).
Intraday interior-gap reconstruction against the calendar is not proven safe
to implement generally in Phase B/C given the calendar seam's day-level (not
session-minute-level) granularity for early closes; intraday continuity is
therefore **PARTIAL** — row count, ordering, duplicate, and future checks are
enforced, but full interior-gap detection against expected intraday
boundaries is out of scope for this bundle and must be reported as PARTIAL,
not silently claimed via row count alone.

## 11. What provider provenance is required?

`md_bars` provenance columns (migration `0042_md_bars_provider_metadata.sql`,
already applied): `provider_id text not null default 'unknown'`,
`provider_source`, `provider_symbol`, `ingest_mode`, `provider_bar_id`,
`provider_updated_at_utc` — all nullable except `provider_id`, whose sentinel
for pre-migration/unattributed rows is the literal string `"unknown"`
(`MdBarProviderMetadata::unknown()`, `mqk-db/src/md.rs:267-277`, and the
migration's own default). **Contract decision:** for an applicable strict
evaluation, every bar inside the bounded continuity window (§10) — not only
the single latest bar — must have `provider_id != "unknown"` AND that
`provider_id` must resolve via `provider_registry::find_provider` to a
`ProviderConfig` with `enabled == true` and
`supports_asset_class("equity")`/`"etf"` and `supports_timeframe(...)` true.
Any violation in the window → `provider_provenance_invalid` (unknown/blank),
`provider_disabled`, or `provider_capability_mismatch` as appropriate, all
blocking. This is stricter than "latest bar only" because unknown-provenance
historical bars silently feed the same strategy warmup window as the latest
bar; the mission's honesty requirement ("no fabricated backfill... no
silently approve unknown provenance") extends to the whole decision-relevant
window, not just its tail.

## 12. What legacy data will become blocked?

Any symbol/timeframe whose `md_bars` rows were written before migration
`0042` (provider_id defaulted to `"unknown"`) and never re-ingested through a
registered provider will now block under §11, even if they previously passed
the legacy 5-bar/staleness-only gate. This is an expected, intentional
tightening — not a regression — per the mission's primary safety invariant.

## 13. What exact remediation unblocks it?

`provider_unknown`/`provider_provenance_invalid` → re-ingest the bounded
required window (§10) through a registered, enabled provider, e.g.
`POST /api/v1/ingest/jobs` with `mode="sync_provider"`,
`source="alpaca"` (or the currently-enabled provider id),
covering at least `required_history_bars + buffer` bars ending at the
expected latest bar. See §"Remediation contract" below for the full mapping.

## 14. Which existing durable event table can store start-attempt readiness evidence?

Three candidates inspected, per the mission's own suggested list:
- `audit_events` (`mqk-db/src/audit.rs`) — **rejected**: `run_id` is a
  required (non-optional) bind parameter tied to an existing `runs` row. The
  strict gate must be able to refuse *before* a `run_id` is ever created
  (insertion point is before `lifecycle.rs`'s run-creation step, see §16), so
  there is no `run_id` to attach on a refusal.
- `autonomous_no_trade_diagnostics` (migration `0044`) — **rejected as the
  target for *this* bundle's evidence**, not because its schema can't hold
  the data (it has `run_id: Option<Uuid>`, `reason_code`, `reason`, `stage`),
  but because its established write contract (`routes/system.rs`'s
  `autonomous_readiness` handler calls
  `st.record_no_trade_diagnostic(...)` on **every** `GET
  /api/v1/autonomous/readiness` poll, not only on actual start attempts) is
  the opposite of this mission's explicit rule ("do not persist every
  ordinary GET request... only for each actual runtime start attempt").
  Reusing it would either violate that rule or require changing that table's
  well-established existing semantics — out of this patch's minimal scope.
- **`sys_autonomous_session_events`** (migration `0032`,
  `mqk-db/src/arm_state.rs:264-297`) — **accepted**. Schema:
  `id text primary key` (caller-supplied, so a deterministic id can be used —
  `insert ... on conflict (id) do nothing` already gives idempotent
  re-run-safe semantics matching the audit-ID determinism rule),
  `ts_utc`, `event_type`, `resume_source` (nullable), `detail text`
  (freeform — sufficient to hold a compact JSON-serialized bounded readiness
  summary), `run_id` (nullable — fits a pre-run-creation refusal), `source`.
  It is already an append-only table reserved for autonomous-supervisor
  lifecycle history distinct from the high-frequency diagnostics table above,
  and its existing helper `persist_autonomous_session_event` /
  `AutonomousSessionEventRow` needs zero schema change to carry this
  evidence. **No new migration is required.**

## 15. Which current tests encode advisory/pass-through behavior that must remain backward compatible?

`scenario_data_freshness_readiness_gate_01.rs`: `dfr_u01` (db=None →
`"unavailable"`, not a blocker), `dfr_u04` (unavailable/not_applicable/ok are
all non-blockers), `dfr_a02` (paper+alpaca no-DB test env permits
unavailable/not_applicable). `scenario_premarket_data_readiness_gate_01.rs`:
`pmr_a01` (empty required → not_applicable, start_allowed=true), `pmr_a09`
(all-unavailable, no blockers → unavailable, start_allowed=true — comment
"unavailable must not block start"), `pmr_e01` (db=None, 1 symbol →
unavailable, start_allowed=true), and critically **`pmr_e04`** (asserts the
n=1 aggregate path is byte-identical to the legacy single-symbol evaluator —
the tightest existing coupling constraint). `scenario_ingest_plan_01.rs`'s
`ip08` normalizes on `(symbol, timeframe)` identity only, blind to differing
per-symbol `strategy_assignments` — if the new evaluator changes
`RequiredSymbolTimeframe`'s dedup identity to include `strategy_id`, this
test's expectation of collapsing to exactly 2 entries would break. **Contract
decision:** the new strict evaluator is an **additive, separate** function
(`daily_data_readiness.rs`), consuming (not replacing) the existing
`RequiredSymbolTimeframe`/`required_symbols_with_source_from_env()` types
unchanged. It layers strategy_id/required_history_bars alongside — it does
not alter the dedup key those legacy tests depend on. All of the above tests
continue to exercise the pre-existing advisory/legacy evaluator unmodified.

## 16. Which exact start function must consume the strict report?

`AppState::start_execution_runtime`, `core-rs/crates/mqk-daemon/src/state/lifecycle.rs:42`.
Full existing gate order (unchanged by this audit, confirmed by direct
reading): reap finished loop (46) → deployment readiness (48) → integrity
armed (59) → live-capital token (67) → active-run conflict (78) → BRK-00R-04
WS continuity, Paper+Alpaca only (85) → BRK-09R reconcile (117) → live WS
continuity (153) → TV-01/02C artifact intake (181) → TV-03C parity evidence
(257) → TV-04F/04A/04D capital policy (337/369/411) → B1A native-strategy
bootstrap + STRATEGY-DORMANCY-01 (456) → `db_pool()?` (533, first DB
requirement) → B2A DB strategy-registry (535) →
**PREMARKET-DATA-READINESS-GATE-01 (587–634, the existing legacy/advisory
gate)** → durable active-run conflict (636) → run creation (653) →
orchestrator build/tick (728+). **Contract decision:** the new strict gate is
inserted immediately after the existing legacy freshness gate (after line
634, still before durable active-run conflict/run creation at 636), since
it needs the same DB pool already fetched at line 533 and must, like the
existing gate, refuse before any run_id is created.

## 17. Does the strict gate block all relevant paper adapters, not only Alpaca?

Per the mission's explicit instruction not to key solely on
`BrokerKind::Alpaca`: **contract decision** — the strict gate's applicability
predicate is `deployment_mode() == Paper && strategy_market_data_source() ==
StrategyMarketDataSource::ExternalSignalIngestion` (the same flag the
existing legacy gate and WS-continuity/reconcile/dormancy gates already key
on). Today that flag is only ever true for Paper+Alpaca (`state.rs:1174-1181`
sets it exactly there), so behavior is unchanged today — but if a future
local `BrokerKind::Paper` adapter is ever wired to set
`ExternalSignalIngestion` (running the same native-strategy loop from
`md_bars`), the strict gate applies automatically with no further code
change. This directly satisfies "do not key applicability solely to
`BrokerKind::Alpaca`" without inventing behavior for adapters that do not yet
exist.

## 18. Is a DB migration actually necessary?

**No new migration required for Phase B's evaluator** (bounded reads only,
against `md_bars` + the existing provider registry JSON file — no schema
change). **No new migration required for Phase C's durable evidence** either
— `sys_autonomous_session_events` (§14) is reused as-is. If Phase B/C
discovers a genuine gap in that reuse during implementation, the next unused
migration id is confirmed as **`0048`** (highest applied is `0047`,
`0047_strategy_promotion_transition_lineage.sql`, per
`core-rs/crates/mqk-db/migrations/manifest.json`) — recorded here so Phase B
does not have to re-derive it, not because a migration is currently planned.

## 19. What is the exact route and response contract?

New: `GET /api/v1/market-data/readiness` (read-only, no auth, mirrors
`ingest-plan`'s public-mount convention per
`ip10_route_requires_no_db_and_is_publicly_mounted_without_auth`). Extends
(does not replace) `GET /api/v1/system/preflight` and `GET
/api/v1/autonomous/readiness` with the same canonical report or a summarized
projection of it, alongside their existing `market_data_freshness`/
`market_data_readiness` fields (which continue to reflect the legacy
evaluator — additive, not removed). Response model and reason codes: see
`REQUIRED READINESS IDENTITY`/`READINESS STATES AND REASON CODES`/`REQUIRED
RESPONSE MODEL` sections of the mission brief verbatim — Phase A adopts them
as specified with one addition: a `known_limitation` string array on the
top-level report for honestly flagging the intraday-continuity `PARTIAL`
scope (§10) so the report never implies stronger proof than it has.

One pre-existing inconsistency to be aware of, not fixed by this bundle:
`routes/ingest.rs`'s `validate_timeframe` accepts only `"1D"/"1m"/"5m"`
(uppercase/lowercase-specific), while `market_data_freshness::timeframe_secs`
accepts a much broader case-insensitive set (`"1d"`, `"daily"`, `"1h"`,
`"30m"`, etc.). The new evaluator must use `mqk_md::Timeframe::parse` (the
same parser `market_data_feed_poll_once` already uses,
`routes/ingest.rs:492`) as its single timeframe-validation authority, so an
"unsupported or ambiguous timeframe" verdict is consistent with what the
ingest/poll routes already accept — not a fourth parser.

## 20. What remains explicitly out of scope?

Crypto/futures/options/forex, LIVE enablement, new provider adapters,
scheduler persistence, provider retry/backoff, automatic ingestion, backtest
GUI data-source repair, strategy selection/allocation, portfolio/P&L
durability, broad GUI redesign, replacing the per-tick staleness gate
(`INTRADAY-MD-FRESHNESS-AUTONOMOUS-01`, a distinct dispatch-time mechanism
proven independent by `scenario_intraday_md_freshness_autonomous_01.rs` — left
untouched), and `MULTI-SYMBOL-DISPATCH-LOOP-01` (per-symbol strategy
assignment wiring) — this bundle consumes the Tier-A single-global-strategy
reality (§3) rather than building that future wiring.

---

## Final contract summary (binding for Phases B–E)

- **Evaluator module**: `core-rs/crates/mqk-daemon/src/daily_data_readiness.rs`, pure + DB-bounded, additive alongside `market_data_freshness.rs` (not a replacement).
- **Strategy metadata seam**: `mqk-strategy` gains `StrategyDataRequirements { minimum_completed_bars }`, populated per §5 table. Unknown id → fail closed (`strategy_requirement_unknown`).
- **Identity**: `(strategy_id, symbol, timeframe, required_history_bars)`, strategy_id from the Tier-A single active fleet entry, symbol/timeframe from the existing shared resolver (§3/§4). No second symbol parser.
- **Calendar**: reuse `MarketCalendarProvider`/`NyseWeekdaysProvider`; `Unknown` → `calendar_unavailable`, blocked.
- **Grace**: `MQK_DATA_READINESS_GRACE_SECS`, default 900s, fail-closed-to-default on invalid/negative.
- **Clock skew**: `MQK_DATA_READINESS_FUTURE_SKEW_SECS`, default 300s.
- **Continuity bound**: `required_history_bars + 2`; daily gap-checked against calendar, intraday reported PARTIAL for interior-gap detection.
- **Provenance**: every bar in the bounded window must be a known, enabled, capability-matching provider — not only the latest bar.
- **Route**: `GET /api/v1/market-data/readiness`, read-only, public-mounted, no provider/DB-write/scheduler/order side effects.
- **Start gate**: `AppState::start_execution_runtime` (`lifecycle.rs`), inserted immediately after the existing legacy PREMARKET-DATA-READINESS-GATE-01 block (after line 634), before durable active-run conflict/run creation.
- **Applicability**: `deployment_mode()==Paper && strategy_market_data_source()==ExternalSignalIngestion` — not hardcoded to `BrokerKind::Alpaca`.
- **Durable evidence**: reuse `sys_autonomous_session_events` (migration 0032, `persist_autonomous_session_event`) with a deterministic `id`, `run_id: None` on refusal / `Some(run_id)` on success, `detail` carrying a compact JSON-serialized bounded readiness summary. **No new migration.**
- **Timeframe parsing**: `mqk_md::Timeframe::parse`, the same authority `market_data_feed_poll_once` uses.
- **Backward compatibility**: zero changes to `market_data_freshness.rs`'s existing evaluator, its dedup identity, or any of the tests enumerated in §15.
