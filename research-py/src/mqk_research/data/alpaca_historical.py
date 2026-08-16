from __future__ import annotations

import json
import os
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Dict, FrozenSet, List, Optional, Sequence, Tuple

import numpy as np
import pandas as pd

from mqk_research.data.bars_provenance import (
    CA_POLICY_ADJUSTED_DATA,
    PRICE_CONVENTION_ALPACA_ALL_ADJUSTED,
    UNIVERSE_MODE_FIXED_EX_ANTE,
    build_bars_provenance_manifest,
    build_corporate_action_evidence,
    build_source_attestation,
    canonical_semantic_bars_hash,
    corporate_action_evidence_id,
)
from mqk_research.ml.util_hash import sha256_file, sha256_json

# ---------------------------------------------------------------------------
# BKT-RESEARCH-MARKET-DATA-AUTHORITY-01
#
# The dedicated RESEARCH historical-data extraction authority for official
# US equity/ETF economic research. Distinct from, and does not modify:
#   - core-rs/crates/mqk-md/src/alpaca_provider.rs: the PAPER/RUNTIME
#     historical provider, which explicitly requests adjustment=raw and
#     feeds md_bars via the daemon's ingestion path. That path is
#     unchanged by this module -- production execution never consumes
#     adjusted or synthetic prices.
#   - mqk_research.data.adapters.bars_postgres: reads md_bars (the
#     raw_unadjusted Postgres research mirror of the runtime data).
#
# This module fetches DIRECTLY from Alpaca's Market Data API (not through
# md_bars) so that official research can use adjustment=all -- verified
# against Alpaca's official documentation (docs.alpaca.markets/reference/
# stockbars and docs.alpaca.markets/reference/corporateactions-1, fetched
# 2026-08-15 via Firecrawl) to apply forward/reverse SPLIT price+volume
# adjustment, cash DIVIDEND price adjustment, and SPIN-OFF price adjustment
# in a single request. Every OTHER corporate-action type Alpaca's
# /v1/corporate-actions endpoint reports (mergers, redemptions, name
# changes, rights distributions, worthless removals, partial calls,
# reorganizations, unit splits, stock dividends) is NOT documented as being
# covered by any adjustment mode -- see _COVERED_BY_ADJUSTMENT_ALL below.
# This module deliberately does NOT guess that an undocumented type is
# safe: any REQUIRES_FAIL_CLOSED_REVIEW event intersecting the requested
# symbol/range fails the whole extraction closed (see
# CorporateActionReviewRequired) rather than silently dropping bars or
# symbols.
#
# A successful, clean extraction produces a bars provenance manifest with
# corporate_action_policy=adjusted_data and
# price_adjustment_convention=alpaca_all_adjusted_v1, bound to a real,
# content-addressed source_attestation (see bars_provenance.
# build_source_attestation) that mqk_research.data.bars_provenance.
# check_corporate_action_integrity independently verifies before any
# economic P&L is computed -- a hand-built manifest asserting the same
# convention string is NOT sufficient (see
# bars_provenance.SourceAttestationUnverifiable).

ALPACA_DATA_BASE_URL = "https://data.alpaca.markets"
BARS_PATH = "/v2/stocks/bars"
CORPORATE_ACTIONS_PATH = "/v1/corporate-actions"

ADJUSTMENT_ALL = "all"
DEFAULT_FEED = "iex"
DEFAULT_TIMEFRAME = "1Day"

ENV_ALPACA_KEY = "ALPACA_API_KEY_PAPER"
ENV_ALPACA_SECRET = "ALPACA_API_SECRET_PAPER"

EXTRACTOR_ID = "mqk_research.data.alpaca_historical.v1"

# Corporate-action type vocabulary, verified 2026-08-15 against Alpaca's
# official /v1/corporate-actions OpenAPI schema
# (docs.alpaca.markets/reference/corporateactions-1). Do not add a type here
# without re-verifying against the live schema -- an unmapped type must fail
# closed (see _effective_windows_for_entry), never be silently ignored.
KNOWN_CORPORATE_ACTION_TYPES: FrozenSet[str] = frozenset(
    {
        "reverse_split",
        "forward_split",
        "unit_split",
        "cash_dividend",
        "stock_dividend",
        "spin_off",
        "cash_merger",
        "stock_merger",
        "stock_and_cash_merger",
        "redemption",
        "name_change",
        "worthless_removal",
        "rights_distribution",
        "partial_call",
        "reorganization",
    }
)

