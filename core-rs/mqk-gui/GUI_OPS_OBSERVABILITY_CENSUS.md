# GUI operations / observability census
Mission: V4-GUI-OPERATIONS-ALERTS-DIAGNOSTICS-OBSERVABILITY-FULL-COMPLETION-01
Baseline: e190cce93672405263733ab5a10b4d9d5903adf2. Initial inspection before source edits.

## Coverage inventory
26 registered screens: controlStation, dashboard, execution, risk, portfolio, reconcile, strategy, ops, session, dailyOperations, config, marketData, ingest, settings, audit, incidents, alerts, operatorTimeline, runtime, metrics, topology, transport, artifacts, backtests, evidence, strategyScanner. Registration: src/features/screens/screenRegistry.tsx; navigation: src/components/layout/leftRailNav.ts; dock panels: src/features/workstation/panelRegistry.ts. Existing ErrorBoundary, truth gates, source badges, responsive CSS, workspace synchronization and detached-window recovery are reused. Research/backtest/scanner implementations remain independently owned.

## Findings and initial dispositions
BLOCKED here means identified implementation/proof pending, not an operator approval request.

| ID | Seam / defect | Initial disposition |
| --- | --- | --- |
| OBS-01 | system/api.ts incidents IIFE discards all rows even on active; legacy.IncidentsWrapper claims not_wired despite alerts_events.rs::incidents returning durable IncidentRow. IncidentsScreen expects unsupported impacts/update/action history. | BLOCKED |
| OBS-02 | api.ts metrics probe casts flat MetricsDashboardResponse to nested SystemMetrics; MetricsScreen and dashboard charts dereference nonexistent sections. Empty metrics sections falsely claim OK. | BLOCKED |
| OBS-03 | api.ts alerts/triage drops truth_state, note and linked_incident_status; created_at is nullable ACK timestamp, not first occurrence. AlertsScreen implies ack resolves attention and invents assignment/escalation semantics. | BLOCKED |
| OBS-04 | useOperatorModel and useSystemModel overlapping interval fetches lack sequencing, cancellation, error containment; order detail race and each poll resets selection. http.ts has no timeout. | BLOCKED |
| OBS-05 | feed/timeline wrappers accept malformed rows; unknown feed truth falls through; ordering lacks stable identity tie-break/dedup. OperatorTimelineScreen calls bounded data complete history. | BLOCKED |
| OBS-06 | RightOpsRail and BottomEventRail bypass endpoint authority, show empty-as-clear; operational dashboard/global status need endpoint-level review. | BLOCKED |
| OBS-07 | api.ts generic object casts accept malformed status, session, market-data, transport, risk/portfolio, OMS. Exceptions can reject entire Promise batch. Several truth checks use deny lists. | BLOCKED |
| OBS-08 | ExecutionScreen detail null quantities/counts use zero defaults; canonical economic null types differ from declared number-only types. | BLOCKED |
| OBS-09 | formatDateTime omits timezone; numeric formatting permits Infinity. Source timestamps, browser observation and market freshness need distinct labels. | BLOCKED |
| OBS-10 | Existing 26 routes/nav, workspace identity and evidence panels require acceptance rerun; evidence is registered in diagnostics but omitted from MONITOR_GROUPS.diagnostics. | BLOCKED |
| OBS-11 | Global degraded/heartbeat gate blanks alerts and diagnostics needed to investigate that condition; external source + not_applicable continuity bypasses execution/reconcile gate. | BLOCKED |
| OBS-12 | ControlStation domain/status readiness and risk/refusal/market-data views need real-contract integration proofs including absent status with reachable unrelated endpoint. | BLOCKED |
| DEP-01 | alerts_events.rs::alerts_triage uses unwrap_or_default for DB ack/link read errors while claiming active. Frontend cannot distinguish genuine empty storage from query failure. Need fail-closed backend error or independently declared per-lane truth; shared backend read-only. | BACKEND_DEPENDENCY |
| DEP-02 | alerts_events active rows lack first/latest occurrence, account/deployment identity; audit_ops timeline/feed bounded SQL reads lack pagination cursor/retention/completeness; no invented chronology or cross-domain identity. | BACKEND_DEPENDENCY |
| DEP-03 | daemon exposes one configured deployment, not independent Paper+Live domain snapshots or hot domain switching. Other domain must stay unavailable; no paper fallback for live. | BACKEND_DEPENDENCY |
| DEP-04 | oms_metrics flat snapshot has no recorded time series; reject_count_today hardcoded zero, missing latency/fees/slippage/scheduler error history cannot be manufactured. Omit unsupported metric. | BACKEND_DEPENDENCY |
| DEP-05 | Durable incidents expose ID/opened/title/severity/open/resolved/linked_alert/opened_by only. No updated_at, resolution timestamp/reason, impact arrays or action history; expose only known fields. | BACKEND_DEPENDENCY |
| DEP-06 | Replace/cancel chains are not_wired; strategy candidate promotion/deployment authorization, broader risk budgets and event-risk screening only where canonical authorities expose them. No new authority. | BACKEND_DEPENDENCY |
| ENV-01 | No live daemon or browser/desktop proof yet. Node/npm/Rust/Git available; Python/pytest absent from PATH. No mqk_readonly/Srclight/Graft/Rust LSP advertised. | BLOCKED |

