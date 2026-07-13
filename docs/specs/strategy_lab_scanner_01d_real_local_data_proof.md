# Strategy Lab Scanner 01D — Real Local-Data Proof

Patch group: `STRATEGY-LAB-COMPLETION-AND-SCANNER-FOUNDATION-01-COMBINED`
Patch: `STRATEGY-LAB-SCANNER-01D-REAL-LOCAL-DATA-PROOF-01`

HEAD at time of run: `2d4fbb38` (`cli: scan strategy candidates from
local bars`).

## 1. What command ran

Positive path — 1D universe, `swing_momentum` (the only strategy with
local data for every registry symbol per the Phase A audit, §4):

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli --bin mqk-cli -- `
  backtest scan-strategies `
  --registry .\config\instruments\equities.json `
  --bars-root .\exports\md_backup `
  --timeframe 1D `
  --strategy swing_momentum `
  --top 20 `
  --out-dir .\exports\strategy_scans
```

Honest-skip path — `5m`, `intraday_scalper` (the empty `exports/
md_backup/5m/` directory proven in Phase A):

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli --bin mqk-cli -- `
  backtest scan-strategies `
  --registry .\config\instruments\equities.json `
  --bars-root .\exports\md_backup `
  --timeframe 5m `
  --strategy intraday_scalper `
  --top 20 `
  --limit-symbols 10 `
  --out-dir .\exports\strategy_scans
