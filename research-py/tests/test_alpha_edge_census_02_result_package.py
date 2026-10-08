"""Census-02 Result #1 packager invariants: the preserved Strategy / factor ledgers are exact copies of the settled evidence and the
packager refuses any count / duplicate / missing / extra / order / status-mix deviation. Synthetic records only: no run directory."""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import c2_testing as ct  # noqa: E402,F401  (puts the Census-02 modules on sys.path)

sys.path.insert(0, str(ct.EXP2 / "results"))
import build_result_package as bp  # noqa: E402


def _chunks(tmp_path: Path, ids: list[str], size: int) -> list[Path]:
    paths = []
    for k in range(0, len(ids), size):
        p = tmp_path / f"chunk_{k // size:05d}.jsonl"
        p.write_text("".join(json.dumps({"t": t, "outcome": "NOT_QUALIFIED"}, sort_keys=True, separators=(",", ":")) + "\n"
                             for t in ids[k:k + size]), encoding="utf-8", newline="\n")
        paths.append(p)
    return paths


IDS = [f"ac02-{i:04d}" for i in range(7)]


def test_strategy_ledger_is_the_exact_chunk_concatenation_in_population_order(tmp_path):
    paths = _chunks(tmp_path, IDS, 3)
    out = bp.strategy_ledger_bytes(paths, IDS)
    assert out == b"".join(p.read_bytes() for p in paths) and out.count(b"\n") == len(IDS)


@pytest.mark.parametrize("mutate,match", [
    (lambda ids: ids[:-1], "rows"),                                  # dropped line
    (lambda ids: ids + ["ac02-extra"], "rows"),                      # extra line
    (lambda ids: ids[:-1] + [ids[0]], "duplicated"),                 # duplicate id (count preserved)
    (lambda ids: ids[:-1] + ["ac02-other"], "differ"),               # same count, different id set
    (lambda ids: [ids[1], ids[0], *ids[2:]], "order"),               # same set, different order
])
def test_strategy_ledger_refuses_any_deviation_from_the_frozen_population(tmp_path, mutate, match):
    with pytest.raises(bp.PackageRefusal, match=match):
        bp.strategy_ledger_bytes(_chunks(tmp_path, mutate(list(IDS)), 3), IDS)


def test_strategy_ledger_refuses_a_blank_or_unterminated_line(tmp_path):
    p = tmp_path / "chunk_00000.jsonl"
    p.write_text('{"t":"a"}\n\n{"t":"b"}\n', encoding="utf-8")
    with pytest.raises(bp.PackageRefusal, match="newline-terminated"):
        bp.strategy_ledger_bytes([p], ["a", "b"])
    p.write_text('{"t":"a"}\n{"t":"b"}', encoding="utf-8")
    with pytest.raises(bp.PackageRefusal, match="newline-terminated"):
        bp.strategy_ledger_bytes([p], ["a", "b"])


FIDS = [f"f{i:02d}" for i in range(5)]


def _records(ids=FIDS, bad=1):
    return [{"factor_id": f, "evaluation_id": "e" + f, "status": "not_evaluable" if i < bad else "succeeded",
             **({"reason": "zero_variance_factor"} if i < bad else {"pvalue": {"p_value": 0.5}})} for i, f in enumerate(ids)]


def test_factor_ledger_is_canonical_one_line_per_record_and_round_trips():
    recs = _records()
    out = bp.factor_ledger_bytes(recs, FIDS, {"succeeded": 4, "not_evaluable": 1})
    assert [json.loads(ln) for ln in out.splitlines()] == recs and out.endswith(b"\n")
    assert out.splitlines()[0] == json.dumps(recs[0], sort_keys=True, separators=(",", ":")).encode("utf-8")


@pytest.mark.parametrize("make,match", [
    (lambda: _records()[:-1], "records"),                                              # dropped record
    (lambda: _records() + [{"factor_id": "fx", "status": "succeeded"}], "records"),    # extra record
    (lambda: [*_records()[:-1], _records()[0]], "duplicated"),                         # duplicate id
    (lambda: [*_records()[:-1], {"factor_id": "other", "status": "succeeded"}], "differ"),
    (lambda: list(reversed(_records())), "order"),
])
def test_factor_ledger_refuses_any_deviation_from_the_frozen_population(make, match):
    with pytest.raises(bp.PackageRefusal, match=match):
        bp.factor_ledger_bytes(make(), FIDS, {"succeeded": 4, "not_evaluable": 1})


def test_factor_ledger_refuses_an_unexpected_status_mix():
    with pytest.raises(bp.PackageRefusal, match="statuses"):
        bp.factor_ledger_bytes(_records(bad=2), FIDS, {"succeeded": 4, "not_evaluable": 1})
    assert bp.factor_ledger_bytes(_records(bad=2), FIDS, None)          # no expectation supplied: only identity checks apply


def test_result01_counts_are_the_frozen_population_sizes():
    assert bp.RESULT01_COUNTS == {"strategy_trials": 9400, "factors": 1075, "factor_status": {"succeeded": 1065, "not_evaluable": 10}}
