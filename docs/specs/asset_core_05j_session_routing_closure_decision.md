# ASSET-CORE-05J — Session Routing Closure Decision

Patch ID: `ASSET-CORE-05J-CLOSURE-AND-ROADMAP-RECONCILE-01`, closing
`ASSET-CORE-05-PER-INSTRUMENT-SESSION-ROUTING-01-COMBINED` (phases
`05G`→`05H`→`05I`→`05J`; see `asset_core_05g_current_session_routing_audit.md`
§0 for why this bundle does not use the originally-proposed `05D`-`05G`
letters).

Docs-only. No code changed by this document.

## 1. Is `ASSET-CORE-05` fully closed?

**No.** `ASSET-CORE-05` (Market Calendar & Session Provider) remains
`PARTIAL / PRODUCTION-CONSUMPTION-OPEN` overall. This bundle
(`ASSET-CORE-05-PER-INSTRUMENT-SESSION-ROUTING-01-COMBINED`) closes its
own narrow scope — see §2 — but does not close the parent roadmap item.

## 2. Which part is now closed?

`CLOSED_LOCAL / MODEL-AND-PARITY-COMPLETE` for this bundle's own scope:

- The one concrete honesty gap found in the audit (`05G`) —
  `resolve_session_profile_for_instrument_metadata` had no explicit
  `"rate"` arm and silently conflated a schema-valid asset class with
  truly-unrecognized garbage strings — is closed (`05H`).
- A single, composed, reusable, pure per-instrument session-routing
  helper (`route_instrument_session_for_metadata`) now exists, replacing
  the previous pattern of two separate functions manually composed
  inline at each route call site (`05H`).
- The pre-existing duplicate private copy of
  `instrument_session_state_for_profile` was eliminated by relocating it
  to `market_calendar.rs` as the one canonical `pub fn` (`05H`).
- Every canonical `InstrumentRegistryV2` asset class (`equity`, `option`,
  `future`, `crypto`, `forex`, `rate`, plus the ETF `instrument_kind`
  variant of `equity`) is now explicitly tested end-to-end through the
  composed router, including a regression guard
  (`ac05i_10`) that fails loudly if a future canonical class is added
  without teaching the router about it (`05I`).
- Equity/ETF routing is proven, at four distinct fixed timestamps
  (regular-open, holiday, both sides of an early close), to match real
  production `NyseWeekdaysProvider` truth exactly — extending, not
  replacing, `ASSET-CORE-05C`'s existing parity proof pattern (`05I`).
- Non-equity profiles are proven, at multiple timestamps including a
  weekend, to always report `model_only`/`is_model_only()==true` and
  `is_production_backed()==false` — never matching production (`05I`).

## 3. What remains model-only?

Everything non-equity, exactly as before this bundle:

