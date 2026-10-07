# MQD high-level runtime architecture (Archify evaluation)

**DESCRIPTIVE / NON-AUTHORITATIVE.**
**STALE IF VIEWED AGAINST A DIFFERENT REPOSITORY COMMIT.**

This diagram observes and explains MiniQuantDesk V4. It does not control it, and it is not trading,
architecture, promotion, Paper or Live authority. Where it disagrees with committed code, tests, Git or DB
proof, those win; then binding specs/ledgers/runbooks; then this diagram. Do not repair architecture to make
the picture cleaner.

- Anchor commit: `c81695de3da1bec7b3b8bc1b951fae8d0e95f307` (branch `main`, CI #630 green at acceptance).
- Produced by Archify 3.0.1, operator-invoked only. Not wired into CI, hooks or schedules.
- Open `mqd-runtime-architecture.html` in a browser (self-contained; no network needed to view).

## Files

| File | Role |
|---|---|
| `mqd-runtime-architecture.json` | Typed Archify source (the reviewable artifact; HTML is derived from it) |
| `mqd-runtime-architecture.html` | Generated self-contained interactive diagram |
| `mqd-runtime-architecture.provenance.json` | MQD provenance: anchor SHA, hashes, tool versions, inference policy |
| `mqd-runtime-architecture.delivery.json` | Archify-native delivery receipt (`archify check --require-provenance` reads it) |
| `mqd-runtime-architecture.finalize-summary.json` | Archify gate summary (validate, deliver, check, browser-check = pass) |

The summary references two larger Archify receipts (`.finalize.json`, `.browser-check.json`) that are not
committed; they embed container-local absolute paths. Strict `check --require-provenance` passes without them.

## What it shows

14 components, 20 relationships. The order path runs
strategy decision → durable outbox (Postgres) → `ExecutionOrchestrator` (the only caller of
`BrokerGateway::submit`) → Alpaca adapter → Alpaca paper endpoint → broker events → `oms_inbox` →
OMS/portfolio apply. Integrity, risk and reconcile gates are evaluated inside the gateway on every submit.

Research is a separate evidence-producing domain: **research has no order-submission authority**. Its only
route into the runtime is operator-authorized: artifacts → Promotion Gate (operator POST, evidence
re-validated) → durable `active_paper` transition row in Postgres → admission gate on the next decision.

Component count is 14 (preferred 8–12, hard cap 15). The excess over 12 is two external endpoints
(data provider, broker) and the operator plane (GUI, mode/config gate), each a separate trust or authority
boundary. Secondary detail lives in the diagram cards.

## Evidence

Every component carries 1–3 `SRC` references (path + line range), which Archify verifies against committed
blobs at the anchor. Archify proves only that the path and range exist at that commit, not that the range
supports the claim. A second, content-level check was run for this artifact: each of the 37 cited ranges was
required to contain its claimed symbol in the anchor blob (37/37).

Archify's schema has no per-edge source field, so edge evidence is recorded here (all paths relative to the
repo root; line numbers valid at the anchor):

| Edge | Evidence |
|---|---|
| operator → daemon (commands, Bearer) | `core-rs/mqk-gui/src/features/ingest/api.ts:693`; `core-rs/crates/mqk-daemon/src/routes.rs:103,1013-1015` |
| daemon → operator (status, `truth_state`) | `core-rs/mqk-gui/src/features/system/api.ts:489-560` |
| mode/config gate → daemon (start gate) | `core-rs/crates/mqk-daemon/src/state/lifecycle.rs:1567-1615` |
| daemon → promotion (token POST) | `core-rs/crates/mqk-daemon/src/routes.rs:924`; `routes/strategy_promotions.rs:1-20` |
| provider → research (GET bars only) | `research-py/src/mqk_research/data/alpaca_historical.py:218-220,650-652` |
| research → promotion (artifact files) | `mqk-daemon/src/promotion_evidence_validation.rs:702`; `research_evidence_gate.rs:83`; `mqk-promotion/src/research_registry.rs:297` (read-only SQLite) |
| promotion → Postgres (durable transition) | `mqk-daemon/src/routes/strategy_promotions.rs:47-49,811`; `mqk-db/src/strategy_promotion.rs:611` |
| provider → ingest (bars) | `mqk-md/src/provider.rs:402`; `mqk-daemon/src/main.rs:146`; `state/autonomous_completed_bar_task.rs:232` |
| ingest → Postgres (`md_bars`) | `mqk-db/src/md.rs:478` |
| Postgres → strategy (bars, promotion, arm) | `mqk-db/src/md.rs:1423`; `mqk-daemon/src/state.rs:4546`; `decision.rs:7-24`; `promotion_gate.rs:184` |
| strategy → Postgres (outbox enqueue) | `mqk-daemon/src/decision.rs:1667`; `state/loop_runner.rs:2166` |
| Postgres → orchestrator (claim outbox) | `mqk-db/src/orders.rs:588`; `mqk-runtime/src/orchestrator.rs:1-20` |
| safety → orchestrator (gate verdicts) | `mqk-execution/src/gateway.rs:1-30`; `mqk-daemon/src/state/orchestrator_build.rs:493-507` |
| orchestrator → adapter (submit) | `mqk-runtime/src/orchestrator.rs:1-12`; `mqk-daemon/src/state/broker.rs:61-70` |
| adapter → broker (REST orders) | `mqk-broker-alpaca/src/lib.rs:1-20`; `mqk-daemon/src/state/broker.rs:144-162` |
| broker → truth (WS/REST events) | `mqk-runtime/src/orchestrator.rs:14-20`; `mqk-runtime/src/alpaca_inbound.rs:1-30` |
| truth → safety (reconcile) | `mqk-daemon/src/state/loop_runner.rs:2308`; `state/types.rs:207-233`; `mqk-reconcile/src/lib.rs:1-14`. Simplification: the REST broker-snapshot fetch that reconcile compares against is not drawn as its own edge (`state/types.rs:756-763`) |
| truth → Postgres (inbox, ledger) | `mqk-db/src/inbox.rs:216`; `mqk-runtime/src/orchestrator/apply.rs:312` |
| Postgres → daemon (durable reads) / daemon → Postgres (arm, run state) | `mqk-db/src/arm_state.rs:15`; `state/loop_runner.rs:179` |

