# CRYPTO-DATA-01I — CoinLore Read-Only Network Verification

Patch ID: `CRYPTO-DATA-01I-COINLORE-READONLY-NETWORK-VERIFY-01`

This is a verification/evidence patch. It is **not** provider implementation,
**not** recurring ingestion, **not** scheduler execution, **not** DB
ingestion, **not** DB mutation, **not** a production registry-v2 cutover,
**not** a portfolio-ledger cutover, **not** risk enforcement, **not** order
routing, **not** broker integration, **not** a DB migration, and **not**
crypto trading enablement. It performs the first explicitly authorized,
read-only, bounded network verification of CoinLore as the crypto provider
candidate chosen by `CRYPTO-DATA-01H-LIVE-CRYPTO-PROVIDER-DECISION-01-COMBINED`
(decided at HEAD `6271c048`), continuing the crypto data lane after
`CRYPTO-DATA-01A`–`01H`/`ASSET-CORE-04F`.

Verified at HEAD `4eaa14c2`.

---

## 1. Verification Scope and Authorization

The operator prompt for this patch explicitly authorized a bounded, read-only
network-verification sequence to CoinLore's public API for BTC and ETH only:
at most 3 HTTP GET requests, no credentials, no API keys, no writes, no
loops, no backfills, no scheduled calls, no provider implementation. This
document records the real requests made under that authorization and their
real responses. No other network host was contacted (no Alpaca, TwelveData,
Polygon, yfinance, Coinbase, Kraken, Binance, IBKR).

---

## 2. Exact Request Count and URLs Used

**2 of the allowed 3 HTTP GET requests were made.** No third request was
necessary because request 1 already returned both BTC and ETH data, and
request 2 (the targeted per-ID endpoint that a real adapter would call)
independently confirmed the identical shape for both symbols in a single
call.

| # | Purpose | URL | Timestamp (UTC) | HTTP status |
|---|---|---|---|---|
| 1 | Discover CoinLore's internal numeric IDs for BTC/ETH and confirm the top-level ticker-list response shape | `https://api.coinlore.net/api/tickers/` | 2026-07-05T15:10:42Z (server `Date` header) | 200 |
| 2 | Confirm the targeted per-ID ticker endpoint (the shape a future adapter would actually call for two configured symbols) returns the same fields for both BTC and ETH in one call | `https://api.coinlore.net/api/ticker/?id=90,80` | 2026-07-05T15:11:21Z (server `Date` header) | 200 |

No credentials, no API key, no header other than a default `curl` User-Agent
were sent on either request. Both requests were single, non-looped, GET
calls with no pagination and no retry.

---

## 3. Current CoinLore API Observations

- Both endpoints are served behind Cloudflare (`Server: cloudflare`), return
  `Content-Type: application/json`, and require no authentication — a bare
  GET returns `200 OK`.
- Response headers on both requests: `Cache-Control: no-store`,
  `Cf-Cache-Status: EXPIRED`, `X-Cache: HIT` (Cloudflare edge-cache
  bookkeeping, not an application-level rate-limit signal), `Access-Control-
  Allow-Origin: *`, `Vary: Accept-Encoding`, standard Cloudflare `Nel`/
  `Report-To`/`CF-RAY`/`alt-svc` diagnostic headers.
- **No rate-limit headers were present on either response** (no
  `X-RateLimit-Limit`, `X-RateLimit-Remaining`, `Retry-After`, or equivalent).
  This patch did not fetch CoinLore's marketing/docs page
  (`https://www.coinlore.com/cryptocurrency-data-api`) to look for
  officially-documented rate-limit guidance, to stay within the smallest
  possible request count — this is recorded as an open item for `01J`, not
  fabricated here.
- `/api/tickers/` (request 1) returns a top-100-by-market-cap list; response
  size 37,100 bytes. Top-level shape: `{"data": [...100 ticker objects...],
  "info": {"coins_num": 14471, "time": 1783264203}}`. The `info.time` field
  is a single global Unix-epoch timestamp for the whole list — it is **not**
  a per-ticker timestamp.
- `/api/ticker/?id=<id1>,<id2>` (request 2) returns a bare JSON array of
  ticker objects for exactly the requested IDs; response size 725 bytes.
  Confirmed comma-separated multi-ID query works in a single call (both BTC
  `id=90` and ETH `id=80` returned together).
