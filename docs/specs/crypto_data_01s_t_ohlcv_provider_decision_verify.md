# CRYPTO-DATA-01S-T — OHLCV Provider Decision + Read-Only Network Verification

Patch ID: `CRYPTO-DATA-01S-T-OHLCV-PROVIDER-DECISION-VERIFY-BUNDLE-01-COMBINED`

This is decision + bounded read-only network verification only. It is **not**
provider adapter implementation, **not** ingestion implementation, **not**
DB writing, **not** `md_bars` mutation, **not** strategy/risk/execution, and
**not** crypto trading enablement. It continues the crypto data lane after
`CRYPTO-DATA-01I-COINLORE-READONLY-NETWORK-VERIFY-01` and
`CRYPTO-DATA-01N-O-P-LATEST-MARK-EVIDENCE-STATUS-BUNDLE-01-COMBINED`, closing
two small adjacent provider-selection slices together because they share the
same lane, safety profile, and validation matrix:

- `CRYPTO-DATA-01S-CRYPTO-OHLCV-PROVIDER-DECISION-01`
- `CRYPTO-DATA-01T-CRYPTO-OHLCV-READONLY-NETWORK-VERIFY-01`

Decided and verified at HEAD `064ca584`.

---

## 1. Executive Decision

**Selected first completed-bar/OHLCV verification and future adapter
candidate: Kraken's public `/0/public/OHLC` endpoint.** CoinLore
(`CRYPTO-DATA-01I`) proved ticker-spot-only with no OHLC fields and no
per-ticker timestamp — it remains disqualified for completed-bar ingestion,
unchanged by this patch. Of the candidates with any real public,
unauthenticated OHLCV surface, Kraken is the only one this patch could
verify live within the authorized 6-request budget while also confirming,
from the real response body (not from memory or marketing docs), that
completion can be derived honestly rather than fabricated: the response's
`result.last` field is byte-for-byte equal to the second-to-last array row's
timestamp, and the final row sits exactly one interval later — a concrete,
provider-supplied signal for "this is the still-forming candle," not an
assumption.

This patch selects Kraken as the next OHLCV **adapter lane** candidate. It
does **not** build the adapter. `CRYPTO-DATA-01H`'s CoinLore lane and this
patch's Kraken lane are not mutually exclusive decisions about the same
candidate — `01H` picked CoinLore as the safest first *verification* target
given zero OHLCV alternatives were known to be public/keyless at the time;
this patch verifies OHLCV specifically and finds Kraken satisfies that need
where CoinLore does not.

Recommended next patch: **`CRYPTO-DATA-01U-KRAKEN-OHLCV-PROVIDER-ADAPTER-LOCAL-INGEST-01`**
(not built here) — a `KrakenHistoricalProvider`/`MarketDataProvider`
adapter, a fourth `provider_registry.rs` factory arm, and a `providers.json`
entry, gated behind `enabled: false` by default.

---

## 2. Current Repo Facts (verified at HEAD `064ca584`, before any network call)

1. **Which crypto data lanes are already closed?** Local-CSV completed-bar
   ingest (`CRYPTO-DATA-01A`–`01G`, proven end-to-end for `BTC/USD` and
   `ETH/USD` via `mqk_md::ingest_csv` + `mqk_db::ingest_provider_bars_to_md_bars_with_provider_metadata`
   + `mqk-cli md ingest-csv`); the CoinLore live-provider *decision*
   (`01H`) and its *verification* (`01I`, `PARTIAL_TICKER_ONLY` — ticker/spot
   only, no OHLCV); a CoinLore latest-mark model/parser/CLI bundle
   (`01J-K-L-M`); an evidence-file-only latest-mark status route (`01N-O-P`);
   and a read-only GUI panel for that route (`01Q-R`). The portfolio
   economics bridge (`ASSET-CORE-04A`–`04F`) consumes `md_bars` rows
   regardless of provenance. No completed-bar/OHLCV network provider exists
   anywhere in this lineage — that is exactly the gap this patch addresses.

