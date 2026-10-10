# MiniQuantDeskV4 — Permanent Completion Contract and Acceptance Matrix

**Contract ID:** `MQD-V4-FINISH-DEFINITION-01`
**Established:** 2026-10-10
**Authority:** Operator's explicit five affirmative completion decisions; this document is a **proposed repository-controlled acceptance baseline** until committed through the normal repo process.
**Scope:** Entire V4 platform. **Not** a declaration that the current code has satisfied this contract.

## 1. The immutable destination

MiniQuantDeskV4 (V4) is **FINISHED** when all five conditions are true:

1. **Every planned subsystem is implemented and integrated**, with objective acceptance evidence.
2. **Autonomous lifecycle works** inside operator-approved policies: discover → ingest/normalize → formalize → research → backtest → independently validate → qualify → promote → deploy → monitor → control risk → retire, with durable identity, auditability and safe restart.
3. **All six asset classes are operational end to end:** US equities, ETFs, listed options, cryptocurrencies, futures and spot forex. Each has working eligible market data, instrument specifications, causal research/backtests, strategy execution, account/risk handling, appropriate broker integration, Paper operation and an explicitly authorized Live capability. No asset is considered finished merely because types, GUI routes or architecture support it.
4. **Practical production reliability is demonstrated:** all frozen critical safety/correctness invariants pass; no known unmitigated material blocker remains; normal recoverable failures are handled and visible; uncertainty fails closed. Impossibility of all failures is **not** an acceptance requirement.
5. **Profitability is a separate, ongoing outcome:** Software completion does not require positive backtests, qualified alpha, a winning strategy or sustained profit. The automated selection process must correctly *reject* unqualified strategies. No promotion or trading authorization may be manufactured to satisfy the completion target.

**Definition of operational:** actual intended integration proven in a controlled environment, not merely compiled, simulated, mocked or described. **Definition of Live capability:** a guarded production route exists and has been validated using authorized safe methods; actual submission of Live orders is **not** required by this contract and may never be done without separate explicit operator authorization. An explicitly operator-disabled Live switch can coexist with V4 FINISHED if the capability itself is validated and ready for authorized use. This is a completion criterion, not authorization to place an order.

## 2. Fixed rules for acceptance, review and change

**Completion statuses:** `NOT STARTED` / `IN PROGRESS` / `IMPLEMENTED` / `INDEPENDENTLY ACCEPTED` / `OPERATIONALLY VERIFIED` / `BLOCKED` / `UNKNOWN-NEEDS-PROOF`. `FINISHED` applies only to the entire platform after all mandatory cells are `OPERATIONALLY VERIFIED` (or a cell explicitly marked `NOT APPLICABLE` by prior operator approval with rationale).

**Review dispositions:**
- **BLOCKER:** reproducible failure or clearly reachable source-level contradiction violating a frozen acceptance criterion, trading-safety authority, factual research evidence, identity/provenance, or essential lifecycle functionality. State impact, reproduction, affected code and minimum remedy. Untested speculation does not qualify.
- **ACCEPT WITH DOCUMENTED LIMITATION:** bounded, disclosed behavior outside the frozen criterion, or explicitly scoped unavailable capability that does not invalidate that subsystem's claimed status. An unresolved mandatory asset/capability prevents *whole-V4* FINISHED without reopening correctly completed components.
- **BACKLOG:** cosmetics, maintainability, optional performance improvements, additional theoretical edge cases, expanded coverage or enhancements not required by this contract.
- **UNKNOWN-NEEDS-PROOF:** consequential uncertainty. Specify the shortest discriminating test or evidence query; never automatically turn uncertainty into a broad repair.

**Finite closure:** One comprehensive controller and initial defect census → agent's targeted negative controls and second sweep → one independent review → at most one consolidated, surgical correction → acceptance decision. An unresolved proven safety violation stays `BLOCKED`; it does **not** justify infinite discretionary extensions. Critical incidents found later may be reopened with new reproducible evidence, not preference changes. **Small commits; large authorized missions; split sessions, not scope.** Full workspace CI is a milestone/release acceptance tool, not a mandatory loop on every local commit.

