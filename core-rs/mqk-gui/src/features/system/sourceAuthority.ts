import type { DataSourceDetail, PanelSourceMap, SourceAuthority, SourceAuthorityDetail } from "./types";

const AUTHORITY_LABEL: Record<SourceAuthority, string> = {
  db_truth: "DB truth",
  runtime_memory: "Runtime memory",
  broker_snapshot: "Broker snapshot",
  placeholder: "Placeholder / mock",
  mixed: "Mixed",
  unknown: "Unknown",
};

export function sourceAuthorityLabel(value: SourceAuthority): string {
  return AUTHORITY_LABEL[value];
}

export function mergeAuthorities(values: SourceAuthority[]): SourceAuthority {
  const unique = Array.from(new Set(values));
  if (unique.length === 0) return "unknown";
  if (unique.length === 1) return unique[0];
  return "mixed";
}

export function detailForPanel(args: {
  sections: string[];
  values: SourceAuthority[];
  note: string;
}): SourceAuthorityDetail {
  const sources = Array.from(new Set(args.values));
  return {
    authority: mergeAuthorities(sources),
    sources,
    sections: args.sections,
    note: args.note,
  };
}

export function disconnectedPanelSources(dataSource: DataSourceDetail): PanelSourceMap {
  const note = dataSource.message ?? "Daemon unreachable; source cannot be proven.";
  const unknown = detailForPanel({ sections: [], values: ["unknown"], note });
  return {
    dashboard: unknown,
    system: unknown,
    execution: unknown,
    risk: unknown,
    reconcile: unknown,
    portfolio: unknown,
    ops: unknown,
  };
}

export function classifyPanelSources(dataSource: DataSourceDetail): PanelSourceMap {
  if (dataSource.state === "disconnected") return disconnectedPanelSources(dataSource);

  const hasPlaceholder = dataSource.mockSections.length > 0;
  const commonBase: SourceAuthority[] = hasPlaceholder ? ["placeholder"] : [];

  const dashboard = detailForPanel({
    sections: ["status", "preflight", "executionSummary", "riskSummary", "reconcileSummary", "positions", "openOrders", "fills"],
    values: ["runtime_memory", "broker_snapshot", ...commonBase],
    note: "Combines runtime status, reconcile state, and broker-trading snapshots.",
  });

  const system = detailForPanel({
    sections: ["status", "preflight", "runtimeLeadership", "configFingerprint", "sessionState"],
    values: ["runtime_memory", ...commonBase],
    note: "Derived from daemon runtime memory and health gates; DB truth is not asserted.",
  });

  const execution = detailForPanel({
    sections: ["executionSummary", "executionOrders", "omsOverview", "transport", "executionTrace", "executionReplay", "executionChart", "causalityTrace"],
    values: ["runtime_memory", "broker_snapshot", ...commonBase],
    note: "Execution combines runtime queues/state with broker-derived order snapshots.",
  });

  const risk = detailForPanel({
    sections: ["riskSummary", "riskDenials", "strategySuppressions", "status"],
    values: ["runtime_memory", "broker_snapshot", ...commonBase],
    note: "Risk posture is computed from runtime gates with broker snapshot exposure data.",
  });

  const reconcile = detailForPanel({
    sections: ["reconcileSummary", "mismatches", "replaceCancelChains", "incidents"],
    values: ["runtime_memory", ...commonBase],
    note: "Reconcile endpoints are daemon-runtime state; DB-backed proof is not exposed.",
  });

  const portfolio = detailForPanel({
    sections: ["portfolioSummary", "positions", "openOrders", "fills"],
    values: ["broker_snapshot", ...commonBase],
    note: "Portfolio surfaces broker snapshot/trading endpoints; not durable DB truth.",
  });

  const ops = detailForPanel({
    sections: ["status", "runtimeLeadership", "actionCatalog"],
    values: ["runtime_memory", ...commonBase],
    note: "Operator mode/actions are runtime control-plane state.",
  });

  return { dashboard, system, execution, risk, reconcile, portfolio, ops };
}

export function sourceAuthorityTone(authority: SourceAuthority): "good" | "warn" | "bad" | "neutral" {
  if (authority === "db_truth") return "good";
  if (authority === "runtime_memory" || authority === "broker_snapshot") return "neutral";
  if (authority === "mixed") return "warn";
  return "bad";
}
