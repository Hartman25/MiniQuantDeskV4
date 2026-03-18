import {
  CORE_PANEL_KEYS,
  type CorePanelKey,
  type DataSourceDetail,
  type PanelSourceMap,
  type SourceAuthority,
  type SystemModel,
} from "./types.ts";

type EvidenceSignal = {
  hasDb: boolean;
  hasRuntime: boolean;
  hasBroker: boolean;
  hasPlaceholder: boolean;
};

type PanelEvidenceHints = {
  db: string[];
  runtime: string[];
  broker: string[];
  placeholder: string[];
};

export type FieldEvidenceHints = PanelEvidenceHints;

// Coarse field-level provenance hints for surfaces whose route semantics changed.
// These intentionally prefer conservative mixed classification over falsely pure
// labels when a single route combines durable state with live in-memory truth.
export const FIELD_EVIDENCE_HINTS: Record<
  "riskSummary" | "riskDenials" | "reconcileSummary" | "reconcileMismatches" | "runtimeLeadership",
  FieldEvidenceHints
> = {
  // /risk/summary combines broker snapshot exposure with durable risk-block state.
  riskSummary: {
    db: ["/risk/summary"],
    runtime: [],
    broker: ["/risk/summary"],
    placeholder: ["riskSummary"],
  },
  // /risk/denials is sourced from sys_risk_denial_events (durable DB table,
  // migration 0026) when a DB pool is available.  Falls back to the in-memory
  // ring buffer (truth_state = "active_session_only") in no-pool environments.
  riskDenials: {
    db: ["/risk/denials"],
    runtime: ["/risk/denials"],
    broker: [],
    placeholder: ["riskDenials"],
  },
  // /reconcile/status loads durable reconcile state when DB is available and
  // falls back to in-memory reconcile status otherwise.
  reconcileSummary: {
    db: ["/reconcile/status"],
    runtime: ["/reconcile/status"],
    broker: [],
    placeholder: ["reconcileSummary"],
  },
  // /reconcile/mismatches derives rows at request time from the current
  // execution snapshot plus the current broker snapshot; this is not a durable
  // mismatch table.
  reconcileMismatches: {
    db: [],
    runtime: ["/reconcile/mismatches"],
    broker: ["/reconcile/mismatches"],
    placeholder: ["mismatches"],
  },
  // /system/runtime-leadership is derived from runtime status plus durable run
  // and reconcile evidence when those records are available.
  runtimeLeadership: {
    db: ["/system/runtime-leadership"],
    runtime: ["/system/runtime-leadership"],
    broker: [],
    placeholder: ["runtimeLeadership"],
  },
};

