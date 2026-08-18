# MiniQuantDesk V4 — Active Patch Ledger

> **HISTORICAL / TECHNICAL RECORD** (noted by `DOCS-TRACKER-CLEANUP-01`, 2026-08-17)
>
> This is a 2026-04-25 planning snapshot, already noted as superseded by `docs/audits/multi_asset_completion_audit.md`. Its "Immediate Active" hardening items predate four months of subsequent work reflected in the current ledger; its backlog-derived sections (§2) are drawn from `miniquantdesk_future_backlog_brainstorm.md`, which remains the retained source for that material.
>
> **Current completion status and remaining work are authoritative only in `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`.**

Generated from:
- Current hostile repo audit findings from the 2026-04-25 audit pass.
- `miniquantdesk_future_backlog_brainstorm.md` uploaded 2026-04-25.

Source-of-truth rule:
Current repo code, migrations, tests, proof scripts, and clean committed proof output outrank prior chat claims, old patch labels, docs, README statements, or “closed” language.

Execution doctrine:
1. Paper execution hardening first.
2. End-to-end proof second.
3. Preserve fail-closed behavior.
4. Live transition only after proof is complete.
5. Future AI/research/visualization/multi-asset ideas stay deferred until execution truth is hardened.

Status legend:
- ACTIVE: should be worked before claiming autonomous paper readiness.
- NEXT: should follow immediately after active blockers.
- STAGED: useful after paper execution is stable.
- DEFERRED: future backlog, not current priority.
- RESEARCH: experimental only, not allowed to authorize execution.
- PARKED: cleanup/architecture debt; not urgent unless touched by related work.

Blocker flags:
- SP = supervised paper
- AP = autonomous paper
- LIVE = live readiness

---

## 1. Immediate Active Ledger — Paper / Live Hardening

