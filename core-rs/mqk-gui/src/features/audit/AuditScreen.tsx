import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { formatDateTime } from "../../lib/format";
import type { SystemModel } from "../system/types";
import { CausalityTraceViewer } from "../execution/components/CausalityTraceViewer";
import { ExecutionReplayViewer } from "../execution/components/ExecutionReplayViewer";

export function AuditScreen({ model }: { model: SystemModel }) {
  return (
    <div className="screen-grid">
      <Panel title="Operator audit trail" subtitle="Structured operator actions and resulting state echoes.">
        <DataTable
          rows={model.auditActions}
          rowKey={(row) => row.audit_ref}
          columns={[
            { key: "at", title: "At", render: (row) => formatDateTime(row.at) },
            { key: "actor", title: "Actor", render: (row) => row.actor },
            { key: "action", title: "Action", render: (row) => row.action_key },
            { key: "env", title: "Environment", render: (row) => row.environment },
            { key: "scope", title: "Scope", render: (row) => row.target_scope },
            { key: "state", title: "Result State", render: (row) => row.result_state },
          ]}
        />
      </Panel>
      <CausalityTraceViewer trace={model.causalityTrace} />
      <ExecutionReplayViewer replay={model.executionReplay} selectedFrameIndex={model.executionReplay?.current_frame_index ?? 0} onSelectFrame={() => undefined} />
      <Panel title="Event feed mirror" subtitle="Bottom rail is for glance view. This is the denser version.">
        <div className="list-stack">
          {model.feed.map((event) => (
            <div key={event.id} className={`alert-card severity-${event.severity}`}>
              <div className="alert-header">
                <strong>{event.source}</strong>
                <span>{formatDateTime(event.at)}</span>
              </div>
              <div className="alert-message">{event.text}</div>
            </div>
          ))}
        </div>
      </Panel>
    </div>
  );
}
