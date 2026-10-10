"""Mutation proof for Strategy Factory invariants.\n\nEach mutant replaces exactly one source fragment in place, runs the named focused tests, and MUST turn them red; the\noriginal bytes are then restored and re-hashed. A surviving mutant (tests stay green), a broken fragment or a failed\nbyte-exact restore fails the run. Usage:  python scripts/factory_mutation_proof.py <set-name>\n\nFragments use \n for line breaks; they are matched against the file's own line endings.\n"""

from __future__ import annotations

import hashlib
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = "src/mqk_research/strategy_factory/"
T_CAT = "tests/test_strategy_factory_catalog_import.py"
T_INT = "tests/test_strategy_factory_intake.py"
T_AI = "tests/test_strategy_factory_ai_normalize.py"
T_ST = "tests/test_strategy_factory_store.py"
T_KN = "tests/test_strategy_factory_knowledge.py"
T_CP = "tests/test_strategy_factory_campaign.py"
T_SC = "tests/test_strategy_factory_scout.py"
T_SF = "tests/test_strategy_factory_scheduler_faults.py"
T_AU = "tests/test_strategy_factory_authority_truth.py"
T_PS = "tests/test_strategy_factory_prior_search.py"
T_LN = "tests/test_strategy_factory_native_lane.py"
T_IM = "tests/test_strategy_factory_implementation.py"
T_E2E = ["tests/test_strategy_factory_resume.py"]
RS = "../core-rs/crates/mqk-strategy/src/engines/grammar_rule_v1.rs"
CARGO = ["cargo:-p", "mqk-strategy", "--lib", "grammar_rule_v1"]

