import { useEffect, useState } from "react";
import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TimelineStageStrip } from "../../components/common/TimelineStageStrip";
import { formatDateTime, formatDurationMs } from "../../lib/format";
import type { SystemModel } from "../system/types";
import { CausalityTraceViewer } from "./components/CausalityTraceViewer";
import { ExecutionReplayViewer } from "./components/ExecutionReplayViewer";
import { ExecutionChartPanel } from "./components/ExecutionChartPanel";
import { ExecutionTraceViewer } from "./components/ExecutionTraceViewer";
import { MetricStripChart } from "./components/MetricStripChart";
import { OmsStateMachineVisualizer } from "./components/OmsStateMachineVisualizer";
import { ReplaceCancelChainInspector } from "./components/ReplaceCancelChainInspector";

export function ExecutionScreen({ model, onSelectTimeline, timelineLoading }: { model: SystemModel; onSelectTimeline: (internalOrderId: string) => void; timelineLoading: boolean }) {
  const timeline = model.selectedTimeline;
  const [selectedReplayFrameIndex, setSelectedReplayFrameIndex] = useState(0);

  useEffect(() => {
    setSelectedReplayFrameIndex(model.executionReplay?.current_frame_index ?? 0);
  }, [model.executionReplay?.replay_id]);

  return (
    <div className="screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Active Orders" value={String(model.executionSummary.active_orders)} detail="Working orders tracked by runtime" tone="good" />
        <StatCard title="Dispatching" value={String(model.executionSummary.dispatching_orders)} detail="Orders in dispatch path" tone={model.executionSummary.dispatching_orders > 0 ? "warn" : "neutral"} />
        <StatCard title="Rejects Today" value={String(model.executionSummary.reject_count_today)} detail="Total rejects since session open" tone={model.executionSummary.reject_count_today > 0 ? "warn" : "good"} />
        <StatCard title="Stuck OMS Orders" value={String(model.omsOverview.stuck_orders)} detail="Orders beyond state SLA" tone={model.omsOverview.stuck_orders > 0 ? "bad" : "good"} />
      </div>

      <div className="metrics-grid">
        {model.metrics.execution.series.map((series) => (
          <MetricStripChart key={series.key} series={series} />
        ))}
      </div>

      <OmsStateMachineVisualizer overview={model.omsOverview} />

      <div className="summary-grid summary-grid-four">
        <StatCard title="Outbox Depth" value={String(model.transport.outbox_depth)} detail="Transport monitor" tone={model.transport.outbox_depth > 0 ? "warn" : "good"} />
        <StatCard title="Inbox Depth" value={String(model.transport.inbox_depth)} detail="Broker/event backlog" tone={model.transport.inbox_depth > 0 ? "warn" : "good"} />
        <StatCard title="Orphaned Claims" value={String(model.transport.orphaned_claims)} detail="Claim tokens needing attention" tone={model.transport.orphaned_claims > 0 ? "bad" : "good"} />
        <StatCard title="Dispatch Retries" value={String(model.transport.dispatch_retries)} detail="Retries across transport" tone={model.transport.dispatch_retries > 0 ? "warn" : "good"} />
      </div>

      <Panel title="In-flight orders" subtitle="Select a row to load its timeline, trace, and replay bundle.">
        <DataTable
          rows={model.executionOrders}
          rowKey={(row) => row.internal_order_id}
          columns={[
            { key: "order", title: "Order", render: (row) => row.internal_order_id },
            { key: "symbol", title: "Symbol", render: (row) => row.symbol },
            { key: "strategy", title: "Strategy", render: (row) => row.strategy_id },
            { key: "status", title: "Status", render: (row) => row.current_status },
            { key: "stage", title: "Stage", render: (row) => row.current_stage },
            { key: "qty", title: "Qty", render: (row) => `${row.filled_qty}/${row.requested_qty}` },
            { key: "age", title: "Age", render: (row) => formatDurationMs(row.age_ms) },
            {
              key: "actions",
              title: "Actions",
              render: (row) => (
                <button className="action-button small" onClick={() => onSelectTimeline(row.internal_order_id)}>
                  Load debug bundle
                </button>
              ),
            },
          ]}
        />
      </Panel>

      {timeline ? (
        <Panel title="Execution timeline viewer" subtitle={`${timeline.symbol} · ${timeline.strategy_id} · ${timeline.internal_order_id}`}>
          <div className="timeline-meta-grid">
            <div><span>Broker order</span><strong>{timeline.broker_order_id ?? "—"}</strong></div>
            <div><span>Current stage</span><strong>{timeline.current_stage}</strong></div>
            <div><span>Current status</span><strong>{timeline.current_status}</strong></div>
            <div><span>Requested / filled</span><strong>{timeline.requested_qty} / {timeline.filled_qty}</strong></div>
            <div><span>Opened</span><strong>{formatDateTime(timeline.opened_at)}</strong></div>
            <div><span>Last update</span><strong>{timelineLoading ? "Loading…" : formatDateTime(timeline.last_updated_at)}</strong></div>
          </div>
          <TimelineStageStrip stages={timeline.stages} />
        </Panel>
      ) : null}

      <div className="two-column-grid">
        <Panel title="Incident rail">
          <div className="list-stack">
            {timeline?.incident_events.map((incident) => (
              <div className={`alert-card severity-${incident.severity}`} key={incident.incident_id}>
                <div className="alert-header">
                  <strong>{incident.incident_type}</strong>
                  <span>{formatDateTime(incident.at)}</span>
                </div>
                <div className="alert-message">{incident.message}</div>
              </div>
            ))}
          </div>
        </Panel>

        <Panel title="Timeline event grid">
          <DataTable
            rows={timeline?.event_rows ?? []}
            rowKey={(row) => row.event_id}
            columns={[
              { key: "at", title: "Timestamp", render: (row) => formatDateTime(row.timestamp) },
              { key: "event", title: "Event", render: (row) => row.event_type },
              { key: "source", title: "Source", render: (row) => row.source_system },
              { key: "stage", title: "Stage", render: (row) => row.stage_key ?? "—" },
              { key: "message", title: "Message", render: (row) => row.message },
            ]}
          />
        </Panel>
      </div>

      <ReplaceCancelChainInspector chains={model.replaceCancelChains} />
      <ExecutionTraceViewer trace={model.executionTrace} />
      <ExecutionChartPanel chart={model.executionChart} replay={model.executionReplay} trace={model.executionTrace} selectedFrameIndex={selectedReplayFrameIndex} onSelectFrame={setSelectedReplayFrameIndex} />
      <CausalityTraceViewer trace={model.causalityTrace} />
      <ExecutionReplayViewer replay={model.executionReplay} selectedFrameIndex={selectedReplayFrameIndex} onSelectFrame={setSelectedReplayFrameIndex} />
    </div>
  );
}