| Priority | Patch ID | Status | Severity | Blocks | Type | Why it matters | Likely files / surfaces | Smallest coherent next step |
|---:|---|---|---|---|---|---|---|---|
| 1 | SEC-SNAPSHOT-01 | ACTIVE | Critical | LIVE | Security / tooling | Repo handoff zips must not include `.env.local`, tokens, provider keys, broker keys, webhooks, proof secrets, or generated credentials. | snapshot/export scripts, `.gitignore`, docs handoff workflow | Rotate exposed keys; add export exclude rules; add proof that exported zips exclude secrets. |
| 2 | BRK-GAP-01 | ACTIVE | Critical | AP, LIVE | Code + proof | Alpaca WS gap recovery is not full lifecycle recovery if non-fill events can be missed during disconnect. | `mqk-runtime/src/alpaca_inbound.rs`, `mqk-daemon/src/state/alpaca_ws_transport.rs`, broker adapter, DB cursor tests | After WS gap, require broker snapshot/reconcile proof before continuity returns to Live; otherwise halt/quarantine. |
| 3 | BRK-REST-02 | ACTIVE | Critical/High | AP, LIVE | Code + proof | Alpaca REST fill recovery must not stop at a single page. High-volume gaps can exceed `page_size=50`. | `mqk-broker-alpaca/src/lib.rs`, Alpaca inbound tests | Implement pagination until exhaustion; prove >50 fills recover, dedupe, and cursor advancement remains durable. |
| 4 | CTRL-ARM-01 | ACTIVE | Critical/High | SP, AP, LIVE | Control-plane truth | HTTP/GUI arm route must not bypass authoritative preflight or directly clear halt/disarm truth. | `mqk-daemon/src/routes/control.rs`, preflight/arm tests, GUI contract tests | Make `/control/arm` call authoritative preflight or downgrade it to desired-arm request only. |
| 5 | EXEC-CANCEL-01 | ACTIVE | High | AP, LIVE | Runtime durability | Missing cancel target should not leave an outbox row in `DISPATCHING` until later cleanup. | `mqk-runtime/src/orchestrator/dispatch.rs`, DB outbox tests | Immediately fail/quarantine/halt missing cancel target with durable audit reason. |
| 6 | EXEC-RETRY-01 | ACTIVE | High | AP, LIVE | DB + runtime | Retryable broker errors need bounded attempts/backoff, not unlimited retry churn. | migrations, `mqk-db/src/orders.rs`, `mqk-execution/src/broker_error.rs`, runtime dispatch | Add attempt count, last error, next eligible time, max-attempt quarantine/halt behavior. |
| 7 | EXEC-CONT-01 | ACTIVE | High | AP, LIVE | Runtime proof | `InboundContinuityUnproven` must always create durable halt/disarm truth. | `mqk-runtime/src/orchestrator.rs`, daemon runtime loop tests | Either persist halt in runtime branch or prove every caller converts it into durable halt/disarm. |
| 8 | OPS-REPAIR-01 | ACTIVE | High | AP, LIVE | Operator workflow | Ambiguous outbox release/reset needs an explicit audited repair workflow, not ad hoc DB reset. | DB outbox helpers, daemon ops routes, audit events, reconcile snapshot code | Add audited repair command/route that fetches broker state, compares local state, emits evidence, then releases/fails. |
| 9 | CI-DB-01 | NEXT | High | Proof governance | CI/proof | Local full DB proof is strong, but operator-class Windows DB proof is not fully CI-enforced unless required separately. | `.github/workflows/ci.yml`, `full_repo_proof.ps1`, docs | Add self-hosted Windows DB lane or state local full proof transcript is mandatory for release. |
| 10 | DOC-READY-01 | NEXT | High | AP, LIVE truth | Docs | `institutional_ready_proof_completed` can be misread as live-ready. Proof-clean must be separated from operational readiness. | `docs/INSTITUTIONAL_READINESS_LOCK.md`, `docs/INSTITUTIONAL_SCORECARD.md`, proof summary output | Split labels into proof-clean, supervised-paper-candidate, autonomous-paper-candidate, live-ready. |
| 11 | OBS-TIME-01 | NEXT | Medium/High | AP, LIVE | Event truth | Inbox/event timelines should store real broker event timestamps/sequences, not placeholder zero. | inbox insert callers, normalized broker events, operator timeline routes | Carry broker timestamp/sequence into inbox and order timeline; add ordering proof. |
| 12 | OPTR-LABEL-01 | NEXT | Medium | AP, LIVE | Operator truth | Not every OMS apply error should be surfaced as `UNKNOWN_ORDER_FILL`. Operators need exact fault taxonomy. | runtime apply path, OMS/apply errors, daemon status/events | Add typed `OmsApplyError` taxonomy and map exact operator labels. |
| 13 | BRK-BLOCKING-01 | PARKED | Medium | Future scale | Architecture | Blocking HTTP inside async runtime is brittle for long-running multi-asset concurrency. | Alpaca adapter | Move to async client or isolate broker IO worker after current execution blockers close. |
| 14 | BRK-PRICE-01 | STAGED | Medium | Multi-asset LIVE | Architecture | Two-decimal equity formatting and generic qty parsing will not survive crypto/futures/options/tick-size instruments. | broker adapter, schemas, instrument model | Add instrument tick/lot metadata before multi-asset work. |
| 15 | ARCH-SINK-01 | PARKED | Medium | Future velocity | Maintainability | Daemon sink files will slow patching and increase regression risk. | `mqk-daemon/src/state.rs`, `api_types.rs`, route modules | Split only after active runtime/broker blockers close. |
| 16 | ASSET-SCOPE-01 | DEFERRED | Medium/Low | Multi-asset LIVE | Architecture | Current canonical path is equity-focused; multi-asset needs real instrument/session/margin abstractions. | execution, broker, risk, portfolio, strategy | Do not start until equity paper path is proven. |
| 17 | DOC-COMMENT-01 | PARKED | Low | No | Docs/comments | Stale comments around broker errors/ambiguous behavior can mislead future patchers. | comments near broker error/outbox helpers | Clean stale comments opportunistically. |
| 18 | HOLD-MIG-01 | PARKED | Low | Maybe LIVE | DB/docs | Held migration around global fill uniqueness needs an explicit keep/drop/promote decision. | migrations, DB tests | Document why run-scoped uniqueness is enough or promote migration. |

---

## 2. Backlog-Derived Patch Ledger — Deferred Until Paper Execution Is Hardened

