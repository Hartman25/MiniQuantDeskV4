# REGISTRY-V2-LIVE-PROVIDER-01A — Current Non-Equity Provider Audit

Patch ID: `REGISTRY-V2-LIVE-PROVIDER-01A-CURRENT-NON-EQUITY-PROVIDER-AUDIT-01`

Audit-only. No code, no network call, no DB access. Written to ground
`ASSET-CORE-01H`'s production-cutover prerequisite #4 —

> At least one non-equity market-data provider live-network-verified
> (not just fixture/CSV-proven) end-to-end into `md_bars`.

— in current repo evidence before any boundary decision (`01B`) or
preflight guard (`01C`) is written. This patch does not attempt the live
proof itself.

---

## 1. Current HEAD and relevant prerequisite commits

- HEAD at the start of this patch: `e53289ff` (`docs: close registry v2
  gate parity`).
- Prerequisite #1 (`BACKTEST-MULTIPLIER-MARGIN-01`): `CLOSED_LOCAL /
  BACKTEST-COMPLETE`, closed at `e3f2f77e`→`2da681e9`.
- Prerequisite #2 (`REGISTRY-V2-TRANSLATION-01A`-`01D`): `CLOSED_LOCAL`,
  closed at `66b617f9`/`f9032c2a` (see
  [registry_v2_translation_01d_closure_decision.md](registry_v2_translation_01d_closure_decision.md)).
- Prerequisite #3 (`REGISTRY-V2-GATE-PARITY-01A`-`01D`): `CLOSED_LOCAL`,
  closed at `f907f65f`/`a3fb7fc5`/`e53289ff` (see
  [registry_v2_gate_parity_01d_closure_decision.md](registry_v2_gate_parity_01d_closure_decision.md)).
- Prerequisite #4 (this patch's subject) and #5 remain open per both
  closure decisions above and
  [roadmap_completion_reconcile_01.md](roadmap_completion_reconcile_01.md)
  §2's `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` row.

## 2. Current status of all non-equity provider/data proof candidates

Evidence source: `config/providers/providers.json`,
`core-rs/crates/mqk-md/src/providers/*.rs`,
`core-rs/crates/mqk-md/src/provider_registry.rs`,
`config/instruments/instruments_v2.crypto_local_marks.example.json`.

| Candidate | `providers.json` status | Asset classes it actually implements | Credentials | Live-network-verified? |
|---|---|---|---|---|
| **Kraken** (`kraken`) | `enabled: false`, `implementation_status: "ohlcv_adapter_fixture_proven_network_opt_in_only"`, `verification_status: "fixture_parser_proven_live_network_not_exercised_by_this_patch"` | Crypto spot OHLCV, `BTC/USD`/`ETH/USD` only, `1D` timeframe only (`kraken_query_pair_for_canonical_symbol` in `core-rs/crates/mqk-md/src/providers/kraken.rs` hard-matches exactly these two strings) | None (`api_key_required: false`, `credential_env_vars: []`) | **Partially** — `CRYPTO-DATA-01S-T` performed one bounded, explicitly-authorized live verification GET per symbol (both returned real 721-row OHLC payloads, documented in `docs/specs/crypto_data_01s_t_ohlcv_provider_decision_verify.json`), but that verification did **not** write to `md_bars` — it was a read-only parse/decision proof, not an end-to-end DB ingest. No live-network run has ever reached `md_bars`. |
| **CoinLore** (`coinlore`) | `enabled: false`, `implementation_status: "latest_mark_parser_implemented_bar_provider_not_applicable"` | Latest-mark ticker only — `verification_status` explicitly states the `/api/tickers/`/`/api/ticker/?id=` endpoints expose "no OHLCV history and no per-ticker timestamp -- not usable as a HistoricalProvider bar source" | None | Yes, for ticker-only reads (`CRYPTO-DATA-01I`), but structurally **cannot** produce `md_bars` rows — it is not a `HistoricalProvider`/bar source at all, so it cannot close prerequisite #4 regardless of further work. |
| **TwelveData** (`twelvedata`) | `enabled: true`, `implementation_status: "implemented_equity_provider"` | `providers.json` lists `asset_classes: ["equity", "etf", "forex", "crypto"]`, but `provider_registry.rs`'s own regression test (`PR-09`) proves the concrete provider construction excludes futures/options, and no test or CLI path in the repo exercises TwelveData against a crypto or forex symbol — the crypto/forex entries in the config array are unverified aspirational metadata, not an implemented, tested code path | Required (`TWELVEDATA_API_KEY`) | No — never network-verified for any non-equity symbol; would additionally require credential provisioning this patch is forbidden from touching. |
| **Alpaca** (`alpaca`) | `enabled: true`, `implementation_status: "implemented_equity_provider"` | `asset_classes: ["equity", "etf"]` only. `docs/audits/multi_asset_completion_audit.md`'s `CRYPTO-EXEC-01` row confirms by direct source read that `mqk-broker-alpaca/src/lib.rs` "never calls `/v2/crypto/*`" | Required (`ALPACA_API_KEY_PAPER`/`ALPACA_API_SECRET_PAPER`) | Not applicable — no non-equity asset class is implemented at all. |
| **Alpha Vantage** (`alphavantage`) | `enabled: false`, `implementation_status: "candidate_unverified"` | Config-only entry; no adapter source file exists in `core-rs/crates/mqk-md/src/` | Required | No — no code exists to run. |
| **Polygon.io** (`polygon`) | `enabled: false`, `implementation_status: "candidate_unverified"` | Config-only entry; no adapter source file exists | Required | No — no code exists to run. |
| **yfinance** (`yfinance`) | `enabled: false`, `implementation_status: "candidate_unverified"` | Config-only entry; no adapter source file exists | None | No — no code exists to run. |

## 3. Why Kraken is the safest / most complete candidate

- It is the only candidate with a real, tested `HistoricalProvider` adapter
  (`KrakenHistoricalProvider` in `core-rs/crates/mqk-md/src/providers/kraken.rs`)
  that parses actual Kraken response shapes, excludes the forming
  (not-yet-committed) candle structurally (`row.time <= result.last`, never
  inferred), and has an existing, tested, DB-writing CLI command
  (`mqk md kraken-ohlc-ingest`, `core-rs/crates/mqk-cli/src/commands/md.rs`)
  that already writes completed bars to `md_bars` via the canonical
  `mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata` path
  — the same write path every other provider ingest command uses, not a new
  or parallel write path.
- It requires **no credentials** (`api_key_required: false`), so a future
  live proof cannot be blocked by, or accidentally touch, `.env.local` or
  any secret.
- Its network-call surface is already fail-closed by construction: both
  `kraken-ohlc-dry-run` and `kraken-ohlc-ingest` require `--input-file` by
  default and refuse to make any network call unless the operator
  explicitly sets `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1` (see
  `ENV_ALLOW_KRAKEN_NETWORK_SMOKE` in `md.rs`). No new gate needs to be
  built for the eventual live proof — it already exists and is already
  tested to fail closed (`scenario_cli_kraken_ohlc_ingest_db_01xy.rs`).
- One bounded live verification of the raw Kraken endpoint has already
  happened once before (`CRYPTO-DATA-01S-T`), so the endpoint shape,
  volume-scaling convention, and forming-candle-exclusion logic are already
  proven against real Kraken data — the only remaining gap is that no
  live run has gone through the ingest command into `md_bars`.
- CoinLore cannot close prerequisite #4 under any amount of further work,
  since it is not a bar/OHLCV source. TwelveData and Alpaca would require
  both new credential provisioning (forbidden by every session's hard
  safety rules around `.env.local`/secrets) and net-new non-equity code
  paths that do not exist today. Alpha Vantage/Polygon/yfinance have zero
  implemented adapter code.

## 4. Which symbols are eligible for proof

- `BTC/USD` and `ETH/USD` — the only two symbols
  `kraken_query_pair_for_canonical_symbol` maps to a Kraken query pair
  (`XBTUSD`, `ETHUSD` respectively), and the only two symbols present in
  the disabled registry-v2 fixture
  (`config/instruments/instruments_v2.crypto_local_marks.example.json`)
  carrying `provider_symbols.kraken_pair`/`kraken_result_key` aliases.

## 5. Which symbols are not eligible

- Every symbol other than `BTC/USD`/`ETH/USD` — `kraken_query_pair_for_canonical_symbol`
  returns `None` for anything else (`ks02_unrecognized_symbol_maps_to_none`
  test), and `kraken_aliases_from_registry_v2` only extracts aliases for
  instruments that explicitly carry `provider_symbols.kraken_pair` — no
  other registry-v2 row does.
- Any timeframe other than `1D` — both `KrakenHistoricalProvider::fetch_bars`
  and `md_kraken_ohlc_ingest` reject any `timeframe` other than `D1`
  before making a network call (`kh02_fetch_bars_rejects_unsupported_timeframe`).

## 6. Which command/path would eventually be used

`mqk md kraken-ohlc-ingest --registry <path> --symbol <BTC/USD|ETH/USD>
--timeframe 1D [--output-dir <dir>]`, with `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1`
set and no `--input-file` given (its only network-triggering configuration).
This command already:

- Resolves the Kraken alias from a registry-v2 document (`--registry`).
- Performs at most one HTTP GET to `https://api.kraken.com/0/public/OHLC`
  when the network opt-in is set and no `--input-file` is given.
- Parses the response, excludes the forming candle, and writes only
  completed bars to `md_bars` via
  `mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata`,
  stamped with truthful `provider_id="kraken"` metadata.
- Connects to whatever database `mqk_db::connect_from_env()` resolves
  (i.e. whatever `MQK_DATABASE_URL` points to at invocation time — see
  §6 of `01B` for why this must be an isolated proof DB, never
  paper/live, by default).
- Prints and optionally writes an evidence JSON containing
  `network_call_made`, `db_write`, `md_bars_write`, `mode`, and bar counts
  — exactly the fields prerequisite #4's closure evidence needs.

## 7. What is still fixture-only or disabled

- The `kraken` provider entry in `config/providers/providers.json` remains
  `"enabled": false` — no production ingestion/scheduling path selects it.
- Both registry-v2 fixture rows (`BTC/USD`, `ETH/USD`) remain
  `"enabled": false`, `"paper_trading_enabled": false`,
  `"live_trading_enabled": false` in
  `instruments_v2.crypto_local_marks.example.json`.
- No recurring sync or Windows Scheduled Task is registered — the existing
  `Register-KrakenOhlcSyncTask.ps1`/`Run-KrakenOhlcSync.ps1` scripts exist
  but registration is a separate, still-not-taken operator action per
  `CRYPTO-DATA-03C`'s closure note (memory:
  `project_crypto_data_03c_kraken_scheduler_task_status_surface_01.md`).
- `kraken-ohlc-ingest` has never been run in `network_smoke` mode against
  the live Kraken endpoint in this repo's history — every existing test
  (`scenario_cli_kraken_ohlc_ingest_db_01xy.rs`, etc.) uses `--input-file`
  fixtures or `httpmock`, never a real network call.

## 8. Why this patch cannot close prerequisite #4 by itself

Prerequisite #4 requires a **live-network-verified** proof reaching
`md_bars`. This patch performs zero network calls and zero DB writes (see
§9). It only identifies which candidate and command are safest to use for
that future proof and prepares the boundary/preflight scaffolding (`01B`,
`01C`) a future, separately-authorized session would need to run it
safely. Auditing the candidate is not the same as proving it.

## 9. What this patch will not do

- No network call of any kind (Kraken, CoinLore, TwelveData, Alpaca, or
  otherwise).
- No DB access or mutation of any kind.
- No credential use; `.env.local` is not read or modified.
- No trading enablement — no `enabled`, `paper_trading_enabled`, or
  `live_trading_enabled` flag is changed on any instrument or provider.
- No production registry-v2 cutover — no runtime/execution/risk/broker/
  OMS/ingestion code path is modified to consume `InstrumentRegistryV2`.
- No scheduler registration.
- No claim of closure for prerequisite #4 — it remains open.

---

## Closure note

This audit does not close prerequisite #4. It grounds the next two phases
(`01B` boundary decision, `01C` preflight guard) in current, verified repo
state: Kraken (`BTC/USD`/`ETH/USD`, `1D`, via `mqk md kraken-ohlc-ingest`)
is the safest and most complete first candidate for the eventual live
proof.
