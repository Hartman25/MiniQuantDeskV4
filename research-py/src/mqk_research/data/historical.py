"""Qualification of versioned historical bars within the existing provenance authority."""

from __future__ import annotations

import datetime as dt
import hashlib
import re
from pathlib import Path

import numpy as np
import pandas as pd

from mqk_research.ml.util_hash import sha256_json

CONTRACT_VERSION = "historical_equity_snapshot_v1"
QUALITY_POLICY = "strict_ohlcv_snapshot_v1"
V3_EXTRACTORS = frozenset(
    {
        "mqk_research.data.alpaca_historical.v3",
        "mqk_research.data.alpaca_historical.diagnostic_v3",
    }
)
COLUMNS = ("symbol", "end_ts", "open", "high", "low", "close", "volume")
CALENDAR_ID = "us_equity_regular_sessions_v1"
CALENDAR_SHA256 = "3249ee517cbc6763b9f73b372d5c6a5f8337d872b91d20a5bca1c27b7a1a76de"
REPO = Path(__file__).resolve().parents[4]


class HistoricalDataUnqualified(RuntimeError):
    pass


def utc_instant(value, name):
    if not isinstance(value, (str, pd.Timestamp, dt.datetime)):
        raise HistoricalDataUnqualified(
            f"{name}: explicit timezone-aware instant required"
        )
    try:
        ts = pd.Timestamp(value)
        if pd.isna(ts) or ts.tzinfo is None:
            raise ValueError("missing timezone")
        return ts.tz_convert("UTC")
    except (ValueError, TypeError, OverflowError) as exc:
        raise HistoricalDataUnqualified(
            f"{name}: invalid or ambiguous timezone"
        ) from exc


def normalized_historical_bars(bars):
    missing = sorted(set(COLUMNS) - set(bars.columns))
    if missing or bars.empty:
        raise HistoricalDataUnqualified(
            f"nonempty full OHLCV required; missing={missing}"
        )
    out = bars[list(COLUMNS)].copy()
    if out["symbol"].isna().any() or any(
        not isinstance(s, str) or not s.strip() for s in out["symbol"]
    ):
        raise HistoricalDataUnqualified("missing/invalid instrument symbol")
    out["end_ts"] = [utc_instant(t, "bar timestamp").isoformat() for t in out["end_ts"]]
    if out.duplicated(["symbol", "end_ts"]).any():
        raise HistoricalDataUnqualified("duplicate (symbol,timestamp)")
    for col in COLUMNS[2:]:
        if any(isinstance(x, (bool, np.bool_)) for x in out[col]):
            raise HistoricalDataUnqualified(f"invalid boolean {col}")
        try:
            values = pd.to_numeric(out[col], errors="raise").to_numpy(dtype=float)
        except (ValueError, TypeError) as exc:
            raise HistoricalDataUnqualified(f"invalid {col}") from exc
        if (
            not np.isfinite(values).all()
            or (values < 0 if col == "volume" else values <= 0).any()
        ):
            raise HistoricalDataUnqualified(f"invalid/non-finite {col}")
        out[col] = values
    if (
        (out["low"] > out["high"])
        | (out["open"] < out["low"])
        | (out["open"] > out["high"])
        | (out["close"] < out["low"])
        | (out["close"] > out["high"])
    ).any():
        raise HistoricalDataUnqualified("inconsistent OHLC range")
    if (
        "is_complete" in bars
        and not bars["is_complete"]
        .map(lambda x: isinstance(x, (bool, np.bool_)) and bool(x))
        .all()
    ):
        raise HistoricalDataUnqualified("partial/unfinalized bars")
    return out.sort_values(["symbol", "end_ts"], kind="mergesort").reset_index(
        drop=True
    )


def full_ohlcv_hash(bars):
    return sha256_json(normalized_historical_bars(bars).to_dict("records"))