- Both endpoints return **the identical per-ticker field set**:
  `id`, `symbol`, `name`, `nameid`, `rank`, `price_usd`, `percent_change_24h`,
  `percent_change_1h`, `percent_change_7d`, `price_btc`, `market_cap_usd`,
  `volume24`, `volume24a`, `csupply`, `tsupply`, `msupply`. **No ticker
  object anywhere carries its own timestamp field** (no `last_updated`,
  no `time`, no `ts` per item).

---

## 4. BTC Result Summary

- Identified reliably in both requests as `id="90"`, `symbol="BTC"`,
  `name="Bitcoin"`, `rank=1` — consistent between the top-100 list (request
  1) and the targeted per-ID lookup (request 2).
- `price_usd="62906.61"` (decimal string) present in both responses,
  identical value across both calls (~39 seconds apart), consistent with a
  short-interval upstream cache rather than a data error.
- `volume24=16261421089.448309` — a **rolling 24-hour trading volume**
  (floating-point JSON number), not a per-bar volume tied to a specific
  timeframe.
- No `open`, `high`, `low`, or `close` fields anywhere in the response. No
  per-ticker timestamp.

---

## 5. ETH Result Summary

- Identified reliably in both requests as `id="80"`, `symbol="ETH"`,
  `name="Ethereum"`, `rank=2` — consistent between both endpoints.
- `price_usd="1777.74"` (decimal string) present in both responses, identical
  value across both calls.
- `volume24=9252669028.469555` — same rolling-24h-volume caveat as BTC.
- Same absence of `open`/`high`/`low`/`close` and per-ticker timestamp as BTC.

---

## 6. Response-Shape Mapping to MiniQuantDesk

Repo shapes (confirmed by direct read of
`core-rs/crates/mqk-md/src/provider.rs` before this verification, unchanged
by this patch):

- **`RawBar`** requires `symbol`, `timeframe`, `end_ts: i64` (UTC epoch
  seconds), `open`/`high`/`low`/`close` (decimal strings), `volume: i64`,
  `is_complete: bool`.
- **`CanonicalBar`** (aliased as `ProviderBar` — the type
  `MarketDataProvider::fetch_historical_bars`/`fetch_latest_closed_bar`
  actually return) has the identical field set to `RawBar`.
- **`md_bars`** storage key is `(symbol, timeframe, end_ts)`; provider
  metadata (`provider_id`, `provider_source`, `ingest_mode`) is stamped by
  the existing, unmodified `ingest_provider_bars_to_md_bars_with_provider_metadata`
  helper — provider-agnostic by construction (§8 of `01H`'s decision doc,
  reconfirmed unread-only here).

Mapping outcome:

- `symbol` -> CoinLore's `symbol` field maps directly (`"BTC"`/`"ETH"`);
  the canonical registry symbol (`"BTC/USD"`/`"ETH/USD"`) would need a new
  `provider_symbols.coinlore` alias entry (e.g. `{"coinlore_id": "90"}` or
  `{"coinlore": "BTC"}`) in a future `01J` patch — this patch does not add
  that key (`config/instruments/*` is out of file scope for `01I`).
- `open`/`high`/`low` -> **do not exist in CoinLore's response.** There is no
  honest, non-fabricated way to populate these three `RawBar` fields from
  either endpoint verified here.
- `close` -> `price_usd` is the only single-value price CoinLore exposes; it
  could honestly populate `close` **only if `open`/`high`/`low` are not
  claimed to be real** (see §7).
- `volume` -> CoinLore's `volume24` is a rolling 24-hour figure, not a
  bar-period volume. Using it as a `RawBar.volume` for a `"1D"` timeframe bar
  would be directionally plausible but is not verified here to align exactly
  with any specific bar-close moment — a `01J` decision, not asserted as fact
  by this patch.
- `end_ts` -> **no per-ticker timestamp exists in either response.** The only
  candidate value is the verification-call's own request time (an "as-of"
  timestamp the *client* would generate, not one CoinLore supplies).
  Populating `end_ts` this way is honest only if labeled as a
  client-observed snapshot time, not a provider-supplied bar-close time.
- `is_complete` -> CoinLore does not expose any completed/forming bar
  concept. Setting `is_complete=true` for a `RawBar` built this way would be
  a factual claim CoinLore's response does not support — it is a single
  point-in-time spot value, not a closed bar.

