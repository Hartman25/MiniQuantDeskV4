"""The 200-row disposition ledger: regenerates byte-identically from the hash-verified workbook, one disposition per
row, derivation precedence, validator negative controls, an independent keyword audit of the reviewer's blockers
against the workbook's own fields, and the bounded-population invariants. Nothing here reads prices or a provider."""

from __future__ import annotations

import copy
import hashlib
import json
import re
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import disposition as d  # noqa: E402
import intake  # noqa: E402
import dispositions_data as data  # noqa: E402

ROOT = HERE.parents[2]
INTAKE_DIR = ROOT / "docs/research/intake"
WORKBOOK = INTAKE_DIR / "MQD_External_Strategy_Idea_Catalog_2026-10-07.xlsx"
COMMITTED_JSON = INTAKE_DIR / d.LEDGER_NAME
COMMITTED_CSV = INTAKE_DIR / d.CSV_NAME
BYTES = WORKBOOK.read_bytes()
FROZEN = intake.freeze(BYTES)
LEDGER = d.build_ledger(BYTES)
ROWS = {r["ext_id"]: r for r in LEDGER["rows"]}
ORIG = {r["ext_id"]: r["original"] for r in FROZEN["rows"]}

# Audit exceptions: the keyword audit flags these; each is a reviewed, reasoned exception.
AUDIT_EXCEPTIONS = {
    "EXT-037": "T-bill is the off-state cash proxy, not a signal input; listed as an operator-declarable missing definition",
    "EXT-053": "Cross-Asset label for a two-asset ETF pair; implementable with US-listed ETFs, so not a futures dependency",
}
EXPECTED_PRIMARY = {
    "DUPLICATE_OF_REGISTERED": 2, "DUPLICATE_WITHIN_CATALOG": 2,
    "FUTURE_ML_OR_FACTORY": 9, "FUTURE_MULTI_ASSET": 56, "INSUFFICIENTLY_SPECIFIED": 5,
    "M1_EQUITY_ETF_HYPOTHESIS": 2, "NEEDS_MULTI_SYMBOL_ENGINE": 28, "NEEDS_NEW_EXECUTION_POLICY": 1,
    "NEEDS_NON_OHLCV_DATA": 62, "NEEDS_SHORT_OR_HEDGE": 29, "PARAMETER_VARIANT_OF_TESTED": 4,
}
# Independent statement of the supported-direction policy (kept separate from disposition.DIRECTION_REQUIRES on purpose).
SUPPORTED = {"LONG_ONLY", "LONG_FLAT"}


def crow(**kw):
    base = {"ext_id": "EXT-001", "blockers": "", "novelty_code": "NEW", "direction": "LONG_FLAT", "mechanism": "M",
            "relations": [], "missing": [], "reason": "r"}
    base.update(kw)
    return base


# ---- regeneration, cardinality, identity ------------------------------------------------------------------

def test_committed_ledgers_regenerate_byte_identically_from_the_verified_workbook():
    assert COMMITTED_JSON.read_bytes() == d.ledger_json(LEDGER)
    assert COMMITTED_CSV.read_bytes() == d.ledger_csv(LEDGER)


def test_exactly_200_rows_one_primary_disposition_each_with_the_catalog_ids():
    assert LEDGER["row_count"] == len(LEDGER["rows"]) == 200
    assert [r["ext_id"] for r in LEDGER["rows"]] == [f"EXT-{i:03d}" for i in range(1, 201)]
    assert [r["ext_id"] for r in LEDGER["rows"]] == [r["ext_id"] for r in FROZEN["rows"]]
    for r in LEDGER["rows"]:
        assert r["primary_disposition"] in d.PRIMARY_DISPOSITIONS
        assert r["native_readiness"] in d.NATIVE_READINESS
        assert r["primary_disposition"] not in r["secondary_dispositions"]
    assert sum(LEDGER["counts"]["primary_disposition"].values()) == 200


