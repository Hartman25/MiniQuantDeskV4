"""FactoryService intake with an AI provider: the model proposes, the deterministic stages decide and persist provenance."""

from __future__ import annotations

from pathlib import Path

from mqk_research.strategy_factory import catalog_import as ci
from mqk_research.strategy_factory.service import FactoryService
from support.factory_fixtures import HEADER, TEST_PROFILE, make_xlsx, row
from test_strategy_factory_ai_normalize import FakeProvider, proposal

REPO = Path(__file__).resolve().parents[2]


class Routing(FakeProvider):
    """Answers the golden conformance probe correctly and every other prompt with the supplied proposal."""

    def complete(self, prompt):
        self.calls += 1
        self.prompts.append(prompt)
        if "Hold SPY while its close is above the 50-day SMA" in prompt:
            return proposal(params={"window": {"value": 50, "evidence": "50-day SMA"}})
        return self.reply


def import_hard(svc, tmp_path):
    sheets = {"IDEAS": [HEADER, row("H-1", "Prose trend idea", rule="Own SPY while price exceeds its 50 day average, otherwise cash", direction="Long / flat"),
                        row("H-2", "Other prose", rule="Own SPY while price exceeds its trailing average, otherwise cash", direction="Long / flat")],
              "VIEW": [["ID", "Note"], ["H-1", "x"]], "CONTROLS": [HEADER], "SOURCES": [["SID", "Title"], ["S1", "P"]]}
    path = tmp_path / "h.xlsx"
    path.write_bytes(make_xlsx(sheets))
    svc.import_catalog(path, ci.CatalogProfile.from_json({**TEST_PROFILE.to_json(), "catalog_family": "svc_fam"}))


def test_without_a_model_prose_stays_unadmitted(tmp_path):
    svc = FactoryService(tmp_path / "f", REPO, grammar_available=True)
    import_hard(svc, tmp_path)
    out = svc.run_intake()
    assert out["ai"]["configured"] is False and "ADMITTED_GRAMMAR" not in out["result"]["disposition_counts"]


def test_a_verified_model_extraction_admits_only_what_the_source_text_proves_and_records_provenance_separately(tmp_path):
    svc = FactoryService(tmp_path / "f", REPO, grammar_available=True)
    import_hard(svc, tmp_path)
    prov = Routing(proposal(params={"window": {"value": 50, "evidence": "50 day average"}}))
    out = svc.run_intake(prov)
    ideas = {i["entry_id"]: i for i in svc.store.latest_ideas().values()}
    assert ideas["H-1"]["disposition"] == "DUPLICATE_OF_KNOWN" or ideas["H-1"]["disposition"] == "ADMITTED_NATIVE"      # 50-day SMA gate is the native trend_sma50
    assert ideas["H-2"]["disposition"] in ("NEEDS_FORMALIZATION", "REJECTED_UNDERSPECIFIED")                          # span not in H-2's text: suggestion only
    assert ideas["H-2"]["ai"]["suggestions"]["window"]["value"] == 50
    assert out["ai"]["configured"] is True and out["ai"]["conformant"] is True
    norms = svc.store.normalizations(ideas["H-1"]["intake_id"])
    assert norms and norms[0]["provider"]["model"] == "fake-1" and norms[0]["response_text"] and norms[0]["status"] == "APPLIED"
    again = svc.run_intake(prov)
    assert again["result"]["result_sha256"] == out["result"]["result_sha256"] and again["ideas_versions_added"] == 0
