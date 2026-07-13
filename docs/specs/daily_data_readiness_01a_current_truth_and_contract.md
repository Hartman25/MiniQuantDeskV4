# DAILY-DATA-READINESS-AND-FRESHNESS-01-COMBINED — Phase A: Current-Truth Audit and Final Contract

Status: **Phase A — design/audit only. No runtime code changed in this phase.**
Starting HEAD: `6dcf8f88` (branch `main`).

This document is the required Phase A deliverable: the current-truth audit (20
questions) and the final contract the remaining phases (B–E) must build
against. It intentionally does not modify any Rust/TS source.

> **Correction notice (`DAILY-DATA-READINESS-01A-CONTRACT-CORRECTION-01`):**
> the version of this document committed at `242de234` contained material
> current-truth errors, corrected below and verified directly against the
> repo in this correction pass (not merely asserted): the multi-symbol
> dispatch loop is **already wired and live** (§1/§3), the bar-start
> timestamp finding does not universally imply
> `end_ts + timeframe_secs <= now_ts` as a daily rule (§6), a typed
> session-schedule seam is required beyond raw `MarketCalendarProvider`
> output (§6a), grace/skew must be timeframe-aware not fixed (§8/§9),
> intraday continuity may not be predeclared `PARTIAL` (§10), the strict
> gate's insertion point and `Option<&PgPool>` signature were wrong (§16),
> durable-evidence ordering needed strengthening (§14), strategy data
> requirements must live on `StrategyMeta` specifically (§5), two of five
> registered strategies use a timeframe (`1h`) the current provider/ingest
> stack cannot serve at all (§10/§13), and asset-class resolution for the
> provider-capability check was unspecified (§11a). Sections not called out
> below are unchanged from the original audit and remain verified.

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

## 1. Which runtime paths consume `md_bars`? — CORRECTED

**Corrected finding: the multi-symbol dispatch loop is already wired and
live, not inert scaffolding.** The original audit trusted
`multi_symbol_config.rs`'s own module-doc comment ("not called from
`tick_strategy_dispatch`, `loop_runner.rs`, or any route") without checking
the actual call site. Direct verification in this correction pass:

- `state/loop_runner.rs:68-71` calls
  `super::build_multi_symbol_runtime_config_from_env()` once, synchronously,
  before the async loop starts, producing
  `Vec<SymbolStrategyAssignment>` (`multi_symbol_assignments`).
- `state/loop_runner.rs:620-622` calls
  `state_arc.tick_strategy_dispatch_multi_symbol(&multi_symbol_assignments).await`
  on every tick — this **is** the live per-tick dispatch entry point, not
  `tick_strategy_dispatch()` (the old single-symbol function, now
  superseded at the call site though still present as a helper).
- `AppState::tick_strategy_dispatch_multi_symbol` (`state.rs:2564-2585`)
  iterates every assignment and calls
  `dispatch_native_strategy_for_symbol_with_bar(&assignment.symbol,
  &assignment.timeframe, bar)` for each one.
- That function (`state.rs:2347+`) ultimately calls
  `invoke_native_strategy_on_bar_from_window`, which invokes
  `self.native_strategy_bootstrap.lock().await.as_mut()?...` — **a single,
  shared `AppState.native_strategy_bootstrap` instance**, not one selected
  by `assignment.strategy_id`. **`tick_strategy_dispatch_multi_symbol` does
  not instantiate or select a strategy per assignment; every assignment is
  dispatched through the same active bootstrap.**
- `NativeStrategyBootstrap::bootstrap` (`mqk-runtime/src/native_strategy.rs:115`)
  still consumes only `ids[0]` from the configured `MQK_STRATEGY_IDS` fleet —
  **the single active bootstrap is still Tier-A single-strategy**, unchanged
  by the multi-symbol loop wiring.