def test_each_row_is_bound_to_the_exact_original_workbook_row_and_keeps_source_direction_verbatim():
    for eid, r in ROWS.items():
        o = ORIG[eid]
        independent = hashlib.sha256(json.dumps(o, ensure_ascii=False, separators=(",", ":")).encode("utf-8")).hexdigest()
        assert r["source_row_sha256"] == independent
        assert (r["source_direction"], r["source_test_long"], r["source_test_short"]) == (
            o["Native_Direction"], o["Test_Long"], o["Test_Short"])
        assert r["strategy_name"] == o["Strategy_Name"]


def test_a_row_hash_is_pinned_to_a_literal_so_the_binding_cannot_drift_with_its_own_implementation():
    assert ROWS["EXT-032"]["source_row_sha256"] == "72e56800d20466bc011655c4479dc43028a69a03758d1b6a240a7a94ae0793e9"


def test_primary_disposition_counts_are_pinned():
    assert LEDGER["counts"]["primary_disposition"] == EXPECTED_PRIMARY


def test_ledger_states_no_trial_or_attempt_and_uses_no_catalog_priority_or_readiness_as_input():
    assert LEDGER["trial_registered"] is False and LEDGER["economic_attempt"] is False
    for banned in ("MQD_Priority", "Implementation_Readiness", "Source_Fidelity", "Known_Bias_Risks"):
        assert banned not in (HERE / "disposition.py").read_text(encoding="utf-8")
        assert all(banned not in r for r in LEDGER["rows"])
    src = (HERE / "disposition.py").read_text(encoding="utf-8")
    assert not re.search(r"EXT-\d{3}", src), "derivation must not special-case any id"
    for banned in ("requests", "urllib", "socket", "subprocess", "sqlite3", "pandas", "alpaca"):
        assert not re.search(rf"^\s*(import|from)\s+{banned}\b", src, re.M), banned


def test_the_five_operator_listed_ids_are_dispositioned_by_the_same_rules_as_every_row():
    got = {i: (ROWS[i]["primary_disposition"], ROWS[i]["feasibility"], ROWS[i]["novelty"])
           for i in ("EXT-032", "EXT-024", "EXT-045", "EXT-070", "EXT-141")}
    assert got == {
        "EXT-032": ("M1_EQUITY_ETF_HYPOTHESIS", d.M1_READY, "GENUINELY_NEW"),
        "EXT-024": ("NEEDS_NON_OHLCV_DATA", "NEEDS_ADDITIONAL_AUTHORITATIVE_DATA", "GENUINELY_NEW"),
        "EXT-045": ("NEEDS_MULTI_SYMBOL_ENGINE", "NEEDS_MULTI_SYMBOL_PORTFOLIO_ENGINE", "SEMANTIC_VARIANT"),
        "EXT-070": ("NEEDS_MULTI_SYMBOL_ENGINE", "NEEDS_MULTI_SYMBOL_PORTFOLIO_ENGINE", "SEMANTIC_VARIANT"),
        "EXT-141": ("NEEDS_SHORT_OR_HEDGE", "NEEDS_SHORT_OR_BORROW_AUTHORITY", "COMPOSITE_OF_EXISTING"),
    }
    # derive() is a pure function of the classification: re-deriving from the table reproduces every row.
    for c in d.parse_table():
        assert d.derive(c)["primary_disposition"] == ROWS[c["ext_id"]]["primary_disposition"]


# ---- derivation precedence (synthetic rows, so the rule is tested independent of the real table) ----------

@pytest.mark.parametrize("blockers,feas,primary", [
    ("FLDPSXU", "NEEDS_FUTURE_ASSET_SUPPORT", "FUTURE_MULTI_ASSET"),
    ("LDPSXU", "NEEDS_ML_OR_ALT_DATA_FRAMEWORK", "FUTURE_ML_OR_FACTORY"),
    ("DPSXU", "NEEDS_ADDITIONAL_AUTHORITATIVE_DATA", "NEEDS_NON_OHLCV_DATA"),
    ("PSXU", "NEEDS_MULTI_SYMBOL_PORTFOLIO_ENGINE", "NEEDS_MULTI_SYMBOL_ENGINE"),
    ("SXU", "NEEDS_SHORT_OR_BORROW_AUTHORITY", "NEEDS_SHORT_OR_HEDGE"),
    ("XU", "NEEDS_NEW_EXECUTION_POLICY", "NEEDS_NEW_EXECUTION_POLICY"),
    ("U", "INSUFFICIENTLY_SPECIFIED", "INSUFFICIENTLY_SPECIFIED"),
    ("", d.M1_READY, "M1_EQUITY_ETF_HYPOTHESIS"),
])
def test_shape_precedence_and_new_row_disposition(blockers, feas, primary):
    out = d.derive(crow(blockers=blockers, missing=["x"] if blockers else []))
    assert out["feasibility"] == feas and out["primary_disposition"] == primary


