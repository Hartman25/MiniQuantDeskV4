# REGISTRY-V2-KRAKEN-LIVE-PROVIDER-PROOF-01 — Closure Decision

Patch ID: `REGISTRY-V2-KRAKEN-LIVE-PROVIDER-PROOF-01`

Proof/evidence patch. Executes exactly the bounded live proof named by
`REGISTRY-V2-LIVE-PROVIDER-01B-FIRST-PROOF-BOUNDARY-DECISION-01` and
`REGISTRY-V2-LIVE-PROVIDER-01D-CLOSURE-AND-ROADMAP-RECONCILE-01`, after the
operator gave the exact required authorization phrase:

> I explicitly authorize ONE bounded Kraken public OHLC live-network proof
> for BTC/USD and ETH/USD into an isolated test/proof database only. Do
> not enable trading, do not use credentials, do not touch paper/live
> broker routing, and stop after evidence.

No source code was changed to perform this proof — the command and
network/DB gates already existed (`REGISTRY-V2-LIVE-PROVIDER-01A` §6).

---

## 1. What was run

```text
MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1
MQK_DATABASE_URL=postgres://postgres:postgres@localhost:5434/mqk_test
mqk-cli md kraken-ohlc-ingest --registry config/instruments/instruments_v2.crypto_local_marks.example.json --symbol BTC/USD --timeframe 1D --output-dir exports/live_provider_proof
mqk-cli md kraken-ohlc-ingest --registry config/instruments/instruments_v2.crypto_local_marks.example.json --symbol ETH/USD --timeframe 1D --output-dir exports/live_provider_proof
```

Two invocations, one per symbol, exactly as `01B` §4 specified. No
`--input-file` was given, so both made exactly one live HTTP GET each to
`https://api.kraken.com/0/public/OHLC` (Kraken's public endpoint; no
credentials sent, none required). `.env.local` was not read or modified by
this proof.

## 2. DB target used

`postgres://postgres:postgres@localhost:5434/mqk_test` — confirmed by
direct `psql` query (`SELECT current_database()`) to be `mqk_test`, the
existing local isolated test/proof database, **distinct** from the paper
database (`localhost:5440/miniquantdesk_paper`). Per `01B` §5's
requirement, the paper database's `md_bars` table was queried
post-proof and shows **zero** rows for `BTC/USD`/`ETH/USD` — confirming
isolation held.

## 3. Evidence produced

Two evidence files, written to the git-ignored `exports/live_provider_proof/`
directory per `01B` §6 (not staged, not committed):

- `kraken_ohlc_ingest_1783542427.json` (BTC/USD)
- `kraken_ohlc_ingest_1783542437.json` (ETH/USD)

Both files match every required field from `01B` §9:

| Field | BTC/USD | ETH/USD |
|---|---|---|
| `mode` | `"network_smoke"` | `"network_smoke"` |
| `network_call_made` | `true` | `true` |
| `db_write` | `true` | `true` |
| `md_bars_write` | `true` | `true` |
| `bars_completed` | `720` | `720` |
| `provider_id` / `provider_source` | `"kraken"` / `"kraken"` | `"kraken"` / `"kraken"` |
| `provider_symbol` | `"XXBTZUSD"` | `"XETHZUSD"` |
| `forming_candle_excluded` | `true` (1 excluded) | `true` (1 excluded) |
| `rows_inserted` / `rows_updated` | `720` / `0` | `720` / `0` |

Independently confirmed via direct `psql` query against `mqk_test`:

```text
symbol  | timeframe | provider_id | provider_source | ingest_mode     | count | min        | max
BTC/USD | 1D        | kraken      | kraken           | provider_ingest | 720   | 1721347200 | 1783468800
ETH/USD | 1D        | kraken      | kraken           | provider_ingest | 720   | 1721347200 | 1783468800
```

Both symbols show real, non-fixture completed bars reaching `md_bars`
through the unmodified canonical
`ingest_provider_bars_to_md_bars_with_provider_metadata` write path, with
truthful `provider_id`/`provider_source` = `"kraken"` (not `"unknown"` or
a fixture label).

## 4. What must remain false/unchanged (per `01B` §10) — verified

- `config/providers/providers.json`'s `kraken.enabled` — **unchanged**;
  `git diff --name-only` shows this file was not modified.
- Both crypto registry-v2 fixture rows' `enabled`/`paper_trading_enabled`/
  `live_trading_enabled` — **unchanged**; same file, not modified.