def build_historical_contract(bars, *, timeframe, asof):
    """Provider mapping-asof is never a revision vintage or a publication clock."""
    daily = timeframe in {"1D", "1Day"}
    return {
        "schema_version": CONTRACT_VERSION,
        "provider": "alpaca",
        "asset_class": "us_equity_etf",
        "instrument_identity": "requested_symbol_only_history_unproven",
        "universe_membership": "fixed_ex_ante_not_pit_membership",
        "calendar": CALENDAR_ID if daily else "unqualified_intraday",
        "calendar_content_sha256": CALENDAR_SHA256 if daily else None,
        "resolution": "1D" if daily else timeframe,
        "timestamp_meaning": "provider_period_start_label_in_end_ts_column",
        "timezone_policy": "explicit_offset_to_utc_session_date_America_New_York",
        "price_unit": "USD",
        "volume_unit": "provider_adjusted_shares",
        "adjustment_application": "provider_all_once_no_local_adjustment",
        "mapping_asof": asof,
        "revision_vintage": None,
        "source_publication_time": None,
        "point_in_time_status": "UNQUALIFIED_UNKNOWN_AVAILABILITY",
        "normalization_version": "strict_provider_ohlcv_v1",
        "quality_policy": QUALITY_POLICY,
        "full_ohlcv_sha256": full_ohlcv_hash(bars),
    }


def verify_historical_contract(bars, manifest):
    contract = manifest.get("historical_data_contract")
    att = manifest.get("source_attestation") or {}
    if contract is None and att.get("extractor_id") not in V3_EXTRACTORS:
        return  # Legacy manifests keep their original interpretation.
    if not isinstance(contract, dict):
        raise HistoricalDataUnqualified("missing versioned historical_data_contract")
    expected = build_historical_contract(
        bars, timeframe=manifest.get("timeframe"), asof=att.get("asof")
    )
    if contract != expected:
        raise HistoricalDataUnqualified(
            "historical contract/content/quality policy mismatch"
        )
    identity = sha256_json(contract)
    if identity != manifest.get("historical_data_contract_id") or identity != att.get(
        "historical_data_contract_id"
    ):
        raise HistoricalDataUnqualified("historical contract identity mismatch")
    if (
        manifest.get("price_adjustment_convention") != "alpaca_all_adjusted_v1"
        or manifest.get("corporate_action_policy") != "adjusted_data"
    ):
        raise HistoricalDataUnqualified("historical adjustment metadata mismatch")


def _calendar():
    path = REPO / "core-rs/crates/mqk-integrity/src/sessions.rs"
    try:
        source = path.read_text(encoding="utf-8")
        block = re.search(r"const FULL_CLOSURES:[^=]*=\s*&\[(.*?)\];", source, re.S)
        if block is None:
            raise ValueError("missing closure table")
        closures = [
            dt.date(int(y), int(m), int(d))
            for y, m, d in re.findall(r"\((\d{4}),\s*(\d+),\s*(\d+)\)", block.group(1))
        ]
        coverage = []
        for name in ("COVERAGE_START", "COVERAGE_END"):
            match = re.search(
                rf"pub const {name}:[^=]*=\s*\((\d+),\s*(\d+),\s*(\d+)\)", source
            )
            coverage.append(dt.date(*(int(x) for x in match.groups())))
        content = f"{CALENDAR_ID}\ncoverage={coverage[0]}..{coverage[1]}" + "".join(
            f"\nclosure={d}" for d in closures
        )
        if hashlib.sha256(
            content.encode()
        ).hexdigest() != CALENDAR_SHA256 or closures != sorted(set(closures)):
            raise ValueError("calendar authority fingerprint mismatch")
        return coverage, set(closures)
    except (OSError, ValueError, AttributeError) as exc:
        raise HistoricalDataUnqualified(
            f"calendar authority unavailable: {exc}"
        ) from exc


