# M1.9 — Paper deployment preflight (read-only)

Scope: the ordered, read-only checks that must pass before a promoted candidate is deployed to Paper.
Every check uses an existing seam. This runbook does not authorize applying migrations, arming, clearing
HALTED/DISARMED, deploying a strategy, starting a run or submitting any order, and Live stays disabled.
Counting and capture after deployment: `docs/runbooks/m1_10_finite_validation.md`.

A failed check is a STOP: record the exact observation, change nothing, and do not substitute a nearby
value. An unreachable seam is `UNAVAILABLE`, never "empty" or "ok".

## P0. Repository baseline (any machine)

| Check | Seam | Pass |
|---|---|---|
| Deploy SHA is the accepted SHA and GitHub CI is green on that exact SHA | `git rev-parse HEAD`; the CI run for it | equal; success |
| Migration chain ends at 0091 and matches the manifest | `bash scripts/guards/check_migration_governance.sh` | `manifest matches authoritative SQL chain`; last `manifest.json` entry is `0091_strategy_held_sizing_state` |
| Capital-fraction dispatch proofs | CI DB lane (`MQK_REQUIRE_DB_PROOF=1`, `:5434`, `check_cf_dispatch_db_proof.sh`) | green on the deploy SHA |

## P1. Environment (deployment host, no process started)

| Check | Pass |
|---|---|
| `MQK_DAEMON_DEPLOYMENT_MODE` | `paper` |
| `MQK_DATABASE_URL` after the launcher's PAPER DB HARD FENCE | host port `5440`, database `miniquantdesk_paper`; never `5432` (live) or `5434` (test) |
| `MQK_DAEMON_ADDR` | unset (loopback `127.0.0.1:8899`) |
| Live routing | no live-routing flag; no Live broker credentials in the Paper environment; `approved_for_live` is DB-constrained false in dynamic-selection evidence (migration 0059) |
| `MQK_STRATEGY_SIZING_POLICY` | selects the capital-fraction policy only for a capital-fraction candidate; no default strategy is configured (the deployed identity comes from the promotion registry, not `.env.local`) |
| Scheduler | `MiniQuantDesk-Paper-Preopen-Startup` Ready, Mon-Fri 02:00 HST (08:00 ET), `Start-MiniQuantDesk.ps1 -Mode Paper -Scheduled`; the older soak tasks Disabled |
| Host sleep / Modern Standby | disabled for the unattended window (operator action, recorded) |

## P2. Paper database (read-only session)

| Check | Query / seam | Pass |
|---|---|---|
| Reachable | `mqk db status` | `db_ok=true` |
| Migrations applied | `SELECT max(version) FROM _sqlx_migrations WHERE success` and `SELECT count(*) FROM _sqlx_migrations WHERE NOT success` | `91` and `0`. Last recorded real value was 76: applying 77-91 is a separate authorization through the canonical boot path (`mqk db migrate`), never done by this runbook |
| Held-sizing table | `SELECT to_regclass('sys_strategy_held_sizing_state') IS NOT NULL` | `true` (migration 0091) |
| Legacy residue | `SELECT count(*) FROM sys_autonomous_daily_operations WHERE state NOT IN (terminal states)`; stale `RUNNING` run rows | recorded as historical (2 stale rows were recorded 2026-10-04); none may belong to the candidate |

## P3. Authority state (daemon read-only routes, or the DB when the daemon is down)

| Check | Seam | Pass |
|---|---|---|
| Daemon preflight | `GET /api/v1/system/preflight` | `db_reachable` true; autonomous blockers listed truthfully (`InboundContinuityUnproven` while DISARMED is expected, not an error) |
| Autonomous readiness | `GET /api/v1/autonomous/readiness` | no blocker other than the explicitly pending operator actions |
| Risk and halt | `GET /api/v1/risk/summary`, `GET /api/v1/system/status` | not blocked; not HALTED. A HALTED or DISARMED state is cleared only by the operator's own arm procedure, never here |
| Reconcile | `GET /api/v1/reconcile/status`, `/mismatches` | status `ok`, zero mismatches |
| Promotions before the candidate | `GET /api/v1/strategy/promotions` | no identity in `active_paper` except the intended candidate; legacy unpromoted strategies (`intraday_scalper` and every other registry row) have no `active_paper` transition and stay inactive |

## P4. Candidate identity (only after Promotion)

| Check | Seam | Pass |
|---|---|---|
| Exact authority | `GET /api/v1/strategy/promotions/check?strategy_id=<id>&symbol=<SYM>&timeframe_secs=86400` | `current_state` is `active_paper` for that exact triple (Gate 3b needs the exact identity; a sibling symbol or strategy never satisfies it) |
| Fingerprint and stress contract | the Promotion route compares the registered semantic fingerprint and the registered stress contract server-side | the check above is tradable; no client-supplied expected value |
| Native calendar horizon | monthly engines (`monthly_*`) resolve month-ends through `us_equity_regular_sessions_v1`, which ends 2026-12-31 | a monthly candidate is deployable only while every bar it will see is before 2026-12-31; otherwise STOP (open item E1, needs OD-6). Non-monthly engines have no such horizon |
| Sizing | a capital-fraction candidate dispatches through `CapitalFractionRuntimeHost` (persist-before-act, recover once before the start barrier) | the deployment's `sys_strategy_held_sizing_state` rows are empty before the first entry and are never edited by hand |
| Strategy universe bound | `REGISTERED_STRATEGY_IDS` count equals `mqk_portfolio::MAX_STRATEGY_UNIVERSE` (a drift test enforces equality) | adding a new engine raises both in the same commit |

## P5. Deployment order (after P0-P4 pass; each step is its own authorization)

1. Apply migrations through the canonical boot path.
2. Re-run P2.
3. Record the accepted post-repair SHA and the deployed `(strategy_id, symbol, timeframe_secs, paper)` identity for the M1.10 policy.
4. Arm per `autonomous_paper_ops.md` after inbound continuity is proven; this is an operator action.
5. Let the scheduled pre-open start run; capture per `m1_10_finite_validation.md`.

## What this runbook proves and does not

It proves each prerequisite that can be read without a candidate, and names the one that cannot be
(`active_paper` for the exact identity, P4). It cannot prove market-hours behaviour: the unattended
pre-open start, the broker session and the 10 countable / 5 consecutive clean sessions need real market
days after a valid deployment.
