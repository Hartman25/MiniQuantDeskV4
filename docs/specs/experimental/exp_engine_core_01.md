# EXP-ENGINE-CORE-01 — Experimental Engine Foundation

**Status: FOUNDATION LANDED / SCANNER-ONLY / DISABLED BY DEFAULT**
**Lane: EXP (research only; not operational truth)**
**Patch ID: EXP-ENGINE-CORE-01**

---

## Purpose

EXP-ENGINE-CORE-01 adds a disabled-by-default experimental engine foundation to
the MiniQuantDesk V4 research infrastructure. It provides the scaffold that
future scanner lanes (e.g. EXP-PENNY-01A) will build on.

This is a **scanner-only, journal-first foundation**. It cannot place orders, call
broker endpoints, or write to any OMS/execution table. Those capabilities do not
exist in this foundation and are not wired anywhere in the codebase.

---

## Hard Boundaries (enforced structurally, not by config)

- **No order submission.** `paper_order_id` and `live_order_id` are always `null`
  in Stage 1. This is set in `candidate_journal.build_candidate_record`, not a
  config flag — it cannot be toggled off.
- **No OMS/outbox access.** The exp_engine package imports nothing from
  `oms_outbox`, `oms_inbox`, or any OMS table. A static import guard in
  `test_exp_engine_safety.py` proves this on every test run.
- **No broker adapter access.** No imports from `mqk_broker`, `BrokerGateway`,
  `alpaca`, or any broker adapter. Proven by the same static guard.
- **No execution orchestrator access.** No imports from `mqk_execution`,
  `mqk_runtime`, or `ExecutionOrchestrator`.
- **Disabled by default.** `MQK_EXPERIMENTAL_ENGINE_ENABLED` defaults to `false`.
  The runner returns exit code 1 immediately if the flag is absent or false.
- **scanner_only mode only.** `MQK_EXPERIMENTAL_ENGINE_MODE` must be
  `scanner_only`. Any other value raises `ValueError` before any scan runs.
- **live_allowed is always false.** `MQK_EXPERIMENTAL_ENGINE_LIVE_ALLOWED=true`
  raises `ValueError` on `config.validate()`. This is not a soft warning.
- **--dry-run required.** The scanner runner requires `--dry-run` as a positional
  flag. It will not run without it.

---

## Files Added

| File | Purpose |
|------|---------|
| `research-py/experiments/exp_engine/__init__.py` | Package marker |
| `research-py/experiments/exp_engine/config.py` | Config gate; reads `MQK_EXPERIMENTAL_ENGINE_*` env vars |
| `research-py/experiments/exp_engine/candidate_journal.py` | JSONL candidate writer; always null order IDs |
| `research-py/experiments/exp_engine/scanner_base.py` | Abstract base class for scanner lanes |
| `research-py/experiments/exp_engine/scanner_runner.py` | Disabled-by-default runner; requires `--dry-run` |
| `research-py/experiments/exp_engine/tests/test_exp_engine_safety.py` | 12 safety + unit tests; static import guard |
| `exports/experimental/candidates/.gitkeep` | Output directory for candidate JSONL files |
| `docs/specs/experimental/exp_engine_core_01.md` | This document |
| `tests/script_guards/test_exp_engine_guard.ps1` | PowerShell static guard |
| `.env.local.example` | Added `MQK_EXPERIMENTAL_ENGINE_*` flags (all disabled) |

---

## Config Flags

All flags are absent-or-false in all default configs. No default enables the engine.

| Variable | Default | Notes |
|----------|---------|-------|
| `MQK_EXPERIMENTAL_ENGINE_ENABLED` | `false` | Must be `true` to run the scanner |
| `MQK_EXPERIMENTAL_ENGINE_MODE` | `scanner_only` | Only valid value in Stage 1 |
| `MQK_EXPERIMENTAL_ENGINE_LIVE_ALLOWED` | `false` | Must remain false; raises if set true |
| `MQK_EXPERIMENTAL_JOURNAL_DIR` | `exports/experimental/candidates` | Output path for JSONL files |

---

## Candidate Journal Schema

Schema version: `exp-candidate-v1`

```json
{
  "schema_version": "exp-candidate-v1",
  "engine_id": "<string>",
  "lane_id": "<string>",
  "strategy_id": "<string>",
  "scanned_at_utc": "<ISO8601>",
  "symbol": "<ticker>",
  "asset_class": "<string>",
  "candidate_type": "<string>",
  "signal_direction": "<long | short | neutral>",
  "confidence_score": "<float | null>",
  "risk_notes": "<string>",
  "rejection_reason": "<string | null>",
  "would_trade": "<bool>",
  "paper_order_id": null,
  "live_order_id": null
}
```

`paper_order_id` and `live_order_id` are always `null` in Stage 1. They are not
optional fields that future code can fill in without a separate Stage 2 promotion
review and schema version bump.

---

## Output Location

Candidate JSONL files are written to:

```
exports/experimental/candidates/<engine_id>_<YYYYMMDD_HHMMSS>.jsonl
```

The `exports/` directory is gitignored. Candidate journals are local only and are
not committed to the repo. The `.gitkeep` file ensures the directory path exists
in the repo without committing runtime output.

---

## Static Safety Guard

`tests/script_guards/test_exp_engine_guard.ps1` proves at CI time:

- `MQK_EXPERIMENTAL_ENGINE_ENABLED` is absent or `false` in `.env.local.example`
- `MQK_EXPERIMENTAL_ENGINE_LIVE_ALLOWED` is absent or `false` in `.env.local.example`
- No file in `research-py/experiments/exp_engine/` (excluding test files) references
  `oms_outbox`, `BrokerGateway`, `broker_adapter`, or `Start-PaperTradingSmoke`
- `paper_order_id` in `candidate_journal.py` is always set to `None`

---

## Relationship to EXP-PENNY-01

EXP-PENNY-01 (`exp_penny_01_breakout_accumulation.md`) is the first planned scanner
lane built on this foundation. Its Stage 1 implementation (EXP-PENNY-01A) will:

- Subclass `ScannerBase`
- Use `build_candidate_record` from `candidate_journal`
- Use the penny-specific journal schema from `exp_penny_01_breakout_accumulation.md`
- Not place any orders

EXP-PENNY-01A cannot land until EXP-ENGINE-CORE-01 is committed. That ordering is
now satisfied.

---

## What Is NOT Provided by This Foundation

- No market data feed integration (that is EXP-PENNY-01A scope)
- No strategy signal logic (EXP-PENNY-01A scope)
- No paper order routing (Stage 3 of EXP-PENNY-01, requires Stage 2 gate pass)
- No GUI panels or readiness claims
- No DB migrations
- No connection to the main execution orchestrator
- No changes to AAPL/5m intraday_scalper behavior

---

## Next Lanes

| Lane ID | Description |
|---------|-------------|
| EXP-PENNY-01A | Penny scanner Stage 1 implementation; subclasses ScannerBase |
| EXP-ENGINE-CORE-02 | Strategy registry for experimental lanes |
| AAPL-5M-NATURAL-SMOKE | Natural market-hours smoke for AAPL/5m (main engine) |

---

## Explicit Non-Goals

- Does not affect AAPL/5m intraday_scalper.
- Does not change broker adapter, WS inbound path, OMS, reconcile, or risk logic.
- Does not add default config that enables any experimental execution.
- Does not generate GUI readiness claims.
- Does not add DB migrations.
