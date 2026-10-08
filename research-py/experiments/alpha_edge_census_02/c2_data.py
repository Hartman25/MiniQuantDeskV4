"""The ONE real-data entrance for Census-02. Nothing here runs at import time and nothing here is reachable except through
c2_runner.run_campaign AFTER the freeze gate. It reuses the accepted Census-01 acquisition / provenance-verified loading
under the frozen request contract; no new provider, feed or adjustment convention exists."""

from __future__ import annotations

import json
import sys
from dataclasses import dataclass
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import c2_borrow as bw  # noqa: E402
import c2_protocol as pr  # noqa: E402
import census as ce  # noqa: E402
import data as dt  # noqa: E402
import search_space as ss1  # noqa: E402


class DataContractRefusal(RuntimeError):
    pass


@dataclass(frozen=True)
class LoadedUniverse:
    universe: dict          # eligible-universe document (symbols = ELIGIBLE only; dispositions cover every seed symbol)
    bars: dict              # symbol -> discovery-fenced bars DataFrame
    bars_manifest: dict     # immutable bars/provenance manifest (carries manifest_sha256)


def require_request_contract(frozen: dict) -> None:
    """The acquisition code, the structural protocol and the frozen predeclaration must carry the same contract."""
    if frozen != pr.DATA_REQUEST_CONTRACT:
        raise DataContractRefusal("frozen request contract differs from the structural protocol")
    if {k: frozen[k] for k in dt.REQUEST_CONTRACT} != dt.REQUEST_CONTRACT:
        raise DataContractRefusal("Census-01 acquisition contract differs from the frozen Census-02 request contract")


def load_discovery_universe(data_dir: Path, request_contract: dict, protocol_id: str) -> LoadedUniverse:
    """Acquire (or reuse a same-contract cache of) every seed symbol, classify exactly one typed disposition each, build the
    provenance manifest and load verified discovery-fenced bars. Refuses unless require_freeze passed in THIS process for
    `protocol_id`; the first statement, before any contract check, file or network access."""
    pr.assert_freeze_gate_passed(protocol_id)
    require_request_contract(request_contract)
    seed = json.loads(bw.SEED_UNIVERSE_FILE.read_text(encoding="utf-8"))
    symbols = sorted(seed["symbols"])
    data_dir = Path(data_dir)
    statuses = dt.acquire_universe(symbols, data_dir)
    dispositions = {s: dt.classify_eligibility(s, statuses[s], data_dir / s) for s in symbols}
    universe = ss1.build_universe(seed, dispositions)
    manifest = ce.build_bars_manifest(universe, data_dir)
    return LoadedUniverse(universe, ce.load_bars(universe, data_dir, manifest), manifest)
