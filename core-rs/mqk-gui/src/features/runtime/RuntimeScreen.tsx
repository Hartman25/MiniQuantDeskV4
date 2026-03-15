import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime, formatLabel, formatNumber } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

export function RuntimeScreen({ model }: { model: SystemModel }) {
  const runtime = model.runtimeLeadership;
  const truthState = panelTruthRenderState(model, "runtime");

  if (truthState === "unimplemented" || truthState === "unavailable" || truthState === "no_snapshot") {
    return <TruthStateNotice state={truthState} />;
  }

  return (
    <div className="screen-grid desk-screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Leader Node" value={runtime.leader_node} detail={`Lease ${runtime.leader_lease_state}`} tone={runtime.leader_lease_state === "held" ? "good" : "warn"} />
        <StatCard title="Generation" value={runtime.generation_id} detail="Active runtime generation" tone="neutral" />
        <StatCard title="Restarts (24h)" value={formatNumber(runtime.restart_count_24h)} detail={`Last restart ${formatDateTime(runtime.last_restart_at)}`} tone={runtime.restart_count_24h === 0 ? "good" : "warn"} />
        <StatCard
          title="Recovery State"
          value={formatLabel(runtime.post_restart_recovery_state)}
          detail={`Checkpoint ${runtime.recovery_checkpoint}`}
          tone={runtime.post_restart_recovery_state === "complete" ? "good" : "warn"}
        />
      </div>

      <Panel title="Runtime checkpoints" subtitle="Restart and leadership transitions captured by runtime state.">
        <DataTable
          rows={runtime.checkpoints}
          rowKey={(row) => row.checkpoint_id}
          columns={[
            { key: "timestamp", title: "Timestamp", render: (row) => formatDateTime(row.timestamp) },
            { key: "type", title: "Checkpoint", render: (row) => formatLabel(row.checkpoint_type) },
            { key: "status", title: "Status", render: (row) => formatLabel(row.status) },
            { key: "generation", title: "Generation", render: (row) => row.generation_id },
            { key: "leader", title: "Leader Node", render: (row) => row.leader_node },
            { key: "note", title: "Note", render: (row) => row.note },
          ]}
        />
      </Panel>
    </div>
  );
}