def test_duplicates_and_variants_keep_their_identity_whatever_the_shape():
    dup = d.derive(crow(blockers="DU", novelty_code="EXACT", relations=[{"kind": "dup", "target": "EXT-002"}]))
    assert dup["primary_disposition"] == "DUPLICATE_WITHIN_CATALOG" and "NEEDS_NON_OHLCV_DATA" in dup["secondary_dispositions"]
    reg = d.derive(crow(novelty_code="EXACT", relations=[{"kind": "dup", "target": data.ALIASES["TOM"]}]))
    assert reg["primary_disposition"] == "DUPLICATE_OF_REGISTERED" and reg["native_readiness"] == "EXISTING_EXACT_ENGINE"
    par = d.derive(crow(blockers="FU", novelty_code="PARAM", relations=[{"kind": "param", "target": "S03"}]))
    assert par["primary_disposition"] == "PARAMETER_VARIANT_OF_TESTED"
    sem = d.derive(crow(blockers="DU", novelty_code="SEM", relations=[{"kind": "adj", "target": "S03"}]))
    assert sem["primary_disposition"] == "NEEDS_NON_OHLCV_DATA"
    assert sem["secondary_dispositions"] == ["ADJACENT_TO_REJECTED", "INSUFFICIENTLY_SPECIFIED"]
    unk = d.derive(crow(blockers="F", novelty_code="UNK"))
    assert unk["primary_disposition"] == "INSUFFICIENTLY_SPECIFIED"


def test_adjacent_to_rejected_is_never_admitted_even_when_fully_specified():
    out = d.derive(crow(novelty_code="SEM", relations=[{"kind": "adj", "target": "S03"}]))
    assert out["primary_disposition"] == "ADJACENT_TO_REJECTED" and out["population_tier"] == "RESERVE_ADJACENT_COMPLETE"
    both = d.derive(crow(novelty_code="SEM", direction="LONG_SHORT", relations=[{"kind": "adj", "target": "S03"}]))
    assert both["population_tier"] is None and both["feasibility"] == "NEEDS_SHORT_OR_BORROW_AUTHORITY"


# ---- directional feasibility: M1-ready needs a supported long/flat direction as well as no blocker ----------------

UNSUPPORTED = sorted(set(d.DIRECTION_REQUIRES) - SUPPORTED)


def test_the_supported_directions_are_exactly_long_only_and_long_flat():
    assert set(d.SUPPORTED_DIRECTIONS) == SUPPORTED
    assert {k for k, v in d.DIRECTION_REQUIRES.items() if v is None} == SUPPORTED
    assert {"LONG_SHORT", "SHORT_ONLY", "HEDGED", "LONG_HEDGE", "MARKET_NEUTRAL"} <= set(UNSUPPORTED)


@pytest.mark.parametrize("direction", sorted(SUPPORTED))
def test_an_unblocked_new_row_with_a_supported_direction_is_m1_ready_and_admitted(direction):
    out = d.derive(crow(direction=direction))
    assert out["feasibility"] == d.M1_READY and out["primary_disposition"] == "M1_EQUITY_ETF_HYPOTHESIS"
    assert out["population_tier"] == "A" and out["effective_blockers"] == ""


@pytest.mark.parametrize("direction", UNSUPPORTED)
def test_an_unblocked_row_with_an_unsupported_direction_is_never_m1_ready_or_admitted(direction):
    for nov, rel in (("NEW", []), ("SEM", [{"kind": "adj", "target": "S03"}])):
        out = d.derive(crow(direction=direction, novelty_code=nov, relations=rel))
        assert out["feasibility"] != d.M1_READY, (direction, nov)
        assert out["primary_disposition"] != "M1_EQUITY_ETF_HYPOTHESIS", (direction, nov)
        assert out["population_tier"] in (None, "B") and out["effective_blockers"] != ""
    # the implied blocker is the one the direction requires
    assert d.DIRECTION_REQUIRES[direction] in d.derive(crow(direction=direction))["effective_blockers"]