---

## 7. OHLCV vs Ticker-Only Conclusion

**CoinLore's verified public endpoints (`/api/tickers/`, `/api/ticker/`) are
ticker/spot-only. Neither exposes OHLCV history or any per-item timestamp.**
This confirms, with real response evidence, the uncertainty `01H` §15
explicitly flagged ("Does CoinLore's public API actually expose historical
OHLCV bars, or only a current spot ticker? This patch does not know.") —
now resolved: **spot ticker only**, for the two endpoints verified. This
patch made no request to `/api/coin/markets/?id=` (a plausible historical/
per-market endpoint) or any other CoinLore path, so this conclusion is
scoped strictly to the two endpoints actually called, not a claim about
every CoinLore endpoint in existence.

A `RawBar`/`CanonicalBar` built from this data would require **fabricating**
`open`, `high`, `low` (by copying `close`) and fabricating `end_ts`/
`is_complete` (by asserting client-observed time as if it were a
provider-confirmed bar close) — both of which this patch's authorizing
prompt and `CLAUDE.md`'s "no fabricated truth" invariant forbid. Therefore
this data **cannot honestly populate the existing `RawBar`/`ProviderBar`
model as a completed bar** without misrepresenting three-quarters of the
OHLC fields and the completion semantics.

It **can** honestly model a **"latest mark" concept distinct from a
completed bar** — a single `(symbol, price_usd, as_of_client_request_time)`
snapshot, explicitly labeled as a spot mark, not a bar. Whether `01J` reuses
`RawBar`/`ProviderBar` with an explicit fabrication-free convention (e.g.
`open=high=low=close`, `is_complete=false` always, `end_ts` = the request's
own wall-clock time) and documents that convention loudly, or introduces a
distinct non-bar type, is a `01J` design decision this patch does not make.

---

## 8. Rate-Limit / Fair-Use Observations

- No `X-RateLimit-*`, `Retry-After`, or any other machine-readable
  rate-limit header was present on either response.
- `Cache-Control: no-store` on both responses indicates CoinLore does not
  want the *client* caching the response, independent of the `Cf-Cache-
  Status`/`X-Cache` values which describe Cloudflare's own edge cache in
  front of the origin.
- This patch did not fetch CoinLore's public documentation page for
  official rate-limit guidance (to keep the request count minimal); no
  official rate-limit number is recorded here. `01J` must not assume an
  unlimited or high rate limit from the mere absence of a header — the
  absence of a header is not evidence of no limit.

---

## 9. Failure Modes Observed or Inferred

**Observed:** both requests returned `200 OK` with well-formed JSON; no
error path was exercised (no invalid ID, no malformed query, no network
failure was deliberately triggered, consistent with the "no loops, no
retries" scope of this patch).

**Inferred, not observed (must not be treated as verified fact):** an
unknown or invalid `id` value passed to `/api/ticker/?id=` would plausibly
return an empty JSON array `[]` with `200 OK` rather than a `4xx` status,
based on the shape of the endpoint (array-of-matches, not object-keyed-by-
id) — this patch did not test that path and does not assert it as
confirmed. A future `01J` adapter must treat an empty-array response as
`MarketDataProviderError::EmptyResponse`, not silently succeed with zero
bars, and must treat any non-JSON or unexpected-shape body as
`MarketDataProviderError::MalformedResponse` — both error types already
exist in `provider.rs` unchanged.

---

## 10. Decision

**`PARTIAL_TICKER_ONLY`**

CoinLore reliably identifies BTC and ETH and returns USD-denominated spot
price data for both (§4, §5) — the provider is not rejected as unfit. But
the two endpoints verified here expose **no OHLCV history and no per-ticker
timestamp** (§6, §7), so `01J` cannot proceed exactly as `01H` originally
scoped it (a `HistoricalProvider`/`RawBar`-shaped adapter assuming real
daily bars). `01J`'s scope must be adapted to a ticker/latest-mark model,
explicitly documented as such, or `01J` must first make one additional,
separately-authorized verification call to `/api/coin/markets/?id=` (an
endpoint this patch did not call) to check whether a market/history view
exists before assuming ticker-only is CoinLore's full capability.

---

## 11. Required Exact Scope for 01J

1. Do **not** assume `RawBar`/`CanonicalBar`'s `open`/`high`/`low` can be
   populated with real values from `/api/tickers/` or `/api/ticker/`.
2. If `01J` proceeds with a ticker-only model, it must either (a) introduce
   an explicit "latest mark" type distinct from a completed bar, or (b)
   reuse `RawBar`/`ProviderBar` only with a loudly-documented,
   non-fabrication convention (e.g. `open=high=low=close=price_usd`,
   `is_complete=false` always, `end_ts` = request wall-clock time, not a
   provider-supplied value) — and this convention must be stated in
   `01J`'s own decision doc, not silently assumed.
3. If `01J` instead wants real OHLCV, it must first make its own
   additional, explicitly-authorized verification call to
   `/api/coin/markets/?id=` (or another undiscovered CoinLore endpoint) —
   not assume one exists from this patch's findings.
4. `01J` must implement `MarketDataProviderError::EmptyResponse` and
   `MalformedResponse` handling for the inferred-but-unobserved failure
   modes in §9.
5. `01J` must default `providers.json.coinlore.enabled=false` (per `01H`
   §14) and gate any network call behind that flag, refusing before any
   request is attempted when disabled — mirroring
   `ProviderFactoryError::DisabledProvider`'s existing fail-closed pattern.
6. `01J` must record CoinLore's still-unknown official rate limit as an
   open item, not assume a specific numeric budget from this patch's
   header-absence observation.
7. `01J` must add `provider_symbols.coinlore` to
   `config/instruments/instruments_v2.crypto_local_marks.example.json` using
   whichever alias shape it chooses (bare ticker `"BTC"`/`"ETH"`, or the
   numeric ID `"90"`/`"80"` confirmed stable across both requests in this
   patch) — out of file scope for `01I`.

---

## 12. What This Patch Did Not Change

This patch (`CRYPTO-DATA-01I`) added only: this evidence document, its
machine-readable JSON artifact, an optional validator script, and
runbook/ledger/audit updates. It did not touch, and made no behavior change
to: any Rust source file anywhere in `core-rs/crates/mqk-md/src`
(`provider.rs`, `provider_registry.rs`, `ingest_csv.rs`, `alpaca_provider.rs`,
`lib.rs` — all read, not edited); `mqk-db/src` (`md.rs` — read, not edited);
`mqk-cli/src/commands/md.rs` (read, not edited); `mqk-daemon`, `mqk-runtime`,
`mqk-execution`, `mqk-broker-alpaca`, `mqk-broker-paper`, `mqk-risk`, or
`mqk-portfolio`; any DB migration; any file under `core-rs/mqk-gui`;
`config/instruments/*`; `config/providers/*`; `.env.local`;
`scripts/windows/Import-LocalCryptoMarks.ps1`;
`scripts/windows/Register-LocalCryptoIngestTask.ps1`; or any
strategy/OMS/outbox/scheduler/provider-implementation code. No daemon
runtime was started. No provider or broker code was written. No DB was
mutated. No trading was enabled.

---

## 13. Safety Confirmation

- No live or paper order was submitted.
- No daemon runtime was started.
- No autonomous runtime was run.
- No market-hours proof was run.
- No provider script was called (`Import-LocalCryptoMarks.ps1`,
  `Register-LocalCryptoIngestTask.ps1` were not invoked).
- No broker API was called — only CoinLore's public, keyless crypto-ticker
  endpoints.
- No API credits were spent (CoinLore requires no key; the two GETs made
  were unauthenticated and free).
- No credentials were used or read. `.env.local` was not read.
- No DB was mutated. No DB migration was added or changed.
- No Rust provider implementation, CLI command, scheduler script, or daemon
  route was changed.
- No GUI file was changed.
- No broker, OMS, order-submit, risk, runtime, or strategy behavior changed.
- No crypto/futures/forex/options trading was enabled.
- Exactly 2 bounded, non-looped, non-retried, read-only HTTP GET requests
  were made, both to `api.coinlore.net`, both within the 3-request cap
  authorized for this patch.
- No raw large response dump was staged; the raw response bodies and headers
  were captured only to the session scratchpad directory (outside the repo)
  and are not part of this commit — only the curated summaries in this
  document and the JSON artifact below are committed.
