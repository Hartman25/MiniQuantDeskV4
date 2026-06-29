# ASSET-CORE-04E — Non-Equity Mark-Data Source Decision

Patch ID: `ASSET-CORE-04E-NON-EQUITY-MARK-DATA-SOURCE-DECISION-01-COMBINED`

This is a decision/spec patch. It is **not** provider execution, **not** live
market-data ingestion, **not** a production portfolio-ledger cutover, **not**
a live valuation cutover, **not** risk enforcement, **not** order routing,
**not** broker integration, **not** a DB migration, and **not** non-equity
enablement. It makes one current-repo-grounded decision: which non-equity
asset class and which source phase should be the first to feed
`ASSET-CORE-04A`/`04B`/`04C`/`04D` with real marks instead of hand-built
fixtures, and records exactly what is required (and not yet built) to get
there.

---

## 1. Executive Decision

**Chosen lane: crypto spot, local-CSV-fixture-first.** The first non-equity
mark-data source for `ASSET-CORE-04` should be a deterministic, local,
no-network CSV source of OHLCV bars for one or two USD-quoted crypto spot
pairs (`BTC/USD`, `ETH/USD`), loaded through the market-data layer's
*existing* CSV-ingestion path into the *existing* `md_bars` schema, behind a
*new but still-disabled* registry-v2 entry. No provider/network call, no DB
migration, and no trading enablement of any kind is part of this decision or
required to prove it.

Recommended next implementation patch: **`CRYPTO-DATA-01A-LOCAL-CSV-MARKS-01`**
(see §12).