**Controlled changes:** Any change to the five finish conditions, mandatory matrix cells, or pass/fail semantics must be a separately identified `FINISH-CONTRACT-CHANGE-NNN` with operator approval, reason, old/new wording and consequences. Ordinary implementation/design choices that satisfy existing criteria do not redefine FINISHED. A new asset class or subsystem not listed here is future scope unless operator explicitly amends the contract.

**Separation of authorities:** AI may discover, interpret, propose and rank; deterministic, auditable code owns identities, eligibility, risk, execution and promotion gates. A human operator controls broker credentials, monetary permissions, allowed assets/venues, capital allocation and risk budgets, legal/account restrictions and Live enablement. No silent default may grant economic authority.

## 3. Mandatory platform-wide acceptance matrix

Every row is independently measurable. Evidence **must identify commit SHA, executable code path, test/trace/artifact, environment and limitations**. Tests must hit production seams, including negative controls where a failure could falsely appear successful.

| ID | Capability | Objective acceptance gate | Minimum proof | Current verification |
|---|---|---|---|---|
| C01 | Canonical authority and configuration | Single versioned authoritative policy/configuration path; no duplicate or fail-open controller | Source trace + invalid/missing config negative control | UNKNOWN-NEEDS-PROOF |
| C02 | Identity and provenance | Strategy, hypothesis, trial, attempt, evaluation, dataset, run, account, order and fill identities stable and auditable; semantic changes correctly versioned | Round-trip, restart and mutation evidence | UNKNOWN-NEEDS-PROOF |
| C03 | Data ingestion and storage | Each asset's raw and normalized market data ingests idempotently, persists durably and reports source, vintage, clock and errors | Actual adapter + failure/replay tests | UNKNOWN-NEEDS-PROOF |
| C04 | Point-in-time and calendars | Features, universe, corporate actions, settlement/expiry/funding and venue calendars cannot leak future-known information | Temporal negative controls and known-date fixtures | UNKNOWN-NEEDS-PROOF |
| C05 | Strategy idea intake | Manual, catalog, approved external and AI-proposed ideas have typed provenance, dedup and explicit admission disposition | Actual intake paths + duplicates and adversarial inputs | INDEPENDENTLY ACCEPTED (Factory software scope; operational intake connectors unverified) |
| C06 | Strategy formalization | Deterministic, versioned executable rule representation; unsupported constructs refuse safely | Native engine integration, serialization, negative controls | INDEPENDENTLY ACCEPTED (Factory implemented grammar; other intended rule families TBD) |
| C07 | Research orchestration | Durable, restartable, bounded search scheduler records full search population, retries and disqualifications | Interrupt/duplicate/failure injection; restart evidence | INDEPENDENTLY ACCEPTED (Factory synthetic scope) |
| C08 | Causal backtesting | Causal order and fill timing; executable prices/costs, no `fwd_ret`-as-P&L, realistic instrument restrictions | End-to-end backtest negative controls, cost/slippage cases | UNKNOWN-NEEDS-PROOF across entire V4 |
| C09 | Evidence / OOS / holdout | Explicitly separated development, confirmation and final holdout; search-adjusted evaluation; no winner-only registration | Tamper/forbidden-window tests, evidence-grade audit | UNKNOWN-NEEDS-PROOF globally |
| C10 | Qualification / Promotion | Deterministic acceptance gates; no promotion on synthetic/exposed/insufficient evidence; independent authorization and trace | Rejected and accepted fixture cases + authorization negative test | UNKNOWN-NEEDS-PROOF |
| C11 | Multi-strategy and multi-symbol engine | Concurrent authorized instances deterministic, resource-bounded, idempotent and failure-isolated | Replay, race, restart and double-dispatch controls | UNKNOWN-NEEDS-PROOF |
| C12 | Cross-asset portfolio / capital | Portfolio exposure, allocation, currency conversion, margin/collateral and net risk calculated without duplicated capital or implicit netting policy | Aggregation and insufficient-capital negative controls | UNKNOWN-NEEDS-PROOF |
| C13 | Risk and emergency controls | Pretrade and continuous risk, kill switch, staleness, leverage, market-open and liquidation controls fail closed | Direct dispatch-path refusal tests and controlled recovery | UNKNOWN-NEEDS-PROOF |
| C14 | Order, broker and reconciliation | Durable idempotent lifecycle, cancellation/replacement, provider rejects, partial fills, restarts and reconcile actual broker state | Real sandbox/Paper adapter traces with injected failures | UNKNOWN-NEEDS-PROOF |
| C15 | Paper lifecycle | Each asset class has verified market-data→signal→risk→intent→broker→ack/fill/reconcile path in real Paper environment | Date/time scoped operational logs, broker records, positions and recovery; no fabricated activity | UNKNOWN-NEEDS-PROOF |
| C16 | Live capability (gated) | Each asset class has approved adapter contract, separate credentials/environment, explicit disabled-by-default authority and safe non-order validation | Live-path safe dry-run/contract checks; no Live orders required or permitted by this test | UNKNOWN-NEEDS-PROOF |
| C17 | Autonomous lifecycle | Eligible strategies promoted, deployed and retired without routine intervention **only when** configured conditions hold; unqualified strategies remain undeployed | End-to-end synthetic control flow + operational audit of allowed transitions | UNKNOWN-NEEDS-PROOF |
| C18 | Research AI/ML | AI proposal/model training and versioning cannot override deterministic decisions; reproducible training/holdout boundaries; optional AI failure does not grant trading authority | Feature leakage, model lineage and fail-closed controls | UNKNOWN-NEEDS-PROOF |
| C19 | GUI / observability / alerting | Correctly scoped true state of subsystem, per-asset workflows, explicit degraded/errors, auditable operator actions | Frontend and backend contract tests plus interactive application exercise | INDEPENDENTLY ACCEPTED (frontend scope only; backend dependencies outstanding) |
| C20 | Security / access / secrets | Least privilege, separate environments, secret hygiene, operator approvals, authenticated privileged actions, reproducible security checks | Negative authorization tests + reviewed config/secret handling | UNKNOWN-NEEDS-PROOF |
| C21 | Crash/restart/ops | Clean startup, stop, bounded retries, recovery and reconciliation after representative service/network/DB interruption | Actual restart/chaos traces and runbook execution | UNKNOWN-NEEDS-PROOF |
| C22 | Deployment & reproducibility | Deterministic build, migrations, configuration, versioned roll-forward and rollback procedures; deploys to intended Windows operator hardware | Fresh install/upgrade/recovery smoke on target platform | UNKNOWN-NEEDS-PROOF |
| C23 | Test/CI acceptance evidence | Required cross-platform, Rust, Python, DB, GUI, security and key broker contract proofs run successfully on release candidate SHA | SHA-linked acceptance run(s), documented skips/waivers | NOT CURRENTLY VERIFIED; automatic workflows operator-disabled |
| C24 | Operational documentation | Runbooks cover data/provider outage, restart, expired credentials, reject, risk halt, monitoring, backups and human escalation | Operator performs dry-run walkthrough | UNKNOWN-NEEDS-PROOF |
| C25 | Policy, entitlements and licensing | Provider and broker capabilities, eligible accounts/assets/venues, market-data licenses and regional restrictions are explicit, with unavailable operation refused | Provider/account read-only capability evidence + denial cases | UNKNOWN-NEEDS-PROOF |
| C26 | Auditable history / incident governance | Durable append-only evidence where required; explain exactly why any decision/trade was made; incident and holdout custody accurate | Reconstruct decision trail from artifacts and DB | UNKNOWN-NEEDS-PROOF |