# Maps the /v1/corporate-actions response's pluralized array keys
# (corporate_actions.<key>) to the singular `types` filter vocabulary above
# -- verified against the same OpenAPI schema.
_RESPONSE_KEY_TO_TYPE: Dict[str, str] = {
    "reverse_splits": "reverse_split",
    "forward_splits": "forward_split",
    "unit_splits": "unit_split",
    "cash_dividends": "cash_dividend",
    "stock_dividends": "stock_dividend",
    "spin_offs": "spin_off",
    "cash_mergers": "cash_merger",
    "stock_mergers": "stock_merger",
    "stock_and_cash_mergers": "stock_and_cash_merger",
    "redemptions": "redemption",
    "name_changes": "name_change",
    "worthless_removals": "worthless_removal",
    "rights_distributions": "rights_distribution",
    "partial_calls": "partial_call",
    "reorganizations": "reorganization",
}

# Which symbol field(s) on each raw corporate-action entry identify the
# ticker(s) affected -- verified against the same OpenAPI schema (multi-leg
# events like mergers/spin-offs/name-changes touch two-plus symbols; every
# affected symbol gets its own flattened entry so universe-intersection
# checks below cannot miss one leg of the event).
_SYMBOL_FIELDS_BY_TYPE: Dict[str, Tuple[str, ...]] = {
    "cash_dividend": ("symbol",),
    "cash_merger": ("acquirer_symbol", "acquiree_symbol"),
    "forward_split": ("symbol",),
    "name_change": ("old_symbol", "new_symbol"),
    "partial_call": ("symbol",),
    "redemption": ("symbol",),
    "reorganization": ("symbol",),
    "reverse_split": ("symbol",),
    "rights_distribution": ("source_symbol", "new_symbol"),
    "spin_off": ("source_symbol", "new_symbol"),
    "stock_and_cash_merger": ("acquirer_symbol", "acquiree_symbol"),
    "stock_dividend": ("symbol",),
    "stock_merger": ("acquirer_symbol", "acquiree_symbol"),
    "unit_split": ("old_symbol", "new_symbol", "alternate_symbol"),
    "worthless_removal": ("symbol",),
}

# Types whose schema includes an ex_date (verified against the OpenAPI
# schema's "required" lists): cash_dividend, forward_split, reverse_split,
# rights_distribution, spin_off, stock_dividend. Every type has a required
# process_date. effective_start uses ex_date when available (conservative:
# ex_date <= process_date), else falls back to process_date; effective_end
# is always process_date (inclusive, closed interval -- matches this repo's
# ForbidEntry/forbidden_periods convention).
_EX_DATE_TYPES: FrozenSet[str] = frozenset(
    {"cash_dividend", "forward_split", "reverse_split", "rights_distribution", "spin_off", "stock_dividend"}
)

CATEGORY_COVERED_BY_ADJUSTMENT = "covered_by_adjustment"
CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW = "requires_fail_closed_review"

# Conservative, docs-verified classification for adjustment="all": Alpaca's
# official documentation states adjustment=all "applies all" of the
# individually-documented raw|split|dividend|spin-off adjustments, i.e.
# forward/reverse SPLIT price+volume adjustment, cash DIVIDEND price
# adjustment, and SPIN-OFF price adjustment. No other corporate-action type
# is documented as covered by any adjustment mode -- this explicitly
# includes stock_dividend and unit_split, which are easy to mistake for
# "the same as cash_dividend/split" but are never named in Alpaca's
# adjustment documentation. Per the mission's "do not guess" instruction,
# anything not in this set is REQUIRES_FAIL_CLOSED_REVIEW.
_COVERED_BY_ADJUSTMENT_ALL: FrozenSet[str] = frozenset(
    {"forward_split", "reverse_split", "cash_dividend", "spin_off"}
)


class AlpacaCredentialsMissing(RuntimeError):
    """Fail-closed: raised when ALPACA_API_KEY_PAPER / ALPACA_API_SECRET_PAPER
    are not present in the environment (this repo's existing Alpaca
    credential convention -- see
    core-rs/crates/mqk-md/src/alpaca_provider.rs::load_alpaca_paper_credentials).
    Never logs, prints, or serializes the values themselves."""


