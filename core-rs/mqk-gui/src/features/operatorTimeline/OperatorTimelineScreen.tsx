import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";
import type { OperatorTimelineCategory } from "../system/types/core";
import type { OperatorTimelineEvent } from "../system/types/ops";

// How many non-null cross-domain link fields this event carries.
// Events with 2+ links cross multiple system domains — highest reconstruction priority.
function linkCount(ev: OperatorTimelineEvent): number {
  return [
    ev.linked_incident_id,
    ev.linked_order_id,
    ev.linked_strategy_id,
    ev.linked_action_key,
    ev.linked_config_diff_id,
    ev.linked_runtime_generation_id,
  ].filter((v) => v != null).length;
}

// Group events by a shared linkage key. Returns only clusters with 2+ events —
// singletons have no cross-event investigative value as clusters.
function groupByLinkKey(
  events: OperatorTimelineEvent[],
  keyFn: (ev: OperatorTimelineEvent) => string | null,
): Map<string, OperatorTimelineEvent[]> {
  const acc = new Map<string, OperatorTimelineEvent[]>();
  for (const ev of events) {
    const key = keyFn(ev);
    if (key == null) continue;
    const group = acc.get(key);
    if (group == null) { acc.set(key, [ev]); } else { group.push(ev); }
  }
  const result = new Map<string, OperatorTimelineEvent[]>();
  for (const [key, group] of acc) {
    if (group.length >= 2) result.set(key, group);
  }
  return result;
}

const CATEGORY_ORDER: readonly OperatorTimelineCategory[] = [
  "incident",
  "alert",
  "operator_action",
  "runtime_restart",
  "mode_transition",
  "config_change",
  "reconcile",
  "runtime_transition",
];