# (id, file relative to research-py, old fragment (must occur exactly once), new fragment, pytest args)
MUTANTS: dict[str, list[tuple[str, str, str, str, list[str]]]] = {
    "intake": [
        ("CI-1 subset check removed", SRC + "catalog_import.py", "if eid not in entries:\n                    raise CatalogImportError(f\"subset sheet",
         "if False:\n                    raise CatalogImportError(f\"subset sheet", [T_CAT]),
        ("CI-2 entry-sheet formula allowed", SRC + "catalog_import.py", "if ledger[\"formula_cells_in_entry_sheets\"]:", "if False:", [T_CAT]),
        ("CI-3 header match always true", SRC + "catalog_import.py",
         "def _header_ok(profile: CatalogProfile, sheets: Mapping[str, list[list[str]]]) -> bool:\n",
         "def _header_ok(profile: CatalogProfile, sheets: Mapping[str, list[list[str]]]) -> bool:\n    return True\n", [T_CAT]),
        ("CI-4 empty id accepted", SRC + "catalog_import.py", "if not row[idx].strip():", "if False:", [T_CAT]),
        ("CI-5 duplicate id accepted", SRC + "catalog_import.py", "if len(set(ids)) != len(ids):", "if False:", [T_CAT]),
        ("FZ-1 missing parameter defaulted", SRC + "formalize.py", "params[p.name] = _fv(None, FieldClass.UNDERSPECIFIED, \"not stated in source\")",
         "params[p.name] = _fv(1, FieldClass.EXPLICIT_SOURCE_RULE, \"default\")", [T_INT]),
        ("FZ-2 explicit value overridable", SRC + "formalize.py", "or tpl[\"params\"][name][\"value\"] is not None:", "or False:", [T_INT]),
        ("FZ-3 out-of-domain decision accepted", SRC + "formalize.py", "or not spec.lo <= v <= spec.hi:", "or False:", [T_INT]),
        ("FZ-4 futures not a blocker", SRC + "formalize.py", "if asset_class in (\"options\", \"fx\", \"crypto\", \"futures\") or", "if False and (", [T_INT]),
        ("DD-1 partial parameters may be exact", SRC + "dedup.py", "if sig[\"complete\"] else []", "", [T_INT]),
        ("DD-2 order dependent population", SRC + "dedup.py", "for idea in sorted(ideas, key=lambda i: i[\"intake_id\"]):", "for idea in list(ideas):", [T_INT]),
        ("DD-3 novelty review overrides any relationship", SRC + "dedup.py",
         "if rec[\"relationship\"] != Relationship.UNKNOWN_NEEDS_REVIEW.value:\n        raise", "if False:\n        raise", [T_INT]),
        ("AD-1 missing parameters admitted", SRC + "admission.py", "if tpl[\"missing_params\"]:", "if False:", [T_INT]),
        ("AD-2 grammar availability ignored", SRC + "admission.py", "if spec.grammar_v1 and grammar_available:", "if spec.grammar_v1:", [T_INT]),
        ("AD-3 exact duplicate re-admitted", SRC + "admission.py", "if rel == Relationship.EXACT_DUPLICATE.value:", "if False:", [T_INT]),
        ("AD-4 data blocker ignored", SRC + "admission.py", "if \"L\" in blockers or \"D\" in blockers:", "if False:", [T_INT]),
        ("PL-1 source conflict accepted", SRC + "pipeline.py",
         "elif prior[\"provenance\"][\"entry_canonical_hash\"] == idea[\"provenance\"][\"entry_canonical_hash\"]:", "elif True:", [T_INT]),
        ("PL-2 source copy not accounted", SRC + "pipeline.py", "copies.append({\"intake_id\": idea[\"intake_id\"],",
         "(lambda *_: None)({\"intake_id\": idea[\"intake_id\"],", [T_INT]),
        ("TP-1 non-canonical grammar name accepted", SRC + "templates.py", "if grammar_strategy_name(t.template_id, params) != name:", "if False:", [T_INT]),
        ("TP-2 parameter bounds not enforced", SRC + "templates.py", "or not p.lo <= v <= p.hi:", "or False:", [T_INT]),
    ],
    "ai": [
        ("AI-1 evidence not verified", SRC + "ai_normalize.py",
         "if isinstance(evidence, str) and evidence.strip() and _norm(evidence) in norm_source and _value_in_span(name, value, evidence) and _has_cue(evidence):",
         "if True:", [T_AI]),
        ("AI-2 evidence need not contain the value", SRC + "ai_normalize.py",
         "    return bool(re.search(rf\"(?<!\\d){value}(?!\\d)\", s))", "    return True", [T_AI]),
        ("AI-3 model may override a recognized idea", SRC + "ai_normalize.py",
         "if tid is None or idea[\"kind\"] not in (\"RULE_TEXT_UNMAPPED\", \"UNRECOGNIZED\", \"DIAGNOSTIC_QUESTION\") or not v[\"verified\"]:",
         "if tid is None or not v[\"verified\"]:", [T_AI]),
        ("AI-4 remote ollama endpoint allowed", SRC + "ai_normalize.py", "if not _loopback(base_url):", "if False:", [T_AI]),
        ("AI-5 cloud without cost authorization", SRC + "ai_normalize.py",
         "if not cost_authorization_ref or not cost_authorization_ref.strip():", "if False:", [T_AI]),
        ("AI-6 non-object response accepted", SRC + "ai_normalize.py",
         "if not isinstance(obj, dict):\n        raise ValueError(\"response JSON is not an object\")",
         "if False:\n        raise ValueError(\"response JSON is not an object\")", [T_AI]),
        ("AI-7 unknown template accepted", SRC + "ai_normalize.py",
         "if tid is not None and (tid not in TEMPLATES or tid == \"legacy_engine\"):", "if False:", [T_AI]),
        ("AI-8 out-of-domain value accepted", SRC + "ai_normalize.py",
         "if isinstance(value, bool) or not isinstance(value, int) or not p.lo <= value <= p.hi:", "if False:", [T_AI]),
        ("AI-9 call budget ignored", SRC + "ai_normalize.py", "if calls_left[0] <= 0:", "if False:", [T_AI]),
        ("AI-10 nonconformant backend used", SRC + "ai_normalize.py", "if conformant is False:", "if False:", [T_AI]),
        ("AI-11 response size unbounded", SRC + "ai_normalize.py", "if len(raw.encode(\"utf-8\")) > MAX_RESPONSE_BYTES:", "if False:", [T_AI]),
        ("AI-12 provider fault drops the entry", SRC + "ai_normalize.py",
         "    except ProviderError as exc:\n        rec.update(status=STATUS_FAILED, notes=[str(exc)])\n        return _with_ai(base, rec), rec",
         "    except ProviderError as exc:\n        raise", [T_AI]),
        ("AI-13 evidence needs no unit cue", SRC + "ai_normalize.py", "and _value_in_span(name, value, evidence) and _has_cue(evidence):", "and _value_in_span(name, value, evidence):", [T_AI]),
        ("AI-14 source tags not neutralized", SRC + "ai_normalize.py", "return re.sub(r\"<\\s*/?\\s*source\\s*>\", \"[source-tag]\", source, flags=re.I)", "return source", [T_AI]),
        ("CI-6 package size unbounded", SRC + "xlsx_reader.py", "if sum(i.file_size for i in zf.infolist()) > MAX_TOTAL_BYTES:", "if False:", [T_CAT]),
        ("CI-7 DTD tolerated", SRC + "xlsx_reader.py", "if b\"<!DOCTYPE\" in data[:4096].upper() or b\"<!ENTITY\" in data.upper():", "if False:", [T_CAT]),
        ("ST-11 decision ref matched as a pattern", SRC + "store.py", "payload[\"intake_id\"], payload[\"decision_ref\"])).fetchone()\n            if clash:", "payload[\"intake_id\"], payload[\"decision_ref\"][:1])).fetchone()\n            if clash:", [T_ST]),
        ("AD-5 unrecognized text not rejected", SRC + "admission.py", "if kind == \"UNRECOGNIZED\":", "if False:", [T_AI]),
    ],
    "rust": [
        ("R-1 sma gate non-strict", RS, "window as i128 * last > sum(win)", "window as i128 * last >= sum(win)", CARGO),
        ("R-2 dual cross non-strict", RS, "slow as i128 * sum(&win[win.len() - f..])\n                    > fast as i128", "slow as i128 * sum(&win[win.len() - f..])\n                    >= fast as i128", CARGO),
        ("R-3 momentum non-strict", RS, "last > close(&win[win.len() - 1 - lookback as usize])", "last >= close(&win[win.len() - 1 - lookback as usize])", CARGO),
        ("R-4 near-high boundary excluded", RS, "let near = 10_000 * last >= (10_000 - proximity_bps as i128) * high;",
         "let near = 10_000 * last > (10_000 - proximity_bps as i128) * high;", CARGO),
        ("R-5 trend filter non-strict", RS, "|| trend_window as i128 * last > sum(", "|| trend_window as i128 * last >= sum(", CARGO),
        ("R-6 non-canonical spelling accepted", RS, "if spec.canonical_name() != name {", "if false {", CARGO),
        ("R-7 fast>=slow accepted", RS, "if fast >= slow {", "if false {", CARGO),
        ("R-8 momentum history off by one", RS, "RuleSpec::AbsMomentum { lookback } => lookback as usize + 1,", "RuleSpec::AbsMomentum { lookback } => lookback as usize,", CARGO),
        ("R-9 short window goes long", RS, "            return 0;\n        };\n        let close = |b: &BarStub|", "            return 1;\n        };\n        let close = |b: &BarStub|", CARGO),
        ("R-10 fingerprint ignores the symbol", RS, ".push_str(&self.symbol)\n        .push_i64(TIMEFRAME_SECS)", ".push_i64(TIMEFRAME_SECS)", CARGO),
        ("R-12 parameter bounds not enforced", RS, "if (lo..=hi).contains(&v) {", "if true {", CARGO),
    ],
    "knowledge": [
        ("KN-1 executable claim tolerated", SRC + "knowledge.py", "if e.get(\"executable_rule\") is not False or", "if False or", [T_KN]),
        ("KN-2 byte pin removed", SRC + "knowledge.py", "if digest != expected_sha256:", "if False:", [T_KN]),
        ("KN-3 default parameters tolerated", SRC + "knowledge.py", "or e.get(\"default_parameters\") not in (None, {})", "or False", [T_KN]),
        ("KN-4 status not checked", SRC + "knowledge.py", "or data.get(\"status\") != EXPECTED_STATUS:", "or False:", [T_KN]),
        ("KN-5 benchmark counted as a candidate", SRC + "formalize.py", "elif _BENCHMARK.match(title) and not matches:", "elif False:", [T_KN]),
        ("KN-6 composite source id dropped", SRC + "catalog_import.py", "f\"{profile.catalog_family}:{file_key}:{r}\"", "f\"{r}\"", [T_KN]),
        ("KN-7 glossary never reaches the prompt", SRC + "ai_normalize.py", "prompt = build_prompt(source, knowledge.prompt_block(terms) if terms else \"\")",
         "prompt = build_prompt(source, \"\")", [T_KN]),
        ("KN-8 control counted as strategy kind", SRC + "contracts.py", "if idea_kind == \"GOVERNANCE_CONTROL\":", "if False:", [T_KN]),
    ],
    "campaign": [
        ("CP-1 invalid grid combinations dropped silently", SRC + "campaign.py", "report[\"excluded\"].append({\"source\": label, \"params\": params, \"reason\": str(exc)})", "pass", [T_CP]),
        ("CP-2 max_trials ignored", SRC + "campaign.py", "if trial_count > spec[\"population\"][\"max_trials\"]:", "if False:", [T_CP]),
        ("CP-3 protocol profile not pinned", SRC + "campaign.py", "if hashlib.sha256(canonical(decl).encode(\"utf-8\")).hexdigest() != pin:", "if False:", [T_CP]),
        ("CP-4 synthetic data may carry a market grade", SRC + "campaign.py", "if official != (spec[\"evidence_grade\"] != \"SYNTHETIC_DIAGNOSTIC\"):", "if False:", [T_CP]),
        ("CP-5 data pins unchecked", SRC + "campaign.py", "if spec[\"data\"][k] != m.get(mk):", "if False:", [T_CP]),
        ("CP-6 unadmitted idea enters a population", SRC + "campaign.py", "if not ok or not rec.get(\"execution_path\"):", "if False:", [T_CP]),
        ("CP-7 legacy engine allowed", SRC + "campaign.py", "CARD_BY_ID[sid].template_id == \"legacy_engine\" and sid not in src.get(\"allow_legacy\", [])", "False", [T_CP]),
        ("CP-8 gate enters the identity", SRC + "campaign.py", "if k != \"execution_gate\"}).encode", "if True}).encode", [T_CP]),
        ("CP-9 grammar used when unavailable", SRC + "campaign.py", "if not grammar_available:\n                raise CampaignError(\"grammar_v1 is not available", "if False:\n                raise CampaignError(\"grammar_v1 is not available", [T_CP]),
        ("EX-1 gate not enforced", SRC + "executor.py", "if gate.get(\"executable\") is not True:", "if False:", [T_CP]),
        ("EX-2 authorization not verified", SRC + "executor.py", "if auth_class != sa.READ_ONLY:\n            try:", "if False:\n            try:", [T_CP]),
        ("EX-3 native binary pin not verified", SRC + "executor.py", "if stage in NATIVE_STAGES:", "if False:", [T_CP]),
        ("EX-4 declaration identity unchecked", SRC + "executor.py", "if declaration_identity(decl) != campaign[\"declaration_sha256\"]:", "if False:", [T_CP]),
        ("SC-1 blocked work re-evaluated every pass", SRC + "scheduler.py", "reevaluate_blocked=(n == 0)", "reevaluate_blocked=True", [T_CP]),
        ("SC-2 job budget ignored", SRC + "scheduler.py", "if budget[0] <= 0:\n                        return ran", "if False:\n                        return ran", [T_CP]),
        ("SC-3 failure not reported", SRC + "scheduler.py", "elif \"FAILED\" in states:", "elif False:", [T_CP]),
        ("SC-4 dead worker leases never recovered", SRC + "scheduler.py", "result.interrupted_recovered = store.recover_expired()", "result.interrupted_recovered = 0", [T_CP]),
    ],
    "resume": [
        ("RS-1 terminal trials re-evaluated on resume", "experiments/m1_native_trend_campaign/run_batch.py", "if \"economic_eval_id\" in rec or \"failed\" in rec:\n        return True", "if False:\n        return True", T_E2E),
        ("RS-2 orphaned attempt left started", "experiments/m1_native_trend_campaign/run_batch.py", "store.finalize_attempt(a[\"attempt_id\"], status=\"failed\", failure_reason=INTERRUPTED_REASON)", "pass", T_E2E),
        ("RS-3 succeeded attempt re-run when index lacks it", "experiments/m1_native_trend_campaign/run_batch.py", "    if done:\n        a = done[-1]", "    if False:\n        a = done[-1]", T_E2E),
    ],
    "impl": [
        ("IM-1 request issued for an under-specified idea", SRC + "implementation.py", "if tid is None or tpl.get(\"missing_params\"):", "if tid is None:", [T_IM]),
        ("IM-2 operator authorization not required", SRC + "implementation.py", "put(\"explicit_operator_authorization\", ok_auth,", "put(\"explicit_operator_authorization\", True,", [T_IM]),
        ("IM-3 request claims to be executable", SRC + "implementation.py", "\"executable_now\": False", "\"executable_now\": True", [T_IM]),
        ("IM-4 rust registration not required", SRC + "implementation.py", "put(\"registered_in_rust\", registered,", "put(\"registered_in_rust\", True,", [T_IM]),
        ("IM-5 request issued for any disposition", SRC + "implementation.py", "if idea.get(\"disposition\") != \"NEEDS_IMPLEMENTATION\":", "if False:", [T_IM]),
    ],
    "scout": [
        ("SCT-1 robots.txt ignored", SRC + "scout.py", "if not self._allowed_by_robots(src, url):", "if False:", [T_SC]),
        ("SCT-2 any content type accepted", SRC + "scout.py", "if res.content_type not in ACCEPTED_TYPES:", "if False:", [T_SC]),
        ("SCT-3 foreign redirect host accepted", SRC + "scout.py", "if urlparse(res.final_url).hostname != src.domain:", "if False:", [T_SC]),
        ("SCT-4 http accepted", SRC + "scout.py", "if p.scheme != \"https\" or not p.hostname", "if not p.hostname", [T_SC]),
        ("SCT-5 rate limit ignored", SRC + "scout.py", "if now - self._last.get(src.domain, -1e18) < src.rate_limit_seconds:", "if False:", [T_SC]),
        ("SCT-6 size cap ignored", SRC + "scout.py", "if len(res.body) > src.max_bytes:", "if False:", [T_SC]),
        ("SCT-7 subdomain suffix match", SRC + "scout.py", "if p.hostname == a.domain and", "if p.hostname.endswith(a.domain) and", [T_SC]),
        ("SCT-8 path prefix ignored", SRC + "scout.py", "and (p.path or \"/\").startswith(a.path_prefix):", ":", [T_SC]),
        ("SCT-9 scripts kept in extracted text", SRC + "scout.py", "SKIP = {\"script\", \"style\", \"iframe\", \"object\", \"embed\", \"noscript\", \"template\", \"svg\", \"canvas\"}", "SKIP = set()", [T_SC]),
        ("SCT-10 access-controlled robots treated as open", SRC + "scout.py", "rp.disallow_all = True", "rp.parse([])", [T_SC]),
        ("SCT-11 retrieval time enters identity", SRC + "scout.py", "\"source\": {\"filename\": f\"scout-{source_id}\", \"format\": \"web\", \"sha256\": sha256_bytes(canonical_json([r[\"sha256\"] for r in records]).encode(\"utf-8\")),", "\"source\": {\"filename\": f\"scout-{source_id}\", \"format\": \"web\", \"sha256\": sha256_bytes(canonical_json([r[\"retrieved_at\"] for r in records]).encode(\"utf-8\")),", [T_SC]),
    ],
    "authority": [
        ("AU-1 exposed development declared promotion-eligible", SRC + "contracts.py", "return {\"promotion_eligible\": False, \"promotion_readiness\": \"NOT_ESTABLISHED\"}", "return {\"promotion_eligible\": True, \"promotion_readiness\": \"NOT_ESTABLISHED\"}", [T_AU]),
        ("AU-2 synthetic declared promotion-eligible", SRC + "contracts.py", "return {\"promotion_eligible\": False, \"promotion_readiness\": \"NOT_ELIGIBLE_SYNTHETIC\"}", "return {\"promotion_eligible\": True, \"promotion_readiness\": \"NOT_ELIGIBLE_SYNTHETIC\"}", [T_AU]),
        ("AU-3 report trusts a stored flag", SRC + "reporting.py", "**promotion_view(decl[\"evidence_grade\"][\"grade\"]),", "\"promotion_eligible\": decl[\"factory\"][\"promotion_eligible\"], \"promotion_readiness\": decl[\"factory\"][\"promotion_readiness\"],", [T_AU]),
        ("AU-4 status reads not-synthetic as eligible", SRC + "status.py", "**promotion_view(c[\"evidence_grade\"]), \"declaration_sha256\"", "\"promotion_eligible\": c[\"evidence_grade\"] != \"SYNTHETIC_DIAGNOSTIC\", \"promotion_readiness\": \"x\", \"declaration_sha256\"", [T_AU]),
        ("AU-5 declaration eligibility hard-coded true", SRC + "campaign.py", "**promotion_view(spec[\"evidence_grade\"]),", "\"promotion_eligible\": True,", [T_AU]),
        ("AU-6 Factory claims the Paper runtime state", SRC + "contracts.py", "\"paper\": \"NOT_TOUCHED_BY_FACTORY\",", "\"paper\": \"INACTIVE\",", [T_AU]),
        ("AU-7 authority scope dropped", SRC + "contracts.py", "\"scope\": \"FACTORY_ACTIONS_ONLY: the actual MQD Promotion, Paper and Live runtime state is not read or asserted here\",", "\"scope\": \"global\",", [T_AU]),
    ],
    "lane": [
        ("LN-1 strict native mode still skips", "tests/support/factory_e2e.py", "skipif(not REQUIRE_NATIVE and not cli_available()", "skipif(not cli_available()", [T_LN]),
        ("LN-2 guard tolerates a skipped required test", "../scripts/guards/check_factory_native_lane.py", "            problems.append(f\"SKIPPED (not allowed): {name}: {msg[:100]}\")", "            pass", [T_LN]),
        ("LN-3 guard accepts any skip reason for the optional test", "../scripts/guards/check_factory_native_lane.py", "OPTIONAL_SKIP[name] in msg", "True", [T_LN]),
        ("LN-4 guard ignores failures", "../scripts/guards/check_factory_native_lane.py", "if case.find(\"failure\") is not None or case.find(\"error\") is not None:", "if False:", [T_LN]),
        ("LN-5 lane drops the strict-native switch", "../.github/workflows/strategy-factory.yml", "MQK_FACTORY_REQUIRE_NATIVE: \"1\"", "MQK_FACTORY_UNUSED: \"1\"", [T_LN]),
        ("LN-6 lane stops running the guard", "../.github/workflows/strategy-factory.yml", "check_factory_native_lane.py \"$RUNNER_TEMP", "true \"$RUNNER_TEMP", [T_LN]),
    ],
    "history": [
        ("PS-1 compile ignores Factory history", SRC + "campaign.py", "known = build_index(repo_root, factory_prior)", "known = build_index(repo_root)", [T_PS]),
        ("PS-2 service compiles without priors", SRC + "service.py", "factory_prior=prior, prior_campaigns=prior_ids)", "factory_prior=(), prior_campaigns=prior_ids)", [T_PS]),
        ("PS-3 intake ignores Factory history", SRC + "service.py", "known = build_index(self.repo_root, factory_prior_entries(self.store.prior_campaign_strategies()[1]))", "known = build_index(self.repo_root)", [T_PS]),
        ("PS-4 frozen campaign recomputed against newer history", SRC + "service.py", "if existing is not None:", "if False:", [T_PS]),
        ("PS-5 stale-history race guard removed", SRC + "store.py", "if have != sorted(expected_prior_campaigns):", "if False:", [T_PS]),
        ("PS-6 history depends on outcomes", SRC + "store.py", "\"select distinct t.campaign_id, t.strategy_name from campaign_trials t join campaigns c using(campaign_id) \"", "\"select distinct t.campaign_id, t.strategy_name from campaign_trials t join campaigns c using(campaign_id) where not exists (select 1 from jobs j where j.campaign_id=c.campaign_id and j.status='failed') \"", [T_PS]),
        ("PS-7 grammar names yield no prior entry", SRC + "known_index.py", "if name.startswith(GRAMMAR_PREFIX):", "if False:", [T_PS]),
        ("PS-8 disclosure omits prior campaigns", SRC + "campaign.py", "\"prior_factory_campaigns\": sorted(prior_campaigns),", "\"prior_factory_campaigns\": [],", [T_PS]),
        ("PS-9 orphan declaration blocks the retry", SRC + "service.py", "if not decl_path.exists() or campaign_mod.declaration_identity(", "if not decl_path.exists() or False and campaign_mod.declaration_identity(", [T_PS]),
        ("PS-10 damaged frozen declaration not refused", SRC + "service.py", "            except (OSError, ValueError) as exc:", "            except ZeroDivisionError as exc:", [T_PS]),
        ("PS-11 frozen identity not verified", SRC + "service.py", "if campaign_mod.declaration_identity(frozen) != existing[\"declaration_sha256\"]:", "if False:", [T_PS]),
    ],
    "faults": [
        ("FT-1 executor exception escapes with a live claim", SRC + "scheduler.py", "    except BaseException as exc:                                         # noqa: BLE001 - nothing may escape with a live claim", "    except KeyboardInterrupt as exc:                                         # noqa: BLE001 - nothing may escape with a live claim", [T_SF]),
        ("FT-2 pre-execution fault mislabeled", SRC + "scheduler.py", "kind = \"pre_execution_error\" if campaign is None else \"executor_exception\"", "kind = \"executor_exception\"", [T_SF]),
        ("FT-3 scheduler error reported as idle", SRC + "scheduler.py", "    if errors or unresolved:", "    if False:", [T_SF]),
        ("FT-4 lost claim not fenced", SRC + "scheduler.py", "        except ClaimLost:", "        except ZeroDivisionError:", [T_SF]),
        ("FT-5 transient finish fault not retried", SRC + "scheduler.py", "FINISH_ATTEMPTS = 3", "FINISH_ATTEMPTS = 1", [T_SF]),
        ("FT-6 invalid executor outcome accepted", SRC + "scheduler.py", "if not isinstance(outcome, Outcome) or outcome.status not in VALID_STATUS:", "if False:", [T_SF]),
        ("FT-7 worker fault swallowed silently", SRC + "scheduler.py", "(errors if errors is not None else []).append(f\"worker {worker_id}: {_describe(exc)}\")", "pass", [T_SF]),
        ("FT-9 unrecorded outcome not listed", SRC + "scheduler.py", "(unresolved if unresolved is not None else []).append(exc.job_id)", "pass", [T_SF]),
        ("FT-10 claim loss is a generic error", SRC + "store.py", "raise ClaimLost(\"finish refused", "raise StoreError(\"finish refused", [T_SF]),
    ],
    "store": [
        ("ST-1 stage order ignored", SRC + "store.py",
         "and not exists (select 1 from jobs p where p.campaign_id=j.campaign_id and p.stage_order<j.stage_order and p.status!='succeeded')", "", [T_ST]),
        ("ST-3 finish ignores the claim token", SRC + "store.py",
         "select * from jobs where job_id=? and claim_token=? and status='running'\", (job_id, token)).fetchone()",
         "select * from jobs where job_id=? and status='running'\", (job_id,)).fetchone()", [T_ST]),
        ("ST-4 interruption not recorded truthfully", SRC + "store.py", "set status='interrupted'", "set status='failed'", [T_ST]),
        ("ST-5 predeclaration mutable", SRC + "store.py",
         "if row[\"spec_sha256\"] != spec_sha or row[\"declaration_sha256\"] != declaration_sha256:", "if False:", [T_ST]),
        ("ST-6 global concurrency limit ignored", SRC + "store.py",
         "if con.execute(\"select count(*) from jobs where status='running' and lease_expires >= ?\", (now,)).fetchone()[0] >= max_running:",
         "if False:", [T_ST]),
        ("ST-7 any job may be retried", SRC + "store.py", "and stage=? and status='failed'\"", "and stage=? and status in ('failed','succeeded')\"", [T_ST]),
        ("ST-8 invalid terminal status accepted", SRC + "store.py", "if status not in (\"succeeded\", \"failed\", \"blocked\"):", "if False:", [T_ST]),
        ("ST-9 duplicate trial keys accepted", SRC + "store.py",
         "if not trials or len({t[\"trial_key\"] for t in trials}) != len(trials):", "if not trials:", [T_ST]),
        ("ST-10 population may differ on re-create", SRC + "store.py", "if have != {t[\"trial_key\"] for t in trials}:", "if False:", [T_ST]),
    ],
}