class AlpacaHistoricalExtractionError(RuntimeError):
    """Fail-closed: raised for any provider HTTP error, incomplete
    pagination, malformed/unmapped response content, duplicate bar,
    non-finite price, or unsupported request shape encountered while
    extracting official research bars/corporate-action data from Alpaca."""


class CorporateActionReviewRequired(RuntimeError):
    """Fail-closed: raised when a REQUIRES_FAIL_CLOSED_REVIEW corporate
    action intersects the requested symbol/range -- see module header and
    find_requires_review_events. Narrow the symbol universe or date range
    to exclude the affected symbol/period, or wait for an explicit handling
    policy for that action type; bars are never silently dropped."""


@dataclass(frozen=True)
class AlpacaCredentials:
    api_key: str
    api_secret: str


def load_alpaca_credentials(env: Optional[Dict[str, str]] = None) -> AlpacaCredentials:
    """Load Alpaca research-data credentials from the environment using this
    repo's existing convention (ALPACA_API_KEY_PAPER / ALPACA_API_SECRET_PAPER
    -- the same paper credentials core-rs/crates/mqk-md/src/alpaca_provider.rs
    uses for historical data; market-data access is tied to the account, not
    to paper-vs-live order routing). Fails closed if either is absent. Never
    logs or returns the values in any exception message."""
    e = env if env is not None else os.environ
    key = e.get(ENV_ALPACA_KEY)
    secret = e.get(ENV_ALPACA_SECRET)
    if not key or not secret:
        raise AlpacaCredentialsMissing(
            f"Missing Alpaca research credentials: set {ENV_ALPACA_KEY} and {ENV_ALPACA_SECRET} "
            "in the environment (repo convention; values are never printed/logged)."
        )
    return AlpacaCredentials(api_key=key, api_secret=secret)


# (url, query_params, headers) -> (status_code, response_body_bytes). Tests
# inject a fake implementation -- no real network call is ever required to
# exercise this module.
HttpGet = Callable[[str, Dict[str, str], Dict[str, str]], Tuple[int, bytes]]


def _default_http_get(url: str, params: Dict[str, str], headers: Dict[str, str]) -> Tuple[int, bytes]:
    qs = urllib.parse.urlencode(params)
    full_url = f"{url}?{qs}" if qs else url
    req = urllib.request.Request(full_url, headers=headers, method="GET")
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:  # noqa: S310 (fixed https host)
            return int(resp.status), resp.read()
    except urllib.error.HTTPError as exc:
        return int(exc.code), exc.read()


def _fetch_all_pages(
    *,
    endpoint_url: str,
    base_params: Dict[str, str],
    credentials: AlpacaCredentials,
    http_get: HttpGet,
    max_pages: int = 500,
) -> Tuple[List[Dict[str, Any]], bool, List[str]]:
    """Paginate an Alpaca Market Data API GET endpoint to completion.
    Returns (raw_json_pages, pagination_complete, per_page_content_hashes).
    Fails closed (AlpacaHistoricalExtractionError) on any non-200 response,
    undecodable JSON, or pagination that does not terminate within
    max_pages -- never returns a partial result silently."""
    headers = {
        "APCA-API-KEY-ID": credentials.api_key,
        "APCA-API-SECRET-KEY": credentials.api_secret,
        "accept": "application/json",
    }
    pages: List[Dict[str, Any]] = []
    hashes: List[str] = []
    params = dict(base_params)
    page_token: Optional[str] = None
    for _ in range(max_pages):
        if page_token:
            params["page_token"] = page_token
        else:
            params.pop("page_token", None)
        status, body = http_get(endpoint_url, params, headers)
        if status != 200:
            raise AlpacaHistoricalExtractionError(
                f"Alpaca API error: endpoint={endpoint_url} status={status} body={body[:500]!r}"
            )
        try:
            payload = json.loads(body.decode("utf-8"))
        except Exception as exc:  # noqa: BLE001 - re-raised as a typed, fail-closed error
            raise AlpacaHistoricalExtractionError(
                f"Alpaca API response JSON decode failed: endpoint={endpoint_url}: {exc}"
            ) from exc
        pages.append(payload)
        hashes.append(sha256_json(payload))
        page_token = payload.get("next_page_token")
        if not page_token:
            return pages, True, hashes
    raise AlpacaHistoricalExtractionError(
        f"Alpaca API pagination did not terminate within max_pages={max_pages}: endpoint={endpoint_url} "
        "-- refusing to return a possibly-incomplete result"
    )


