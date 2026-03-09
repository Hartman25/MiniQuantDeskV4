import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { formatDateTime, formatDurationMs } from "../../lib/format";
import type { SystemModel } from "../system/types";

export function MarketDataScreen({ model }: { model: SystemModel }) {
  const q = model.marketDataQuality;

  return (
    <div className="screen-grid desk-screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Overall Health" value={q.overall_health} detail="Top-level feed health" tone={q.overall_health === "critical" ? "bad" : q.overall_health === "warning" ? "warn" : "good"} />
        <StatCard title="Stale Symbols" value={String(q.stale_symbol_count)} detail="Symbols over freshness threshold" tone={q.stale_symbol_count > 0 ? "warn" : "good"} />
        <StatCard title="Missing Bars" value={String(q.missing_bar_count)} detail="Bar continuity issues" tone={q.missing_bar_count > 0 ? "bad" : "good"} />
        <StatCard title="Strategy Blocks" value={String(q.strategy_blocks)} detail="Strategies blocked by market-data issues" tone={q.strategy_blocks > 0 ? "bad" : "good"} />
      </div>

      <Panel title="Venue quality" subtitle="Freshness, disagreement, and update continuity across venues.">
        <DataTable
          rows={q.venues}
          rowKey={(row) => row.venue_key}
          columns={[
            { key: "venue", title: "Venue", render: (row) => row.label },
            { key: "health", title: "Health", render: (row) => row.health },
            { key: "lag", title: "Freshness Lag", render: (row) => formatDurationMs(row.freshness_lag_ms) },
            { key: "degraded", title: "Degraded Symbols", render: (row) => row.symbols_degraded },
            { key: "missing", title: "Missing Updates", render: (row) => row.missing_updates },
            { key: "disagreement", title: "Disagreements", render: (row) => row.disagreement_count },
            { key: "last", title: "Last Good", render: (row) => formatDateTime(row.last_good_at) },
            { key: "note", title: "Note", render: (row) => row.note },
          ]}
        />
      </Panel>

      <Panel title="Data quality issues" subtitle="Feed problems that can degrade or block trading.">
        <DataTable
          rows={q.issues}
          rowKey={(row) => row.issue_id}
          columns={[
            { key: "severity", title: "Severity", render: (row) => row.severity },
            { key: "scope", title: "Scope", render: (row) => row.scope },
            { key: "symbol", title: "Symbol", render: (row) => row.symbol ?? "—" },
            { key: "venue", title: "Venue", render: (row) => row.venue },
            { key: "type", title: "Issue Type", render: (row) => row.issue_type },
            { key: "status", title: "Status", render: (row) => row.status },
            { key: "detected", title: "Detected", render: (row) => formatDateTime(row.detected_at) },
            { key: "note", title: "Note", render: (row) => row.note },
          ]}
        />
      </Panel>
    </div>
  );
}
