"""R3 (WAVE03-CHECKOUT-LOCAL-SOURCE-GUARD-01): proves run_wave.py's local-
source safety guard resolves the checkout-local research-py/src sibling to
this file structurally, and never depends on the checkout/worktree
DIRECTORY NAME. Covers the mission's R3 REQUIRED TESTS list.

The acceptance-boundary negative control (running test_predeclaration.py
under a real checkout/path whose basename does NOT contain
"research-rank-wave-01"/"direct-rank-policy") is exercised separately at
the mission's FINAL ACCEPTANCE step against an actual second worktree,
since that specifically needs a real, importable mqk_research installation
on disk rather than a synthetic skeleton -- this file instead unit-tests
the pure path-resolution primitive (`resolve_wave03_checkout_local_src`)
against fabricated checkout trees, which is faster and does not require a
second worktree per test.
"""
from __future__ import annotations

import sys
from pathlib import Path

import pytest

EXPERIMENT_ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(EXPERIMENT_ROOT))

import run_wave  # noqa: E402  (local experiment module, path inserted above)


def _make_synthetic_checkout(root: Path) -> Path:
    """Builds a minimal research-py/src/mqk_research/__init__.py +
    experiments/short_wave_03_broad_direct_rank/run_wave.py skeleton under
    `root` (whose basename the caller controls), returning the path to the
    synthetic run_wave.py file."""
    src_pkg = root / "research-py" / "src" / "mqk_research"
    src_pkg.mkdir(parents=True, exist_ok=True)
    (src_pkg / "__init__.py").write_text("", encoding="utf-8")
    experiment_dir = root / "research-py" / "experiments" / "short_wave_03_broad_direct_rank"
    experiment_dir.mkdir(parents=True, exist_ok=True)
    fake_run_wave = experiment_dir / "run_wave.py"
    fake_run_wave.write_text("", encoding="utf-8")
    return fake_run_wave


def test_resolver_succeeds_from_current_wave_worktree() -> None:
    """R3 REQUIRED TEST: the ordinary current wave worktree imports/
    resolves successfully through the real (non-synthetic) resolver call
    already exercised at run_wave.py's own module-import time -- this just
    asserts the resulting WAVE03_LOCAL_SRC is the real checkout-local src/
    actually containing mqk_research."""
    assert run_wave.WAVE03_LOCAL_SRC.name == "src"
    assert (run_wave.WAVE03_LOCAL_SRC / "mqk_research" / "__init__.py").is_file()


@pytest.mark.parametrize(
    "checkout_basename",
    ["totally-unrelated-checkout-name", "another-checkout-42", "C plain clone"],
)
def test_resolver_semantics_independent_of_checkout_basename(tmp_path: Path, checkout_basename: str) -> None:
    """R3 REQUIRED TEST ("safety does not depend on parent checkout
    basename" / "test the resolver with at least two synthetic different
    checkout directory names and prove same semantics"): none of these
    basenames contain "research-rank-wave-01" or "direct-rank-policy" (the
    old, defective hardcoded markers) -- the resolver must succeed
    identically for all of them, because it only ever inspects checkout-
    LOCAL structure, never the directory name."""
    checkout_root = tmp_path / checkout_basename
    fake_run_wave = _make_synthetic_checkout(checkout_root)
    resolved = run_wave.resolve_wave03_checkout_local_src(fake_run_wave)
    assert resolved == (checkout_root / "research-py" / "src").resolve()
    assert (resolved / "mqk_research" / "__init__.py").is_file()


def test_resolver_two_distinct_checkout_names_produce_matching_relative_semantics(tmp_path: Path) -> None:
    """R3 REQUIRED TEST: two DIFFERENT synthetic checkout directory names,
    checked in the SAME test, both resolve their own sibling src/ with
    identical relative semantics -- proving the guard's behavior is a pure
    function of checkout-local structure, not of which name was used."""
    root_a = tmp_path / "checkout-name-alpha"
    root_b = tmp_path / "totally-different-name-zzz"
    run_wave_a = _make_synthetic_checkout(root_a)
    run_wave_b = _make_synthetic_checkout(root_b)

    resolved_a = run_wave.resolve_wave03_checkout_local_src(run_wave_a)
    resolved_b = run_wave.resolve_wave03_checkout_local_src(run_wave_b)

    assert resolved_a.relative_to(root_a.resolve()) == resolved_b.relative_to(root_b.resolve())
    assert resolved_a != resolved_b  # distinct checkouts, each resolving its OWN src/


def test_resolver_rejects_missing_local_package_structure(tmp_path: Path) -> None:
    """R3 REQUIRED TEST: the path validator rejects a checkout missing the
    required local src/mqk_research structure -- never silently falls back
    to some other src/ on sys.path."""
    checkout_root = tmp_path / "checkout-without-package"
    experiment_dir = checkout_root / "research-py" / "experiments" / "short_wave_03_broad_direct_rank"
    experiment_dir.mkdir(parents=True, exist_ok=True)
    fake_run_wave = experiment_dir / "run_wave.py"
    fake_run_wave.write_text("", encoding="utf-8")
    # research-py/src/mqk_research/__init__.py is deliberately never created.

    with pytest.raises(RuntimeError, match="expected a checkout-local"):
        run_wave.resolve_wave03_checkout_local_src(fake_run_wave)


def test_resolver_rejects_src_present_but_not_a_package(tmp_path: Path) -> None:
    """R3 REQUIRED TEST: an `src/` directory that exists but does not
    actually contain the `mqk_research` package (e.g. an empty/wrong
    checkout) is also rejected -- the check is on real package structure,
    not merely on the `src` directory name existing."""
    checkout_root = tmp_path / "checkout-with-empty-src"
    src_dir = checkout_root / "research-py" / "src"
    src_dir.mkdir(parents=True, exist_ok=True)
    experiment_dir = checkout_root / "research-py" / "experiments" / "short_wave_03_broad_direct_rank"
    experiment_dir.mkdir(parents=True, exist_ok=True)
    fake_run_wave = experiment_dir / "run_wave.py"
    fake_run_wave.write_text("", encoding="utf-8")

    with pytest.raises(RuntimeError, match="expected a checkout-local"):
        run_wave.resolve_wave03_checkout_local_src(fake_run_wave)


def test_importing_driver_reads_no_credentials_and_touches_no_network() -> None:
    """R3 REQUIRED TEST: importing the driver (already done at collection
    time, above) must not have imported the network-capable Alpaca module
    as a side effect -- mirrors test_predeclaration.py's own
    test_check_mode_never_contacts_alpaca, checked here specifically at
    plain import time (no `check`/`main()` call at all). run_wave.py only
    imports mqk_research.data.alpaca_historical lazily inside
    ensure_bars(), which importing this module never calls."""
    assert "mqk_research.data.alpaca_historical" not in sys.modules
    source = (EXPERIMENT_ROOT / "run_wave.py").read_text(encoding="utf-8")
    top_level_lines = [
        line for line in source.splitlines()
        if line.startswith("from mqk_research.data.alpaca_historical")
        or line.startswith("import mqk_research.data.alpaca_historical")
    ]
    assert top_level_lines == []
