# Alpha Edge Census 01 — V3 correction: MCP / skill disposition (IR-10)

Mission `V4-ALPHA-EDGE-CENSUS-01-FINAL-CONDITIONAL-CORRECTION-02`. Repo code/tests/evidence are authoritative over any
tool output; every tool result below was cross-checked against git or the working tree.

| Tool | Disposition | Concrete evidence |
|---|---|---|
| `mqk_readonly` | **USED** (available, applicable) | `mqk_current_head` = `0032dda3…` at start and `df1a635f…` after D2; `mqk_repo_status` = `main...origin/main [ahead 13]` then `[ahead 15]`; `mqk_git_log` (14 commits, matched `git log`); `mqk_find_symbol resolve_factor` -> `conditional.py:451`; `mqk_search_code REUSE_STATUSES` -> the three production sites + mutation harness; `mqk_list_smoke_logs` (read-only; `smoke_logs/` never staged); `mqk_git_show bba24e86` -> commit header + 7-file stat identical to `git show --stat` (+599/-184). |
| Srclight | **SKIPPED — stale index reverified** | `index_status`: `last_commit 0752f406…`, `indexed_at 2026-10-05T06:15:11Z`; HEAD is `0032dda3…` at start / `df1a635f…` later and contains the correction. A call-graph answer from this index would describe pre-correction code, so graph queries were not used; `mqk_find_symbol`/`mqk_search_code` + native Read of the exact files replaced them. Re-indexing was not performed (not required for the patch). |
| Graft | **UNAVAILABLE — `CONNECT_TIMEOUT`** | `MCP server graft connection timed out after 30000ms` at session start and on a later `ToolSearch`. Continued with native tools as the mission allows. |
| `mqd-test-proof` skill | **USED** | Applied to the CR-01/CR-02 proofs: claim/proof-class/falsifiability review. Verdicts: condition projection CONFIRMED (CODE/TEST); factor-level resume CONFIRMED (CODE/TEST); real-data condition equivalence LIKELY until the freeze-time equivalence proof runs on the 88-symbol universe. The review exposed one vacuous fixture (S08 never gapped in the synthetic bars) which was fixed with an event-rich fixture and a mandatory "the execution-only parameter really changes `d`" guard. |
| Context7 / Firecrawl / Playwright | Not applicable | No third-party library/provider behaviour was uncertain; no browser-facing behaviour. |
| Agents / subagents | Not used | Mission prohibits them. |

## Defect census (narrow scope + adjacent seams)

| Seam | Finding | Disposition |
|---|---|---|
| Condition identity | Strategy `exit`/`hold` params minted duplicate conditional hypotheses (434 vs 219 semantic conditions). | FIXED + PROVEN (`test_d1_*`, D01/D02/D03) |
| FactorSpec creation | `factor_spec` accepted any config params; lookback included S04/S09 exit window. | FIXED + PROVEN (guard refuses execution-only params / forged `condition_id`; `condition_lookback`) |
| FactorEvaluationSpec | `evaluation_id` is fixed per factor by the registered runner; a retry is the same evaluation. | ALREADY CORRECT + PROVEN (`test_d2_case_a`: retry shares `evaluation_id`) |
| Factor registry | The registered runner idempotently auto-registers; an unfrozen factor could be attempted. | FIXED + PROVEN (`resolve_factor` refuses an unregistered factor; no auto-registration) |
| Factor attempts / resume | Config-level resume could re-attempt a terminal horizon after a sibling horizon failed. | FIXED + PROVEN (Cases A–E, D05/D06, M20/M21) |
| Aggregate/result files | Config-level aggregate was the only progress record. | FIXED + PROVEN (per-factor records, registry-bound; reconstruction or fail closed) |
| FDR population | Population must be the registered V3 family only. | ALREADY CORRECT + PROVEN (D04/D07; V2 factors in the same store do not enter) |
| Edge Registry | Conditional records keyed by Strategy config; neighbours over the 434-grid. | FIXED + PROVEN (`condition_id`, 219-grid adjacency, D08) |
| V2/V3 lineage | V2 attempts must not satisfy V3. | FIXED + PROVEN (separate family, separate V3 registry DB, D10) |
| Tooling evidence | `mqk_readonly` disposition was not recorded. | FIXED (this document) |
