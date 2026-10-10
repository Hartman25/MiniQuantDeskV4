"""Read-only view of the Final-Holdout access-incident ledger for Factory readiness and reports.

The ledger can only BLOCK an independence claim; this view never grants one. It reuses the accepted module and its
pinned-chain verification unchanged.
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any, Mapping

from mqk_research.strategy_factory.campaign import EXPERIMENTS_REL


def summary(repo_root: Path, decl: Mapping[str, Any]) -> dict[str, Any]:
    exp = str(Path(repo_root) / EXPERIMENTS_REL)
    if exp not in sys.path:
        sys.path.insert(0, exp)
    import holdout_incident  # noqa: PLC0415 - accepted authority next to the runner
    return holdout_incident.truth_summary(dict(decl))