This recommendation matches the mission brief's own preferred target and is
independently corroborated, not merely assumed: it is the same conclusion
`docs/audits/multi_asset_completion_audit.md` reached from a full repo audit
(§7-§9, "Crypto first because it is structurally the simplest non-equity
class"), and it is the same conclusion this repo's own
`mqk-execution::asset_risk_policy` module already encodes in working code
(see §2, fact 9).

---

## 2. Current Repo Facts

Grounded by direct file reads at HEAD `1b47a639`, answering the mission's
ten pre-flight questions in order.

1. **Market-data provider traits that exist today** (`core-rs/crates/mqk-md/src/provider.rs`):
   `Provider` (legacy, sync, raw `RawBar`), `HistoricalProvider` (async, used
   by the Alpaca and TwelveData adapters), and `MarketDataProvider` (the
   newer capability-aware async contract — `capabilities()`, `health()`,
   `rate_limits()`, `fetch_historical_bars`, `fetch_latest_closed_bar` —
   which wraps a `HistoricalProvider` via `HistoricalProviderMarketDataAdapter`).
   `ProviderAssetClass` (`Equity`/`Etf`/`Crypto`/`Futures`/`Options`/`Forex`/`Other`)
   is a capability-metadata tag only; it gates nothing.

2. **Asset classes provider configs claim to support**
   (`config/providers/providers.json`): `twelvedata` declares
   `equity/etf/forex/crypto`; `alphavantage`/`polygon` declare
   `equity/etf/options/futures/forex/crypto`; `yfinance` declares
   `equity/etf/futures/forex/crypto`; `coinlore` declares `crypto` only.
   These `asset_classes` arrays are **aspirational capability metadata**, not
   implementation proof — every one of those providers except `twelvedata`
   and `alpaca` is `"implementation_status": "candidate_unverified"` /
   `"enabled": false`.

3. **Provider implementations that actually exist in code**: exactly two —
   `AlpacaHistoricalProvider` (`alpaca_provider.rs`) and
   `TwelveDataHistoricalProvider` (`lib.rs`), both implementing
   `HistoricalProvider`, both wired today only against equity symbols from
   `config/instruments/equities.json`. No AlphaVantage, Polygon, yfinance, or
   CoinLore Rust implementation exists anywhere in `mqk-md` — those four are
   config-only candidates with zero code.

4. **Do any provider implementations truly support crypto/futures/options/forex
   today, or only declare metadata?** Only metadata. Even the structurally
   closest case — `TwelveDataHistoricalProvider`'s HTTP transport is
   mechanically symbol-agnostic (it sends whatever symbol string it is given
   to TwelveData's generic `/time_series` endpoint and parses a generic
   OHLCV JSON shape) — is self-labeled `"implemented_equity_provider"` in
   `providers.json`, has never been exercised in this repo with a non-equity
   symbol in any test or production code path, and carries
   `"verification_status": "repo_implemented_official_limits_unverified"`.
   Treating it as a working crypto source today would be exactly the kind of
   unverified claim `providers.json` itself already flags for every broader
   capability declaration. `coinlore`, the one provider config scoped to
   crypto specifically, has zero implementation and `enabled: false`.

5. **What `md_bars` stores today, and is it symbol-only or instrument-aware?**
   Symbol-only. `core-rs/crates/mqk-db/migrations/0003_backtest_schema.sql`
   (unchanged in this respect through the current latest migration, `0043`)
   defines `md_bars` with primary key `(symbol, timeframe, end_ts)`; `symbol`
   is a bare `text` column. Migration `0042_md_bars_provider_metadata.sql`
   added `provider_id`/`provider_source`/`provider_symbol`/`ingest_mode`/
   `provider_bar_id`/`provider_updated_at_utc` columns, but still no
   `instrument_id` and no `asset_class` column.

6. **Does any current table or struct carry `instrument_id` into bars?** No.
   Confirmed by reading the full live mark-lookup chain end-to-end
   (`mqk-daemon/src/routes/portfolio.rs::portfolio_economics_status`,
   `ASSET-CORE-04D`): execution-snapshot position `pos.symbol` → the
   `ASSET-CORE-04B` bridge keyed by `r.symbol` →
   `mqk_db::fetch_recent_completed_bars_for_strategy(pool, symbol, timeframe, 1)`
   → `select ... from md_bars where symbol = $1 and timeframe = $2`. Every
   link in that chain is a bare `symbol: &str`/`String`; `instrument_id`
   never appears.

7. **Can `md_bars` safely store `BTC/USD` or futures symbols without
   ambiguity?** Yes, mechanically, with no schema change. `md_bars.symbol` is
   an unconstrained `text` column read/written through parameterized binds
   (`sqlx::query(...).bind(symbol)`) — no charset or format constraint at the
   DB layer. The generic CSV ingestion path
   (`core-rs/crates/mqk-md/src/ingest_csv.rs`) is column-mapped,
   case-insensitive, and equally symbol-format-agnostic. No symbol in the
   current 88-row production equity registry contains a `/`, so a
   slash-delimited pair string like `"BTC/USD"` is unambiguous against the
   existing universe. (A real implementation patch still must *pick* one
   canonical symbol-string convention — e.g. `"BTC/USD"` vs `"BTCUSD"` —
   since the schema itself enforces nothing; see §6.)

8. **Does registry-v2 have enough metadata for crypto/futures/options/forex
   contracts?** Yes for all four, and crypto's shape is the simplest.
   `core-rs/crates/mqk-md/src/instrument_registry_v2.rs` (`ASSET-CORE-01B`)
   already defines validated `ContractDefinitionV2::CryptoPair{base,quote}`,
   `Future{root,expiry,multiplier,tick_size_micros}`,
   `Option{underlying,expiry,strike_micros,right,multiplier}`, and
   `ForexPair{base,quote}` — every non-equity `enabled=true` row fails
   closed unless a test-only `allow_enabled_non_equity_for_testing` flag is
   set. A committed example fixture
   (`config/instruments/instruments_v2.backtest_suggestions.example.json`)
   already demonstrates two disabled futures (`ES_TEST`, `MES_TEST`) and one
   disabled crypto pair (`BTCUSD_TEST`) with explicit economics multiplier
   metadata, proven by repo tests (`ex01`/`ex02`/`v2status_06`). No options
   or forex fixture exists yet — crypto and futures are the only two
   non-equity classes with any committed fixture precedent today, and
   `CryptoPair{base,quote}` (two required strings, no expiry/strike/
   multiplier/tick-size fields) is the simplest contract shape of the four.

9. **What non-equity asset class has the lowest implementation risk for real
   marks?** Crypto — confirmed by this repo's own
   `mqk-execution::asset_risk_policy` module (commit `04a8fb50`,
   "execution: add asset risk router foundation"). `crypto_policy()` is the
   **only** non-equity policy with both `requires_margin_model: false` and
   `requires_contract_multiplier: false`. `future_policy()`/`option_policy()`
   require both (plus expiry/roll/chain/Greeks); `forex_policy()` requires a
   margin model plus pip/lot/leverage. Crypto's two flagged requirements
   (`requires_session_profile`, `requires_currency_conversion`) are also the
   lightest version among the four: a 24/7 "session" needs no
   holiday/early-close calendar logic (unlike futures' Globex RTH/ETH, or
   equities' NYSE calendar already built in `mqk-integrity::calendar`/
   `mqk-daemon::state::market_calendar`), and `requires_currency_conversion`
   is a non-issue in the common case — a USD-quoted pair like `BTC/USD`
   against a USD `account_currency` passes `ASSET-CORE-04A`'s
   `value_position_economics` currency check trivially, since that check
   only refuses when `quote_currency != account_currency`. This matches
   `docs/audits/multi_asset_completion_audit.md`'s independent, repeatedly
   stated conclusion (§1, §7, §8, §9) that crypto should be the first live
   non-equity vertical, and its `CRYPTO-DATA-01` ("24/7 market ingestion")
   roadmap entry is sequenced directly after the `ASSET-CORE-04`
   portfolio-ledger work this 04E patch continues.

10. **What must be built before `ASSET-CORE-04` can value non-equity
    positions from real marks?** (a) A real (non-`_TEST`) registry-v2 entry
    for at least one crypto pair, with `enabled=false`/
    `paper_trading_enabled=false`/`live_trading_enabled=false` honesty flags
    intact; (b) a deterministic, local, no-network source of OHLCV bars for
    that pair — the CSV path, since `ingest_csv.rs` already exists and is
    asset-class-agnostic; (c) a decision (made by this patch, §6) for the
    canonical symbol-string convention that will key the resulting
    `md_bars` rows, so the existing symbol-keyed mark-lookup chain (fact 6
    above) can find them with **no code change to the lookup path itself**;
    (d) explicit confirmation that no session/margin/currency-conversion
    path is wired into runtime/risk/order code. All four are out of this
    patch's scope and are deferred to the implementation patch named in §12.

---

## 3. Candidate Lanes Evaluated

| Lane | Repo evidence | Verdict |
|---|---|---|
| **A. Crypto spot** (`BTC/USD`, `ETH/USD`) | Simplest registry-v2 contract shape (`CryptoPair{base,quote}`, two strings); only non-equity `asset_risk_policy` with `requires_margin_model=false` *and* `requires_contract_multiplier=false`; only committed-fixture non-equity class alongside futures; USD-quoted pairs trivially pass the existing single-currency check. | **Chosen** (combined with lane E) |
| **B. Futures** (`ES`/`MES`/`NQ`/`MNQ`) | Registry-v2 shape exists and is fixture-proven (`ES_TEST`/`MES_TEST`), but `asset_risk_policy::future_policy()` requires both a margin model and contract-multiplier enforcement, plus continuous-contract/roll and a real Globex session calendar (`ASSET-CORE-05` is still equity-NYSE-shaped only — confirmed by `mqk-integrity::calendar`/`market_calendar.rs`). Highest existing fixture maturity of the three rejected lanes, but still strictly higher-risk than crypto on every repo-evidenced axis. | Deferred |
| **C. Options** | Registry-v2 shape exists (`Option{underlying,expiry,strike_micros,right,multiplier}`) but has **zero** committed fixture anywhere (unlike crypto/futures). `asset_risk_policy::option_policy()` requires margin model, contract multiplier, plus chain/Greeks/IV/OI/assignment — none of which exist anywhere in the repo (confirmed by the multi-asset audit's exhaustive grep: zero functional matches for `strike\|expiry\|greeks\|implied_vol\|assignment`). Current ingestion is OHLCV-bars-only; an options chain is a structurally different data shape this repo has never ingested. | Deferred |
| **D. Forex** | Registry-v2 shape exists (`ForexPair{base,quote}`) but, per the mission brief's own instruction, is lower priority unless repo evidence proves otherwise — it does not. `asset_risk_policy::forex_policy()` requires a margin model plus pip/lot/leverage sizing and a 24x5 (not 24/7) session model; `requires_currency_conversion=true` is a real, not trivial, requirement for non-USD pairs (e.g. `EUR/USD`), unlike crypto's common USD-quoted case. No forex fixture is committed anywhere. | Deferred |
| **E. Local CSV fixture-first** | `ingest_csv.rs` already exists, is asset-class-agnostic (case-insensitive column mapping, decimal-string prices, no float), and is the **read** side of an existing, proven pattern (`mqk_db::ingest_provider_bars_to_md_bars`-style persistence). No provider/network/API-credit dependency. Matches this repo's own established "no big-bang cutover" discipline (`ASSET-CORE-04A`/`04B`/`04C`/`04D` are all explicitly model-only/zero-production-caller slices). | **Chosen** (combined with lane A) |

Lane A and lane E are not mutually exclusive — the decision is their
**combination**: crypto is the *asset class*, local CSV is the *source
phase*. This mirrors exactly how the mission brief's own suggested patch
names (`CRYPTO-MARK-DATA-LOCAL-FIRST-01` / `CRYPTO-DATA-01A-LOCAL-CSV-MARKS-01`)
already frame it.

---

## 4. Chosen First Lane

**Crypto spot marks (`BTC/USD`, `ETH/USD`), sourced first from a local,
committed CSV fixture, loaded through the existing CSV-ingestion path into
the existing `md_bars` schema.** No network provider call is part of this
lane's first phase. A real provider (TwelveData crypto, CoinLore, or another
candidate) is an explicit, separate, later decision (§15) — not assumed,
not scheduled, and not required to prove the model chain end-to-end with
real (non-fixture) bars.

---

## 5. Explicitly Rejected / Deferred Lanes and Why

- **Futures** — deferred. Requires a real margin model and contract-
  multiplier enforcement (both `false` in equity/crypto's policy, both
  `true` in futures'), a continuous-contract/roll concept (does not exist
  anywhere in the repo — confirmed zero hits for
  `ES\|MES\|NQ\|MNQ\|Globex\|CME` outside config/fixtures/docs), and a real
  non-equity session calendar (`ASSET-CORE-05` is still NYSE-shaped only).
  Strictly more prerequisites than crypto on every axis this repo currently
  evidences.
- **Options** — deferred. No committed fixture exists at all (crypto and
  futures both have one). Requires a chain/Greeks/IV/OI/assignment data
  model the repo has never ingested (current pipeline is OHLCV-bars-only),
  on top of the same margin/multiplier requirements futures has.
- **Forex** — deferred, per the mission brief's own default priority, which
  current repo evidence supports rather than contradicts: a real margin
  model, pip/lot/leverage sizing, a 24x5 (not 24/7) session model, and a
  non-trivial `requires_currency_conversion` for any non-USD pair, with zero
  committed fixture anywhere.
- **Live/network crypto data (TwelveData-crypto, CoinLore, or any other
  provider)** — deferred, not rejected outright. Mechanically plausible (see
  §2 fact 4) but explicitly unverified by this repo's own
  `providers.json` conventions, and this patch is barred from any network
  call. A future patch should test/verify a real provider explicitly before
  relying on it — exactly the discipline `providers.json` already applies to
  every other "candidate_unverified" entry.

---

## 6. Required Storage Shape

No DB migration is required to prove the *next* implementation patch
(local CSV fixture, still model-only/non-trading) — `md_bars`'s existing
`(symbol, timeframe, end_ts)` schema can store crypto bars today (§2 fact 7).
The implementation patch must still make one explicit, currently-undecided
choice this document surfaces rather than defers silently:

- **Canonical symbol-string convention** for crypto rows in `md_bars` and in
  registry-v2 `provider_symbols`/`symbol` fields — e.g. `"BTC/USD"` (matches
  the registry-v2 test fixtures' existing convention,
  `instrument_registry_v2.rs`'s `base_crypto()` helper and the
  `ASSET-CORE-04B` ledger entry's own `BTC/USD` example) vs. `"BTCUSD"`
  (matches the committed example fixture's `BTCUSD_TEST` symbol). This
  patch does not resolve that ambiguity; it flags it as the first concrete
  question the next patch must answer (see §15).
- Before any real production/trading cutover for non-equity marks (not the
  next patch — a later one), a cleaner instrument-aware bars schema (e.g. an
  `instrument_id` or `asset_class` column on `md_bars`, or a parallel
  instrument-keyed bars table) would be warranted rather than indefinitely
  overloading the `symbol` text column with ad hoc pair-string conventions
  across multiple asset classes. That migration is explicitly **not**
  required for the local-CSV-fixture-first proof this decision recommends
  next.

---

## 7. Required Provider/Source Shape

- **Phase 1 (recommended next patch, §12):** a committed local CSV fixture
  (e.g. `config/fixtures/` or a `mqk-md` test-fixture path — exact location
  is the next patch's decision, not this one's) read through the existing
  `ingest_csv.rs` → `RawBar` path, with no network call and no new provider
  trait implementation. This proves the storage/lookup chain with
  deterministic, repeatable, real (non-network) bars.
- **Phase 2 (later, separate patch — not scheduled by this decision):** a
  real provider implementation for crypto specifically. `providers.json`'s
  own `coinlore` (crypto-only, free, no key required) and `twelvedata`
  (already implemented as a generic HTTP/JSON time-series client, see §2
  fact 4) are the two existing config-level candidates; either would need
  explicit testing/verification before `providers.json`'s
  `implementation_status`/`verification_status` fields for crypto could
  honestly change from their current values. This patch does not choose
  between them — that choice is explicitly deferred (§15).

---

## 8. Required Registry-v2 Fields

A real (non-`_TEST`) crypto registry-v2 entry needs, at minimum, the fields
`instrument_registry_v2.rs` already validates today — no schema change:

- `instrument_id` (e.g. `"crypto:GLOBAL:BTCUSD"`), `symbol` (the canonical
  string chosen per §6), `asset_class: "crypto"`.
- `currency`/`quote_currency` both `"USD"` for a USD-quoted pair (so
  `ASSET-CORE-04A`'s currency check passes trivially, per §2 fact 9).
- `contract: { "kind": "crypto_pair", "base": "BTC", "quote": "USD" }`.
- `provider_symbols` mapping to whichever source phase is active (e.g.
  `{"local_csv": "BTC/USD"}` for Phase 1).
- `enabled: false`, `paper_trading_enabled: false`, `live_trading_enabled: false`
  — unconditionally, per this patch's safety boundaries (§13) and the
  registry-v2 validator's own fail-closed rule (`enabled=true` + non-equity
  requires the test-only `allow_enabled_non_equity_for_testing` escape
  hatch, which must never be set in a real fixture).
- Optionally, `economics: { "contract_multiplier": 1 }` — mirroring the
  committed `BTCUSD_TEST` example fixture's spot-multiplier convention,
  enabling `backtest_economics_suggestion_for_instrument` to report
  `"active"`/`"registry_v2_explicit"` for the new real entry the same way it
  already does for the test fixture.

This is `CRYPTO-REGISTRY-01` from the existing multi-asset audit roadmap
(§5, Phase 3) in substance, even though this decision recommends bundling it
into the same next patch as the local-CSV marks proof rather than sequencing
it as a fully separate patch (see §12).

---

## 9. Required Calendar/Session Interaction

None, for this decision or the recommended next patch. `asset_risk_policy::crypto_policy()`
already documents `requires_session_profile: true` for any *routing*
decision, and `ASSET-CORE-05`'s existing session work
(`ASSET-CORE-05-MARKET-CALENDAR-GENERALIZE-01-COMBINED`) already has a
crypto 24/7 *scaffold* concept — but both are model-only/unwired today, and
neither needs to be touched to load and value fixture marks. A real
production/trading cutover for crypto would need that 24/7 session model to
graduate from scaffold to authoritative, but that is explicitly out of scope
for both this decision and the recommended next patch.

---

## 10. Required Test Fixtures

For the recommended next patch (not built by this patch):

- A small, committed local CSV file with deterministic OHLCV rows for
  `BTC/USD` (and optionally `ETH/USD`) at one timeframe (e.g. `1D`),
  following `ingest_csv.rs`'s existing column contract
  (`symbol,timeframe,end_ts,open,high,low,close,volume,is_complete`).
  Decimal-string prices, no floats, `is_complete=true` for closed bars —
  identical contract equity CSV fixtures already use.
  A real (non-`_TEST`-suffixed) registry-v2 entry for the same pair(s),
  per §8, kept disabled.
- A scenario test proving the CSV fixture parses via `ingest_csv.rs` and
  produces `RawBar` rows with the chosen canonical symbol string,
  *independent* of whether those rows are ever persisted to `md_bars` in
  that same patch (persistence proof can reuse the existing
  `scenario_md_ingest_csv.rs` pattern already in `mqk-db/tests/`).

---

## 11. Required No-Network Proof Path

The recommended next patch's entire proof must run with zero network
access: parse the committed local CSV fixture → produce `RawBar`/`CanonicalBar`
rows → (optionally) persist to a local/test `md_bars` table the same way
`mqk-db/tests/scenario_md_ingest_csv.rs` already proves for equities → read
back through the existing symbol-keyed lookup
(`fetch_recent_completed_bars_for_strategy`) using the chosen crypto symbol
string → feed the resulting mark into the *unmodified* `ASSET-CORE-04A`/
`04B`/`04C` model chain and observe a real (non-fixture-economics,
real-mark) `Active` valuation for a crypto position. This is exactly the
pattern `ASSET-CORE-04D`'s own DB-backed tests already use for equities
(seed a `md_bars` row, query it back, value it) — extended to one new
symbol string, with zero new infrastructure.

---

## 12. Required Future Implementation Patch ID

**`CRYPTO-DATA-01A-LOCAL-CSV-MARKS-01`** — a sub-slice of the existing
multi-asset-audit roadmap's `CRYPTO-DATA-01` ("24/7 market ingestion"),
named in the same `-01A`/`-01B`/... convention this repo already uses for
`ASSET-CORE-01A`/`01B`/`01C`/`01D`. Scope: add one real (disabled)
registry-v2 entry for `BTC/USD` (and optionally `ETH/USD`) — closing
`CRYPTO-REGISTRY-01` from the audit roadmap as a side effect — plus a
committed local CSV fixture and a no-network proof (§10-§11) that real
(non-fixture-economics) marks reach `ASSET-CORE-04A`/`04B`/`04C`/`04D` for at
least one crypto position. Zero network calls, zero DB migration, zero
trading enablement.

This does not change the audit roadmap's own sequencing recommendation
(crypto lane after `ASSET-CORE-04` foundation work, §7 "Months 6-8"); it
narrows the very first step of that lane to the smallest safe slice the
current repo can prove without a network dependency.

---

## 13. Safety Boundaries

Unconditionally true of this decision and must remain true of the patch
named in §12:

- No live or paper order submitted, ever, for any crypto/futures/options/forex
  instrument.
- No provider/broker network call. No API credits spent.
- No DB migration. `md_bars`'s existing schema is sufficient for the
  recommended next patch (§6).
- No registry-v2 entry with `enabled=true` for any non-equity instrument
  outside the validator's existing `#[cfg(test)]`-only escape hatch.
- No change to `PortfolioState`, `compute_portfolio_weights`,
  `/api/v1/portfolio/live-weights`, or `/api/v1/portfolio/economics/status`
  behavior.
- No change to risk, OMS, broker, runtime, or strategy code.
- No session/calendar enforcement change — crypto 24/7 remains a
  documented model assumption, not a wired runtime behavior.

---

## 14. What This Patch Did Not Change

This patch (`ASSET-CORE-04E`) added only: this decision document, its
machine-readable JSON artifact, an optional validator script (§ledger entry),
and ledger/audit updates. It did not touch, and made no behavior change to:
any Rust source file in `core-rs/crates/mqk-daemon/src`,
`mqk-runtime`, `mqk-execution`, `mqk-broker-alpaca`, `mqk-broker-paper`,
`mqk-risk`, `mqk-md/src` (provider implementations), or `mqk-portfolio/src`;
any DB migration; any file under `core-rs/mqk-gui`; `config/instruments/*`;
`config/providers/*`; `.env.local`; or any strategy/OMS/outbox code. No
daemon runtime was started. No provider, broker, or network call was made.

---

## 15. Open Questions Before Live/Paper Non-Equity Enablement

These are explicitly **not** answered by this patch — they are the honest
list of what the recommended next patch (§12), and the patches after it,
still need to resolve before any real trading enablement could even be
considered for crypto, let alone other asset classes:

1. **Canonical symbol-string convention** for crypto in `md_bars` and
   registry-v2 (`"BTC/USD"` vs `"BTCUSD"` vs another convention) — flagged
   in §6, not resolved here.
2. **Which real provider** (if any) eventually supplies live crypto marks —
   TwelveData (already implemented as a generic client, unverified for
   crypto), CoinLore (crypto-only, unimplemented), or another candidate —
   and what explicit verification step proves it before
   `providers.json`'s `implementation_status`/`verification_status` fields
   for crypto could honestly change.
3. **Whether `md_bars`'s symbol-only schema should ever be replaced** with an
   instrument-aware schema (instrument_id/asset_class-carrying), and at what
   point (this decision explicitly defers that to a real production cutover,
   not the next patch).
4. **Whether crypto's 24/7 session "requirement"** (`asset_risk_policy::crypto_policy`)
   should graduate from `ASSET-CORE-05`'s existing scaffold to an
   authoritative provider before any paper-trading consideration — this
   decision takes no position beyond "not needed for marks-only proof."
5. **Whether `CryptoPair`'s `{base, quote}` shape needs a fractional
   quantity-precision/minimum-trade-size policy** before any order sizing
   could ever be considered — `InstrumentEconomics.quantity_scale` already
   models this as descriptive metadata (`ASSET-CORE-04A`), but nothing
   enforces it anywhere, and this decision does not propose that it should.
6. **Account-currency generalization**: `ASSET-CORE-04D`'s route hardcodes
   `account_currency = "USD"`; this happens to make crypto's USD-quoted
   pairs trivially currency-consistent today, but no patch has yet asked
   whether the account-currency assumption itself should ever become
   configurable.

None of these questions block or are answered by this decision. They are
recorded so the next patch (§12) inherits an honest, explicit list rather
than silently assuming any of them away.
