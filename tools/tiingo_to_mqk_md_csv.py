import csv
import json
import os
import time
from datetime import datetime, timezone
from pathlib import Path

import requests

# Usage:
#   python tools/tiingo_to_mqk_md_csv.py AAPL,MSFT,NVDA 2011-01-01 2026-02-24 data\canonical\tiingo_top10_1D.csv
#
# Notes:
# - Uses Tiingo EOD endpoint via header auth.
# - Writes canonical CSV matching mqk-md CSV contract:
#   symbol,timeframe,end_ts,open,high,low,close,volume,is_complete

TIINGO_API_KEY = os.environ.get("TIINGO_API_KEY")
if not TIINGO_API_KEY:
    raise SystemExit("Missing env var: TIINGO_API_KEY")

BASE = "https://api.tiingo.com/tiingo/daily"

def iso_to_epoch_seconds(iso_str: str) -> int:
    # Tiingo returns ISO timestamps like 2017-08-01T00:00:00.000Z in many examples
    # Robust parse:
    s = iso_str.strip()
    if s.endswith("Z"):
        s = s[:-1] + "+00:00"
    dt = datetime.fromisoformat(s)
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return int(dt.astimezone(timezone.utc).timestamp())

def fetch_prices(symbol: str, start: str, end: str):
    url = f"{BASE}/{symbol}/prices"
    params = {
        "startDate": start,
        "endDate": end,
        "resampleFreq": "daily",
        # You can add "columns" here, but Tiingo typically returns the full set including adj* on EOD.
    }
    headers = {"Authorization": f"Token {TIINGO_API_KEY}"}
    r = requests.get(url, params=params, headers=headers, timeout=60)
    r.raise_for_status()
    return r.json()

from decimal import Decimal, ROUND_HALF_UP, InvalidOperation

def to_6dp_str(x) -> str:
    """
    Convert Tiingo numeric to a decimal string with <= 6 fractional digits
    (micro price precision), no scientific notation.
    """
    try:
        d = Decimal(str(x)).quantize(Decimal("0.000001"), rounding=ROUND_HALF_UP)
    except (InvalidOperation, ValueError):
        raise ValueError(f"invalid decimal price: {x!r}")
    # Normalize but keep fixed-point
    s = format(d, "f")
    # Optional: strip trailing zeros but keep at least 1 decimal if desired.
    # md.rs accepts either whole or frac up to 6 digits.
    if "." in s:
        s = s.rstrip("0").rstrip(".") or "0"
    return s

def pick_adj(row: dict, key: str, fallback: str) -> str:
    v = row.get(key)
    if v is None:
        v = row.get(fallback)
    if v is None:
        raise ValueError(f"missing {key}/{fallback} in row keys={list(row.keys())}")
    return to_6dp_str(v)

def main(symbols_csv: str, start: str, end: str, out_csv: str):
    symbols = [s.strip().upper() for s in symbols_csv.split(",") if s.strip()]
    out_path = Path(out_csv)
    out_path.parent.mkdir(parents=True, exist_ok=True)

    fieldnames = ["symbol","timeframe","end_ts","open","high","low","close","volume","is_complete"]

    with out_path.open("w", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=fieldnames)
        w.writeheader()

        for sym in symbols:
            # Pull
            data = fetch_prices(sym, start, end)

            # Save raw for audit
            raw_path = Path("data/raw/tiingo") / f"{sym}_{start}_to_{end}.json"
            raw_path.parent.mkdir(parents=True, exist_ok=True)
            raw_path.write_text(json.dumps(data, indent=2), encoding="utf-8")

            # Convert
            for row in data:
                # date field name is typically "date"
                end_ts = iso_to_epoch_seconds(row["date"])

                # Prefer adjusted fields if present
                o = pick_adj(row, "adjOpen", "open")
                h = pick_adj(row, "adjHigh", "high")
                l = pick_adj(row, "adjLow", "low")
                c = pick_adj(row, "adjClose", "close")

                # Prefer adjVolume if present
                vol = row.get("adjVolume", row.get("volume", 0))
                try:
                    vol_int = int(vol)
                except Exception:
                    vol_int = 0  # keep deterministic; you’ll catch this in quality gate
                if vol_int < 0:
                    vol_int = 0

                w.writerow({
                    "symbol": sym,
                    "timeframe": "1D",
                    "end_ts": str(end_ts),
                    "open": o,
                    "high": h,
                    "low": l,
                    "close": c,
                    "volume": str(vol_int),
                    "is_complete": "true",
                })

            # Be nice to rate limits
            time.sleep(0.25)

    print(f"Wrote canonical CSV: {out_path}")

if __name__ == "__main__":
    import sys
    if len(sys.argv) != 5:
        raise SystemExit("Usage: python tools/tiingo_to_mqk_md_csv.py SYMBOLS startDate endDate out.csv")
    main(sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4])