## Conceptual, unknown and inferred

- **CONCEPTUAL (not wired, not drawn as live):** Live trading; non-equity asset classes (V2 scaffold,
  `mqk-execution/src/lib.rs:28-36`); IBKR adapter (crate exists, not a daemon dependency).
- **UNKNOWN (not inspected):** whether a daemon is running, Paper DB contents and migration level, the
  deployed strategy, promotion rows. The diagram quotes documented state at the anchor (Paper INACTIVE,
  Live DISABLED, no promoted candidate, `PROMOTION_AUTHORITY = NONE`; `README_TECHNICAL.md`,
  `docs/CURRENT_MISSION.md`, `research-py/experiments/alpha_edge_confirmation_01/results/CONFIRMATION_RESULT.md`)
  and labels it as documentation, not runtime proof.
- **INFERRED:** "research-py has no order path" is a repository-search result (no broker/outbox/order
  endpoint, no DB writes outside `exp_distributed` SQLite, one network egress: GET `data.alpaca.markets`),
  reinforced by Cargo dependency edges (`mqk-backtest` and `mqk-promotion` do not depend on broker, runtime or
  daemon crates). It is not a proof of absence.
- **Not drawn:** CLI direct DB bookkeeping (`mqk run/db/ingest`, which is not the daemon control plane),
  WS transport and gap recovery, Postgres `audit_events`, crypto and option modules.

## Discrepancies found (documented only; nothing was changed)

1. `core-rs/crates/mqk-daemon/src/state/autonomous_completed_bar_driver.rs` header says the driver is not
   started from `main.rs`; `main.rs:146` spawns it for Paper+Alpaca (the header is stale).
2. `mqk-daemon` declares a dependency on `mqk-audit` (`Cargo.toml:51`) but never references it; audit rows
   go to Postgres `audit_events` via `mqk-db`.
3. `README_TECHNICAL.md` §2 cites an older accepted baseline (`fac22592`, CI #623) than this anchor.

## Verification performed for this artifact

Archify `finalize` (validate, deliver, strict check, browser check) with `--repo-root` evidence verification;
desktop screenshot reviewed by the author (no crossings; one advisory 3-bend route, promotion → Postgres);
content check of all 37 references; secret-pattern scan; typed-source/HTML SHA-256 recorded in the provenance
file. Negative controls (isolated copies, discarded): anchor SHA missing or nonexistent, hash mismatches,
HTML tamper, title or provenance relabelled authoritative, Live tag changed to enabled, research edges to
orchestrator/broker/Postgres, component without sources, secret-shaped string, evidence line/path/end-line/
commit broken, and a shifted evidence range.

## Regenerating

Operator-invoked only. From this directory, with `ARCHIFY_UPDATE_CHECK_DISABLED=1` and a Chromium path in
`ARCHIFY_CHROME`:

```
node ../../../.claude/skills/archify/bin/archify.mjs finalize architecture \
  mqd-runtime-architecture.json mqd-runtime-architecture.html \
  --repo-root ../../.. --quality showcase --json
```

A regenerated diagram must re-anchor `meta.repository.revision` to the new commit and refresh the provenance
file; otherwise it is stale. Archify itself is a vendored third-party skill (`.claude/skills/archify/`,
`skills-lock.json`); it has no MQD authority.
