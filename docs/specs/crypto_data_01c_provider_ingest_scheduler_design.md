# CRYPTO-DATA-01C — Provider Ingest + Scheduler Design

Patch ID: `CRYPTO-DATA-01C-PROVIDER-INGEST-SCHEDULER-DESIGN-01-COMBINED`

This is a design/spec/readiness patch. It is **not** provider implementation,
**not** network ingestion, **not** scheduler implementation, **not** live
market-data execution, **not** a production registry-v2 cutover, **not** a
portfolio-ledger cutover, **not** risk enforcement, **not** order routing,
**not** broker integration, **not** a DB migration, and **not** crypto
trading enablement. It decides the next implementation lane for real BTC/USD
and ETH/USD crypto spot marks reaching `md_bars`, continuing the lane
`ASSET-CORE-04E` opened and `CRYPTO-DATA-01A`/`01B`/`ASSET-CORE-04F` proved
end-to-end at the model and route level using fixture/local data only.

Decided at HEAD `6535db8e`.

---

## 1. Executive Decision

**Chosen lane: explicit, operator-run, local-file ingestion — not a live
network provider.** The next implementation patch should *not* attempt to
wire a real network crypto data provider. Direct repo evidence (§2, §3) shows
every candidate network provider is either entirely unimplemented in this
codebase or explicitly unverified for crypto, and this patch is barred from
making any network call to verify one. The only lane that can be advanced
safely and honestly today is to **operationalize the local-CSV path
`CRYPTO-DATA-01A`/`01B` already proved** into an explicit, default-off,
operator-triggered import script — modeled directly on this repo's own
`scripts/windows/Register-PremarketDataRefreshTask.ps1` /
`Prep-PremarketMarketData.ps1` scheduler precedent — that wraps the
**already-existing, already-generic** `mqk-cli md ingest-csv` command. No new
Rust ingestion code is required for this; the gap is entirely in the
operator-facing wrapper, fail-closed gating, evidence shape, and (optionally)
Windows Task Scheduler registration.

Recommended next implementation patch:
**`CRYPTO-DATA-01D-EXPLICIT-LOCAL-CRYPTO-INGEST-RUNNER-01`** (see §13).

This is a direct continuation of `ASSET-CORE-04E`'s own deferred decision
(§7/§15 of that document): "Phase 2" (a real network provider) was
explicitly *not chosen*, only flagged as a future, separately-verified
decision. Nothing in the repo since `ASSET-CORE-04E` (`CRYPTO-DATA-01A`,
`CRYPTO-DATA-01B`, `ASSET-CORE-04F`) has implemented or verified a network
crypto provider, so that decision cannot honestly be made now either — this
patch does not pretend otherwise.

---

## 2. Current Repo Facts

Grounded by direct file reads at HEAD `6535db8e`, answering the mission's ten
pre-flight questions in order.

1. **Provider interfaces that exist today** (`core-rs/crates/mqk-md/src/provider.rs`):
   `Provider` (legacy sync, raw `RawBar`), `HistoricalProvider` (async,
   implemented by the two providers below), and `MarketDataProvider` (the
   capability-aware async contract — `capabilities()`, `health()`,
   `rate_limits()`, `fetch_historical_bars`, `fetch_latest_closed_bar`).
   `ProviderAssetClass::Crypto` exists as a capability-metadata tag and maps
   to canonical trading class `"crypto"` (`provider_asset_class_trading_class`)
   — this is a label, not an enablement path; it gates nothing downstream.

2. **Provider implementations that exist today**: exactly two —
   `AlpacaHistoricalProvider` (`alpaca_provider.rs`) and
   `TwelveDataHistoricalProvider` (`lib.rs`), both `HistoricalProvider`, both
   wired only against equity symbols. `mqk-md/src/provider_registry.rs::build_market_data_provider_from_config`
   — the **single factory function** that turns a `providers.json` entry into
   a live `MarketDataProvider` — has match arms for exactly `"fake"`,
   `"twelvedata"`, `"alpaca"`; every other `provider_id` (including
   `"coinlore"`, `"polygon"`, `"alphavantage"`, `"yfinance"`) falls through to
   `ProviderFactoryError::UnsupportedProvider`. This is direct, mechanical
   proof — not inference — that no crypto-capable provider factory exists in
   code today.