## Sources read
CLAUDE.md; .claude/rules/gui_rules.md; bounded Master Plan/ledger sections; GUI_CONVERGENCE_CHECKLIST, GUI_PATCH_TRACKER, GUI_OPTIONAL_PANELS_BACKLOG; relevant canonical specification DOCX paragraphs extracted successfully from word/document.xml; src feature inventory, API/model/hooks, HTTP/legacy adapters, truth/source authority, row types; alerts_events.rs, audit_ops.rs, oms_metrics.rs and api_types.rs. Adjacent execution_flow, transport_quality, reconcile, market_data_readiness, autonomous_paper_status discovered for focused contract inspection. Tauri bootstrap/artifact-read implementation inspected; no trading authority added. Existing source/SSR tests discovered; no Playwright config in repository.

## Proof policy
Production adapters + real React render; transport mocks at fetch boundary; old implementation RED or killed representative mutation. Synthetic transport proof is CODE/TEST only, never DB/provider/Paper/desktop operational proof. Final dispositions must be supported by actual run output. Full workspace Rust acceptance prohibited. No shared master ledger, daemon, schema, DB, strategy, promotion or research changes.

## Final dispositions (implementation 2dfa4d1bab05b55a386f41f74af849f87e028ae8)

| IDs | Final disposition | Load-bearing evidence |
| --- | --- | --- |
| OBS-01 | FIXED+PROVEN | incidentContract fetch/model/React tests; row-drop mutation killed; browser incident detail |
| OBS-02 | FIXED+PROVEN | metricsContract fetch/model/React tests; missing-metric-zero mutation killed; daemon CC05 six tests |
| OBS-03 | FIXED+PROVEN for frontend; DEP-01 remains | alertContract and observabilityRendering tests; active ACK/resolved-case browser detail; storage-read caveat |
| OBS-04 | FIXED+PROVEN | latestRequest timeout/sequence tests; stale-publish mutation killed; browser delayed selection, refresh and identity invalidation |
| OBS-05 | FIXED+PROVEN within bounded history | historyContract tests; duplicate fixture collapses to one timeline row; explicit bounded-history copy |
| OBS-06 | FIXED+PROVEN | compact-rail, dashboard-lane and haltSummary render tests; missing-endpoint browser observations |
| OBS-07 | FIXED+PROVEN for consumed operational DTOs | status/snapshot/economic/row/orderDetail contract tests and malformed HTTP-body negative controls |
| OBS-08 | FIXED+PROVEN | nullable economic/render tests; order-detail fractional aggregate withheld; actual row quantity/price preservation assertions |
| OBS-09 | FIXED+PROVEN | explicit UTC formatting and nonfinite guards; no-offset timestamps refused in incident/ACK/metrics/reconcile contracts |
| OBS-10 | FIXED+PROVEN | registered Evidence monitor integration; all 26 final production-bundle navigation snapshots, no error boundaries |
| OBS-11 | FIXED+PROVEN | stale/degraded diagnostic rendering tests; fault evidence retained in production browser with warnings |
| OBS-12 | FIXED+PROVEN within current single-deployment authority | source/status/viewModel tests; missing status leaves Tier-0 unknown; Live-disabled and changed-account browser proof |
| DEP-01..06 | BACKEND_DEPENDENCY | unchanged daemon source; exact disposition in GUI_OPS_OBSERVABILITY_FINAL.md |
| DEP-07 | BACKEND_DEPENDENCY | order_history.rs replay cumulative_filled uses nullable whole quantity unwrap_or(0); GUI withholds unsupported totals |
| ENV-01 | PARTIAL_TEST_PROOF | browser and native startup now observed; native input/reload/packaging and genuine operational Paper/Live remain unproven |

