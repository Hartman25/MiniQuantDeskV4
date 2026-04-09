// core-rs/mqk-gui/src/features/system/api.ts
//
// Public API surface for the system feature.
//
// This file owns:
//   - fetchOperatorModel()   — main model assembly (all backend probes + assembly)
//   - Order-detail fetches   — per-order timeline/trace/replay/chart/causality
//
// Exported helpers live in dedicated modules:
//   - http.ts     — fetchJsonCandidate, fetchJsonCandidates, tryFetchJson, postJson
//   - legacy.ts   — legacy protocol adapters, normalizers, mappers
//   - actions.ts  — invokeOperatorAction

import { withClassifiedPanelSources } from "./sourceAuthority";
import {
  fetchJsonCandidate,
  fetchJsonCandidates,
  tryFetchJson,
  type EndpointFetchResult,
} from "./http";
import {
  deriveDataSourceDetail,
  deriveExecutionSummaryFromOrders,
  mapActiveAlertsResponse,
  mapDaemonCatalog,
  mapEventsFeedResponse,
  mapExecutionOutboxWrapper,
  mapFillQualityWrapper,
  mapPaperJournalWrapper,
  mapLegacyPortfolioSummary,
  mapLegacyPositionsResponse,
  mapLegacyTradingFillsToRows,
  mapLegacyTradingOrdersToExecutionOrders,
  mapLegacyTradingOrdersToOpenOrders,
  mapLegacyStatusToSystemStatus,
  nowIso,
  type ActiveAlertsWrapper,
  type ConfigDiffsWrapper,
  type DaemonActionCatalogResponse,
  type DaemonAuditActionsWrapper,
  type DaemonArtifactsWrapper,
  type DaemonOperatorTimelineWrapper,
  type DaemonOrderTimelineResponse,
  type EventsFeedWrapper,
  type ExecutionOutboxWrapper,
  type FillQualityWrapper,
  type LegacyDaemonStatusSnapshot,
  type LegacyTradingAccountResponse, // exported from legacy.ts
  type LegacyTradingFillsResponse,
  type LegacyTradingOrdersResponse,
  type LegacyTradingPositionsResponse,
  type PaperJournalWrapper,
  type PortfolioFillsResponse,
  type PortfolioOpenOrdersResponse,
  type PortfolioPositionsResponse,
  type ReconcileMismatchesResponse,
  type RiskDenialsResponse,
  type StrategySummaryWrapper,
  type StrategySuppressionsWrapper,
  type SystemTopologyWrapper,
  type IncidentsWrapper,
  type ReplaceCancelChainsWrapper,
  type AlertTriageWrapper,
} from "./legacy";
import type {
  AlertTriageRow,
  ArtifactRegistrySummary,
  ArtifactRow,
  AuditActionRow,
  ConfigFingerprintSummary,
  ExecutionOrderRow,
  ExecutionSummary,
  OrderCausalityResponse,
  OrderChartResponse,
  OrderReplayResponse,
  OrderTimelineSurface,
  OrderTraceResponse,
  OrderTimelineTruthState,
  ExplicitSurfaceTruth,
  FeedEvent,
  IncidentCase,
  MarketDataQualitySummary,
  MetadataSummary,
  OmsOverview,
  OperatorActionDefinition,
  OperatorAlert,
  OperatorTimelineCategory,
  OperatorTimelineEvent,
  PortfolioSummary,
  PreflightStatus,
  ReconcileMismatchRow,
  ReconcileSummary,
  ReplaceCancelChainRow,
  RiskDenialRow,
  RiskSummary,
  RuntimeLeadershipSummary,
  ServiceTopology,
  SessionStateSummary,
  SystemMetrics,
  SystemModel,
  SystemStatus,
  TransportSummary,
} from "./types";
import { DEFAULT_PREFLIGHT, DEFAULT_STATUS } from "./types";

export { invokeOperatorAction } from "./actions";

// ---------------------------------------------------------------------------
// Internal helpers (used only within fetchOperatorModel)
// ---------------------------------------------------------------------------

function objectOrFallback<T>(value: unknown, fallback: T): T {
  return value && typeof value === "object" ? (value as T) : fallback;
}

// ---------------------------------------------------------------------------
// Main model assembly
// ---------------------------------------------------------------------------

