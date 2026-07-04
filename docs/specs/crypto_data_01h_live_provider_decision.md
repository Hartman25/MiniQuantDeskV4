# CRYPTO-DATA-01H — Live Crypto Provider Decision

Patch ID: `CRYPTO-DATA-01H-LIVE-CRYPTO-PROVIDER-DECISION-01-COMBINED`

This is a decision/spec/readiness patch. It is **not** provider implementation,
**not** a network-call patch, **not** API verification by live request, **not**
live market-data ingestion, **not** scheduler execution, **not** a production
registry-v2 cutover, **not** a portfolio-ledger cutover, **not** risk
enforcement, **not** order routing, **not** broker integration, **not** a DB
migration, and **not** crypto trading enablement. It decides the first
network-authorized verification lane and the first implementation lane for a
real live/network BTC/USD and ETH/USD crypto market-data provider, continuing
the lane `CRYPTO-DATA-01C` opened and `01D`–`01G`/`ASSET-CORE-04F` proved
end-to-end using local-CSV fixtures only.

Decided at HEAD `6271c048`.

---

## 1. Executive Decision

**Chosen first network-authorized verification lane: CoinLore.** Chosen first
implementation lane (after verification): a CoinLore-backed
`MarketDataProvider` adapter feeding the existing, unmodified local-mark
storage path. Direct repo evidence (§2, §3) confirms every provider candidate
remains either entirely unimplemented in this codebase or explicitly
self-labeled unverified for crypto — nothing has changed on this axis since
`CRYPTO-DATA-01C` (HEAD `6535db8e`). This patch does not build a provider; it
narrows the eight-candidate field `01C` left open to one ranked choice for
the next two, separately-authorized patches, and records why the alternatives
are deferred or rejected.

CoinLore is chosen over TwelveData (the only other candidate with working
Rust HTTP-client code today) because CoinLore requires **no credential** and
is **crypto-only in scope**: a future verification call cannot consume or
interfere with the already-configured `TWELVEDATA_API_KEY` rate-limit budget
that equity ingestion depends on (`8 requests/minute` free tier, per
`config/providers/providers.json`), and a CoinLore adapter cannot accidentally
acquire equity-fetch capability it was never meant to have. This is a scope
and blast-radius argument, not a claim that TwelveData's crypto endpoint is
inferior or broken — it has simply never been exercised against a non-equity
symbol by any test in this repo, exactly as `01C` recorded.

Recommended next patches:
1. **`CRYPTO-DATA-01I-COINLORE-READONLY-NETWORK-VERIFY-01`** — first
   network-authorized verification patch (see §4).
2. **`CRYPTO-DATA-01J-COINLORE-PROVIDER-ADAPTER-LOCAL-INGEST-01`** — first
   implementation patch, built only after `01I` succeeds (see §4).

---

## 2. Current Repo Facts

Grounded by direct file reads at HEAD `6271c048`, answering the mission's
twelve pre-flight questions in order. Nothing below required a network call;
all facts are static-code/config observations.

1. **Provider interfaces that exist today**
   (`core-rs/crates/mqk-md/src/provider.rs`): three, unchanged from `01C`:
   `Provider` (legacy sync, raw `RawBar`, object-safe via `Box<dyn Provider>`),
   `HistoricalProvider` (async, implemented by the two concrete providers
   below), and `MarketDataProvider` (the capability-aware async contract —
   `provider_id()`, `capabilities()`, `health()`, `rate_limits()`,
   `fetch_historical_bars()`, `fetch_latest_closed_bar()`).
   `ProviderAssetClass::Crypto` exists as a capability-metadata label
   (`provider_asset_class_trading_class` maps it to `"crypto"`) — it gates
   nothing downstream; it is not an enablement path.

2. **Provider factory arms actually buildable today**
   (`mqk-md/src/provider_registry.rs::build_market_data_provider_from_config`,
   the single function that turns a `providers.json` entry into a live
   `MarketDataProvider`): exactly three match arms — `"fake"`, `"twelvedata"`,
   `"alpaca"`. Every other `provider_id` string, including `"coinlore"`,
   `"polygon"`, `"alphavantage"`, `"yfinance"`, falls through to
   `ProviderFactoryError::UnsupportedProvider` (`provider_registry.rs:281`).
   This is mechanical proof, not inference: adding a fourth match arm is
   required before any other provider can be constructed by this factory at
   all, regardless of its `providers.json` entry.

