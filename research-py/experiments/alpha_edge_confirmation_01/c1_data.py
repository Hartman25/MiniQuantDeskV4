"""Per-symbol SIP/adjustment=all acquisition and provenance-verified loading for the Confirmation window
[2024-01-01, 2026-03-01). Every entry point runs the committed-freeze guard FIRST; the provider transport is wrapped so a
request or response reaching the final holdout is refused before any row is kept."""

from __future__ import annotations

import contextlib
import json
import os
import time
import urllib.error
from datetime import datetime, timezone
from pathlib import Path

import pandas as pd

import c1_cohort as cc
import c1_protocol as cp
from partitions import PartitionBreach  # noqa: E402  (census dir on sys.path via c1_cohort)

REPO = cc.REPO
WINDOW_START = pd.Timestamp(cp.WARMUP_START, tz="UTC")
WINDOW_END_EXCLUSIVE = pd.Timestamp(cp.SCORE_END_EXCLUSIVE, tz="UTC")
REQUEST_CONTRACT = cp.REQUEST_CONTRACT
ELIGIBLE = "ELIGIBLE"
EXCLUDED_DATA_UNAVAILABLE = "EXCLUDED_DATA_UNAVAILABLE"
EXCLUDED_UNSUPPORTED_CORPORATE_ACTION = "EXCLUDED_UNSUPPORTED_CORPORATE_ACTION"
EXCLUDED_PROVENANCE_REJECTED = "EXCLUDED_PROVENANCE_REJECTED"
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


def require_confirmation_window(end_ts, *, what: str) -> dict:
    """Every timestamp must lie in [2024-01-01, 2026-03-01); an empty input is refused (a guard that checked nothing proves
    nothing). Returns the observed bounds."""
    ts = pd.to_datetime(pd.Series(end_ts), utc=True)
    if ts.empty:
        raise PartitionBreach(f"{what}: empty timestamp set proves nothing")
    lo, hi = ts.min(), ts.max()
    if hi >= WINDOW_END_EXCLUSIVE:
        raise PartitionBreach(f"{what}: row at {hi.isoformat()} is not strictly before {WINDOW_END_EXCLUSIVE.date()} "
                              "(final-holdout fence)")
    if lo < WINDOW_START:
        raise PartitionBreach(f"{what}: row at {lo.isoformat()} precedes the Confirmation request window")
    return {"min": lo.isoformat(), "max": hi.isoformat(), "rows": int(len(ts))}


class GuardedTransport:
    """Wraps the Alpaca transport: refuses a bars request whose window is not the frozen one and a bars response carrying
    any row at or after the fence. Corporate-action calls pass through (metadata only; disclosed)."""

    def __init__(self, inner):
        from mqk_research.data import alpaca_historical as ah
        self.inner, self.bars_path = inner, ah.BARS_PATH
        self.bar_requests, self.raw_rows, self.raw_max_t = 0, 0, None

    def __call__(self, url, params, headers):
        if not str(url).endswith(self.bars_path):
            return self.inner(url, params, headers)
        if pd.Timestamp(params["end"]) != WINDOW_END_EXCLUSIVE or pd.Timestamp(params["start"]) != WINDOW_START:
            raise PartitionBreach(f"bars request window {params.get('start')}..{params.get('end')} differs from the frozen one")
        self.bar_requests += 1
        status, body = self.inner(url, params, headers)
        if status == 200:
            for sym_rows in (json.loads(body.decode("utf-8")).get("bars") or {}).values():
                for row in sym_rows or []:
                    t = pd.Timestamp(row["t"])
                    t = t.tz_localize("UTC") if t.tzinfo is None else t.tz_convert("UTC")
                    if t >= WINDOW_END_EXCLUSIVE:
                        raise PartitionBreach(f"provider returned a bar at {t.isoformat()} >= {WINDOW_END_EXCLUSIVE.date()}")
                    self.raw_rows += 1
                    self.raw_max_t = t if self.raw_max_t is None or t > self.raw_max_t else self.raw_max_t
        return status, body


@contextlib.contextmanager
def guarded_provider():
    from mqk_research.data import alpaca_historical as ah
    original = ah._default_http_get  # noqa: SLF001
    guard = GuardedTransport(original)
    ah._default_http_get = guard  # noqa: SLF001
    try:
        yield guard
    finally:
        ah._default_http_get = original  # noqa: SLF001


def _freeze_kwargs(freeze) -> dict:
    return dict(freeze) if freeze else {}