def fetch_historical_bars(
    *,
    symbols: Sequence[str],
    start_utc: pd.Timestamp,
    end_utc: pd.Timestamp,
    timeframe: str = DEFAULT_TIMEFRAME,
    adjustment: str = ADJUSTMENT_ALL,
    feed: str = DEFAULT_FEED,
    credentials: Optional[AlpacaCredentials] = None,
    http_get: Optional[HttpGet] = None,
    base_url: str = ALPACA_DATA_BASE_URL,
) -> Tuple[pd.DataFrame, Dict[str, Any]]:
    """Fetch complete, deterministically-normalized historical bars from
    Alpaca's official /v2/stocks/bars endpoint. Returns (bars_df, fetch_meta).
    bars_df has columns [symbol, end_ts (ISO-8601 UTC string), open, high,
    low, close, volume], sorted (symbol, end_ts) ascending. Fails closed on:
    provider HTTP error, incomplete pagination, a missing 'bars' key,
    duplicate (symbol,end_ts) rows, non-finite OHLC prices, or any requested
    symbol returning zero bars."""
    symbols_norm = sorted({s.strip().upper() for s in symbols if s.strip()})
    if not symbols_norm:
        raise ValueError("symbols must be non-empty")
    if start_utc.tzinfo is None or end_utc.tzinfo is None:
        raise ValueError("start_utc/end_utc must be timezone-aware (UTC)")
    start_utc = start_utc.tz_convert("UTC")
    end_utc = end_utc.tz_convert("UTC")
    if end_utc <= start_utc:
        raise ValueError("end_utc must be > start_utc")

    creds = credentials or load_alpaca_credentials()
    getter = http_get or _default_http_get

    base_params = {
        "symbols": ",".join(symbols_norm),
        "timeframe": timeframe,
        "start": start_utc.isoformat(),
        "end": end_utc.isoformat(),
        "adjustment": adjustment,
        "feed": feed,
        "limit": "10000",
        "sort": "asc",
    }
    endpoint_url = f"{base_url}{BARS_PATH}"
    pages, complete, page_hashes = _fetch_all_pages(
        endpoint_url=endpoint_url, base_params=base_params, credentials=creds, http_get=getter
    )

    rows: List[Dict[str, Any]] = []
    for page in pages:
        bars_by_symbol = page.get("bars")
        if bars_by_symbol is None:
            raise AlpacaHistoricalExtractionError(
                f"Alpaca bars response missing required 'bars' key: endpoint={endpoint_url}"
            )
        for sym, bar_list in bars_by_symbol.items():
            for b in bar_list or []:
                ts = pd.Timestamp(b["t"])
                ts = ts.tz_localize("UTC") if ts.tzinfo is None else ts.tz_convert("UTC")
                rows.append(
                    {
                        "symbol": str(sym).strip().upper(),
                        "end_ts": ts,
                        "open": float(b["o"]),
                        "high": float(b["h"]),
                        "low": float(b["l"]),
                        "close": float(b["c"]),
                        "volume": float(b.get("v", 0.0)),
                    }
                )

    if not rows:
        raise AlpacaHistoricalExtractionError(
            f"Alpaca returned zero bars for symbols={symbols_norm} window=[{start_utc.isoformat()}, "
            f"{end_utc.isoformat()})"
        )

    df = pd.DataFrame(rows)

    for col in ("open", "high", "low", "close"):
        values = df[col].to_numpy(dtype=float)
        if not np.isfinite(values).all():
            raise AlpacaHistoricalExtractionError(f"Alpaca bars contain non-finite {col!r} values")

    dup_mask = df.duplicated(subset=["symbol", "end_ts"], keep=False)
    if bool(dup_mask.any()):
        dup_rows = df.loc[dup_mask, ["symbol", "end_ts"]].drop_duplicates().to_dict("records")
        raise AlpacaHistoricalExtractionError(f"Alpaca bars contain duplicate (symbol,end_ts) rows: {dup_rows}")

    df = df.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)
    df["end_ts"] = df["end_ts"].map(lambda t: t.isoformat())

    present = set(df["symbol"].unique().tolist())
    missing = sorted(set(symbols_norm) - present)
    if missing:
        raise AlpacaHistoricalExtractionError(f"Alpaca returned no bars at all for symbol(s): {missing}")

    meta: Dict[str, Any] = {
        "pagination_complete": complete,
        "raw_response_content_hashes": page_hashes,
        "resolved_feed": feed,
        "resolved_adjustment": adjustment,
        "resolved_timeframe": timeframe,
        "requested_start_utc": start_utc.isoformat(),
        "requested_end_utc": end_utc.isoformat(),
        "returned_coverage_start_utc": df["end_ts"].min(),
        "returned_coverage_end_utc": df["end_ts"].max(),
        "symbols_requested": symbols_norm,
        "symbols_returned": sorted(present),
        "endpoint": endpoint_url,
    }
    return df, meta