- `state.rs:2602-` (`retain_targets_matching_symbol`) is a **fixed-target-symbol
  guard**: because the bootstrap's `StrategyHost` emits `TargetPosition.symbol`
  fixed at construction time from `MQK_STRATEGY_SYMBOL` (independent of which
  symbol's bar window was actually just dispatched), a target whose symbol
  does not match the dispatched assignment's symbol is dropped rather than
  submitted under a misattributed symbol. Documented at
  `docs/design/native_multi_symbol_dispatch.md` as the "per-symbol strategy
  bootstrap gap." This guard is a no-op for the legacy single-symbol case
  (bootstrap's baked symbol == dispatched symbol) but is load-bearing for
  every non-primary symbol in a watchlist-v2 multi-symbol assignment list
  today.

**The `multi_symbol_config.rs` module-header comment claiming this is
unwired is stale documentation, not source of runtime truth** — a live
example of exactly the failure mode `.claude/rules/audit_repo_truth_rules.md`
warns against ("if a doc claims something is done, verify... if memory
contradicts the current file state, trust the file"), and a caution to
carry into Phase B: do not trust any module's self-description without
grepping its actual call sites.

**Practical consequence for readiness:** watchlist-v2 currently supplies
real per-symbol `SymbolStrategyAssignment{symbol, strategy_id, timeframe}`
values that are genuinely dispatched, one bar-tick at a time, for every
listed symbol — but every assignment's strategy **decision** is produced by
the one shared bootstrap (`ids[0]`'s engine), regardless of what
`assignment.strategy_id` says. When `assignment.strategy_id != ids[0]`, the
assignment is dispatched (its bars are loaded, its symbol enters the tick),
but any resulting target is for the *wrong* strategy's logic, filtered only
by the symbol-match guard above — it is not filtered on strategy identity
mismatch at all today. This is the exact gap the corrected readiness
identity (§3) must surface as a blocking condition rather than silently
trusting `assignment.strategy_id`.

## 2. Which of those are autonomous PAPER paths?

Unchanged: the wired consumer above IS the autonomous PAPER path
(`start_execution_runtime` → gates → native-strategy bar ticks via
`tick_strategy_dispatch_multi_symbol`). There is no second autonomous PAPER
path today.

## 3. What is the exact canonical assignment source? — CORRECTED

**Corrected finding: two different resolvers exist today and disagree on
identity richness — this is exactly the discrepancy this bundle must not
perpetuate.**

- The execution loop (`loop_runner.rs:69`) uses
  `build_multi_symbol_runtime_config_from_env()` →
  `MultiSymbolRuntimeConfig.symbols: Vec<SymbolStrategyAssignment{symbol,
  strategy_id, timeframe}>` — full per-symbol identity including a
  configured strategy id, source-preference: approved watchlist-v2 artifact
  (`strategy_assignments` map, one entry per symbol) else
  `build_legacy_single_symbol_config` (single `MQK_STRATEGY_SYMBOL` +
  first `MQK_STRATEGY_IDS` entry + `MQK_STRATEGY_MD_TIMEFRAME`).
- The freshness gate, ingest-plan route, preflight, and autonomous-readiness
  all instead use
  `market_data_freshness::required_symbols_with_source_from_env()` →
  `Vec<RequiredSymbolTimeframe{symbol, timeframe}>` — **symbol/timeframe
  only, no strategy id at all.**

**Contract decision (corrected):** the strict evaluator must consume
`build_multi_symbol_runtime_config_from_env()`'s
`Vec<SymbolStrategyAssignment>` as its canonical assignment source — the
same call the execution loop actually makes — not the symbols-only
resolver. This is not a new parser; it is switching to the richer one that
already exists and is already load-bearing in the loop. Legacy single-symbol
fallback is preserved automatically (`build_legacy_single_symbol_config` is
already the fallback branch inside the same function). Config-builder `Err`
(both watchlist-v2 and legacy fallback fail) or an empty assignment list
must fail closed for an applicable autonomous PAPER start (`required_assignments_missing`).

**Corrected readiness identity**, replacing the Tier-A-only tuple from the
original audit:
```text
symbol
timeframe
configured_strategy_id       // assignment.strategy_id from build_multi_symbol_runtime_config_from_env()
effective_runtime_strategy_id // the single active bootstrap's strategy id (fleet[0] / ids[0])
required_history_bars        // derived from effective_runtime_strategy_id's StrategyMeta (§5)
```
Until per-symbol strategy bootstrapping is implemented (explicitly out of
scope — see §20), any assignment where
`configured_strategy_id != effective_runtime_strategy_id` must produce
`runtime_strategy_assignment_mismatch` and `start_allowed=false` for that
assignment. This directly reflects the real gap documented in §1: today the
runtime would silently dispatch such an assignment through the wrong
strategy's logic (filtered only by the symbol-match guard, not a strategy
match), and the readiness evaluator must not certify that combination as
"ready." This bundle does **not** implement per-symbol strategy
bootstrapping — it only makes the existing mismatch a visible, blocking
readiness fact instead of a silent one.

`required_history_bars` is derived from `effective_runtime_strategy_id`
(the strategy that will *actually* run), not `configured_strategy_id` —
using the configured-but-not-running id's requirement would be dishonest
about which engine's warmup actually matters for the bars that get loaded.

## 4. Can ingest plan and runtime assignment ever disagree? — CORRECTED

**Yes, today, on strategy id** — because ingest-plan/preflight/autonomous-readiness
use the symbols-only resolver (§3) while the loop uses the richer
per-symbol-assignment resolver. They cannot disagree on symbol/timeframe
(both ultimately read the same watchlist-v2 artifact or the same legacy env
vars), but only the loop's resolver carries `strategy_id` at all today.

**Contract decision:** Phase C must extend `GET
/api/v1/market-data/ingest-plan`, `GET /api/v1/system/preflight`, and `GET
/api/v1/autonomous/readiness` to additionally surface
`configured_strategy_id` (and, where applicable, `effective_runtime_strategy_id`)
per assignment, sourced from the same `build_multi_symbol_runtime_config_from_env()`
call the strict evaluator and the loop use — not a duplicate parse. This is
additive to their existing `symbol`/`timeframe`/`source` fields (confirmed
safe against `ip09_response_shape_has_all_documented_fields_with_correct_types`,
which asserts presence/type, not a closed field set — see §15).
Phase E's closure proof must include a test proving the ingest plan, the new
readiness route, and the loop's actual `multi_symbol_assignments` list agree
on symbol, timeframe, *and* configured strategy ID for the same environment
— not just symbol/timeframe as the original (pre-correction) contract
required.

## 5. What history does each active strategy require? — CORRECTED (binding location)

Audited values are unchanged and remain verified:

| strategy_id | file:line | base lookback | extra bars | **minimum_completed_bars** |
|---|---|---|---|---|
| `swing_momentum` | `engines/swing_momentum.rs:5,8` | 20 | 0 | **20** |
| `mean_reversion` | `engines/mean_reversion.rs:5,8` | 20 | 0 | **20** |
| `volatility_breakout` | `engines/volatility_breakout.rs:5,8,32` | 20 | +1 (separate current-bar comparison, line 32/41) | **21** |
| `intraday_scalper` | `engines/intraday_scalper.rs:215,221` | 5 | 0 | **5** |
| `intraday_short_scalper` | `engines/intraday_scalper.rs:218,221` (shares `signal_from_recent`) | 5 | 0 | **5** |

**Corrected binding location:** the original audit left the exact home
ambiguous ("new file... or a field on `StrategyMeta`"). **Corrected
decision: this metadata must be attached to `StrategyMeta`
(`mqk-strategy/src/plugin_registry.rs`, alongside `name`/`version`/
`timeframe_secs`/`description`) or an equivalent strategy-owned plugin
metadata contract — never a separate daemon-side hardcoded strategy-id →
bar-count lookup table.** A daemon-side table would silently drift from the
engine's real `LOOKBACK` constant the moment either changes independently;
binding it to the plugin's own registered metadata means the engine and its
declared requirement can only change together, in the same file, in the
same review.

```rust
// mqk-strategy/src/plugin_registry.rs (or the engine's own meta() constructor)
pub struct StrategyDataRequirements { pub minimum_completed_bars: usize }
```
populated by literally re-exporting each engine's existing `LOOKBACK`
(+1 for `volatility_breakout`) constant into its own `meta()` — zero change
to indicator math, thresholds, or signals. A strategy id not found in the
registry, or found but missing this metadata, fails closed with
`strategy_requirement_unknown` — never a silent default of 5.

## 6. What timestamp convention does each enabled provider use? — CORRECTED (scope of the "+timeframe_secs" rule)

**Retained finding, unchanged:** `end_ts` is generally the bar-**start**
timestamp / provider label for every enabled provider, proven directly in
code comments (`alpaca_provider.rs:85-91`, `mqk-md/src/lib.rs:202-208`).

**Corrected finding: this does not mean every timeframe, especially `1D`,
is "complete" exactly when `end_ts + timeframe_secs <= now_ts`.** That
formula is `filter_completed_provider_bars`'s *ingest-time* completeness
filter (deciding whether to accept/reject a freshly-fetched provider bar as
"done forming"), not a general definition of "the expected row for trading
date D has landed." Treating it as the latter conflates two different
questions:
- *Ingest-time completeness* — is this specific bar's period over? (the
  existing formula, unchanged, still correct for that purpose.)
- *Readiness-time expectation* — for trading date D (or session interval I),
  does the **expected** row exist yet, given the actual NYSE calendar and
  the provider's proven daily-bar label convention?

**Corrected contract:**
- **Daily rows**: expected by the **calendar's expected NYSE trading date**
  (via the typed schedule seam, §6a), not by adding `86_400` to a raw
  `end_ts`. Phase B must **prove** (not assume) what `end_ts` value Alpaca's
  daily bars actually use as their label (e.g. UTC midnight of the trading
  date vs. some other convention) against real/fixture ingested `1D` data
  before writing the date-to-`end_ts` mapping — the audit found the general
  start-timestamp convention but did not verify the daily-specific label
  value, and the mission's own instruction ("do not assume an exact
  NYSE-close timestamp if stored provider bars use another canonical label")
  applies here directly.
- **Daily availability**: a trading date's row is expected once
  `session_close_utc(date) + publication_grace <= now_utc` (§6a/§8) — not a
  fixed calendar-day count.
- **Intraday expected timestamps**: from a **session-open-anchored grid**,
  not raw epoch-hour alignment. `mqk_md::latest_closed_bar_end_ts`'s
  `now_ts.div_euclid(cadence) * cadence` arithmetic aligns to **UTC epoch
  boundaries** — correct for ingest-time polling cadence, but **wrong** as
  an expected-bar generator for a session that opens at 09:30 ET, which is
  not a UTC-epoch-aligned instant. A `1h` grid must be `09:30, 10:30, 11:30,
  ..., 15:30/16:00 ET` (regular session is 6.5 hours — the final slot ends
  at the actual close, it is not a full hour), not "whatever whole UTC hour
  contains 09:30 ET." A `5m`/`1m` grid is anchored the same way (`09:30 +
  n*300s` / `09:30 + n*60s`), which happens to coincide with epoch alignment
  only because 09:30 ET's UTC equivalent (14:30 or 13:30 UTC depending on
  DST) is itself a multiple of both 60 and 300 seconds — this coincidence
  must not be relied on for `1h`, where it does not hold in the same way
  once the session-open offset is considered.
- **Early-close grids** end at the actual shortened close
  (`session_close_utc` from §6a for that date), not the normal 16:00 ET
  close.
- **Provider-specific normalization must stay explicit**: any provider other
  than Alpaca that this bundle's evaluator might touch (none are enabled
  today per §10/§13) must have its own proven label convention documented
  before being trusted, not assumed to match Alpaca's.

Existing reusable pure helpers (`mqk-md/src/lib.rs:146-168`,
`Timeframe::duration_secs()`) remain useful for cadence arithmetic *within*
a session-anchored grid (e.g. stepping `n * duration_secs()` from
`session_open_utc`), but must not be used directly against raw UTC-epoch
alignment for intraday expected-bar computation as the original audit
implied.

## 6a. Typed market-session schedule seam — NEW REQUIREMENT

**Corrected finding: `MarketCalendarProvider::session_for()`'s single-instant
result is not enough information to compute expected bars or continuity.**
It answers "what state is `now_utc` in," not "what is today's session open,
close, and previous trading date." A new typed helper is required:

```rust
pub struct MarketSessionSchedule {
    pub market_date: (i64, i64, i64),       // (year, month, day) ET
    pub session_open_utc: DateTime<Utc>,     // 09:30 ET, DST-correct
    pub session_close_utc: DateTime<Utc>,    // 16:00 ET, or early-close time
    pub previous_trading_date: (i64, i64, i64),
    pub is_early_close: bool,
    pub calendar_source: &'static str,       // mirrors MarketSessionTruth::source
    pub coverage_state: CalendarCoverageState,
}

pub enum CalendarCoverageState { Active, Stale, Invalid, OutOfRange, Unknown }
```

Built by walking `MarketCalendarProvider` day-by-day backward/forward from
`now_utc` (using its existing DST/holiday/early-close classification) rather
than reinventing calendar math — this is composition, not a second calendar
implementation.

**Corrected coverage requirement — this is the critical part:**
`NyseWeekdaysProvider`'s own module doc (`state/market_calendar.rs:163-169`)
states plainly that its holiday/early-close table is bounded to 2023–2028,
and that **dates outside this bound "fall through to ordinary time-of-day
classification rather than failing closed."** That means the underlying
static heuristic provider does **not**, by itself, satisfy "unknown/stale/
invalid/out-of-range calendar coverage blocks." The typed schedule seam
must impose its **own** explicit coverage-window check (the same 2023–2028
bound, or whatever bound the active provider actually proves) and report
`CalendarCoverageState::OutOfRange` → blocking, *on top of* whatever the
underlying provider returns — it cannot simply trust `MarketSessionState`
directly for this purpose. `ExchangeSourcedCalendarProvider`'s own
`in_coverage`/`source_state` fields (already fail-closed to `Unknown`
outside coverage or when not `Active`) are the better composition target
where available; the schedule seam must not regress that stricter behavior
by falling back to the non-fail-closed static provider path unless the
bundle explicitly documents doing so.

**`FixedWindowOverrideProvider` is explicitly insufficient calendar
authority for continuity checks** (per its own doc: "does not consult any
exchange calendar; DST correctness is the operator's responsibility") — if
this override is active, the schedule seam must report
`coverage_state: Unknown` (or a dedicated `configured_override_insufficient`
reason) rather than silently using the fixed window as if it were calendar
truth for gap detection.

Tests for this seam must use **injected schedules/calendars** (fixture
`MarketSessionSchedule` values / fixture `ExchangeSourcedCalendarProvider`
data), not the wall clock or the live 2023–2028 static table alone, so
edge cases (coverage boundary, early close, holiday-adjacent weekend) are
deterministic.

## 7. What is the exact expected-bar rule for premarket/regular/postmarket/weekend/holiday/early-close?

Contract unchanged in shape from the original audit, now expressed in terms
of §6a's typed schedule instead of raw `MarketSessionState`:
- **Premarket**: latest required bar is the previous trading session's
  final applicable bar, found via `schedule.previous_trading_date` (a real
  calendar walk, not a fixed day-count), for the resolved schedule of that
  previous date.
- **Regular session, intraday**: after `grid_slot_close_utc + effective_grace`
  (§8), require the interval that just closed on the **session-open-anchored
  grid** (§6). Before the first interval closes, the prior session's final
  bar remains acceptable.
- **Postmarket**: require the final regular-session bar for
  `schedule.market_date`, after grace.
- **Daily**: require the row for `schedule.market_date` once
  `schedule.session_close_utc + effective_grace <= now_utc` (§6).
- **Calendar unknown/out-of-range** (`CalendarCoverageState` not `Active`):
  fail closed — `calendar_unavailable`, blocking.

## 8. What publication grace is safe? — CORRECTED (timeframe-aware)

**Corrected: a fixed universal 900s grace is rejected.** A 15-minute grace
on a `1m` bar would accept a bar effectively 15x its own interval late as
still "on time" — too loose to mean anything for a fast timeframe. A 15-minute
grace on a `1D` bar is reasonable; on a `1m`/`5m` bar it is not.

**Contract decision:** retain `MQK_DATA_READINESS_GRACE_SECS` as the
operator-configured **ceiling** (default 900s, same fail-closed-to-default
behavior on invalid/negative as the existing
`intraday_bar_max_age_secs_from_env` pattern), but compute:
```text
effective_grace_seconds = min(configured_grace_seconds, timeframe.duration_secs())
```
so grace can never exceed one full interval of the timeframe being
evaluated. Both `configured_grace_seconds` and `effective_grace_seconds`
are surfaced separately in the response (`ingestion_grace_seconds` is
replaced by this pair) so an operator can see when the ceiling actually
bound the effective value.

## 9. What future-bar skew is safe? — CORRECTED (timeframe-aware)

**Corrected: a fixed universal 300s tolerance is rejected**, for the same
reason as §8 in reverse — 300s of "acceptable future skew" on a `1m` bar is
five bars' worth of slack, too loose to catch a genuinely bad future-dated
row.

**Contract decision:** retain `MQK_DATA_READINESS_FUTURE_SKEW_SECS` as the
configured ceiling (default 300s, same fail-closed pattern), but compute:
```text
effective_future_skew_seconds = min(configured_future_skew_seconds, 60, timeframe.duration_secs())
```
A stored bar whose `end_ts > now_ts + effective_future_skew_seconds` is
rejected as `latest_bar_future`. Both configured and effective values are
surfaced separately, same as §8.

## 10. How will bounded continuity be calculated? — CORRECTED (no predeclared PARTIAL)

**Corrected: the contract must not guarantee a `PARTIAL` bundle before
implementation starts.** The original audit's "intraday continuity is
PARTIAL, row-count-only" language is exactly the kind of optimistic
pre-scoping the mission's proof discipline prohibits — "count/order/future
checks alone cannot produce a ready verdict when interior intraday
continuity is unverified."

**Corrected contract:** for every timeframe this bundle allows into strict
autonomous PAPER readiness, Phase B must implement one of:
1. **Full session-anchored continuity** — using the §6a schedule to
   generate the exact expected grid of interval boundaries for the bounded
   window and diffing it against actual stored `end_ts` values, catching
   interior gaps precisely (this is required, not merely "count matches
   expected count," which can mask a gap-and-compensating-duplicate or an
   off-by-one at either edge); or
2. **An explicit `unsupported_intraday_continuity` blocker** for that
   timeframe — meaning that timeframe simply cannot pass strict readiness
   at all yet, honestly, rather than passing on a weaker (count/order/
   duplicate/future-only) proof that is silently presented as equivalent to
   full continuity.

Query bound remains `required_history_bars_for_the_effective_strategy + 2`
(fixed, documented buffer), never a full-lifetime scan, for both daily and
intraday. Row count, strict ascending `end_ts` ordering, no duplicate
`end_ts`, and no bar beyond `effective_future_skew_seconds` (§9) are checked
unconditionally for every timeframe as necessary-but-not-sufficient
conditions — but per the corrected rule above, they alone must never
produce `readiness_state=ready` for a timeframe whose interior-gap proof is
not implemented; such a timeframe blocks with `unsupported_intraday_continuity`
regardless of what count/order/duplicate/future checks show.

**Practical scope note:** two of the five registered strategies
(`mean_reversion`, `volatility_breakout`) use `1h`, and two more
(`intraday_scalper`, `intraday_short_scalper`) use `5m`; only
`swing_momentum` uses `1D`. Phase B must decide, and state plainly in its
own commit, which of `1h`/`5m`/`1m` it implements full session-anchored
continuity for versus which it blocks with
`unsupported_intraday_continuity` — this is a legitimate, honest, fail-closed
scope decision under this corrected rule, not a defect to hide. See also
§13: `1h` cannot currently be remediated via the ingest-job route at all
regardless of continuity-proof status.

## 11. What provider provenance is required?

Unchanged from the original audit: every bar inside the bounded continuity
window (§10) — not only the latest bar — must have `provider_id !=
"unknown"` and resolve via `provider_registry::find_provider` to an
`enabled` `ProviderConfig` supporting the assignment's asset class and
timeframe. Violations block as `provider_provenance_invalid`,
`provider_disabled`, or `provider_capability_mismatch`.

## 11a. Asset-class resolution for the provider-capability check — NEW REQUIREMENT

**Corrected gap: the original audit's provider-capability check (§11) never
specified how an assignment's asset class is determined.** Silently
classifying every symbol as `"equity"` would be exactly the kind of
optimistic default the mission's honesty rules forbid.

**Contract decision:** asset class must be derived from the canonical
instrument registry/metadata seam — `mqk_md::instrument_registry`'s
`TrackedInstrument::trading_asset_class()` (the live, production-consumed
v1 registry; per project history, registry v2
(`instrument_registry_v2::InstrumentDefinitionV2.asset_class`) remains an
optional additive seam, not yet the sole trading truth, so v1 is the
authority for this gate unless/until that changes independently of this
bundle). A symbol absent from the registry, or present with an empty/
unrecognized `asset_class` (outside `provider_registry`'s
`supports_asset_class` vocabulary), must block as
`provider_capability_mismatch` (or a dedicated `asset_class_unknown` reason
folded into it) — never default to `"equity"`.

## 12. What legacy data will become blocked?

Unchanged: any symbol/timeframe whose `md_bars` rows predate migration
`0042` (`provider_id = "unknown"`) and were never re-ingested through a
registered provider now blocks under §11, even though the legacy 5-bar gate
previously passed it. Intentional tightening, not a regression.

## 13. What exact remediation unblocks it? — CORRECTED (timeframe capability caveat)

**Corrected finding: the current enabled provider registry capability and
the existing ingest-job route both support only `1D`/`1m`/`5m`.** Verified
directly: `config/providers/providers.json`'s Alpaca entry declares
`"supported_timeframes": ["1D", "1m", "5m"]` (no `"1h"`), and
`routes/ingest.rs`'s `validate_timeframe` (used by `POST
/api/v1/ingest/jobs`) accepts only `"1D"|"1d"`, `"1m"|"1min"|"1minute"`,
`"5m"|"5min"|"5minute"` — `1h` is rejected by both. `mqk_md::Timeframe::parse`
itself *can* parse `"1h"`/`"H1"` structurally, but `capabilities_from_provider_config`
filters `supported_timeframes` against the registry config, so Alpaca's
declared capabilities exclude `H1` regardless of what the parser accepts.

**Corrected contract: do not claim `volatility_breakout`/`1h` (or
`mean_reversion`/`1h`) can be repaired using the current ingest-job route.**
For any assignment whose `(provider, timeframe)` combination is not in the
provider's declared `supported_timeframes`, the evaluator must report
`provider_capability_mismatch` with `start_allowed=false` and a remediation
note stating plainly that a future provider/timeframe-support patch (out of
this bundle's scope) is required — not a `POST /api/v1/ingest/jobs`
suggestion that would itself be refused. Practical consequence: under this
corrected contract, `mean_reversion` and `volatility_breakout` assignments
cannot reach `readiness_state=ready` today regardless of what `md_bars`
actually contains for them, until a separate timeframe-support patch lands.
This is the honest, fail-closed outcome the mission's primary safety
invariant requires, not a bug in this contract.

For `provider_unknown`/`provider_provenance_invalid` on a timeframe the
provider *does* support (`1D`/`1m`/`5m`): re-ingest the bounded required
window (§10) through a registered, enabled provider via `POST
/api/v1/ingest/jobs` (`mode="sync_provider"`, `source="alpaca"`), covering
at least `required_history_bars + 2` bars ending at the expected latest bar.

## 14. Which existing durable event table can store start-attempt readiness evidence? — CORRECTED (ordering strengthened)

Table choice unchanged and confirmed: **`sys_autonomous_session_events`**
(migration `0032`, `mqk-db/src/arm_state.rs:264-297`,
`persist_autonomous_session_event`/`AutonomousSessionEventRow`, `id text
primary key` with `on conflict (id) do nothing`, `run_id` nullable). No new
migration required. `audit_events` remains rejected (requires an existing
`run_id`); `autonomous_no_trade_diagnostics` remains rejected as the target
for *this* evidence (its established contract already persists on every
`GET /api/v1/autonomous/readiness` poll, which this bundle's rule
explicitly forbids for its own evidence).

**Corrected ordering requirements (strengthened beyond "best-effort,
non-fatal"):**
1. A schema-versioned, bounded readiness-evaluation event
   (`event_type = "daily_data_readiness_evaluated"`, deterministic `id`
   derived from the evaluation's inputs, `run_id = None`) is persisted
   **before** run creation, for every applicable strict evaluation at an
   actual start attempt (never on a GET).
2. **A `ready` verdict (`start_allowed=true`) is refused if that pre-start
   evidence cannot be persisted** — i.e., persistence success becomes part
   of the gate itself for an otherwise-ready start, not a fire-and-forget
   side effect. Failure reason: `readiness_evidence_persist_failed`,
   blocking.
3. A `blocked` verdict returns its block reason **after** the evidence
   persist is attempted, regardless of whether that attempt succeeds —
   evidence-write failure does not need to succeed to return an
   already-blocked verdict (only a would-be-`ready` verdict is gated on
   persistence success, per point 2).
4. A start that proceeds to actual run creation appends a **second, linked**
   event (`event_type = "daily_data_readiness_run_linked"`, `run_id =
   Some(run_id)`, `detail` referencing the first event's `evaluation_id`) —
   `sys_autonomous_session_events` has no `UPDATE` path
   (`on conflict (id) do nothing`), so linkage is a second row, not a
   mutation of the first.
5. `GET /api/v1/market-data/readiness` (and the summarized projections on
   preflight/autonomous-readiness) never persist an event, under any
   truth_state — this is unchanged from the original audit but is now
   explicit that it applies even to the new pre-start-evidence event types.
6. If the DB itself is unavailable or the readiness query fails, the
   overall verdict is already `db_unavailable`/`query_failed` (blocking) —
   the response must additionally and honestly report
   `evidence_persisted: false` (there is no DB to write evidence to), not
   silently omit the field as if the question did not apply.

`evaluation_id` (a stable, deterministic identifier — e.g. UUIDv5 over the
evaluation's assignment-set hash + evaluated-minute bucket) is included in
the JSON `detail` of both event types so the two rows can be correlated by
an operator or by a later closure-proof test.

## 15. Which current tests encode advisory/pass-through behavior that must remain backward compatible? — CORRECTED (new identity fields)

Unchanged list of legacy-gate tests that must keep passing exactly as
before: `dfr_u01`, `dfr_u04`, `dfr_a02` (`scenario_data_freshness_readiness_gate_01.rs`);
`pmr_a01`, `pmr_a09`, `pmr_e01`, and critically `pmr_e04`
(`scenario_premarket_data_readiness_gate_01.rs`); `ip08`'s
`(symbol, timeframe)`-only dedup identity (`scenario_ingest_plan_01.rs`).
The strict evaluator remains additive and separate
(`daily_data_readiness.rs`), consuming — not replacing — the legacy
`market_data_freshness.rs` module and its existing types.

**Corrected addition:** because the canonical assignment source is now
`build_multi_symbol_runtime_config_from_env()` (§3), not the symbols-only
resolver, Phase B/C's new tests must additionally prove: (a) a
`configured_strategy_id != effective_runtime_strategy_id` fixture produces
`runtime_strategy_assignment_mismatch` and blocks; (b) an assignment whose
timeframe the active provider does not support (`1h`) produces
`provider_capability_mismatch` and blocks regardless of `md_bars` content
(§13); (c) a timeframe without a full continuity implementation produces
`unsupported_intraday_continuity` and blocks, never `ready` (§10); (d) the
ingest-plan/preflight/autonomous-readiness extension (§4) agrees with the
loop's actual `multi_symbol_assignments` on `configured_strategy_id`, not
only symbol/timeframe.

## 16. Which exact start function must consume the strict report? — CORRECTED (insertion point and signature)

`AppState::start_execution_runtime`, `core-rs/crates/mqk-daemon/src/state/lifecycle.rs:42`.
Gate order unchanged (see original audit for the full list, lines 46–791).

**Corrected insertion point and signature.** The original contract placed
the strict gate *after* the existing `let db = self.db_pool()?;` (line 533),
reusing the already-fetched pool. **Corrected: the evaluator must accept
`Option<&PgPool>` (`self.db.as_ref()`, not a required `&PgPool`), and the
applicable strict evaluation must run *after* native-strategy
bootstrap/assignment resolution (the existing B1A gate, ~line 456-531) but
*before* the existing hard `db_pool()?` call at line 533.** Reasoning: the
strict evaluator must be the thing that produces the canonical
`db_unavailable` verdict (with its own structured reason/response,
consistent with the dedicated route) when the DB is absent — not let a
separate, generic `db_pool()?` error surface first and pre-empt the
evaluator from ever running or reporting its own honest state. The
pre-existing legacy advisory freshness gate (`PREMARKET-DATA-READINESS-GATE-01`,
lines 587-634, which genuinely does require the pool already fetched at
line 533) remains exactly where it is, later in the sequence, for full
backward compatibility — this bundle does not move or alter it.

## 17. Does the strict gate block all relevant paper adapters, not only Alpaca?

Unchanged: applicability predicate is `deployment_mode() == Paper &&
strategy_market_data_source() == StrategyMarketDataSource::ExternalSignalIngestion`,
not hardcoded to `BrokerKind::Alpaca`. Verified unchanged in this correction
pass.

## 18. Is a DB migration actually necessary?

Unchanged: no new migration required for Phase B's evaluator or Phase C's
durable evidence (`sys_autonomous_session_events` reused as-is). Next
unused migration id remains `0048` if a genuine gap is later found.

## 19. What is the exact route and response contract? — CORRECTED (no PARTIAL guarantee; remediation caveat)

`GET /api/v1/market-data/readiness` (read-only, no auth, public-mounted like
`ingest-plan`), extending (not replacing) `GET /api/v1/system/preflight` and
`GET /api/v1/autonomous/readiness`'s existing `market_data_freshness`/
`market_data_readiness` fields (legacy evaluator, untouched).

**Corrected:** the original audit's planned `known_limitation` array for a
guaranteed intraday-`PARTIAL` claim is removed — per §10, no timeframe may
report `ready` without either full continuity proof or an explicit
`unsupported_intraday_continuity` block; there is nothing left to caveat
generically once that rule is enforced per-assignment via reason codes.
Similarly, remediation text (§13) must never suggest an ingest-job
timeframe the current provider registry does not support.

Timeframe parsing authority unchanged: `mqk_md::Timeframe::parse`, the same
parser `market_data_feed_poll_once` uses — not a fourth parser, and not
`routes/ingest.rs`'s stricter `validate_timeframe` (which is CSV/ingest-job-specific,
not a general timeframe-validity authority).

## 20. What remains explicitly out of scope?

Unchanged: crypto/futures/options/forex, LIVE enablement, new provider
adapters, scheduler persistence, provider retry/backoff, automatic
ingestion, backtest GUI data-source repair, strategy selection/allocation,
portfolio/P&L durability, broad GUI redesign, the per-tick staleness gate
(`INTRADAY-MD-FRESHNESS-AUTONOMOUS-01`, left untouched), and **per-symbol
strategy bootstrapping** — this bundle surfaces the
`configured_strategy_id`/`effective_runtime_strategy_id` mismatch (§3) as a
blocking readiness fact but does not implement per-symbol bootstrap
selection to resolve it. Also newly out of scope per §13: any provider/
timeframe-support patch needed to make `1h` assignments passable — that is
future work, not this bundle's job to build.

---

## Final contract summary (binding for Phases B–E) — CORRECTED

- **Evaluator module**: `core-rs/crates/mqk-daemon/src/daily_data_readiness.rs`, pure + DB-bounded (`Option<&PgPool>`), additive alongside `market_data_freshness.rs`.
- **Strategy metadata seam**: `StrategyMeta` (or equivalent plugin-owned metadata) gains `StrategyDataRequirements { minimum_completed_bars }`, populated per §5 table, bound in `mqk-strategy` itself — no daemon-side hardcoded lookup. Unknown → `strategy_requirement_unknown`.
- **Assignment source**: `build_multi_symbol_runtime_config_from_env()` (the same call the execution loop makes), not the symbols-only resolver. Legacy single-symbol fallback preserved. Empty/`Err` → `required_assignments_missing`.
- **Identity**: `(symbol, timeframe, configured_strategy_id, effective_runtime_strategy_id, required_history_bars)`. Mismatch between configured/effective strategy id → `runtime_strategy_assignment_mismatch`, blocked.
- **Calendar**: new typed `MarketSessionSchedule` seam (§6a) composed over `MarketCalendarProvider`, with its own coverage-window fail-closed check (provider itself does not fail closed out-of-range). `Unknown`/`OutOfRange` → `calendar_unavailable`, blocked.
- **Timestamps**: `end_ts` is bar-start; daily expectation keyed by calendar trading date + proven provider daily label (verify, don't assume); intraday expectation from a session-open-anchored (09:30 ET) grid, not epoch-hour alignment.
- **Grace**: `effective_grace_seconds = min(configured_grace_seconds[default 900], timeframe.duration_secs())`, both surfaced.
- **Clock skew**: `effective_future_skew_seconds = min(configured_future_skew_seconds[default 300], 60, timeframe.duration_secs())`, both surfaced.
- **Continuity**: full session-anchored proof or explicit `unsupported_intraday_continuity` block per timeframe — never a predeclared PARTIAL, never count-only.
- **Provenance**: every bar in the bounded window must be a known, enabled, capability-matching (asset-class + timeframe) provider — asset class resolved via `mqk_md::instrument_registry::TrackedInstrument::trading_asset_class()`, never defaulted to equity.
- **Provider/timeframe reality**: only `1D`/`1m`/`5m` are currently servable; `1h` assignments (`mean_reversion`, `volatility_breakout`) block as `provider_capability_mismatch` until a separate future patch, honestly.
- **Route**: `GET /api/v1/market-data/readiness`, read-only, public-mounted; preflight/autonomous-readiness/ingest-plan extended to also surface `configured_strategy_id`.
- **Start gate**: `AppState::start_execution_runtime`, inserted after native-strategy bootstrap/assignment resolution but **before** `db_pool()?` (moved earlier than originally contracted), evaluator itself producing `db_unavailable`. Legacy advisory gate stays where it is.
- **Applicability**: `deployment_mode()==Paper && strategy_market_data_source()==ExternalSignalIngestion` — unchanged, not hardcoded to `BrokerKind::Alpaca`.
- **Durable evidence**: `sys_autonomous_session_events`, pre-run evaluation event + linked post-run event (two rows, no UPDATE path), a `ready` verdict blocked if its own evidence write fails, blocked verdicts return after the (not-required-to-succeed) evidence attempt, GET never persists, DB-unavailable cases honestly report `evidence_persisted: false`. No new migration.
- **Backward compatibility**: zero changes to `market_data_freshness.rs`'s existing evaluator, its dedup identity, or any test enumerated in §15.