## Second adversarial sweep

Completed the original bounded sweep and adjacent callers, without restarting the census. Findings below are frontend FIXED+PROVEN unless marked otherwise. The final 1,340-test suite has zero failures/skips. Five actual unsafe-behavior mutations fail assertions and exact source bytes are restored.

| Seam | Correction / proof |
| --- | --- |
| Canonical refusals | 401/403/409/500/503 and malformed success cannot silently select legacy truth; GET timeout includes body decoding; refusal mutation killed |
| Independent missing lanes | Positions/orders/fills, reconciliation details, dashboard alerts/session/config and compact rails distinguish absent from authoritative empty |
| Model contracts | Core scalars/wrappers, durable outbox/fill/Paper lanes, strategy/readiness and selected order DTOs validate consumed fields; malformed lanes fail independently |
| Async identity | URL-pinned observations, latest-request generations, bounded non-overlapping automatic polls, account/mode/generation-bound details; delayed first order cannot overwrite second |
| Execution flow | Requested order/run scope checked; changing filters clears obsolete evidence; unknown truth rejected |
| Diagnostic semantics | Stale/degraded valid evidence retained with warning, missing required truth takes precedence; metadata is not artifact generation |
| Replay and causality | Fractional cumulative/open aggregates withheld; request-time OMS state labeled; unsupported playback controls removed; proven intent lanes retained |
| Endpoint/navigation | Vite environment read corrected; inline validated settings replaces unsupported prompt; desktop-owned configuration locked by existing bootstrap; Evidence registered |
| Alert storage | Backend-active triage can hide DB read failure (DEP-01); frontend explicitly disclaims independent read success and does not hide ongoing fault after ACK |
| Timeline headers | Final continuation found unvalidated header objects/quantities/times; same order-detail guard now validates header and fill rows; header-trust mutation killed |
| Timestamp determinism | Final continuation found no-offset source timestamps accepted by Date.parse; existing explicit-zone validator now used for incident, ACK, metric and reconciliation instants |
| Failure wording | Removed unsupported promise that starting runtime/WS will restore an absent endpoint |
| Permissions/untrusted text | Read-only browser fixture request log is GET-only; no new control API; React escapes external strings (incident script-text negative assertion) |
| Resource/reload boundaries | Interval cleanup/generation invalidation retained; GET timers clear in finally; browser refresh and endpoint reload exercised; native reload not proved |
| Ownership | All changed paths are under mqk-gui; daemon/research/schema/central docs/smoke_logs diffs empty; no other worktree edits |

Remaining known ordinary deterministic frontend defects from this bounded sweep: NONE. This is a local implementation/proof disposition, not independent acceptance or a claim that unavailable backend functionality is implemented.