3. **Which provider configs claim crypto support, and which are only
   metadata/unverified?** (`config/providers/providers.json`, read directly):
   - `twelvedata`: `asset_classes: ["equity","etf","forex","crypto"]`,
     `enabled: true`, but `implementation_status:
     "implemented_equity_provider"` and `verification_status:
     "repo_implemented_official_limits_unverified"` — the struct name
     (`TwelveDataHistoricalProvider`), source name (`"twelvedata"`), and every
     test in `mqk-md/src/lib.rs` are equity-symbol-only.
   - `alpaca`: `asset_classes: ["equity","etf"]` — crypto is **not claimed at
     all**, confirmed independently by direct read of
     `alpaca_provider.rs`: `bars_url()` returns
     `"{base_url}/v2/stocks/bars"` unconditionally; there is no
     `/v1beta3/crypto/*` call anywhere in the file.
   - `alphavantage`, `polygon`, `yfinance`: all declare crypto in
     `asset_classes`, all `enabled: false`, all
     `implementation_status: "candidate_unverified"`, zero Rust type, zero
     factory arm.
   - `coinlore`: `asset_classes: ["crypto"]` only (the sole crypto-exclusive
     entry), `api_key_required: false`, `enabled: false`,
     `implementation_status: "candidate_unverified"`, zero Rust type, zero
     factory arm.
   - Coinbase, Kraken, Binance: zero presence anywhere in `providers.json`,
     `mqk-md/src`, or any test — no entry, no type, no factory arm, no doc
     reference outside prose recording this exact absence.

4. **Does any current provider implementation actually map BTC/USD or
   ETH/USD to a live crypto endpoint?** No. `AlpacaHistoricalProvider` is
   hardcoded to the equities `/v2/stocks/bars` endpoint
   (`alpaca_provider.rs:71`) with no crypto-endpoint code path at all.
   `TwelveDataHistoricalProvider::build_time_series_url()` (`lib.rs:423`)
   builds a generic `{base_url}/time_series` URL and passes whatever
   `symbol` string the caller supplies — it is mechanically symbol-agnostic
   (no equity-only validation in the request path), but no test, CLI path, or
   production call in this repo has ever passed it a `"BTC/USD"`-shaped
   symbol. This is the same "mechanically plausible, not proven" finding
   `01C` §3 already recorded; nothing has changed.

5. **Does any current provider implementation handle 24/7 crypto sessions or
   crypto-specific symbol mapping?** No. Neither `AlpacaHistoricalProvider`
   nor `TwelveDataHistoricalProvider` contains any session-calendar or
   symbol-translation logic for crypto (e.g. `BTC/USD` → an exchange-specific
   pair code). `ASSET-CORE-05`'s session-profile scaffold
   (`docs/audits/multi_asset_completion_audit.md`) remains a model-layer
   concept unconnected to any provider fetch path.

6. **Which existing CLI commands can ingest provider data today?**
   `core-rs/crates/mqk-cli/src/commands/md.rs` exposes three: `ingest-csv`
   (fully generic — any `--path`, any free-text `--source`; the proven
   `CRYPTO-DATA-01A`–`01G` crypto path), `ingest-provider`, and
   `sync-provider`.

7. **Are `ingest-provider` and `sync-provider` provider-locked to specific
   provider IDs?** Yes, unchanged from `01C`. Both
   `md_ingest_provider`/`md_sync_provider` (`commands/md.rs:266`,
   `commands/md.rs:441`) contain the identical hard gate:
   `if source_lc != "twelvedata" && source_lc != "alpaca" { anyhow::bail!(...) }`.
   Neither command can be pointed at `"coinlore"` (or any other provider
   string) today without a code change — the bail fires before any provider
   object is even constructed, and independently of `provider_registry.rs`'s
   own three-arm factory limit (§2) since these two CLI commands do not call
   `build_market_data_provider_from_config` at all — they construct
   `TwelveDataHistoricalProvider`/`AlpacaHistoricalProvider` directly.

8. **How do provider rows reach `md_bars` today?** Through exactly one
   asset-class-agnostic upsert helper,
   `mqk_db::md::ingest_provider_bars_to_md_bars` (with-metadata sibling:
   `ingest_provider_bars_to_md_bars_with_provider_metadata`, wired to the CSV
   path since `CRYPTO-DATA-01F`), keyed on `(symbol, timeframe, end_ts)`. This
   path is provider-agnostic by construction — it takes a `Vec<ProviderBar>`
   and does not care which provider produced them. No change is needed here
   for any future provider; the gap is entirely upstream, at the
   fetch/factory layer (§2, §7).