def _daily_session_check(bars, manifest, snapshot):
    if manifest.get("timeframe") not in {"1D", "1Day"}:
        raise HistoricalDataUnqualified(
            "qualified intraday resolution/calendar unsupported"
        )
    coverage, closures = _calendar()
    start = utc_instant(manifest.get("start_utc"), "window start")
    end = utc_instant(manifest.get("end_utc"), "window end")
    if end <= start:
        raise HistoricalDataUnqualified("invalid query window")
    first, last = (
        start.tz_convert("America/New_York").date(),
        end.tz_convert("America/New_York").date(),
    )
    if first < coverage[0] or last > coverage[1]:
        raise HistoricalDataUnqualified("query outside calendar authority coverage")
    expected = []
    for day in pd.date_range(first, last, freq="D"):
        date = day.date()
        label = pd.Timestamp(date).tz_localize("America/New_York").tz_convert("UTC")
        if start <= label < end and date.weekday() < 5 and date not in closures:
            expected.append(label.isoformat())
    if not expected:
        raise HistoricalDataUnqualified("no expected sessions in query window")
    times = [utc_instant(t, "bar timestamp") for t in bars["end_ts"]]
    for ts in times:
        local = ts.tz_convert("America/New_York")
        if local != local.normalize():
            raise HistoricalDataUnqualified(
                "daily timestamp is not an America/New_York midnight session label"
            )
        # A following civil midnight is conservative finalization evidence for all
        # supported daily sessions, including early closes; no publication claim.
        final_boundary = (local.normalize() + pd.DateOffset(days=1)).tz_convert("UTC")
        if snapshot < final_boundary:
            raise HistoricalDataUnqualified(
                "partial session: snapshot precedes conservative finalization boundary"
            )
    for symbol in manifest["symbol_universe"]:
        actual = sorted(
            utc_instant(t, "bar timestamp").isoformat()
            for t in bars.loc[bars["symbol"] == symbol, "end_ts"]
        )
        if actual != expected:
            raise HistoricalDataUnqualified(
                f"session coverage mismatch for {symbol}: missing/extra sessions (halt/delisting cause unproven)"
            )
    return len(expected)


def require_historical_dataset(
    bars,
    manifest,
    *,
    decision_time_utc,
    require_point_in_time=False,
    require_pit_universe=False,
    require_stable_instruments=False,
):
    from mqk_research.data.bars_provenance import (
        check_corporate_action_integrity,
        require_bars_match_manifest,
        require_registered_bars_provenance,
        provenance_identity_fragment_canonical_timeframe,
    )

    if not manifest.get("historical_data_contract"):
        raise HistoricalDataUnqualified(
            "missing historical contract; legacy data is not historically qualified"
        )
    require_registered_bars_provenance(manifest)
    require_bars_match_manifest(bars, manifest)
    check_corporate_action_integrity(bars, manifest)
    if require_point_in_time:
        raise HistoricalDataUnqualified(
            "provider historical publication/revision vintage unavailable; retrieval and mapping-asof are not PIT evidence"
        )
    if require_pit_universe:
        raise HistoricalDataUnqualified(
            "historical PIT universe membership provider capability unavailable"
        )
    if require_stable_instruments:
        raise HistoricalDataUnqualified(
            "complete symbol/delisting/stable instrument history capability unavailable"
        )
    decision = utc_instant(decision_time_utc, "research decision")
    snapshot = utc_instant(
        (manifest.get("source_attestation") or {}).get("retrieval_timestamp_utc"),
        "retrieval snapshot",
    )
    if decision < snapshot:
        raise HistoricalDataUnqualified(
            "future-known snapshot/revision refused before retrieval; PIT availability unknown"
        )
    sessions = _daily_session_check(bars, manifest, snapshot)
    return {
        "eligibility": "QUALIFIED_RETROSPECTIVE_SNAPSHOT",
        "point_in_time_qualified": False,
        "dataset_id": sha256_json(
            provenance_identity_fragment_canonical_timeframe(manifest)
        ),
        "expected_sessions_per_symbol": sessions,
        "research_decision_utc": decision.isoformat(),
        "retrieval_snapshot_utc": snapshot.isoformat(),
        "publication_time": None,
        "revision_vintage": None,
        "mapping_asof": manifest["historical_data_contract"]["mapping_asof"],
        "universe_membership": "NOT_PIT",
        "instrument_history": "UNPROVEN",
        "timestamp_meaning": manifest["historical_data_contract"]["timestamp_meaning"],
    }
