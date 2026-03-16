import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

export function OperatorTimelineScreen({ model }: { model: SystemModel }) {
  const truthState = panelTruthRenderState(model, "operatorTimeline");

  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
  }

  return (
    <div className="screen-grid desk-screen-grid">
      <Panel title="Operator timeline" subtitle="Chronological record of alerts, restarts, config changes, actions, and incidents.">
        <DataTable
          rows={model.operatorTimeline}
          rowKey={(row) => row.timeline_event_id}
          columns={[
            { key: "at", title: "At", render: (row) => formatDateTime(row.at) },
            { key: "category", title: "Category", render: (row) => row.category },
            { key: "severity", title: "Severity", render: (row) => row.severity },
            { key: "title", title: "Title", render: (row) => row.title },
            { key: "summary", title: "Summary", render: (row) => row.summary },
            { key: "actor", title: "Actor", render: (row) => row.actor },
            { key: "incident", title: "Incident", render: (row) => row.linked_incident_id ?? "—" },
            { key: "order", title: "Order", render: (row) => row.linked_order_id ?? "—" },
          ]}
        />
      </Panel>
    </div>
  );
}