@pytest.mark.parametrize("direction,blocker", [("LONG_SHORT", "S"), ("SHORT_ONLY", "S"), ("HEDGED", "S"),
                                               ("LONG_HEDGE", "S"), ("MARKET_NEUTRAL", "S"), ("LONG_ROTATION", "P"),
                                               ("MODEL", "L"), ("BEARISH_OPTIONS", "F")])
def test_named_unsupported_directions_imply_the_documented_blocker(direction, blocker):
    out = d.derive(crow(direction=direction))
    assert blocker in out["effective_blockers"] and out["feasibility"] == d.BLOCKERS[blocker]


def test_an_unknown_direction_is_refused_by_the_validator():
    rows = copy.deepcopy(table_rows())
    rows[30]["direction"] = "SIDEWAYS"
    with pytest.raises(d.DispositionError):
        d.validate(rows)


def test_ledger_m1_ready_rows_all_have_a_supported_direction_and_match_an_independent_oracle():
    for c in d.parse_table():
        r = ROWS[c["ext_id"]]
        oracle_ready = (c["blockers"] == "") and c["direction"] in SUPPORTED
        assert (r["feasibility"] == d.M1_READY) == oracle_ready, c["ext_id"]
        assert r["direction_supported"] == (c["direction"] in SUPPORTED)
        if r["feasibility"] == d.M1_READY:
            assert r["direction"] in SUPPORTED and r["effective_blockers"] == ""
    assert sorted(e for e, r in ROWS.items() if r["feasibility"] == d.M1_READY) == [
        "EXT-032", "EXT-051", "EXT-169", "EXT-170"]
    for tier in ("A", "B"):
        for eid in LEDGER["population"][tier]:
            assert ROWS[eid]["direction"] in SUPPORTED


def test_ext_179_is_long_short_and_is_not_m1_ready_or_in_any_tier():
    r = ROWS["EXT-179"]
    assert r["direction"] == "LONG_SHORT" and ORIG["EXT-179"]["Native_Direction"] == "Both"
    assert r["blockers"] == "" and r["effective_blockers"] == "S" and r["direction_supported"] is False
    assert r["feasibility"] == "NEEDS_SHORT_OR_BORROW_AUTHORITY" and r["native_readiness"] == "REQUIRES_NEW_POLICY"
    assert r["primary_disposition"] == "NEEDS_SHORT_OR_HEDGE" and "ADJACENT_TO_REJECTED" in r["secondary_dispositions"]
    assert r["population_tier"] is None and all("EXT-179" not in t for t in LEDGER["population"].values())


def test_adjacent_to_rejected_remains_visible_as_an_applicable_label_after_the_direction_rule():
    anywhere = LEDGER["counts"]["any_applicable_disposition"]
    sem_like = sum(1 for r in ROWS.values() if r["novelty"] in ("SEMANTIC_VARIANT", "COMPOSITE_OF_EXISTING", "COMPLEMENT", "MIRROR"))
    assert anywhere["ADJACENT_TO_REJECTED"] == sem_like == 88
    assert LEDGER["counts"]["primary_disposition"].get("ADJACENT_TO_REJECTED", 0) == 0
    assert LEDGER["counts"]["feasibility"][d.M1_READY] == 4


def test_overlay_is_not_a_standalone_population_candidate():
    out = d.derive(crow(blockers="U", direction="OVERLAY", missing=["base strategy"]))
    assert out["population_tier"] is None


# ---- validator negative controls --------------------------------------------------------------------------

def table_rows():
    return d.parse_table()


def mutated(idx, **kw):
    rows = copy.deepcopy(table_rows())
    rows[idx].update(kw)
    return rows