**No automatic promotion of statuses:** Previously accepted parts are credited only within their reviewed scope. `UNKNOWN-NEEDS-PROOF` is **not** an assertion of a defect. An operational gate may be unverified while its implementation is excellent.

## 4. Mandatory per-asset operational matrix

Each of six rows must pass **all 12 gates**. An asset has status `OPERATIONALLY VERIFIED` only after the real adapter + operational Paper evidence and safe Live-capability validation exist. No unconditional claim of actual Live order functionality without separately authorized testing; specific account/broker entitlements may require an operator decision.

**Gate definitions:** `A` instrument specs and canonical identity; `B` market/reference data provenance and point-in-time history; `C` sessions, instrument life cycle and trading constraints; `D` native strategy formalization and deterministic signal generation; `E` realistic causal cost-aware backtest; `F` evidence grading and Promotion compatibility; `G` allocation/margin/collateral/currency/Greeks where applicable; `H` risk gates and safe halts; `I` suitable broker adapter and supported orders; `J` actual Paper execution/reconciliation and restart evidence; `K` guarded Live capability in approved account without mandatory Live order; `L` GUI/monitoring/alerts/operator control.

| Asset class | A identity | B data | C market rules | D strategies | E backtest | F promotion | G capital | H risk | I broker | J Paper | K Live-gated | L UI/ops | Whole class |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| US stocks | U | U | U | U | U | U | U | U | U | U | U | U | U |
| ETFs | U | U | U | U | U | U | U | U | U | U | U | U | U |
| Listed options | U | U | U | U | U | U | U | U | U | U | U | U | U |
| Crypto spot (and derivatives only if authorized separately) | U | U | U | U | U | U | U | U | U | U | U | U | U |
| Futures | U | U | U | U | U | U | U | U | U | U | U | U | U |
| Spot forex | U | U | U | U | U | U | U | U | U | U | U | U | U |