export function OperatorTimelineScreen({ model }: { model: SystemModel }) {
  const truthState = panelTruthRenderState(model, "operatorTimeline");

  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
  }

  // Newest first for all display surfaces.
  const events = [...model.operatorTimeline].sort(
    (a, b) => new Date(b.at).getTime() - new Date(a.at).getTime(),
  );

  // Category posture — how many events per OperatorTimelineCategory.
  const byCategory = new Map<string, number>();
  for (const ev of events) {
    byCategory.set(ev.category, (byCategory.get(ev.category) ?? 0) + 1);
  }

  // Cross-domain linkage clusters — events sharing the same linkage key.
  // Each dimension is a separate investigation axis.
  const incidentClusters    = groupByLinkKey(events, (ev) => ev.linked_incident_id);
  const orderClusters       = groupByLinkKey(events, (ev) => ev.linked_order_id);
  const strategyClusters    = groupByLinkKey(events, (ev) => ev.linked_strategy_id);
  const runtimeGenClusters  = groupByLinkKey(events, (ev) => ev.linked_runtime_generation_id);
  const configDiffClusters  = groupByLinkKey(events, (ev) => ev.linked_config_diff_id);
  const actionKeyClusters   = groupByLinkKey(events, (ev) => ev.linked_action_key);

  const hasAnyClusters =
    incidentClusters.size > 0 ||
    orderClusters.size > 0 ||
    strategyClusters.size > 0 ||
    runtimeGenClusters.size > 0 ||
    configDiffClusters.size > 0 ||
    actionKeyClusters.size > 0;

  // High-linkage events: 2+ non-null cross-domain links.
  // These cross multiple system domains and are the first place to start
  // when reconstructing a session sequence.
  const highLinkage = events.filter((ev) => linkCount(ev) >= 2);

  const clusterDimensions = [
    { dimension: "incident",          clusters: incidentClusters },
    { dimension: "runtime generation", clusters: runtimeGenClusters },
    { dimension: "order",             clusters: orderClusters },
    { dimension: "strategy",          clusters: strategyClusters },
    { dimension: "config diff",       clusters: configDiffClusters },
    { dimension: "action key",        clusters: actionKeyClusters },
  ].filter(({ clusters }) => clusters.size > 0);

  return (
    <div className="screen-grid desk-screen-grid">

      {/* Category posture — which domains are dominating the current chronology.
          This is the first question Timeline owns: not "were there alerts?" but
          "which categories are present and in what proportion?" */}
      <Panel
        title="Category posture"
        subtitle={
          events.length === 0
            ? "No timeline events recorded."
            : `${events.length} event${events.length === 1 ? "" : "s"} across ${byCategory.size} categor${byCategory.size === 1 ? "y" : "ies"} — which domains are active in the current chronology.`
        }
      >
        {events.length === 0 ? (
          <div className="empty-state">No timeline events recorded yet.</div>
        ) : (
          <div className="timeline-category-strip">
            {CATEGORY_ORDER.map((cat) => {
              const count = byCategory.get(cat) ?? 0;
              if (count === 0) return null;
              return (
                <div key={cat} className="timeline-category-pill">
                  <span className="timeline-category-label">{cat.replace(/_/g, " ")}</span>
                  <span className="timeline-category-count">{count}</span>
                </div>
              );
            })}
          </div>
        )}
      </Panel>

      {/* Cross-domain linkage clusters — where to begin reconstruction.
          Grouping by shared linkage key reveals cross-domain sequences:
          multiple events sharing the same incident, order, strategy, runtime
          generation, config diff, or action key signal a connected event chain.
          No other screen groups by these dimensions. */}
      <Panel
        title="Cross-domain linkage clusters"
        subtitle={
          !hasAnyClusters
            ? "No multi-event linkage clusters in current timeline. Each event is independent."
            : "Events sharing a linkage key form a cross-domain chain. Start reconstruction from the largest cluster."
        }
      >
        {!hasAnyClusters ? (
          <div className="empty-state">No linkage clusters. All events are independent or the timeline is sparse.</div>
        ) : (
          <div className="operator-timeline-stack">
            {clusterDimensions.map(({ dimension, clusters }) =>
              [...clusters.entries()].map(([key, clusterEvents]) => (
                <div key={`${dimension}-${key}`} className="operator-timeline-card severity-info">
                  <div className="operator-timeline-head">
                    <strong>
                      {dimension}: <code>{key}</code>
                    </strong>
                    <span className="operator-timeline-meta">
                      {clusterEvents.length} linked events
                    </span>
                  </div>
                  <div className="operator-timeline-meta">
                    {clusterEvents.map((ev) => (
                      <span key={ev.timeline_event_id}>
                        {formatDateTime(ev.at)} · {ev.category.replace(/_/g, " ")} · {ev.title}
                      </span>
                    ))}
                  </div>
                </div>
              ))
            )}
          </div>
        )}
      </Panel>

      {/* High-linkage events — cross-domain reconstruction priority.
          An event with 2+ cross-domain links touches multiple system domains
          simultaneously. These are the highest-priority starting points when
          the operator needs to reconstruct what happened and in what order. */}
      {highLinkage.length > 0 && (
        <Panel
          title="High-linkage events — cross-domain reconstruction priority"
          subtitle={`${highLinkage.length} event${highLinkage.length === 1 ? "" : "s"} with 2 or more cross-domain links. These touch multiple system domains and are the first place to start when reconstructing a session sequence.`}
        >
          <div className="operator-timeline-stack">
            {highLinkage.map((ev) => {
              const links = [
                ev.linked_incident_id         && `incident: ${ev.linked_incident_id}`,
                ev.linked_order_id            && `order: ${ev.linked_order_id}`,
                ev.linked_strategy_id         && `strategy: ${ev.linked_strategy_id}`,
                ev.linked_action_key          && `action: ${ev.linked_action_key}`,
                ev.linked_config_diff_id      && `diff: ${ev.linked_config_diff_id}`,
                ev.linked_runtime_generation_id && `gen: ${ev.linked_runtime_generation_id}`,
              ].filter(Boolean) as string[];

              return (
                <div
                  key={ev.timeline_event_id}
                  className={`operator-timeline-card severity-${ev.severity}`}
                >
                  <div className="operator-timeline-head">
                    <strong>
                      {ev.category.replace(/_/g, " ")} · {ev.title}
                    </strong>
                    <span className="operator-timeline-meta">{formatDateTime(ev.at)}</span>
                  </div>
                  <div className="operator-timeline-meta">
                    {links.map((link) => (
                      <span key={link}>{link}</span>
                    ))}
                  </div>
                  {ev.summary && (
                    <div className="operator-timeline-meta">{ev.summary}</div>
                  )}
                </div>
              );
            })}
          </div>
        </Panel>
      )}

      {/* Full event ledger — demoted to secondary surface.
          All cross-link columns shown so the operator can trace any event to
          its linked domain objects without switching screens. */}
      <Panel
        title="Full event ledger — complete chronological record"
        subtitle="All timeline events, newest first. Use the linkage columns to navigate cross-domain sequences."
        compact
      >
        {events.length === 0 ? (
          <div className="empty-state">No timeline events recorded yet.</div>
        ) : (
          <DataTable
            rows={events}
            rowKey={(row) => row.timeline_event_id}
            columns={[
              { key: "at",       title: "At",       render: (row) => formatDateTime(row.at) },
              { key: "category", title: "Category", render: (row) => row.category.replace(/_/g, " ") },
              { key: "severity", title: "Sev",      render: (row) => row.severity },
              { key: "title",    title: "Title",    render: (row) => row.title },
              { key: "actor",    title: "Actor",    render: (row) => row.actor ?? "—" },
              { key: "incident", title: "Incident", render: (row) => row.linked_incident_id ?? "—" },
              { key: "order",    title: "Order",    render: (row) => row.linked_order_id ?? "—" },
              { key: "strategy", title: "Strategy", render: (row) => row.linked_strategy_id ?? "—" },
              { key: "action",   title: "Action",   render: (row) => row.linked_action_key ?? "—" },
              { key: "gen",      title: "Gen",      render: (row) => row.linked_runtime_generation_id ?? "—" },
            ]}
          />
        )}
      </Panel>
    </div>
  );
}