```

The mission's suggested command used an `mqk research scan-strategies`
namespace; per the Phase A audit (§2, §8) this repo has no `mqk
research` namespace — every backtest/research tool lives under `mqk
backtest <subcmd>` — so the command was added there instead, per the
mission's own "use existing naming conventions if different" allowance.

## 2. What local data was used

The real, already-committed `exports/md_backup/1D/*.csv` tree (88
files) and the real, already-committed-empty `exports/md_backup/5m/`
directory (0 files). No synthetic or fixture data was used for this
phase — the CLI fixture tests in Phase C already prove the mechanism
against tiny synthetic fixtures; this phase proves it against the
repo's actual local data.

## 3. How many symbols resolved from registry

88 (all enabled equities in `config/instruments/equities.json`; no
`--limit-symbols` was applied on the 1D run). The 5m run applied
`--limit-symbols 10` to keep the honest-skip proof small and fast.

## 4. How many symbols scanned

1D run: 88 (`universe_count=88`).
5m run: 10 (`universe_count=10`).

## 5. How many candidates ranked

1D run: **88** (`ranked_count=88`, `skipped_count=0`) — every enabled
equity has enough local 1D bars (all comfortably above the scanner's
60-bar minimum; the smallest local file still has 1,187 bars) for
`swing_momentum` to produce a scoreable result.

5m run: **0** (`ranked_count=0`, `skipped_count=10`).

## 6. How many skipped

1D run: 0.
5m run: 10, all `missing_bars_file` (`data_missing`) — `skip_reason=missing_bars_file count=10`.

## 7. Top candidates table (1D / swing_momentum, top 10 of 20 printed)

| rank | symbol | score (alpha_pct) | total_return_pct | trade_count | win_rate_pct |
|------|--------|--------------------|-------------------|-------------|--------------|
| 1    | LCID   | 95.95              | -1.16             | 134         | 11.94        |
| 2    | PLUG   | 94.67              | -3.57             | 752         | 12.63        |
| 3    | CHPT   | 94.35              | -2.43             | —           | —            |
| 4    | RIVN   | 83.59              | -0.27             | —           | —            |
| 5    | LYFT   | 82.50              | -0.38             | —           | —            |
| 6    | TAN    | 77.62              | -1.04             | —           | —            |
| 7    | MARA   | 76.11              | -2.02             | —           | —            |
| 8    | BITO   | 74.71              | -0.11             | —           | —            |
| 9    | ICLN   | 62.69              | -0.18             | —           | —            |
| 10   | OPEN   | 50.56              | -0.23             | —           | —            |

Full 20-row printout, exact `candidates.csv`/`candidates.json`, and
`summary.json` are in the artifact directory (§9). Score is `alpha_pct`
(strategy return minus buy-and-hold benchmark) — every top-ranked
candidate here has a **negative** `total_return_pct` and beats its
benchmark only because the benchmark fell further (e.g. LCID:
`total_return_pct=-1.16%`, `benchmark_return_pct≈-97%` implied by
`alpha_pct=95.95`). This is exactly the kind of result the scanner is
supposed to surface honestly, not launder into a "winning strategy"
claim — see §14/§15.

## 8. Top skip reasons

1D run: none (`top_skip_reasons: []`).
5m run: `missing_bars_file` ×10 (100% of the scanned universe) — the
scanner did not silently drop these symbols or crash; it explicitly
counted and labeled every one.

## 9. Artifact path

1D run: `exports/strategy_scans/e64dfd8d-a249-536d-8b2f-010000c2af34/`
(`manifest.json`, `candidates.json`, `candidates.csv`, `summary.json`).

5m run: `exports/strategy_scans/46e77637-b931-5237-945d-542dc3254c58/`
(same four files).

Both `scan_id` values are the deterministic UUIDv5 described in Phase
A/B/C — re-running either command with the same inputs reproduces the
same `scan_id` and directory name.

## 10. Were artifacts staged?

**No.** `exports/` is already `.gitignore`d (`.gitignore:29` and
`:138`); `git status --short exports/` reports nothing for either run
directory. Neither `git add` nor any commit in this bundle touches
`exports/strategy_scans/`.

## 11. Were provider/broker/network calls made?

**No.** Both commands ran with no network access attempted by the CLI
process itself — `mqk backtest scan-strategies` never imports a
provider or broker client (confirmed at the source level in Phase B/C);
the only IO performed was reading the two local input trees and writing
the local output tree.

## 12. Were orders submitted?

**No.** No `oms_outbox`/`oms_inbox` row was written (no DB connection
was ever opened — the command doesn't link against a DB write path for
this feature), no broker adapter was invoked, and no paper or live order
of any kind was submitted.

## 13. Did 5m missing/partial data behave honestly?

**Yes.** Every one of the 10 scanned symbols was labeled
`truth_state=data_missing`, `reason_code=missing_bars_file` — not
silently skipped from the output, not defaulted to a fake score, and
not a crash. `ranked_count=0` / `skipped_count=10` is printed explicitly
in both the manifest and the summary.

## 14. What does this prove?

- The scanner can resolve a real registry universe (88 symbols) and
  evaluate it against real local bar data end to end, off-market, with
  no provider/broker/network/DB dependency.
- It produces a durable, reviewable artifact tree exactly matching the
  Phase A/B schema design.
- It honestly reports total data unavailability for an entire timeframe
  (5m) rather than fabricating a result.
- It does **not** claim any of the ranked 1D candidates represent a
  profitable or promotable strategy — every top-ranked candidate in this
  proof run has a negative absolute return; the ranking reflects
  relative (alpha-vs-benchmark) performance only, which is exactly what
  the scanner is documented to compute (Phase B `score` derivation) and
  is not, by itself, a trading signal.

## 15. What remains open

- No real 1H or 5m local bar data exists in this repo yet, so
  `mean_reversion`, `volatility_breakout`, `intraday_scalper`, and
  `intraday_short_scalper` remain unproven against real local data (only
  proven honestly-skipped, per §13). Closing that gap is a data-ingestion
  concern, not a scanner-core concern.
- The scanner's `swing_momentum` results above are **not** evidence of a
  promotable strategy — every top candidate lost money in absolute
  terms. No threshold, gate, or promotion logic was touched or implied
  by this proof; that determination remains entirely out of scope for
  this foundation patch.
- No daemon route or GUI surface reads scanner output yet (by design —
  see Phase A §6, CLI-first was the selected first surface).