`U` = UNVERIFIED **for this completion contract**, not absent functionality. Replace cells only with evidence-linked statuses after code and operational inspection. Equity/ETF capability stays first reference implementation while the mission's final target includes all six classes.

**Asset-specific mandatory examples:**
- **Stocks:** splits, dividends, symbol changes, delistings, short-sale/borrow constraints, halts and equity settlement.
- **ETFs:** stock-like execution plus distributions, structural/corporate changes, occasional premium/discount, liquidity and any leveraged/inverse characteristics.
- **Listed options:** underlying, contract symbol/strike/right/expiry/multiplier, OCC lifecycle/adjustments, exercise/assignment, early exercise, contract liquidity, spread and Greeks exposure; approved option permissions and strategies.
- **Crypto:** venue and custody/account segmentation, tick/lot rules, 24/7 calendars, exchange maintenance, funding only where applicable, venue outages and withdrawal/counterparty exposure.
- **Futures:** root/contract/expiry, roll and continuous-data distinction, first notice/last trading, variation margin, contract multiplier, session breaks and exchange risk limits.
- **Forex:** pair/base/quote conventions, spread/rollover/swap, timezone/weekend gaps, financing, leverage, conversion and venue/account suitability.

Whether leveraged crypto derivatives, OTC options or additional asset classes are mandatory requires a separate explicit operator scope decision; **spot crypto, listed options, exchange-traded futures and spot forex** are the defined V4 baseline, not vague placeholders.

## 5. Autonomous lifecycle acceptance sequence

The following MUST work for at least one **authorized, qualified fixture** per asset class and across multiple simultaneous strategies, then for real market data and a controlled Paper context as provider/account constraints permit:

`IDEA → PROVENANCE → DEDUP → FORMALIZE → DECLARE POPULATION → DATA ELIGIBILITY → CAUSAL RESEARCH/BACKTEST → OOS GRADE → PROMOTION DECISION → POLICY-AUTHORIZED DEPLOYMENT → CONTINUOUS RISK → ORDER/RECONCILIATION → MONITOR → RETIRE/RECOVER`

A negative control follows the same path with insufficient evidence, expired data, no capital, disabled venue or revoked authorization; it must **stop at the correct gate**, without generating orders, fills, positions, or false readiness. Retried jobs cannot mint new hypotheses/trials. No strategy can acquire Promotion solely because it won an exposed search.