9. **Which provider would be cheapest to implement first if no live network
   proof is allowed in this patch?** By *code-reuse* cost alone, TwelveData
   is cheapest — `TwelveDataHistoricalProvider` already exists, already
   compiles, already speaks a generic OHLCV JSON shape, and a
   crypto-capability patch could in principle just pass a `"BTC/USD"`-shaped
   symbol through unmodified code. But "cheapest to write" is not the same
   question as "safest to verify," and this patch answers the latter (§10),
   because verification is the actual next gate, not code volume: no line of
   provider code can be honestly written as "crypto-capable" without a real
   network call proving the endpoint accepts and correctly returns crypto
   symbols, and this patch is barred from making that call.

10. **Which provider would be safest to verify first in a future
    network-authorized patch?** CoinLore. It is the only crypto-exclusive
    candidate (`asset_classes: ["crypto"]`, `api_key_required: false`) —
    verifying it touches zero credentials, zero shared-rate-limit budget with
    equity ingestion, and zero risk of an incorrect crypto-symbol request
    silently landing on an equity-scoped API key. TwelveData's crypto
    endpoint would require reusing the equity-provisioned
    `TWELVEDATA_API_KEY`, meaning a verification mistake (wrong symbol
    format, unexpected error response) could consume shared free-tier quota
    (`"8 requests/minute on free tier"`, per `providers.json`) that equity
    ingestion also depends on. CoinLore's public, keyless, crypto-only scope
    has no such coupling.

11. **Symbol convention at each layer** (unchanged from `01C` §5, reconfirmed
    against current fixtures):
    - **Canonical registry symbol:** `"BTC/USD"` / `"ETH/USD"` (the
      `InstrumentDefinitionV2.symbol` field,
      `config/instruments/instruments_v2.crypto_local_marks.example.json`).
    - **Provider symbol alias:** the existing `provider_symbols` map on each
      registry-v2 instrument already carries `{"local_csv": "BTC/USD"}` /
      `{"local_csv": "ETH/USD"}`. A future verified provider adds a new key
      to this same map (e.g. `provider_symbols.coinlore`) — no schema change.
      CoinLore's public API documents crypto identifiers as bare tickers
      (e.g. `"BTC"`), so the eventual alias value is expected to differ in
      shape from the canonical slash-pair symbol; this patch does not add
      that key (`config/instruments/*` is out of file scope) and does not
      assert the exact alias value without a verification call.
    - **`md_bars` symbol:** identical to the canonical registry symbol
      (`"BTC/USD"` / `"ETH/USD"`) — `md_bars`'s primary key column is a bare
      unconstrained `text`, and every existing local-CSV row already uses
      this exact string, unchanged by this patch.
    - **Route/query symbol:** identical again — `GET
      /api/v1/portfolio/economics/status?symbol=BTC%2FUSD` (URL-encoded
      slash) is the proven `ASSET-CORE-04F` route-level convention; no
      provider-specific symbol ever reaches this layer, since translation
      happens only at the provider-adapter boundary.

12. **What must the next implementation patch build first?** Exactly one
    small, isolated addition: a `MarketDataProvider`/`HistoricalProvider`
    implementation for CoinLore (a fourth `provider_registry.rs` factory
    match arm plus one new provider struct), gated behind
    `providers.json`'s existing `enabled` flag (default `false` until an
    operator flips it) — built only *after* `01I` (§4) proves, via one
    explicitly-authorized, rate-limited network call, that CoinLore's public
    endpoint returns BTC/USD and ETH/USD spot data in a shape this repo's
    existing `RawBar`/`ProviderBar` model can represent without a schema
    change.

---

## 3. Provider Candidates Evaluated