These are created from `miniquantdesk_future_backlog_brainstorm.md`. They are intentionally staged or deferred unless they directly strengthen execution truth.

### 2.1 Trade journal / review / analytics

| Priority | Patch ID | Status | Severity | Blocks | Type | Why it matters | Likely files / surfaces | Smallest coherent next step |
|---:|---|---|---|---|---|---|---|---|
| 19 | JOURNAL-THESIS-01 | STAGED | Medium | No | DB + runtime + GUI | Store why each trade was entered/exited: entry reason, exit reason, strategy, regime, confidence. | DB migrations, runtime order intent metadata, GUI order detail | Add thesis table or JSON contract linked to trade/order IDs; no effect on execution authorization. |
| 20 | JOURNAL-THESIS-02 | STAGED | Low/Medium | No | Operator UX | Make thesis visible in trade/order detail without letting it become execution authority. | GUI execution/order detail, daemon read route | Add read-only thesis panel and tests. |
| 21 | ANALYTICS-MAE-MFE-01 | STAGED | Medium | No | Analytics | Track maximum adverse/favorable excursion for post-trade review and stop/target tuning. | market data replay, fills, portfolio analytics tables | Define MAE/MFE calculation source and persistence; prove deterministic replay. |
| 22 | TRADE-REVIEW-01 | STAGED | Medium | No | Analytics/reporting | Weekly trade review should summarize win rate, expectancy, MAE/MFE, strategy/time/regime performance, top losses, stop-outs. | analytics module, report generator, artifacts | Generate deterministic weekly markdown/JSON report from DB. |
| 23 | TRADE-REVIEW-02 | DEFERRED | Low/Medium | No | Scheduling/export | Automate weekly report creation after analytics foundation exists. | scheduler/scripts, artifacts directory | Add manual command first; schedule later. |

### 2.2 Strategy quality and risk rules

| Priority | Patch ID | Status | Severity | Blocks | Type | Why it matters | Likely files / surfaces | Smallest coherent next step |
|---:|---|---|---|---|---|---|---|---|
| 24 | STRAT-TIME-FILTER-01 | STAGED | Medium | AP quality | Strategy/risk gate | Avoid low-quality opening trades or reduce sizing during opening volatility. | strategy config, calendar/session provider, risk/strategy decision path | Add deterministic no-trade window config and `no_trade_reason`. |
| 25 | STRAT-TIME-FILTER-02 | DEFERRED | Low/Medium | No | Operator truth | Surface time-filter decisions in strategy summary and review reports. | daemon strategy routes, GUI strategy screen | Add read-only display once filter exists. |
| 26 | RISK-STOP-IMMUTABILITY-01 | STAGED | High | AP, LIVE | Risk/execution safety | Prevent widening stop loss after entry; allow unchanged/tighter stop or reduced exposure only. | risk, execution replace/cancel, OMS lifecycle | Add tighten-only stop invariant with tests. |
| 27 | RISK-STOP-IMMUTABILITY-02 | STAGED | Medium/High | LIVE | Proof/reconcile | Prove stop replacements cannot widen risk even across restart/replay/reconcile. | DB tests, runtime tests, reconcile | Add crash/restart proof for stop immutability. |

### 2.3 Regime, decay, and strategy health

| Priority | Patch ID | Status | Severity | Blocks | Type | Why it matters | Likely files / surfaces | Smallest coherent next step |
|---:|---|---|---|---|---|---|---|---|
| 28 | REGIME-01 | STAGED | Medium | No | Schema/attribution | Persist regime labels for performance attribution: trend/range/high-vol/low-vol/panic/accumulation. | DB schema, strategy decision metadata, analytics | Add regime enum/taxonomy and attach to strategy decisions/trades. |
| 29 | REGIME-02 | DEFERRED | Medium | No | Deterministic classifier | Add baseline deterministic regime classifier, not AI-authoritative execution. | strategy/research module, market data | Implement simple explainable classifier after attribution schema exists. |
| 30 | DECAY-01 | DEFERRED | Medium | No | Analytics | Detect strategy edge deterioration from realized performance. | analytics, strategy metrics, alerts | Add rolling metrics and decay status read model. |
| 31 | DECAY-02 | DEFERRED | High | AP/LIVE later | Control/risk | Reduce size, disable strategy, or alert operator when decay threshold is breached. | strategy control, risk, alerts | Alert-only first; automated throttle/disable only after proof. |

