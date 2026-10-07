# MQD high-level runtime architecture (Archify evaluation)

**DESCRIPTIVE / NON-AUTHORITATIVE.**
**STALE IF VIEWED AGAINST A DIFFERENT REPOSITORY COMMIT.**

This diagram observes and explains MiniQuantDesk V4. It does not control it, and it is not trading,
architecture, promotion, Paper or Live authority. Where it disagrees with committed code, tests, Git or DB
proof, those win; then binding specs/ledgers/runbooks; then this diagram. Do not repair architecture to make
the picture cleaner.

- Anchor commit: `f4e4fb3e5fc34302c81d282a329956e019c35094` (production fix `LiveShadow: refuse new economic orders at every admission seam`; local branch `liveshadow-no-order-authority-closure-01`, not pushed, CI not yet run on it). Previous anchor: `c81695de3da1bec7b3b8bc1b951fae8d0e95f307` (`main`, CI #630).
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

| Edge | Evidence (repo-root-relative, valid at the anchor commit) |
|---|---|
| operator → daemon (commands, Bearer) | `core-rs/mqk-gui/src/features/ingest/api.ts:693`; `core-rs/crates/mqk-daemon/src/routes.rs:103,1013-1015` |
| daemon → operator (status, `truth_state`) | `core-rs/mqk-gui/src/features/system/api.ts:489-560` |
| mode/config gate → daemon (start gate) | `core-rs/crates/mqk-daemon/src/state/lifecycle.rs:1567-1615` |
| daemon → promotion (token POST) | `core-rs/crates/mqk-daemon/src/routes.rs:924`; `core-rs/crates/mqk-daemon/src/routes/strategy_promotions.rs:1-20` |
| provider → research (GET bars only) | `research-py/src/mqk_research/data/alpaca_historical.py:218-220,650-652` |
| research → promotion (artifact files) | `core-rs/crates/mqk-daemon/src/promotion_evidence_validation.rs:702`; `core-rs/crates/mqk-daemon/src/research_evidence_gate.rs:83`; `core-rs/crates/mqk-promotion/src/research_registry.rs:297` (read-only SQLite) |
| promotion → Postgres (durable transition) | `core-rs/crates/mqk-daemon/src/routes/strategy_promotions.rs:47-49,811`; `core-rs/crates/mqk-db/src/strategy_promotion.rs:611` |
| provider → ingest (bars) | `core-rs/crates/mqk-md/src/provider.rs:402`; `core-rs/crates/mqk-daemon/src/main.rs:146`; `core-rs/crates/mqk-daemon/src/state/autonomous_completed_bar_task.rs:232` |
| ingest → Postgres (`md_bars`) | `core-rs/crates/mqk-db/src/md.rs:478` |
| Postgres → strategy (bars, promotion, arm) | `core-rs/crates/mqk-db/src/md.rs:1423`; `core-rs/crates/mqk-daemon/src/state.rs:4546`; `core-rs/crates/mqk-daemon/src/decision.rs:7-24`; `core-rs/crates/mqk-daemon/src/promotion_gate.rs:184` |
| strategy → Postgres (outbox enqueue) | `core-rs/crates/mqk-daemon/src/decision.rs:1667`; `core-rs/crates/mqk-daemon/src/state/loop_runner.rs:2117` |
| Postgres → orchestrator (claim outbox) | `core-rs/crates/mqk-db/src/orders.rs:642`; `core-rs/crates/mqk-runtime/src/orchestrator.rs:1-20` |
| safety → orchestrator (gate verdicts) | `core-rs/crates/mqk-execution/src/gateway.rs:1-30`; `core-rs/crates/mqk-daemon/src/state/orchestrator_build.rs:493-507` |
| orchestrator → adapter (submit) | `core-rs/crates/mqk-runtime/src/orchestrator.rs:1-12`; `core-rs/crates/mqk-daemon/src/state/broker.rs:61-70` |
| adapter → broker (REST orders) | `core-rs/crates/mqk-broker-alpaca/src/lib.rs:1-20`; `core-rs/crates/mqk-daemon/src/state/broker.rs:144-162` |
| broker → truth (WS/REST events) | `core-rs/crates/mqk-runtime/src/orchestrator.rs:14-20`; `core-rs/crates/mqk-runtime/src/alpaca_inbound.rs:1-30` |
| truth → safety (reconcile) | `core-rs/crates/mqk-daemon/src/state/loop_runner.rs:2259`; `core-rs/crates/mqk-daemon/src/state/types.rs:207-233`; `core-rs/crates/mqk-reconcile/src/lib.rs:1-14`; (simplification: the REST broker-snapshot fetch reconcile compares against is not drawn as its own edge; see `core-rs/crates/mqk-daemon/src/state/types.rs:765-772`) |
| truth → Postgres (inbox, ledger) | `core-rs/crates/mqk-db/src/inbox.rs:216`; `core-rs/crates/mqk-runtime/src/orchestrator/apply.rs:312` |
| Postgres → daemon (durable reads) / daemon → Postgres (arm, run state, manual orders) | `core-rs/crates/mqk-db/src/arm_state.rs:15`; `core-rs/crates/mqk-daemon/src/state/loop_runner.rs:179`; `core-rs/crates/mqk-daemon/src/routes/execution.rs:198-455` |

Mode and order-authority claims. Broker connectivity is not order authority, and the launcher workflow is not a daemon invariant:

| Claim | Evidence (repo-root-relative) |
|---|---|
| Paper + Alpaca routes to `paper-api.alpaca.markets`; the local `LockedPaperBroker` is refused as an execution path | `core-rs/crates/mqk-daemon/src/state/broker.rs:144-162`, `core-rs/crates/mqk-daemon/src/state/broker.rs:167-185` |
| LiveShadow and LiveCapital map to the real Alpaca live base URL (`api.alpaca.markets`) with the `_LIVE` credential pair (connectivity) | `core-rs/crates/mqk-daemon/src/state/broker.rs:144-162`, `core-rs/crates/mqk-daemon/src/state/broker.rs:186-200` |
| **Canonical launcher only:** LiveShadow is real broker connectivity with no orders submitted, no arm, no runtime auto-start | `scripts/windows/Start-MiniQuantDesk.ps1:501-527`, `scripts/windows/Start-MiniQuantDesk.ps1:1819` |
| The repo itself calls LiveShadow's no-order behaviour a runtime design invariant that its smoke wrapper does not prove | `scripts/windows/Start-LiveShadowSmoke.ps1:48-50`, `scripts/windows/Start-LiveShadowSmoke.ps1:345-347` |
| **Strategy-originated orders fail closed in LiveShadow:** `PromotionRunMode::from(LiveShadow)` is `Live`; the gate denies any mode other than Paper with `promotion_live_not_authorized` | `core-rs/crates/mqk-daemon/src/promotion_gate.rs:45-66`, `core-rs/crates/mqk-daemon/src/promotion_gate.rs:184-205` |
| Both strategy-originated outbox writers call that gate before their outbox write (internal decision seam; strategy signal route) | `core-rs/crates/mqk-daemon/src/decision.rs:1400-1412`, `core-rs/crates/mqk-daemon/src/routes/strategy.rs:1152-1175`, `core-rs/crates/mqk-daemon/src/routes/strategy.rs:1448` |
| DB-backed test: LiveShadow+Alpaca internal decision is refused, `promotion_live_not_authorized`, zero outbox rows (`internal_active_paper_denied_when_daemon_mode_is_live`; `#[ignore]`, needs `MQK_DATABASE_URL`; passed against a local disposable Postgres in the fix mission) | `core-rs/crates/mqk-daemon/tests/scenario_strategy_promotion_runtime_gate_01.rs:1231-1268` |
| **Generic arm has no mode fence:** `check_arm_safety` checks reconcile and risk only; `integrity_arm` and `arm-execution` call it | `core-rs/crates/mqk-daemon/src/routes/helpers.rs:574-614`, `core-rs/crates/mqk-daemon/src/routes/control_plane.rs:48-56`, `core-rs/crates/mqk-daemon/src/routes/control_plane.rs:239-241` |
| Hermetic proof arms a LiveShadow+Alpaca state through `/v1/integrity/arm` (HTTP 200) and starts the runtime through `/v1/run/start` (HTTP 200, active bootstrap): arm and run-start stay available in LiveShadow by design, so broker truth can be observed | `core-rs/crates/mqk-daemon/src/state/hermetic_positive_proofs.rs:171-191`, `core-rs/crates/mqk-daemon/src/state/hermetic_positive_proofs.rs:216-259` |
| **Canonical order-authority rule (fix):** only `PAPER` and `LIVE-CAPITAL` run modes may create a new economic order; `LIVE-SHADOW`, `BACKTEST` and any unknown mode fail closed. `DeploymentMode::allows_new_economic_order` delegates to the same primitive | `core-rs/crates/mqk-db/src/orders.rs:375-377`, `core-rs/crates/mqk-daemon/src/state/types.rs:719-721` |
| **Durable fence (fix):** `outbox_enqueue_new_order_for_running_run` checks the locked run row's `mode` inside the run-state fence transaction and returns `RunModeForbidsNewOrder` with zero mutation. Manual order, internal decision, strategy signal, operator flatten and pre-event flatten all use it; the unfenced `outbox_enqueue_for_running_run` remains only for manual cancel of an existing broker-mapped order, enforced by a source guard test | `core-rs/crates/mqk-db/src/orders.rs:448-455`, `core-rs/crates/mqk-daemon/src/routes/execution.rs:382`, `core-rs/crates/mqk-daemon/src/decision.rs:1667`, `core-rs/crates/mqk-daemon/src/routes/strategy.rs:1448`, `core-rs/crates/mqk-daemon/src/pre_event_flatten.rs:511` |
| **Manual order route (fixed):** it refuses by deployment mode before any lifecycle, DB or arm work, then keeps its arm, run, options-lifecycle gates and now uses the fenced enqueue. The source still describes it as "a second real economic-order-admission surface" | `core-rs/crates/mqk-daemon/src/routes/execution.rs:216-229`, `core-rs/crates/mqk-daemon/src/routes/execution.rs:340-355`, `core-rs/crates/mqk-daemon/src/routes/execution.rs:382` |
| Pre-event flatten (system-generated close orders) enqueues nothing for a mode that may not create orders; the operator flatten route is Paper-only | `core-rs/crates/mqk-daemon/src/pre_event_flatten.rs:495-505`, `core-rs/crates/mqk-daemon/src/state/loop_runner.rs:1196-1201`, `core-rs/crates/mqk-daemon/src/routes/control_plane.rs:1744-1745` |
| **Hermetic proofs (isolated disposable Postgres, no broker, no network):** Paper positive control `hermetic_order_submit_enqueues_one_pending_outbox_row` still enqueues one `PENDING` row; `hermetic_live_shadow_manual_order_refused_with_zero_outbox_rows` is refused (HTTP 403) with zero rows | `core-rs/crates/mqk-daemon/src/state/hermetic_positive_proofs.rs:578-606`, `core-rs/crates/mqk-daemon/src/state/hermetic_positive_proofs.rs:705-733` |
| DB-backed proofs of the durable fence, the external signal route and pre-event flatten under LiveShadow (plus a Paper positive control) | `core-rs/crates/mqk-db/tests/scenario_outbox_new_order_mode_fence_01.rs:65-170`, `core-rs/crates/mqk-daemon/tests/scenario_liveshadow_no_order_authority_01.rs:43-170` |
| A start-gate code comment names "LiveShadow running in monitor-only mode" as an example of a deployment allowed a dormant strategy; it is a comment about strategy dormancy, not an order fence | `core-rs/crates/mqk-daemon/src/state/lifecycle.rs:1795-1798` |
| LiveCapital start is refused unless `live_trust_complete=true`; Paper/LiveShadow to LiveCapital transitions are fail-closed | `core-rs/crates/mqk-daemon/src/state/lifecycle.rs:1567-1615`, `core-rs/crates/mqk-daemon/src/mode_transition.rs:213` |

**Not tested:** no real order was submitted to any broker, and none must be. All proofs stop at the durable outbox; nothing here claims anything about downstream broker submission. Rows enqueued before this fix are not migrated or purged. LiveCapital behaviour is unchanged.

## Conceptual, unknown and inferred

- **CONCEPTUAL / NOT ENABLED:** LiveCapital order execution (start refused); non-equity asset classes (V2 scaffold,
  `core-rs/crates/mqk-execution/src/lib.rs:28-36`); IBKR adapter (crate exists, not a daemon dependency).
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

## Discrepancies found (items 1-3 documented only; item 4 fixed at the anchor)

1. `core-rs/crates/mqk-daemon/src/state/autonomous_completed_bar_driver.rs` header says the driver is not
   started from `core-rs/crates/mqk-daemon/src/main.rs`; `core-rs/crates/mqk-daemon/src/main.rs:146` spawns it for Paper+Alpaca (the header is stale).
2. `mqk-daemon` declares a dependency on `mqk-audit` (`core-rs/crates/mqk-daemon/Cargo.toml:51`) but never references it; audit rows
   go to Postgres `audit_events` via `mqk-db`.
3. `README_TECHNICAL.md` §2 cites an older accepted baseline (`fac22592`, CI #623) than this anchor.
4. **SAFETY DISCREPANCY (disclosed at `c81695de`, FIXED at `f4e4fb3`):** the launcher and docs described LiveShadow as no-order, but `POST /api/v1/execution/orders` had no deployment-mode gate and a hermetic proof enqueued a `PENDING` manual-order row under LiveShadow. LiveShadow now refuses every new-order admission seam server-side (see the mode and order-authority table above). Generic arm/run-start remain deliberately unfenced so LiveShadow can observe broker truth.

## Verification performed for this artifact

Archify `finalize` (validate, deliver, strict check, browser check) with `--repo-root` evidence verification;
desktop screenshot reviewed by the author (no crossings; one advisory 3-bend route, promotion → Postgres);
content check of all 37 component references; every path token in this README resolved at the anchor and every
line range lies inside its file, with no shorthand; secret-pattern scan; typed-source/HTML SHA-256 recorded in
the provenance file. The order-authority claims were read against source at the anchor; the cited tests
were executed against a local disposable Postgres during the fix mission (no broker, no network). Negative controls (isolated copies, discarded): anchor SHA changed,
missing or nonexistent, hash mismatches, HTML tamper, title or provenance relabelled authoritative,
unconditional LiveShadow "no-order"/"monitor-only" reintroduced, LiveShadow relabelled as order-submitting or
disabled, the manual-order discrepancy removed, actual broker submission falsely claimed, research edges to
orchestrator/broker/Postgres, component without sources, secret-shaped string, evidence line/path/end-line/
commit broken, a shifted evidence range, and shortened or nonexistent README evidence paths.

## Revision note

Revision 3 (previous version) superseded revision 2's statement that LiveShadow is "monitor-only / no-order".
That is true of the canonical launcher workflow, but it is not a proven daemon-wide invariant at the anchor:
strategy-originated LiveShadow orders fail closed at the promotion gate, while the generic arm/run-start routes
and the separate manual order route are not mode-fenced, and an existing hermetic proof enqueues a `PENDING`
manual-order row under LiveShadow. The diagram and README now state this as a disclosed safety discrepancy
(fourth discrepancy above) and do not claim or test actual broker submission. Revision 2 corrected the
broker-connectivity versus order-authority wording and made every README evidence path repo-root-relative.
The source anchor and topology are unchanged. This package is observational; it repairs nothing.

Revision 4 (anchor `f4e4fb3`) re-anchors the map after the LiveShadow no-new-order repair. The topology is unchanged;
only the LiveShadow order-authority claims, the affected source line citations and the anchor changed. The
previously disclosed discrepancy is kept as history and marked fixed. Revisions 1-3 below are unchanged in meaning.

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
