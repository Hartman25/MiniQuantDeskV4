import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { formatDateTime } from "../../lib/format";
import type { SystemModel } from "../system/types";
import { CausalityTraceViewer } from "../execution/components/CausalityTraceViewer";
import { ExecutionReplayViewer } from "../execution/components/ExecutionReplayViewer";

export function ReconcileScreen({ model }: { model: SystemModel }) {
  const r = model.reconcileSummary;

  return (
    <div className="screen-grid desk-screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard
          title="Reconcile Status"
          value={r.status}
          detail={`Last run ${formatDateTime(r.last_run_at)}`}
          tone={r.status === "critical" ? "bad" : r.status === "warning" ? "warn" : "good"}
        />
        <StatCard title="Mismatched Positions" value={String(r.mismatched_positions)} tone={r.mismatched_positions > 0 ? "warn" : "good"} />
        <StatCard title="Mismatched Orders" value={String(r.mismatched_orders)} tone={r.mismatched_orders > 0 ? "warn" : "good"} />
        <StatCard title="Unmatched Events" value={String(r.unmatched_broker_events)} tone={r.unmatched_broker_events > 0 ? "warn" : "good"} />
      </div>

      <div className="desk-panel-grid desk-panel-grid-primary">
        <Panel title="Mismatch grid" subtitle="Primary broker-vs-internal disagreement surface.">
          <DataTable
            rows={model.mismatches}
            rowKey={(row) => row.id}
            columns={[
              { key: "domain", title: "Domain", render: (row) => row.domain },
              { key: "symbol", title: "Symbol", render: (row) => row.symbol },
              { key: "internal", title: "Internal", render: (row) => row.internal_value },
              { key: "broker", title: "Broker", render: (row) => row.broker_value },
              { key: "note", title: "Note", render: (row) => row.note },
            ]}
          />
        </Panel>

        <Panel title="Correction / chain state" subtitle="What still needs operator attention right now.">
          <div className="metric-list">
            <div><span>Mismatched fills</span><strong>{r.mismatched_fills}</strong></div>
            <div><span>Unmatched events</span><strong>{r.unmatched_broker_events}</strong></div>
            <div><span>Replace/cancel chains</span><strong>{model.replaceCancelChains.length}</strong></div>
            <div><span>Active incidents</span><strong>{model.incidents.length}</strong></div>
            <div><span>Latest runtime generation</span><strong>{model.runtimeLeadership.generation_id}</strong></div>
            <div><span>Recovery state</span><strong>{model.runtimeLeadership.post_restart_recovery_state}</strong></div>
          </div>
        </Panel>
      </div>

      <Panel title="Replace / cancel chains" compact>
        <DataTable
          rows={model.replaceCancelChains}
          rowKey={(row) => row.chain_id}
          columns={[
            { key: "symbol", title: "Symbol", render: (row) => row.symbol },
            { key: "strategy", title: "Strategy", render: (row) => row.strategy_id },
            { key: "action", title: "Action", render: (row) => row.action_type },
            { key: "status", title: "Status", render: (row) => row.status },
            { key: "request", title: "Requested", render: (row) => formatDateTime(row.request_at) },
            { key: "notes", title: "Notes", render: (row) => row.notes },
          ]}
        />
      </Panel>

      <div className="desk-component-grid">
        <CausalityTraceViewer trace={model.causalityTrace} />
        <ExecutionReplayViewer
          replay={model.executionReplay}
          selectedFrameIndex={model.executionReplay?.current_frame_index ?? 0}
          onSelectFrame={() => undefined}
        />
      </div>
    </div>
  );
}
