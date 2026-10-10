# M1 historical equity/ETF data foundations

Acceptance authority: `docs/MQD_V4_PERMANENT_COMPLETION_CONTRACT_2026-10-10.md`, especially C02/C03/C04/C08/C09/C21/C25/C26. This mission supplies local CODE/TEST proof only, not operational provider evidence or whole-V4 completion. Independent acceptance is pending.

Original verified baseline: 8f438740ac926092bb522a906283b6a24f126d67. Implementation baseline: d3506ac40334eb79e21f1164e4a7337a3b36a7a8 (only the operator-requested completion contract and ledger link added). Branch: feature/m1-historical-pit-data-01. Worktree: C:/Users/Zacha/Downloads/MQD_Temporary_Worktrees/m1-historical-pit-data-01.

## Pre-edit bounded defect census

| Area / evidence seam | Disposition | Finding / bounded response |
|---|---|---|
| Provider parsing, normalization (`alpaca_historical.fetch_historical_bars`) | FIX | Missing volume defaults to zero; naive provider time assumes UTC; invalid/negative OHLCV, impossible open/range and unexpected symbols are not rejected. Validate before normalization. Out-of-order transport is deterministically sorted; duplicates refuse. |
| Pagination / partial provider failure (`_fetch_all_pages`) | FIX | HTTP errors and max-pages already refuse; malformed top-level payload and repeated/malformed tokens need typed refusal. No partial extraction is returned. |
| Raw vs adjusted / double adjustment (`_neutral_extract`, CA integrity gate) | ALREADY CORRECT + PROVEN | Research requests provider adjustment=all exactly once; local code does not reapply splits. Runtime requests raw. Existing offline extractor/attestation tests prove the split. |
| Corporate actions and role continuity | ALREADY CORRECT + PROVEN | Complete process-date discovery, role/CUSIP evidence, policy fingerprint and unresolved-action refusal exist. 171 extractor/attestation baseline tests pass. No new policy for reviewed events. |
| Publication, retrieval, effective time, vintage and decision time | FIX | Source retrieval and mapping-asof are recorded, but there is no requestable historical-PIT qualification gate. Record explicit unknown publication/revision availability; reject PIT claims and decisions before the snapshot. Mapping-asof is not a revision vintage. |
| Symbol changes, delistings, stable identifiers | GENUINE HARD STOP | Existing CA role evidence is not complete instrument/symbol history. Provider capability for complete historical continuity is unavailable/unproven. Represent blocked capability; never invent delisting dates. |
| Universe survivorship (`universe.snapshot`) | ALREADY CORRECT + PROVEN | Current registry snapshot explicitly reports non-PIT membership. Fixed ex-ante is the existing supported universe mode; no historical membership source exists. |
| Historical universe membership source | GENUINE HARD STOP | No licensed PIT membership source established. Refuse requests requiring it. |
| Calendar / sessions / timezone | FIX | Rust `sessions.rs` is existing bounded 2016-2026 daily authority (including exceptional closures); `calendar.rs` has 2023-2028 early closes. Qualified daily loader must verify labels and missing/extra sessions against these authorities, never derive sessions from prices. |
| Halt-specific missing bars / unsupported intraday qualification | GENUINE HARD STOP | No complete halt-history source established. Missing expected sessions must refuse qualification, without asserting the cause. Do not invent an intraday calendar. |
| Missing, stale, partial, duplicate bars and units | FIX | Extractor quality and qualified consumer gate must enforce nonempty complete OHLCV with USD price/share-volume conventions. Calendar/finalization requirements are explicit, not inferred from completeness of pagination. |
| Schema/migrations/query/binds (`md_bars`, migrations 0003/0042, adapter.history) | OUT OF SCOPE | Runtime DB retains current keyed observations and upserts revisions. Do not change concurrent runtime-owned storage. It is not a PIT archive; research qualification must refuse treating it as one. Existing adapter filters is_complete where present; older schemas do not prove finality. |
| Existing manifests / identity (`bars_provenance`) | FIX | Existing close hash and pricing high/low hash intentionally have frozen semantics. Extend existing manifest with versioned full-OHLCV historical contract, identity-bound only when present; retain legacy identity interpretation. No parallel dataset registry. |
| Transport/layout identity | ALREADY CORRECT + PROVEN | Existing semantic hashes sort content; source identity excludes raw page bytes and retrieval clock. Maintain this, including directory moves and formatting. |
| Restart/retry/atomic publication (`write_research_extraction_artifacts`) | FIX | Four in-place writes can overwrite an existing dataset or leave partial publication. Stage a complete sibling directory, verify and atomically publish without replacement; exact retry is idempotent; revisions require a new destination. |
| Cache / stale configuration authority | FIX | Reuse must verify complete artifacts, checksums and semantic provenance, never accept path or caller hash alone. Changed semantic contract changes identity and prevents reuse. |
| Failure / fallback | ALREADY CORRECT + PROVEN | Provider errors and unsupported CA refuse; diagnostic injection cannot mint official authority. No fallback provider downloads authorized. |
| Replay / reproducibility | FIX | Add deterministic local round-trip and partial-publication/retry proof at writer/loader seams. Raw page checksums remain evidence; raw page bodies are not currently retained. |
| Research / Backtest consumers (`economic_walkforward`, registered wrapper) | FIX | Add optional explicit qualification requirements through existing loader/provenance interfaces; check full-OHLCV extension in existing content gate. Preserve legacy behavior except demonstrated invalid-data acceptance. |
| Factory evidence (`campaign.check_data_authority`) | ALREADY CORRECT + PROVEN | Synthetic/official grade mismatch refuses; keep fixtures usable. Prove affected guards with focused tests; no broad Factory rewrite. |
| Feature/label chronology / trial counts | ALREADY CORRECT + PROVEN | Existing future-execution gate and distinct trial/attempt paths remain unchanged; existing causal/identity regressions will run. fwd_ret remains a label. |
| Reserved holdout / HOA-KISS-EXT032-01 | OUT OF SCOPE | No real holdout files or incident content accessed. Existing deny/reserved logic remains unchanged; offline negative proof only. Never infer incident resolution. |
| Test bypasses / skip claims | FIX | New proof is deterministic, offline and cannot skip for absent provider/DB. Existing opt-in DB tests are not DB proof. No CI or provider-success claims. |
| TODO/FIXME/unimplemented paths | OUT OF SCOPE | Non-equity adapter stubs explicitly unsupported; no targeted historical production TODO discovered in the inspected data modules. No unrelated implementations. |