export async function fetchOperatorModel(): Promise<SystemModel> {
  const statusProbe = await fetchJsonCandidates<SystemStatus | LegacyDaemonStatusSnapshot>(["/api/v1/system/status", "/v1/status"]);
  const healthProbe = await fetchJsonCandidates<MetadataSummary>(["/api/v1/system/metadata"]);

  // Extract legacy status only when the statusProbe itself resolved via the
  // legacy path.  Do NOT fire a second fetch — canonical is preferred and if
  // canonical failed the probe already tried the legacy path as a fallback.
  const statusCanonical = statusProbe.ok && statusProbe.endpoint === "/api/v1/system/status";
  const legacyStatusFromProbe: LegacyDaemonStatusSnapshot | null =
    statusProbe.ok && statusProbe.endpoint === "/v1/status"
      ? (statusProbe.data as LegacyDaemonStatusSnapshot)
      : null;

  const probes = await Promise.all([
    fetchJsonCandidates<PreflightStatus>(["/api/v1/system/preflight"]),
    fetchJsonCandidates<ExecutionSummary>(["/api/v1/execution/summary"]),
    // Execution orders: two-step fetch to distinguish "no snapshot" from "not mounted".
    // HTTP 503 = OMS loop has no snapshot → keep canonical path as missing (record in
    //   missingEndpoints) so isMissingPanelTruth fires and the execution panel blocks.
    //   Do NOT fall through to legacy broker orders — that would hide the missing truth.
    // HTTP 404 / network error = canonical not mounted → fall through to /v1/trading/orders.
    (async (): Promise<EndpointFetchResult<ExecutionOrderRow[] | LegacyTradingOrdersResponse>> => {
      const canonical = await fetchJsonCandidate<ExecutionOrderRow[]>("/api/v1/execution/orders");
      if (canonical.ok) return canonical;
      // 503 = explicit no-snapshot signal; keep canonical as the failed probe result.
      if (canonical.error === "HTTP 503") return canonical;
      // Any other failure (404 = unmounted, network error) → try legacy path.
      return fetchJsonCandidate<LegacyTradingOrdersResponse>("/v1/trading/orders");
    })(),
    fetchJsonCandidates<OmsOverview>(["/api/v1/oms/overview"]),
    fetchJsonCandidates<SystemMetrics>(["/api/v1/metrics/dashboards"]),
    fetchJsonCandidates<PortfolioSummary | LegacyTradingAccountResponse>(["/api/v1/portfolio/summary", "/v1/trading/account"]),
    // Portfolio positions: canonical route returns snapshot_state wrapper.
    // "active" → rows are real broker truth; "no_snapshot" → broker snapshot absent.
    // no_snapshot is returned as a failed probe (ok: false) so the endpoint lands in
    // missingEndpoints and isMissingPanelTruth fires, blocking the portfolio panel.
    // HTTP failure (404 = unmounted, network error) → fall through to legacy.
    (async (): Promise<EndpointFetchResult<PortfolioPositionsResponse | LegacyTradingPositionsResponse>> => {
      const canonical = await fetchJsonCandidate<PortfolioPositionsResponse>("/api/v1/portfolio/positions");
      if (canonical.ok) {
        if ((canonical.data as PortfolioPositionsResponse).snapshot_state === "no_snapshot") {
          return { ...canonical, ok: false, error: "no_broker_snapshot" };
        }
        return canonical;
      }
      return fetchJsonCandidate<LegacyTradingPositionsResponse>("/v1/trading/positions");
    })(),
    // Portfolio open orders: same snapshot_state pattern as positions.
    (async (): Promise<EndpointFetchResult<PortfolioOpenOrdersResponse | LegacyTradingOrdersResponse>> => {
      const canonical = await fetchJsonCandidate<PortfolioOpenOrdersResponse>("/api/v1/portfolio/orders/open");
      if (canonical.ok) {
        if ((canonical.data as PortfolioOpenOrdersResponse).snapshot_state === "no_snapshot") {
          return { ...canonical, ok: false, error: "no_broker_snapshot" };
        }
        return canonical;
      }
      return fetchJsonCandidate<LegacyTradingOrdersResponse>("/v1/trading/orders");
    })(),
    // Portfolio fills: same snapshot_state pattern as positions.
    (async (): Promise<EndpointFetchResult<PortfolioFillsResponse | LegacyTradingFillsResponse>> => {
      const canonical = await fetchJsonCandidate<PortfolioFillsResponse>("/api/v1/portfolio/fills");
      if (canonical.ok) {
        if ((canonical.data as PortfolioFillsResponse).snapshot_state === "no_snapshot") {
          return { ...canonical, ok: false, error: "no_broker_snapshot" };
        }
        return canonical;
      }
      return fetchJsonCandidate<LegacyTradingFillsResponse>("/v1/trading/fills");
    })(),
    fetchJsonCandidates<RiskSummary>(["/api/v1/risk/summary"]),
    // Risk denials: truth_state === "no_snapshot" means the execution loop is not
    // running — denial truth is unavailable and must not render as "zero denials."
    // The IIFE returns ok: false in that case so the endpoint lands in
    // missingEndpoints and isMissingPanelTruth fires for the risk panel.
    // No legacy fallback: there is no pre-canonical denial surface.
    (async (): Promise<EndpointFetchResult<RiskDenialRow[]>> => {
      const canonical = await fetchJsonCandidate<RiskDenialsResponse>("/api/v1/risk/denials");
      if (!canonical.ok) {
        // HTTP failure (404 = unmounted, network error).
        return { ok: false, endpoint: canonical.endpoint, error: canonical.error ?? "fetch_failed" };
      }
      const data = canonical.data as RiskDenialsResponse;
      if (data.truth_state === "no_snapshot" || data.truth_state === "not_wired") {
        // "no_snapshot": execution loop not running — denial truth entirely absent.
        // "not_wired":   execution loop running but denial accumulator not yet
        //   implemented; [] would falsely claim authoritative zero denials.
        // Both states are fail-closed: emit as failed probe so endpoint lands in
        // missingEndpoints and isMissingPanelTruth fires → risk panel blocks.
        return { ok: false, endpoint: canonical.endpoint, error: "no_denial_truth" };
      }
      // Only "active" (future: denial accumulator wired and proven) passes through.
      return { ok: true, endpoint: canonical.endpoint, data: data.denials };
    })(),
    fetchJsonCandidates<ReconcileSummary>(["/api/v1/reconcile/status"]),
    (async (): Promise<EndpointFetchResult<ReconcileMismatchRow[]>> => {
      const canonical = await fetchJsonCandidate<ReconcileMismatchesResponse>("/api/v1/reconcile/mismatches");
      if (!canonical.ok) {
        return { ok: false, endpoint: canonical.endpoint, error: canonical.error ?? "fetch_failed" };
      }
      const data = canonical.data as ReconcileMismatchesResponse;
      // Reconcile mismatch rows are a live derived truth surface, not a durable
      // table read. `no_snapshot` / `stale` must therefore fail closed so empty
      // rows never masquerade as authoritative zero mismatches.
      if (data.truth_state === "no_snapshot" || data.truth_state === "stale") {
        return { ok: false, endpoint: canonical.endpoint, error: "no_reconcile_detail_truth" };
      }
      return { ok: true, endpoint: canonical.endpoint, data: data.rows };
    })(),
    // strategy/summary: "not_wired" is passed through ok:true so truthRendering.ts
    // can surface it as an explicit not_wired render state (feature mounted, not wired).
    // "no_db" = DB unavailable → fail-closed: emit as ok:false so the endpoint lands
    // in missingEndpoints, isMissingPanelTruth fires, and the strategy panel blocks
    // with no_snapshot instead of rendering an empty row set as authoritative.
    (async (): Promise<EndpointFetchResult<StrategySummaryWrapper>> => {
      const r = await fetchJsonCandidate<StrategySummaryWrapper>("/api/v1/strategy/summary");
      if (!r.ok || r.data == null) return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };
      const wrapper = r.data as StrategySummaryWrapper;
      if (wrapper.truth_state === "no_db") {
        // DB unavailable → fail-closed: endpoint lands in missingEndpoints so
        // isMissingPanelTruth fires and the strategy panel blocks with no_snapshot.
        return { ok: false, endpoint: r.endpoint, error: "strategy_registry_unavailable" };
      }
      return { ok: true, endpoint: r.endpoint, data: wrapper };
    })(),
    // alerts/active: daemon returns ActiveAlertsResponse wrapper (not a bare array).
    // truth_state is always "active" per spec; fail-closed defensively on any other value.
    // Map ActiveAlertRow[] → OperatorAlert[] using canonical field names.
    // Prior to this fix the fetch was typed as OperatorAlert[] (array), so Array.isArray()
    // on the wrapper object always returned false → silent empty array (fake-healthy state).
    (async (): Promise<EndpointFetchResult<OperatorAlert[]>> => {
      const r = await fetchJsonCandidate<ActiveAlertsWrapper>("/api/v1/alerts/active");
      if (!r.ok || r.data == null) {
        return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };
      }
      const wrapper = r.data as ActiveAlertsWrapper;
      // Daemon spec: truth_state is always "active". Guard fail-closed anyway.
      if (wrapper.truth_state !== "active") {
        return { ok: false, endpoint: r.endpoint, error: "alerts_truth_unavailable" };
      }
      return { ok: true, endpoint: r.endpoint, data: mapActiveAlertsResponse(wrapper) };
    })(),
    // events/feed: daemon returns EventsFeedResponse wrapper (not a bare array).
    // truth_state "backend_unavailable" = no DB pool → fail closed (empty feed must not
    // render as authoritative "no events"). "active" → map EventFeedRow[] → FeedEvent[].
    // Prior to this fix the fetch was typed as FeedEvent[] → silent empty feed always.
    (async (): Promise<EndpointFetchResult<FeedEvent[]>> => {
      const r = await fetchJsonCandidate<EventsFeedWrapper>("/api/v1/events/feed");
      if (!r.ok || r.data == null) {
        return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };
      }
      const wrapper = r.data as EventsFeedWrapper;
      if (wrapper.truth_state === "backend_unavailable") {
        // No DB → empty rows not authoritative; fail closed so BottomEventRail shows
        // honest unavailable state instead of empty-as-healthy.
        return { ok: false, endpoint: r.endpoint, error: "feed_backend_unavailable" };
      }
      return { ok: true, endpoint: r.endpoint, data: mapEventsFeedResponse(wrapper) };
    })(),
    // audit/operator-actions: daemon returns {canonical_route, truth_state, backend, rows}.
    // "backend_unavailable" means durable operator-action history is unavailable and
    // must fail closed rather than render as authoritative empty history.
    (async (): Promise<EndpointFetchResult<AuditActionRow[]>> => {
      const r = await fetchJsonCandidate<DaemonAuditActionsWrapper>("/api/v1/audit/operator-actions");
      if (!r.ok || r.data == null) return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };

      const wrapper = r.data as DaemonAuditActionsWrapper;
      if (wrapper.truth_state !== "active") {
        return { ok: false, endpoint: r.endpoint, error: "operator_history_backend_unavailable" };
      }

      const rows: AuditActionRow[] = wrapper.rows.map((row) => ({
        audit_ref: row.audit_event_id,
        at: row.ts_utc,
        action_key: row.requested_action,
        result_state: row.disposition,
        // target_scope: use run_id when present (durable run reference), else provenance_ref.
        target_scope: row.run_id ?? row.provenance_ref,
        warnings: [],
        // actor and environment are not available from audit_events per-row; omitted.
      }));
      return { ok: true, endpoint: r.endpoint, data: rows };
    })(),
    // A3: system/topology — mounted; truth_state always "active" (in-memory derivation).
    // Preserve wrapper so screens can inspect truth_state and services array.
    (async (): Promise<EndpointFetchResult<ServiceTopology>> => {
      const r = await fetchJsonCandidate<SystemTopologyWrapper>("/api/v1/system/topology");
      if (!r.ok || r.data == null) return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };
      const w = r.data as SystemTopologyWrapper;
      if (w.truth_state !== "active") return { ok: false, endpoint: r.endpoint, error: "topology_unavailable" };
      return { ok: true, endpoint: r.endpoint, data: { updated_at: w.updated_at, services: w.services as ServiceTopology["services"] } };
    })(),
    fetchJsonCandidates<TransportSummary>(["/api/v1/execution/transport"]),
    // A3: incidents — mounted; truth_state always "not_wired" (no incident manager).
    // Returns ok:false so "incidents" lands in usedMockSections (honest degraded authority).
    (async (): Promise<EndpointFetchResult<IncidentCase[]>> => {
      const r = await fetchJsonCandidate<IncidentsWrapper>("/api/v1/incidents");
      if (!r.ok || r.data == null) return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };
      const w = r.data as IncidentsWrapper;
      // "not_wired" = mounted but feature absent; must not render as authoritative empty.
      if (w.truth_state === "not_wired") return { ok: false, endpoint: r.endpoint, error: "incidents_not_wired" };
      return { ok: true, endpoint: r.endpoint, data: [] };
    })(),
    // A4: replace/cancel chains — mounted; truth_state always "not_wired" (no lineage tracking).
    (async (): Promise<EndpointFetchResult<ReplaceCancelChainRow[]>> => {
      const r = await fetchJsonCandidate<ReplaceCancelChainsWrapper>("/api/v1/execution/replace-cancel-chains");
      if (!r.ok || r.data == null) return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };
      const w = r.data as ReplaceCancelChainsWrapper;
      if (w.truth_state === "not_wired") return { ok: false, endpoint: r.endpoint, error: "replace_cancel_chains_not_wired" };
      return { ok: true, endpoint: r.endpoint, data: [] };
    })(),
    // A4: alerts/triage — mounted; truth_state "alerts_no_triage" (source real, lifecycle not).
    // Map daemon triage rows → AlertTriageRow[]. Passes ok:true because the alert source is real.
    (async (): Promise<EndpointFetchResult<AlertTriageRow[]>> => {
      const r = await fetchJsonCandidate<AlertTriageWrapper>("/api/v1/alerts/triage");
      if (!r.ok || r.data == null) return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };
      const w = r.data as AlertTriageWrapper;
      const rows: AlertTriageRow[] = (w.rows ?? []).map((row) => ({
        alert_id: row.alert_id,
        severity: row.severity as AlertTriageRow["severity"],
        status: row.status as AlertTriageRow["status"],
        title: row.title,
        domain: row.domain,
        linked_incident_id: row.linked_incident_id,
        linked_order_id: row.linked_order_id,
        linked_strategy_id: row.linked_strategy_id,
        created_at: row.created_at,
        assigned_to: row.assigned_to,
      }));
      return { ok: true, endpoint: r.endpoint, data: rows };
    })(),
    fetchJsonCandidates<SessionStateSummary>(["/api/v1/system/session"]),
    fetchJsonCandidates<ConfigFingerprintSummary>(["/api/v1/system/config-fingerprint"]),
    fetchJsonCandidates<MarketDataQualitySummary>(["/api/v1/market-data/quality"]),
    fetchJsonCandidates<RuntimeLeadershipSummary>(["/api/v1/system/runtime-leadership"]),
    // audit/artifacts: daemon returns {canonical_route, truth_state, backend, rows}
    // where each row is one run from the runs table. "backend_unavailable" means
    // durable artifact history is unavailable and must fail closed.
    (async (): Promise<EndpointFetchResult<ArtifactRegistrySummary>> => {
      const r = await fetchJsonCandidate<DaemonArtifactsWrapper>("/api/v1/audit/artifacts");
      if (!r.ok || r.data == null) return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };

      const wrapper = r.data as DaemonArtifactsWrapper;
      if (wrapper.truth_state !== "active") {
        return { ok: false, endpoint: r.endpoint, error: "operator_artifact_backend_unavailable" };
      }

      const artifacts: ArtifactRow[] = wrapper.rows.map((row) => ({
        artifact_id: row.artifact_id,
        artifact_type: row.artifact_type as ArtifactRow["artifact_type"],
        created_at: row.created_at_utc,
        status: "ready" as const,
        linked_order_id: null,
        linked_incident_id: null,
        linked_run_id: row.run_id,
        // storage_path and note are not available from the runs-table artifact source.
      }));

      // last_updated_at: newest artifact created_at (rows are already desc by started_at_utc).
      const lastUpdatedAt = artifacts.length > 0 ? artifacts[0].created_at : null;
      const summary: ArtifactRegistrySummary = {
        last_updated_at: lastUpdatedAt,
        ready_count: artifacts.length,
        pending_count: 0,
        failed_count: 0,
        artifacts,
      };
      return { ok: true, endpoint: r.endpoint, data: summary };
    })(),
    // strategy/suppressions: preserve the daemon wrapper so the GUI can render
    // mounted-but-not-wired truth distinctly from authoritative active-empty rows.
    (async (): Promise<EndpointFetchResult<StrategySuppressionsWrapper>> => {
      const r = await fetchJsonCandidate<StrategySuppressionsWrapper>("/api/v1/strategy/suppressions");
      if (!r.ok || r.data == null) return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };
      return { ok: true, endpoint: r.endpoint, data: r.data as StrategySuppressionsWrapper };
    })(),
    // system/config-diffs: preserve the daemon wrapper so the GUI can render
    // mounted-but-not-wired truth distinctly from authoritative active-empty rows.
    (async (): Promise<EndpointFetchResult<ConfigDiffsWrapper>> => {
      const r = await fetchJsonCandidate<ConfigDiffsWrapper>("/api/v1/system/config-diffs");
      if (!r.ok || r.data == null) return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };
      return { ok: true, endpoint: r.endpoint, data: r.data as ConfigDiffsWrapper };
    })(),
    // ops/operator-timeline: daemon returns {canonical_route, truth_state, backend, rows}
    // where each row is a runtime lifecycle transition or operator action from runs +
    // audit_events. "backend_unavailable" means durable operator timeline truth is
    // unavailable and must fail closed.
    (async (): Promise<EndpointFetchResult<OperatorTimelineEvent[]>> => {
      const r = await fetchJsonCandidate<DaemonOperatorTimelineWrapper>("/api/v1/ops/operator-timeline");
      if (!r.ok || r.data == null) return { ok: false, endpoint: r.endpoint, error: r.error ?? "fetch_failed" };

      const wrapper = r.data as DaemonOperatorTimelineWrapper;
      if (wrapper.truth_state !== "active") {
        return { ok: false, endpoint: r.endpoint, error: "operator_timeline_backend_unavailable" };
      }

      const events: OperatorTimelineEvent[] = wrapper.rows.map((row) => ({
        // provenance_ref is the durable DB reference (e.g. "runs:{id}:started_at_utc"
        // or "audit_events:{id}"); use as the stable event identity.
        timeline_event_id: row.provenance_ref,
        at: row.ts_utc,
        // "runtime_transition" and "operator_action" are the two kinds emitted by the daemon.
        category: row.kind as OperatorTimelineCategory,
        // "info" is the correct baseline severity for lifecycle and operator-action events;
        // no severity escalation data exists in the durable DB sources.
        severity: "info" as const,
        title: row.detail,
        summary: row.detail,
        // actor: not available in runs or audit_events per-row; omitted (optional field).
        linked_incident_id: null,
        linked_order_id: null,
        linked_strategy_id: null,
        linked_action_key: null,
        linked_config_diff_id: null,
        linked_runtime_generation_id: row.run_id ?? null,
      }));
      return { ok: true, endpoint: r.endpoint, data: events };
    })(),
    // Canonical Action Catalog: daemon-authoritative action availability.
    fetchJsonCandidates<DaemonActionCatalogResponse>(["/api/v1/ops/catalog"]),
    // GUI-OPS-02: Execution outbox — durable intent timeline for the active run.
    // truth_state "active" = DB + active run, rows are authoritative.
    // "no_active_run" / "no_db" = rows empty but not authoritative zero.
    // Returns structured surface (not plain array) so screens can inspect truth_state.
    fetchJsonCandidates<ExecutionOutboxWrapper>(["/api/v1/execution/outbox"]),
    // GUI-OPS-02: Fill quality telemetry for the active run (TV-EXEC-01).
    fetchJsonCandidates<FillQualityWrapper>(["/api/v1/execution/fill-quality"]),
    // GUI-OPS-01: Paper journal — fills_lane + admissions_lane with independent truth_states.
    fetchJsonCandidates<PaperJournalWrapper>(["/api/v1/paper/journal"]),
  ]);

  const [
    preflightR,
    executionSummaryR,
    executionOrdersR,
    omsOverviewR,
    metricsR,
    portfolioSummaryR,
    positionsR,
    openOrdersR,
    fillsR,
    riskSummaryR,
    riskDenialsR,
    reconcileSummaryR,
    mismatchesR,
    strategiesR,
    alertsR,
    feedR,
    auditActionsR,
    topologyR,
    transportR,
    incidentsR,
    replaceCancelChainsR,
    alertTriageR,
    sessionStateR,
    configFingerprintR,
    marketDataQualityR,
    runtimeLeadershipR,
    artifactRegistryR,
    strategySuppressionsR,
    configDiffsR,
    operatorTimelineR,
    actionCatalogR,
    outboxR,
    fillQualityR,
    paperJournalR,
  ] = probes;

  const daemonReachable = statusProbe.ok || healthProbe.ok || Boolean(legacyStatusFromProbe) || probes.some((p) => p.ok);
  const connected = daemonReachable;

  const legacyOrdersResponse = executionOrdersR.ok && executionOrdersR.endpoint === "/v1/trading/orders"
    ? (executionOrdersR.data as LegacyTradingOrdersResponse)
    : openOrdersR.ok && openOrdersR.endpoint === "/v1/trading/orders"
      ? (openOrdersR.data as LegacyTradingOrdersResponse)
      : null;

  const legacyPositionsResponse = positionsR.ok && positionsR.endpoint === "/v1/trading/positions"
    ? (positionsR.data as LegacyTradingPositionsResponse)
    : null;

  const legacyAccountResponse = portfolioSummaryR.ok && portfolioSummaryR.endpoint === "/v1/trading/account"
    ? (portfolioSummaryR.data as LegacyTradingAccountResponse)
    : null;

  const legacyFillsResponse = fillsR.ok && fillsR.endpoint === "/v1/trading/fills"
    ? (fillsR.data as LegacyTradingFillsResponse)
    : null;

  // HTTP 503 from /api/v1/execution/orders = explicit no-snapshot signal.
  // In that case the probe stays as { ok: false, endpoint: "/api/v1/execution/orders" }
  // so the endpoint lands in missingEndpoints, isMissingPanelTruth fires, and the
  // execution panel blocks.  Do NOT use legacy orders to fill this gap.
  const executionOrdersIsNoSnapshot = !executionOrdersR.ok && executionOrdersR.error === "HTTP 503";
  const executionOrdersCanonical = executionOrdersR.ok && Array.isArray(executionOrdersR.data);
  const executionOrders = executionOrdersCanonical
    ? (executionOrdersR.data as ExecutionOrderRow[])
    : executionOrdersIsNoSnapshot
      ? []  // No-snapshot signal: return empty rather than misleading legacy broker orders.
      : mapLegacyTradingOrdersToExecutionOrders(legacyOrdersResponse) ?? [];

  // GUI-CONTRACT-02: per-order detail endpoints are not yet mounted on the
  // daemon. They are not probed in the batch model assembly to avoid 404
  // noise on every refresh cycle and to stop polluting usedMockSections
  // whenever any order exists (which degraded execution panel authority
  // regardless of whether the detail views were in use).
  // Dedicated exported functions (fetchExecutionTimeline etc.) remain
  // available for screen-level calls when these routes are eventually mounted.
  const selectedTimeline: OrderTimelineSurface | null = null;
  const executionTrace: OrderTraceResponse | null = null;
  const executionReplay: OrderReplayResponse | null = null;
  const executionChart: OrderChartResponse | null = null;
  const causalityTrace: OrderCausalityResponse | null = null;

  const usedMockSections: string[] = [];

  // When system status resolved via legacy (/v1/status) instead of canonical
  // (/api/v1/system/status), mark it as non-canonical. "status" is in the ops
  // panel's placeholder evidence hints — this push ensures the ops authority
  // classification degrades to "placeholder" (or "mixed") so the truth gate
  // fires and the action catalog is not shown on approximate legacy data.
  if (!statusCanonical) usedMockSections.push("status");
  // executionOrders non-canonical in two cases:
  //   1. Legacy path used (/v1/trading/orders): fabricated strategy_id/stage → "mixed" authority.
  //   2. No-snapshot signal (HTTP 503): canonical endpoint in missingEndpoints; "executionOrders"
  //      in mockSections. Combined with missingEndpoints, isMissingPanelTruth fires for the
  //      execution panel → no_snapshot gate blocks the screen.
  if (!executionOrdersCanonical) usedMockSections.push("executionOrders");

  const durableOperatorHistoryKeys = new Set(["auditActions", "artifactRegistry", "operatorTimeline"]);

  const explicitSurfaceTruthOrUnknown = (
    result: EndpointFetchResult<{ truth_state: "active" | "not_wired" | "no_db" | "registry"; backend?: string | null }>,
  ): ExplicitSurfaceTruth => {
    if (!result.ok || result.data == null) {
      return { truth_state: "unknown", backend: null };
    }
      return {
      truth_state: result.data.truth_state === "registry" ? "active" : result.data.truth_state,
      backend: result.data.backend ?? null,
    };
  };

  const useObject = <T,>(key: string, result: EndpointFetchResult<T>, fallback: T): T => {
    if (result.ok && result.data !== undefined) return result.data;

    // Durable operator-history endpoints are mounted but can explicitly report
    // backend_unavailable. That condition must surface through missingEndpoints
    // as unavailable durable truth, not be relabeled as placeholder/mock wiring.
    if (!durableOperatorHistoryKeys.has(key)) {
      usedMockSections.push(key);
    }

    return fallback;
  };
  const useArray = <T,>(key: string, result: EndpointFetchResult<T[]>, fallback: T[]): T[] => {
    if (result.ok && Array.isArray(result.data)) return result.data;

    // Same rule as useObject above: missing durable operator history is a
    // fail-closed truth gap, not a placeholder surface.
    if (!durableOperatorHistoryKeys.has(key)) {
      usedMockSections.push(key);
    }

    return fallback;
  };

  const executionSummary =
    executionSummaryR.ok && executionSummaryR.data !== undefined
      ? executionSummaryR.data
      : deriveExecutionSummaryFromOrders(executionOrders);
  // Push when canonical failed (even if derived summary is non-null from legacy orders).
  if (!executionSummary || !executionSummaryR.ok) usedMockSections.push("executionSummary");

  const portfolioSummaryCanonical =
    portfolioSummaryR.ok &&
    portfolioSummaryR.endpoint === "/api/v1/portfolio/summary" &&
    portfolioSummaryR.data !== undefined;
  const portfolioSummary = portfolioSummaryCanonical
    ? (portfolioSummaryR.data as PortfolioSummary)
    : mapLegacyPortfolioSummary(legacyAccountResponse);
  if (!portfolioSummary || !portfolioSummaryCanonical) usedMockSections.push("portfolioSummary");

  // Canonical portfolio routes return a structured wrapper with snapshot_state.
  // "active" = broker snapshot present; rows are authoritative (may be empty).
  // "no_snapshot" = broker snapshot absent; rows are empty, must not be read as truth.
  // The check is on the typed snapshot_state field — not HTTP status string matching.
  const positionsIsCanonical =
    positionsR.ok && positionsR.endpoint === "/api/v1/portfolio/positions";
  const positionsIsActive =
    positionsIsCanonical &&
    (positionsR.data as PortfolioPositionsResponse).snapshot_state === "active";
  const positions = positionsIsActive
    ? (positionsR.data as PortfolioPositionsResponse).rows
    : mapLegacyPositionsResponse(legacyPositionsResponse);
  if (!positionsIsActive) usedMockSections.push("positions");

  const openOrdersIsCanonical =
    openOrdersR.ok && openOrdersR.endpoint === "/api/v1/portfolio/orders/open";
  const openOrdersIsActive =
    openOrdersIsCanonical &&
    (openOrdersR.data as PortfolioOpenOrdersResponse).snapshot_state === "active";
  const openOrders = openOrdersIsActive
    ? (openOrdersR.data as PortfolioOpenOrdersResponse).rows
    : mapLegacyTradingOrdersToOpenOrders(legacyOrdersResponse);
  if (!openOrdersIsActive) usedMockSections.push("openOrders");

  const fillsIsCanonical =
    fillsR.ok && fillsR.endpoint === "/api/v1/portfolio/fills";
  const fillsIsActive =
    fillsIsCanonical &&
    (fillsR.data as PortfolioFillsResponse).snapshot_state === "active";
  const fills = fillsIsActive
    ? (fillsR.data as PortfolioFillsResponse).rows
    : mapLegacyTradingFillsToRows(legacyFillsResponse);
  if (!fillsIsActive) usedMockSections.push("fills");

  const metadata =
    healthProbe.ok && healthProbe.data !== undefined
      ? (healthProbe.data as MetadataSummary)
      : null;
  if (!metadata) usedMockSections.push("metadata");

  // GUI-CONTRACT-02: selectedTimeline/executionTrace/executionReplay/
  // executionChart/causalityTrace are always null in the batch model (not
  // probed — see above). They are NOT added to usedMockSections because
  // their absence should not degrade core execution panel authority.

  // Resolve action catalog BEFORE dataSource computation so a catalog endpoint failure
  // is visible in dataSource.mockSections and properly degrades the ops panel authority.
  const catalogResult = (() => {
    if (actionCatalogR.ok && actionCatalogR.data !== undefined) {
      const raw = actionCatalogR.data as DaemonActionCatalogResponse;
      if (raw.actions && Array.isArray(raw.actions)) {
        return mapDaemonCatalog(raw);
      }
    }
    return null;
  })();

  const strategySummaryTruth = explicitSurfaceTruthOrUnknown(strategiesR as EndpointFetchResult<StrategySummaryWrapper>);
  const strategySuppressionsTruth = explicitSurfaceTruthOrUnknown(strategySuppressionsR as EndpointFetchResult<StrategySuppressionsWrapper>);
  const configDiffsTruth = explicitSurfaceTruthOrUnknown(configDiffsR as EndpointFetchResult<ConfigDiffsWrapper>);

  // B2B: fail closed on "no_db" truth state — do not treat empty rows as authoritative.
  // "registry" = authoritative; rows may include synthetic "blocked_not_registered" entries.
  // Legacy "active" accepted; "not_wired" / "no_db" → empty (fail closed).
  const strategies = (() => {
    if (!strategiesR.ok || strategiesR.data == null) return [];
    const wrapper = strategiesR.data as StrategySummaryWrapper;
    if (wrapper.truth_state === "no_db" || wrapper.truth_state === "not_wired") return [];
    return wrapper.rows;
  })();
  const strategySuppressions = strategySuppressionsR.ok && strategySuppressionsR.data !== undefined
    ? (strategySuppressionsR.data as StrategySuppressionsWrapper).rows
    : [];
  const configDiffs = configDiffsR.ok && configDiffsR.data !== undefined
    ? (configDiffsR.data as ConfigDiffsWrapper).rows
    : [];
  // null means the endpoint was unreachable or returned an invalid shape.
  // An empty array is valid and means no actions are currently available (e.g., all halted).
  if (catalogResult === null) {
    usedMockSections.push("actionCatalog");
  }
  const resolvedActionCatalog: OperatorActionDefinition[] = catalogResult ?? [];

  // GUI-OPS-02: Execution outbox / fill quality surfaces — via extracted mapper functions
  // (see legacy.ts). Mappers handle truth_state canonicalization and null-safety.
  const executionOutbox = mapExecutionOutboxWrapper(outboxR.ok && outboxR.data != null ? outboxR.data as ExecutionOutboxWrapper : null);
  const fillQualityTelemetry = mapFillQualityWrapper(fillQualityR.ok && fillQualityR.data != null ? fillQualityR.data as FillQualityWrapper : null);
  // GUI-OPS-01: Paper journal surface — dual-lane via extracted mapper.
  const paperJournal = mapPaperJournalWrapper(paperJournalR.ok && paperJournalR.data != null ? paperJournalR.data as PaperJournalWrapper : null);

  const dataSource = deriveDataSourceDetail({
    probeResults: [statusProbe, healthProbe, ...probes],
    usedMockSections,
    daemonReachable,
  });

  const unavailableStatus: SystemStatus = {
    ...DEFAULT_STATUS,
    daemon_reachable: connected,
  };
  const unavailablePreflight: PreflightStatus = {
    ...DEFAULT_PREFLIGHT,
    daemon_reachable: connected,
    blockers: connected ? ["Backend preflight truth unavailable"] : ["Daemon unreachable"],
  };
  const unavailableExecutionSummary: ExecutionSummary = {
    active_orders: 0,
    pending_orders: 0,
    dispatching_orders: 0,
    reject_count_today: 0,
    cancel_replace_count_today: 0,
    avg_ack_latency_ms: null,
    stuck_orders: 0,
  };
  const unavailableOmsOverview: OmsOverview = {
    total_active_orders: 0,
    stuck_orders: 0,
    missing_transition_orders: 0,
    state_nodes: [],
    transition_edges: [],
    orders: [],
  };
  const unavailableMetrics: SystemMetrics = {
    runtime: { key: "runtime", title: "Runtime", description: "Backend truth unavailable", series: [] },
    execution: { key: "execution", title: "Execution", description: "Backend truth unavailable", series: [] },
    fillQuality: { key: "fill_quality", title: "Fill Quality", description: "Backend truth unavailable", series: [] },
    reconciliation: { key: "reconciliation", title: "Reconciliation", description: "Backend truth unavailable", series: [] },
    riskSafety: { key: "risk_safety", title: "Risk/Safety", description: "Backend truth unavailable", series: [] },
  };
  const unavailablePortfolioSummary: PortfolioSummary = {
    account_equity: 0,
    cash: 0,
    long_market_value: 0,
    short_market_value: 0,
    daily_pnl: 0,
    buying_power: 0,
  };
  const unavailableRiskSummary: RiskSummary = {
    gross_exposure: 0,
    net_exposure: 0,
    concentration_pct: 0,
    daily_pnl: 0,
    drawdown_pct: 0,
    loss_limit_utilization_pct: 0,
    kill_switch_active: false,
    active_breaches: 0,
  };
  const unavailableReconcileSummary: ReconcileSummary = {
    status: "unknown",
    last_run_at: null,
    mismatched_positions: 0,
    mismatched_orders: 0,
    mismatched_fills: 0,
    unmatched_broker_events: 0,
  };
  const unavailableMetadata: MetadataSummary = {
    build_version: "unknown",
    api_version: "unknown",
    broker_adapter: "unknown",
    endpoint_status: "unknown",
  };
  const unavailableTopology: ServiceTopology = { updated_at: nowIso(), services: [] };
  const unavailableTransport: TransportSummary = {
    outbox_depth: 0,
    inbox_depth: 0,
    max_claim_age_ms: 0,
    dispatch_retries: 0,
    orphaned_claims: 0,
    duplicate_inbox_events: 0,
    queues: [],
  };
  const unavailableSessionState: SessionStateSummary = {
    market_session: "closed",
    exchange_calendar_state: "closed",
    system_trading_window: "disabled",
    strategy_allowed: false,
    next_session_change_at: null,
    notes: connected ? ["Backend session truth unavailable"] : ["Daemon unreachable"],
    // AP-09: deployment identity fields are absent when daemon session truth is
    // unavailable.  Leave optional fields undefined rather than asserting stale
    // values — the caller must not infer mode/adapter from unavailable state.
    deployment_start_allowed: false,
  };
  const unavailableConfigFingerprint: ConfigFingerprintSummary = {
    config_hash: "unknown",
    risk_policy_version: "unknown",
    strategy_bundle_version: "unknown",
    build_version: "unknown",
    environment_profile: "unknown",
    runtime_generation_id: "unknown",
    last_restart_at: null,
  };
  const unavailableMarketDataQuality: MarketDataQualitySummary = {
    overall_health: "unknown",
    freshness_sla_ms: 0,
    stale_symbol_count: 0,
    missing_bar_count: 0,
    venue_disagreement_count: 0,
    strategy_blocks: 0,
    venues: [],
    issues: [],
  };
  const unavailableRuntimeLeadership: RuntimeLeadershipSummary = {
    leader_node: "unknown",
    leader_lease_state: "lost",
    generation_id: "unknown",
    restart_count_24h: null, // unavailable: daemon not reachable, no authoritative count
    last_restart_at: null,
    // Use "in_progress" not "degraded": "degraded" in panelTruthRenderState
    // triggers a system-wide degraded overlay on every panel.  The fallback
    // represents missing truth, not a real degraded recovery state.
    post_restart_recovery_state: "in_progress",
    recovery_checkpoint: "unknown",
    checkpoints: [],
  };
  const unavailableArtifactRegistry: ArtifactRegistrySummary = {
    last_updated_at: null,
    ready_count: 0,
    pending_count: 0,
    failed_count: 0,
    artifacts: [],
  };

  const resolvedStatus: SystemStatus = objectOrFallback(
    statusProbe.ok && statusProbe.endpoint === "/api/v1/system/status"
      ? statusProbe.data
      // statusProbe already tried /v1/status as a fallback before giving up;
      // if it resolved there, use the legacy mapping.  No additional fetch.
      : legacyStatusFromProbe
        ? mapLegacyStatusToSystemStatus(legacyStatusFromProbe)
        : null,
    unavailableStatus,
  );

  return withClassifiedPanelSources({
    status: resolvedStatus,
    // Preflight is fail-closed: if the canonical endpoint did not return a
    // valid response, surface explicit unavailable state rather than a
    // silently-derived fake preflight with no blockers.
    preflight: objectOrFallback(
      preflightR.ok ? preflightR.data : null,
      unavailablePreflight,
    ),
    executionSummary: executionSummary ?? unavailableExecutionSummary,
    executionOrders,
    selectedTimeline,
    omsOverview: useObject("omsOverview", omsOverviewR, unavailableOmsOverview),
    executionTrace,
    executionReplay,
    executionChart,
    causalityTrace,
    metrics: useObject("metrics", metricsR, unavailableMetrics),
    portfolioSummary: portfolioSummary ?? unavailablePortfolioSummary,
    positions: positions ?? [],
    openOrders: openOrders ?? [],
    fills: fills ?? [],
    riskSummary: useObject("riskSummary", riskSummaryR, unavailableRiskSummary),
    riskDenials: useArray("riskDenials", riskDenialsR, []),
    reconcileSummary: useObject("reconcileSummary", reconcileSummaryR, unavailableReconcileSummary),
    mismatches: useArray("mismatches", mismatchesR, []),
    strategies,
    alerts: useArray("alerts", alertsR, []),
    feed: useArray("feed", feedR, []),
    auditActions: useArray("auditActions", auditActionsR, []),
    metadata: metadata ?? unavailableMetadata,
    topology: useObject("topology", topologyR, unavailableTopology),
    transport: useObject("transport", transportR, unavailableTransport),
    incidents: useArray("incidents", incidentsR, []),
    replaceCancelChains: useArray("replaceCancelChains", replaceCancelChainsR, []),
    alertTriage: useArray("alertTriage", alertTriageR, []),
    sessionState: useObject("sessionState", sessionStateR, unavailableSessionState),
    configFingerprint: useObject("configFingerprint", configFingerprintR, unavailableConfigFingerprint),
    marketDataQuality: useObject("marketDataQuality", marketDataQualityR, unavailableMarketDataQuality),
    runtimeLeadership: useObject("runtimeLeadership", runtimeLeadershipR, unavailableRuntimeLeadership),
    artifactRegistry: useObject("artifactRegistry", artifactRegistryR, unavailableArtifactRegistry),
    strategySummaryTruth,
    strategySuppressionsTruth,
    configDiffsTruth,
    strategySuppressions,
    configDiffs,
    operatorTimeline: useArray("operatorTimeline", operatorTimelineR, []),
    // Daemon-authoritative Action Catalog from GET /api/v1/ops/catalog.
    // Resolved before dataSource so catalog failures reach dataSource.mockSections
    // and degrade the ops panel truth authority correctly.
    actionCatalog: resolvedActionCatalog,
    // GUI-OPS-02/01: New truthful surfaces.
    executionOutbox,
    fillQualityTelemetry,
    paperJournal,
    dataSource,
    connected,
    lastUpdatedAt: nowIso(),
  });
}

