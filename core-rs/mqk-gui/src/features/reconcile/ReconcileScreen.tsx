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
    <div className="screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Reconcile Status" value={r.status} detail={`Last run ${formatDateTime(r.last_run_at)}`} tone={r.status === "critical" ? "bad" : r.status === "warning" ? "warn" : "good"} />
        <StatCard title="Mismatched Positions" value={String(r.mismatched_positions)} tone={r.mismatched_positions > 0 ? "warn" : "good"} />
        <StatCard title="Mismatched Orders" value={String(r.mismatched_orders)} tone={r.mismatched_orders > 0 ? "warn" : "good"} />
        <StatCard title="Unmatched Events" value={String(r.unmatched_broker_events)} tone={r.unmatched_broker_events > 0 ? "warn" : "good"} />
      </div>
      <Panel title="Mismatch grid">
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
      <CausalityTraceViewer trace={model.causalityTrace} />
      <ExecutionReplayViewer replay={model.executionReplay} selectedFrameIndex={model.executionReplay?.current_frame_index ?? 0} onSelectFrame={() => undefined} />
    </div>
  );
}