3. **Do any provider configs claim crypto support, and is it implemented?**
   `config/providers/providers.json`: `twelvedata` declares
   `["equity","etf","forex","crypto"]` but is self-labeled
   `implementation_status: "implemented_equity_provider"` and
   `verification_status: "repo_implemented_official_limits_unverified"` —
   never exercised against a non-equity symbol anywhere in the repo.
   `alpaca` declares **only** `["equity","etf"]` — crypto is not even
   *claimed*, confirming the existing audit note
   (`docs/audits/multi_asset_completion_audit.md` line 176: "Alpaca adapter
   never calls `/v2/crypto/*`"). `alphavantage`/`polygon`/`yfinance` declare
   crypto in `asset_classes` but are all `implementation_status:
   "candidate_unverified"` / `enabled: false`, with zero Rust implementation
   and zero factory match arm (§2). `coinlore` (crypto-only, no API key
   required) is the same: `candidate_unverified` / `enabled: false` / zero
   implementation. Coinbase, Kraken, and Binance have **zero presence**
   anywhere in this repo — no config entry, no Rust type, no factory arm, no
   test, no mention outside docs/ledger prose recording this exact fact.

4. **What exact path inserts bars into `md_bars` today?**
   `mqk_db::ingest_provider_bars_to_md_bars` (`core-rs/crates/mqk-db/src/md.rs`)
   — a single upsert helper (`insert ... on conflict (symbol, timeframe,
   end_ts) do update ...`) used by *every* ingestion entry point: the CLI's
   `ingest-csv`, `ingest-provider`, and `sync-provider` commands, and
   `CRYPTO-DATA-01B`'s DB-backed test. It is asset-class-agnostic — nothing in
   its signature or body inspects `asset_class`. `mqk_md::ingest_csv::parse_csv_file`
   (the **read**-only CSV parser feeding it) is equally asset-class-agnostic
   and already proved on a `BTC/USD` fixture by `CRYPTO-DATA-01A`.

5. **What CLI surface exists today, and does it already support a
   non-network crypto path with zero new code?** Yes. Direct read of
   `core-rs/crates/mqk-cli/src/main.rs::MdCmd::IngestCsv` and
   `core-rs/crates/mqk-cli/src/commands/md.rs::md_ingest_csv` confirms
   `mqk-cli md ingest-csv --path <file> --timeframe <tf> --source <any free-text label>`
   is **already fully generic**: `path` is any filesystem path, `source` is
   an arbitrary string (default `"csv"`, not constrained to any provider
   list), and the underlying parser/DB-upsert path neither knows nor cares
   about asset class. This command could ingest a manually-prepared
   `BTC/USD` CSV into `md_bars` **today, with zero new Rust code**. By direct
   contrast, `MdCmd::IngestProvider` and `MdCmd::SyncProvider` hard-validate
   `source` against exactly `"twelvedata"`/`"alpaca"` (`md_ingest_provider`/
   `md_sync_provider`, both: `if source_lc != "twelvedata" && source_lc != "alpaca" { bail!(...) }`)
   — those two commands cannot be pointed at a crypto symbol honestly today,
   since neither underlying provider is verified (or, for Alpaca, even
   declared) for crypto.

6. **What scheduler precedent exists for recurring market-data work?** Two
   committed, working precedents, both **explicit-operator-registered**, both
   scoped to data-prep only:
   - `scripts/windows/Register-PremarketDataRefreshTask.ps1` — registers a
     Windows Scheduled Task whose action calls **only**
     `Prep-PremarketMarketData.ps1`. Its own header states the safety
     invariant directly: "does NOT start the daemon, runtime, or trading...
     does NOT call `Start-PaperTradingSmoke.ps1`... does NOT enqueue orders."
     Has `-CheckOnly` (preview, zero mutation) and `-Unregister` modes, writes
     a `task_registration.json` record and per-run transcript logs under
     `exports/market_data/scheduled/`.
   - `scripts/windows/Refresh-IntradayMarketData.ps1` — explicit `-CheckOnly`
     (read-only) / `-Once` (single run) / interval-loop modes; guards that
     `MQK_DATABASE_URL` contains port `5440` (paper DB only) before doing
     anything; never prints credentials; calls only `mqk-cli md sync-provider`
     (equity-only, per §5) and writes an evidence JSON
     (`exports/market_data/intraday_refresh_*.json`) with explicit
     `freshness_truth_state`/`reason_code`/`passed` fields, failing closed
     (`PAPER-SMOKE-MD-REFRESH-FAIL-CLOSED-01`, documented in its own source)
     whenever provider config is missing, the provider call fails, bar count
     is below threshold, or the latest bar is stale.
   Neither script ever touches the daemon, runtime, broker, OMS, or order
   path. Both are the direct shape the next implementation patch should copy
   for crypto — substituting `mqk-cli md ingest-csv` (local file, no
   provider/network call) for `sync-provider` (network call).

7. **What evidence/status shape exists for ingestion?** Two layers, both
   reusable as-is: (a) `mqk_db::md::{CoverageQualityReport, MdQualityReport}`
   — `rows_read`/`rows_ok`/`rows_rejected`/`rows_inserted`/`rows_updated` plus
   per-symbol/per-group stats — written to the `md_quality_reports` table and
   exported as `data_quality.json` by every existing CLI ingestion command,
   with zero changes needed to reuse for a crypto CSV; (b)
   `Refresh-IntradayMarketData.ps1`'s own evidence-JSON convention
   (`schema_version`, `produced_at_utc`, `mode`, `source`, per-symbol
   freshness/pass-fail fields) under `exports/market_data/`. One real gap:
   `md_ingest_csv` (the CLI handler) always calls the metadata-less
   `mqk_db::ingest_provider_bars_to_md_bars`, which stamps
   `MdBarProviderMetadata::unknown()` — so `md_bars.provider_id` is literally
   `"unknown"` for every CSV-ingested row regardless of the `--source` label
   passed on the command line (the label only reaches the
   `md_quality_reports.stats_json.source` field, not the row itself). The
   metadata-aware sibling, `ingest_provider_bars_to_md_bars_with_provider_metadata`,
   already exists but is not wired to the CLI's CSV path. This is a real,
   minor, repo-confirmed gap — recorded here, not fixed here (§13).

8. **What symbol convention should real crypto ingest use?** Canonical
   `"BTC/USD"` / `"ETH/USD"` — already the established, fixture-proven, and
   route-proven convention (`ASSET-CORE-04E` §6, `CRYPTO-DATA-01A`/`01B`,
   `ASSET-CORE-04F`'s live route test). No change to this convention is
   proposed. The committed real registry-v2 fixture
   (`config/instruments/instruments_v2.crypto_local_marks.example.json`)
   carries exactly one instrument, `BTC/USD`, with
   `provider_symbols: {"local_csv": "BTC/USD"}`. No `ETH/USD` registry-v2
   entry or CSV fixture exists anywhere in the repo today — confirmed by a
   repo-wide search for `ETH/USD`/`ETHUSD`/`ETH_USD` matching only this
   decision's own forthcoming docs, never any config, fixture, or test.

9. **Can existing `md_bars` store `BTC/USD` and `ETH/USD` without schema
   changes?** Yes, unchanged from `ASSET-CORE-04E`'s finding: `md_bars`'s
   primary key `(symbol, timeframe, end_ts)` uses a bare unconstrained `text`
   `symbol` column (`0003_backtest_schema.sql`, unchanged through the current
   latest migration `0043`), with no charset/format constraint at the DB
   layer and no equity symbol containing `/`. `CRYPTO-DATA-01B` already
   proved a real `BTC/USD` row insert/readback round-trip against the local
   paper DB with zero migration.

10. **What should the next implementation patch build first?** Exactly one
    thing: an explicit, default-off, operator-run wrapper script (plus,
    optionally, its own-default-unregistered Task Scheduler registration
    script) around the **already-working** `mqk-cli md ingest-csv` command,
    with fail-closed evidence output. It should *not* attempt a network
    provider — every candidate is unimplemented or unverified (§3), and
    verifying one requires a network call this design patch (and, by the
    same logic, any patch that has not been explicitly authorized to spend
    API calls) cannot make.

---

## 3. Provider Candidates Evaluated

| Candidate | Repo evidence | Classification |
|---|---|---|
| **TwelveData crypto** | `TwelveDataHistoricalProvider` exists and is mechanically symbol-agnostic (generic `/time_series` HTTP/JSON client); `providers.json` declares crypto in `asset_classes`. But `implementation_status: "implemented_equity_provider"`, `verification_status: "repo_implemented_official_limits_unverified"`, never exercised against a non-equity symbol in any test or production path. | **Unverified for crypto.** Mechanically plausible, not proven. |
| **Alpaca crypto market data** | `providers.json`'s `alpaca` entry declares `asset_classes: ["equity", "etf"]` only — crypto is not even claimed. `mqk-broker-alpaca` has no `/v2/crypto/*` call anywhere (confirmed directly; matches the existing `CRYPTO-EXEC-01` audit-roadmap note). | **Not supported today**, not merely unverified. |
| **Coinbase public market data** | Zero presence anywhere in the repo: no config entry, no Rust type, no factory arm, no test, no doc reference outside this decision's own evaluation. | **Unimplemented**, zero repo evidence either way. |
| **Kraken public market data** | Same as Coinbase — zero presence anywhere. | **Unimplemented**, zero repo evidence either way. |
| **Polygon crypto** | `providers.json` declares crypto in `asset_classes`, but `implementation_status: "candidate_unverified"`, `enabled: false`, zero Rust client, zero `provider_registry.rs` factory arm. | **Unimplemented**, config-only metadata. |
| **CoinLore** | The one provider config scoped *exclusively* to crypto, no API key required (`api_key_required: false`) — the most attractive candidate **if and when** a future, explicitly-network-authorized patch verifies it. Today: `candidate_unverified`, `enabled: false`, zero Rust implementation, zero factory arm. | **Unimplemented**, but the leading future candidate (§16). |
| **Local CSV (file-drop)** | `mqk_md::ingest_csv::parse_csv_file` + `mqk_db::ingest_provider_bars_to_md_bars` + `mqk-cli md ingest-csv` already fully prove this path end-to-end for `BTC/USD`, including DB persistence (`CRYPTO-DATA-01B`) and HTTP-route-level valuation (`ASSET-CORE-04F`). Zero network, zero new code required for the ingest step itself. | **Chosen** — already proven, zero unverified claims. |
| **Manual operator import** | Not structurally different from "local CSV" — it is the *operational pattern* (operator periodically updates/replaces the local file and re-runs ingestion) rather than a different data path. This design folds it into the chosen lane as the scheduler shape (§8), not a separate technical candidate. | **Chosen**, as the operating model for the local-CSV lane. |

No candidate above is rejected as *permanently* unsuitable for crypto. Every
network candidate is **deferred** pending an explicit, separately-authorized
verification step that this patch — and, by its own hard safety rules, the
recommended next patch — cannot take.

---

## 4. Chosen First Provider/Source Lane

**Local file ingestion via the existing, generic `mqk-cli md ingest-csv`
command, operationalized as an explicit, default-off, operator-triggered
script (optionally registered as a Windows Scheduled Task that calls only
that script).** This is not a new data path — it is the same path
`CRYPTO-DATA-01A`/`01B`/`ASSET-CORE-04F` already proved, wrapped in the
operator-facing scaffolding (fail-closed gating, evidence shape, optional
recurring registration) that a real operating procedure needs, mirroring
this repo's own `Register-PremarketDataRefreshTask.ps1` /
`Prep-PremarketMarketData.ps1` precedent exactly.

This is the safest available lane because it is the **only** candidate in
§3 with zero unverified claims: every live network candidate is either
unimplemented in this codebase or explicitly self-labeled unverified for
crypto, and verifying one honestly requires a network call this patch is
barred from making (and the recommended next patch, §13, is scoped to avoid
needing).

---

## 5. Provider Symbol Convention

- Canonical instrument symbol stays **`BTC/USD`** / **`ETH/USD`** — no
  change. This already matches `instrument_registry_v2.rs`'s `CryptoPair`
  fixture convention, the committed CSV fixture, and the registry-v2 row's
  own `provider_symbols.local_csv` key.
- `provider_symbols` is already a map (not a single string) on each
  registry-v2 instrument, so adding a future verified network provider's
  alias (e.g. `provider_symbols.coinlore = "BTC"` or
  `provider_symbols.twelvedata_crypto = "BTC/USD"`) later requires no schema
  change — only a new map entry on the existing `BTC/USD` row. This design
  does not add any such entry now (`config/instruments/*` is out of this
  patch's file scope).
- `ETH/USD` has no registry-v2 entry or CSV fixture today (§2 fact 8). If the
  next implementation patch wants a second symbol, it should add a real
  (disabled) `ETH/USD` registry-v2 row in the exact shape of the existing
  `BTC/USD` row, plus its own committed CSV fixture — not a structural
  change, an additive one.

---

## 6. Storage Path Into Existing `md_bars`

Unchanged from `ASSET-CORE-04E`/`CRYPTO-DATA-01B`: `md_bars`'s existing
`(symbol, timeframe, end_ts)` schema stores crypto bars today with zero
migration, via the existing `mqk_db::ingest_provider_bars_to_md_bars` upsert
helper. The next implementation patch's only real gap here (§2 fact 7,
carried forward to §13) is that the CLI's CSV path does not currently pass
real `MdBarProviderMetadata` (e.g. `provider_id: "local_crypto_manual"`)
through to the row — it always stores `provider_id = "unknown"`. Fixing that
is optional polish for the next patch, not a blocker: the symbol-keyed read
path (`fetch_recent_completed_bars_for_strategy`) that
`ASSET-CORE-04B`/`04A`/`04C`/`04D`/`04F` all depend on never reads
`provider_id` at all.

---

## 7. Registry-v2 Requirements

No new registry-v2 *shape* is required — `ContractDefinitionV2::CryptoPair{base,quote}`
already exists, validates, and is fixture-proven for `BTC/USD`
(`config/instruments/instruments_v2.crypto_local_marks.example.json`,
`enabled`/`paper_trading_enabled`/`live_trading_enabled` all `false`,
`allow_enabled_non_equity_for_testing` absent). This patch does not modify
that file (`config/instruments/*` is out of scope per the mission's file
restrictions). The next implementation patch may, optionally and additively:

- Add a real, disabled `ETH/USD` `CryptoPair{base:"ETH",quote:"USD"}` row in
  the same file, in the exact honesty-flag shape as the existing `BTC/USD`
  row, if a second symbol is wanted.
- Populate a real `provider_id` via
  `ingest_provider_bars_to_md_bars_with_provider_metadata` (already exists,
  just not wired to the CLI's CSV command) so `md_bars` rows ingested by the
  new scheduler script are distinguishable from equity rows and from each
  other by source — useful for evidence/audit, not required for correctness.

Neither change is required to prove the next patch's scheduler/evidence
shape; both are explicitly optional, additive follow-ups.

---

## 8. Scheduler Shape

Modeled directly on the two existing precedents (§2 fact 6), adapted for a
**local-file** source instead of a network provider:

1. **Import script** (e.g. `scripts/windows/Import-LocalCryptoMarks.ps1`),
   the `Refresh-IntradayMarketData.ps1` shape:
   - `-CheckOnly` mode: reports current `md_bars` coverage/freshness for the
     configured crypto symbols, zero mutation. Default-safe entry point.
   - `-Once` mode: runs `mqk-cli md ingest-csv --path <configured local file>
     --timeframe <tf> --source local_crypto_manual` exactly once, then
     reports the resulting `CoverageQualityReport` and writes an evidence
     JSON.
   - No automatic interval-loop mode by default — unlike
     `Refresh-IntradayMarketData.ps1`'s 5-minute equity loop, crypto's manual
     local-file source has no reason to re-run more often than the operator
     actually updates the file. An interval-loop mode may be added later
     **only** if the operator explicitly wants polling for file changes —
     not implied by this design.
   - Hard guard: paper DB only (mirrors the existing port-5440 guard
     convention) — this script must refuse to run against any
     non-paper-tagged `MQK_DATABASE_URL`, exactly like
     `Refresh-IntradayMarketData.ps1` already does.
   - Fails closed exactly like `Refresh-IntradayMarketData.ps1`'s own
     documented `PAPER-SMOKE-MD-REFRESH-FAIL-CLOSED-01` discipline: missing
     file, zero rows read, zero rows accepted, or stale latest-bar timestamp
     must all produce a non-zero exit code and an explicit `passed: false`
     evidence field — never a silent or optimistic "OK".
2. **Optional registration script** (e.g.
   `scripts/windows/Register-LocalCryptoIngestTask.ps1`), the
   `Register-PremarketDataRefreshTask.ps1` shape: registers a Windows
   Scheduled Task whose action calls **only** the import script above (never
   the daemon, runtime, or any trading script), with the same
   `-CheckOnly`/`-Unregister` modes and the same
   `task_registration.json`-style evidence record. **Default: not
   registered.** Registration is itself an explicit, separate operator
   action — this design does not register anything, and the next
   implementation patch should not auto-register either.
3. **Where the local file's numbers come from is explicitly an
   operator-trust boundary**, not a verified data source. Neither this
   patch, nor the recommended next patch, makes any claim about provenance,
   accuracy, or timeliness of operator-supplied crypto prices — that is an
   open question (§16), not something a scheduler around `ingest-csv` can
   resolve.

---

## 9. Rate-Limit / API-Credit Guardrails

**Not applicable to the chosen lane** — local file ingestion makes zero
network calls, so there is no rate limit or API credit to guard. Recorded
for the future, not designed now: if and when a real network provider is
later verified and chosen (§16), it must reuse the rate-limit capability
surface that already exists and is already unused for this purpose —
`mqk_md::provider::MarketDataProviderRateLimits` (`calls_per_minute`,
`calls_per_day`, `remaining_calls`, `notes`) and `providers.json`'s own
`rate_limit_notes` field per entry (e.g. TwelveData's documented
"8 requests/minute on free tier" note) — before any recurring *automatic*
network call is implemented. This design does not specify those limits for
any candidate, since none is chosen yet.

---

## 10. Evidence/Status Requirements

Reuse, do not reinvent:

- `mqk_db::md::{CoverageQualityReport, MdQualityReport}` — already produced
  by every existing ingestion command and persisted to `md_quality_reports`;
  zero changes needed to reuse for a crypto CSV import.
- The CLI's existing `data_quality.json` artifact convention
  (`exports/md_ingest/<ingest_id>/data_quality.json`).
- `Refresh-IntradayMarketData.ps1`'s evidence-JSON convention — adapted for
  "local crypto import" semantics: `schema_version`, `produced_at_utc`,
  `mode` (`check_only`|`once`), `source` (`"local_csv_manual"`, not a
  provider name), `symbols`, per-symbol `completed_count`/`max_ts_iso`/
  `staleness_min`/`gate`/`passed`/`fail_reasons`. No `provider_configured`
  or credential-presence field is needed (there is no provider/credential
  in this lane). Written under `exports/market_data/` matching the existing
  directory convention.
- `task_registration.json` convention (if the optional registration script
  is built) — mirrors `Register-PremarketDataRefreshTask.ps1`'s existing
  record shape exactly.

---

## 11. Failure States and Fail-Closed Behavior

All of the following already exist and require no new logic, only correct
wiring by the next patch:

- Missing CSV file → `mqk_md::ingest_csv::CsvIngestError::Io` (existing).
- Missing/misnamed header column → `CsvIngestError::MissingHeader` (existing).
- Per-row rejects (bad timeframe, duplicate in batch, out-of-order, OHLC
  sanity violation, negative volume) → counted in `RejectCounts`/
  `CoverageTotals`, never silently dropped (existing, in
  `mqk_db::md::ingest_provider_bars_to_md_bars_inner`).
- Zero rows read or zero rows accepted → `coverage.rows_read == 0` /
  `coverage.rows_ok == 0` is an explicit, visible report field, not an
  absence of output — the wrapper script (§8) must treat either as a
  fail-closed condition (`passed: false`, non-zero exit), not a quiet no-op.
- Stale latest bar (file not updated recently enough) → the wrapper script
  must compute and gate on staleness the same way
  `Refresh-IntradayMarketData.ps1` already does for equities, with its own
  explicit `freshness_truth_state`/`reason_code`.
- Non-paper DB target → hard refusal before any mutation, mirroring the
  existing port-5440 guard.

No new fail-closed *logic* needs inventing — the discipline already exists
in two places in this repo; the next patch's job is to apply it to the
crypto-local-file case.

---

## 12. Operator Run Commands Planned for Next Implementation

These already work today, with zero new code, and form the basis of the
next patch's wrapper script:

```powershell
# Already-working ingestion (today, no new code):
cargo run -p mqk-cli --bin mqk-cli -- md ingest-csv `
  --path config\fixtures\crypto\btcusd_1d_local_manual.csv `
  --timeframe 1D `
  --source local_crypto_manual

# Planned (next patch) — explicit, default-off, fail-closed wrapper:
powershell -ExecutionPolicy Bypass -File scripts\windows\Import-LocalCryptoMarks.ps1 -CheckOnly
powershell -ExecutionPolicy Bypass -File scripts\windows\Import-LocalCryptoMarks.ps1 -Once

# Planned (next patch, optional) — Task Scheduler registration, default unregistered:
powershell -ExecutionPolicy Bypass -File scripts\windows\Register-LocalCryptoIngestTask.ps1 -CheckOnly
powershell -ExecutionPolicy Bypass -File scripts\windows\Register-LocalCryptoIngestTask.ps1
```

None of the above were executed by this patch. The first command is recorded
as a present-tense repo fact (§2 fact 5), not a result this patch produced.

---

## 13. What the Next Implementation Patch Should Build

**`CRYPTO-DATA-01D-EXPLICIT-LOCAL-CRYPTO-INGEST-RUNNER-01`**, scoped to:

1. `scripts/windows/Import-LocalCryptoMarks.ps1` — the wrapper script
   described in §8, around the existing `mqk-cli md ingest-csv` command. Zero
   new Rust code required for the ingest step itself.
2. Optionally, `scripts/windows/Register-LocalCryptoIngestTask.ps1` — the
   Task Scheduler registrar described in §8, default-unregistered.
3. Optionally, a real disabled `ETH/USD` registry-v2 row + CSV fixture
   (§5/§7), if a second symbol is wanted in the same patch.
4. Optionally, wiring `ingest_provider_bars_to_md_bars_with_provider_metadata`
   into the CLI's CSV path so crypto-imported rows carry a real
   `provider_id` (§2 fact 7, §6) instead of `"unknown"` — a small, isolated,
   additive change to `mqk-cli`/`mqk-db` call sites, not a schema change.
5. Evidence JSON under `exports/market_data/` per §10.

Explicitly **not** in `01D`'s scope either: no network provider, no daemon
scheduler/background job, no DB migration, no trading enablement. The
network-provider decision (§16) remains open for a later, separately
authorized patch.

---

## 14. What This Patch Does Not Change

This patch (`CRYPTO-DATA-01C`) adds only: this design document, its
machine-readable JSON artifact, an optional validator script, and
ledger/audit updates. It did not touch, and made no behavior change to: any
Rust source file anywhere in `core-rs/crates/mqk-daemon/src`,
`mqk-runtime`, `mqk-execution`, `mqk-broker-alpaca`, `mqk-broker-paper`,
`mqk-risk`, `mqk-md/src` (including `provider.rs`, `provider_registry.rs`,
`ingest_csv.rs` — read, not edited), `mqk-db/src` (including `md.rs` — read,
not edited), or `mqk-portfolio/src`; any DB migration; any file under
`core-rs/mqk-gui`; `config/instruments/*`; `config/providers/*`;
`.env.local`; or any strategy/OMS/outbox/scheduler/provider-implementation
code. No daemon runtime was started. No provider, broker, or network call
was made. No file was staged outside this patch's stated scope.

---

## 15. Safety Boundaries

Unconditionally true of this decision and must remain true of the patch
named in §13:

- No live or paper order submitted, ever, for any crypto/futures/options/forex
  instrument.
- No provider/broker network call. No API credits spent.
- No DB migration. `md_bars`'s existing schema is sufficient (§6).
- No registry-v2 entry with `enabled=true` for any non-equity instrument
  outside the validator's existing `#[cfg(test)]`-only escape hatch.
- No change to `PortfolioState`, `compute_portfolio_weights`,
  `/api/v1/portfolio/live-weights`, or `/api/v1/portfolio/economics/status`
  behavior.
- No change to risk, OMS, broker, runtime, or strategy code.
- No automatic/background scheduler — every run is operator-explicit
  (`-CheckOnly`/`-Once`), and Task Scheduler registration (if built) defaults
  to unregistered and only ever calls the import script, never the daemon or
  any trading script.
- No session/calendar enforcement change — crypto 24/7 remains a documented
  model assumption, not a wired runtime behavior.

---

## 16. Open Questions Before Crypto Trading Enablement

Carried forward and extended from `ASSET-CORE-04E` §15 — none of these are
answered or required by this patch:

1. **Who/what is the trusted source of the manually-imported CSV's prices?**
   This design explicitly treats that as an operator-trust boundary (§8),
   not a verified data source. No patch in this lane has addressed data
   provenance/accuracy for the local-file case, and none should claim to
   without an explicit operator decision.
2. **Which real network provider (if any) eventually supplies live crypto
   marks**, and what explicit, separately-authorized verification step
   (necessarily involving a real network call) proves it before
   `providers.json`'s `implementation_status`/`verification_status` fields
   for crypto could honestly change? `CoinLore` (crypto-only, no API key) is
   the leading candidate for that future verification step on cost/scope
   grounds alone, ahead of TwelveData (already-implemented client but
   equity-labeled and crypto-unverified) — this design does not choose
   between them, only ranks them as candidates for a future, explicitly
   network-authorized patch.
3. **Should `ETH/USD` get its own registry-v2 entry and CSV fixture now or
   later?** This design leaves it as an optional, additive next-patch
   decision (§5/§13), not a requirement.
4. **Should the CLI's CSV ingestion path be made provider-metadata-aware**
   (§2 fact 7, §6, §13 item 4) before or after the scheduler wrapper lands?
   This design treats it as optional polish, not a blocker, for `01D`.
5. **Whether `md_bars`'s symbol-only schema should ever be replaced** with an
   instrument-aware schema — unchanged open question from `ASSET-CORE-04E`
   §15, not resolved or advanced by this patch.
6. **Whether crypto's 24/7 session "requirement" should graduate** from
   `ASSET-CORE-05`'s existing scaffold to an authoritative provider before any
   paper-trading consideration — unchanged open question from
   `ASSET-CORE-04E` §15.
7. **Account-currency generalization** — unchanged open question from
   `ASSET-CORE-04E` §15; `ASSET-CORE-04D`/`04F`'s route still hardcodes
   `account_currency = "USD"`.

None of these questions block or are answered by this decision. They are
recorded so the next patch (§13) inherits an honest, explicit list rather
than silently assuming any of them away.
