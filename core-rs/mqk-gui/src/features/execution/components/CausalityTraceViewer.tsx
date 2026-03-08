
import { DataTable } from "../../../components/common/DataTable";
import { Panel } from "../../../components/common/Panel";
import { formatDateTime, formatDurationMs, formatLabel, formatMoney } from "../../../lib/format";
import type { CausalityTrace } from "../../system/types";

export function CausalityTraceViewer({ trace }: { trace: CausalityTrace | null }) {
  if (!trace) {
    return <Panel title="Causality trace viewer"><div className="empty-state">Select an order to inspect upstream signal, intent, execution, portfolio, and reconcile causality.</div></Panel>;
  }

  return (
    <Panel title="Causality trace viewer" subtitle={`${trace.symbol} · ${trace.strategy_id} · ${trace.internal_order_id}`}>
      <div className="timeline-meta-grid">
        <div><span>Incident</span><strong>{trace.incident_id}</strong></div>
        <div><span>Broker order</span><strong>{trace.broker_order_id ?? "—"}</strong></div>
        <div><span>OMS</span><strong>{formatLabel(trace.current_oms_state)}</strong></div>
        <div><span>Execution</span><strong>{formatLabel(trace.current_execution_state)}</strong></div>
        <div><span>Reconcile</span><strong>{formatLabel(trace.current_reconcile_status)}</strong></div>
        <div><span>Outcome</span><strong>{formatLabel(trace.terminal_outcome)}</strong></div>
      </div>

      <div className="causality-node-grid">
        {trace.nodes.map((node) => (
          <div key={node.node_key} className={`causality-node-card severity-${node.status === "critical" ? "critical" : node.status === "warning" ? "warning" : "info"}`}>
            <div className="alert-header">
              <strong>{node.title}</strong>
              <span>{node.timestamp ? formatDateTime(node.timestamp) : "—"}</span>
            </div>
            <div className="summary-detail">{formatLabel(node.node_type)} · {formatLabel(node.subsystem)}</div>
            <div className="summary-detail">ID {node.linked_id ?? "—"} · Δ {formatDurationMs(node.elapsed_from_prev_ms)}</div>
            <div className="summary-detail">{node.summary}</div>
            {node.anomaly_tags.length > 0 ? <div className="summary-detail">Anomalies: {node.anomaly_tags.join(", ")}</div> : null}
          </div>
        ))}
      </div>

      <div className="two-column-grid">
        <Panel title="Identity correlation" compact>
          <div className="metric-list compact-list">
            <div><span>Signal</span><strong>{trace.correlation.signal_id ?? "—"}</strong></div>
            <div><span>Decision</span><strong>{trace.correlation.decision_id ?? "—"}</strong></div>
            <div><span>Intent</span><strong>{trace.correlation.intent_id ?? "—"}</strong></div>
            <div><span>Outbox</span><strong>{trace.correlation.outbox_id ?? "—"}</strong></div>
            <div><span>Claim token</span><strong>{trace.correlation.claim_token ?? "—"}</strong></div>
            <div><span>Dispatch</span><strong>{trace.correlation.dispatch_attempt_id ?? "—"}</strong></div>
            <div><span>Inbox IDs</span><strong>{trace.correlation.inbox_ids.join(", ") || "—"}</strong></div>
            <div><span>Fill IDs</span><strong>{trace.correlation.fill_ids.join(", ") || "—"}</strong></div>
            <div><span>Portfolio IDs</span><strong>{trace.correlation.portfolio_event_ids.join(", ") || "—"}</strong></div>
            <div><span>Reconcile case</span><strong>{trace.correlation.reconcile_case_id ?? "—"}</strong></div>
            <div><span>Audit chain</span><strong>{trace.correlation.audit_chain_id ?? "—"}</strong></div>
            <div><span>Run ID</span><strong>{trace.correlation.run_id ?? "—"}</strong></div>
          </div>
        </Panel>

        <Panel title="Portfolio + reconcile effects" compact>
          <div className="metric-list compact-list">
            <div><span>Position</span><strong>{trace.portfolio_effects.pre_position_qty} → {trace.portfolio_effects.post_position_qty}</strong></div>
            <div><span>Net delta</span><strong>{trace.portfolio_effects.net_position_delta}</strong></div>
            <div><span>Avg price effect</span><strong>{formatMoney(trace.portfolio_effects.average_price_effect)}</strong></div>
            <div><span>Cash delta</span><strong>{formatMoney(trace.portfolio_effects.cash_delta)}</strong></div>
            <div><span>Buying power delta</span><strong>{formatMoney(trace.portfolio_effects.buying_power_delta)}</strong></div>
            <div><span>Exposure delta</span><strong>{formatMoney(trace.portfolio_effects.exposure_delta)}</strong></div>
            <div><span>Realized PnL</span><strong>{formatMoney(trace.portfolio_effects.realized_pnl_delta)}</strong></div>
            <div><span>Unrealized PnL</span><strong>{formatMoney(trace.portfolio_effects.unrealized_pnl_delta)}</strong></div>
            <div><span>Allocation effect</span><strong>{trace.portfolio_effects.strategy_allocation_effect}</strong></div>
            <div><span>Reconcile status</span><strong>{formatLabel(trace.reconcile_outcome.reconcile_status)}</strong></div>
            <div><span>Corrected fields</span><strong>{trace.reconcile_outcome.corrected_fields.join(", ") || "—"}</strong></div>
            <div><span>Escalation</span><strong>{formatLabel(trace.reconcile_outcome.operator_escalation_status)}</strong></div>
          </div>
        </Panel>
      </div>

      <Panel title="Correlated timeline" compact>
        <DataTable
          rows={trace.timeline}
          rowKey={(row) => row.row_id}
          columns={[
            { key: "at", title: "At", render: (row) => formatDateTime(row.timestamp) },
            { key: "subsystem", title: "Subsystem", render: (row) => formatLabel(row.subsystem) },
            { key: "event", title: "Event", render: (row) => formatLabel(row.event_type) },
            { key: "correlation", title: "Correlation", render: (row) => row.correlation_id },
            { key: "state", title: "State", render: (row) => `${row.before_state} → ${row.after_state}` },
            { key: "delta", title: "Δ", render: (row) => formatDurationMs(row.latency_since_prev_ms) },
            { key: "summary", title: "Summary", render: (row) => row.summary },
          ]}
        />
      </Panel>

      <Panel title="Breakpoint / anomaly rail" compact>
        <div className="list-stack">
          {trace.anomalies.length === 0 ? (
            <div className="empty-state">No causal breakpoints recorded.</div>
          ) : (
            trace.anomalies.map((anomaly) => (
              <div key={anomaly} className="alert-card severity-warning">
                <div className="alert-header">
                  <strong>{formatLabel(anomaly)}</strong>
                  <span>{trace.reconcile_outcome.correction_timestamp ? formatDateTime(trace.reconcile_outcome.correction_timestamp) : "Active"}</span>
                </div>
                <div className="alert-message">This chain contains a recorded breakpoint that should be cross-checked against trace, replay, and reconcile outputs.</div>
              </div>
            ))
          )}
        </div>
      </Panel>
    </Panel>
  );
}
