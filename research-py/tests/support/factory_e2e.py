"""Integrated Strategy Factory harness: a synthetic, clearly-labelled bars fixture, a campaign spec builder and an
operator stand-in. The operator stand-in mints a stage authorization with a SYNTHETIC key through the accepted
`stage_authorization.mint` (the real signing and verification path); the Factory itself never mints. Nothing here
touches a provider, a broker, the Paper database or a reserved holdout window.
"""

from __future__ import annotations

import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import pandas as pd

REPO = Path(__file__).resolve().parents[3]
EXP = REPO / "research-py" / "experiments" / "m1_native_trend_campaign"
sys.path.insert(0, str(REPO / "research-py" / "src"))
sys.path.insert(0, str(EXP))

from mqk_research.data.bars_provenance import (  # noqa: E402
    CA_POLICY_FORBID_AFFECTED_PERIODS, PRICE_CONVENTION_RAW_UNADJUSTED, UNIVERSE_MODE_FIXED_EX_ANTE,
    build_bars_provenance_manifest, build_corporate_action_evidence)

TEST_KEY = "f" * 40
DEFAULT_CLI = Path(os.environ.get("MQK_FACTORY_CLI") or "C:/tmp/mqk-target-factory/debug/mqk-cli.exe")
HOLDOUT_START, HOLDOUT_END = "2021-07-01T00:00:00Z", "2022-01-01T00:00:00Z"


def cli_available() -> bool:
    return DEFAULT_CLI.is_file()


def make_bars_dir(root: Path, symbols=("SPY", "QQQ"), seed: int = 7, drift: float = 0.0004, end: str = "2021-06-30", name: str = "bars_src") -> dict:
    """A verified-looking bars directory (research_bars.csv + provenance + corporate-action files) of SYNTHETIC prices."""
    data = Path(root) / name
    data.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(seed)
    dates = pd.bdate_range("2018-01-01", end, tz="UTC")
    rows = []
    for k, sym in enumerate(symbols):
        px = 100.0 + 10 * k
        for d in dates:
            px *= 1.0 + rng.normal(drift, 0.01)
            rows.append({"symbol": sym, "end_ts": d.isoformat(), "open": px, "high": px * 1.002, "low": px * 0.998, "close": px, "volume": 1_000_000})
    csv = data / "research_bars.csv"
    pd.DataFrame(rows).to_csv(csv, index=False)
    bars = pd.read_csv(csv)
    ts = pd.to_datetime(bars["end_ts"], utc=True)
    start, end = ts.min().isoformat(), (ts.max() + pd.Timedelta(seconds=1)).isoformat()
    ev = build_corporate_action_evidence(source_provider_id="synthetic_fixture", covered_symbol_universe=sorted(symbols),
                                         coverage_start_utc=start, coverage_end_utc=end, corporate_action_entries=())
    manifest = build_bars_provenance_manifest(
        price_provenance={"close_column": "close", "provider_ids_observed": ["synthetic_fixture"], "price_adjustment_convention": PRICE_CONVENTION_RAW_UNADJUSTED,
                          "provider_metadata_available": True, "convention_basis": "synthetic factory fixture"},
        corporate_action_policy=CA_POLICY_FORBID_AFFECTED_PERIODS, corporate_action_evidence_id=ev["evidence_id"],
        corporate_action_evidence=ev, forbidden_periods=(), timeframe="1Day", start_utc=start, end_utc=end,
        symbol_universe=sorted(symbols), universe_mode=UNIVERSE_MODE_FIXED_EX_ANTE, bars=bars, artifact_path=csv)
    (data / "research_bars_provenance.json").write_text(json.dumps(manifest), encoding="utf-8")
    (data / "corporate_actions.json").write_text(json.dumps(ev), encoding="utf-8")
    (data / "corporate_actions_provenance.json").write_text(json.dumps({"synthetic": True}), encoding="utf-8")
    return {"dir": data, "manifest": manifest, "symbols": sorted(symbols)}


def make_spec(campaign_id: str, bars: dict, *, sources, grade="SYNTHETIC_DIAGNOSTIC", max_trials=20) -> dict:
    m = bars["manifest"]
    pins = {"expected_artifact_sha256": m["artifact_sha256"], "expected_row_count": m["row_count"],
            "expected_canonical_semantic_bars_hash": m["canonical_semantic_bars_hash"]}
    if m.get("source_attestation_id"):
        pins["expected_source_attestation_id"] = m["source_attestation_id"]
    return {
        "schema": "factory_campaign_spec_v1", "campaign_id": campaign_id, "evidence_grade": grade, "protocol_profile": "m1_batch03_v1",
        "predeclared_utc_date": "2026-10-10",
        "population": {"sources": sources, "symbols": bars["symbols"], "max_trials": max_trials},
        "data": {"mode": "reuse", "reuse_from": str(bars["dir"]).replace("\\", "/"), "feed": "synthetic", "adjustment": "none",
                 "start_utc": "2018-01-01T00:00:00Z", "end_utc": HOLDOUT_START, "asof": "2026-10-10", **pins},
        "partition": {"evaluation_start_utc": "2019-07-01T00:00:00Z", "test_months": 6, "holdout_months": 6, "expected_folds": 4,
                      "holdout_boundary": {"version": "fixed_holdout_boundary_v1", "holdout_start_utc": HOLDOUT_START, "holdout_end_utc": HOLDOUT_END}},
    }


def operator_release_and_authorize(decl_path: Path, cli: Path, auth_dir: Path | None = None, *, classes=None, now=None, key=TEST_KEY,
                                   valid_hours=24, in_run_dir: bool = False) -> dict:
    """Operator stand-in: release the gate (re-issue) and mint a signed stage authorization for this exact declaration."""
    import hashlib
    from datetime import timedelta
    import stage_authorization as sa
    decl = json.loads(Path(decl_path).read_text(encoding="utf-8"))
    decl["execution_gate"] = {"status": "FACTORY_RELEASED_BY_OPERATOR_STANDIN", "executable": True, "blocker": None,
                              "rule": "synthetic test release; identity is unchanged because execution_gate is excluded from it"}
    Path(decl_path).write_text(json.dumps(decl, indent=1, sort_keys=True), encoding="utf-8")
    classes = classes or [c for c in sa.AUTHORIZABLE if c not in sa.INCIDENT_BLOCKED]
    auth = sa.mint(decl, list(classes), operator="test-operator", approval_ref="TEST", key=key, now=now or datetime.now(timezone.utc),
                   valid_for=timedelta(hours=valid_hours), cli_sha256=hashlib.sha256(Path(cli).resolve().read_bytes()).hexdigest())
    if in_run_dir:                                   # the per-campaign convention the executor looks for
        auth_dir = Path(decl["run_dir"])
        name = "stage_authorization.json"
    else:
        name = "authorization.json"
    auth_dir.mkdir(parents=True, exist_ok=True)
    path = auth_dir / name
    path.write_text(json.dumps(auth), encoding="utf-8")
    return {"auth": auth, "path": path, "env": {sa.KEY_ENV: key, sa.AUTH_FILE_ENV: str(path)}}
