from __future__ import annotations

from pathlib import Path

# Scaffold only: this file shows the intended flow.
# You can wire it to your existing mqk_research.cli later.

def main() -> int:
    # 1) ingest/load bars (db or csv)
    # 2) build features (feature_set_v1)
    # 3) select universe
    # 4) generate signal_pack.csv
    # 5) run backtest consuming signal_pack
    # 6) build report (and optional tax drag)
    print("example_momentum scaffold: not wired")
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