@pytest.mark.parametrize("name,rows", [
    ("missing row", table_rows()[:-1]),
    ("out of order", table_rows()[1:] + table_rows()[:1]),
    ("unknown novelty", mutated(5, novelty_code="MAYBE")),
    ("unknown direction", mutated(5, direction="SIDEWAYS")),
    ("bad blocker", mutated(5, blockers="Z")),
    ("repeated blocker", mutated(5, blockers="DD", missing=["x"])),
    ("unknown relation target", mutated(7, relations=[{"kind": "sem", "target": "S99"}])),
    ("self relation", mutated(7, relations=[{"kind": "pair", "target": "EXT-008"}])),
    ("unknown relation kind", mutated(7, relations=[{"kind": "friend", "target": "S03"}])),
    ("EXACT without dup", mutated(50, relations=[])),
    ("NEW with adjacent relation", mutated(0, relations=[{"kind": "adj", "target": "S03"}])),
    ("SEM without any variant relation", mutated(7, relations=[])),
    ("INSUFFICIENT without listing what is missing", mutated(7, missing=[])),
    ("blocker-free row listing missing", mutated(31, missing=["x"])),
    ("empty reason", mutated(0, reason="")),
])
def test_validator_refuses_inconsistent_classifications(name, rows):
    with pytest.raises(d.DispositionError):
        d.validate(rows)


def test_malformed_line_and_wrong_workbook_are_refused():
    import test_external_idea_intake as synth
    with pytest.raises(d.DispositionError):
        d.parse_table("EXT-001|a|b")
    with pytest.raises(intake.IntakeError):
        d.build_ledger(BYTES[:-1])           # hash first: one byte short never reaches classification
    # a structurally perfect 200-row workbook with different bytes must still be refused by the pin
    with pytest.raises(intake.IntakeError, match="sha256 mismatch"):
        d.build_ledger(synth.build_xlsx(synth.good_sheets()))


# ---- independent audit of the reviewer's blockers against the workbook's own fields -----------------------

def audit_row(eid, b):
    """Findings for one row given a blocker string (authored blockers, so the audit is independent of derivation)."""
    o = ORIG[eid]
    out = []
    req, tf, fam = o["Required_Data"].lower(), o["Signal_Timeframe"], o["Canonical_Family"]
    if o["Asset_Class"] in ("Options", "Futures", "FX", "Crypto", "Futures/FX", "Cross-Asset") and "F" not in b:
        out.append((eid, "asset class implies a future asset"))
    kws = [k for k in ("fundamental", "earnings", "vix", "yield", "t-bill", "valuation", "factor returns",
                       "sentiment", "news", "option chain", "funding", "shares outstanding", "open interest",
                       "rates", "constituent") if k in req]
    if kws and not (set(b) & set("FLD")):
        out.append((eid, f"data keywords {kws} need F/L/D"))
    if tf == "Intraday" and not (set(b) & set("FLD")):
        out.append((eid, "intraday timeframe needs D/F/L"))
    if fam.startswith("ML") and "L" not in b:
        out.append((eid, "ML family needs L"))
    if o["Native_Direction"].lower().startswith("market-neutral") and not (set(b) & set("PS")):
        out.append((eid, "market-neutral needs P/S"))
    if "pair" in fam.lower() and "P" not in b:
        out.append((eid, "pairs family needs P"))
    return out


def audit_findings():
    return [f for eid in ORIG for f in audit_row(eid, ROWS[eid]["blockers"])]


def test_keyword_audit_of_blockers_has_no_unexplained_finding():
    unexplained = [f for f in audit_findings() if f[0] not in AUDIT_EXCEPTIONS]
    assert unexplained == []
    assert {f[0] for f in audit_findings()} == set(AUDIT_EXCEPTIONS), "stale exception: every exception must still fire"


def test_audit_would_catch_a_dropped_blocker_and_the_direction_rule_is_a_second_line_of_defence():
    assert audit_row("EXT-024", "D") == []
    assert [f[1] for f in audit_row("EXT-024", "")] and "vix" in ORIG["EXT-024"]["Required_Data"].lower()
    rows = copy.deepcopy(table_rows())
    i = next(k for k, r in enumerate(rows) if r["ext_id"] == "EXT-024")
    rows[i]["blockers"] = ""                        # drop the VIX data blocker: the LONG_SHORT direction still blocks
    out = d.derive(rows[i])
    assert out["primary_disposition"] == "NEEDS_SHORT_OR_HEDGE" and out["feasibility"] != d.M1_READY


