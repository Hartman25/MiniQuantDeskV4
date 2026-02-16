# Test Fixture Formats and Scenario Harness Skeleton (V4)

This spec defines the canonical fixture formats and the minimal Rust test harness layout
for golden scenario tests and contract tests.

Design goals:
- few fixtures, reused everywhere
- deterministic, stable parsing
- zero “magic” implicit fields

---

## 1) Bar Fixture CSV Format

File location:
- `tests/fixtures/bars/*.csv`

Each bar CSV represents:
- **one instrument**
- **one timeframe**
- **one data source**
You pass instrument/timeframe/source as parameters from the test.

### 1.1 Required header

CSV header MUST be exactly:

`ts_close_utc,open,high,low,close,volume`

### 1.2 Types
- `ts_close_utc`: RFC3339 timestamp in UTC, e.g. `2026-01-05T21:00:00Z`
- `open/high/low/close`: decimal (string parse)
- `volume`: integer or decimal (string parse), >= 0

### 1.3 Invariants (validated by loader)
- strictly increasing `ts_close_utc`
- OHLC structural validity:
  - low <= min(open, high, close)
  - high >= max(open, low, close)
- no duplicate timestamps

### 1.4 Example row
`2026-01-05T21:00:00Z,470.12,471.05,469.80,470.90,125003`

### 1.5 Metadata
No metadata lines inside CSV.
Tests provide:
- instrument_id (or symbol)
- timeframe (e.g. 1H)
- calendar (e.g. NYSE)
- expected interval

---

## 2) Broker Snapshot JSON Format

File location:
- `tests/fixtures/broker/*.json`

This is the canonical input to reconciliation tests.

### 2.1 JSON schema (minimal v0)

```json
{
  "captured_at_utc": "2026-01-05T21:00:00Z",
  "account": { "equity": "10000.00", "cash": "5000.00", "currency": "USD" },
  "orders": [
    {
      "broker_order_id": "B123",
      "client_order_id": "MAIN_...",
      "symbol": "SPY",
      "side": "BUY",
      "type": "MARKET",
      "status": "NEW",
      "qty": "10",
      "limit_price": null,
      "stop_price": null,
      "created_at_utc": "2026-01-05T21:00:00Z"
    }
  ],
  "fills": [
    {
      "broker_fill_id": "F123",
      "broker_order_id": "B123",
      "client_order_id": "MAIN_...",
      "symbol": "SPY",
      "side": "BUY",
      "qty": "10",
      "price": "470.90",
      "fee": "0.10",
      "ts_utc": "2026-01-05T21:00:01Z"
    }
  ],
  "positions": [
    { "symbol": "SPY", "qty": "10", "avg_price": "470.90" }
  ]
}
```

Notes:
- all money/price fields are strings for decimal safety
- `client_order_id` prefix is the engine namespace anchor (`MAIN_` / `EXP_`)
- missing or unknown broker orders are detected via reconcile diff

---

## 3) Fill Duplicate JSONL Format

File location:
- `tests/fixtures/fills/*.jsonl`

One JSON object per line:

```json
{"broker_fill_id":"F123","broker_order_id":"B123","client_order_id":"MAIN_...","symbol":"SPY","side":"BUY","qty":"10","price":"470.90","fee":"0.10","ts_utc":"2026-01-05T21:00:01Z"}
```

Used to verify:
- inbox dedupe by `broker_fill_id`
- ledger fill idempotency

---

## 4) Golden Scenario Harness (Rust) — Minimal Layout

### 4.1 Workspace layout (proposed)
- `core-rs/` Rust workspace
  - `crates/mqk-testkit/`  (test utilities + fixture loader)
  - `crates/mqk-backtest/` (parity backtester runner)
  - `crates/mqk-runtime/`  (event loop skeleton)
  - `crates/mqk-schemas/`  (types: envelope, intents, orders, fills)
  - `crates/mqk-db/`       (repos + embedded migration runner)
  - `crates/mqk-cli/`      (CLI)

For now we provide only a **compile-ready skeleton** for mqk-testkit and a scenario test module.

### 4.2 Testkit responsibilities
- load bar CSV fixtures -> Vec<Bar>
- load broker snapshot fixture -> BrokerSnapshot
- run a parity backtest scenario:
  - feed bars into runtime/backtester
  - collect emitted orders/fills
  - export artifacts to temp dir
- assert helpers:
  - assert protective stop exists after entry
  - assert trailing stop monotonic (never loosens)
  - assert deterministic replay matches (hash compare)

### 4.3 Scenario test pattern
Each scenario test should:
1) load fixture
2) run scenario with config overlay
3) assert invariants
4) (optionally) compare against golden artifacts

---

## 5) Golden Artifact File Formats

Artifacts directory per run:
- `orders.csv` (stable schema)
- `fills.csv` (stable schema)
- `equity_curve.csv`
- `metrics.json`
- `audit.jsonl`

Golden tests compare:
- exact content for stable ids
- for float fields: compare as decimals or within tolerance (configurable)

---

## 6) Fixture Governance Rules
- Keep <= 10 core fixtures for Phase 1.
- New fixture allowed only if it replaces multiple ad-hoc datasets.
- Never embed secrets or real broker account ids in fixtures.
