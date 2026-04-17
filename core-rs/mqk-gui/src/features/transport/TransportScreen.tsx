import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
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

  const hasOrphanedClaims = t.orphaned_claims > 0;
  const hasDuplicates = t.duplicate_inbox_events > 0;
  const hasRetries = t.dispatch_retries > 0;
  const hasExceptions = hasOrphanedClaims || hasDuplicates || hasRetries;
  const claimAgeBreached = t.max_claim_age_ms > 300000;
  const claimAgeWarning = t.max_claim_age_ms > 120000;

  const outboxQueues = t.queues.filter((q) => q.direction === "outbox");
  const inboxQueues = t.queues.filter((q) => q.direction === "inbox");

  return (
    <div className="screen-grid desk-screen-grid">

      {/* Exception triage — rendered only when transport has non-clean state */}
      {hasExceptions && (
        <Panel
          title="Transport exceptions — operator attention required"
          subtitle="One or more exception conditions are active. Investigate before relying on transport as healthy."
        >
          <div className="operator-timeline-stack">
            {hasOrphanedClaims && (
              <div className="operator-timeline-card severity-critical">
                <div className="operator-timeline-head">
                  <strong>
                    Orphaned claims — {t.orphaned_claims} claim{t.orphaned_claims === 1 ? "" : "s"} without a live owner
                  </strong>
                </div>
                <p className="operator-timeline-meta">
                  Orphaned claims indicate outbox tokens that survived a crash or restart without being resolved.
                  These block idempotent re-submission. Review per-queue detail below and resolve before starting execution.
                </p>
              </div>
            )}
            {hasDuplicates && (
              <div className="operator-timeline-card severity-warning">
                <div className="operator-timeline-head">
                  <strong>
                    Duplicate inbox events — {t.duplicate_inbox_events} duplicate{t.duplicate_inbox_events === 1 ? "" : "s"} detected
                  </strong>
                </div>
                <p className="operator-timeline-meta">
                  Duplicate inbox events indicate the broker sent the same event more than once, or the inbound
                  dedup path missed an idempotency key. Review inbox queue rows below for the affected queue.
                </p>
              </div>
            )}
            {hasRetries && (
              <div className="operator-timeline-card severity-warning">
                <div className="operator-timeline-head">
                  <strong>
                    Dispatch retries — {t.dispatch_retries} retr{t.dispatch_retries === 1 ? "y" : "ies"} in flight
                  </strong>
                </div>
                <p className="operator-timeline-meta">
                  Retry pressure means outbound submit attempts are not completing cleanly. If retry count is
                  rising, broker connectivity or outbox claim lifecycle may be impaired.
                </p>
              </div>
            )}
          </div>
        </Panel>
      )}

      {/* Queue backlog posture — outbox vs inbox separated */}
      <div className="two-column-grid">
        <Panel
          title="Outbox posture"
          subtitle={`${outboxQueues.length} outbound queue${outboxQueues.length === 1 ? "" : "s"} — work pending dispatch to broker`}
        >
          <div className="metric-list">
            <div>
              <span>Depth</span>
              <strong className={t.outbox_depth > 0 ? "state-warning" : undefined}>{t.outbox_depth}</strong>
            </div>
            <div>
              <span>Dispatch retries</span>
              <strong className={hasRetries ? "state-warning" : undefined}>{t.dispatch_retries}</strong>
            </div>
            <div>
              <span>Orphaned claims</span>
              <strong className={hasOrphanedClaims ? "state-critical" : undefined}>{t.orphaned_claims}</strong>
            </div>
            <div>
              <span>Max claim age</span>
              <strong className={claimAgeBreached ? "state-critical" : claimAgeWarning ? "state-warning" : undefined}>
                {formatDurationMs(t.max_claim_age_ms)}
              </strong>
            </div>
          </div>
        </Panel>

        <Panel
          title="Inbox posture"
          subtitle={`${inboxQueues.length} inbound queue${inboxQueues.length === 1 ? "" : "s"} — broker events pending apply`}
        >
          <div className="metric-list">
            <div>
              <span>Depth</span>
              <strong className={t.inbox_depth > 0 ? "state-warning" : undefined}>{t.inbox_depth}</strong>
            </div>
            <div>
              <span>Duplicate events</span>
              <strong className={hasDuplicates ? "state-warning" : undefined}>{t.duplicate_inbox_events}</strong>
            </div>
          </div>
        </Panel>
      </div>

      {/* Per-queue investigative detail — identify the first queue to investigate */}
      <Panel
        title="Queue investigative detail"
        subtitle="Per-queue breakdown of depth, lag, retry, duplicate, and orphaned-claim pressure. Lag and exception columns first."
      >
        <DataTable
          rows={t.queues}
          rowKey={(row) => row.queue_id}
          columns={[
            { key: "queue", title: "Queue", render: (row) => row.queue_id },
            { key: "direction", title: "Dir", render: (row) => row.direction },
            { key: "status", title: "Status", render: (row) => row.status },
            { key: "depth", title: "Depth", render: (row) => row.depth },
            { key: "lag", title: "Lag", render: (row) => formatDurationMs(row.lag_ms) },
            { key: "oldest", title: "Oldest Age", render: (row) => formatDurationMs(row.oldest_age_ms) },
            { key: "retries", title: "Retries", render: (row) => row.retry_count },
            { key: "dupes", title: "Dupes", render: (row) => row.duplicate_events },
            { key: "claims", title: "Orphaned", render: (row) => row.orphaned_claims },
            { key: "last", title: "Last Activity", render: (row) => formatDateTime(row.last_activity_at) },
            { key: "notes", title: "Notes", render: (row) => row.notes },
          ]}
        />
      </Panel>
    </div>
  );
}
