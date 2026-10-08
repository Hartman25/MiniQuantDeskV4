"""Deterministic derivation, validation and emission of the 200-row external-idea disposition ledger.

Inputs: the hash-verified workbook (via intake.freeze) and the reviewer-authored classification in
dispositions_data.py. Feasibility, primary disposition, native readiness and population tier are DERIVED here,
so no result, return claim, catalog priority or readiness label can influence them. Nothing registers a trial,
reads prices, or calls a provider.

    python disposition.py <workbook.xlsx> --out <dir>
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import io
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import intake  # noqa: E402
from dispositions_data import ALIASES, TABLE  # noqa: E402

SCHEMA = "external_idea_disposition_ledger_v1"
LEDGER_NAME = "external_idea_disposition_ledger_v1.json"
CSV_NAME = "external_idea_disposition_ledger_v1.csv"

BLOCKERS = {
    "F": "NEEDS_FUTURE_ASSET_SUPPORT",
    "L": "NEEDS_ML_OR_ALT_DATA_FRAMEWORK",
    "D": "NEEDS_ADDITIONAL_AUTHORITATIVE_DATA",
    "P": "NEEDS_MULTI_SYMBOL_PORTFOLIO_ENGINE",
    "S": "NEEDS_SHORT_OR_BORROW_AUTHORITY",
    "X": "NEEDS_NEW_EXECUTION_POLICY",
    "U": "INSUFFICIENTLY_SPECIFIED",
}
PRECEDENCE = "FLDPSXU"
SHAPE = "FLDPSX"
M1_READY = "M1_READY_SINGLE_SYMBOL_LONG_FLAT_DAILY_OHLCV"
NOVELTY = {
    "EXACT": "EXACT_DUPLICATE", "PARAM": "PARAMETER_VARIANT", "MIRROR": "MIRROR", "COMPL": "COMPLEMENT",
    "COMP": "COMPOSITE_OF_EXISTING", "SEM": "SEMANTIC_VARIANT", "NEW": "GENUINELY_NEW", "UNK": "UNKNOWN_NEEDS_REVIEW",
}
SHAPE_DISPOSITION = {
    "F": "FUTURE_MULTI_ASSET", "L": "FUTURE_ML_OR_FACTORY", "D": "NEEDS_NON_OHLCV_DATA",
    "P": "NEEDS_MULTI_SYMBOL_ENGINE", "S": "NEEDS_SHORT_OR_HEDGE", "X": "NEEDS_NEW_EXECUTION_POLICY",
}
PRIMARY_DISPOSITIONS = frozenset(SHAPE_DISPOSITION.values()) | {
    "DUPLICATE_OF_REGISTERED", "DUPLICATE_WITHIN_CATALOG", "PARAMETER_VARIANT_OF_TESTED", "ADJACENT_TO_REJECTED",
    "INSUFFICIENTLY_SPECIFIED", "M1_EQUITY_ETF_HYPOTHESIS",
}
NATIVE_READINESS = frozenset({
    "EXISTING_EXACT_ENGINE", "EXISTING_ENGINE_BEHAVIOR_IDENTICAL_CONFIG", "NEW_ENGINE_NEEDED",
    "MISSING_AUTHORITATIVE_DATA", "REQUIRES_NEW_POLICY", "NOT_EXECUTABLE_IN_M1", "INSUFFICIENT_SPEC",
})
# Native direction -> the blocker it implies when no explicit blocker already covers it. Only LONG_ONLY and LONG_FLAT
# map to None: long or long/flat is the one direction Paper supports (short opens are blocked). A symmetrical rule such
# as LONG_SHORT is not M1-ready until a separately specified, identity-distinct long/flat form exists; none is specified.
DIRECTION_REQUIRES = {
    "LONG_ONLY": None, "LONG_FLAT": None,
    "LONG_SHORT": "S", "SHORT_ONLY": "S", "MARKET_NEUTRAL": "S", "HEDGED": "S", "LONG_HEDGE": "S",
    "LONG_ROTATION": "P",
    "SHORT_VOL": "F", "LONG_VOL": "F", "BEARISH_OPTIONS": "F", "BULLISH_OPTIONS": "F", "LONG_OPTIONS": "F",
    "MODEL": "L", "OVERLAY": "U", "UNKNOWN": "U",
}
DIRECTIONS = frozenset(DIRECTION_REQUIRES)
SUPPORTED_DIRECTIONS = frozenset(d for d, need in DIRECTION_REQUIRES.items() if need is None)
RELATION_KINDS = frozenset({"dup", "param", "sem", "adj", "comp", "mirror", "compo", "pair", "neighbor"})
BASE_FAMILIES = frozenset(
    [f"S{i:02d}" for i in range(1, 15)] + [f"SH{i:02d}" for i in range(1, 14)] + [f"LS{i:02d}" for i in range(1, 5)]
    + ["DISCOVERY_01", "SHORT_01", "SHORT_WAVE_02", "WAVE06_LIQ01", "WAVE06_VOL01"])
EXT_RE = re.compile(r"^EXT-\d{3}$")
ID_RE = re.compile(r"^EXT-(\d{3})$")


class DispositionError(Exception):
    pass


def parse_table(table: str = TABLE) -> list[dict]:
    rows = []
    for line in table.strip().splitlines():
        parts = line.split("|")
        if len(parts) != 8:
            raise DispositionError(f"malformed line (need 8 fields): {line[:60]!r}")
        eid, blk, nov, direction, mech, rel, missing, reason = (p.strip() for p in parts)
        relations = []
        if rel != "-":
            for item in rel.split(";"):
                kind, _, target = item.partition(":")
                relations.append({"kind": kind, "target": ALIASES.get(target, target)})
        rows.append({
            "ext_id": eid, "blockers": "" if blk == "-" else blk, "novelty_code": nov, "direction": direction,
            "mechanism": mech, "relations": relations,
            "missing": [] if missing == "-" else [m.strip() for m in missing.split(";")], "reason": reason,
        })
    return rows


def _native_ids() -> frozenset[str]:
    return frozenset(ALIASES.values())


def validate(rows: list[dict]) -> None:
    if [r["ext_id"] for r in rows] != [f"EXT-{i:03d}" for i in range(1, intake.EXPECTED_ROWS + 1)]:
        raise DispositionError("classification must cover every catalog id exactly once, in order")
    ids = {r["ext_id"] for r in rows}
    native = _native_ids()
    for r in rows:
        i = r["ext_id"]
        if r["novelty_code"] not in NOVELTY:
            raise DispositionError(f"{i}: unknown novelty {r['novelty_code']!r}")
        if r["direction"] not in DIRECTIONS:
            raise DispositionError(f"{i}: unknown direction {r['direction']!r}")
        if set(r["blockers"]) - set(BLOCKERS) or len(set(r["blockers"])) != len(r["blockers"]):
            raise DispositionError(f"{i}: bad blockers {r['blockers']!r}")
        if not r["mechanism"] or not r["reason"]:
            raise DispositionError(f"{i}: mechanism and reason are required")
        kinds = {x["kind"] for x in r["relations"]}
        if not kinds <= RELATION_KINDS:
            raise DispositionError(f"{i}: unknown relation kind {kinds - RELATION_KINDS}")
        for x in r["relations"]:
            t = x["target"]
            ok = t in native or t in BASE_FAMILIES or (EXT_RE.match(t) and t in ids and t != i)
            if not ok:
                raise DispositionError(f"{i}: relation target {t!r} is not a known identity")
        nov = r["novelty_code"]
        if nov == "EXACT" and "dup" not in kinds:
            raise DispositionError(f"{i}: EXACT needs a dup relation")
        if nov == "PARAM" and "param" not in kinds:
            raise DispositionError(f"{i}: PARAM needs a param relation")
        if nov in ("SEM", "COMP", "COMPL", "MIRROR") and not (kinds & {"sem", "adj", "comp", "mirror", "compo"}):
            raise DispositionError(f"{i}: {nov} needs a sem/adj/comp/mirror/compo relation")
        if nov == "COMPL" and "comp" not in kinds:
            raise DispositionError(f"{i}: COMPL needs a comp relation")
        if nov == "NEW" and kinds & {"dup", "param", "sem", "adj", "comp", "mirror", "compo"}:
            raise DispositionError(f"{i}: GENUINELY_NEW cannot carry a duplicate/variant/adjacent relation")
        if "U" in r["blockers"] and not r["missing"]:
            raise DispositionError(f"{i}: INSUFFICIENTLY_SPECIFIED must list exactly what is missing")
        if not r["blockers"] and r["missing"]:
            raise DispositionError(f"{i}: a row with no blocker must not list missing elements")


def effective_blockers(r: dict) -> str:
    """Authored blockers plus the blocker implied by an unsupported native direction, in precedence order."""
    implied = DIRECTION_REQUIRES[r["direction"]]
    have = set(r["blockers"]) | ({implied} if implied else set())
    return "".join(c for c in PRECEDENCE if c in have)


def _is_native_dup(r: dict) -> bool:
    return any(x["kind"] == "dup" and x["target"] in _native_ids() for x in r["relations"])


def derive(r: dict) -> dict:
    blockers = effective_blockers(r)
    nov = r["novelty_code"]
    shape = [c for c in PRECEDENCE if c in blockers and c in SHAPE]
    feasibility = BLOCKERS[next((c for c in PRECEDENCE if c in blockers), "")] if blockers else M1_READY
    labels = []
    if nov == "UNK":
        labels.append("INSUFFICIENTLY_SPECIFIED")
    if nov == "EXACT":
        labels.append("DUPLICATE_OF_REGISTERED" if _is_native_dup(r) else "DUPLICATE_WITHIN_CATALOG")
    if nov == "PARAM":
        labels.append("PARAMETER_VARIANT_OF_TESTED")
    labels += [SHAPE_DISPOSITION[c] for c in shape]
    if nov in ("SEM", "COMP", "COMPL", "MIRROR"):
        labels.append("ADJACENT_TO_REJECTED")
    if "U" in blockers:
        labels.append("INSUFFICIENTLY_SPECIFIED")
    if not labels:
        labels.append("M1_EQUITY_ETF_HYPOTHESIS")
    labels = list(dict.fromkeys(labels))
    primary = labels[0]
    if _is_native_dup(r):
        readiness = "EXISTING_EXACT_ENGINE"
    elif shape:
        first = shape[0]
        readiness = ("NOT_EXECUTABLE_IN_M1" if first in "FLP" else
                     "MISSING_AUTHORITATIVE_DATA" if first == "D" else "REQUIRES_NEW_POLICY")
    elif "U" in blockers or nov == "UNK":
        readiness = "INSUFFICIENT_SPEC"
    else:
        readiness = "NEW_ENGINE_NEEDED"
    if primary == "M1_EQUITY_ETF_HYPOTHESIS":
        tier = "A"
    elif nov == "NEW" and not shape and blockers == "U" and r["direction"] != "OVERLAY":
        tier = "B"
    elif nov in ("SEM", "COMP") and not blockers:
        tier = "RESERVE_ADJACENT_COMPLETE"
    else:
        tier = None
    return {
        "feasibility": feasibility, "effective_blockers": blockers, "novelty": NOVELTY[nov], "primary_disposition": primary,
        "secondary_dispositions": labels[1:], "native_readiness": readiness, "population_tier": tier,
        "blocker_labels": [BLOCKERS[c] for c in PRECEDENCE if c in blockers],
    }


def row_sha256(original: dict) -> str:
    return hashlib.sha256(json.dumps(original, ensure_ascii=False, separators=(",", ":")).encode("utf-8")).hexdigest()


def build_ledger(workbook_bytes: bytes, *, expected_sha256: str = intake.EXPECTED_SHA256) -> dict:
    frozen = intake.freeze(workbook_bytes, expected_sha256=expected_sha256)
    classified = parse_table()
    validate(classified)
    by_id = {r["ext_id"]: r for r in frozen["rows"]}
    out_rows = []
    for c in classified:
        orig = by_id[c["ext_id"]]["original"]
        d = derive(c)
        out_rows.append({
            "ext_id": c["ext_id"], "strategy_name": orig["Strategy_Name"], "asset_class": orig["Asset_Class"],
            "source_spec_status": orig["Spec_Status"], "source_row_sha256": row_sha256(orig),
            "source_direction": orig["Native_Direction"], "source_test_long": orig["Test_Long"],
            "source_test_short": orig["Test_Short"], "source_timeframe": orig["Signal_Timeframe"],
            "blockers": c["blockers"], "effective_blockers": d["effective_blockers"],
            "direction_supported": c["direction"] in SUPPORTED_DIRECTIONS,
            "feasibility": d["feasibility"], "blocker_labels": d["blocker_labels"],
            "novelty": d["novelty"], "direction": c["direction"], "mechanism": c["mechanism"],
            "relations": c["relations"], "missing": c["missing"], "reason": c["reason"],
            "primary_disposition": d["primary_disposition"], "secondary_dispositions": d["secondary_dispositions"],
            "native_readiness": d["native_readiness"], "population_tier": d["population_tier"],
            "calendar_bound": c["mechanism"].startswith("CAL_"),
        })
    counts = {}
    for key in ("primary_disposition", "feasibility", "novelty", "native_readiness"):
        tally = {}
        for r in out_rows:
            tally[r[key]] = tally.get(r[key], 0) + 1
        counts[key] = dict(sorted(tally.items()))
    anywhere = {}
    for r in out_rows:
        for lab in [r["primary_disposition"], *r["secondary_dispositions"]]:
            anywhere[lab] = anywhere.get(lab, 0) + 1
    counts["any_applicable_disposition"] = dict(sorted(anywhere.items()))
    tiers = {t: [r["ext_id"] for r in out_rows if r["population_tier"] == t]
             for t in ("A", "B", "RESERVE_ADJACENT_COMPLETE")}
    return {
        "schema": SCHEMA, "status": intake.STATUS, "trial_registered": False, "economic_attempt": False,
        "source_sha256": frozen["source_sha256"], "row_count": len(out_rows), "counts": counts,
        "population": tiers, "rows": out_rows,
    }


def ledger_json(ledger: dict) -> bytes:
    return (json.dumps(ledger, ensure_ascii=False, indent=1) + "\n").encode("utf-8")


def ledger_csv(ledger: dict) -> bytes:
    buf = io.StringIO()
    w = csv.writer(buf, lineterminator="\n")
    w.writerow(["ext_id", "strategy_name", "asset_class", "feasibility", "novelty", "primary_disposition",
                "secondary_dispositions", "native_readiness", "population_tier", "direction", "direction_supported",
                "effective_blockers", "mechanism",
                "relations", "missing", "reason"])
    for r in ledger["rows"]:
        w.writerow([r["ext_id"], r["strategy_name"], r["asset_class"], r["feasibility"], r["novelty"],
                    r["primary_disposition"], ";".join(r["secondary_dispositions"]), r["native_readiness"],
                    r["population_tier"] or "", r["direction"], r["direction_supported"],
                    r["effective_blockers"], r["mechanism"],
                    ";".join(f"{x['kind']}:{x['target']}" for x in r["relations"]), ";".join(r["missing"]),
                    r["reason"]])
    return buf.getvalue().encode("utf-8")


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("workbook", type=Path)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args(argv)
    try:
        ledger = build_ledger(args.workbook.read_bytes())
    except (intake.IntakeError, DispositionError, OSError) as exc:
        print(f"REFUSED: {exc}", file=sys.stderr)
        return 2
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / LEDGER_NAME).write_bytes(ledger_json(ledger))
    (args.out / CSV_NAME).write_bytes(ledger_csv(ledger))
    print(json.dumps(ledger["counts"]["primary_disposition"], indent=1))
    print("population:", {k: v for k, v in ledger["population"].items()})
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