2. **Why is CoinLore not acceptable for completed-bar/OHLCV ingestion?**
   `01I`'s two verified endpoints (`/api/tickers/`, `/api/ticker/?id=`)
   return only `price_usd` (a single current spot value) and `volume24` (a
   rolling 24h figure) — no `open`/`high`/`low` and no per-ticker timestamp
   exist in either response. `01J`'s `LatestMark` type structurally cannot
   produce `RawBar`/`CanonicalBar`'s required fields (proven at the wire
   level by a serialization test asserting the bar-like field names never
   appear). `config/providers/providers.json`'s `coinlore` entry records
   `implementation_status: "latest_mark_parser_implemented_bar_provider_not_applicable"`
   and `verification_status: "ticker_only_network_verified_01i_no_ohlcv"` —
   this is a settled, network-proven fact, not an open question.

3. **Which provider candidates already exist in config only?**
   `alphavantage`, `polygon`, `yfinance` — all `enabled: false`,
   `implementation_status: "candidate_unverified"`, all declare `"crypto"`
   in `asset_classes`, zero Rust type, zero factory arm
   (`config/providers/providers.json`, confirmed unchanged from `01H`).
   Kraken, Coinbase, Binance: **zero presence anywhere** in
   `providers.json`, `mqk-md/src`, or any test, prior to this patch (this
   patch adds no config/code — see §13).

4. **Which provider candidates have Rust implementation today?**
   `TwelveDataHistoricalProvider` (equity-tested only,
   `verification_status: "repo_implemented_official_limits_unverified"`,
   mechanically symbol-agnostic per `01H` §2.4 but never exercised against a
   crypto symbol in any test); `AlpacaHistoricalProvider` (hardcoded to
   `/v2/stocks/bars`, `asset_classes: ["equity","etf"]` only — crypto not
   even declared); CoinLore's `mqk_md::providers::coinlore` ticker
   parser/client (latest-mark only, deliberately not wired into
   `build_market_data_provider_from_config`). No Kraken/Coinbase/Binance
   Rust code exists.

5. **Which provider candidates would require a new provider adapter?**
   All of them, for a completed-bar/OHLCV lane specifically: Kraken,
   Coinbase Exchange, and Binance/Binance.US have zero Rust presence and
   need a new `HistoricalProvider`/`MarketDataProvider` implementation plus
   a new `provider_registry.rs` factory arm (confirmed unchanged — the
   factory (`core-rs/crates/mqk-md/src/provider_registry.rs:247-282`,
   re-read at this HEAD) still has exactly three match arms: `"fake"`,
   `"twelvedata"`, `"alpaca"`; every other string, including any future
   `"kraken"`, falls through to `ProviderFactoryError::UnsupportedProvider`
   at line 281). TwelveData crypto would technically reuse existing code
   but has never been proven against a crypto symbol and requires a shared,
   rate-limited credential (`TWELVEDATA_API_KEY`) this patch is barred from
   spending against (no credentials, no API keys, no keyed API credits).

6. **Which current repo types must a real OHLCV provider eventually
   produce?** `RawBar`/`CanonicalBar` (aliased as `ProviderBar`,
   `core-rs/crates/mqk-md/src/provider.rs:23-43,142`): `symbol`,
   `timeframe`, `end_ts: i64` (UTC epoch seconds, bar **close**), `open`/
   `high`/`low`/`close` (decimal strings), `volume: i64`, `is_complete:
   bool`. The capability-aware `MarketDataProvider` trait
   (`provider.rs:402-422`) additionally requires `provider_id()`,
   `capabilities()`, `health()`, `rate_limits()`,
   `fetch_historical_bars()`, `fetch_latest_closed_bar()`.

7. **Which current DB path would a future adapter eventually reuse?**
   `mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata`
   (already wired to the CSV path since `01F`), keyed on
   `(symbol, timeframe, end_ts)`, provider-agnostic by construction — no
   schema change or migration is needed for a Kraken-sourced row; it would
   set `provider_id = "kraken"`, an accurate `ingest_mode` (e.g.
   `"network_provider"`, not `"csv_import"`), unchanged from `01H` §6's
   analysis of this same reuse path.