def sha(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def run_set(name: str) -> int:
    results, bad = [], 0
    for mid, rel, old, new, tests in MUTANTS[name]:
        path = ROOT / rel
        orig = path.read_bytes()
        text = orig.decode("utf-8")
        nl = "\r\n" if "\r\n" in text else "\n"
        old, new = old.replace("\n", nl), new.replace("\n", nl)
        if text.count(old) != 1:
            print(f"BROKEN MUTANT {mid}: fragment occurs {text.count(old)} times")
            bad += 1
            continue
        try:
            path.write_bytes(text.replace(old, new).encode("utf-8"))
            if tests[0].startswith("cargo:"):
                import os
                env = {**os.environ, "CARGO_TARGET_DIR": os.environ.get("CARGO_TARGET_DIR", "C:/tmp/mqk-target-factory")}
                proc = subprocess.run(["cargo", "test", tests[0][6:], *tests[1:], "-j", "2"], cwd=ROOT.parent / "core-rs",
                                      capture_output=True, text=True, env=env)
            else:
                proc = subprocess.run([sys.executable, "-m", "pytest", *tests, "-q", "-x", "--tb=no", "-p", "no:cacheprovider"],
                                      cwd=ROOT, capture_output=True, text=True)
            killed = proc.returncode != 0
        finally:
            path.write_bytes(orig)
        restored = sha(path.read_bytes()) == sha(orig)
        results.append((mid, killed, restored))
        if not killed or not restored:
            bad += 1
        print(f"{'KILLED  ' if killed else 'SURVIVED'} restored={restored} {mid}")
    print(f"{sum(1 for _, k, _ in results if k)}/{len(MUTANTS[name])} killed, byte-exact restore {all(r for *_, r in results)}")
    return 1 if bad else 0


if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] not in MUTANTS:
        print("sets:", ", ".join(MUTANTS))
        raise SystemExit(2)
    raise SystemExit(run_set(sys.argv[1]))
