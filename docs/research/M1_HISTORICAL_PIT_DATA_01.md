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
