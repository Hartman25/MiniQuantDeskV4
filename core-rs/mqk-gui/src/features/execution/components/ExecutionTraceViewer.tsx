import { DataTable } from "../../../components/common/DataTable";
import { Panel } from "../../../components/common/Panel";
import { formatDateTime, formatDurationMs, formatMoney } from "../../../lib/format";
import type { ExecutionTrace } from "../../system/types";

export function ExecutionTraceViewer({ trace }: { trace: ExecutionTrace | null }) {
  if (!trace) {
    return <Panel title="Execution trace viewer"><div className="empty-state">Select an order to inspect a full transport and broker trace.</div></Panel>;
  }

  return (
    <Panel title="Execution trace viewer" subtitle={`${trace.symbol} · ${trace.strategy_id} · ${trace.internal_order_id}`}>
      <div className="timeline-meta-grid">
        <div><span>Broker order</span><strong>{trace.broker_order_id ?? "—"}</strong></div>
        <div><span>OMS state</span><strong>{trace.current_oms_state}</strong></div>
        <div><span>Execution state</span><strong>{trace.current_execution_state}</strong></div>
        <div><span>Submit time</span><strong>{formatDateTime(trace.submit_time)}</strong></div>
        <div><span>Replay</span><strong>{trace.replay_available ? "Available" : "Unavailable"}</strong></div>
        <div><span>Outbox</span><strong>{trace.correlation.outbox_id ?? "—"}</strong></div>
      </div>

      <div className="two-column-grid">
        <Panel title="Correlation IDs" compact>
          <div className="metric-list compact-list">
            <div><span>Claim token</span><strong>{trace.correlation.claim_token ?? "—"}</strong></div>
            <div><span>Dispatch attempt</span><strong>{trace.correlation.dispatch_attempt_id ?? "—"}</strong></div>
            <div><span>Inbox IDs</span><strong>{trace.correlation.inbox_ids.join(", ") || "—"}</strong></div>
            <div><span>Fill IDs</span><strong>{trace.correlation.fill_ids.join(", ") || "—"}</strong></div>
            <div><span>Reconcile case</span><strong>{trace.correlation.reconcile_case_id ?? "—"}</strong></div>
            <div><span>Audit chain</span><strong>{trace.correlation.audit_chain_id ?? "—"}</strong></div>
          </div>
        </Panel>
        <Panel title="State ladder" compact>
          <DataTable
            rows={trace.state_ladder}
            rowKey={(row) => row.key}
            columns={[
              { key: "at", title: "At", render: (row) => formatDateTime(row.at) },
              { key: "oms", title: "OMS", render: (row) => row.oms_state },
              { key: "exec", title: "Execution", render: (row) => row.execution_state },
              { key: "broker", title: "Broker", render: (row) => row.broker_state },
              { key: "reconcile", title: "Reconcile", render: (row) => row.reconcile_state },
            ]}
          />
        </Panel>
      </div>

      <Panel title="Trace timeline" compact>
        <DataTable
          rows={trace.timeline}
          rowKey={(row) => row.trace_event_id}
          columns={[
            { key: "timestamp", title: "Timestamp", render: (row) => formatDateTime(row.timestamp) },
            { key: "subsystem", title: "Subsystem", render: (row) => row.subsystem },
            { key: "event_type", title: "Event", render: (row) => row.event_type },
            { key: "transition", title: "State", render: (row) => `${row.before_state} → ${row.after_state}` },
            { key: "latency", title: "Δ", render: (row) => formatDurationMs(row.latency_since_prev_ms) },
            { key: "summary", title: "Summary", render: (row) => row.summary },
          ]}
        />
      </Panel>

      <div className="two-column-grid">
        <Panel title="Broker messages" compact>
          <DataTable
            rows={trace.broker_messages}
            rowKey={(row) => row.message_id}
            columns={[
              { key: "timestamp", title: "At", render: (row) => formatDateTime(row.timestamp) },
              { key: "direction", title: "Dir", render: (row) => row.direction },
              { key: "type", title: "Type", render: (row) => row.message_type },
              { key: "summary", title: "Normalized", render: (row) => row.normalized_summary },
            ]}
          />
        </Panel>
        <Panel title="Fills / economics" compact>
          <DataTable
            rows={trace.fills}
            rowKey={(row) => row.fill_id}
            columns={[
              { key: "timestamp", title: "At", render: (row) => formatDateTime(row.timestamp) },
              { key: "qty", title: "Qty", render: (row) => row.qty },
              { key: "price", title: "Price", render: (row) => formatMoney(row.price) },
              { key: "avg", title: "Avg Fill", render: (row) => formatMoney(row.average_fill_price) },
              { key: "fees", title: "Fees", render: (row) => formatMoney(row.fee_actual) },
            ]}
          />
        </Panel>
      </div>
    </Panel>
  );
}
