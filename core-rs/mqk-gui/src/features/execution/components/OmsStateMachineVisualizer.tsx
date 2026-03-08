import { Panel } from "../../../components/common/Panel";
import { formatDateTime, formatDurationMs, formatLabel } from "../../../lib/format";
import type { OmsOverview } from "../../system/types";

export function OmsStateMachineVisualizer({ overview }: { overview: OmsOverview }) {
  return (
    <Panel title="OMS state machine visualizer" subtitle="State supervision for working and recently completed orders.">
      <div className="summary-grid summary-grid-five">
        {overview.state_nodes.map((node) => (
          <div key={node.state} className="summary-card panel stat-neutral">
            <div className="eyebrow">{formatLabel(node.state)}</div>
            <div className="summary-value">{node.active_count}</div>
            <div className="summary-detail">Warn {node.warning_count} · Stuck {node.over_sla_count}</div>
            <div className="summary-detail">P95 dwell {formatDurationMs(node.p95_dwell_ms)}</div>
          </div>
        ))}
      </div>

      <div className="oms-flow-grid">
        {overview.transition_edges.map((edge) => (
          <div key={`${edge.from_state}-${edge.to_state}`} className="oms-edge-card">
            <strong>{formatLabel(edge.from_state)} → {formatLabel(edge.to_state)}</strong>
            <span>{edge.transition_count} transitions</span>
            <span>Median {formatDurationMs(edge.median_latency_ms)}</span>
            <span>Anomalies {edge.anomaly_count}</span>
          </div>
        ))}
      </div>

      <div className="table-grid">
        <div className="table-row table-head">
          <span>Order</span>
          <span>Symbol</span>
          <span>State</span>
          <span>Stage</span>
          <span>Qty</span>
          <span>Dwell</span>
          <span>Entered</span>
        </div>
        {overview.orders.map((row) => (
          <div className={`table-row ${row.is_stuck ? "row-critical" : ""}`} key={row.internal_order_id}>
            <span>{row.internal_order_id}</span>
            <span>{row.symbol}</span>
            <span>{formatLabel(row.oms_state)}</span>
            <span>{row.execution_stage}</span>
            <span>{row.filled_qty}/{row.requested_qty}</span>
            <span>{formatDurationMs(row.dwell_ms)} / SLA {formatDurationMs(row.sla_ms)}</span>
            <span>{formatDateTime(row.entered_state_at)}</span>
          </div>
        ))}
      </div>
    </Panel>
  );
}