## Implementation sequence and proof plan

1. Strict provider normalization and typed pagination refusals; pre-fix RED tests and focused extractor proof.
2. Versioned historical manifest extension, full OHLCV identity and explicit fail-closed PIT/capability/calendar qualification; real consumer-path negative proof and legacy regressions.
3. Immutable atomic extraction publication, integrity-checked reuse and qualified artifact loading; repeat/layout/revision/crash/corruption proofs.
4. Final adversarial definitions/callers/callees sweep; one combined affected-subsystem acceptance; compact report and local commits.

Required test cases 1-18 from the controller are tracked by named tests in the final proof index. Provider revisions/PIT universe/history cannot be positively proven with synthetic fixtures: their correct outcome is explicit refusal. Early closes are legitimate daily sessions; finalization will use supported session-close evidence only. Unsupported dates/resolutions refuse qualification.

## Tooling and boundaries

mqk_readonly, Srclight and Graft unavailable in the exposed tool inventory. Inspected repository-local mqd-diagnose, mqd-test-proof, mqd-review-patch, mqd-handoff and mqd-external-research SKILL.md files; native rg, focused source reads and deterministic tests are used. External research not needed for a new provider claim. Existing repository Python environment lacked pytest; mission-local ignored .venv-test-tools supplies pytest/ruff, using bundled Python and installed repo scientific dependencies. No provider downloads, secrets, .env.local, actual holdout, trading or other-agent checkout edits. smoke_logs/ protected. CI DISABLED / NOT RUN. NO PUSH.

## Implementation proof (patches 1-2)

Patch 1 `cfde09cd`: parser controls reproduced 20 failures / 2 passes before the repair; focused post-fix parser/extractor proof was 145 passed. Baseline extractor/attestation proof was 171 passed. Configured full lint reported 101 pre-existing typing/import modernization findings; no unrelated cleanup applied. Isolated E9/F lint and compile checks passed.

