# ASSET-CORE-05G — Current Session Routing Audit

Patch ID: `ASSET-CORE-05G-CURRENT-SESSION-ROUTING-AUDIT-01` (part of
`ASSET-CORE-05-PER-INSTRUMENT-SESSION-ROUTING-01-COMBINED`).

Docs-only. No code changed by this document.

## 0. Patch-ID collision note (read first)

The mission that produced this bundle proposed phase IDs
`ASSET-CORE-05D`/`05E`/`05F`/`05G` for this work. Current-repo evidence
(`git log`, `MiniQuantDesk_Master_Patch_Ledger_v2.md`) shows those four
letters are **already assigned** to a different, already-closed
sub-lineage:

- `ASSET-CORE-05D-EQUITY-SESSION-V2-CUTOVER-SCAFFOLD-01-COMBINED` — `CLOSED_LOCAL / PARTIAL`
- `ASSET-CORE-05E-EQUITY-SESSION-V2-ACTIVE-CUTOVER-HOOK-01-COMBINED` — `CLOSED_LOCAL / PARTIAL`
- `ASSET-CORE-05F-V2-EQUITY-ACTIVE-PROOF-RUNBOOK-COLLECTOR-01-COMBINED` — `CLOSED_LOCAL / PARTIAL`

Those three patches are about the equity-only production runtime
session-source cutover (`MQK_RUNTIME_SESSION_SOURCE`), which is an
unrelated concern from this bundle's per-instrument routing/model scope.
Reusing their letters would corrupt the ledger's truth record (two
different meanings under one ID) and violate this repo's audit-truth
rules (deterministic, non-colliding identifiers; repo state is
authoritative over any prior planning document).

