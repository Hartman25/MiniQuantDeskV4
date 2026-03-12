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

const PANEL_EVIDENCE_HINTS: Record<CorePanelKey, PanelEvidenceHints> = {
  dashboard: { db: ["/portfolio", "/reconcile"], runtime: ["/system/status", "/system/preflight"], broker: ["/broker", "/execution/orders"], placeholder: ["status", "preflight"] },
  metrics: { db: ["/metrics"], runtime: ["/metrics"], broker: ["/metrics"], placeholder: ["metrics"] },
  execution: { db: ["/execution/timeline", "/execution/trace"], runtime: ["/execution/summary", "/execution/orders"], broker: ["/execution/orders", "/execution/replay"], placeholder: ["executionSummary", "executionOrders"] },
  risk: { db: ["/risk/summary"], runtime: ["/system/status"], broker: ["/execution/orders"], placeholder: ["riskSummary"] },
  portfolio: { db: ["/portfolio/positions", "/portfolio/summary"], runtime: ["/portfolio"], broker: ["/execution/fills"], placeholder: ["portfolioSummary", "positions", "fills"] },
  reconcile: { db: ["/reconcile/summary", "/reconcile/mismatches"], runtime: ["/system/runtime-leadership"], broker: ["/reconcile", "/broker"], placeholder: ["reconcileSummary", "mismatches"] },
  strategy: { db: ["/strategy/suppressions"], runtime: ["/strategy/rows"], broker: ["/execution/orders"], placeholder: ["strategies", "strategySuppressions"] },
  audit: { db: ["/audit/actions"], runtime: ["/system/metadata"], broker: [], placeholder: ["auditActions"] },
  ops: { db: ["/system/config-diffs"], runtime: ["/system/status", "/system/preflight"], broker: ["/broker"], placeholder: ["status", "preflight", "configDiffs"] },
  settings: { db: ["/system/config-diffs"], runtime: ["/system/metadata"], broker: [], placeholder: ["metadata", "configDiffs"] },
  topology: { db: ["/system/topology"], runtime: ["/system/topology"], broker: ["/broker"], placeholder: ["topology"] },
  transport: { db: ["/system/transport"], runtime: ["/system/transport"], broker: ["/broker"], placeholder: ["transport"] },
  incidents: { db: ["/system/incidents"], runtime: ["/system/incidents"], broker: ["/broker"], placeholder: ["incidents"] },
  alerts: { db: ["/system/alerts"], runtime: ["/system/status"], broker: ["/broker"], placeholder: ["alerts"] },
  session: { db: ["/system/session-state"], runtime: ["/system/status"], broker: [], placeholder: ["sessionState"] },
  config: { db: ["/system/config-fingerprint"], runtime: ["/system/runtime-leadership"], broker: [], placeholder: ["configFingerprint", "runtimeLeadership"] },
  marketData: { db: ["/system/market-data-quality"], runtime: ["/system/status"], broker: ["/broker", "/market-data"], placeholder: ["marketDataQuality"] },
  runtime: { db: ["/system/runtime-leadership"], runtime: ["/system/status", "/system/preflight", "/system/runtime-leadership"], broker: [], placeholder: ["runtimeLeadership", "status", "preflight"] },
  artifacts: { db: ["/system/artifacts"], runtime: ["/system/operator-timeline"], broker: [], placeholder: ["artifactRegistry"] },
  operatorTimeline: { db: ["/system/operator-timeline"], runtime: ["/system/operator-timeline"], broker: ["/broker"], placeholder: ["operatorTimeline"] },
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
