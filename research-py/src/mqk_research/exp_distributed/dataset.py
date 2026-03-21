from __future__ import annotations

from dataclasses import replace
from pathlib import Path
from typing import Iterable

import pandas as pd

from .hashing import short_hash, stable_hash, sha256_file
from .models import DatasetFingerprint, JobSpec


_REQUIRED_COLUMNS = {"symbol", "timeframe", "end_ts", "close"}


def _normalize_end_ts(series: pd.Series) -> pd.Series:
    if pd.api.types.is_integer_dtype(series) or pd.api.types.is_float_dtype(series):
        numeric = series.astype("int64")
        threshold_ms = 10_000_000_000
        if int(numeric.abs().max()) >= threshold_ms:
            return pd.to_datetime(numeric, unit="ms", utc=True)
        return pd.to_datetime(numeric, unit="s", utc=True)
    return pd.to_datetime(series, utc=True)


def load_dataset(path: Path) -> pd.DataFrame:
    frame = pd.read_csv(path)
    missing = sorted(_REQUIRED_COLUMNS.difference(frame.columns))
    if missing:
        raise ValueError(f"dataset missing required columns: {missing}")
    data = frame.copy()
    data["symbol"] = data["symbol"].astype(str).str.upper()
    data["timeframe"] = data["timeframe"].astype(str)
    data["ts_utc"] = _normalize_end_ts(data["end_ts"])
    data["close"] = data["close"].astype(float)
    if "open" in data.columns:
        data["open"] = data["open"].astype(float)
    data = data.sort_values(["symbol", "ts_utc", "end_ts"], kind="mergesort").reset_index(drop=True)
    return data


def load_job_slice(job: JobSpec) -> pd.DataFrame:
    data = load_dataset(Path(job.dataset_fingerprint.dataset_path))
    start = pd.Timestamp(job.window.start_utc).tz_convert("UTC")
    end = pd.Timestamp(job.window.end_utc).tz_convert("UTC")
    filtered = data[
        (data["timeframe"] == job.timeframe)
        & (data["symbol"].isin(job.symbols))
        & (data["ts_utc"] >= start)
        & (data["ts_utc"] <= end)
    ].copy()
    filtered = filtered.sort_values(["ts_utc", "symbol"], kind="mergesort").reset_index(drop=True)
    if filtered.empty:
        raise ValueError(
            "filtered dataset slice is empty "
            f"for symbols={job.symbols} start={job.window.start_utc} end={job.window.end_utc}"
        )
    return filtered


def build_dataset_fingerprint(
    dataset_path: Path,
    symbols: Iterable[str],
    start_utc: str,
    end_utc: str,
    timeframe: str,
    dataset_sha256: str | None = None,
) -> DatasetFingerprint:
    normalized_symbols = sorted(dict.fromkeys([str(symbol).upper() for symbol in symbols]))
    dataset_hash = dataset_sha256 or sha256_file(dataset_path)
    selection_payload = {
        "dataset_path": str(dataset_path),
        "dataset_sha256": dataset_hash,
        "symbols": normalized_symbols,
        "start_utc": start_utc,
        "end_utc": end_utc,
        "timeframe": timeframe,
    }
    return DatasetFingerprint(
        dataset_path=str(dataset_path),
        dataset_sha256=dataset_hash,
        selection_sha256=short_hash(selection_payload, length=32),
        timeframe=timeframe,
        symbols=normalized_symbols,
        start_utc=start_utc,
        end_utc=end_utc,
    )


def finalize_dataset_fingerprint(base: DatasetFingerprint, filtered: pd.DataFrame) -> DatasetFingerprint:
    digest_payload = filtered[["symbol", "timeframe", "end_ts", "close"]].to_dict(orient="records")
    return replace(
        base,
        filtered_row_count=int(len(filtered)),
        filtered_sha256=stable_hash(digest_payload),
    )
