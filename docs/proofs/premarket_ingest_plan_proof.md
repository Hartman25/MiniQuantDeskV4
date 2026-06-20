# PREMARKET-INGEST-PLAN-PROOF-01 — CLOSED_LOCAL

Proof/integration patch. No trading/order/live path touched. Branch `main`,
HEAD at proof time `da749e9` ("test: isolate event risk and session window env
state"), following `f58b258` ("data: expose watchlist ingest plan").

Claim under proof: the daemon ingest-plan route, the premarket script's
`-SymbolsFromIngestPlan` mode, and the multi-symbol premarket readiness gate
all resolve the **same** required symbols/timeframe, from the **same**
source, with no fallback to the full instrument registry.

## 1. Why this closes with almost no code change

`required_symbols_for_freshness_gate_from_env` (the premarket readiness
gate's resolver, used by `system/preflight` and `autonomous/readiness`) is a
thin wrapper over `required_symbols_with_source_from_env`
([market_data_freshness.rs:559-561](../../core-rs/crates/mqk-daemon/src/market_data_freshness.rs#L559-L561)):

```rust
pub fn required_symbols_for_freshness_gate_from_env() -> Vec<RequiredSymbolTimeframe> {
    required_symbols_with_source_from_env().required
}
```

The ingest-plan route handler calls the same function directly
([routes/ingest.rs:2844-2845](../../core-rs/crates/mqk-daemon/src/routes/ingest.rs#L2844-L2845)):

```rust
pub(crate) async fn market_data_ingest_plan() -> impl IntoResponse {
    let resolution = required_symbols_with_source_from_env();
```

This is a structural guarantee, not a behavioral coincidence: the route and
the readiness gate cannot disagree about required symbols because they are
two callers of the identical Rust function. The route handler takes no
`State<AppState>` parameter at all — it cannot reach the DB, a provider
client, or a broker adapter even if it tried
([routes/ingest.rs:2840-2844](../../core-rs/crates/mqk-daemon/src/routes/ingest.rs#L2840-L2844)
doc comment: "Read-only. No DB, no provider/broker calls, no network
access."). It is mounted on the public, no-auth router
([routes.rs:338-343](../../core-rs/crates/mqk-daemon/src/routes.rs#L338-L343)).

No production code changed in this patch. One new cross-surface test was
added (§3) and this proof doc was added. Everything else was proof-only.

## 2. Source preference (shared by every caller)

1. An approved `watchlist-v2` artifact (`MQK_PAPER_WATCHLIST_PATH`) — every
   symbol in `artifact.symbols`, paired with the shared
   `MQK_STRATEGY_MD_TIMEFRAME`. Source label `watchlist_v2`.
2. Otherwise, the legacy single `MQK_STRATEGY_SYMBOL` /
   `MQK_STRATEGY_MD_TIMEFRAME` pair. Source label `env_strategy_symbol`.
3. Otherwise, an empty required-symbol list. Source label `none`. **The
   ~80-symbol instrument registry is never used as a fallback** — proven by
   `ip06_nothing_configured_is_not_configured_and_never_uses_instrument_registry`.

A configured-but-broken watchlist (missing file, invalid JSON, or
`approved_for_autonomous_paper=false`) degrades to source 2 or 3 and reports
`truth_state="degraded"` with an explanatory warning — it never silently
produces an empty-looking `"active"` result.

## 3. Proof type: static/source + scenario test (existing + new)

Pre-existing, unmodified by this patch:

- [`scenario_ingest_plan_01.rs`](../../core-rs/crates/mqk-daemon/tests/scenario_ingest_plan_01.rs)
  — IP-01..IP-11 (11 tests): legacy fallback, watchlist-v2 precedence,
  missing/invalid/not-approved watchlist degraded states, normalization,
  response shape, no-registry-fallback (IP-06), public/no-auth/no-DB mount
  (IP-10).
- [`scenario_premarket_data_readiness_gate_01.rs`](../../core-rs/crates/mqk-daemon/tests/scenario_premarket_data_readiness_gate_01.rs)
  — PMR-N/A/E/R/API/DB (25 tests): the same resolver, normalizer, and
  aggregator, plus `system/preflight` and `autonomous/readiness` integration
  (PMR-API01..03).

New in this patch — **IP-12**
(`ip12_ingest_plan_and_preflight_market_data_readiness_agree_on_required_symbols`,
added to `scenario_ingest_plan_01.rs`): configures one watchlist-v2 artifact
(`AAPL`, `MSFT`) plus a decoy legacy `MQK_STRATEGY_SYMBOL=SPY`, then calls
*both* `GET /api/v1/market-data/ingest-plan` (no-DB `AppState`) and
`GET /api/v1/system/preflight` (paper+alpaca `AppState`, real HTTP through
`routes::build_router`, in-process) against the identical environment, and
asserts:

- `ingest-plan.required_symbols` (sorted) `==`
  `preflight.market_data_readiness.required_symbols` (sorted) — both
  `["AAPL", "MSFT"]`.
- Neither surface leaks the decoy `SPY`.
- Every `preflight.market_data_readiness.per_symbol[].timeframe` equals
  `ingest-plan.timeframe`.

This closes the one gap the pre-existing suites left: each suite tested its
own surface thoroughly, but nothing previously called both surfaces in one
test against the same environment and diffed the result.

### Test run results

```
cargo test -p mqk-daemon --test scenario_ingest_plan_01
running 12 tests ... test result: ok. 12 passed; 0 failed; 0 ignored

cargo test -p mqk-daemon --test scenario_premarket_data_readiness_gate_01
running 25 tests ... test result: ok. 25 passed; 0 failed; 0 ignored

cargo check -p mqk-daemon
Finished `dev` profile [unoptimized + debuginfo] target(s)

cargo clippy -p mqk-daemon --test scenario_ingest_plan_01
zero new warnings (two pre-existing too_many_arguments warnings in
unrelated, untouched routes/ingest.rs functions)
```

No pre-existing test failures were encountered (PMR-DB01 skips gracefully
without `MQK_DATABASE_URL`; it was not run against a real DB in this step).

## 4. Proof type: script fail-closed (no daemon required)

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Prep-PremarketMarketData.ps1 -SymbolsFromIngestPlan -DaemonPort 1 -CheckOnly
```

Result: **exit code 1**.

```
[PREP] OK: Paper DB guard passed (port 5440 confirmed).
[PREP] OK: TWELVEDATA_API_KEY is configured (value not printed).
[PREP] OK: ALPACA_API_KEY_PAPER / ALPACA_API_SECRET_PAPER configured (values not printed).
=== Resolving symbols/timeframe from daemon ingest-plan (port 1) ===
[PREP] FAIL: Could not reach daemon ingest-plan route at http://127.0.0.1:1/api/v1/market-data/ingest-plan (Unable to connect to the remote server).
[PREP] FAIL: Start the daemon first, or omit -SymbolsFromIngestPlan to use -Symbols/-Timeframe directly.
```

Confirmed: nonzero exit, clear failure naming the unreachable route, no
fallback to the `AAPL` default (the script exits before `$symbolList` is
ever assigned), no DB connection attempted (only a string match on the
configured DB URL), no provider call, no credential value printed.

## 5. Proof type: local daemon (live, real HTTP)

Paper Postgres (`mqk-paper-postgres`, port 5440) was already running. The
daemon was started locally against it (process-local env vars only,
`.env.local` not modified) with a temporary watchlist-v2 fixture
(`MSFT`, `NVDA`) at a path outside the repo, plus a decoy
`MQK_STRATEGY_SYMBOL=SPY`.

### Safety incident during this step, and its correction

The first daemon start did **not** set `MQK_DAEMON_ADAPTER_ID`, on the
assumption that the daemon's default broker adapter is `paper` (non-Alpaca).
That assumption was wrong: `.env.local` in this working copy already
contains `MQK_DAEMON_ADAPTER_ID=alpaca` plus real
`ALPACA_API_KEY_PAPER`/`ALPACA_API_SECRET_PAPER` values, and `dotenvy`
loads any env var from `.env.local` that is not already set in the
process — which these were not, since this proof never set them.
`main.rs` unconditionally calls `spawn_alpaca_paper_ws_task` at boot
(independent of `start_execution_runtime`), and with deployment_mode=Paper
and broker_kind=Alpaca resolved, it opened a **real** WebSocket connection
to `wss://paper-api.alpaca.markets/stream`, authenticated, and subscribed to
the `trade_updates` channel. This was caught immediately from the daemon's
own boot log (`alpaca_ws: connecting` / `auth acknowledged` / `listen
acknowledged`).

**Impact**: paper environment only; no order was submitted; no
`start_execution_runtime` call was ever made (this WS task is independent
daemon-boot plumbing per `BRK-00R-05`, not the OMS/execution path); no
position or fill was affected. The connection was a passive
connect+authenticate+subscribe, open for under two seconds before discovery.

**Remediation**: the daemon was stopped immediately
(`mqk-daemon.exe` process killed, port 8899 reachability re-checked and
confirmed down). Root cause: `spawn_alpaca_paper_ws_task`
([state/alpaca_ws_transport.rs:117-123](../../core-rs/crates/mqk-daemon/src/state/alpaca_ws_transport.rs#L117-L123))
checks `deployment_mode == Paper && broker_kind == Alpaca` *before* it checks
credential presence — so the only reliable way to guarantee no WS attempt is
to force a non-Alpaca adapter, not to blank the credential env vars (which
`std::env::var` would still read as `Ok("")`, not `Err`, and the task would
still attempt to connect with empty credentials). The daemon was restarted
with `MQK_DAEMON_ADAPTER_ID=paper` forced explicitly, and the boot log was
checked **before** issuing any further request:

```
startup_truth: background task start outcomes (BOOT-VALID-01)
alpaca_ws_started=false session_controller_started=false bar_ticker_started=false
```

All subsequent route/script calls in this proof ran against this corrected,
verified-safe daemon instance.

### Route proof (real HTTP, `GET`, against the corrected daemon)

`GET /api/v1/market-data/ingest-plan`:

```json
{
  "canonical_route": "/api/v1/market-data/ingest-plan",
  "truth_state": "active",
  "symbol_source": "watchlist_v2",
  "required_symbols": ["MSFT", "NVDA"],
  "timeframe": "1D",
  "warnings": []
}
```

`SPY` (the decoy legacy symbol) does not appear — watchlist-v2 correctly
superseded it.

`GET /api/v1/system/preflight` (relevant fields, adapter forced to `paper`
for safety — see above):

```json
{ "daemon_mode": "paper", "adapter_id": "paper", "autonomous_readiness_applicable": false }
```

`market_data_readiness` is `null`, exactly as documented
(`PMR-API02`-equivalent behavior: the field is only populated for
paper+alpaca). The cross-surface symbol-equality claim for *this specific
gate* is proven safely in-process by IP-12 (§3), not by this live run —
enabling it live would have required the same Alpaca adapter that caused
the incident above.

### Script proof (real HTTP call from PowerShell to the real daemon)

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Prep-PremarketMarketData.ps1 -SymbolsFromIngestPlan -CheckOnly
```

Result: **exit code 0**.

```
=== Resolving symbols/timeframe from daemon ingest-plan (port 8899) ===
[PREP] OK: Ingest plan: source=watchlist_v2 truth_state=active symbols=MSFT, NVDA timeframe=1D
[PREP] Symbols: MSFT, NVDA  Timeframe: 1D
=== CHECK-ONLY: schema, bar counts, key presence (no mutations) ===
[PREP] OK: docker command available.
[PREP] OK: Paper DB is reachable (pg_isready OK).
[PREP] OK: md_bars table present.
[PREP] OK: MSFT/1D: completed=614  range=2024-01-02..2026-06-12  staleness=8d
[PREP] WARN:   -> stale by 8d (threshold=4d; sync needed)
[PREP] OK: NVDA/1D: completed=614  range=2024-01-02..2026-06-12  staleness=8d
[PREP] WARN:   -> stale by 8d (threshold=4d; sync needed)
=== CHECK-ONLY complete ===
[PREP] OK: CheckOnly passed (no mutations performed).
```

The script resolved `MSFT, NVDA` / `1D` from the live route — not its own
`AAPL` default, and not the decoy `SPY` — proving it genuinely consumes the
route rather than coincidentally matching a default. `-CheckOnly` performed
only read queries (`pg_isready`, `SELECT count`/`min`/`max` via
`docker exec ... psql`); no evidence file was written (confirmed via
`git ls-files --others` before/after: identical, pre-existing-only untracked
file list) and no `md ingest-csv`/`sync-provider`/`ingest-provider`
subprocess ran (those are gated behind the non-`CheckOnly` full-prep path).

### Cleanup

Daemon stopped (`mqk-daemon.exe` process killed); port 8899 reachability
re-checked and confirmed down; temporary watchlist fixture file removed.
Docker container `mqk-paper-postgres` was only ever read from
(`pg_isready`, `SELECT`) and was left running as found.

## 6. Safety confirmation

- No broker submit code touched or exercised.
- No live routing touched.
- No order/outbox/inbox rows written (`start_execution_runtime` was never
  called in this proof; no run was active).
- No DB migrations added.
- `.env.local` was not modified (read-only, by pre-existing script/daemon
  behavior this patch did not change).
- No secrets printed (script reports key/credential *presence* only).
- No short-entry path touched. No B5/risk gate touched.
- **Provider/broker calls**: the final, verified configuration used for
  every recorded route/script proof made zero provider or broker calls. One
  transient real Alpaca paper WS connect+auth+subscribe occurred earlier in
  this session due to an incorrect assumption about the daemon's default
  adapter (see §5); no order was submitted, no `start_execution_runtime`
  call occurred, and the daemon was stopped within seconds of the
  connection appearing in its own boot log. Documented here in full rather
  than omitted.
- No paper or live orders were submitted at any point in this session.

## 7. Verdict

**PREMARKET-INGEST-PLAN-PROOF-01 CLOSED_LOCAL.**

Route, script, and readiness gate are proven — by structural code identity,
by 37 pre-existing scenario-test assertions, by one new cross-surface
scenario test (IP-12), and by a live local daemon + real script run — to
agree on required symbols/timeframe and source, with no fallback to the
instrument registry and a clearly fail-closed unreachable-daemon path.