8. **Which exact files are sufficient for this decision/verification
   bundle?** Exactly the six listed in this patch's strict file scope
   (§14) — this decision doc, its JSON artifact, a validator script, the
   local-crypto-marks runbook, the patch ledger, and the multi-asset
   completion audit. No Rust/GUI/config file is touched (confirmed in §13).

---

## 3. Candidate Comparison

| Candidate | Repo presence before this patch | Auth required | Live-verified by this patch | Classification |
|---|---|---|---|---|
| **Kraken public OHLC** (`api.kraken.com/0/public/OHLC`) | None (zero config/code/test) | No (`security: []` per official docs, confirmed §5) | **Yes** — 2 bounded GETs, BTC/USD and ETH/USD both confirmed (§6, §7) | **Selected — first OHLCV adapter lane** |
| **Coinbase Exchange product candles** (`api.exchange.coinbase.com/products/{id}/candles`) | None | Publicly documented as unauthenticated for market-data endpoints, **not independently verified live in this patch** (out of scope once Kraken's verification already satisfied the mission with room under the 6-request cap) | No | `repo_unimplemented`, `not_network_verified_this_patch` — plausible future candidate, not ruled out |
| **Binance / Binance.US klines** (`api.binance.com` / `api.binance.us`) | None | Publicly documented as unauthenticated for market-data endpoints, **not independently verified live in this patch**. `api.binance.com` carries known geo-restriction risk for U.S.-origin requests (Binance's own terms restrict U.S. persons on the global site); `api.binance.us` is a separate, geographically-scoped deployment with its own listing/availability constraints — neither claim is verified here, both are flagged honestly as open risk | No | `repo_unimplemented`, `not_network_verified_this_patch`, `geo_restriction_risk_flagged` |
| **TwelveData crypto** | `TwelveDataHistoricalProvider` implemented and compiling, but equity-tested only | Yes — `TWELVEDATA_API_KEY` (shared with equity ingestion) | No — credentialed calls are forbidden by this patch's hard safety rules regardless of merit | `credential_required_excluded_from_this_verification` |
| **Alpaca crypto market data** | `providers.json` declares `asset_classes: ["equity","etf"]` only; `alpaca_provider.rs::bars_url()` hardcoded to `/v2/stocks/bars`, no crypto endpoint code path | Yes — `ALPACA_API_KEY_PAPER`/`ALPACA_API_SECRET_PAPER` | No | `structurally_unfit_for_first_lane` (unchanged conclusion from `01H` §3) |
| **CoinLore** | `LatestMark` model/parser/CLI/route/GUI panel fully built (`01J`–`01R`) | No | Already verified by `01I` (2 GETs, reused here as fact, not re-called) | `ticker_only_network_verified_01i_no_ohlcv` — **not eligible** for a completed-bar lane |
| **Continuing local CSV only** | Proven end-to-end (`01A`–`01G`) | N/A (no network) | N/A | `repo_implemented` — remains the only *proven ingest path* until an adapter patch lands |

No candidate above is rejected as *permanently* unsuitable except Alpaca
(structurally absent crypto endpoint) and CoinLore (network-proven
ticker-only). Coinbase and Binance remain open, undecided candidates — this
patch selects one first lane (Kraken), not a final ranking of all
alternatives.

---

## 4. Why Kraken Was Selected

- **No credential, no API key** — confirmed directly from Kraken's own API
  documentation (`security: []` on the OHLC endpoint spec, §5), matching
  this patch's hard "no credentials" rule with zero ambiguity.
- **Real, documented, and now live-confirmed OHLCV fields** — `time`,
  `open`, `high`, `low`, `close`, `vwap`, `volume`, `count` per entry
  (§5, §6), unlike CoinLore's ticker-only shape.
- **Conservative completion semantics are provable, not assumed.** Kraken's
  own documentation states the last array entry is "the current,
  not-yet-committed timeframe" (§5, confirmed by direct docs fetch, not
  memory). This patch's own two live GETs (§6, §7) independently confirmed
  a *mechanical* signal for this: `result.last` — the cursor Kraken itself
  publishes for incremental polling — is exactly equal to the
  second-to-last row's timestamp in both the BTC and ETH responses, and the
  final row sits exactly one interval (86 400 seconds at `interval=1440`)
  later. This means a future adapter can derive `is_complete` from a
  provider-supplied value (`row.time <= result.last`) rather than from a
  clock-based heuristic or, worse, fabricating completion the way CoinLore
  would have required.
- **Both BTC/USD and ETH/USD resolve cleanly** under Kraken's public
  alt-name convention (`XBTUSD`, `ETHUSD`), confirmed live (§6).
- **No shared-budget risk.** Unlike TwelveData (shared `TWELVEDATA_API_KEY`
  quota with equity ingestion), a Kraken adapter's own rate limit (once
  established) would apply only to crypto calls.

