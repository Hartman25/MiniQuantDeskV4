import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime, formatDurationMs } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

export function MarketDataScreen({ model }: { model: SystemModel }) {
  const q = model.marketDataQuality;
  const truthState = panelTruthRenderState(model, "marketData");

  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
  }

  // Issues currently blocking at least one strategy — shown first.
  const blockingIssues = q.issues.filter((i) => i.affected_strategies.length > 0);

  // Venue triage: critical first, then warning, then ok; tie-break by freshness lag descending.
  const sortedVenues = [...q.venues].sort((a, b) => {
    const healthOrder = { critical: 0, warning: 1, ok: 2 };
    const aH = healthOrder[a.health as keyof typeof healthOrder] ?? 9;
    const bH = healthOrder[b.health as keyof typeof healthOrder] ?? 9;
    if (aH !== bH) return aH - bH;
    return (b.freshness_lag_ms ?? 0) - (a.freshness_lag_ms ?? 0);
  });

  // Issue ledger: critical first, then warning, then info.
  const severityOrder = { critical: 0, warning: 1, info: 2 };
  const sortedIssues = [...q.issues].sort((a, b) => {
    const aS = severityOrder[a.severity as keyof typeof severityOrder] ?? 9;
    const bS = severityOrder[b.severity as keyof typeof severityOrder] ?? 9;
    return aS - bS;
  });

  return (
    <div className="screen-grid desk-screen-grid">

      {/* Primary pressure signals — strategy blocks and disagreement lead */}
      <div className="summary-grid summary-grid-four">
        <StatCard
          title="Strategy Blocks"
          value={String(q.strategy_blocks)}
          detail="Strategies blocked by data-quality issues"
          tone={q.strategy_blocks > 0 ? "bad" : "good"}
        />
        <StatCard
          title="Venue Disagreements"
          value={String(q.venue_disagreement_count)}
          detail="Cross-venue price or bar disagreements"
          tone={q.venue_disagreement_count > 0 ? "warn" : "good"}
        />
        <StatCard
          title="Stale Symbols"
          value={String(q.stale_symbol_count)}
          detail={`Over freshness SLA (${formatDurationMs(q.freshness_sla_ms)})`}
          tone={q.stale_symbol_count > 0 ? "warn" : "good"}
        />
        <StatCard
          title="Missing Bars"
          value={String(q.missing_bar_count)}
          detail="Bar continuity gaps detected"
          tone={q.missing_bar_count > 0 ? "bad" : "good"}
        />
      </div>

      {/* Strategy-blocking issues — pinned first when any exist */}
      {blockingIssues.length > 0 && (
        <Panel
          title="Strategy-blocking issues"
          subtitle="Data-quality issues currently blocking one or more strategies. Resolve these before investigating venue or pipeline issues."
        >
          <DataTable
            rows={blockingIssues}
            rowKey={(row) => row.issue_id}
            columns={[
              { key: "severity", title: "Severity", render: (row) => row.severity },
              { key: "scope", title: "Scope", render: (row) => row.scope },
              { key: "type", title: "Issue Type", render: (row) => row.issue_type },
              { key: "symbol", title: "Symbol", render: (row) => row.symbol ?? "—" },
              { key: "venue", title: "Venue", render: (row) => row.venue ?? "—" },
              { key: "lag", title: "Freshness Lag", render: (row) => row.freshness_lag_ms != null ? formatDurationMs(row.freshness_lag_ms) : "—" },
              { key: "strategies", title: "Blocked Strategies", render: (row) => row.affected_strategies.join(", ") },
              { key: "detected", title: "Detected", render: (row) => formatDateTime(row.detected_at) },
            ]}
          />
        </Panel>
      )}

      {/* Venue freshness triage — degraded venues sorted first by freshness lag */}
      <Panel
        title="Venue freshness triage"
        subtitle="Freshness lag, missing updates, and disagreement pressure by venue. Degraded venues sorted first; worst lag within each health tier at top."
      >
        {q.venues.length === 0 ? (
          <div className="empty-state">No venue data reported.</div>
        ) : (
          <DataTable
            rows={sortedVenues}
            rowKey={(row) => row.venue_key}
            columns={[
              { key: "health", title: "Health", render: (row) => row.health },
              { key: "venue", title: "Venue", render: (row) => row.label },
              { key: "lag", title: "Freshness Lag", render: (row) => formatDurationMs(row.freshness_lag_ms) },
              { key: "stale", title: "Stale Symbols", render: (row) => row.symbols_degraded },
              { key: "missing", title: "Missing Updates", render: (row) => row.missing_updates },
              { key: "disagreement", title: "Disagreements", render: (row) => row.disagreement_count },
              { key: "last", title: "Last Good", render: (row) => formatDateTime(row.last_good_at) },
              { key: "note", title: "Note", render: (row) => row.note },
            ]}
          />
        )}
      </Panel>

      {/* Issue scope ledger — all issues, critical first, with freshness lag and affected strategies */}
      <Panel
        title="Issue scope ledger"
        subtitle="All active data-quality issues by scope (symbol / venue / pipeline). Critical severity first. Use freshness lag and affected-strategy columns to prioritise resolution."
      >
        {q.issues.length === 0 ? (
          <div className="empty-state">No data-quality issues recorded.</div>
        ) : (
          <DataTable
            rows={sortedIssues}
            rowKey={(row) => row.issue_id}
            columns={[
              { key: "severity", title: "Severity", render: (row) => row.severity },
              { key: "scope", title: "Scope", render: (row) => row.scope },
              { key: "type", title: "Issue Type", render: (row) => row.issue_type },
              { key: "symbol", title: "Symbol", render: (row) => row.symbol ?? "—" },
              { key: "venue", title: "Venue", render: (row) => row.venue ?? "—" },
              { key: "lag", title: "Freshness Lag", render: (row) => row.freshness_lag_ms != null ? formatDurationMs(row.freshness_lag_ms) : "—" },
              { key: "strategies", title: "Affected Strategies", render: (row) => row.affected_strategies.length > 0 ? row.affected_strategies.join(", ") : "—" },
              { key: "status", title: "Status", render: (row) => row.status },
              { key: "detected", title: "Detected", render: (row) => formatDateTime(row.detected_at) },
            ]}
          />
        )}
      </Panel>
    </div>
  );
}