def acquire_symbol(symbol: str, sym_dir: Path, *, freeze=None) -> dict:
    """Fetch one symbol. The committed-freeze guard runs before the cache, credentials or any provider call."""
    cc.require_freeze(**_freeze_kwargs(freeze))
    from mqk_research.data.alpaca_historical import (
        AlpacaHistoricalExtractionError, CorporateActionReviewRequired,
        extract_research_bars_with_provenance, write_research_extraction_artifacts)
    status_path = Path(sym_dir) / "status.json"
    if status_path.exists():
        prior = json.loads(status_path.read_text(encoding="utf-8"))
        if prior.get("request_contract") != REQUEST_CONTRACT:
            raise SystemExit(f"fail-closed: cached {symbol} was acquired under a different request contract")
        return prior
    load_alpaca_env()
    rec = {"symbol": symbol, "request_contract": REQUEST_CONTRACT, "infrastructure_attempts": 0,
           "first_request_utc": datetime.now(timezone.utc).isoformat()}
    for attempt in range(1, RETRIES + 1):
        rec["infrastructure_attempts"] = attempt
        try:
            with guarded_provider() as guard:
                result = extract_research_bars_with_provenance(
                    symbols=[symbol], start_utc=WINDOW_START, end_utc=WINDOW_END_EXCLUSIVE,
                    asof=REQUEST_CONTRACT["asof"], timeframe="1Day", feed="sip")
            bounds = require_confirmation_window(result["bars"]["end_ts"], what=f"{symbol} fetched bars")
            paths = write_research_extraction_artifacts(Path(sym_dir), result)
            rec.update({"disposition": "DATA_PRESENT", "rows": bounds["rows"], "first_end_ts": bounds["min"],
                        "last_end_ts": bounds["max"], "bar_requests": guard.bar_requests, "raw_rows": guard.raw_rows,
                        "raw_max_t": guard.raw_max_t.isoformat() if guard.raw_max_t is not None else None,
                        "artifact_names": sorted(p.name for p in paths.values())})
            break
        except PartitionBreach:
            raise
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
    Path(sym_dir).mkdir(parents=True, exist_ok=True)
    status_path.write_text(json.dumps(rec, sort_keys=True, indent=1), encoding="utf-8")
    return rec


def require_sip_all_contract(manifest: dict) -> None:
    att = manifest.get("source_attestation") or {}
    for key, val in (("feed", "sip"), ("adjustment_mode", "all"), ("source_provider_id", "alpaca")):
        if att.get(key) != val:
            raise SystemExit(f"fail-closed: bars attestation {key}={att.get(key)!r} != {val!r} (SIP/adjustment=all only)")
    if pd.Timestamp(att.get("requested_start_utc")) != WINDOW_START or \
            pd.Timestamp(att.get("requested_end_utc")) != WINDOW_END_EXCLUSIVE:
        raise SystemExit("fail-closed: bars attestation window differs from the frozen Confirmation request window")


def load_symbol_bars(sym_dir: Path, *, freeze=None) -> tuple[pd.DataFrame, dict]:
    """Freeze guard first; then bars verified against the provenance manifest (physical sha256 + canonical semantic hash +
    registered-provenance shape) and fenced to [2024-01-01, 2026-03-01)."""
    cc.require_freeze(**_freeze_kwargs(freeze))
    from mqk_research.data.bars_provenance import require_bars_match_manifest, require_registered_bars_provenance
    from mqk_research.ml.util_hash import sha256_file
    sym_dir = Path(sym_dir)
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
    require_confirmation_window(bars["end_ts"], what=str(csv_path))
    bars["end_ts"] = pd.to_datetime(bars["end_ts"], utc=True)
    return bars.sort_values("end_ts", kind="mergesort").reset_index(drop=True), manifest


def classify_symbol(symbol: str, status: dict, sym_dir: Path, *, freeze=None) -> dict:
    """One typed disposition per universe symbol; a failure never shrinks the universe silently."""
    acq = status.get("disposition")
    rec = {"symbol": symbol, "acquisition_status": acq}
    if acq == "NON_EVALUABLE_UNSUPPORTED_CORPORATE_ACTION":
        return {**rec, "disposition": EXCLUDED_UNSUPPORTED_CORPORATE_ACTION, "detail": status.get("detail", "")}
    if acq != "DATA_PRESENT":
        return {**rec, "disposition": EXCLUDED_DATA_UNAVAILABLE, "detail": status.get("detail", "")}
    try:
        bars, manifest = load_symbol_bars(sym_dir, freeze=freeze)
    except (cc.FreezeRefusal, PartitionBreach):
        raise
    except (SystemExit, Exception) as exc:  # noqa: BLE001 - every verification failure is a typed exclusion
        return {**rec, "disposition": EXCLUDED_PROVENANCE_REJECTED, "detail": f"{type(exc).__name__}: {str(exc)[:300]}"}
    return {**rec, "disposition": ELIGIBLE, "observations": int(len(bars)), "artifact_sha256": manifest["artifact_sha256"]}