def _effective_windows_for_entry(action_type: str, raw: Dict[str, Any]) -> List[Tuple[str, str, str]]:
    """Flatten one raw /v1/corporate-actions entry into
    (symbol, effective_start_ts, effective_end_ts) tuples, one per affected
    symbol leg. Fails closed on an unmapped action_type or an entry with no
    populated symbol field -- never silently skips an entry."""
    symbol_fields = _SYMBOL_FIELDS_BY_TYPE.get(action_type)
    if symbol_fields is None:
        raise AlpacaHistoricalExtractionError(
            f"Unmapped corporate action type from Alpaca response: {action_type!r} -- refusing to "
            "silently ignore an unrecognized event; verify KNOWN_CORPORATE_ACTION_TYPES against "
            "current Alpaca documentation"
        )
    process_date = raw.get("process_date")
    if not process_date:
        raise AlpacaHistoricalExtractionError(
            f"Alpaca corporate action entry missing required process_date: type={action_type} raw={raw}"
        )
    start_date = raw.get("ex_date") if action_type in _EX_DATE_TYPES else None
    start_date = start_date or process_date

    out: List[Tuple[str, str, str]] = []
    for field in symbol_fields:
        sym = raw.get(field)
        if sym:
            out.append((str(sym).strip().upper(), str(start_date), str(process_date)))
    if not out:
        raise AlpacaHistoricalExtractionError(
            f"Alpaca corporate action entry has no populated symbol field(s): type={action_type} "
            f"expected_fields={symbol_fields} raw={raw}"
        )
    return out


def fetch_corporate_actions(
    *,
    symbols: Sequence[str],
    start_utc: pd.Timestamp,
    end_utc: pd.Timestamp,
    types: Optional[Sequence[str]] = None,
    credentials: Optional[AlpacaCredentials] = None,
    http_get: Optional[HttpGet] = None,
    base_url: str = ALPACA_DATA_BASE_URL,
) -> Tuple[List[Dict[str, Any]], Dict[str, Any]]:
    """Fetch complete corporate-action evidence from Alpaca's official
    /v1/corporate-actions endpoint for the same symbol universe/range as a
    bars request. Returns (entries, fetch_meta): entries is a flat,
    normalized list of {"symbol","action_type","effective_start_ts",
    "effective_end_ts"} dicts merged across every corporate_actions.<type>
    response bucket and every page (see _effective_windows_for_entry).
    Fails closed on: provider HTTP error, incomplete pagination, a missing
    'corporate_actions' key, or an unknown requested type."""
    symbols_norm = sorted({s.strip().upper() for s in symbols if s.strip()})
    if not symbols_norm:
        raise ValueError("symbols must be non-empty")
    if start_utc.tzinfo is None or end_utc.tzinfo is None:
        raise ValueError("start_utc/end_utc must be timezone-aware (UTC)")
    start_utc = start_utc.tz_convert("UTC")
    end_utc = end_utc.tz_convert("UTC")

    creds = credentials or load_alpaca_credentials()
    getter = http_get or _default_http_get

    requested_types = sorted({t.strip() for t in (types or KNOWN_CORPORATE_ACTION_TYPES) if t.strip()})
    unknown_types = sorted(set(requested_types) - KNOWN_CORPORATE_ACTION_TYPES)
    if unknown_types:
        raise ValueError(f"Unknown corporate action type(s) requested: {unknown_types}")

    base_params = {
        "symbols": ",".join(symbols_norm),
        "types": ",".join(requested_types),
        "start": start_utc.date().isoformat(),
        "end": end_utc.date().isoformat(),
        "region": "us",
        "limit": "1000",
        "data_quality": "complete",
        "sort": "asc",
    }
    endpoint_url = f"{base_url}{CORPORATE_ACTIONS_PATH}"
    pages, complete, page_hashes = _fetch_all_pages(
        endpoint_url=endpoint_url, base_params=base_params, credentials=creds, http_get=getter
    )

    entries: List[Dict[str, Any]] = []
    for page in pages:
        ca = page.get("corporate_actions")
        if ca is None:
            raise AlpacaHistoricalExtractionError(
                f"Alpaca corporate-actions response missing required 'corporate_actions' key: "
                f"endpoint={endpoint_url}"
            )
        unmapped_keys = sorted(set(ca.keys()) - set(_RESPONSE_KEY_TO_TYPE.keys()))
        if unmapped_keys:
            raise AlpacaHistoricalExtractionError(
                f"Alpaca corporate-actions response contains unmapped bucket(s) not in this module's "
                f"verified vocabulary: {unmapped_keys} -- refusing to silently ignore a possibly-new "
                "corporate-action type; update KNOWN_CORPORATE_ACTION_TYPES/_RESPONSE_KEY_TO_TYPE/"
                "_SYMBOL_FIELDS_BY_TYPE against current Alpaca documentation first"
            )
        for response_key, action_type in _RESPONSE_KEY_TO_TYPE.items():
            for raw in ca.get(response_key) or []:
                for symbol, eff_start, eff_end in _effective_windows_for_entry(action_type, raw):
                    entries.append(
                        {
                            "symbol": symbol,
                            "action_type": action_type,
                            "effective_start_ts": eff_start,
                            "effective_end_ts": eff_end,
                        }
                    )

    meta: Dict[str, Any] = {
        "pagination_complete": complete,
        "raw_response_content_hashes": page_hashes,
        "requested_start_utc": start_utc.isoformat(),
        "requested_end_utc": end_utc.isoformat(),
        "requested_types": requested_types,
        "symbols_requested": symbols_norm,
        "endpoint": endpoint_url,
    }
    return entries, meta


