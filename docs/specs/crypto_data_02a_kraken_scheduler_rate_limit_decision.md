# CRYPTO-DATA-02A — Kraken Scheduler Rate-Limit Decision

Patch ID: `CRYPTO-DATA-02A-KRAKEN-SCHEDULER-RATE-LIMIT-DECISION-01`

This is a decision/spec patch. It is **not** a Windows Scheduled Task
registration, **not** a daemon recurring job, **not** a live Kraken API call
beyond the two bounded, keyless, read-only documentation-page fetches
recorded below, **not** a broker/execution/risk/OMS/runtime change, **not**
a DB migration, **not** a config-flag change, **not** crypto trading
enablement. It records a repo-local, conservative cadence/rate-limit policy
for a **future** Kraken OHLC scheduled sync, continuing after the fixture/
DB/sync/status/GUI/registry-readiness lane closed by:

```text
2f5d1840 md: add Kraken content-diff sync
8962a649 daemon: expose Kraken OHLC evidence status
82d14e31 gui: show Kraken OHLC sync status
4f157253 docs: decide Kraken crypto registry cutover
9ced0989 md: add crypto registry readiness check
49be663b gui: show crypto registry readiness
```

Decided at HEAD `49be663b`.

---

## 1. Executive Decision

**Do not register a scheduler in this patch.** Treat a future scheduled
Kraken sync as data-only and operator-controlled, using a conservative
**daily** cadence rather than intraday polling. `config/providers/
providers.json`'s `kraken` entry stays `enabled: false`; `BTC/USD`/`ETH/USD`
registry-v2 rows stay `enabled: false`, `paper_trading_enabled: false`,
`live_trading_enabled: false` — none of this decision's content touches
those files.

---

## 2. Official Source Verification (2 bounded documentation reads)

Per this mission's network rule, at most 2 documentation-only page fetches
were made — zero calls to any Kraken **API** endpoint
(`/0/public/OHLC`, `/0/public/Ticker`, `/0/public/Trades`,
`/0/public/AssetPairs`).

### Read 1 — `https://docs.kraken.com/api/docs/guides/spot-rest-ratelimits` (accessed 2026-07-06)

This page documents **authenticated** REST rate limits as a tiered
call-counter system: Starter (max counter 15, -0.33/sec decay), Intermediate
(max counter 20, -0.5/sec decay), Pro (max counter 20, -1/sec decay). It does
not state a numeric limit for **public, non-authenticated** market-data
endpoints, and does not clarify whether public calls share this counter.
Result: **insufficient on its own** to verify the public OHLC rate limit.

### Read 2 — `https://support.kraken.com/hc/en-us/articles/206548367-What-is-the-API-call-rate-limit` (accessed 2026-07-06)

This official Kraken support article states:

> "Calling the public endpoints at a frequency of 1 per second (or less)
> would remain within the rate limits."

and:

> "Public endpoints are rate limited by IP address and currency pair for
> calls to Trades and OHLC, and by IP address only for calls to all other
> public endpoints."

and that exceeding the limit causes additional calls to be "restricted for a
few seconds (or possibly longer if calls continue to be made while the rate
limits are active)."

**Result: `rate_limit_verification_status = "verified"`.** Kraken does not
publish a precise numeric requests-per-minute ceiling for public endpoints
beyond this 1-call/second guideline, but the guideline itself, the
IP+currency-pair scoping for OHLC specifically, and the throttling
consequence are all explicitly documented by Kraken's own support content —
sufficient to build a conservative policy without guessing.

---

## 3. Answers to the 15 Required Decision Questions

1. **What Kraken rate limit was verified?** A public-endpoint guideline of
   "1 call per second or less remains within the rate limits" (§2, Read 2).
   No stricter numeric per-minute/per-day ceiling is published for public
   endpoints.
2. **Which official source was used?**
   `support.kraken.com/hc/en-us/articles/206548367` (primary, gives the
   numeric guideline); `docs.kraken.com/api/docs/guides/spot-rest-ratelimits`
   (secondary, confirms the authenticated-tier system does not describe
   public endpoints, ruling out misapplying those numbers here).
3. **Does the rate limit apply to public REST market-data endpoints?** Yes —
   the verified guideline is explicitly about "public endpoints," which
   includes `/0/public/OHLC`.
