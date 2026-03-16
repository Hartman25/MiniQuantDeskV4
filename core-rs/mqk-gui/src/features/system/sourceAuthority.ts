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
  // Risk summary is derived from the execution snapshot (runtime memory). Denials are DB-logged.
  risk: {
    db: ["/risk/denials"],
    runtime: ["/risk/summary", "/system/status"],
    broker: [],
    placeholder: ["riskSummary", "riskDenials"],
  },
  // Portfolio data originates from the broker (account/positions/fills are broker snapshot).
  portfolio: {
    db: [],
    runtime: [],
    broker: ["/portfolio/summary", "/portfolio/positions", "/portfolio/fills", "/trading/account", "/trading/positions", "/trading/fills"],
    placeholder: ["portfolioSummary", "positions", "fills"],
  },
  // Reconcile records are persisted in Postgres — always DB truth.
  reconcile: {
    db: ["/reconcile/status", "/reconcile/mismatches"],
    runtime: [],
    broker: [],
    placeholder: ["reconcileSummary", "mismatches"],
  },
  // Strategy rows are runtime OMS state. Suppressions are DB-persisted records.
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
  // Runtime leadership is pure daemon runtime state — no DB query backing in current arch.
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