**Qualification vs activity:** If no candidate meets the economic bar, V4 remains software-capable and reports `NO_ELIGIBLE_STRATEGY` with zero new deployment. An empty but correctly controlled Paper system may prove *refusal*, but cannot alone prove end-to-end operational order handling; that requires separate, explicitly permitted test evidence without inventing market events or lowering trading gates.

## 6. Frozen release acceptance / objective stopping rule

V4 receives `FINISHED — SOFTWARE AND OPERATIONS` **only when**:

- All C01–C26 mandatory platform rows have current evidence-backed `OPERATIONALLY VERIFIED`, or have a prior explicit, narrowly justified operator `NOT APPLICABLE` approval.
- All 72 asset gate cells (6 × 12) are evidence-backed pass, with representative per-asset Paper operations and safe Live capability verification, and all required autonomous control paths demonstrated.
- One release-candidate commit / immutable artifact version is identified. A comprehensive cross-platform/integration acceptance run on that exact code passes, with no quietly skipped critical tests. Pre-release CI can be operator-disabled during development; **release acceptance CI cannot be skipped and still claim FINISHED**.
- The blocker register contains **zero known unmitigated material correctness/safety defects**. Nonmaterial limitations and ordinary backlog items remain allowed and do not change FINISHED.
- Operator approves the **completion claim** after examining a concise evidence index; this is distinct from enabling Live trading.

**No arbitrary requirements:** No mandate for positive alpha, a minimum profit, zero warnings, perfect code style, universal fault immunity, fixed mutation count, repeated full-suite runs, real-money order submission, or infinite operational soak. Where a meaningful soak is needed, define its duration and purpose in the *relevant subsystem's predeclared acceptance plan*; do not extend it retroactively. Contract-wide proof should use **representative failures and targeted negative controls**, not every theoretical fault permutation.

## 7. Current starting point — evidence-scoped, not a full audit

At the time this contract was drafted, the user reported and GitHub independently showed `origin/main = 8f438740ac926092bb522a906283b6a24f126d67` with Strategy Factory and GUI merged. Strategy Factory software / synthetic E2E is independently accepted within that scope, including concurrency correction and strict native CI at `10520b8dba505fc89f0aec83bddb35f515b0b098`. GUI frontend/observability scope was independently accepted; several daemon/backend dependencies remain. Automatic CI workflows were intentionally disabled by the operator on 2026-10-10. Combined main CI success is **not asserted**. Nothing herein resolves the holdout incident or asserts provider, Paper, Live or profitability evidence. Existing ledger claims on `main` may be stale after merge and should be reconciled as ordinary controlled documentation when next touched—not used to revoke accepted functionality.

**This is the stable finish line, not a claim that we are already at it.**

## 8. Operator decisions that can be supplied as policies without changing FINISHED

The system can implement fail-closed policy fields before values are supplied. Later input from the operator is required for enabled venues/brokers/accounts per asset; capital allocations and cross-asset risk budgets; supported live order types and optional derivatives strategies; data-provider entitlements/licensing; autonomous promotion thresholds; retention and kill-switch policy; and release authorization. These are **configuration/authorization decisions**, not permission to re-open the definition of software completion. The exact choice of provider or broker is an implementation decision unless it changes the asset coverage or authority contract.

## 9. Repository adoption instructions

Store this contract in a tracked canonical location, e.g. `docs/program/MQD_V4_PERMANENT_COMPLETION_CONTRACT.md`, and link it from the Master Program Plan and Ledger. Adopt it in **one documentation-only coherent commit**, after confirming it does not contradict a higher-priority accepted safety or regulatory contract. No live data, holdout access, DB migration, trading configuration change, CI policy change or trading action is authorized by adoption. If a conflict is found, present the precise conflicting clauses as an operator decision rather than silently weakening either contract. Revisions must follow `FINISH-CONTRACT-CHANGE-NNN`.
