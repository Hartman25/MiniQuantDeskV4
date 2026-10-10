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