Coinbase Exchange and Binance were not ruled out — they were not
live-tested in this patch because Kraken's verification already answered
the mission's core question (does a real, honest, unauthenticated OHLCV
candidate exist, and can its completion semantics be trusted) within 3 of
the 6 authorized requests, and spending additional requests to run a
three-way live comparison for a decision that only needs to name *one*
first lane was judged disproportionate. If Kraken's future adapter patch
(`01U`) runs into an unforeseen blocker (e.g. an undocumented rate limit
that makes recurring polling impractical), Coinbase Exchange is recorded
here as the most likely next candidate to verify, ahead of Binance, given
Binance's flagged (not yet resolved) geo-restriction risk.

---

## 5. Docs Verification (before any data call)

Fetched (read-only): `https://docs.kraken.com/api/docs/rest-api/get-ohlc-data`
(Kraken's official REST API reference for the OHLC endpoint).

Confirmed directly from that page, not from memory:

- **Authentication:** the endpoint spec declares `security: []` — public,
  no API key, no signature required.
- **Response entry field order (8-element array):** `time` (integer),
  `open` (string), `high` (string), `low` (string), `close` (string),
  `vwap` (string), `volume` (string), `count` (integer).
- **Completion caveat (exact quote):** "The last entry in the OHLC array is
  for the current, not-yet-committed timeframe, and will always be present,
  regardless of the value of `since`." This is the precise fact the
  authorizing prompt flagged and required this patch to verify itself
  rather than trust secondhand — confirmed true.
- **Rate limits:** not present on this specific endpoint-reference page;
  this patch did not fetch a second Kraken docs page to find a numeric
  limit, to keep the total request count minimal (see §8 for the honest
  gap this leaves).

---

## 6. Bounded Live Network Verification

**3 of the 6 authorized GET requests were used** (1 docs fetch + 2 live
data calls). No other host was contacted. No POST/PUT/DELETE. No
credentials, API keys, or auth headers were sent on either data request.

| # | Purpose | Method | URL | Timestamp (UTC, response `Date` header) | HTTP status | Response size |
|---|---|---|---|---|---|---|
| 1 | Confirm OHLC endpoint auth requirement, field order, and completion caveat from official docs | GET | `https://docs.kraken.com/api/docs/rest-api/get-ohlc-data` | 2026-07-05 (docs fetch, no per-request `Date` header captured) | 200 (fetched successfully) | n/a (docs page, summarized not stored) |
| 2 | Live daily-OHLC verification for BTC/USD | GET | `https://api.kraken.com/0/public/OHLC?pair=XBTUSD&interval=1440` | 2026-07-05T20:38:09Z | 200 | 62 277 bytes |
| 3 | Live daily-OHLC verification for ETH/USD | GET | `https://api.kraken.com/0/public/OHLC?pair=ETHUSD&interval=1440` | 2026-07-05T20:38:21Z | 200 | 61 855 bytes |

Both data responses returned `{"error": [], "result": {...}}`. No error was
present in either response. Both requests were single, non-looped,
non-retried GETs with default `curl` headers only (no auth header, no API
key, no cookie sent — Kraken's `Set-Cookie` responses were not reused for
any follow-up request; only 2 data requests were made in total).

---

## 7. BTC/USD and ETH/USD Verification Results

### BTC/USD

- Query pair `XBTUSD` resolved to Kraken's internal pair key `XXBTZUSD` in
  the response — consistent, single unambiguous match.
- 721 daily rows returned (`interval=1440`).
- Sample row (second-to-last, a **committed** bar):
  `[1783123200, "62539.0", "63443.4", "62290.9", "63085.8", "62887.5", "1317.15941434", 34953]`
- Sample row (final array entry, the **forming/current** bar):
  `[1783209600, "63085.4", "63085.4", "62393.3", "62662.2", "62710.6", "546.31624218", 23702]`
- `result.last = 1783123200` — byte-identical to the second-to-last row's
  `time`, confirming that row (and every row before it) is committed, and
  the final row (`1783209600 = 1783123200 + 86400`) is exactly one day
  later — the still-open candle, matching the docs' caveat exactly.
- Normalized mapping for the last **committed** BTC row (not the forming
  one), honestly labeled:
  - `symbol`: `"BTC/USD"` (canonical) ← Kraken pair `"XXBTZUSD"` (would need
    a new `provider_symbols.kraken_pair` alias in a future adapter patch —
    not added here, out of file scope; §9)
  - `timeframe`: `"1D"`
  - `end_ts`: **not** `row.time` directly — Kraken's `time` field is the
    **start** of the bar period (proven by the `result.last`-vs-row-time
    arithmetic above: consecutive committed rows are spaced exactly
    `interval * 60` seconds apart starting from `time`, and `result.last`
    — described by Kraken as the cursor for the last *committed* candle —
    equals the committed row's own `time`, not `time + interval`). A
    future adapter must compute `end_ts = time + interval_seconds` to
    honestly populate `RawBar.end_ts` ("bar end timestamp") — using
    `time` as-is would understate the close time by a full interval.
  - `open`/`high`/`low`/`close`: `"62539.0"`/`"63443.4"`/`"62290.9"`/`"63085.8"`
    — real values, no fabrication needed.
  - `volume`: `"1317.15941434"` — a decimal string in **base-currency
    units (BTC)**, not an integer; `RawBar.volume` is typed `i64`, so a
    future adapter must decide a truthful integer representation (e.g.
    scaled satoshis, or a documented precision-loss truncation) — this
    patch does not decide that conversion, it only surfaces the mismatch
    honestly.
  - `is_complete`: `true` for this row, derivable from
    `row.time <= result.last`.

### ETH/USD

- Query pair `ETHUSD` resolved to Kraken's internal pair key `XETHZUSD`.
- 721 daily rows returned.
- Sample row (second-to-last, committed):
  `[1783123200, "1756.30", "1805.52", "1742.93", "1778.72", "1777.37", "15588.01759742", 16101]`
- Sample row (final, forming):
  `[1783209600, "1778.72", "1784.35", "1747.15", "1775.24", "1771.74", "13144.87516625", 11548]`
- `result.last = 1783123200` — identical pattern to BTC: matches the
  second-to-last row exactly; the final row is exactly one interval later.
- Normalized mapping for the last committed ETH row: `symbol="ETH/USD"`
  (Kraken pair `"XETHZUSD"`), `timeframe="1D"`,
  `end_ts = 1783123200 + 86400 = 1783209600` (per the same start-vs-end
  correction above), `open="1756.30"`, `high="1805.52"`, `low="1742.93"`,
  `close="1778.72"`, `volume="15588.01759742"` (base-currency ETH units,
  same `i64`-conversion open question as BTC), `is_complete=true`.

Both symbols behave identically: same field shape, same completion-cursor
mechanics, same forming-candle exclusion rule. No fabrication was required
for either symbol's OHLC fields — every value came directly from the
provider response.

---

## 8. Rate-Limit and Credential Posture

- **Credentials:** none required or used. Kraken's own OHLC endpoint spec
  declares `security: []`.
- **Rate limits:** **not established by this patch.** The specific
  docs page fetched (§5) did not carry rate-limit numbers for public
  market-data endpoints; this patch did not fetch a second docs page to
  find them, to keep the total request count to 3 of the authorized 6.
  Kraken is publicly known to operate a tiered "API Counter" rate-limit
  system for some endpoint categories, but this patch does not assert a
  specific number since it did not independently verify one — this is
  recorded as an explicit open item for `01U`, not fabricated here.
  Neither live response carried an `X-RateLimit-*`/`Retry-After` header.
- **Geo/licensing:** not evaluated in this patch beyond confirming the two
  live requests succeeded from this environment. A future adapter patch
  should not assume identical network reachability from every deployment
  environment (e.g. a cloud region with different Kraken access policies)
  without its own check.

---

## 9. Symbol Mapping

- **Canonical registry symbols:** `"BTC/USD"` / `"ETH/USD"` — unchanged,
  already proven at every existing layer (`md_bars`, portfolio economics
  route, GUI).
- **Kraken query alt-name:** `"XBTUSD"` / `"ETHUSD"` (confirmed live to
  resolve unambiguously, §7).
- **Kraken response pair key:** `"XXBTZUSD"` / `"XETHZUSD"` (Kraken's
  internal identifier, present as the JSON object key wrapping each pair's
  row array — confirmed live, §7).
- **`provider_symbols` map:** the existing flat `BTreeMap<String, String>`
  on each registry-v2 instrument (already holding `local_csv`,
  `coinlore_id`, `coinlore_symbol` per instrument) can carry a new
  `kraken_pair` (or `kraken_alt_name`) key with **no schema change** —
  exactly the same extensibility `01L` already used for CoinLore. This
  patch does **not** add that key (`config/instruments/*` is out of file
  scope here) and does not assert which exact value (`"XBTUSD"` vs
  `"XXBTZUSD"`) a future adapter should store without that adapter's own
  design decision — both were observed live and either is viable.

---

## 10. Completion Semantics (Summary)

1. Kraken's own documentation states the last OHLC array entry is always
   the current, not-yet-committed candle (§5) — independently confirmed
   live: the final row in both the BTC and ETH responses sits exactly one
   interval after `result.last` (§7).
2. A future adapter must **drop or explicitly mark incomplete** every row
   whose `time > result.last` (in practice, today, this is only ever the
   final array entry, but a future adapter should implement the general
   rule, not a fixed "drop the last row" special case, since Kraken's own
   wording ("regardless of the value of `since`") implies this is a
   structural property of the endpoint, not an artifact of the exact
   `since` value used).
3. `RawBar.end_ts` must be computed as `row.time + interval_seconds`, not
   read directly from `row.time` (§7) — `row.time` is the bar's **start**,
   confirmed by the `result.last`-vs-row arithmetic, not an assumption.
4. No OHLC field is fabricated anywhere in this verification: every
   `open`/`high`/`low`/`close`/`volume` value quoted in §7 is copied
   verbatim from the real response body.

---

## 11. Future Adapter Plan (Not Built by This Patch)

For a future, separately-authorized `CRYPTO-DATA-01U-KRAKEN-OHLCV-PROVIDER-ADAPTER-LOCAL-INGEST-01`:

1. **Parser only, first:** a pure function parsing Kraken's
   `{"error": [...], "result": {"<pair>": [[time, open, high, low, close,
   vwap, volume, count], ...], "last": <ts>}}` shape into
   `RawBar`/`CanonicalBar`, applying the `end_ts = time + interval_seconds`
   correction and the `is_complete = time <= result.last` rule (§10),
   tested against a **mocked** HTTP response (no live network call in
   tests), mirroring the existing `httpmock`-based pattern already used in
   `alpaca_provider.rs`'s test module.
2. **Provider adapter:** a `KrakenHistoricalProvider` implementing
   `HistoricalProvider`/`MarketDataProvider`, plus a fourth
   `provider_registry.rs` factory match arm for `"kraken"` (today's factory
   is unchanged at exactly three arms — §2 item 5) — gated behind
   `providers.json`'s existing `enabled` flag pattern, default `false`.
3. **CLI dry-run/evidence:** a read-only CLI surface mirroring
   `mqk md coinlore-latest-mark`'s fixture-first, network-opt-in design
   (`--input-file` default, an explicit env-var opt-in for a real network
   smoke test) before any recurring/scheduled call is considered.
4. **DB ingest only after proof:** reuse
   `ingest_provider_bars_to_md_bars_with_provider_metadata` unchanged
   (§2 item 7) — no migration required — only after the parser and adapter
   tests above pass and an operator explicitly authorizes a DB-writing
   patch.
5. **GUI/status later:** an operator-visible status surface, if warranted,
   following the same evidence-file-first pattern `01N`–`01R` already
   established for CoinLore, not assumed to require a new pattern.
6. **Volume-unit and `i64` conversion decision** (§7) and **numeric
   rate-limit determination** (§8) are both explicitly deferred to `01U`,
   not resolved by this patch.

---

## 12. Safety Boundaries

- No live or paper order submitted, ever.
- Exactly 3 bounded, non-looped, non-retried, read-only GET requests were
  made in this entire patch (1 docs fetch + 2 data calls), well within the
  authorized maximum of 6.
- No POST/PUT/DELETE. No credentials, API keys, or auth headers sent or
  used. No `.env.local` read.
- No DB connection opened. No DB mutation. No DB migration.
- No `md_bars` write. No `latest_marks` write.
- No provider adapter, factory arm, or CLI ingestion path added.
- No Rust source file, GUI file, or config file changed (confirmed §13).
- No raw response body committed — raw responses were written only to the
  session scratchpad directory outside the repo; this document and its
  JSON artifact contain curated summaries only.
- No crypto/futures/options/forex trading enabled at any point.

---

## 13. What This Patch Does Not Change

This patch adds only: this decision/verification document, its
machine-readable JSON artifact, a validator script, and
runbook/ledger/audit updates. It did **not** touch, and made no behavior
change to: any file under `core-rs/crates/mqk-md/src` (`provider.rs`,
`provider_registry.rs`, `ingest_csv.rs`, `alpaca_provider.rs`,
`providers/coinlore.rs`, `latest_mark.rs`, `lib.rs` — all read via this
patch's inspection, not edited); `core-rs/crates/mqk-db/src` (`md.rs` —
read, not edited); `core-rs/crates/mqk-cli/src/commands/md.rs` (read, not
edited); `mqk-daemon`, `mqk-runtime`, `mqk-execution`, `mqk-broker-alpaca`,
`mqk-broker-paper`, `mqk-risk`, or `mqk-portfolio`; any DB migration; any
file under `core-rs/mqk-gui`; `config/instruments/*`;
`config/providers/providers.json`; `.env.local`;
`scripts/windows/Import-LocalCryptoMarks.ps1`;
`scripts/windows/Register-LocalCryptoIngestTask.ps1`; or any
strategy/OMS/outbox/scheduler/provider-implementation code. No daemon
runtime was started. No provider, broker, or network call other than the
3 documented in §6 was made. No API credits were spent (Kraken's public
endpoint requires none). No DB was mutated.

---

## 14. Remaining Blockers / Open Items

1. **No Kraken adapter exists.** This patch is decision + verification
   only; `01U` (or equivalent) must build the parser, provider, factory
   arm, and tests before any Kraken-sourced bar can reach `md_bars`.
2. **Kraken's numeric rate limit is unknown** (§8) — must be established
   before any recurring/scheduled call is built.
3. **`RawBar.volume`'s `i64` type vs. Kraken's fractional base-currency
   volume string** (§7) is an unresolved representation decision for the
   adapter patch.
4. **`ingest-provider`/`sync-provider` remain hard-locked** to
   `"twelvedata"|"alpaca"` — unchanged by this patch, an open question for
   whichever future patch adds the Kraken factory arm.
5. **Coinbase Exchange and Binance/Binance.US remain unverified
   candidates** — not ruled out, simply not live-tested in this patch
   (§4). Binance in particular carries an unresolved, honestly-flagged
   geo-restriction risk.
6. **No production registry-v2 cutover, no crypto session/calendar runtime
   enforcement, no crypto risk policy activation, no crypto broker/paper
   execution, no crypto strategy** — all unchanged, all still fully open.
7. Local CSV import remains the only **proven** crypto `md_bars` ingest
   path until `01U` (or equivalent) actually lands and its own DB-backed
   proof closes.

This patch does not imply crypto trading readiness, completed-bar provider
readiness, or OHLCV adapter readiness in any respect.
