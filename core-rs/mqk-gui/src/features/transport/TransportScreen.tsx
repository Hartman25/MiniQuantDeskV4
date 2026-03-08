import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { formatDateTime, formatDurationMs, formatLabel } from "../../lib/format";
import type { SystemModel } from "../system/types";

export function TransportScreen({ model }: { model: SystemModel }) {
  const t = model.transport;
  return (
    <div className="screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Outbox Depth" value={String(t.outbox_depth)} tone={t.outbox_depth > 0 ? "warn" : "good"} />
        <StatCard title="Inbox Depth" value={String(t.inbox_depth)} tone={t.inbox_depth > 0 ? "warn" : "good"} />
        <StatCard title="Oldest Claim Age" value={formatDurationMs(t.max_claim_age_ms)} tone={t.max_claim_age_ms > 180000 ? "bad" : "neutral"} />
        <StatCard title="Duplicate Inbox Events" value={String(t.duplicate_inbox_events)} tone={t.duplicate_inbox_events > 0 ? "warn" : "good"} />
      </div>
      <Panel title="Transport queue monitor" subtitle="First-class outbox/inbox supervision for claim age, retries, lag, orphaning, and duplicates.">
        <DataTable
          rows={t.queues}
          rowKey={(row) => row.queue_id}
          columns={[
            { key: "queue", title: "Queue", render: (row) => row.queue_id },
            { key: "direction", title: "Direction", render: (row) => formatLabel(row.direction) },
            { key: "status", title: "Status", render: (row) => row.status },
            { key: "depth", title: "Depth", render: (row) => String(row.depth) },
            { key: "age", title: "Oldest Age", render: (row) => formatDurationMs(row.oldest_age_ms) },
            { key: "retry", title: "Retries", render: (row) => String(row.retry_count) },
            { key: "dup", title: "Dupes", render: (row) => String(row.duplicate_events) },
            { key: "orphan", title: "Orphaned", render: (row) => String(row.orphaned_claims) },
            { key: "lag", title: "Lag", render: (row) => formatDurationMs(row.lag_ms) },
            { key: "last", title: "Last Activity", render: (row) => formatDateTime(row.last_activity_at) },
          ]}
        />
      </Panel>
    </div>
  );
}
