# MiniQuantDeskV4 — Full-System Completion Situational Audit

**Audit:** `FULL-SYSTEM-COMPLETION-SITUATIONAL-AUDIT-01` (L2 of `MASTER-LEDGER-CURRENT-TRUTH-AND-FULL-SYSTEM-COMPLETION-AUDIT-01`)
**Audit date:** 2026-08-30
**Mode:** READ-ONLY situational audit. No application code, DB, broker, or trading behavior was modified in this session. No Live trading, no broker orders, no Paper DB mutation, no offsite-backup invocation.
**Branch:** `main` · **HEAD:** `70ed507acfe02ef860b8378b9e5eddb25a36065d` · **origin/main:** identical (verified via `git rev-parse`).
**Predecessor:** `MASTER-LEDGER-CURRENT-TRUTH-CLOSURE-01` (L1), commit `ba7234ae`, which reconciled `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` to this same HEAD immediately before this audit. This audit treats that reconciled ledger as current baseline truth and does not re-derive what L1 already settled (the strategy-identity/promotion-binding wave, the corrected multi-symbol-dispatch-panic-isolation closure, the halt-without-auto-flatten decision, the recovery/offsite-backup proof-truth split).

---

## Domain: Research / Backtest / Promotion (audit domains 1-3)

**Status: CONFIRMED — engineering-closed and independently accepted**, per the ledger's own `RESEARCH_BACKTEST_V1_COMPLETE — CLOSED — INDEPENDENTLY ACCEPTED` verdict (2026-08-28, `RESEARCH-BACKTEST-V1-FINAL-INTEGRATION-AND-ACCEPTANCE-01`) plus the further strategy-identity/promotion-binding wave this audit's L1 pass recorded as `PUSHED-VERIFIED` (`4ef6b643`..`70ed507a`, 2026-08-29). This domain is not re-audited from scratch here per CLAUDE.md §2 ("do not re-audit settled architecture unless current evidence contradicts it") — no such contradiction was found. Summary of what is CONFIRMED at current HEAD:

- Trial/attempt/evaluation-slice identity kept distinct; retries do not manufacture independent trials; winner-only registration is rejected (`CANONICAL_PROMOTION_DECISION`, `CROSS_CANDIDATE_AUTHORITY` — independently reviewed).
- Walk-forward chronology, purge/embargo, and final-holdout preservation are frozen contracts with dedicated negative controls (`OOS_RESEARCH_AUTHORITY`, holdout-reservation/consumption tests).
- DSR/PBO sensitivity and a genuine shuffled/null-control placebo are REQUIRED (non-optional) P9 scenarios — a `not_evaluable` result fails closed rather than silently excluding the check (`DSR_PBO_SENSITIVITY`, `GENUINE_SHUFFLED_PLACEBO`).
- Backtest causal fill timing, execution-model identity, commission/slippage/stress, and P7A/P7B economic replay stress are evidence-bound to the exact evaluated trial (content-hash-authenticated, not merely "latest successful attempt") (`P7A_P7B_REPLAY`, `STRESS_SUITE_AUTHORITY`).
- The production HTTP promotion boundary (`POST /api/v1/strategy/promotions/transition`) persists the exact evidence lineage judged, proven via a raw Postgres readback against a fresh disposable DB plus a mutation-style RED control (wrong lineage identity provably fails) (`DURABLE_PROMOTION_LINEAGE`).
- **New since the 2026-08-28 acceptance** (L1's wave): promotion/backtest/dynamic-selection/stress-suite/robustness-gauntlet evidence is now additionally bound to the strategy's own semantic configuration fingerprint, not merely its name/opaque config id or a `verified_v1` status flag — closing a class of "same label, different behavior" identity gaps. See L1 commit `ba7234ae` for full citation detail; not repeated here.

**What this does NOT mean** (unchanged from the 2026-08-28 verdict, still true at this HEAD): `PROVEN_ALPHA`, promotion-readiness for an arbitrary new strategy, `SHORT-WAVE-03` execution, Paper forward validation, or Live readiness. Each remains a separate, unestablished stage — see Alpha Readiness (§J) and Full-V4 (§F) below.

**Open item carried forward (not closed by this audit, tracked as NOW-lane):** migration `0067_dynamic_selection_plan_candidates_config_identity.sql` exists as a committed artifact at this HEAD (added by the same wave) but this audit found no evidence it has been applied to, or confirmed-absent-from, the running Paper database — that is an operational step, not a code-review one, and is listed in the Master Remaining Ledger below.

---

## Domain: Strategy Framework (audit domain 4)

Researched by a dedicated background agent; adopted here.

- **CONFIRMED** — the plugin registry (`mqk-strategy/src/plugin_registry.rs`) rejects duplicate-name registration with a typed error rather than silently overwriting.
- **CONFIRMED** — short-side trading is real, not a stub: `intraday_scalper.rs` produces genuine negative targets under `allow_short_signals`/`is_short_only`, with a distinct registered identity (`intraday_short_scalper`) and a 13-case test file. **Nuance**: that same test file documents that `StrategyHost` enforces `MultiStrategyNotAllowed` — true *concurrent* long+short dispatch on the same symbol is not yet wired (explicitly deferred to a future patch); "long/short" support today is per-instance, not concurrently multiplexed.
- **CONFIRMED** — panic isolation matches L1's finding exactly, nothing new.
- **CONFIRMED** — the `max_concurrent_symbols` cap still fails closed rather than truncating-and-surfacing (`multi_symbol_config.rs:45-59`), matching the ledger's own still-open `MULTI-SYMBOL-CAP1-TRUNCATE-SURFACE-01` item.
- **Nuance worth carrying forward**: the watchlist-v2-driven multi-symbol *config builder* (`multi_symbol_config.rs`) documents itself as not called from the live tick/loop path, while the multi-symbol *dispatch loop* itself (`tick_strategy_dispatch_multi_symbol_with_bar_facts`) is confirmed called every tick from `loop_runner.rs`. I.e. dispatch-if-given-multiple-symbols is production-wired, but whether the production path is actually fed more than one symbol (vs. a single-symbol env fallback) in practice was not settled — flagged UNKNOWN-NEEDS-PROOF, not a confirmed gap.
- **CONFIRMED** — `mqk-strategy/src/semantic_identity.rs` (the fingerprinting seam L1's wave binds evidence to) is real and present.

## Domain: Data (audit domain 5)

- **CONFIRMED, corrects the mission's implicit framing** — there is no market-data WebSocket streaming in `mqk-md`; historical/intraday bar ingestion is REST-poll only (`alpaca_provider.rs::fetch_bars`, scheduled via `ingest_jobs.rs`). The only live Alpaca WebSocket in the codebase subscribes to `trade_updates` (broker order-execution events, in `state/alpaca_ws_transport.rs`), not price bars. "Streaming ingestion" for market data itself does not exist as a capability — only REST-poll bars plus a separate order-event stream.
- **CONFIRMED** — the staleness gate (`market_data_freshness.rs`, `evaluate_md_freshness_snapshot`/`_status`) is called directly from the production dispatch path, not merely tested; and a genuine N-symbol readiness aggregation exists (`evaluate_md_freshness_status_for_symbols` → `MultiSymbolFreshnessReport`), consumed in `state/lifecycle.rs`.
- **CONFIRMED** — provider provenance is enforced at the registry level: an unregistered/unknown provider string is rejected with a typed error, not silently accepted (`mqk-md/src/provider_registry.rs`).
- **CONFIRMED, real and code-acknowledged gap** — corporate-action / event-risk handling is materially weaker live than in the backtest engine: the backtest engine has a real `ForbidPeriods` policy, but the live/paper path (`routes/execution_order_analysis.rs:330-334`) explicitly self-documents "No earnings calendar feed is connected" and "No pre-event position flattening gate exists" — only an operator-declared static blackout file. This is not manufactured; it's the code's own comment.

## Domain: Execution / OMS (audit domain 6)

- **CONFIRMED** — the OMS state machine (`mqk-execution/src/oms/state_machine.rs`) restricts terminal states to `Filled|Cancelled|Rejected`, transitions driven by explicit `OmsEvent` values. Whether every `OmsEvent` emission site in the codebase originates only from real broker events (never synthesized) was not exhaustively traced — LIKELY, not fully proven negative.
- **CONFIRMED** — `outbox_claim_batch` (`mqk-db/src/orders.rs`) is genuinely atomic: one SQL statement, `FOR UPDATE SKIP LOCKED` CTE + `UPDATE ... RETURNING`, safe under concurrent dispatchers.
- **CONFIRMED** — duplicate-order prevention is a real DB constraint (`idempotency_key` + `ON CONFLICT DO NOTHING`), restart-safe by construction (not in-memory state).
- **CONFIRMED** — Paper/Live isolation is structural, not conventional: a single `deployment_mode` enum gates both the Alpaca base URL and which credential set is used together (`state/broker.rs`), and the in-process "fake fill" broker kind is explicitly refused as not a real execution path.

---

## Domain: Multi-Asset (audit domain 15)

**Status: CONFIRMED equities/ETFs-only in production trading truth; other asset classes are data/economics-layer scaffolding at most.** This matches the ledger's own "Highest-Risk Incomplete Subsystems" line (Options/Futures/Forex ~5%, enum + risk-multiplier stub only, explicitly gated off) and this session's own repo memory (ETF-FOUNDATION-01/ETF-RISK-CLOSURE-01 closed; ASSET-CORE-01 through 05 lineage closed-local but PARTIAL; CRYPTO-DATA-01 series closed-local, data/read-only only). Spot-checked this session:

- No dedicated crypto/futures/options/forex execution crates exist under `core-rs/crates/` — multi-asset support is implemented as an `AssetClass`/economics layer inside `mqk-portfolio` (`dynamic_selection.rs`, `instrument_economics.rs`, `portfolio_economics.rs`), not separate per-asset-class trading paths. CONFIRMED via directory listing.
- Crypto has real local-data provider integration (Kraken/Coinlore, per repo memory) but no live crypto execution path.
- Per CLAUDE.md §21 and the mission's own instruction, this audit does not implement or further investigate future asset classes — this section only characterizes current state for the Full-V4 backlog (§F below).

---

## Domain: Live (audit domain 16)

**Status: CONFIRMED NOT READY, hard-gated by design, re-verified at current HEAD (not merely carried forward from the 2026-08-28 ledger verdict).**

- `live_trust_complete` is hardcoded `False` throughout `research-py`'s TV-03/deployment-parity pipeline, with no code path to set it `True` — confirmed at current HEAD: `research-py/src/mqk_research/contracts.py:452` ("ALWAYS False in current builds; no mechanism to lift"), `research-py/src/mqk_research/deployment/parity.py:124` ("ALWAYS False in current builds; no mechanism to set True"), enforced by a dedicated negative-control test suite (`research-py/tests/test_backtest_parity_provenance_bkt_prov02.py`, `BP-07`) that asserts this stays `False` even when all provenance fields are supplied.
- `buying_power` in the daemon's portfolio summary route is still aliased directly to `cash` rather than derived from Alpaca's real `buying_power`/`daytrading_buying_power` fields — confirmed at current HEAD: `core-rs/crates/mqk-daemon/src/routes/portfolio.rs:440`, `buying_power: Some(cash)`. This is economically meaningful for a margin account and remains an open, real defect (not fabricated truth, since it degrades to `None` rather than a wrong non-`cash` value when no snapshot exists — but the `Some(cash)` value itself is inaccurate whenever margin financing differs from cash-on-hand).
- No live-capital smoke-test tooling was found in this session's (or prior recorded) evidence.
- **This audit did not enable, test, or probe Live in any way** — both findings above are static code reads only, consistent with the mission's Live-safety constraints.

---

## Domain: Risk (audit domain 7)

Researched by a dedicated background agent; this is the single most consequential finding of this audit and was independently spot-verified by this session directly (`mqk-runtime/src/runtime_risk.rs`, `mqk-execution/src/gateway.rs`) before being written up — see the correction committed to `docs/specs/halt_without_auto_flatten_decision.md` (commit `a49ff9cc`) for the halt/flatten-specific implication.

- **CONFIRMED, corrects this audit's own earlier premise** — `mqk-risk::evaluate()` IS used in live production, transitively: `mqk-daemon` → `mqk-runtime` → `mqk-risk`. `RuntimeRiskGate` (`mqk-runtime/src/runtime_risk.rs`) wraps it and is wired as the middle of a three-gate order-submission pipeline (`IntegrityGate` → `RiskGate` → `ReconcileGate`, `mqk-execution/src/gateway.rs::enforce_gates`), constructed per-run in `mqk-daemon/src/state/orchestrator_build.rs`. Every order submission is gated by it.
- **CONFIRMED — deterministic defect: daily-loss and max-drawdown checks are structurally inert against real live losses.** `RuntimeRiskGate::from_run_config` builds one static `RiskInput { equity_micros: initial_equity_micros, .. }` **once, at orchestrator construction time**, and `RiskState`'s `day_start_equity_micros`/`peak_equity_micros` are seeded from that same frozen value. No call site was found that updates `equity_micros` with the run's actual current equity before calling `evaluate_gate_for_request`. Net effect: a real intraday loss or drawdown cannot cause these two checks to fire, because the "current equity" they compare against never moves from the run's starting value. Verified directly: `mqk-runtime/src/runtime_risk.rs:36-49`, `mqk-daemon/src/state/orchestrator_build.rs:288-304` (comment confirms "evaluated once at run-start").
- **CONFIRMED — deterministic defect: reject-storm detection is dead code in production.** `RiskState::record_reject()` is the only mutator that would ever populate a reject-count window; it has zero call sites in `mqk-daemon` or `mqk-runtime` — only in `mqk-risk`'s own unit test. The reject-storm kill switch cannot trigger in the live system regardless of configuration.
- **CONFIRMED — deterministic defect: PDT guard is a hardcoded no-op.** `RuntimeRiskGate::from_run_config` passes `pdt: PdtContext::ok()` unconditionally; `PdtContext::blocked()` exists but is never constructed by daemon/runtime code. The PDT guard cannot deny an order today, independent of `pdt_auto_enabled`.
- **CONFIRMED — inconsistent fail-closed posture within the same config struct**: an absent `daily_loss_limit` fails closed (denial), but an absent `max_drawdown` silently disables that check (`max_drawdown_limit_micros = 0`) rather than failing closed — the two sibling fields do not share a policy.
- **CONFIRMED — real, working controls found elsewhere, independent of the frozen-equity engine**: short-side entry is blocked fail-closed at the signal-classification stage regardless of strategy (`dry_run_strategy.rs`, `ShortEntryConfig::fail_closed`); a separate per-signal exposure/policy gate (`capital_policy/portfolio_risk.rs`, Gate 1g) runs at signal ingestion and fails closed on an unreadable/invalid policy file; and the halt gate (`enforce_deadman_or_halt`) genuinely runs before dispatch on every tick, with every halt/gap path being a full loop exit, not a skip — matches `execution_rules.md`'s "not optional" requirement.

**Why this matters for the mission's finish line:** CLAUDE.md frames this repository as one where "real money will be deployed." A daily-loss-limit and max-drawdown gate that cannot structurally fire during a live run is not a cosmetic gap — it is the two headline capital-preservation controls silently not doing their stated job for the life of any run, Paper or Live. This is listed as a NOW-lane critical-path item below and in the final report's deterministic-defects list, not filed as a routine backlog item.

## Domain: Portfolio / Accounting (audit domain 8)

- **CONFIRMED** — durable, deterministic FIFO lot accounting (`mqk-portfolio/src/accounting.rs::apply_fill`): buys cover shorts before opening long, sells reduce longs before opening short, realized P&L computed per fill, cash adjusted with fee; positions tracked in a `BTreeMap` (deterministic ordering, multi-symbol).
- **CONFIRMED** — fills reach the portfolio only through the inbox (`inbox_insert_deduped_with_identity` / `inbox_mark_applied`), not a direct write from the dispatch/order-submit path; the durable-replay path reuses the exact same duplicate-fill guard the live apply path uses, keyed on OMS per-order applied-event identity.
- **CONFIRMED, architectural nuance worth recording** — `apply_fill` itself has no internal dedup; idempotency is enforced one layer up at the DB/inbox boundary, by design (inbox is the intended dedup authority). This is sound as the accepted architecture, but means the pure accounting function alone provides no safety net if some future caller ever bypassed the inbox.
- **No portfolio-specific open gaps found** beyond the cross-cutting risk-engine gaps above.

## Domain: Reconciliation / Recovery (audit domain 9)

- **CONFIRMED** — the reconcile gate (`ReconcileTruthGate::is_clean`) is one of the three gates checked on every order submission, and fails closed on a lock-acquisition failure rather than passing optimistically.
- **CONFIRMED** — startup is not blind: broker snapshot reconciliation happens as part of orchestrator construction, before the first tick, and fails closed on invalid/ambiguous broker data (non-integer quantities, bad timestamps).
- **CONFIRMED** — `sys_reconcile_status_state`'s "clean reconcile" bar (`status='ok'` and zero mismatched positions/orders/fills/unmatched broker events) is the same bar used both for live order gating and for releasing a stale prior-day autonomous-operation row — one shared definition, not two.
- **CONFIRMED** — the 2026-08-24/25 stale-operation fix (L1) is present, load-bearing, and test-exercised (not dead code) — independently re-confirmed by this agent from a different angle than L1's own git-log-based confirmation.
- **CONFIRMED** — broker position baseline adoption exists as an explicit, operator-triggered repair route (`POST /api/v1/ops/repair/adopt-broker-position-baseline`) — a pre-existing broker position blocking startup reconcile requires operator action, not automatic adoption. This is a scope note (manual-intervention dependency for otherwise-unattended startup), not a defect.
- **LIKELY, not fully traced** — single-instance run leadership/fencing: a leadership-lease mechanism was located (`release_runtime_leadership()` called on every halt/exit path, lease logic present in `mqk-runtime/src/orchestrator.rs`) but the acquire-side fencing-token comparison and its test coverage were not read in full this session — flagged for a follow-up read before treating two-daemon-instance fencing as fully proven.

---

## Domain: Autonomous Paper Operations (audit domain 10)

Researched by a dedicated background agent; findings adopted here after review.

- **CONFIRMED** — Coordinator phase sequence to strategy evaluation is real and gated: `tick_autonomous_daily_coordinator` (`autonomous_daily_coordinator.rs:317-524`) resolves calendar plan → DB presence → runtime-binding config → operation identity → `create_or_recover` → coverage-authority anchor → `dispatch_by_state`, whose preopen/preflight handlers only proceed to `awaiting_open`/`start_retrying` when `daily_data_readiness::evaluate_readiness_with_binding` reports `start_allowed=true` — a strict, independent data-readiness gate that exists regardless of the stale-row fix.
- **CONFIRMED** — the stale-row fix (L1's `eaf9f953`..`5346f90a`) sits directly on the real bar-dispatch call path: `autonomous_completed_bar_task.rs:243` calls `fetch_relevant_open_autonomous_daily_operation` directly, not via a fallback branch. With the fix merged, a fresh run reaching `running` that receives a completed bar can now proceed to strategy dispatch.
- **CONFIRMED — other independent gates still apply post-fix**, any one of which can still block reaching evaluation on a given day: calendar/session-plan resolution, runtime-binding assignment config, `resolve_autonomous_runtime_context`, market-data readiness (`evaluate_readiness_with_binding`), coverage-authority anchor binding, and a deployment-mode/broker gate requiring Paper+Alpaca+`ExternalSignalIngestion`.
- **CONFIRMED — no-signal vs never-evaluated is a real, structural distinction**, not string-matching: `AutonomousDailyOutcomeEvidenceSummary.evaluation_count` is tracked separately from `outbox_count`/`fill_count`, and the outcome classifier distinguishes `NoTradeStrategyEvaluatedNoSignal` (→ `STATE_COMPLETED_NO_TRADE`) from `MissingEvaluationEvidence` (→ `EvidenceBlocked`, fail-closed) — a day that never actually evaluated cannot be misread as a clean no-signal day.
- **CONFIRMED — durable per-day evidence exists**: `sys_autonomous_daily_operations` (`bars_observed`/`bars_dispatched`, no SQL defaults) and `sys_autonomous_daily_bar_dispatches` (per-bar claim table, `claimed/completed/uncertain/failed`, keyed to prevent double-dispatch on restart).
- **CONFIRMED, corrects the pre-L1 ledger** — Discord daily-outcome/degradation alerting is real and wired, not "built but unused" as the older ledger text claimed: `session_controller.rs:399-484` sends `notify_run_status` on `OutcomeFinalized` and `notify_critical_alert` on `OutcomeEvidenceDegraded`, backed by a real `reqwest`-based webhook delivery gated only on `DISCORD_WEBHOOK_URL` being configured.
- **LIKELY, not fully traced** — restart resume of a carried-over position: `current_positions` is derived fresh from the live execution snapshot each tick rather than stale in-process state, and a "BROKER-BASELINE-01" seeding path exists, but the full cold-start call chain was not traced end-to-end this session.

## Domain: Daemon / Control Plane (audit domain 11)

- **CONFIRMED** — spot-checked routes (`/api/v1/reconcile/status`, `/api/v1/autonomous/readiness`, `/api/v1/ops/action`) return real backend state with honest unavailable/not-applicable markers, not stubs — e.g. `reconcile.rs` maps unknown state to `"never_run"` explicitly rather than defaulting silently, and the readiness route returns `truth_state: "not_applicable"` for a non-paper+alpaca daemon instead of fabricating a value.
- **CONFIRMED** — one single armed/halted source of truth (`StateIntegrityGate` wrapping one `Arc<RwLock<IntegrityState>>`), consumed consistently across 14 files with no evidence of a second, divergent flag.
- **CONFIRMED** — dynamic selection (host-switching) is wired directly into the production start path (`lifecycle.rs:632`), not test-only.
- **CONFIRMED** — Paper/Live mode is read from exactly one source (`state.deployment_mode()`, itself backed by one env-var read site) — no ad hoc re-derivation found elsewhere.

## Domain: GUI (audit domain 12)

- **CONFIRMED** — the `truth_state` hard-block pattern from `gui_rules.md` actually holds in code, spot-checked on `ExecutionScreen.tsx` and `PortfolioScreen.tsx`: both block rendering data unless `truthState === null`, and both are stricter than the documented minimum (also blocking on `stale`/`degraded`, not only the fully-unavailable states).
- **CONFIRMED** — operator actions fail closed on non-2xx (`invokeOperatorAction`), with an explicit refusal to silently fall back to a legacy endpoint.
- **Characterization**: a read-only operator console plus a bounded, data-driven action surface (arm/halt/flatten-class actions via `/api/v1/ops/action`), not a full order-entry trading UI — consistent with the mission's instruction not to penalize this domain for cosmetic gaps.
- **No open gaps found** in the screens/module spot-checked.

---

## Domain: Git / CI / Repository Governance (audit domain 14)

Researched by a dedicated background agent this session; findings independently verified as internally consistent (workflow structure, migration count, `gh api` branch-protection call) and adopted here.

- **CONFIRMED** — One workflow, `.github/workflows/ci.yml`, five jobs: `gui-contract` (GUI truth tests + daemon/GUI contract gate + non-empty waivers-doc check), `guards` (unsafe-pattern/migration-governance/ignored-proof/dep-inheritance/toolchain-convergence checks, each with its own mutation-negative proof), `rust` (fmt + `clippy -D warnings` + `cargo test --workspace` against an ephemeral Postgres 16 service), `db-proof` (fresh-Postgres migration bootstrap + narrow DB safety proof set), `windows` (PowerShell guards + fmt/clippy/tests, explicitly no DB lane, documented low-memory posture).
- **CONFIRMED** — Migrations `0001`-`0067` present, sequential, no gaps/duplicates.
- **CONFIRMED** — Targeted secrets grep found no real committed secret (the one AWS-key-shaped hit, `mqk-config/tests/scenario_secrets_excluded.rs:39`, is AWS's own published example placeholder, used inside a test that proves secrets *are* excluded).
- **CONFIRMED, notable gap** — `gh api repos/Hartman25/MiniQuantDeskV4/branches/main/protection` returns HTTP 404 "Branch not protected". **`main` currently has no GitHub branch-protection rule** — no required status checks, no required reviews enforced at the platform level. CI passing is advisory only; nothing at the GitHub-settings layer prevents a direct force-push or unreviewed merge to `main`. Given CLAUDE.md's "real money will be deployed" posture, this is the clearest governance gap found in this audit and is listed as a NOW-lane item below.
- **CONFIRMED** — `docs/specs/testing_strategy.md` documents the test philosophy; CI itself is the executable source of truth for exact reproduction commands. No top-level `CONTRIBUTING.md` exists.

## Domain: Testing (audit domain 13)

Same background agent; adopted here.

- **CONFIRMED** — 520 test files under `core-rs/crates/*/tests/`. `#[ignore]` appears 994 times across 117 files, but this is not undisciplined: `scripts/test/ignored_test_inventory.csv` (743 rows) tracks every one with an explicit reason and a runnable repro command (725 `SAFE_DB_5434` requiring `MQK_DATABASE_URL`, 9 `MANUAL_EXTERNAL`, 8 `SAFE_LOCAL`), and CI's `guards` job (`check_ignored_load_bearing_proofs.sh`) rejects a bare unreasoned `#[ignore]` on a fixed allowlist of promoted-proof files.
- **CONFIRMED** — DB-backed tests run by default in CI (not manual-only) via the `rust` job's ephemeral Postgres service.
- **CONFIRMED** — Genuine negative/adversarial controls exist and were spot-checked directly, not merely claimed: `scenario_lookahead_bias_proof.rs` (5 orthogonal lookahead vectors via a spy strategy) and `scenario_negative_slippage_rejected.rs` (true negative-input rejection, not just a happy-path check).
- **LIKELY / historical, not re-verified this session** — the ledger's own patch-closure history references a recurring `scenario_autonomous_completed_bar_driver_01` test with "9 pre-existing failures" noted across multiple 2026-08 closure entries. This audit did not re-run the suite to confirm current status at HEAD `70ed507a` — flagged as a follow-up check, not asserted as still-failing.

## Domain: Recent Operational Incidents (from ledger §28, spot-read this session)

Three items the ledger's own `LEDGER-CLOSURE-CONSOLIDATION-01` pass (2026-08-24/25) recorded and this audit read directly (not re-investigated):

- **`DATA-READINESS-BAR-COVERAGE-AUTHORITY-01` — CLOSED, INDEPENDENTLY ACCEPTED** (ledger §28/§33). This is the same stale-`sys_autonomous_daily_operations`-row family this audit's L1 pass independently re-confirmed merged into `main` (commits `eaf9f953`..`5346f90a`) from repo memory — the ledger's own record additionally shows it went through independent review and acceptance, not merely a self-assessed local fix. Strengthens the L1 finding rather than contradicting it.
- **`AUTONOMOUS-DAILY-STOPPING-EVIDENCE-DEGRADED-OSCILLATION-01` — CLOSED** (2026-08-24). A family of independently-duplicated close-priority checks in `autonomous_daily_coordinator.rs` that had each individually reimplemented the same `now_utc >= effective_operation_close_utc` gate without a shared exemption predicate — unified onto one shared check.
- **`DAEMON-EXIT-20260824` — STATUS=UNKNOWN-NEEDS-PROOF, still open.** `mqk-daemon.exe` disappeared without an explicit kill during a recorded soak session; forensic evidence shows a precise temporal correlation with the host entering Windows Modern Standby (sleep) for the same ~4h22m window, with the daemon's own tick loop resuming normally on wake and then going silent ~2 minutes later — no crash dump, no Windows Application Error event, no surviving stdout/stderr log for the actual exit window. The ledger is explicit that no production patch is justified without the exact mechanism, and none has been attempted. This is an operational-hygiene risk for future soak sessions (host sleep/power-management during an autonomous session), not a proven code defect — carried forward to the Master Remaining Ledger below rather than re-investigated here.
- **No fresh soak-session evidence found locally since the fix landed.** `smoke_logs/` (protected, inspected read-only, listing only) has no files newer than late July 2026 at the top level and nothing post-dating 2026-08-14 in its `ops/` subdirectory. Combined with §33's own statement ("No Paper soak is claimed to have passed by this entry — none was run as part of this acceptance"), this means: the stale-row blocker is code-closed and independently accepted, but no session in this audit's evidence trail shows it actually being operationally re-validated by a fresh live soak run since 2026-08-25.

---

## A. Executive Verdict

**Equity/ETF Paper V1:** Close, but not honestly callable as operationally complete. Every engineering layer this audit inspected in depth — research/backtest/promotion evidence, order lifecycle/outbox/idempotency, portfolio accounting, reconciliation, autonomous-operation scheduling, the daemon control plane, and the GUI's truth-state discipline — is CONFIRMED sound at the code level, including two significant corrections that make things *better* than the prior ledger stated (Discord daily-outcome alerting is real, not unused; the 2026-08-24 stale-operation blocker is independently-accepted CLOSED, not merely locally fixed). Set against that: this audit found a previously-undocumented, deterministic, safety-critical gap in the live risk gate (daily-loss-limit and max-drawdown checks compare against equity frozen at run start, so they cannot fire against a real intraday loss; reject-storm detection has zero production call sites; the PDT guard is a hardcoded no-op) — three of the account-level capital-preservation controls CLAUDE.md's priority ordering exists to protect are not doing their job today. No RED lookahead/leakage/chronology defect was found anywhere in Research/Backtest. The honest read: the trading *mechanics* are done; the *safety net* around them has a real, fixable hole that should close before treating further soak accumulation as meaningful proof of fail-closed safety.

**Full envisioned MiniQuantDeskV4:** Far from complete, and deliberately so — this is not a criticism, it is the accepted design (CLAUDE.md §21: generalize only when current work requires it). Equities/ETFs have real production depth; every other asset class is, at most, a data/economics-layer scaffold with zero live execution path. Live capital readiness is intentionally and completely gated off behind an unimplemented trust-chain mechanism, plus one confirmed real defect (`buying_power` aliased to `cash`) that would need fixing regardless of when Live work resumes. GUI is a sufficient read-only operator console, not a full trading UI, by design. None of this blocks Finish Line A.

## B. Current Authoritative Baseline

- **Branch:** `main` · **HEAD:** `70ed507acfe02ef860b8378b9e5eddb25a36065d` · **origin/main:** identical.
- **Latest accepted wave:** the strategy-identity/promotion-binding wave (`4ef6b643`..`70ed507a`, 2026-08-29), recorded `PUSHED-VERIFIED` by this audit's own L1 pass (commit `ba7234ae`), itself immediately following the 2026-08-28 independently-accepted `RESEARCH_BACKTEST_V1_COMPLETE` closure and the 2026-08-24/25 independently-accepted Paper-backend stale-operation-release wave (ledger §33).

## C. What Is Actually Complete

- Research → Backtest → OOS/robustness evidence → canonical promotion evaluation → production HTTP promotion boundary → durable Postgres evidence lineage: independently accepted end to end, and now additionally bound to strategy semantic-configuration identity (not just name/opaque id).
- Multi-symbol dispatch panic isolation (host-quarantine mechanism, corrected description in L1).
- Order lifecycle: atomic, idempotent outbox claim; inbox-only, dedup-guarded portfolio writes; structural (not conventional) Paper/Live isolation via one `deployment_mode` gate.
- Reconciliation: one shared "clean reconcile" definition used both for live order gating and for stale-operation release; startup reconcile is not blind and fails closed on ambiguous broker data.
- Autonomous daily operations: the 2026-08-24/25 stale-row blocker is independently-accepted CLOSED and sits on the real bar-dispatch call path; a genuine `evaluation_count`-based distinction between "no signal" and "never evaluated" prevents the exact false-positive the mission warned about; Discord daily-outcome/degradation alerting is real.
- Daemon control plane: single source of truth for armed/halted state; dynamic selection wired to the production start path; mode isolation from one source.
- GUI: `truth_state` hard-block discipline holds in the screens checked, stricter than the documented minimum.
- Testing/CI: 520 test files, disciplined `#[ignore]` tracking (743-row inventory, CI-enforced reason requirement on promoted-proof files), DB-backed tests run by default in CI, genuine adversarial negative controls (lookahead, negative-input rejection) confirmed by direct reading, not assumed.
- Recovery: local backup/restore round-trip is reproduced-proven (real `pg_dump`, real disposable-DB restore); halt/flatten separation is a deliberate, evidenced design decision.

## D. Critical Path to Equity/ETF Paper V1

1. **Bind the live risk gate to real runtime state (equity, reject count, PDT).** Status: OPEN. Why required: two of the account-level kill switches CLAUDE.md's safety priorities exist for cannot fire today. Evidence: `mqk-runtime/src/runtime_risk.rs` (frozen `equity_micros`), `mqk-risk/src/types.rs::record_reject` (zero production callers), `PdtContext::ok()` hardcoded. Acceptance: a live-equity-drop test proves the daily-loss/max-drawdown gate denies; a reject-storm negative control proves the same; PDT context reflects real state (or is explicitly, visibly disabled rather than silently no-op'd). Patch class: CODE + TEST.
2. **Confirm migration `0067`'s status against the Paper database.** Status: OPEN (UNKNOWN-NEEDS-PROOF — this audit did not inspect the Paper DB, per mission constraint). Why required: the strategy-semantic-identity wave depends on it; an un-migrated Paper DB could behave differently than the code this audit reviewed assumes. Acceptance: a read-only confirmation (migration-manifest check against the running Paper DB) that `0067` is applied, or an explicit plan to apply it before the next session. Patch class: DB-PROOF / OPERATIONAL-PROOF.
3. **Re-validate the stale-operation fix with a real, fresh soak session.** Status: OPEN. Why required: the fix is code-closed and independently accepted, but no session in evidence has actually run a fresh Paper day against it — §33 says so explicitly, and this audit found no newer local evidence. Acceptance: one clean autonomous daily session, on the current binary, that reaches `strategy_evaluation_count > 0` (or a legitimate no-signal verdict distinguishable per the `evaluation_count`/outcome-classifier mechanism confirmed above). Patch class: OPERATIONAL-PROOF.
4. **Enable GitHub branch protection on `main`.** Status: OPEN (CONFIRMED via `gh api`, HTTP 404 "Branch not protected"). Why required: CI passing is currently advisory only; nothing prevents a direct unreviewed push to the branch this whole ledger treats as ground truth. Acceptance: required status checks configured for at least the `rust`, `guards`, and `gui-contract` jobs. Patch class: CONFIG (GitHub settings, not code).
5. **Decide and, if needed, mitigate the live corporate-action/event-risk gap.** Status: OPEN. Why required: the live path has no earnings-calendar feed and no pre-event flattening gate (only an operator-declared static blackout file), materially weaker than the backtest engine's `ForbidPeriods`. Acceptance: either an explicit, documented operator process that covers this during active soak sessions, or a scoped patch. Patch class: DOC (near-term) or CODE (if a live gate is built).
6. **Fix the operational cause of `DAEMON-EXIT-20260824`.** Status: OPEN (UNKNOWN_NEEDS_PROOF root cause per the ledger itself). Why required: an unattended autonomous session must survive ordinary operational faults; a host entering sleep mid-session and the daemon going silent afterward is a soak-session-invalidating event if it recurs. Acceptance: disable host sleep/Modern-Standby during autonomous sessions (operational config) and/or add a watchdog/crash-detection mechanism. Patch class: OPERATIONAL-PROOF (config) or CODE (watchdog).
7. **Confirm current status of the `max_concurrent_symbols` fed-in-production question.** Status: OPEN (UNKNOWN-NEEDS-PROOF). Why required: the dispatch loop is confirmed wired, but whether production ever actually supplies more than one symbol (vs. a single-symbol env fallback) was not settled. Acceptance: trace or test what `loop_runner.rs` actually passes as `assignments` in the current deployment configuration. Patch class: TEST/CODE-PROOF.

## E. Non-Critical Backlog

- `MULTI-SYMBOL-CAP1-TRUNCATE-SURFACE-01` (ledger-tracked, still `READY`/open) — graceful truncation instead of fail-closed when the watchlist exceeds `max_concurrent_symbols`.
- Concurrent long+short multi-strategy dispatch on the same symbol (currently single-strategy-per-host; short-side itself works, just not multiplexed with long).
- Real offsite Backblaze B2 restore proof (L1 finding — local restic round-trip is proven, real-B2 round-trip is not).
- Leadership/fencing full trace for two-daemon-instance safety (LIKELY, not fully proven this session).
- `max_drawdown` config-absence should fail closed like its `daily_loss_limit` sibling, for internal consistency (small, low-urgency fix once item D.1 above is otherwise in flight).
- Market-data WebSocket streaming, if ever desired over the current REST-poll ingestion model (not required for Finish Line A as currently scoped — bars, not ticks, drive strategy evaluation).
- `scenario_autonomous_completed_bar_driver_01`'s historically-referenced "9 pre-existing failures" — status at current HEAD not re-verified this session; worth a quick check by the next patch owner.

## F. Full-V4 Remaining Work

- **Multi-asset:** crypto has local data-provider integration only, no live execution path; futures/options/FX are enum + risk-multiplier stubs, explicitly gated off. All multi-asset execution work is unstarted by design.
- **Live:** hard-gated behind an unimplemented `live_trust_complete` trust-chain mechanism (by design); `buying_power` aliasing to `cash` is a confirmed, real defect independent of that gate and would need fixing before any Live consideration; no live-capital smoke-test tooling exists.
- **GUI:** current state is a sufficient read-only operator console with a bounded action surface — further maturity (full trading UI, broader visualization) is a deliberate non-goal for now, not a gap against Finish Line A.
- **Cloud/failover:** out of scope, not investigated this audit; no evidence found of active work in this direction.

## G. Known Defects

Deterministic, code-evidenced only (limitations/deferred-features/unproven-claims are listed separately above, not here):

1. **Live risk gate uses run-start-frozen equity** — daily-loss-limit and max-drawdown checks cannot fire against a real intraday loss. `mqk-runtime/src/runtime_risk.rs`. Severity: HIGH (capital-preservation control silently inert).
2. **Reject-storm detection has zero production call sites** — `RiskState::record_reject()` is only called from `mqk-risk`'s own test. Severity: HIGH (a documented kill-switch cannot trigger).
3. **PDT guard is a hardcoded no-op** — `PdtContext::ok()` unconditionally. Severity: MEDIUM (relevant only to accounts subject to PDT rules, but silently non-functional regardless of config).
4. **`max_drawdown` fails open (silently disabled) on missing config, inconsistent with `daily_loss_limit`'s fail-closed sibling behavior.** Severity: LOW-MEDIUM.
5. **`buying_power` aliased to `cash` in the portfolio summary route** — inaccurate for a margin account. Severity: MEDIUM, but out of near-term scope (Live-gated).
6. **`main` has no GitHub branch protection.** Severity: MEDIUM (process/governance, not a runtime defect).
7. **Live corporate-action/event-risk handling has no earnings-calendar feed and no pre-event flattening gate**, unlike the backtest engine. Severity: MEDIUM, self-documented in code, not hidden.
8. **`DAEMON-EXIT-20260824` root cause remains `UNKNOWN_NEEDS_PROOF`** (Windows sleep correlation, not a proven code defect) — listed here as a tracked open risk, not asserted as a confirmed code defect.

Proposed patch IDs (not implemented, per mission scope): `RUNTIME-RISK-LIVE-STATE-BINDING-01` (defects 1-3), `RUNTIME-RISK-DRAWDOWN-FAIL-CLOSED-01` (defect 4), `LIVE-PORTFOLIO-BUYING-POWER-AUTHORITY-01` (defect 5, Full-V4/Live lane), `REPO-GOVERNANCE-BRANCH-PROTECTION-01` (defect 6, config not code), `LIVE-EVENT-RISK-CALENDAR-GATE-01` (defect 7, if a code fix is chosen over an operator-process mitigation).

## H. Test / Proof Matrix

| Subsystem | Code proof | Unit/integration proof | DB proof | Provider proof | Paper proof | CI proof |
|---|---|---|---|---|---|---|
| Research/Backtest/Promotion | CONFIRMED | CONFIRMED | CONFIRMED (disposable PG, HTTP route readback) | N/A | N/A (research-stage) | CONFIRMED (runs in CI) |
| Strategy identity/semantic binding | CONFIRMED | CONFIRMED | CONFIRMED (per L1) | N/A | UNKNOWN-NEEDS-PROOF (migration 0067 vs live Paper DB) | CONFIRMED |
| Multi-symbol dispatch panic isolation | CONFIRMED | CONFIRMED (real-panic negative controls) | N/A | N/A | UNKNOWN-NEEDS-PROOF (no fresh soak since fix) | CONFIRMED |
| Execution/OMS | CONFIRMED | CONFIRMED | CONFIRMED (atomic outbox claim) | N/A | UNKNOWN-NEEDS-PROOF | CONFIRMED |
| Risk gate (structural wiring) | CONFIRMED | CONFIRMED | N/A | N/A | UNKNOWN — likely-inert per defects 1-3 | CONFIRMED (tests pass, but don't exercise live-equity-change scenario) |
| Portfolio/Accounting | CONFIRMED | CONFIRMED | CONFIRMED | N/A | UNKNOWN-NEEDS-PROOF | CONFIRMED |
| Reconciliation/Recovery | CONFIRMED | CONFIRMED | CONFIRMED | N/A | UNKNOWN-NEEDS-PROOF (no fresh soak) | CONFIRMED |
| Autonomous Paper Ops | CONFIRMED | CONFIRMED | CONFIRMED | LIKELY (Alpaca REST) | UNKNOWN-NEEDS-PROOF (no fresh soak since 2026-08-25) | CONFIRMED |
| Daemon/Control Plane | CONFIRMED | CONFIRMED | N/A | N/A | UNKNOWN-NEEDS-PROOF | CONFIRMED |
| GUI | CONFIRMED | CONFIRMED (contract gate) | N/A | N/A | N/A | CONFIRMED |
| Recovery/offsite backup | CONFIRMED (scripts) | CONFIRMED (local round-trip) | N/A | CONFIRMED locally / OUTSTANDING for real B2 | N/A | not CI-covered (manual runbook) |
| Live | CONFIRMED gated-off | CONFIRMED (`BP-07` negative control) | N/A | N/A | N/A (correctly never exercised) | CONFIRMED |
| Multi-asset (non-equity) | PARTIAL (data layer only) | PARTIAL | PARTIAL | LIKELY (crypto providers) | N/A | CONFIRMED for what exists |

## I. Operational Paper Readiness

**YES-WITH-NON-BLOCKING-LIMITATIONS.**

The one previously-tracked hard blocker (stale-operation release) is code-closed and independently accepted. Nothing found in this audit's evidence trail structurally prevents starting a controlled Paper session today. The limitations are real but non-blocking for *starting*: the frozen-equity risk-gate defects mean the session's own kill-switches for daily-loss/max-drawdown/reject-storm will not meaningfully protect it (Paper capital is not real, so this is a soak-validity concern, not a capital-loss concern) — but item D.1 should close before treating any resulting soak evidence as proof that fail-closed safety controls work, since right now they provably do not fire. The next session should also budget for confirming migration `0067`'s Paper-DB status and disabling host sleep for the session's duration.

## J. Alpha Readiness

**YES-WITH-LIMITATIONS.**

`RESEARCH_BACKTEST_V1_COMPLETE` is independently accepted end to end; the semantic-identity wave closes a real class of "same label, different behavior" gaps in promotion evidence. Nothing in this audit's Research/Backtest/Promotion review found a reason to block real candidate research and promotion work. The limitation: candidates promoted under pre-semantic-identity evidence rules may need evidence regeneration under the new binding rules (this audit did not inspect any actual promoted-candidate rows — DB inspection was out of scope — so this is a process caution, not a confirmed current problem) and the risk-gate defects above mean any resulting Paper-forward validation of a promoted candidate inherits the same soak-validity caveat as §I.

## K. 10-20 Session Soak Readiness

For a session to count as VALID, it must show, via durable evidence (not daemon-uptime alone):
- `sys_autonomous_daily_operations` reached `running` **and** `sys_autonomous_daily_bar_dispatches` has real rows for that day (bars were actually dispatched, not silently blocked).
- `AutonomousDailyOutcomeEvidenceSummary.evaluation_count > 0` **or** an explicit, evidence-backed `NoTradeStrategyEvaluatedNoSignal` classification — never a bare zero-orders day with no evaluation evidence, which the outcome classifier itself distinguishes from `MissingEvaluationEvidence`/`EvidenceBlocked`.
- No `GapDetected`, no unresolved reconcile drift, no crash-orphaned exit (i.e., not a repeat of `DAEMON-EXIT-20260824`'s unexplained-silence pattern) for that day.
- Given the confirmed frozen-equity risk-gate defect, a session where a real drawdown occurred but the risk gate did not react should be flagged, not silently counted as a clean pass.

Per the evidence gathered, **zero sessions since the 2026-08-25 fix landed have been confirmed VALID in this audit's evidence trail** — the fix is accepted, but unexercised by a real subsequent session as far as local evidence shows.

## L. Master Remaining Ledger

**NOW** (required before equity/ETF code-complete): items D.1-D.4 above (risk-gate live-state binding, migration 0067 confirmation, fresh soak re-validation, branch protection).

**NEXT** (required before/for controlled Paper): items D.5-D.7 (event-risk mitigation decision, `DAEMON-EXIT-20260824` operational fix, multi-symbol-in-production confirmation).

**AFTER SOAK** (valuable, deliberately deferred): everything in §E (truncate-and-surface cap behavior, concurrent long/short dispatch, real offsite B2 proof, fencing full trace, `max_drawdown` fail-closed consistency, MD streaming, the one historical test-failure follow-up).

**FULL-V4** (multi-asset/Live/product expansion): everything in §F (multi-asset execution, Live buying-power/trust-chain, GUI maturity beyond operator-sufficiency, cloud/failover).

## M. Recommended Next Mission

**`RUNTIME-RISK-LIVE-STATE-BINDING-01`** — bind `RuntimeRiskGate` to real, current runtime state instead of values frozen at run start: (a) feed actual current equity into the risk evaluator on each gate check so daily-loss-limit and max-drawdown can genuinely fire; (b) call `RiskState::record_reject()` from the real order-rejection path so reject-storm detection is live; (c) replace the hardcoded `PdtContext::ok()` with real account PDT state (or, if PDT truly does not apply to this account/asset class today, replace it with an explicit, visible "not applicable" state rather than a silent always-pass). This is the single highest-leverage finite blocker found in this audit: it is the one deterministic, safety-critical, currently-inert set of controls standing between "the mechanics work" and "the mechanics are actually protected the way CLAUDE.md's priority ordering requires," and every other NOW-lane item (migration confirmation, a fresh soak run, branch protection) is either lower-severity or a precondition-free operational step rather than an engineering patch. Recommend running it as a CLAUDE.md-style autonomous wave with three sequential patches (equity binding → reject-storm wiring → PDT context), each with its own RED/GREEN proof against a live-equity-change / synthetic-reject-burst negative control, since each is independently testable and none should be merged bundled with the others.