Patch 2: existing bars manifest extended with `historical_data_contract` + derived ID. Full OHLCV (including open and volume) is identity-bound. Fresh extractor identity is v3; v1/v2 attestations retain their original verification and hash interpretation. V3 source attestation binds the new contract ID; stripping it refuses. Legacy data remains usable by legacy callers but cannot claim historical qualification. The canonical-timeframe identity helper preserves daily alias equivalence. Snapshot retrieval remains audit metadata and cannot manufacture candidates; changed provider content changes semantic identity.

`economic_walkforward.load_bars`, `run_economic_walkforward`, and `run_registered_economic_walkforward_eval` accept explicit historical requirements. Requests for PIT bars, PIT membership or complete stable-instrument/delisting history refuse as unavailable capabilities. Qualified retrospective snapshots require full content and official-authority/CA verification, a decision at/after retrieval, strict NY daily labels, and exact expected sessions from the existing Rust calendar. A next-civil-midnight finalization bound is intentionally conservative and valid for early closes; the system does not assert exact historical publication/finality clocks. No intraday or halt-specific qualification is invented.

Focused patch-2 proof: foundation/extractor/attestation/bars-provenance tests, 294 passed. Includes provider split adjustment exactly once, DST, Thanksgiving/early close, exceptional 2025 closure, missing/extra sessions, partial session, unsupported dates/resolution, wrong timezone, future snapshot, unknown policies, volume/open tamper, legacy identity and synthetic authority refusal. These are CODE/TEST fixtures, not provider/economic evidence.

## Patch 3 publication/replay proof

Writer controls were RED before repair: repeat writes changed published mtimes, a mid-write crash left the final directory visible, and a corrupt cache was overwritten instead of refused. Atomic publication now stages all four files under an OS-owned nonblocking lock, fsyncs files, validates their complete checksum/content set, then renames the sibling staging directory into place. Existing final directories are validated and never overwritten. Interrupted staging is recovered under released OS ownership; unknown staging contents refuse. No random stage naming, background retry, stale-lock timeout or hidden wall-clock policy. Lock contention is an explicit retryable refusal. Power-loss/filesystem directory-metadata durability is not proven by process-crash tests.

`load_research_extraction_artifacts` verifies manifest self-integrity, the exact artifact inventory, physical hashes/counts, source/CA/content binding and optional historical requirements. CLI `run_alpaca_research_extraction` uses the existing semantic provenance identity for destinations, so provider revisions/policy changes choose a new path. New historical-contract daily aliases share semantic identity; legacy manifests retain the prior raw-label behavior. CSV writer/loader round trips use exact float parsing.

Focused foundation proof: 72 passed, including real child-process os._exit during partial staging, OS lock release and deterministic retry, rename crash, corruption of each artifact, repeated no-write reuse, equivalent layout/retrieval/daily alias reuse, exact floating-point round trip, and changed-volume CLI revision destinations. Existing diagnostic extractor writer fixture also passed (63-test run before additional controls). No economic evidence inferred from fixtures.

## Final adversarial second sweep

Changed definitions and actual direct callers/callees were checked, including the research CLI, registered/economic loaders, provenance verification, immutable writer/reader, Factory data-pinning/reuse paths and both existing Rust calendar authorities. Three demonstrated defects were repaired: impossible mapping dates reached transport; v2 attestations could acquire caller-added full-OHLCV claims without binding those fields into source identity; half-open queries at calendar coverage edges included irrelevant out-of-coverage civil dates. All seven regression controls went RED before the repair. Qualified API malformed manifests/windows now produce explicit refusals. Legacy Factory staging remains on its accepted protocol; no Factory scheduler/authorization, holdout policy, registry counting or runtime seam changed. New qualified artifact loading requires the complete verified set.

Three targeted source mutations were killed: remove full-OHLCV binding (4 failing controls), bypass future-snapshot decision boundary (1 failing control), bypass physical checksum verification using a semantically unchanged newline alteration (1 failing control). Each source was restored byte-for-byte before proceeding. No broad mutation campaign.

Review axes: contract - changes confined to historical research/data and direct consumers, no safety-authority expansion; quality - existing manifest/identity/loader/writer reused, one focused historical-contract module, no dataset registry or backtest engine; proof - real wrapper/loader/CLI/writer paths, pre-fix RED and guard-removal falsification, real child-process crash, no provider/DB/CI claims. Independent review remains pending.

## Final local verdict and review inventory