- `crypto_continuous`, `futures_globex`, `forex_24x5` remain
  `implementation_status: "model_only"` — no authoritative exchange
  calendar backs any of them. Futures classification still uses a
  hardcoded illustrative `FuturesSessionWindows` fixture, not a real CME
  (or other) schedule. Forex does not model the real Friday-evening-to-
  Sunday-evening ET rollover. Crypto asserts "always continuous" as a
  concept-level claim, not a per-exchange-downtime-aware truth.
  Fixed-income/`"rate"` instruments have no session-profile shape at all
  (deliberately not invented in this bundle — see `05H`'s "deliberately
  not done").
- The `route_instrument_session_for_metadata` composition itself is not
  called by any route, trading, admission, risk, broker, or runtime
  path. It exists as a pure library function proven correct by its own
  test file, nothing more.

## 4. Does production trading/admission consume session-v2?

**No.** Confirmed by direct source read: `AutonomousSessionSchedule::is_in_session`
(`mqk-daemon/src/state/session_controller.rs`) — the actual live/paper
autonomous-trading session gate — still only consults `FixedUtcWindow` or
`NyseRegularSession` (with the pre-existing, unrelated `ASSET-CORE-05D`/
`05E` opt-in `MQK_RUNTIME_SESSION_SOURCE=v2_equity_active` equity-only
hook, default `legacy`, untouched by this bundle). This gate has no
instrument/symbol parameter — it is not, and cannot become, "per-
instrument" without new architecture in the execution/orchestrator/Gate
0 path, which this bundle's hard safety rules place out of scope.
`route_instrument_session_for_metadata` has zero callers outside its own
test file.

## 5. Are non-equity sessions authoritative production calendars?

**No.** Confirmed unchanged from before this bundle: crypto/futures/forex
classification remains model-only, with no CME holiday table, no FX
market-center calendar, and no crypto exchange-specific downtime
modeled anywhere in the repo.

## 6. Are BTC/USD or ETH/USD enabled?

**No.** `git diff --stat` across all three phases of this bundle
(`05G`→`05H`→`05I`) touched zero files under `config/`. `BTC/USD.enabled`,
`paper_trading_enabled`, and `live_trading_enabled` remain exactly as
committed before this bundle (`false`, per
`REGISTRY-V2-INSTRUMENT-ENABLEMENT-01-BTC-USD-DECISION-01`).

## 7. What changed in code/tests/docs?

- **Code:** `core-rs/crates/mqk-daemon/src/state/market_calendar.rs`
  (new `"rate"` arm; relocated `instrument_session_state_for_profile`;
  new `route_instrument_session_for_metadata` + `InstrumentSessionRoute`),
  `core-rs/crates/mqk-daemon/src/state.rs` (re-export the two new/moved
  symbols), `core-rs/crates/mqk-daemon/src/routes/system.rs` (import swap
  only — two existing call sites now call the relocated `pub fn` instead
  of a private local copy; zero behavior change, proven by unchanged
  11/11 and 15/15 route test results).
- **Tests:** new
  `core-rs/crates/mqk-daemon/tests/scenario_asset_core_05_session_routing_parity_01.rs`
  (11 tests).
- **Docs:** `docs/specs/asset_core_05g_current_session_routing_audit.md`
  (new), this closure decision doc (new),
  `MiniQuantDesk_Master_Patch_Ledger_v2.md` (four new entries: `05G`,
  `05H`, `05I`, and this `05J` entry), plus the roadmap reconciliation
  in `docs/audits/multi_asset_completion_audit.md` and
  `docs/specs/roadmap_completion_reconcile_01.md` accompanying this doc.

## 8. What remains before non-equity trading can use this?

Unchanged from the `05G` audit's §6/§8 and prior `ASSET-CORE-05` ledger
entries — this bundle does not narrow this list, only the model/parity
gap around it:

1. A production per-instrument admission decision point (new
   architecture in the strategy-signal/Gate-0/orchestrator path — the
   current gate is a single global, non-instrument-aware schedule).
2. At least one authoritative, non-heuristic, non-fixture non-equity
   calendar (real CME holiday/session data for futures; a real FX
   market-center calendar for forex; real per-exchange downtime data for
   crypto).
3. An explicit, separate operator decision to route a live production
   gate through per-instrument session truth, plus its own proof
   standard (live wall-clock, paper-only, operator-supervised) — the
   same category of proof `ASSET-CORE-05E`/`05F` already established as
   a precedent for the equity-only cutover, not yet attempted for any
   non-equity instrument.
4. Separately, `CRYPTO-EXEC-01`/`CRYPTO-RISK-01`/`CRYPTO-STRAT-01`
   (broker execution, risk, and strategy code for any non-equity asset
   class) remain `MISSING` per `roadmap_completion_reconcile_01.md` —
   session routing alone does not unblock trading even once closed.

## 9. What next patch is recommended?

No further `ASSET-CORE-05` model/parity work is recommended as a
standalone next step — the safe, closable model/parity scope identified
by `05G`'s audit is now closed. The next patch that would actually move
`ASSET-CORE-05` toward production consumption is a *new*, separately-
scoped and separately-authorized item: a real per-instrument admission
architecture decision (§8 item 1) paired with at least one authoritative
non-equity calendar source (§8 item 2) — both of which are
production/runtime-behavior-changing and explicitly out of scope for a
"safe closure" bundle like this one. Until an operator authorizes that
category of change, `ASSET-CORE-05` should remain `PARTIAL /
PRODUCTION-CONSUMPTION-OPEN` on the roadmap.

## Verdict

**`ASSET-CORE-05-PER-INSTRUMENT-SESSION-ROUTING-01-COMBINED`: `CLOSED_LOCAL
/ MODEL-AND-PARITY-COMPLETE`.** The safe model/parity scope identified by
this bundle's own audit is fully closed, and the remaining
production-use boundary is explicitly and completely split out (§8).

**`ASSET-CORE-05` (parent roadmap item): `PARTIAL /
PRODUCTION-CONSUMPTION-OPEN`**, unchanged in category, percentage
nudged up to reflect the closed model/parity gap (see roadmap doc
updates accompanying this entry).
