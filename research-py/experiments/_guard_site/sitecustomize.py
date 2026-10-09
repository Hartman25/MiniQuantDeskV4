"""Loaded automatically by every Python child that inherits `PYTHONPATH=<this directory>` (see `_netguard`).
Installs the same non-removable audit guard in the child before any of its code runs."""

import os
import sys

_EXPERIMENTS = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _EXPERIMENTS not in sys.path:
    sys.path.insert(0, _EXPERIMENTS)
import _netguard  # noqa: E402

_netguard.install_child()