**LOCALLY COMPLETE (authorized software scope); INDEPENDENT ACCEPTANCE PENDING.** No whole-V4 FINISHED, provider operational verification, market OOS qualification or profitability claim. The permanent completion contract is the acceptance authority; unavailable mandatory capabilities still block their platform matrix cells.

| Commit | Coherent invariant |
|---|---|
| cfde09cd | Strict provider normalization; typed malformed/pagination refusal; pre-edit census |
| b2ad6f51 | V3 source-bound historical OHLCV contract; explicit snapshot/PIT/capability qualification; existing economic consumers |
| a734e5d7 | Immutable atomic publication, integrity-checked reuse/loading, revision-sensitive CLI paths and exact CSV round trip |
| dbe061911570d69dc31949c87660258c7066063c | Adversarial sweep repairs: legacy qualification downgrade, calendar edges, invalid asof and precise malformed-input refusals |

Implementation/proof SHA: `dbe061911570d69dc31949c87660258c7066063c`. Final subsequent commit is documentation only; discover its exact SHA with `git log -1`. Starting implementation HEAD: `d3506ac40334eb79e21f1164e4a7337a3b36a7a8`; original controller baseline `8f438740ac926092bb522a906283b6a24f126d67` was fetched/verified before the operator-directed documentation-only fast-forward. Branch/worktree are recorded above. No rebase/reset/stash/clean used.

| Changed file | Why required / exact seam |
|---|---|
| research-py/src/mqk_research/data/alpaca_historical.py | `_fetch_all_pages`, `fetch_historical_bars`, `fetch_corporate_actions`, `_require_resolved_asof`: strict source parsing. `_mint_manifest`: v3 contract binding. `write_research_extraction_artifacts` + `load_research_extraction_artifacts` and focused lock/validation helpers: atomic immutable publication/recovery and verified local reuse. |
| research-py/src/mqk_research/data/bars_provenance.py | Extend existing v3 attestation/manifest identity, preserve v1/v2 interpretation, enforce full contract in `require_bars_match_manifest`; canonical daily aliases only for new contracts. |
| research-py/src/mqk_research/data/historical.py | The focused qualification implementation: canonical OHLCV, pinned existing Rust daily calendar, explicit unknown PIT/history capabilities, decision/snapshot chronology and deterministic report. This extends existing bars authority; no new dataset registry. |
| research-py/src/mqk_research/ml/economic_walkforward.py | `load_bars` and `run_economic_walkforward` receive optional historical requirements and persist qualification report; reject infinite closes and preserve CSV float identity. |
| research-py/src/mqk_research/ml/economic_registry_integration.py | `run_registered_economic_walkforward_eval`: historical preflight before registry/classification side effects, forward requirements to existing economic evaluation. |
| research-py/src/mqk_research/cli.py | `run_alpaca_research_extraction`: semantic destination identity rather than window-only identity; revision changes cannot overwrite an earlier extraction. |
| research-py/tests/test_alpaca_historical.py | Existing version assertions updated to fresh v3 while explicitly retaining legacy v2 verification. |
| research-py/tests/test_historical_data_foundations.py | 84 deterministic real-seam foundation/negative/recovery tests; no provider/DB/native binary dependency or skipped path. |
| docs/research/M1_HISTORICAL_PIT_DATA_01.md | Mission-specific census, decisions, proof index and independent-review handoff; global ledger untouched by mission patches. |

Actual production chain: research CLI -> official extractor -> strict provider pagination/normalization -> corporate-action review -> v3 source attestation + existing bars manifest -> atomic writer -> verified local artifact loader. Registered economic consumer -> optional qualified `load_bars` preflight -> existing trial/attempt/classification flow -> existing `run_economic_walkforward` -> existing content/CA/pricing gates -> causal folds. Factory uses its existing provenance pins, declared grade and accepted economic bridge; its engine, scheduler and authorization were not rewritten. Existing frozen artifact-copy stages retain their legacy stage protocol; qualification does not infer completeness from a directory or manifest alone.

### Identity and temporal truth

Full dataset identity is the hash of the existing semantic provenance fragment (canonical daily timeframe for new historical contracts), including provider IDs, query range, symbols, fixed-ex-ante universe mode, adjustment/CA policy and evidence, source attestation and the new contract ID. The new contract binds full normalized OHLCV, USD/provider-adjusted-share units, asset class, calendar content, requested-symbol-only identity status, resolution, timestamp meaning/timezone, mapping-asof, normalization and quality policy. Paths, CSV layout, page boundaries, retrieval clock, results and physical file checksums are outside economic identity. Physical artifacts have separately verified hashes and manifest integrity.