### 2.4 AI explanation / forensics — never execution authority

| Priority | Patch ID | Status | Severity | Blocks | Type | Why it matters | Likely files / surfaces | Smallest coherent next step |
|---:|---|---|---|---|---|---|---|---|
| 32 | FORENSICS-AI-01 | DEFERRED | Medium | No | Export/analysis | Let AI analyze completed trades after the fact: why lost, what changed, pattern breakdown. | artifacts, journal, reports | Export trade packet for external/manual AI review. |
| 33 | FORENSICS-AI-02 | DEFERRED | Medium | No | Storage/operator UX | Store AI forensic notes as non-authoritative annotations. | DB notes table, GUI review screen | Add annotation storage with explicit `not_execution_authority=true`. |
| 34 | LLM-EXPLAIN-01 | DEFERRED | Low/Medium | No | Explanation layer | Explain why a trade happened, signal alignment, regime, risk posture. | journal, strategy decisions, GUI | Read-only explanation generated from existing deterministic facts. |
| 35 | ML-SAFETY-01 | STAGED | High | LIVE later | Safety guard | Ensure AI/LLM/ML outputs cannot directly authorize live trades. | execution gateway, strategy signal boundary, docs/tests | Add explicit guard/test that LLM-sourced decisions cannot bypass deterministic gateway. |

### 2.5 Multi-agent signal council — research only

| Priority | Patch ID | Status | Severity | Blocks | Type | Why it matters | Likely files / surfaces | Smallest coherent next step |
|---:|---|---|---|---|---|---|---|---|
| 36 | ML-REGIME-01 | RESEARCH | Low/Medium | No | Research | Research-side regime classifier for later signal analysis. | research sidecar, artifacts | Keep offline/research-only. |
| 37 | ML-SIGNAL-01 | RESEARCH | Low/Medium | No | Research | Specialist signal scoring for later study. | research sidecar | Produce scores only; no order authority. |
| 38 | ML-COUNCIL-01 | RESEARCH | Medium | No | Research | Weighted council verdict engine for research comparison. | research sidecar | Prove verdict cannot route to execution. |

### 2.6 AI development workflow / verifier tooling

| Priority | Patch ID | Status | Severity | Blocks | Type | Why it matters | Likely files / surfaces | Smallest coherent next step |
|---:|---|---|---|---|---|---|---|---|
| 39 | CODEX-VERIFY-01 | STAGED | Medium | No | Dev workflow | Use Codex as independent verifier for repo audits, diff validation, test verification, and failure analysis. | docs/dev workflow, scripts/prompts | Add a repo-local `docs/workflows/CODEX_VERIFIER.md` and verification prompt template. |
| 40 | AGENT-FLOW-DEV-01 | DEFERRED | Low/Medium | No | Dev observability | Explore Agent Flow for visualizing Claude/Codex/tool-call behavior. | external dev tooling docs | Add optional local setup notes only; do not make repo runtime dependency. |
| 41 | AI-STACK-01 | DEFERRED | Low | No | Workflow docs | Document ChatGPT architect + Claude implementation + Codex verification + Agent Flow observability. | docs/workflows | Keep outside runtime. |

### 2.7 MiniQuantDesk internal flow visualization

