import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { formatDateTime, formatDurationMs, formatLabel, formatNumber } from "../../lib/format";
import type { SystemModel } from "../system/types";

export function MarketDataScreen({ model }: { model: SystemModel }) {
  const md = model.marketDataQuality;

  return (
    <div className="screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Overall Health" value={formatLabel(md.overall_health)} tone={md.overall_health === "critical" ? "bad" : md.overall_health === "warning" ? "warn" : md.overall_health === "ok" ? "good" : "neutral"} />
        <StatCard title="Stale Symbols" value={formatNumber(md.stale_symbol_count)} tone={md.stale_symbol_count > 0 ? "warn" : "good"} />
        <StatCard title="Missing Bars" value={formatNumber(md.missing_bar_count)} tone={md.missing_bar_count > 0 ? "bad" : "good"} />
        <StatCard title="Strategy Blocks" value={formatNumber(md.strategy_blocks)} tone={md.strategy_blocks > 0 ? "bad" : "good"} />
      </div>

      <Panel title="Venue health" subtitle="See which feed layer is degrading before execution starts lying to you.">
        <DataTable
          rows={md.venues}
          rowKey={(row) => row.venue_key}
          columns={[
            { key: "venue", title: "Venue", render: (row) => row.label },
            { key: "health", title: "Health", render: (row) => formatLabel(row.health) },
            { key: "lag", title: "Freshness Lag", render: (row) => formatDurationMs(row.freshness_lag_ms) },
            { key: "symbols", title: "Symbols Degraded", render: (row) => formatNumber(row.symbols_degraded) },
            { key: "missing", title: "Missing Updates", render: (row) => formatNumber(row.missing_updates) },
            { key: "disagreement", title: "Disagreements", render: (row) => formatNumber(row.disagreement_count) },
            { key: "good", title: "Last Good", render: (row) => formatDateTime(row.last_good_at) },
            { key: "note", title: "Note", render: (row) => row.note },
          ]}
        />
      </Panel>

      <Panel title="Active data-quality issues" subtitle="This is what should suppress strategies or force operator review.">
        <DataTable
          rows={md.issues}
          rowKey={(row) => row.issue_id}
          columns={[
            { key: "id", title: "Issue", render: (row) => row.issue_id },
            { key: "severity", title: "Severity", render: (row) => formatLabel(row.severity) },
            { key: "scope", title: "Scope", render: (row) => formatLabel(row.scope) },
            { key: "symbol", title: "Symbol", render: (row) => row.symbol ?? "—" },
            { key: "venue", title: "Venue", render: (row) => row.venue ?? "—" },
            { key: "type", title: "Issue Type", render: (row) => formatLabel(row.issue_type) },
            { key: "lag", title: "Lag", render: (row) => formatDurationMs(row.freshness_lag_ms) },
            { key: "strategies", title: "Affected Strategies", render: (row) => row.affected_strategies.join(", ") || "—" },
            { key: "status", title: "Status", render: (row) => formatLabel(row.status) },
            { key: "detected", title: "Detected", render: (row) => formatDateTime(row.detected_at) },
            { key: "note", title: "Note", render: (row) => row.note },
          ]}
        />
      </Panel>
    </div>
  );
}