// ---------------------------------------------------------------------------
// Order-detail fetches (per-order deep-dive surfaces)
// ---------------------------------------------------------------------------

// A5A: fetch per-order execution timeline from the canonical daemon route.
// Returns null only on network/fetch failure. no_db is preserved as a typed
// surface so the screen can render an explicit unavailable-truth notice.
export async function fetchExecutionTimeline(internalOrderId: string): Promise<OrderTimelineSurface | null> {
  const r = await fetchJsonCandidate<DaemonOrderTimelineResponse>(
    `/api/v1/execution/orders/${internalOrderId}/timeline`,
  );
  if (!r.ok || r.data == null) return null;
  const d = r.data;
  const VALID_STATES: OrderTimelineTruthState[] = ["active", "no_fills_yet", "no_order", "no_db"];
  const truth_state: OrderTimelineTruthState = VALID_STATES.includes(
    d.truth_state as OrderTimelineTruthState,
  )
    ? (d.truth_state as OrderTimelineTruthState)
    : "no_order";
  return {
    canonical_route: d.canonical_route,
    truth_state,
    backend: d.backend,
    internal_order_id: d.order_id,
    broker_order_id: d.broker_order_id ?? null,
    symbol: d.symbol ?? null,
    strategy_id: null,
    requested_qty: d.requested_qty ?? null,
    filled_qty: d.filled_qty ?? null,
    current_status: d.current_status ?? null,
    current_stage: d.current_stage ?? null,
    last_updated_at: d.last_event_at ?? null,
    rows: d.rows ?? [],
  };
}

export async function fetchExecutionTrace(internalOrderId: string): Promise<OrderTraceResponse | null> {
  return tryFetchJson<OrderTraceResponse>([`/api/v1/execution/orders/${internalOrderId}/trace`]);
}

export async function fetchExecutionReplay(internalOrderId: string): Promise<OrderReplayResponse | null> {
  return tryFetchJson<OrderReplayResponse>([`/api/v1/execution/orders/${internalOrderId}/replay`]);
}

export async function fetchExecutionChart(internalOrderId: string): Promise<OrderChartResponse | null> {
  return tryFetchJson<OrderChartResponse>([`/api/v1/execution/orders/${internalOrderId}/chart`]);
}

export async function fetchCausalityTrace(internalOrderId: string): Promise<OrderCausalityResponse | null> {
  return tryFetchJson<OrderCausalityResponse>([`/api/v1/execution/orders/${internalOrderId}/causality`]);
}

// requestSystemModeTransition was removed (H-7 / PC-1):
// /api/v1/ops/change-mode is intentionally NOT mounted on the daemon.
// Mode transitions require a controlled restart with configuration reload.
// The change-system-mode action key returns 409 from /api/v1/ops/action
// as a defense-in-depth rejection. Callers were removed in H-7.