- No Windows Scheduled Task was registered — confirmed via
  `Get-ScheduledTask -TaskName "*Kraken*"`, zero results.
- No recurring `kraken-ohlc-sync` invocation occurred — only the two
  one-shot `kraken-ohlc-ingest` calls above.
- No `mqk-runtime`/`mqk-execution`/`mqk-risk`/`mqk-broker-alpaca` file was
  touched or began consuming the newly-written `md_bars` rows.

## 5. Post-proof no-cutover check (per `01B` §11)

- `git diff --name-only` (pre-proof commit `517773e4` → current working
  tree) touches no file outside `docs/` — this closure decision doc is the
  only new tracked file this patch adds.
- The proof database (`mqk_test`, port 5434) is distinct from the paper
  database (`miniquantdesk_paper`, port 5440) by connection string; the
  paper database's `md_bars` table shows zero new Kraken-sourced rows.
- `config/providers/providers.json` and
  `config/instruments/instruments_v2.crypto_local_marks.example.json` are
  byte-identical to their pre-proof state (`git diff` shows no changes to
  either).
- No scheduled task exists.

## 6. Is prerequisite #4 closed?

**Yes.** `ASSET-CORE-01H` §5 prerequisite #4 reads: "At least one
non-equity market-data provider live-network-verified (not just
fixture/CSV-proven) end-to-end into `md_bars`." This proof performed a
real live network call to Kraken's public OHLC endpoint for both `BTC/USD`
and `ETH/USD`, and real (non-fixture) completed bars reached `md_bars`
through the existing, unmodified, guarded CLI/DB write path, with evidence
confirming every field `01B` §9 required. This closes prerequisite #4 for
the scope named: one non-equity provider (Kraken), proven end-to-end into
`md_bars`, in an isolated proof database.

This does **not** imply production readiness — see §7.

## 7. What remains before production cutover

1. ~~`BACKTEST-MULTIPLIER-MARGIN-01` closed~~ — satisfied.
2. ~~Symbol/`instrument_id` translation layer~~ — satisfied.
3. ~~Gate 0 / broker-submit routing-guard parity~~ — satisfied.
4. ~~Live-network non-equity provider proof~~ — **now satisfied by this
   patch.**
5. An explicit operator decision to enable `enabled=true` for a specific,
   named non-equity instrument — **still open**. This proof does not
   constitute that decision: `kraken.enabled`, `BTC/USD.enabled`, and
   `ETH/USD.enabled` all remain `false` in the committed config. Enabling
   any of them requires a separate, explicit operator decision naming the
   instrument, not inference from this proof's success.

`REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` remains blocked on
prerequisite #5 alone and is still **not** recommended next — one
prerequisite open is still open; a production-cutover decision patch
should be written once #5 is explicitly taken, not preemptively.

## 8. What was deliberately not done

- No credentials were used or provisioned (Kraken's public endpoint
  requires none).
- `.env.local` was not read or modified.
- No config flag was changed — `kraken.enabled` and both crypto fixture
  rows' enablement flags remain `false`.
- No trading was enabled (paper or live).
- No scheduler/task registration occurred.
- No broker/execution/risk/OMS/runtime/strategy/portfolio code was
  touched.
- The evidence JSON files and the DB rows in `mqk_test` are not staged or
  committed to git — `exports/` is git-ignored, and the isolated proof
  database is not part of the repo.
- Prerequisite #5 was not attempted, decided, or inferred.

---

## Closure decision

```text
REGISTRY-V2-KRAKEN-LIVE-PROVIDER-PROOF-01 is CLOSED_LOCAL / LIVE-PROOF-COMPLETE.
Prerequisite #4 of ASSET-CORE-01H's production-cutover checklist is now
CLOSED: a live Kraken OHLC network call reached md_bars for both BTC/USD
and ETH/USD (720 completed bars each) through the existing, unmodified,
guarded CLI/DB write path, into an isolated test/proof database
(mqk_test), never paper/live. No credentials used, no trading enabled, no
config flags changed, no scheduler registered, no production cutover.
Prerequisite #5 (explicit operator enablement) remains OPEN.
REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01 remains blocked on prerequisite
#5 alone.
```

**Recommended next patch:** an explicit operator-enablement decision patch
for prerequisite #5 (naming exactly which instrument, if any, the operator
wants to enable) — only if and when the operator makes that decision
explicitly. Absent that, no further registry-v2 boundary work is
recommended; `ASSET-CORE-05`'s independent per-instrument session-routing
gap remains available as a lower-risk parallel track untouched by this
boundary.