| Priority | Patch ID | Status | Severity | Blocks | Type | Why it matters | Likely files / surfaces | Smallest coherent next step |
|---:|---|---|---|---|---|---|---|---|
| 42 | FLOW-01 | DEFERRED | Medium | No | Event model | Define internal trading-flow event vocabulary: market data -> signal -> risk -> intent -> broker -> fill -> OMS -> portfolio -> reconcile -> alerts. | schemas, runtime events, daemon DTOs | Create contract only; no GUI yet. |
| 43 | FLOW-02 | DEFERRED | Medium | No | Runtime instrumentation | Emit flow events at deterministic pipeline boundaries. | runtime, execution, risk, broker, portfolio, reconcile | Add non-authoritative trace emission. |
| 44 | FLOW-03 | DEFERRED | Medium | No | DB/proof | Persist flow events with stable trace IDs and retention policy. | migrations, DB crate, tests | Prove trace linkage without affecting execution. |
| 45 | FLOW-04 | DEFERRED | Low/Medium | No | GUI | Visualize flow timeline in operator console. | GUI, daemon route | Build read-only view after backend contract exists. |
| 46 | FLOW-05 | DEFERRED | Low | No | Operator UX | Add filters for symbol, strategy, order, run, severity, stage. | GUI | Add after FLOW-04. |
| 47 | FLOW-06 | DEFERRED | Low/Medium | No | Performance | Retention/performance guard so visualization cannot slow trading loop. | DB indexes, runtime async emission | Add proof of bounded overhead. |

### 2.8 Multi-asset expansion

| Priority | Patch ID | Status | Severity | Blocks | Type | Why it matters | Likely files / surfaces | Smallest coherent next step |
|---:|---|---|---|---|---|---|---|---|
| 48 | ASSET-REGISTRY-01 | DEFERRED | Medium | Multi-asset | Architecture | Instrument registry for equities/crypto/futures/options/forex metadata. | schemas, DB, config, broker adapters | Define instrument identity, tick size, lot size, session, venue. |
| 49 | ASSET-FEED-01 | DEFERRED | Medium | Multi-asset | Market data | Multi-feed manager with per-asset providers and freshness semantics. | mqk-md, runtime, config | Do after single-asset/equity paper proof. |
| 50 | ASSET-RISK-01 | DEFERRED | High | Multi-asset LIVE | Risk | Per-asset risk models for margin, leverage, contract sizing, Greeks later. | risk, portfolio | Do after instrument registry. |
| 51 | ASSET-SCHED-01 | DEFERRED | Medium | Multi-asset | Runtime scheduling | Concurrent scheduling by asset/session/venue. | runtime, strategy, session calendar | Do after broker/runtime concurrency proof. |
| 52 | ASSET-PORTFOLIO-01 | DEFERRED | High | Multi-asset LIVE | Portfolio/accounting | Portfolio aggregation across asset classes and currencies. | portfolio, DB, reconcile | Do after risk model. |

---

## 3. Prompt Grouping Plan

Use these groups to reduce AI-token waste. A group means “one prompt can reasonably ask Claude/Codex to inspect and patch this bundle.” For high-risk runtime patches, still request one coherent patch at a time inside the prompt and require the full changed functions/sections back.

### Group A — Secret hygiene + readiness wording + proof governance

Patch IDs:
- SEC-SNAPSHOT-01
- DOC-READY-01
- CI-DB-01
- DOC-COMMENT-01

Can be one prompt: YES.
Why: mostly docs/scripts/proof labels; low runtime risk.
Recommended agent: Claude for patch, Codex for verification.
Proof:
- export secret-exclusion test if possible
- full proof script smoke
- docs grep for misleading `live-ready` wording

### Group B — Broker inbound recovery hardening

Patch IDs:
- BRK-GAP-01
- BRK-REST-02
- EXEC-CONT-01
- OBS-TIME-01
- OPTR-LABEL-01

Can be one prompt: PARTIAL.
Use one prompt for diagnosis and patch plan, but implement as 3–5 separate patches.
Why: same runtime/broker/inbox truth surface, but too critical to merge blindly.
Recommended patch order:
1. BRK-REST-02
2. OBS-TIME-01
3. EXEC-CONT-01
4. OPTR-LABEL-01
5. BRK-GAP-01

### Group C — Execution durability / dispatch safety

Patch IDs:
- EXEC-CANCEL-01
- EXEC-RETRY-01

Can be one prompt: YES, but patch sequentially.
Why: both touch outbox/dispatch semantics and can share tests.
Recommended agent: Claude implementation, Codex hostile proof review.
Proof:
- missing cancel target durable outcome
- retry attempts bounded
- max-attempt quarantine/halt behavior

### Group D — Operator arm and repair control plane

Patch IDs:
- CTRL-ARM-01
- OPS-REPAIR-01