| Candidate | Repo evidence | Classification |
|---|---|---|
| **TwelveData crypto** | `TwelveDataHistoricalProvider` exists, compiles, and is mechanically symbol-agnostic; `providers.json` declares crypto in `asset_classes`. `implementation_status: "implemented_equity_provider"`, `verification_status: "repo_implemented_official_limits_unverified"`; never exercised against a non-equity symbol in any test. Shares a rate-limited credential (`TWELVEDATA_API_KEY`) with equity ingestion. | `repo_unverified` |
| **Alpaca crypto market data** | `providers.json`'s `alpaca` entry declares `asset_classes: ["equity","etf"]` only — crypto is not claimed. `alpaca_provider.rs::bars_url()` is hardcoded to `/v2/stocks/bars`; no `/v1beta3/crypto/*` call exists anywhere in the file. | `rejected_for_first_lane` |
| **Coinbase public market data** | Zero presence anywhere in the repo: no `providers.json` entry, no Rust type, no factory arm, no test. | `repo_unimplemented` |
| **Kraken public market data** | Zero presence anywhere in the repo: no `providers.json` entry, no Rust type, no factory arm, no test. | `repo_unimplemented` |
| **Polygon crypto** | `providers.json` declares crypto in `asset_classes`, `implementation_status: "candidate_unverified"`, `enabled: false`, zero Rust client, zero factory arm. | `deferred` |
| **CoinLore public crypto data** | The one provider config scoped exclusively to crypto (`asset_classes: ["crypto"]`), no API key required. `candidate_unverified`, `enabled: false`, zero Rust implementation, zero factory arm today. | `candidate_for_network_verification` — **chosen first verification lane** |
| **AlphaVantage crypto** | `providers.json` declares crypto in `asset_classes`, `implementation_status: "candidate_unverified"`, `enabled: false`, zero Rust implementation. | `deferred` |
| **yfinance crypto** | `providers.json` declares crypto in `asset_classes`, `implementation_status: "candidate_unverified"`, `enabled: false`, zero Rust implementation; unofficial wrapper subject to ToS changes. | `deferred` |
| **Continuing local CSV only** | `mqk_md::ingest_csv::parse_csv_file` + `mqk_db::ingest_provider_bars_to_md_bars_with_provider_metadata` + `mqk-cli md ingest-csv` + `Import-LocalCryptoMarks.ps1`/`Register-LocalCryptoIngestTask.ps1` already prove this path end-to-end for `BTC/USD` and `ETH/USD` (`CRYPTO-DATA-01A`–`01G`). Zero network, zero unverified claims, remains the only proven path today. | `repo_implemented` — remains the baseline until a network lane closes |

No network candidate is rejected as *permanently* unsuitable for crypto
except Alpaca, whose current repo shape (crypto not even declared, no
crypto-endpoint code) makes it structurally unfit for a *first* verification
lane specifically — not a judgment on Alpaca's crypto API in general, which
this repo has never called.

---

## 4. Chosen First Provider Verification Lane and First Implementation Lane

**First network-authorized verification patch:
`CRYPTO-DATA-01I-COINLORE-READONLY-NETWORK-VERIFY-01`.**
Scope for that future patch (not built by this one): one explicit,
operator-authorized, rate-limited, read-only HTTP GET to CoinLore's public
crypto-ticker endpoint for BTC and ETH, confirming the response shape maps
cleanly to this repo's existing `RawBar` fields (symbol, OHLC or last-price,
timestamp — CoinLore's public API is documented as spot-ticker, not OHLCV
bars, so that patch's first job is confirming what shape is actually
available, not assuming daily bars exist). No credential required. No write
to `md_bars`. No CLI change. Evidence of the raw response (with any
rate-limit headers) captured as an artifact, not committed as executable
provider code.

**First implementation patch:
`CRYPTO-DATA-01J-COINLORE-PROVIDER-ADAPTER-LOCAL-INGEST-01`.**
Scope for that future patch (not built by this one, and only after `01I`
succeeds): a `CoinLoreHistoricalProvider` (or capability-appropriate
equivalent) implementing `HistoricalProvider`/`MarketDataProvider`, a fourth
`provider_registry.rs` factory match arm for `"coinlore"`, and a
`providers.json` update flipping `coinlore.implementation_status` to reflect
reality — gated behind `enabled: false` by default, requiring an explicit
operator opt-in before any recurring or CLI-triggered network call. This
patch would **not** touch `ingest-provider`/`sync-provider`'s hardcoded
`"twelvedata"|"alpaca"` gate unless it also proves the crypto-specific
staleness/session semantics those commands assume for equities do not
silently mis-apply to a 24/7 crypto source — an open question carried
forward (§16).

---

## 5. Symbol Convention and Provider Alias Mapping

See §2 item 11 for the full per-layer breakdown. Summary: canonical symbols
(`BTC/USD`, `ETH/USD`) and `md_bars`/route symbols are unchanged and already
proven; only the `provider_symbols` map gains a new key
(`provider_symbols.coinlore`) when `01J` lands, with no schema change
required because that map already exists and already holds one entry
(`local_csv`) per instrument.

