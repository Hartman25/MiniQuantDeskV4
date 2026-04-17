import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime, formatLabel, formatNumber } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

export function RuntimeScreen({ model }: { model: SystemModel }) {
  const runtime = model.runtimeLeadership;
  const truthState = panelTruthRenderState(model, "runtime");

  // Hard-close on any compromised truth state: degraded recovery state shown as clean
  // leadership truth would be misleading about the most critical runtime invariant.
  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
  }

  const leadershipClean = runtime.leader_lease_state === "held";
  const recoveryClean = runtime.post_restart_recovery_state === "complete";
  const hasRestarts = runtime.restart_count_24h !== null && runtime.restart_count_24h > 0;
  const needsAttention = !leadershipClean || !recoveryClean || hasRestarts;

  return (
    <div className="screen-grid desk-screen-grid">

      {/* Operator attention — only rendered when continuity is not clean */}
      {needsAttention && (
        <Panel
          title="Operator attention required"
          subtitle="One or more runtime continuity conditions must be resolved before relying on this runtime as authoritative truth."
        >
          <div className="operator-timeline-stack">
            {!leadershipClean && (
              <div className={`operator-timeline-card severity-${runtime.leader_lease_state === "lost" ? "critical" : "warning"}`}>
                <div className="operator-timeline-head">
                  <strong>Leadership interrupted — lease {formatLabel(runtime.leader_lease_state)}</strong>
                </div>
                <p className="operator-timeline-meta">
                  Leader node {runtime.leader_node} has not confirmed the lease. Confirm re-establishment before
                  treating any runtime output as authoritative. Do not start execution under contested or lost leadership.
                </p>
              </div>
            )}
            {!recoveryClean && (
              <div className="operator-timeline-card severity-warning">
                <div className="operator-timeline-head">
                  <strong>Recovery not complete — {formatLabel(runtime.post_restart_recovery_state)}</strong>
                </div>
                <p className="operator-timeline-meta">
                  Last checkpoint: {runtime.recovery_checkpoint}. Do not start execution until post-restart recovery
                  reaches complete. Review checkpoint timeline below.
                </p>
              </div>
            )}
            {hasRestarts && (
              <div className="operator-timeline-card severity-info">
                <div className="operator-timeline-head">
                  <strong>
                    Restart activity in last 24h — {runtime.restart_count_24h}{" "}
                    restart{runtime.restart_count_24h === 1 ? "" : "s"}
                  </strong>
                </div>
                <p className="operator-timeline-meta">
                  Last restart: {formatDateTime(runtime.last_restart_at)}. Review checkpoint timeline below for
                  restart and recovery sequence.
                </p>
              </div>
            )}
          </div>
        </Panel>
      )}

      {/* Runtime leadership and restart/recovery — two-column operator brief */}
      <div className="two-column-grid">
        <Panel title="Runtime leadership" subtitle="Current leader, lease continuity, and active generation.">
          <div className="metric-list">
            <div>
              <span>Leader node</span>
              <strong>{runtime.leader_node}</strong>
            </div>
            <div>
              <span>Lease state</span>
              <strong className={leadershipClean ? "state-ok" : "state-warning"}>
                {formatLabel(runtime.leader_lease_state)}
              </strong>
            </div>
            <div>
              <span>Generation</span>
              <strong>{runtime.generation_id}</strong>
            </div>
          </div>
        </Panel>

        <Panel title="Restart and recovery" subtitle="Restart count, last restart, and post-restart recovery progress.">
          <div className="metric-list">
            <div>
              <span>Restarts (24h)</span>
              <strong className={hasRestarts ? "state-warning" : undefined}>
                {runtime.restart_count_24h === null ? "—" : formatNumber(runtime.restart_count_24h)}
              </strong>
            </div>
            <div>
              <span>Last restart</span>
              <strong>{formatDateTime(runtime.last_restart_at)}</strong>
            </div>
            <div>
              <span>Recovery state</span>
              <strong className={recoveryClean ? "state-ok" : "state-warning"}>
                {formatLabel(runtime.post_restart_recovery_state)}
              </strong>
            </div>
            <div>
              <span>Recovery checkpoint</span>
              <strong>{runtime.recovery_checkpoint}</strong>
            </div>
          </div>
        </Panel>
      </div>

      {/* Checkpoint timeline — primary investigative body */}
      <Panel
        title="Runtime checkpoint timeline"
        subtitle="Ordered log of restart, leadership transition, and recovery events. Most recent first. Use this to trace restart sequences and confirm recovery completion."
      >
        <DataTable
          rows={[...runtime.checkpoints].reverse()}
          rowKey={(row) => row.checkpoint_id}
          columns={[
            { key: "timestamp", title: "Timestamp", render: (row) => formatDateTime(row.timestamp) },
            { key: "type", title: "Event", render: (row) => formatLabel(row.checkpoint_type) },
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
