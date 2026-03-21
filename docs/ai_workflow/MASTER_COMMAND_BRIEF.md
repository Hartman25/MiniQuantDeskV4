# MiniQuantDesk V4 — Master Command Brief

Keep this file compact and current. This is the top-level command map to give an AI before non-trivial repo work.

## Repo identity

- **Project name:** MiniQuantDesk V4
- **Current stage:** strong partial platform with active closure work still open
- **Primary purpose:** build an institutional-style, deterministic trading and research platform with truthful operator surfaces, durable artifacts, controlled execution, and auditable proof
- **What this system is not:** not yet a complete trading business, not yet a proven alpha engine, not yet a mature live-ops platform, not yet a multi-asset production stack

## Canonical system boundary

- **Canonical engine:** `MAIN`
- **Non-canonical / experimental areas:** `EXP` research-side lanes only
- **Live-authoritative areas:** canonical MAIN execution/runtime/control truth only
- **Mounted but not always fully wired areas:** some operator surfaces may be mounted before their final authoritative backend is complete; treat them with suspicion until proven
- **Areas that must remain fail-closed:** daemon/operator truth surfaces, restart/control semantics, suppressions/summary/config-diff style truth, any area where unavailable truth could be mistaken for authoritative empty truth

## Major domains

### 1. Core Rust platform
- **Purpose:** canonical runtime, daemon, DB, execution, backtest, GUI support surfaces
- **Owning paths:** `core-rs/crates/*`
- **Operator relevance:** high
- **Truth sources:** code, DB-backed behavior, scenario proof, readiness docs

### 2. Research Python layer
- **Purpose:** research workflows, experiments, artifacts, supporting evaluation
- **Owning path:** `research-py/`
- **Operator relevance:** medium for research, low for canonical operator truth
- **Truth sources:** code, experiment manifests, research-side tests; not canonical ops truth

### 3. Readiness authority
- **Purpose:** define readiness standard and scoring
- **Owning docs:** `docs/INSTITUTIONAL_READINESS_LOCK.md`, `docs/INSTITUTIONAL_SCORECARD.md`
- **Operator relevance:** very high for readiness judgments
- **Truth sources:** these docs plus committed-state proof

### 4. Patch planning / workflow
- **Purpose:** organize closure work and AI/operator process
- **Owning docs:** remaining-work patch plan, operator ledger, AI workflow pack
- **Operator relevance:** high for execution sequencing
- **Truth sources:** current operator-maintained docs only

## High-level architecture

- Rust core provides canonical daemon/runtime/db/execution/backtest surfaces.
- Python research layer provides non-canonical research workflows and EXP-side experimentation.
- DB-backed truth is preferred where readiness rules require it.
- Mounted surfaces must not imply authoritative truth unless backed by the right source.
- Canonical proof matters more than optimistic implementation claims.
- EXP may share foundations but must not widen MAIN operational truth.

## Non-negotiable invariants

- no fabricated truth
- no optimistic defaults on operator surfaces
- DB-backed truth where required by readiness rules
- deterministic behavior where expected
- explicit truth-state distinction between unavailable, empty, and present
- fail closed when authority is unavailable
- MAIN and EXP must remain distinct in operational meaning

## Current project posture snapshot

### Readiness
- Full committed-state proof transcript exists and is valuable.
- Readiness is still not automatically “closed forever” if a source-level truth concern remains disputed.

### Completion
- Infrastructure is strong.
- Platform completion is still partial.

### Trading viability
- Alpha, economics, and business realism are still open questions.

### Live ops
- Supervised paper/shadow-style work is more credible than unattended production use.

### Maintainability
- Several sink files remain too large and need staged decomposition later.

## Current MAIN remaining-work list

### Blockers
1. **IR-01** — Control operator-audit durable-truth closure
2. **IR-02** — Operator-action audit proof promotion

### Completion / strengthening
3. **DOC-01** — Source-of-truth documentation reconciliation
4. **CC-01** — Authoritative strategy-fleet registry and summary surface
5. **CC-02** — Durable strategy suppressions and mounted truth
6. **CC-03** — Controlled restart / mode-transition workflow
7. **CC-04** — Canonical OMS overview surface
8. **CC-05** — Canonical metrics dashboards surface

### Trading viability
9. **TV-01** — Research → backtest → execution artifact contract closure
10. **TV-02** — Deployment economics and tradability gate
11. **TV-03** — Shadow/live parity evidence chain
12. **TV-04** — Portfolio and capital-allocation realism

### Live ops
13. **LO-01** — Operator runbook expansion
14. **LO-02** — Stressed recovery proof matrix
15. **LO-03** — Live-shadow / live-capital end-to-end operational proof

### Maintainability / refactor
16. **MT-01** — Decompose daemon state/routes sink files
17. **MT-02** — Extract runtime orchestrator phases
18. **MT-03** — Modularize DB access layer and GUI system API layer

## Default instructions for any serious AI task

- Start narrow.
- Identify the active audit axis or patch objective.
- Use subsystem brief + patch packet + minimal file bundle.
- Keep MAIN and EXP separate.
- Do not claim closure beyond the evidence.