---

## 6. Storage Target and Metadata Semantics

Unchanged from `01C`/`01F`: `md_bars` (`symbol, timeframe, end_ts` primary
key) stores crypto bars today with zero migration. Provider-metadata
stamping (`provider_id`, `provider_source`, `ingest_mode`) already exists via
`ingest_provider_bars_to_md_bars_with_provider_metadata` (wired to the CSV
path since `CRYPTO-DATA-01F`) — a future CoinLore adapter would reuse this
same helper with `provider_id = "coinlore"`, `ingest_mode` set to a new,
accurate value (e.g. `"network_provider"`, not `"csv_import"`), requiring no
new column or migration.

---

## 7. Rate-Limit / API-Credit Guardrails

Not exercised by this patch (no network call is made here). Required for the
future `01I`/`01J` patches:

- CoinLore's own documented rate limits must be read from its official docs
  during `01I` (a network-authorized patch, not this one) and recorded in
  `providers.json.rate_limit_notes` before `01J` builds any recurring call.
- Any future automatic/scheduled CoinLore call must reuse the existing,
  currently-unused `MarketDataProviderRateLimits` capability surface
  (`calls_per_minute`, `calls_per_day`, `remaining_calls`, `notes`) — the same
  guardrail `01C` §9 already specified for any future network provider.
- `01I`'s verification call itself must be a single bounded request, not a
  loop or backfill — this is an operator-authorized spot-check, not an
  ingestion run.

---

## 8. Evidence/Status Requirements

For `01I`: a captured raw-response artifact (request URL with no embedded
secret, HTTP status, response body or a truncated/redacted sample, timestamp
of the call) proving what CoinLore actually returns for BTC and ETH —
recorded in that patch's own decision/evidence doc, not fabricated here.

For `01J`: reuse, unchanged, the existing evidence surfaces this lineage
already established — `mqk_db::md::{CoverageQualityReport, MdQualityReport}`
(written to `md_quality_reports` and exported as `data_quality.json` by every
existing ingestion command) and the `exports/market_data/*.json` convention
used by `Refresh-IntradayMarketData.ps1`/`Import-LocalCryptoMarks.ps1`. No
new evidence schema is required.

---

## 9. Failure States and Fail-Closed Behavior

Carried forward from `01C`/`01D` unchanged, plus one CoinLore-specific
addition for the future `01I`/`01J` patches to implement:

- Missing/malformed CoinLore response shape → typed
  `MarketDataProviderError::MalformedResponse`, not a silent empty result.
- CoinLore rate-limited or unavailable →
  `MarketDataProviderError::RateLimited` / `ProviderUnavailable` (both types
  already exist in `provider.rs`; no new error variant needed).
- Disabled provider (`providers.json.coinlore.enabled == false`, the default)
  → `ProviderFactoryError::DisabledProvider`, refused before any network call
  is attempted — the same fail-closed gate every other provider in this
  factory already has (`provider_registry.rs:234`).
- Non-paper DB target, missing file, zero rows, stale bar → unchanged
  existing gates from `01D`'s `Import-LocalCryptoMarks.ps1` convention, which
  a future network-sourced ingest path should mirror rather than replace.

---

## 10. Required Config/Env Variables