# ---- within-catalog duplicates and relationship preservation ----------------------------------------------

def test_identical_normalized_rules_at_one_timeframe_are_flagged_or_explicitly_exempt():
    groups = {}
    for eid, o in ORIG.items():
        key = tuple(o[k] for k in ("Long_Logic", "Short_Logic", "Exit_or_Rebalance", "Key_Parameters", "Asset_Class",
                                   "Signal_Timeframe"))
        groups.setdefault(key, []).append(eid)
    exempt = {"EXT-105"}      # same text as EXT-102 only because the catalog under-specifies two named structures
    for ids in (g for g in groups.values() if len(g) > 1):
        for later in ids[1:]:
            if later in exempt:
                assert ROWS[later]["novelty"] == "GENUINELY_NEW"
            else:
                assert ROWS[later]["novelty"] == "EXACT_DUPLICATE"
                assert any(x["kind"] == "dup" and x["target"] == ids[0] for x in ROWS[later]["relations"])


def test_every_delivery_intraday_pair_and_mirror_link_resolves_to_a_real_row():
    for r in LEDGER["rows"]:
        for x in r["relations"]:
            if x["target"].startswith("EXT-"):
                assert x["target"] in ROWS and x["target"] != r["ext_id"]
    assert any(x["kind"] == "comp" for r in LEDGER["rows"] for x in r["relations"])


# ---- bounded population ------------------------------------------------------------------------------------

def test_population_is_exactly_the_derived_tiers_and_is_selected_without_any_economic_input():
    assert LEDGER["population"] == {"A": ["EXT-032", "EXT-169"], "B": ["EXT-037", "EXT-044"],
                                    "RESERVE_ADJACENT_COMPLETE": []}
    for eid in LEDGER["population"]["A"]:
        r = ROWS[eid]
        assert r["feasibility"] == d.M1_READY and r["novelty"] == "GENUINELY_NEW" and not r["missing"]
        assert r["native_readiness"] == "NEW_ENGINE_NEEDED" and r["calendar_bound"] is True
    for eid in LEDGER["population"]["B"]:
        r = ROWS[eid]
        assert r["blockers"] == "U" and r["missing"] and r["novelty"] == "GENUINELY_NEW"
    # completeness: every GENUINELY_NEW, shape-clean, fully specified row is in tier A (nothing silently dropped)
    shape_clean_new = [e for e, r in ROWS.items()
                       if r["novelty"] == "GENUINELY_NEW" and not set(r["effective_blockers"]) & set("FLDPSXU")]
    assert sorted(shape_clean_new) == LEDGER["population"]["A"]
    # no duplicate, variant, adjacent or blocked row is ever in a proposed tier A/B
    for tier in ("A", "B"):
        for eid in LEDGER["population"][tier]:
            assert ROWS[eid]["primary_disposition"] in ("M1_EQUITY_ETF_HYPOTHESIS", "INSUFFICIENTLY_SPECIFIED")


def test_registered_duplicates_map_to_real_native_engines_and_catalog_order_breaks_no_tie_by_score():
    for eid, native in (("EXT-051", "turn_of_month_last1_first3"), ("EXT-170", "halloween_nov_apr")):
        r = ROWS[eid]
        assert r["primary_disposition"] == "DUPLICATE_OF_REGISTERED"
        assert any(x["kind"] == "dup" and x["target"] == native for x in r["relations"])
    engines = (ROOT / "core-rs/crates/mqk-strategy/src/engines")
    for native in {x["target"] for r in LEDGER["rows"] for x in r["relations"] if x["target"] in data.ALIASES.values()}:
        assert (engines / f"{native}.rs").exists(), native


def test_the_registered_intake_workbook_is_still_the_pinned_one():
    assert intake.sha256_hex(BYTES) == intake.EXPECTED_SHA256
    assert json.loads(COMMITTED_JSON.read_text(encoding="utf-8"))["source_sha256"] == intake.EXPECTED_SHA256
