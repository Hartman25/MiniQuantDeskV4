import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime, formatDurationMs } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

export function TransportScreen({ model }: { model: SystemModel }) {
  const t = model.transport;
  const truthState = panelTruthRenderState(model, "transport");

  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
  }

  return (
    <div className="screen-grid desk-screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Outbox Depth" value={String(t.outbox_depth)} detail="Queued outbound work" tone={t.outbox_depth > 0 ? "warn" : "good"} />
        <StatCard title="Inbox Depth" value={String(t.inbox_depth)} detail="Queued inbound broker events" tone={t.inbox_depth > 0 ? "warn" : "good"} />
        <StatCard title="Max Claim Age" value={formatDurationMs(t.max_claim_age_ms)} detail="Oldest live claim token" tone={t.max_claim_age_ms > 300000 ? "bad" : t.max_claim_age_ms > 120000 ? "warn" : "good"} />
        <StatCard title="Orphaned Claims" value={String(t.orphaned_claims)} detail="Claims needing attention" tone={t.orphaned_claims > 0 ? "bad" : "good"} />
      </div>

      <Panel title="Transport queues" subtitle="Primary view for outbox/inbox backlog, lag, duplicates, and orphaned claims.">
        <DataTable
          rows={t.queues}
          rowKey={(row) => row.queue_id}
          columns={[
            { key: "queue", title: "Queue", render: (row) => row.queue_id },
            { key: "direction", title: "Direction", render: (row) => row.direction },
            { key: "status", title: "Status", render: (row) => row.status },
            { key: "depth", title: "Depth", render: (row) => row.depth },
            { key: "oldest", title: "Oldest Age", render: (row) => formatDurationMs(row.oldest_age_ms) },
            { key: "lag", title: "Lag", render: (row) => formatDurationMs(row.lag_ms) },
            { key: "retries", title: "Retries", render: (row) => row.retry_count },
            { key: "dupes", title: "Duplicates", render: (row) => row.duplicate_events },
            { key: "claims", title: "Orphaned Claims", render: (row) => row.orphaned_claims },
            { key: "last", title: "Last Activity", render: (row) => formatDateTime(row.last_activity_at) },
            { key: "notes", title: "Notes", render: (row) => row.notes },
          ]}
        />
      </Panel>
    </div>
  );
}