None required by this patch. For `01I`/`01J` (future, not created here):
CoinLore's public endpoint requires no API key
(`config/providers/providers.json`'s existing `coinlore.api_key_required:
false`), so no new credential env var is anticipated. If a future
verification finds CoinLore requires a key after all (contradicting current
`providers.json` metadata), that discovery itself would need to update
`providers.json` and add a `credential_env_vars` entry, following the exact
pattern `twelvedata`/`alpaca` already use — this patch does not assume that
outcome.

---

## 11. Required Tests for the Next Implementation Patch

For `01J` (not built here): unit tests for CoinLore response parsing against
a **mocked** HTTP server (the same `httpmock`-based pattern already used in
`alpaca_provider.rs`'s test module — zero real network calls in the test
suite itself), a `provider_registry.rs` factory test proving the new
`"coinlore"` arm builds correctly and is refused when
`providers.json.coinlore.enabled == false` (mirroring
`provider_factory_disabled_provider_fails_truthfully`), and a DB-backed
round-trip test in the same shape as
`scenario_crypto_local_mark_db_persistence_01b.rs`, proving a CoinLore-sourced
bar reaches `md_bars` and back through the unmodified `ASSET-CORE-04`
chain identically to a CSV-sourced one.

---

## 12. What the Next Implementation Patch Should Build

See §4. In order: `01I` (verification, one real bounded network call,
operator-authorized) must land and produce evidence before `01J`
(implementation, provider adapter + factory arm + tests, zero live trading
impact) is attempted. Neither patch is built by this one.

---

## 13. What This Patch Does Not Change

This patch (`CRYPTO-DATA-01H`) adds only: this decision document, its
machine-readable JSON artifact, an optional validator script, and
ledger/audit/runbook updates. It did not touch, and made no behavior change
to: any Rust source file anywhere in `core-rs/crates/mqk-md/src`
(`provider.rs`, `provider_registry.rs`, `ingest_csv.rs`, `alpaca_provider.rs`,
`lib.rs` — all read, not edited), `mqk-db/src` (`md.rs` — read, not edited),
`mqk-daemon`, `mqk-runtime`, `mqk-execution`, `mqk-broker-alpaca`,
`mqk-broker-paper`, `mqk-risk`, or `mqk-portfolio`; any DB migration; any file
under `core-rs/mqk-gui`; `config/instruments/*`; `config/providers/*`;
`.env.local`; `scripts/windows/Import-LocalCryptoMarks.ps1`;
`scripts/windows/Register-LocalCryptoIngestTask.ps1`; or any
strategy/OMS/outbox/scheduler/provider-implementation code. No daemon runtime
was started. No provider, broker, or network call was made. No API credits
were spent. No DB was mutated.

---

## 14. Safety Boundaries

Unconditionally true of this decision and must remain true of the patches
named in §4:

- No live or paper order submitted, ever, for any crypto/futures/options/
  forex instrument.
- No provider/broker network call in this patch. No API credits spent.
- `01I` (future) may make exactly one explicit, operator-authorized,
  read-only, rate-limited network call — never more, never a loop, never a
  write.
- No DB migration. `md_bars`'s existing schema is sufficient (§6).
- No registry-v2 entry with `enabled=true` for any non-equity instrument.
- No change to `PortfolioState`, `compute_portfolio_weights`,
  `/api/v1/portfolio/live-weights`, or `/api/v1/portfolio/economics/status`
  behavior.
- No change to risk, OMS, broker, runtime, or strategy code.
- `01J` (future) must default `providers.json.coinlore.enabled` to `false` —
  any live use requires an explicit, separate operator opt-in.

---

## 15. Open Questions Before Crypto Trading Enablement

Carried forward and extended from `01C` §16 — none of these are answered or
required by this patch:

1. **Does CoinLore's public API actually expose historical OHLCV bars, or
   only a current spot ticker?** This patch does not know — that is exactly
   what `01I`'s single verification call must determine before `01J` assumes
   a bar-shaped response. If CoinLore turns out to be ticker-only, `01J`'s
   scope (or provider choice) would need to change.
2. **Who authorizes the `01I` network call**, and how is that authorization
   recorded? This patch assumes an explicit operator decision, not an
   automatic escalation from this decision doc alone.
3. **Should `ingest-provider`/`sync-provider`'s hardcoded
   `"twelvedata"|"alpaca"` gate be widened to include `"coinlore"`**, or
   should crypto network ingestion get its own CLI subcommand given crypto's
   24/7-session difference from equity market hours? Left open for `01J`.
4. **Should CoinLore ever become a paid/keyed tier if the free public
   endpoint proves insufficient (rate limits, missing history)?** Not
   evaluated here; would require a new decision patch if it arises.
5. **Whether `md_bars`'s symbol-only schema should ever be replaced** with an
   instrument-aware schema — unchanged open question from `ASSET-CORE-04E`
   §15 and `01C` §16, not resolved or advanced by this patch.
6. **Whether crypto's 24/7 session "requirement" should graduate** from
   `ASSET-CORE-05`'s existing scaffold to an authoritative provider before any
   paper-trading consideration — unchanged open question, carried forward
   again.
7. **Account-currency generalization** — unchanged open question from
   `ASSET-CORE-04E`/`01C`; `ASSET-CORE-04D`/`04F`'s route still hardcodes
   `account_currency = "USD"`.

None of these questions block or are answered by this decision. They are
recorded so the next patches (`01I`, `01J`) inherit an honest, explicit list
rather than silently assuming any of them away.
