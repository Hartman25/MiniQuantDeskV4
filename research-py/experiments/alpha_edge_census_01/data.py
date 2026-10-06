"""Per-symbol SIP/adjustment=all daily bar acquisition + provenance-verified loading for the census.

Each symbol is extracted on its own so one symbol's corporate-action review requirement or missing history
makes only that symbol NON_EVALUABLE; it never shrinks the universe. No IEX fallback exists."""

from __future__ import annotations

import json
import os
import sys
import time
import urllib.error
from pathlib import Path

import pandas as pd

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "src"))

from partitions import DATA_REQUEST_START_UTC, DISCOVERY_END_EXCLUSIVE, require_discovery_only  # noqa: E402

REPO = HERE.parents[2]
REQUEST_CONTRACT = {
    "provider": "alpaca", "feed": "sip", "adjustment": "all", "timeframe": "1Day",
    "start_utc": DATA_REQUEST_START_UTC.isoformat(), "end_utc_exclusive": DISCOVERY_END_EXCLUSIVE.isoformat(),
    "asof": "2026-10-05", "extractor": "mqk_research.data.alpaca_historical.extract_research_bars_with_provenance",
}
RETRIES = 5
_TRANSIENT_MARKERS = ("status=429", "status=500", "status=502", "status=503", "status=504")


def load_alpaca_env() -> None:
    """Load only the two research-data credential keys from .env.local; values are never printed."""
    want = ("ALPACA_API_KEY_PAPER", "ALPACA_API_SECRET_PAPER")
    if all(os.environ.get(k) for k in want):
        return
    env_file = REPO / ".env.local"
    if env_file.exists():
        for line in env_file.read_text(encoding="utf-8").splitlines():
            k, _, v = line.partition("=")
            if k.strip() in want and v.strip():
                os.environ[k.strip()] = v.strip().strip('"').strip("'")
    missing = [k for k in want if not os.environ.get(k)]
    if missing:
        raise SystemExit(f"fail-closed: credentials unavailable: {missing}")


def _is_transient(exc: BaseException) -> bool:
    if isinstance(exc, (urllib.error.URLError, TimeoutError, ConnectionError)):
        return True
    return any(m in str(exc) for m in _TRANSIENT_MARKERS)


def acquire_symbol(symbol: str, sym_dir: Path) -> dict:
    """Fetch one symbol. Returns the status record (also written to status.json)."""
    from mqk_research.data.alpaca_historical import (
        AlpacaHistoricalExtractionError, CorporateActionReviewRequired,
        extract_research_bars_with_provenance, write_research_extraction_artifacts)
    status_path = sym_dir / "status.json"
    if status_path.exists():
        prior = json.loads(status_path.read_text(encoding="utf-8"))
        if prior.get("request_contract") != REQUEST_CONTRACT:
            raise SystemExit(f"fail-closed: cached {symbol} was acquired under a different request contract")
        return prior
    load_alpaca_env()
    rec = {"symbol": symbol, "request_contract": REQUEST_CONTRACT, "infrastructure_attempts": 0}
    for attempt in range(1, RETRIES + 1):
        rec["infrastructure_attempts"] = attempt
        try:
            result = extract_research_bars_with_provenance(
                symbols=[symbol], start_utc=DATA_REQUEST_START_UTC, end_utc=DISCOVERY_END_EXCLUSIVE,
                asof=REQUEST_CONTRACT["asof"], timeframe="1Day", feed="sip")
            require_discovery_only(result["bars"]["end_ts"], what=f"{symbol} fetched bars")
            paths = write_research_extraction_artifacts(sym_dir, result)
            rec.update({"disposition": "DATA_PRESENT", "rows": int(len(result["bars"])),
                        "first_end_ts": str(result["bars"]["end_ts"].min()),
                        "last_end_ts": str(result["bars"]["end_ts"].max()),
                        "artifact_names": sorted(p.name for p in paths.values())})
            break
        except CorporateActionReviewRequired as exc:
            rec.update({"disposition": "NON_EVALUABLE_UNSUPPORTED_CORPORATE_ACTION", "detail": str(exc)[:600]})
            break
        except Exception as exc:  # noqa: BLE001 - classified below, never swallowed silently
            if _is_transient(exc) and attempt < RETRIES:
                time.sleep(2 ** attempt)
                continue
            kind = ("DATA_UNAVAILABLE_PROVIDER_ERROR" if isinstance(exc, AlpacaHistoricalExtractionError)
                    else "DATA_UNAVAILABLE_UNEXPECTED_ERROR")
            rec.update({"disposition": kind, "detail": f"{type(exc).__name__}: {str(exc)[:600]}"})
            break
    sym_dir.mkdir(parents=True, exist_ok=True)
    status_path.write_text(json.dumps(rec, sort_keys=True, indent=1), encoding="utf-8")
    return rec


def acquire_universe(symbols, data_dir: Path) -> dict:
    data_dir = Path(data_dir)
    return {s: acquire_symbol(s, data_dir / s) for s in symbols}


def require_sip_all_contract(manifest: dict) -> None:
    """The attested retrieval must be SIP + adjustment=all over the frozen request window; no IEX/other feed."""
    att = manifest.get("source_attestation") or {}
    for key, val in (("feed", "sip"), ("adjustment_mode", "all"), ("source_provider_id", "alpaca")):
        if att.get(key) != val:
            raise SystemExit(f"fail-closed: bars attestation {key}={att.get(key)!r} != {val!r} (SIP/adjustment=all only)")
    if pd.Timestamp(att.get("requested_start_utc")) != DATA_REQUEST_START_UTC or (
            pd.Timestamp(att.get("requested_end_utc")) != DISCOVERY_END_EXCLUSIVE):
        raise SystemExit("fail-closed: bars attestation window differs from the frozen discovery request window")


def load_symbol_bars(sym_dir: Path) -> tuple[pd.DataFrame, dict]:
    """Load one symbol's bars verified against its provenance manifest (physical sha256 + canonical semantic
    hash + fixed_ex_ante registered-provenance shape) and fenced to the discovery partition."""
    from mqk_research.data.bars_provenance import require_bars_match_manifest, require_registered_bars_provenance
    from mqk_research.ml.util_hash import sha256_file
    manifest = json.loads((sym_dir / "research_bars_provenance.json").read_text(encoding="utf-8"))
    csv_path = sym_dir / "research_bars.csv"
    if sha256_file(csv_path) != manifest["artifact_sha256"]:
        raise SystemExit(f"fail-closed: {csv_path} does not match its provenance artifact_sha256")
    bars = pd.read_csv(csv_path)
    if len(bars) != manifest["row_count"]:
        raise SystemExit(f"fail-closed: {csv_path} row count differs from its provenance manifest")
    require_registered_bars_provenance(manifest)
    require_sip_all_contract(manifest)
    require_bars_match_manifest(bars, manifest)
    require_discovery_only(bars["end_ts"], what=str(csv_path))
    bars["end_ts"] = pd.to_datetime(bars["end_ts"], utc=True)
    return bars.sort_values("end_ts", kind="mergesort").reset_index(drop=True), manifest