Per `.claude/rules/audit_repo_truth_rules.md` ("repo state is
authoritative... if a doc claims something is done, verify... if memory
contradicts the current file state, trust the file"), this bundle
renumbers to the next free letters instead:

| Original (planning doc) | Used in this bundle | Reason |
|---|---|---|
| `ASSET-CORE-05D-CURRENT-SESSION-ROUTING-AUDIT-01` | `ASSET-CORE-05G-CURRENT-SESSION-ROUTING-AUDIT-01` | `05D` taken |
| `ASSET-CORE-05E-PURE-INSTRUMENT-SESSION-ROUTER-01` | `ASSET-CORE-05H-PURE-INSTRUMENT-SESSION-ROUTER-01` | `05E` taken |
| `ASSET-CORE-05F-SESSION-ROUTING-PARITY-TESTS-01` | `ASSET-CORE-05I-SESSION-ROUTING-PARITY-TESTS-01` | `05F` taken |
| `ASSET-CORE-05G-CLOSURE-AND-ROADMAP-RECONCILE-01` | `ASSET-CORE-05J-CLOSURE-AND-ROADMAP-RECONCILE-01` | `05G` now used above |

`05A`-`05F` and the unlettered `ASSET-CORE-05-MARKET-CALENDAR-GENERALIZE-01-COMBINED`
all remain untouched by this bundle. `05G`-`05J` were confirmed free via
`rg -n "^### ASSET-CORE-05" MiniQuantDesk_Master_Patch_Ledger_v2.md`
before use. All other mission content (scope, safety rules, phase order,
file allowlists) is unchanged — only the ID labels differ from the
originating planning document.

## 1. Current HEAD and prerequisite commits

`git log --oneline -120` at the start of this bundle shows `HEAD` at
`65ac8956` ("docs: decide registry v2 BTC/USD enablement
(decision-only)"), a descendant of the expected pre-flight commit. No
tracked files were dirty; no files were staged.

Relevant prior commits found in `git log` (subject lines, oldest to
newest of this lineage):

- `37a64404` daemon: add multi-asset session classification seam (`ASSET-CORE-05A`)
- `880d8e1f` docs: record session classification seam closure (`ASSET-CORE-05A` closure)
- `0f25cda9` daemon: strengthen equity calendar session profiles (`ASSET-CORE-05B-COMBINED`)
- `8bb015f7` calendar: add session profile diagnostics (`ASSET-CORE-05-MARKET-CALENDAR-GENERALIZE-01-COMBINED`)
- `bfc9a413` daemon: surface instrument session profiles (`ASSET-CORE-05B-INSTRUMENT-SESSION-STATUS-01-COMBINED`)
- `9d0a4797` daemon: add session parity shadow status (`ASSET-CORE-05C-SESSION-PARITY-STATUS-SHADOW-01-COMBINED`)
- `aeae9315` daemon: scaffold v2 equity session source (`ASSET-CORE-05D-EQUITY-SESSION-V2-CUTOVER-SCAFFOLD-01-COMBINED`)
- `1b5d7233` daemon: add v2 equity session active hook (`ASSET-CORE-05E-EQUITY-SESSION-V2-ACTIVE-CUTOVER-HOOK-01-COMBINED`)
- `c4c25cda` docs: add v2 equity session proof collector (`ASSET-CORE-05F-V2-EQUITY-ACTIVE-PROOF-RUNBOOK-COLLECTOR-01-COMBINED`)

Ledger status line (`docs/specs/roadmap_completion_reconcile_01.md`,
`ASSET-CORE-05` row): `PARTIAL` (~35%), category
`Production-consumption-open`.

## 2. Existing session model surfaces

- `MarketCalendarProvider` trait + `MarketSessionState`/`MarketSessionTruth`
  (`core-rs/crates/mqk-daemon/src/state/market_calendar.rs`) — the real,
  production-wired equity session truth. Implementations:
  `NyseWeekdaysProvider` (heuristic, DST/holiday/early-close aware, the
  only provider actually consulted by the live autonomous session gate),
  `FixedWindowOverrideProvider` (operator UTC HH:MM override), and
  `ExchangeSourcedCalendarProvider` (fail-closed / fallback-to-static
  seam for a future authoritative exchange feed — no such feed is wired
  today).
- `MarketVenueSessionKind` (`ASSET-CORE-05A`) — asset-class-agnostic
  session-shape enum (`Closed`/`PreMarket`/`Regular`/`PostMarket`/
  `Extended`/`Overnight`/`Continuous`/`Unknown`), with
  `MarketSessionState::to_venue_kind()` mapping the real equity state
  onto it.
- `MarketSessionProfile` (`ASSET-CORE-05A`) — names the four repo-native
  profiles: `equity_us_regular` (`implementation_status:
  "implemented_current"`), `crypto_continuous`, `futures_globex`,
  `forex_24x5` (all three non-equity profiles `"model_only"`).
  `supported_session_profiles()` returns all four, deterministically
  ordered.
- Pure model-only classifiers (`ASSET-CORE-05A`, same file):
  `classify_equity_us_regular_session`, `classify_crypto_continuous_session`,
  `classify_forex_weekday_continuous_session`,
  `classify_futures_globex_session` (caller-supplied
  `FuturesSessionWindows`, no real CME calendar).
- **`resolve_session_profile_for_instrument_metadata(asset_class,
  instrument_kind) -> SessionProfileResolution`** (`ASSET-CORE-05B`,
  same file) — this is already a pure, deterministic, per-instrument
  session-*profile* router: given an instrument's `asset_class` +
  `instrument_kind` strings, it returns which `MarketSessionProfile`
  would govern that instrument's session truth, with an honest
  `SessionProfileResolutionTruth` (`Active` for real equity/ETF,
  `UnsupportedAssetClass` for crypto/future/futures/forex/option/options,
  `Unknown` for blank or unrecognized input).
- `instrument_session_state_for_profile(profile, now_utc) ->
  (&str, &str)` (private fn, `core-rs/crates/mqk-daemon/src/routes/system.rs`)
  — the second half of routing: given a resolved `MarketSessionProfile`
  and a timestamp, classifies the actual session state (equity via
  `NyseWeekdaysProvider`; crypto/futures/forex via the `ASSET-CORE-05A`
  model-only classifiers, futures using a hardcoded illustrative
  `FuturesSessionWindows` fixture).
- `classify_instrument_session_parity_row(symbol, asset_class,
  instrument_kind, now_utc)` (private fn, same file, `ASSET-CORE-05C`) —
  composes the two functions above and compares the equity/ETF result
  against real `NyseWeekdaysProvider` truth, reporting
  `matched`/`mismatched`/`unknown`/`model_only`/`unsupported_model_only`.

**Finding:** a pure per-instrument session *router* already exists
(`resolve_session_profile_for_instrument_metadata`), and a pure
per-instrument session *state classifier* already exists
(`instrument_session_state_for_profile`), and a pure *parity comparator*
composing both already exists (`classify_instrument_session_parity_row`).
These are NOT new capabilities this bundle needs to invent. See §5-§6
for what is genuinely missing.

## 3. Existing read-only diagnostics

- `GET /api/v1/system/instrument-sessions/status`
  (`system_instrument_sessions_status`, `ASSET-CORE-05B`) — loads the
  configured v1 registry, converts to `InstrumentRegistryV2` in memory,
  validates, and for every instrument reports its resolved
  `session_profile_id`, `profile_truth_state`
  (`production_backed`/`model_only`/`session_profile_unavailable`),
  classified `session_state`, and a hardcoded `trading_uses_this: false`
  per row. Response-level fields `production_cutover_enabled`,
  `trading_uses_session_v2`, `runtime_uses_session_v2` are all hardcoded
  `false`.
- `GET /api/v1/system/instrument-sessions/parity`
  (`system_instrument_sessions_parity`, `ASSET-CORE-05C`) — same
  load/convert/validate path; per-instrument parity verdict against real
  production truth for equity/ETF, `model_only` for non-equity. Response
  carries `shadow_only: true` plus the same three hardcoded-`false`
  cutover fields.
- `instrument_session_shadow_summary_now()` — compact version of the
  parity summary embedded on `/api/v1/system/status` and
  `/api/v1/system/preflight` (`instrument_session_shadow` field,
  `ASSET-CORE-05C`).
- `/api/v1/system/session` — `session_profile`, `session_authority`,
  `session_profile_is_open`, `session_profile_reason_code`,
  `session_profile_message`, `supported_session_profiles`
  (`ASSET-CORE-05-MARKET-CALENDAR-GENERALIZE-01-COMBINED`). Reports the
  currently *active* profile (still always `equity_us_regular` in
  practice) plus the full supported-profile list for operator visibility.
- `RuntimeSessionSourceSummaryResponse` (`runtime_session_source` field
  on `SystemStatusResponse`/`PreflightStatusResponse`, `ASSET-CORE-05D`/
  `05E`) — equity-only v2-vs-legacy cutover-candidate diagnostics.
  Unrelated to per-instrument routing; not touched by this bundle.
- GUI: `Settings / Operations` renders the session-profile panel (from
  `ASSET-CORE-05-MARKET-CALENDAR-GENERALIZE-01-COMBINED`); a separate GUI
  surface exists for the instrument-sessions status/parity data per the
  ledger's `05B`/`05C` entries.

## 4. Current production session gate source

The daemon's actual live/paper autonomous-trading session gate is
`AutonomousSessionSchedule::is_in_session` (`state/session_controller.rs`).
This is a **single global schedule for the whole daemon process** — not
per-instrument, not per-symbol. Two arms:

- `FixedUtcWindow` — operator-configured raw UTC HH:MM window
  (`MQK_SESSION_START_HH_MM`/`MQK_SESSION_STOP_HH_MM`), no calendar.
- `NyseRegularSession` (default) — calls
  `CalendarSpec::NyseWeekdays.classify_market_session(ts) == "regular"`
  directly (equity-only), with an `ASSET-CORE-05E` opt-in hook that
  substitutes the `ASSET-CORE-05D` v2-equity-candidate decision when
  `MQK_RUNTIME_SESSION_SOURCE=v2_equity_active` is explicitly set
  (default remains legacy).

**This gate has no instrument/symbol parameter at all.** There is no
call site anywhere in the runtime/orchestrator/strategy path that asks
"is *this instrument* in session" — only "is the daemon's one global
session open right now". This is the deepest reason true per-instrument
production session routing is not a small closure: it requires a new
parameter threading through the strategy-signal/order-submission path
(Gate 0, orchestrator dispatch), which is explicitly forbidden territory
for this bundle (`mqk-execution/*`, `mqk-runtime/*`, `mqk-risk/*`,
`mqk-portfolio/*`, `mqk-broker-*/*`, `config/*`, DB migrations, strategy
code are all listed as forbidden files unless the operator is asked
first).

## 5. Current per-instrument/session-v2 model source

`InstrumentRegistryV2`/`InstrumentDefinitionV2`
(`core-rs/crates/mqk-md/src/instrument_registry_v2.rs`, `ASSET-CORE-01B`)
is the only per-instrument metadata model with an `asset_class` field
rich enough to drive session-profile routing. `CANONICAL_ASSET_CLASSES_V2
= ["equity", "option", "future", "crypto", "forex", "rate"]`. This
registry remains, per `ASSET-CORE-01`'s own closure boundary, **not
production trading truth** — the daemon converts the real v1
`equities.json` file to v2 in memory on each request (`convert_v1_registry_to_v2`)
purely for these diagnostic routes; nothing writes or reads a standalone
v2 file in production.

## 6. Exact gap: what remains unwired

Cross-referencing §2 against `CANONICAL_ASSET_CLASSES_V2` (§5) surfaces
one concrete, narrow, honest gap in the existing router:

**`resolve_session_profile_for_instrument_metadata` has no explicit
`"rate"` arm.** `"rate"` is a valid `CANONICAL_ASSET_CLASSES_V2` member
(fixed-income/rates instruments, `ASSET-CORE-01E`), but the resolver's
match statement only names `"equity"`, `"crypto"`, `"future"`/`"futures"`,
`"forex"`, `"option"`/`"options"`, falling through to the generic `_ =>
Unknown` arm with `reason: "unrecognized asset_class"` for anything else
— including `"rate"`. That reason text is misleading for `"rate"`
specifically: it is not an unrecognized/garbage string, it is a known,
schema-valid asset class that simply has no session-profile shape
defined yet. This is the one place where the router's honesty contract
(distinguish "recognized-but-unsupported" from "truly unknown") is
currently violated for a class the schema itself already recognizes.

Beyond that narrow gap, the two "routing" pieces
(`resolve_session_profile_for_instrument_metadata` +
`instrument_session_state_for_profile`) are not currently composed into
a single reusable, independently-testable pure function — each route
handler in `routes/system.rs` calls both inline. A single composed
helper would remove that duplication and give per-asset-class parity
tests a stable one-call target, consistent with this repo's existing
"reuse rather than duplicate" convention (cited repeatedly across
`ASSET-CORE-04*`/`ASSET-CORE-05*` ledger entries).

**Everything else named in the original mission as "the gap"
(true per-instrument production session *routing* — i.e., production
admission/trading actually consulting per-instrument session profiles;
authoritative non-equity calendars; any non-equity trading/admission
use) is real and remains open, but is architecturally a production
runtime change (§4) and/or a live-data/authoritative-calendar
acquisition problem — neither is safely closable within this bundle's
hard safety rules (no runtime/execution/risk/broker changes, no network
calls, no config/enablement changes).**

## 7. Safe closure target for this bundle

1. Close the `"rate"` gap in `resolve_session_profile_for_instrument_metadata`
   (or a clearly-documented sibling) — recognized, schema-valid,
   model-only/unsupported, never `Unknown`/"unrecognized" for that one
   string.
2. Add one composed, reusable, pure per-instrument session-routing
   helper (asset_class + instrument_kind + timestamp -> resolved profile
   + classified session state in one call), reusing
   `resolve_session_profile_for_instrument_metadata` and the existing
   classifiers rather than duplicating their logic.
3. Add focused unit/parity tests proving every canonical v2 asset class
   routes to its documented profile, `"rate"` is explicit and fail-closed/
   model-only (not conflated with truly-unknown strings), enabled/disabled
   status never changes the routing decision, and the composed helper's
   output for equity/ETF matches the real production
   `NyseWeekdaysProvider` state at fixed timestamps — extending, not
   replacing, `ASSET-CORE-05C`'s existing parity proof pattern.
4. Reconcile `MiniQuantDesk_Master_Patch_Ledger_v2.md` and the two
   roadmap docs with an honest verdict (expected: still `PARTIAL /
   PRODUCTION-CONSUMPTION-OPEN` — see §8).

## 8. Explicit non-goals

- No production cutover of any kind. `production_cutover_enabled`,
  `runtime_uses_session_v2`, and `trading_uses_session_v2` remain `false`
  on every response this bundle touches or adds.
- No change to `AutonomousSessionSchedule::is_in_session` or any other
  live/paper trading admission behavior.
- No non-equity (crypto/futures/options/forex/rates) trading enablement.
  `BTC/USD.enabled` and every other non-equity `enabled`/
  `paper_trading_enabled`/`live_trading_enabled` flag stay exactly as
  currently committed.
- No config flag change (`config/defaults/base.yaml` or any other config
  file untouched).
- No DB, network, or provider call of any kind.
- No change to `mqk-execution`, `mqk-runtime`, `mqk-risk`,
  `mqk-portfolio`, `mqk-broker-*`, or any strategy code.

`ASSET-CORE-05` production trading/admission behavior is unchanged by
this bundle. Non-equity session profiles remain model-only after this
bundle, exactly as before it. This bundle does not claim non-equity
trading is enabled, and does not claim a production cutover occurred.