Can be one prompt: YES for route/control-plane audit; patch separately if large.
Why: both are operator-control truth issues.
Proof:
- HTTP arm cannot bypass preflight
- ambiguous repair requires broker snapshot evidence and audit event

### Group E — Risk/strategy hardening add-ons

Patch IDs:
- STRAT-TIME-FILTER-01
- STRAT-TIME-FILTER-02
- RISK-STOP-IMMUTABILITY-01
- RISK-STOP-IMMUTABILITY-02

Can be one prompt: YES for design/audit, but implementation should split into time-filter and stop-immutability patches.
Why: both affect strategy/risk gating and operator no-trade reasons.
Do after active broker/execution blockers.

### Group F — Trade journal + analytics/reporting foundation

Patch IDs:
- JOURNAL-THESIS-01
- JOURNAL-THESIS-02
- ANALYTICS-MAE-MFE-01
- TRADE-REVIEW-01
- TRADE-REVIEW-02
- REGIME-01

Can be one prompt: YES for schema/design; likely multiple implementation patches.
Why: all need shared trade identity, strategy ID, regime, fill/order linkage.
Do after execution truth is proven.

### Group G — Regime + decay strategy health

Patch IDs:
- REGIME-02
- DECAY-01
- DECAY-02

Can be one prompt: YES.
Why: all are analytics/control around strategy health.
Guardrail: alert-only before any automated disable/throttle.

### Group H — AI forensics/explanation safety

Patch IDs:
- FORENSICS-AI-01
- FORENSICS-AI-02
- LLM-EXPLAIN-01
- ML-SAFETY-01

Can be one prompt: YES.
Why: one safety boundary should govern all AI-generated explanations/notes.
Guardrail: no AI output can authorize or modify execution.

### Group I — AI dev workflow / Codex / Agent Flow

Patch IDs:
- CODEX-VERIFY-01
- AGENT-FLOW-DEV-01
- AI-STACK-01

Can be one prompt: YES.
Why: docs/dev-workflow only; no runtime dependency.
Guardrail: Agent Flow remains optional local tooling.

### Group J — Internal trading flow visualization

Patch IDs:
- FLOW-01
- FLOW-02
- FLOW-03
- FLOW-04
- FLOW-05
- FLOW-06

Can be one prompt: YES for architecture/design; implementation should be staged.
Recommended patch order:
1. FLOW-01 contract
2. FLOW-02 emission
3. FLOW-03 persistence/proof
4. FLOW-04 GUI
5. FLOW-05 filters
6. FLOW-06 retention/performance

### Group K — Multi-asset architecture foundation

Patch IDs:
- BRK-PRICE-01
- ASSET-SCOPE-01
- ASSET-REGISTRY-01
- ASSET-FEED-01
- ASSET-RISK-01
- ASSET-SCHED-01
- ASSET-PORTFOLIO-01

Can be one prompt: YES for future architecture audit only.
Do not implement until equity paper path is proven.
Recommended first implementation later:
1. ASSET-REGISTRY-01
2. BRK-PRICE-01
3. ASSET-FEED-01
4. ASSET-RISK-01
5. ASSET-SCHED-01
6. ASSET-PORTFOLIO-01

---

## 4. Recommended Immediate Execution Order

1. Group A: Secret hygiene + readiness wording + proof governance.
2. BRK-REST-02 alone.
3. OBS-TIME-01 alone.
4. EXEC-CONT-01 alone.
5. OPTR-LABEL-01 alone.
6. BRK-GAP-01 alone.
7. Group C: EXEC-CANCEL-01 + EXEC-RETRY-01, preferably sequential within one prompt.
8. Group D: CTRL-ARM-01, then OPS-REPAIR-01.
9. Re-run full repo proof on clean tree.
10. Only then consider staged risk/strategy/reporting backlog.

---

## 5. Do-Not-Start-Yet List

Do not start these until paper execution hardening and end-to-end proof are clean:
- ML-REGIME-01
- ML-SIGNAL-01
- ML-COUNCIL-01
- AGENT-FLOW-DEV-01, unless purely local workflow docs
- FLOW-01 through FLOW-06
- ASSET-* multi-asset expansion
- heavy strategy experimentation
- advanced AI execution systems