def classify_corporate_action_type(action_type: str, *, adjustment_mode: str = ADJUSTMENT_ALL) -> str:
    """Pure classification, no IO. Returns CATEGORY_COVERED_BY_ADJUSTMENT
    only for the docs-verified subset of types covered by the given
    adjustment mode; everything else (including every type this module does
    not have a verified mapping for) is CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW."""
    if adjustment_mode != ADJUSTMENT_ALL:
        raise ValueError(
            f"classify_corporate_action_type only has a verified mapping for adjustment_mode="
            f"{ADJUSTMENT_ALL!r}, got {adjustment_mode!r}"
        )
    if action_type not in KNOWN_CORPORATE_ACTION_TYPES:
        raise ValueError(f"Unknown corporate action type: {action_type!r}")
    return CATEGORY_COVERED_BY_ADJUSTMENT if action_type in _COVERED_BY_ADJUSTMENT_ALL else CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW


def find_requires_review_events(
    entries: Sequence[Dict[str, Any]],
    *,
    symbol_universe: Sequence[str],
    start_utc: pd.Timestamp,
    end_utc: pd.Timestamp,
    adjustment_mode: str = ADJUSTMENT_ALL,
) -> List[Dict[str, Any]]:
    """Return the subset of `entries` that are CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW,
    affect a symbol in `symbol_universe`, and whose
    [effective_start_ts, effective_end_ts] window intersects
    [start_utc, end_utc) -- deterministic, order-independent (sorted by
    symbol/action_type/effective_start_ts)."""
    universe = {s.strip().upper() for s in symbol_universe}
    start_bound = start_utc.tz_convert("UTC")
    end_bound = end_utc.tz_convert("UTC")
    hits: List[Dict[str, Any]] = []
    for e in entries:
        if e["symbol"] not in universe:
            continue
        if classify_corporate_action_type(e["action_type"], adjustment_mode=adjustment_mode) != CATEGORY_REQUIRES_FAIL_CLOSED_REVIEW:
            continue
        e_start = pd.Timestamp(e["effective_start_ts"]).tz_localize("UTC") if pd.Timestamp(e["effective_start_ts"]).tzinfo is None else pd.Timestamp(e["effective_start_ts"]).tz_convert("UTC")
        e_end = pd.Timestamp(e["effective_end_ts"]).tz_localize("UTC") if pd.Timestamp(e["effective_end_ts"]).tzinfo is None else pd.Timestamp(e["effective_end_ts"]).tz_convert("UTC")
        if e_end >= start_bound and e_start < end_bound:
            hits.append(e)
    return sorted(hits, key=lambda e: (e["symbol"], e["action_type"], e["effective_start_ts"]))