Provider event/bar label is recorded as period-start despite the legacy end_ts column name. CA effective windows remain distinct from process-date discovery. Provider publication time and revision vintage are unknown; mapping-asof is entity resolution only. Retrieval is an audit snapshot, never historical availability evidence. Research/evaluation decision is explicit and must not precede that snapshot. The supported positive grade is only QUALIFIED_RETROSPECTIVE_SNAPSHOT; point_in_time_qualified is always false. A missing expected session refuses without inventing a halt, delisting or prior ticker mapping. Calendar coverage is the existing 2016-2026 Rust session-date authority; early closes count as sessions. Conservative next-local-midnight finalization may refuse otherwise final same-day observations. Intraday and exact halt/publication-history qualifications are unavailable.

### SHA-bound affected acceptance

On implementation SHA `dbe061911570d69dc31949c87660258c7066063c`, bundled Python 3.12 with repo scientific dependencies (pandas 3.0.1), pytest 9.1.1, Windows, numerical threads constrained to one:

```text
python -m pytest research-py/tests/test_historical_data_foundations.py research-py/tests/test_alpaca_historical.py research-py/tests/test_source_attestation.py research-py/tests/test_bars_provenance.py research-py/tests/test_economic_walkforward.py research-py/tests/test_experiment_registry.py research-py/tests/test_execution_pricing_parity_p7a.py research-py/tests/test_universe_snapshot.py research-py/tests/test_holdout_wiring.py research-py/tests/test_holdout_ledger.py research-py/tests/test_strategy_factory_campaign.py::test_unsafe_specs_are_refused research-py/tests/test_strategy_factory_authority_truth.py::test_every_supported_grade_is_covered_and_none_is_promotion_eligible -q --tb=short -p no:cacheprovider
557 passed in 48.74s; exit 0; no skips
```

All provider paths in these fixtures use injected or monkeypatched offline transports; none contacts a provider. Separately verified the actual `MQK_HERMETIC_NO_PROVIDER=1` boundary: credential loading and default HTTP both raise ProviderAccessDenied before environment/HTTP access. The acceptance invocation included an inert MQK_RESEARCH_DENY_PROVIDER_ACCESS variable; it is not claimed as enforcement. An earlier direct economic/registry regression invocation had no retained completion output; no separate pass claim is made for it. The SHA-bound combined acceptance above supplies those results.

Final compileall on all changed Python production/test files passed. Final `ruff check --isolated --select E9,F` on all changed Python files passed. Changed/new sections were formatted, without whole-file cleanup. `git diff --check d3506ac4 HEAD` passed. The configured broad lint had pre-existing modernization findings as recorded earlier; it is not claimed green. No Rust source changed. Full local workspace acceptance: NOT RUN under the laptop resource-safety rule. Broad CI is operator-disabled, not delegated or claimed passed. No DB-backed/live-provider proof or broad Factory/native E2E rerun.

### Controller proof index (items 1-18)

All new named tests below are in test_historical_data_foundations.py unless another file is stated.