4. **Is OHLC rate-limited by IP and pair?** Yes, explicitly: Trades and OHLC
   are "rate limited by IP address **and currency pair**"; all other public
   endpoints are limited by IP address only (`ohlc_rate_limit_scope:
   "ip_and_pair"`).
5. **What conservative cadence is allowed for future scheduled sync?**
   Daily (`recommended_default_cadence: "daily"`). Nothing in the verified
   guidance requires daily-only cadence — it is this decision's own
   conservative choice, consistent with the guidance rather than derived
   from a stated Kraken policy, matching this repo's existing
   `CRYPTO-REGISTRY-02` §7 invariant #4 (scheduler must default
   disabled/opt-in) and the 1D-only timeframe this Kraken adapter already
   supports.
6. **Should BTC/USD and ETH/USD be synced together or staggered?**
   Staggered/sequential, never concurrent (`concurrency:
   "sequential_only"`) — since OHLC is limited by IP **and pair**, two
   pairs are two independent rate-limit buckets, but sequential calls with a
   deliberate gap keep total request rate well under the 1/sec guideline
   with margin.
7. **Are retries allowed?** Yes, bounded: `max_retries: 2` with exponential
   backoff and jitter, failing closed (no partial/successful state assumed)
   once retries are exhausted.
8. **How should backoff work after rate-limit or provider errors?**
   Exponential with jitter (`retry_policy.backoff: "exponential"`,
   `jitter: true`), consistent with the support article's own description
   of the consequence ("restricted for a few seconds, or longer if calls
   continue") — a future scheduler must back off rather than retry
   immediately.
9. **What max network calls per scheduled run?**
   `max_total_network_calls_per_run: 2` — this decision does not add any
   config/status network check beyond the two pair OHLC calls, so total
   equals `max_ohlc_calls_per_run`.
10. **Minimum interval between per-pair calls?** `min_seconds_between_pair_calls:
    2` — double the verified 1-call/second guideline, giving margin rather
    than calling at exactly the documented ceiling.
11. **Minimum interval between scheduled runs?**
    `min_seconds_between_scheduled_runs: 86400` (24 hours), matching the
    daily cadence decision (§5 above).
12. **May a future scheduler run if registry readiness is unsafe?** No.
    Required invariant: registry readiness (`CRYPTO-REGISTRY-03`'s CLI /
    `CRYPTO-REGISTRY-04`'s route) must classify `active`/
    `data_ready_manual_only` or better immediately before any run.
13. **May a future scheduler run if latest Kraken evidence is unsafe/stale?**
    No. Required invariant: the `CRYPTO-DATA-01AD` evidence status
    (`kraken-ohlc/status`) must not report `unsafe_evidence`, and staleness
    handling is left to that route's own policy — a future scheduler must
    consult it, not assume freshness.
14. **May a future scheduler run if crypto trading remains disabled?** Yes —
    this is the **required** state, not a blocker. A scheduler is
    exclusively a data-ingestion action; it must run (if ever registered)
    precisely while `kraken.enabled`/`paper_trading_enabled`/
    `live_trading_enabled` all stay `false`, and must refuse to run (fail
    closed) if it ever observes any of those flipped `true`, since that
    would indicate an unreviewed change to the safety boundary this whole
    lane depends on.
15. **What invariants are required before task registration?** Recorded in
    `required_invariants_before_scheduler` in the JSON artifact — repeated
    here: Kraken provider stays disabled absent a separate decision; both
    crypto rows' trading flags stay `false`; registry readiness and
    scheduler readiness (§ below, `CRYPTO-DATA-02B`) both report a ready
    state immediately before any run; recurring-call design reuses the
    existing `MarketDataProviderRateLimits` capability surface (unchanged
    guardrail carried forward from `CRYPTO-DATA-01H`/`01C`/
    `CRYPTO-REGISTRY-02`); task registration is its own, separately
    authorized patch with its own decision document (not this one); the
    task defaults to unregistered/opt-in; the task must not share a lock,
    cursor, or state slot with any existing equity-ingestion scheduler or
    the local-crypto-CSV-import task (`CRYPTO-DATA-01E`'s
    `Register-LocalCryptoIngestTask.ps1`).

---

## 4. Why a Daily, Sequential, Backoff-First Policy

- **Margin over the documented guideline.** The verified 1-call/second
  guideline is a ceiling, not a target; `min_seconds_between_pair_calls: 2`
  and a strictly sequential (never concurrent) call pattern keep any future
  run an order of magnitude under that ceiling even accounting for the
  IP+pair-scoped limiter Kraken describes for OHLC/Trades specifically.
- **Daily cadence matches the data, not just the rate limit.** The existing
  Kraken adapter (`CRYPTO-DATA-01U-V-W`) only supports `1D` bars; polling
  more often than once a day cannot produce new completed daily candles and
  would only add unnecessary request volume against a rate limit whose exact
  numeric ceiling for public endpoints Kraken does not publish.
- **Fail-closed retry/backoff, not optimistic retry.** Per
  `CLAUDE.md`'s fail-closed invariant, bounded retries (2) with exponential
  backoff and jitter, and a hard failure (not a silent partial success) once
  exhausted, mirrors the support article's own description of what
  triggering the rate limit does (temporary restriction that can extend if
  calls continue) — retrying blindly would risk exactly that.

---

## 5. What This Patch Does Not Change

This patch (`CRYPTO-DATA-02A`) adds only: this decision document, its
machine-readable JSON artifact, a validator script, and
ledger/runbook/audit updates. It does not touch, and makes no behavior
change to: `config/providers/providers.json`,
`config/instruments/instruments_v2.crypto_local_marks.example.json`, any
Rust source file in `core-rs/crates/mqk-md/src`, `mqk-cli`, `mqk-daemon`,
`mqk-runtime`, `mqk-execution`, `mqk-broker-alpaca`, `mqk-broker-paper`,
`mqk-risk`, `mqk-portfolio`, any file under `core-rs/mqk-gui`, any DB
migration, `.env.local`, `scripts/windows/*`, or any strategy/OMS/outbox/
scheduler code. No daemon runtime was started. No Kraken **API** endpoint
was called. No credential was read. No API credits were spent (the two
reads were of Kraken's public documentation/support **web pages**, not the
market-data API). No DB was mutated. No Windows Scheduled Task was
registered.

---

## 6. Safety Boundaries

Unconditionally true of this decision and must remain true of
`CRYPTO-DATA-02B`/`02C`:

- `kraken.enabled` stays `false` in `config/providers/providers.json`.
- `BTC/USD.enabled` and `ETH/USD.enabled` stay `false` in
  `instruments_v2.crypto_local_marks.example.json`.
- `paper_trading_enabled` and `live_trading_enabled` stay `false` for both
  rows.
- No Windows Scheduled Task registration. No daemon background job.
- No call to any Kraken **API** endpoint. Only 2 bounded, keyless
  documentation/support **page** fetches, both recorded in §2.
- No DB migration. No DB write.
- No change to risk, OMS, broker, runtime, or strategy code.
- No claim of crypto trading readiness or scheduler-registration readiness
  beyond "prerequisites satisfied for a future, separately authorized
  registration patch to be considered" — registration itself remains out of
  scope here and in `02B`/`02C`.

---

## 7. Recommended Next Patches

1. **`CRYPTO-DATA-02B-KRAKEN-SCHEDULER-READINESS-CLI-01`** — a read-only
   operator CLI proving whether a future Kraken scheduled sync is currently
   allowed by this policy, the provider/registry config, and (optionally)
   latest Kraken evidence — without registering anything.
2. **`CRYPTO-DATA-02C-KRAKEN-SCHEDULER-READINESS-STATUS-SURFACE-01`**
   (conditional on `02B` closing cleanly and staying small) — expose the
   same read-only truth through a daemon route and GUI panel.

---

## 8. Remaining Gaps (Unchanged by This Decision)

- No recurring/scheduled Kraken sync of any kind.
- No Windows Scheduled Task registered for Kraken.
- No daemon recurring job for Kraken.
- No production registry-v2 cutover (`enabled` stays `false`).
- No crypto risk policy activation.
- No crypto paper or live execution.
- No crypto strategy.
- `CRYPTO-DATA-01`, `CRYPTO-REGISTRY-01`, `ASSET-CORE-04` remain `PARTIAL`,
  not `CLOSED`.
