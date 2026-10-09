# StockNest Research-Data Intake — Capability Assessment (Component D)

Status: **NO SUPPORTED API FOUND — INGESTION DISABLED BY CONSTRUCTION**.

## 1. Service identity (verified, not assumed)

Per the mission's caution not to substitute a similarly-named GitHub repo:
verification was done against the actual product at `https://stocknest.app/`,
not any repository.

| Check | Method | Result |
|---|---|---|
| Direct fetch of `stocknest.app` | `WebFetch` | **HTTP 403 Forbidden** — automated access is blocked at the edge; no page content, no API surface, retrievable this way |
| Product identity | `WebSearch` ("stocknest.app ... fundamentals screener ...") | Confirmed real, distinct product: *"compare up to 5 stocks across 77 metrics — DCF valuations, growth trends, profitability scores, and a 20+ filter screener"* — a consumer fundamental-analysis/comparison web app |
| Public API / developer docs | `WebSearch` ("stocknest.app" API documentation OR developer OR terms of service) | **No results found.** No `docs.stocknest.app`, no `api.stocknest.app`, no developer portal, no licensing terms surfaced by search |
| 13F / insider transactions / congressional disclosures | Both searches | **Not documented for StockNest.** Those capabilities belong to unrelated services that appeared only as search noise (Financial Modeling Prep, Barebone AI, FinTurtle, secform4.com) — not conflated into this assessment |

## 2. Verified vs unsupported capability matrix

| Mission-listed capability | Verified for StockNest? |
|---|---|
| Fundamental statements / company metrics | Plausible (consumer UI mentions 77 metrics), but **no documented machine-readable access** |
| Screeners | Plausible (consumer UI, 20+ filter screener), **no documented API** |
| Institutional 13F filings | **Not found** — not documented for this product |
| Insider transactions | **Not found** |
| Congressional disclosures | **Not found** |
| Earnings / corporate events | **Not found** |
| Historical data exports | **Not found** |
| Public API | **Not found** — direct site access is bot-blocked (403); no developer docs located |
| Authentication | Unknown (no docs found) |
| Licensing / redistribution | Unknown (no terms located) |
| Rate limits | Unknown |
| Historical revisions / point-in-time availability | Unknown |

No capability in this matrix is asserted beyond what was actually found. Absence of evidence is recorded as `unknown`/`unverified`, never as a silent "yes."

## 3. Acceptance path taken

Per the mission's explicit fallback ("Acceptance if no supported API exists: Do not stop the entire controller"), this component implements the **smallest useful, provider-neutral, offline source/schema validation boundary**, extending the existing provenance architecture's pattern (content-addressed IDs, fail-closed preflight checks run before any economic use — the same shape as `research-py/src/mqk_research/data/bars_provenance.py`) rather than that module itself, which is OHLC-bars/corporate-action-specific and would be a semantic mismatch for discrete disclosure events (a 13F snapshot, an insider Form-4, a congressional trade disclosure are not bars).

See `research-py/src/mqk_research/data/altdata_provenance.py`:

- A provider-neutral `AltDataEvent` contract that explicitly separates event
  date/time, public disclosure date/time, provider ingestion timestamp, and
  data revision version — so a congressional trade's execution date is never
  confused with its later public disclosure date, and a reporting period is
  never confused with the date its results became public.
- `require_point_in_time_available(event, as_of_utc)` fails closed whenever
  `as_of_utc` is earlier than the event's public disclosure time — a future
  (not-yet-knowable) event can never be treated as available to a strategy.
- A `PROVIDER_CAPABILITIES` registry recording, per provider, whether an API
  is actually available and whether research use is licensed — both default
  to fail-closed `False`/`"unknown"`. The `stocknest` entry in this registry
  is the durable record of the verification in §1 and §2, not a live check.
- `fetch_from_provider(...)` is a real function, not a placeholder that
  silently returns data: it raises `ProviderUnavailableError` for every
  provider currently in the registry, including `stocknest`, naming exactly
  which capability is missing.

## 4. Future authorization requirements (for whoever revisits this)

To move `stocknest` (or any alt-data provider) from disabled to enabled in
`PROVIDER_CAPABILITIES`, an operator must separately supply and the next
session must record:

1. Confirmed existence of a documented, authorized API or data-export
   mechanism (not scraping the consumer UI).
2. Its authentication requirements and whether MQD is licensed to use it
   for research (redistribution/commercial-use terms).
3. Documented point-in-time / revision-history behavior, so
   `public_disclosure_datetime_utc` can be populated from real provider
   metadata rather than assumed.
4. Rate limits and symbol/identifier mapping (ticker ambiguity, delistings).

None of this is implemented speculatively here — the registry entry stays
`api_available=False` until that evidence exists.