| Item | Load-bearing proof |
|---|---|
| 1 repeat ingest | test_repeat_publication_idempotent_and_revision_never_overwrites |
| 2 equivalent transport/layout | test_equivalent_layout_daily_alias_and_retrieval_preserve_publication; existing page-segmentation attestation test |
| 3 semantic policy identity | test_semantic_contract_policy_changes_identity_and_refuses_unknown_policy; existing test_bars_provenance policy identity cases |
| 4 historical query/revision snapshots | test_same_query_different_provider_revision_changes_identity; true provider vintage remains unavailable and is never inferred from retrieval |
| 5 future-known refusal | test_future_revision_refused_at_earlier_decision; killed boundary-removal mutation |
| 6 split once | test_provider_split_adjustment_applied_exactly_once |
| 7 missing adjustment metadata | test_missing_adjustment_metadata_refused |
| 8 duplicates/out-of-order | existing test_fetch_bars_duplicate_symbol_end_ts_fails_closed; test_zero_volume_and_out_of_order_transport_are_valid |
| 9 holiday/early close/boundary | test_qualified_real_loader_holiday_early_close_and_boundary; DST, exceptional closure and exact coverage-edge tests |
| 10 timezone | test_wrong_daily_timezone_refused_by_qualified_loader; test_naive_csv_timestamp_cannot_be_hidden_by_legacy_utc_conversion |
| 11 invalid price/volume | test_provider_invalid_fields_refused; test_provider_missing_volume_is_not_zero; full-OHLCV gate controls |
| 12 crash/restart | test_real_process_crash_releases_lock_and_retry_recovers; write/rename crash and lock-contention controls |
| 13 corruption | test_corrupt_artifact_set_refused_without_repair; test_physical_checksum_tamper_with_semantic_identical_bytes_refused; killed checksum-removal mutation |
| 14 symbol/delisting ambiguity | unexpected-symbol refusal; test_unavailable_provider_capabilities_fail_closed (stable-instruments case) |
| 15 missing provider capability | PIT, PIT universe, stable-instrument and unsupported-resolution refusal cases |
| 16 synthetic grading | test_synthetic_cannot_be_qualified_market_evidence; Factory unsafe-grade/none-promotion-eligible acceptance cases |
| 17 unaffected consumers | economic_walkforward, experiment_registry, pricing parity, universe and bars/source provenance acceptance files above |
| 18 holdout remains excluded | test_holdout_never_produces_economic_output / reserved-not-evaluated pricing test; synthetic temporary holdout-ledger/wiring controls; no real reserved dataset/incident access |

### Limitations, blockers and backlog

No known unmitigated ordinary deterministic defect remains in the implemented bounded path after the sweep. Capability blockers: licensed/available historical publication/revision vintages, complete stable instrument/symbol/delisting history, historical PIT universe membership, intraday/halt-specific qualification. Operator approval/entitlement is required before adding or downloading any new provider history; none is needed to accept the implemented explicit refusals. Positive claims for these capabilities require actual provider evidence, not synthetic fixtures.

Operational limitations: no real-provider extraction, no licensed-history completeness proof, no DB-vintage archive/migration, no power-loss/filesystem directory-metadata durability trace. Existing runtime md_bars upserts remain current-state observations and cannot serve as PIT history. Source hashes establish integrity/lineage within the existing trusted-process model; they do not independently authenticate a vendor or prove entitlement. Legacy datasets have their historical verification scope and cannot silently acquire the new grade. Mandatory whole-V4 matrix cells remain unverified/blocked as appropriate.

Backlog (not local blockers): provider-supplied authenticated vintage/PIT history when authorized; proven historical instrument/universe sources; exact session publication/finality evidence and intraday/halt coverage; operational filesystem/provider verification in an authorized environment. No new economic/operator policy was invented.

Boundaries: no holdout dataset was read/fetched/evaluated and no HOA-KISS-EXT032-01 incident was adjudicated, inferred resolved or represented clean. Synthetic temporary-ledger tests do not access the real incident or reserved data. No Paper, Live, broker, Promotion, orders, fills, positions, GUI, daemon scheduling or shared runtime edits. No secrets or .env.local read/printed/committed. smoke_logs/ untouched. The mission worktree is under the required Downloads root; other checkouts were not edited. Tracked/untracked working tree was clean before this documentation-only final commit; ignored tooling/proof logs stay local. CI: DISABLED / NOT RUN. **NO PUSH.**

### Compact handoff

SNAPSHOT AT HEAD dbe061911570d69dc31949c87660258c7066063c
NOT AUTHORITATIVE AFTER HEAD CHANGES

Authoritative repository: Hartman25/MiniQuantDeskV4. Branch/worktree above. Origin/main observed d3506ac40334eb79e21f1164e4a7337a3b36a7a8. Scope: V4-M1-HISTORICAL-EQUITY-PIT-DATA-FOUNDATIONS-01. Status: local implementation complete, independent acceptance pending. Accepted/frozen references: permanent completion contract, CLAUDE.md, Research_Backtest_V1_Closeout_Audit.md; trial/attempt, causal execution and holdout semantics not reopened. All four implementation commits have CODE/TEST evidence and ACCEPTANCE PENDING. Next action: one independent acceptance review of this inventory and SHA-bound proof; at most one consolidated surgical correction under the permanent completion contract. No autonomous continuation, push, provider download, trading or global ledger reconciliation is authorized by this handoff.