const PANEL_EVIDENCE_HINTS: Record<CorePanelKey, PanelEvidenceHints> = {
  // Dashboard is a summary of multiple source types: inherently mixed in a healthy system.
  dashboard: {
    db: ["/reconcile/status"],
    runtime: ["/system/status", "/system/preflight", "/execution/summary"],
    broker: ["/portfolio/summary", "/trading/account"],
    placeholder: ["status", "preflight", "portfolioSummary"],
  },
  // Metrics endpoint is deferred; when implemented it will be runtime daemon state.
  metrics: {
    db: [],
    runtime: ["/metrics/dashboards"],
    broker: [],
    placeholder: ["metrics"],
  },
  // Canonical orders/summary come from OMS (runtime). Legacy /trading/orders is broker snapshot.
  // Timeline/trace/replay are DB-backed audit artifacts.
  execution: {
    db: ["/execution/timeline", "/execution/trace", "/execution/replay"],
    runtime: ["/execution/summary", "/execution/orders"],
    broker: ["/trading/orders"],
    placeholder: ["executionSummary", "executionOrders"],
  },
  // Risk summary is derived from broker_snapshot (runtime memory).
  // Risk denials are sourced from execution_snapshot (runtime memory, not DB-persisted).
  // Both surfaces are runtime truth; the panel authority resolves to runtime_memory
  // when both endpoints are in realEndpoints.
  risk: {
    db: [],
    runtime: ["/risk/summary", "/risk/denials", "/system/status"],
    broker: [],
    placeholder: ["riskSummary", "riskDenials"],
  },
  // Portfolio data originates from the broker (account/positions/orders/fills are broker snapshot).
  portfolio: {
    db: [],
    runtime: [],
    broker: ["/portfolio/summary", "/portfolio/positions", "/portfolio/orders/open", "/portfolio/fills", "/trading/account", "/trading/positions", "/trading/fills"],
    placeholder: ["portfolioSummary", "positions", "openOrders", "fills"],
  },
  // Reconcile records are persisted in Postgres — always DB truth.
  reconcile: {
    db: ["/reconcile/status", "/reconcile/mismatches"],
    runtime: [],
    broker: [],
    placeholder: ["reconcileSummary", "mismatches"],
  },
  // Strategy summary and suppressions are both "not_wired": no real strategy-fleet
  // registry or suppression persistence exists yet.  Both IIFEs return ok:false,
  // so neither endpoint lands in realEndpoints.  "strategies" and
  // "strategySuppressions" land in mockSections → hasPlaceholder=true, realCount=0
  // → authority resolves to "placeholder" → panelTruthRenderState returns
  // "unimplemented" and the StrategyScreen hard-blocks.
  // The evidence hints are kept in their intended final-state form (db for
  // suppressions, runtime for summary) so that when a real source is wired in a
  // future patch, the authority classification requires no change here.
  strategy: {
    db: ["/strategy/suppressions"],
    runtime: ["/strategy/summary"],
    broker: [],
    placeholder: ["strategies", "strategySuppressions"],
  },
  // Audit surfaces are always DB-backed (postgres.audit_events / postgres.artifacts).
  audit: {
    db: ["/audit/operator-actions", "/audit/artifacts", "/ops/operator-timeline"],
    runtime: [],
    broker: [],
    placeholder: ["auditActions", "artifactRegistry", "operatorTimeline"],
  },
  // Ops panel mixes runtime status with DB operator history and config diffs.
  // /ops/catalog is daemon runtime state: availability changes with each runtime state change.
  ops: {
    db: ["/ops/operator-timeline", "/system/config-diffs"],
    runtime: ["/system/status", "/system/preflight", "/ops/catalog"],
    broker: [],
    placeholder: ["status", "preflight", "configDiffs", "operatorTimeline", "actionCatalog"],
  },
  // Config fingerprint and metadata are daemon runtime state. Config diffs are DB-persisted.
  settings: {
    db: ["/system/config-diffs"],
    runtime: ["/system/metadata", "/system/config-fingerprint", "/system/runtime-leadership"],
    broker: [],
    placeholder: ["metadata", "configFingerprint", "runtimeLeadership", "configDiffs"],
  },
  // Topology is daemon runtime state — no DB backing.
  topology: {
    db: [],
    runtime: ["/system/topology"],
    broker: [],
    placeholder: ["topology"],
  },
  // Transport is daemon runtime state (outbox/inbox depth from execution layer).
  transport: {
    db: [],
    runtime: ["/execution/transport"],
    broker: [],
    placeholder: ["transport"],
  },
  // Incidents are persisted records (DB).
  incidents: {
    db: ["/incidents"],
    runtime: [],
    broker: [],
    placeholder: ["incidents"],
  },
  // Active alerts are live daemon state (runtime). Triage records are DB-persisted.
  alerts: {
    db: ["/alerts/triage"],
    runtime: ["/alerts/active", "/system/status"],
    broker: [],
    placeholder: ["alerts", "alertTriage"],
  },
  // Session state (trading windows, calendar) is daemon runtime — no DB.
  session: {
    db: [],
    runtime: ["/system/session", "/system/status"],
    broker: [],
    placeholder: ["sessionState"],
  },
  // Config fingerprint and runtime leadership are daemon runtime state. Diffs are DB.
  config: {
    db: ["/system/config-diffs"],
    runtime: ["/system/config-fingerprint", "/system/runtime-leadership"],
    broker: [],
    placeholder: ["configFingerprint", "runtimeLeadership", "configDiffs"],
  },
  // Market data quality assessment is daemon runtime state.
  marketData: {
    db: [],
    runtime: ["/market-data/quality", "/system/status"],
    broker: [],
    placeholder: ["marketDataQuality"],
  },
  // Runtime leadership is primarily daemon runtime state.  restart_count_24h is
  // DB-backed (runs table, started_at_utc > now()-24h) and returns null when DB is absent.
  runtime: {
    db: [],
    runtime: ["/system/runtime-leadership", "/system/status", "/system/preflight"],
    broker: [],
    placeholder: ["runtimeLeadership", "status", "preflight"],
  },
  // Artifact registry is DB-backed (audit artifact records).
  artifacts: {
    db: ["/audit/artifacts", "/ops/operator-timeline"],
    runtime: [],
    broker: [],
    placeholder: ["artifactRegistry", "operatorTimeline"],
  },
  // Operator timeline is durable DB audit log.
  operatorTimeline: {
    db: ["/ops/operator-timeline", "/audit/operator-actions"],
    runtime: [],
    broker: [],
    placeholder: ["operatorTimeline", "auditActions"],
  },
};