def extract_research_bars_with_provenance(
    *,
    symbols: Sequence[str],
    start_utc: pd.Timestamp,
    end_utc: pd.Timestamp,
    timeframe: str = DEFAULT_TIMEFRAME,
    feed: str = DEFAULT_FEED,
    credentials: Optional[AlpacaCredentials] = None,
    http_get: Optional[HttpGet] = None,
    base_url: str = ALPACA_DATA_BASE_URL,
    retrieval_timestamp_utc: Optional[str] = None,
) -> Dict[str, Any]:
    """Official, narrow entry point for fixed_ex_ante US equity/ETF research
    extraction: fetches adjustment=all bars AND corporate-actions for the
    SAME symbol/range from Alpaca, fails closed
    (CorporateActionReviewRequired) if any REQUIRES_FAIL_CLOSED_REVIEW event
    intersects the request, and otherwise returns a fully-formed,
    source-attested bars provenance manifest ready to pass directly as
    `bars_provenance` to
    mqk_research.ml.economic_registry_integration.run_registered_economic_walkforward_eval
    -- no manual manifest fabrication required (see mission "REGISTERED
    ECONOMIC INTEGRATION"). Returns {"bars": DataFrame, "manifest": {...},
    "corporate_action_evidence": {...}, "corporate_action_entries": [...]}."""
    symbols_norm = sorted({s.strip().upper() for s in symbols if s.strip()})

    bars_df, bars_meta = fetch_historical_bars(
        symbols=symbols_norm,
        start_utc=start_utc,
        end_utc=end_utc,
        timeframe=timeframe,
        adjustment=ADJUSTMENT_ALL,
        feed=feed,
        credentials=credentials,
        http_get=http_get,
        base_url=base_url,
    )
    ca_entries, ca_meta = fetch_corporate_actions(
        symbols=symbols_norm,
        start_utc=start_utc,
        end_utc=end_utc,
        credentials=credentials,
        http_get=http_get,
        base_url=base_url,
    )

    review_required = find_requires_review_events(
        ca_entries,
        symbol_universe=symbols_norm,
        start_utc=start_utc,
        end_utc=end_utc,
        adjustment_mode=ADJUSTMENT_ALL,
    )
    if review_required:
        raise CorporateActionReviewRequired(
            "Fail-closed: the following corporate action(s) intersect the requested symbol/range and "
            f"are NOT covered by Alpaca's adjustment={ADJUSTMENT_ALL!r} semantics -- refusing to "
            "produce official research bars provenance until an explicit handling policy exists for "
            "them. Narrow the symbol universe or date range to exclude the affected symbol/period, or "
            f"wait for a future patch that adds explicit handling: {review_required}"
        )

    evidence = build_corporate_action_evidence(
        source_provider_id="alpaca",
        covered_symbol_universe=symbols_norm,
        coverage_start_utc=start_utc.tz_convert("UTC").isoformat(),
        coverage_end_utc=end_utc.tz_convert("UTC").isoformat(),
        corporate_action_entries=[
            {
                "symbol": e["symbol"],
                "action_type": e["action_type"],
                "effective_start_ts": e["effective_start_ts"],
                "effective_end_ts": e["effective_end_ts"],
            }
            for e in ca_entries
        ],
    )

    bars_hash = canonical_semantic_bars_hash(bars_df)
    retrieval_ts = retrieval_timestamp_utc or pd.Timestamp.now(tz="UTC").isoformat()

    attestation = build_source_attestation(
        source_provider_id="alpaca",
        extractor_id=EXTRACTOR_ID,
        api_endpoint_bars=bars_meta["endpoint"],
        api_endpoint_corporate_actions=ca_meta["endpoint"],
        symbols=symbols_norm,
        requested_start_utc=bars_meta["requested_start_utc"],
        requested_end_utc=bars_meta["requested_end_utc"],
        returned_coverage_start_utc=bars_meta["returned_coverage_start_utc"],
        returned_coverage_end_utc=bars_meta["returned_coverage_end_utc"],
        adjustment_mode=ADJUSTMENT_ALL,
        feed=feed,
        asof=None,
        pagination_complete_bars=bars_meta["pagination_complete"],
        pagination_complete_corporate_actions=ca_meta["pagination_complete"],
        corporate_action_query_coverage={
            "requested_start_utc": ca_meta["requested_start_utc"],
            "requested_end_utc": ca_meta["requested_end_utc"],
            "requested_types": ca_meta["requested_types"],
            "symbols_requested": ca_meta["symbols_requested"],
        },
        category_b_events_found=[],
        raw_response_content_hashes={
            "bars_pages": bars_meta["raw_response_content_hashes"],
            "corporate_action_pages": ca_meta["raw_response_content_hashes"],
        },
        canonical_semantic_bars_hash=bars_hash,
        canonical_corporate_action_evidence_hash=corporate_action_evidence_id(evidence),
        retrieval_timestamp_utc=retrieval_ts,
    )

    price_provenance = {
        "close_column": "close",
        "provider_metadata_available": True,
        "provider_ids_observed": ["alpaca"],
        "price_adjustment_convention": PRICE_CONVENTION_ALPACA_ALL_ADJUSTED,
        "convention_basis": (
            "mqk_research.data.alpaca_historical requested adjustment=all directly from Alpaca's "
            "official /v2/stocks/bars endpoint (verified against docs.alpaca.markets to apply "
            "split+dividend+spin-off price/volume adjustment); see the manifest's source_attestation "
            "for full retrieval evidence."
        ),
    }

    manifest = build_bars_provenance_manifest(
        price_provenance=price_provenance,
        corporate_action_policy=CA_POLICY_ADJUSTED_DATA,
        corporate_action_evidence_id=corporate_action_evidence_id(evidence),
        corporate_action_evidence=evidence,
        forbidden_periods=(),
        timeframe=timeframe,
        start_utc=start_utc.tz_convert("UTC").isoformat(),
        end_utc=end_utc.tz_convert("UTC").isoformat(),
        symbol_universe=symbols_norm,
        universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE,
        bars=bars_df,
        source_attestation=attestation,
    )

    return {
        "bars": bars_df,
        "manifest": manifest,
        "corporate_action_evidence": evidence,
        "corporate_action_entries": ca_entries,
    }


