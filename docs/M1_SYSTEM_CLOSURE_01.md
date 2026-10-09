# M1 system closure — `V4-M1-SYSTEM-CLOSURE-CANDIDATE-READY-01` (2026-10-04, a Sunday)

Baseline `fac225922d3b46bb842bea22f2c9676f7cfb5b58` (= origin/main, GitHub CI #623, run `37222460113`, SUCCESS 6/6). Local commits, not pushed. No economic attempt, holdout untouched, no Paper/Live mutation, no real-market evidence claimed. The follow-on correction controller (`V4-M1-SYSTEM-CLOSURE-CORRECTION-PLUS-BATCH03-PREPARATION-01`) adds the three corrections recorded under "Correction 01" below and a prospective Batch 03 declaration that has NOT been executed (`docs/research/M1_BATCH03_PREDECLARATION.md`).

## Verdict

`M1_SYSTEM_CANDIDATE_READY = true` as of the capital-fraction daemon closure (Phase B of `V4-BATCH03-DISCOVERY-THEN-CAPITAL-FRACTION-DAEMON-CLOSURE-01`, 2026-10-05; see Finding E). Before it, the value was `false` for exactly one reason: Finding E. Every candidate-independent defect found is fixed and proven. M1 stays `M1_BLOCKED` (no qualified candidate, M1.9 deployment, M1.10 real sessions). The value was held `false` while the independent-review correction (`V4-BATCH03-CAPFRAC-INDEPENDENT-REVIEW-CORRECTION-01`: whole-tick atomic durable commit, cap-vs-durable-Q disposition, direct env-path tests, fresh-DB regression isolation) was open, and is `true` again now that it is closed.

## Closure matrix (category = exactly one)

| ID | Requirement | Authority / seam | Status | Needs |
|---|---|---|---|---|
| M1.1-M1.6 | Research→Promotion, Paper deploy path, provider gates, risk/OMS/accounting, reconcile, runtime ownership | manifest, code/tests | ALREADY_CORRECT_PROVE (no contradiction found; Promotion route re-proven 33/33 DB tests incl. real research pipeline) | none |
| M1.7, M1.8 | genuine trade / no-trade lifecycle | accepted frozen evidence | ALREADY_CORRECT_PROVE (frozen) | none |
| A | exact required stress scenario for a capital-fraction candidate | registered trial identity → Promotion | FIX_NOW: fixed + proven (`3ba296c7`) | none |
| B | US-equity calendar authorities | `calendar.rs` vs `sessions.rs` | FIX_NOW: fixed + proven (`7e69b9b7`) | none |
| C | `1D` vs `1Day` | `require_bars_match_manifest` | FIX_NOW: granularity check fixed (`2ab0aa41`); trial identity of FUTURE trials made label-independent by Correction 01 (`b1cebdfb`) | none |
| D | parallel fixture race | `scenario_scan_canonical_config_binding_01` | FIX_NOW: fixed + proven (`e78806b6`) | none |
| D2 | env race in autofresh scheduler tests | `scenario_market_data_autofresh_required_universe_01` | FIX_NOW: fixed + proven (`2101f410`) | none |
| M1.10-machinery | derived 10/5 count | none existed | FIX_NOW: `soak_ledger` (`f825e994`) + runbook; bound to the deployed identity by Correction 01 (`24500e17`) | none |
| E | daemon dispatch of capital-fraction strategies | `mqk-runtime::capital_fraction_host` routed by the daemon host pool | FIX_NOW: fixed + proven (`fbf2a362`, `ae93f170`, `6ad76924`) | none (M1.9 prerequisite: verify migration 0091 on the real Paper DB) |
| M1.9 candidate deployment | a promoted candidate deployed to Paper | - | BLOCKED_NO_CANDIDATE | candidate |
| M1.9 market-hours observation | deployed runtime under an open session | - | WAITING_MARKET_SESSION | market |
| M1.10 | 10 countable / 5 consecutive clean real sessions | `soak_ledger`, runbook `docs/runbooks/m1_10_finite_validation.md` | WAITING_MARKET_SESSION (after a candidate) | candidate + 10 sessions |
| host sleep / Modern Standby | unattended-session precondition | - | OPERATOR_ACTION_REQUIRED before M1.10 | operator |
| unattended pre-open start result, `DAEMON-EXIT-20260824` recurrence | real evidence | - | WAITING_MARKET_SESSION | market |
| reboot/outage, B2 restore, LiveShadow, Live | G2.8 historical list | - | OUTSIDE_M1 (destructive or later milestones; not performed) | - |

## Known findings

**A. Stress authority.** The campaign predeclaration was not a production authority; Promotion accepted any consistent strictly-smaller fraction. Now the registered trial identity may carry `signal_source.stress_contract` (schema `p7a_p7b_stress_contract_v1`: `scenario_id`, `allocation_fraction_bps`, `stress_execution_slippage_bps`, `stress_execution_volatility_mult_bps`, `max_drawdown_ceiling_bps`; every behavior-bearing P7A/P7B stress input, census-verified; the superseded 2-key shape is refused (the 2-key shape described here was widened to the full 6-key contract by Correction 01 below); execution pricing must be at least the baseline and strictly worse in one input) (absent for every historical trial, so no id changes; the Batch 02 runner registers it only on an explicit `register_stress_contract: true`). `mqk-promotion` reads it from the durable registry and `verify_registered_stress_contract` refuses a capital-fraction candidate whose hash-verified P7A/P7B echo differs from it in ANY single field, or that has none; a fixed-quantity candidate must carry none. The daemon route calls it before `evaluate_promotion`. No universal fraction is chosen. Proof: Python 3 new test groups, Rust `cfsb02a-f` + wiring guard; mutations killed (per-field: scenario, fraction, slippage, volatility, ceiling comparisons; reader schema/arity; echo bps exactness; registration adversity/schema/ceiling checks; runner field plumbing), plus the earlier set: stress-bps check dropped, scenario check dropped, missing contract accepted, fixed+contract accepted, reader ignores contract, reader drops the strictly-below check (6/6), plus the Python identity/opt-in tests. Batch 02 trial ids are unchanged (predeclaration/erratum/registration tests green). Closed historical Batch 02 candidates are not promotable under this rule (they carry no contract); they are rejected anyway.

**B. Calendar.** Callers: `NyseWeekdaysProvider` (Paper) and the system/session surfaces use `calendar.rs`; daily research uses `sessions.rs` (v1, 2016-2026, bound into H1/H2 identity, untouched). Defects fixed in `calendar.rs`: missing closure 2025-01-09, false closure 2027-12-31 (NYSE publishes no Friday observance for Saturday 2028-01-01), missing early closes 2023-07-03 / 2025-07-03 / 2028-07-03 (NYSE page lists 2028-07-03), and weekday extrapolation of the wall-clock classifiers outside 2023-2028. Wall-clock classifiers now report `closed` outside the table; gap detection keeps expecting weekday bars there (a missing bar surfaces rather than being excused). Parity test pins both authorities on every shared date 2023-2026, the published 2027-2028 closures, early closes, weekends, year boundaries and coverage edges; 5/5 mutations killed; daemon lib 1121/0 and the calendar-dependent daemon binaries green.

**C. `1D`/`1Day`.** Internal label `1D`, Alpaca transport label `1Day`; for HISTORICAL trials the declared label stays identity-bearing (`1D` and `1Day` manifests have different identity fragments, preserving every historical trial id); this is superseded for FUTURE trials by Correction 01 (IR-SYS-01), which was wrongly left unresolved here. The granularity check fired only for the literal `1D` and tested UTC midnight, so real ET-midnight daily bars were falsely rejected under `1D` while hourly bars passed under the real label `1Day`. Both daily labels now select the check; a day boundary is midnight UTC or America/New_York. `1H`, `60Min`, `5Min`, `1day` and unknown labels never become daily (unchanged, not rejected here). Rust replay sites already accept exactly `1D|1Day` and refuse the rest. 3/3 mutations killed.

**D. Fixture race.** Per-fixture unique directory: 25/25 failures at `--test-threads=4` before, 0/25 after. A second, independent race (shared process env across 19 async + 3 sync tests of the autofresh scheduler file; failed 3 of 4 local runs) is fixed by one tokio mutex; 0/45 failures after.

**E. Capital-fraction Paper dispatch (hard stop).** With `MQK_STRATEGY_SIZING_POLICY` selecting the capital-fraction policy the daemon builds a `DurableStateRequired` registry; every stateless seam (`instantiate_verified`: bootstrap, host pool, promotion identity, dry run) refuses it, so no capital-fraction strategy can run. `mqk-runtime::capital_fraction_host` (persist-before-act, recover from `sys_strategy_held_sizing_state`) exists and is tested but has no daemon caller; `EffectiveRuntimeBinding` readers (readiness, coverage, outcome, bar driver) derive identity from the bootstrap and would also need the contract path. This was recorded as deferred in G2.15; it is a multi-seam runtime change (not an ordinary defect fix) and therefore is not attempted here. Decision required (smallest): authorize a dedicated daemon-wiring controller for capital-fraction dispatch **or** choose the historical fixed-quantity sizing for the next campaign. Until then the system fails closed (no capital-fraction order can be created).

**E RESOLVED (2026-10-05).** The operator authorized the dedicated controller. The daemon host pool now holds a typed host per `(symbol, strategy_id, timeframe)`; capital-fraction bindings use `CapitalFractionRuntimeHost` (recover once before the start barrier, persist held sizing before the result is returned, one deployment id per binding), fixed-quantity bindings stay stateless, and Promotion Gate 3b/risk/halt/reconcile/OMS are unchanged. The daemon bar windows also now carry true OHLCV (close-only bars had starved ATR/range engines). Proof: 12 DB-backed tests and 13/13 mutations killed; see `docs/CURRENT_MISSION.md` §-34 for residuals. The sentence above is historical.

## M1.9 candidate-independent matrix (read-only, off-market; no secrets)

| Item | Status |
|---|---|
| Paper DB identity (`mqk-paper-postgres`, `miniquantdesk_paper`, port 5440; session forced read-only) | VERIFIED_NOW |
| Config/mode: `MQK_DAEMON_DEPLOYMENT_MODE=paper`; no live-routing flag; no `MQK_ALLOW_PROVIDER_API_CALLS` | VERIFIED_NOW |
| Live disabled / untouched; daemon not running | VERIFIED_NOW |
| Alpaca Paper adapter: mode-derived endpoint pairing | VERIFIED by code/tests; runtime observation WAITING_MARKET_SESSION |
| Provider authority / session-aware freshness (Sunday requires Friday 2026-10-02, never weekend bars) | VERIFIED_NOW |
| Scheduler: `MiniQuantDesk-Paper-Preopen-Startup` Ready, Mon-Fri 02:00 HST (08:00 ET), `Start-MiniQuantDesk.ps1 -Mode Paper -Scheduled`; older soak tasks Disabled | VERIFIED_NOW (registration); unattended start WAITING_MARKET_SESSION |
| Risk/arm/reconcile: `DISARMED` (`InboundContinuityUnproven`, 2026-09-25), `sys_risk_block_state` not blocked, reconcile `ok` 0 mismatches | VERIFIED_NOW (truthful; re-arm after cursor proof at deployment: WAITING_CANDIDATE) |
| OMS: 9 ACKED + 3 SENT historical outbox rows; 8 unapplied inbox rows all from May-June 2026 test-era runs; 2 stale `RUNNING` run rows (2026-06-23, 2026-07-09), latest run 2026-09-25 `HALTED`; no active run | OBSERVED, historical residue, no active-run impact |
| Promotion enforcement + no-default deployment: 0 promotion transitions; registry has 41 rows (38 enabled, 1 genuine `intraday_scalper`); `.env.local` selects AAPL/5m target qty 3; Gate 3b refuses without exact `active_paper` | VERIFIED_NOW: nothing can create a Paper outbox order |
| Paper DB migrations: applied through 76, repo head 0091 | WAITING_CANDIDATE (apply through the canonical boot path at deployment) |
| Candidate deployment identity; market-hours runtime proof | WAITING_CANDIDATE / WAITING_MARKET_SESSION |

Status: `M1.9_PREDEPLOYMENT_READY` for every item that can exist without a candidate; M1.9 itself stays OPEN.

## M1.10

No code tallied the count before this controller; it was a runbook convention. `soak_ledger` now derives it (rules and the capture checklist in `docs/runbooks/m1_10_finite_validation.md`). Sunday, weekends, holidays, startup-only days and days before a valid promoted deployment never count; a repair changes the accepted SHA and restarts the count. Status: `M1.10_READY_TO_START_AFTER_VALID_DEPLOYMENT`; not started, not counted today.

## Correction 01 (`V4-M1-SYSTEM-CLOSURE-CORRECTION-PLUS-BATCH03-PREPARATION-01`, 2026-10-04)

Independent review found three residual defects. Each is one invariant and one commit; none changes a historical trial id.

**IR-SYS-01 - canonical daily timeframe identity (`b1cebdfb`).** A `1D` manifest and a `1Day` manifest of the same bars produced two different trial ids, so one hypothesis could be registered twice under two identities. A declaration may now opt in with `data.timeframe_identity = "canonical_semantic_v1"`; the bars provenance then binds the canonical daily label `1D` (the raw transport label stays in the manifest as provenance) and unknown labels are refused. The opt-in is explicit and versioned: every Batch 01/02 declaration is untouched, so their trial ids are byte-identical.

**IR-SYS-02 - full stress contract (`b9521471`).** The registered contract bound only scenario and fraction, so execution slippage, execution volatility and the drawdown ceiling could still be chosen after results. The contract is now the six-key `p7a_p7b_stress_contract_v1` (`schema`, `scenario_id`, `allocation_fraction_bps`, `stress_execution_slippage_bps`, `stress_execution_volatility_mult_bps`, `max_drawdown_ceiling_bps`), bound into `signal_source.stress_contract` (hence into the trial id), strict-read by the registry reader, adversity-checked at registration (allocation strictly below baseline; execution pricing at least the baseline and strictly worse in one input), and compared field by field at Promotion. The superseded 2-key shape is refused. The DSR/PBO sensitivity knobs belong to other gauntlet scenarios and are outside the P7A/P7B census. The `mqk-daemon` crate was NOT compiled in that window: the Promotion route signature was left unchanged, so no daemon source needed editing.

**IR-SYS-03 - M1.10 deployment identity (`24500e17`).** A soak session record carried no deployment identity, so a session of any strategy, symbol, timeframe or runtime could count. Records now carry a `DeploymentIdentity` that must equal the policy's: a mismatch is `WrongDeployment`, two same-date records with different identities exclude the date, and an incomplete policy identity counts nothing. `LedgerPolicy::m1_10` is unchanged otherwise.

Second sweep for this correction (each item FIXED+PROVEN or ALREADY CORRECT+PROVEN): duplicate authority (one canonical-timeframe function, one stress reader, one ledger identity type); fail-open (absent/unknown/short contract or identity refuses); unknown-label fallback (refused, never defaulted to daily); caller-controlled expected values (the expected stress comes from the durable registry, not the candidate); identity omission (every field is in the id; dropping any one moves it or is refused); historical identity mutation (none: opt-in only, Batch 02 ids pinned); result-defined identity (no result value enters any identity); bool-only authority (the gate checks the contract fields, not a flag).

## Second adversarial sweep (every item FIXED+PROVEN, ALREADY CORRECT+PROVEN, or BLOCKED)

Unpromoted/default strategy trading: ALREADY CORRECT (0 promotions; gate tests). Research-only identity bypass, strategy-id-only match, fixed-quantity benchmark authorizing capital-fraction, missing benchmark/stress authority fallback: ALREADY CORRECT + new gate. Wrong stress scenario authorizing Promotion, candidate choosing its stress after results, caller-invented stress: FIXED (registry-sourced, registered before the first attempt). Calendar disagreement Research/Backtest/Paper: FIXED (parity test); Sunday as outage: ALREADY CORRECT + new test; holiday/early close/outdated coverage: FIXED. 1D/1Day identity/unknown alias: FIXED/ALREADY CORRECT. Flaky tests hiding defects: FIXED (two races). Local config selecting an unpromoted strategy: ALREADY CORRECT (gate). Paper active without `active_paper`: ALREADY CORRECT. Live routing: ALREADY CORRECT, untouched. Docs claiming Batch 02 not pushed: FIXED here. M1 complete before 10/5; historical sessions counted after a repair: ALREADY CORRECT + `soak_ledger` (`WrongCodeSha`). Silent Batch 03; holdout consumed: NO. Capital-fraction candidate tradable in Paper: BLOCKED (E).

## Post-Discovery update (2026-10-08)

`M1_SYSTEM_CANDIDATE_READY = true` is re-confirmed for non-monthly candidates. Two candidate-independent items were found and handled by `V4-M1-POST-DISCOVERY-CANDIDATE-AUTHORITY-AND-OOS-READINESS-01` (`docs/research/M1_POST_DISCOVERY_CANDIDATE_AUTHORITY.md`): the M1.10 ledger now counts Paper sessions through 2028 (it ended 2026-12-31), and the capital-fraction store/restart DB proofs run in the CI DB lane. The monthly engines (F01/F05/F08/F10) remain fail-closed flat for bars on/after 2026-12-31 until the operator authorizes a calendar-contract migration (OD-6).

## KISS campaign update (2026-10-09)

E1 (monthly/calendar engines fail closed flat from 2026-12-31) is resolved at the helper level by the `CalendarContract` seam and the `us_equity_regular_sessions_v2` contract (`docs/research/M1_KISS_EXT032_CLOSEOUT.md` C1); registered v2-bound siblings of those engines remain DEFERRED (each is a new strategy identity). The existing engines and their fingerprints are unchanged (96/96 identical to the baseline build).