function hasEndpoint(realEndpoints: string[], hints: string[]) {
  return hints.some((hint) => realEndpoints.some((endpoint) => endpoint.includes(hint)));
}

function hasMockSection(mockSections: string[], hints: string[]) {
  return mockSections.includes("all") || hints.some((hint) => mockSections.includes(hint));
}

export function classifyAuthority(signal: EvidenceSignal, connected: boolean): SourceAuthority {
  if (!connected) return "unknown";

  const realCount = [signal.hasDb, signal.hasRuntime, signal.hasBroker].filter(Boolean).length;

  if (realCount === 0) {
    return signal.hasPlaceholder ? "placeholder" : "unknown";
  }

  if (signal.hasPlaceholder || realCount > 1) {
    return "mixed";
  }

  if (signal.hasDb) return "db_truth";
  if (signal.hasRuntime) return "runtime_memory";
  if (signal.hasBroker) return "broker_snapshot";
  return "unknown";
}

export function classifyFieldSource(dataSource: DataSourceDetail, connected: boolean, hints: FieldEvidenceHints): SourceAuthority {
  if (!connected || dataSource.state === "disconnected") {
    return "unknown";
  }

  if (dataSource.state === "mock") {
    return "placeholder";
  }

  return classifyAuthority(
    {
      hasDb: hasEndpoint(dataSource.realEndpoints, hints.db),
      hasRuntime: hasEndpoint(dataSource.realEndpoints, hints.runtime),
      hasBroker: hasEndpoint(dataSource.realEndpoints, hints.broker),
      hasPlaceholder: hasMockSection(dataSource.mockSections, hints.placeholder),
    },
    connected,
  );
}

export function emptyPanelSourceMap(authority: SourceAuthority): PanelSourceMap {
  return CORE_PANEL_KEYS.reduce((acc, panel) => {
    acc[panel] = authority;
    return acc;
  }, {} as PanelSourceMap);
}

export function classifyPanelSources(dataSource: DataSourceDetail, connected: boolean): PanelSourceMap {
  if (!connected || dataSource.state === "disconnected") {
    return emptyPanelSourceMap("unknown");
  }

  if (dataSource.state === "mock") {
    return emptyPanelSourceMap("placeholder");
  }

  return CORE_PANEL_KEYS.reduce((acc, panel) => {
    const hints = PANEL_EVIDENCE_HINTS[panel];
    const signal: EvidenceSignal = {
      hasDb: hasEndpoint(dataSource.realEndpoints, hints.db),
      hasRuntime: hasEndpoint(dataSource.realEndpoints, hints.runtime),
      hasBroker: hasEndpoint(dataSource.realEndpoints, hints.broker),
      hasPlaceholder: hasMockSection(dataSource.mockSections, hints.placeholder),
    };
    acc[panel] = classifyAuthority(signal, connected);
    return acc;
  }, {} as PanelSourceMap);
}

export function withClassifiedPanelSources(model: Omit<SystemModel, "panelSources">): SystemModel {
  return {
    ...model,
    panelSources: classifyPanelSources(model.dataSource, model.connected),
  };
}