def write_research_extraction_artifacts(run_dir: Path, result: Dict[str, Any]) -> Dict[str, Path]:
    """Write the deterministic durable artifacts for one extraction result
    (see extract_research_bars_with_provenance) into `run_dir`:
    research_bars.csv, research_bars_provenance.json, corporate_actions.json,
    corporate_actions_provenance.json. The manifest's `artifact_sha256`/
    `row_count` (PHYSICAL file facts) are filled in from the file actually
    written here -- distinct from `canonical_semantic_bars_hash` (the
    CANONICAL SEMANTIC identity, already fixed at extraction time and never
    changed by this function; see bars_provenance.canonical_semantic_bars_hash)."""
    run_dir = Path(run_dir)
    run_dir.mkdir(parents=True, exist_ok=True)

    bars_path = run_dir / "research_bars.csv"
    manifest_path = run_dir / "research_bars_provenance.json"
    ca_entries_path = run_dir / "corporate_actions.json"
    ca_provenance_path = run_dir / "corporate_actions_provenance.json"

    bars_out = result["bars"].sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(drop=True)
    bars_out.to_csv(bars_path, index=False, lineterminator="\n")

    manifest = dict(result["manifest"])
    manifest["artifact_sha256"] = sha256_file(bars_path)
    manifest["row_count"] = int(len(bars_out))
    manifest_path.write_text(
        json.dumps(manifest, sort_keys=True, separators=(",", ":")), encoding="utf-8"
    )

    ca_entries_path.write_text(
        json.dumps(
            {"corporate_action_entries": result["corporate_action_entries"]},
            sort_keys=True,
            separators=(",", ":"),
        ),
        encoding="utf-8",
    )
    ca_provenance_path.write_text(
        json.dumps(result["corporate_action_evidence"], sort_keys=True, separators=(",", ":")),
        encoding="utf-8",
    )

    return {
        "bars_csv": bars_path,
        "bars_provenance_json": manifest_path,
        "corporate_actions_json": ca_entries_path,
        "corporate_actions_provenance_json": ca_provenance_path,
    }
