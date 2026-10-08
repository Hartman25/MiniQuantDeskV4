"""Session-start snapshot of the real Census-02 run directory, taken before pytest collects (and so imports) any test module or
Census-02 module. Lets the import-safety proof see an import-time side effect even when pytest itself performs that import."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import census02_run_snapshot as _snap  # noqa: E402


def pytest_configure(config):
    config._census02_run_dir_at_session_start = _snap.tree_snapshot(_snap.REAL_RUN_DIR)


@pytest.fixture
def census02_run_dir_at_session_start(request):
    return request.config._census02_run_dir_at_session_start
