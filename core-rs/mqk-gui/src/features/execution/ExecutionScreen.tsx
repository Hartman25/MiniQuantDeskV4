import { useEffect, useState } from "react";
import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TimelineStageStrip } from "../../components/common/TimelineStageStrip";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime, formatDurationMs } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";
import { CausalityTraceViewer } from "./components/CausalityTraceViewer";
import { ExecutionReplayViewer } from "./components/ExecutionReplayViewer";
import { ExecutionChartPanel } from "./components/ExecutionChartPanel";
import { ExecutionTraceViewer } from "./components/ExecutionTraceViewer";
import { MetricStripChart } from "./components/MetricStripChart";
import { OmsStateMachineVisualizer } from "./components/OmsStateMachineVisualizer";
import { ReplaceCancelChainInspector } from "./components/ReplaceCancelChainInspector";

export function ExecutionScreen({
  model,
  onSelectTimeline,
  timelineLoading,
}: {
  model: SystemModel;
  onSelectTimeline: (internalOrderId: string) => void;
  timelineLoading: boolean;
}) {
  const timeline = model.selectedTimeline;
  const truthState = panelTruthRenderState(model, "execution");
  const [selectedReplayFrameIndex, setSelectedReplayFrameIndex] = useState(0);

  useEffect(() => {
    setSelectedReplayFrameIndex(model.executionReplay?.current_frame_index ?? 0);
  }, [model.executionReplay?.replay_id]);

  // Hard-close on any compromised truth state: stale execution orders and dispatching counts
  // must not render as current data. Inline notice is insufficient — operator sees data + warning,
  // reads data first, and acts on stale order state.
  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
  }

  return (
    <div className="screen-grid desk-screen-grid execution-workspace">
      <div className="summary-grid summary-grid-four">
        <StatCard
          title="Active Orders"
          value={String(model.executionSummary.active_orders)}
          detail="Working orders tracked by runtime"
          tone="good"
        />
        <StatCard
          title="Dispatching"
          value={String(model.executionSummary.dispatching_orders)}
          detail="Orders in dispatch path"
          tone={model.executionSummary.dispatching_orders > 0 ? "warn" : "neutral"}
        />
        <StatCard
          title="Rejects Today"
          value={String(model.executionSummary.reject_count_today)}
          detail="Total rejects since session open"
          tone={model.executionSummary.reject_count_today > 0 ? "warn" : "good"}
        />
        <StatCard
          title="Stuck OMS Orders"
          value={String(model.omsOverview.stuck_orders)}
          detail="Orders beyond state SLA"
          tone={model.omsOverview.stuck_orders > 0 ? "bad" : "good"}
        />
      </div>

      <div className="metrics-grid desk-panel-row">
        {model.metrics.execution.series.map((series) => (
          <MetricStripChart key={series.key} series={series} />
        ))}
      </div>

      <div className="summary-grid summary-grid-four">
        <StatCard
          title="Outbox Depth"
          value={String(model.transport.outbox_depth)}
          detail="Transport monitor"
          tone={model.transport.outbox_depth > 0 ? "warn" : "good"}
        />
        <StatCard
          title="Inbox Depth"
          value={String(model.transport.inbox_depth)}
          detail="Broker/event backlog"
          tone={model.transport.inbox_depth > 0 ? "warn" : "good"}
        />
        <StatCard
          title="Orphaned Claims"
          value={String(model.transport.orphaned_claims)}
          detail="Claim tokens needing attention"
          tone={model.transport.orphaned_claims > 0 ? "bad" : "good"}
        />
        <StatCard
          title="Dispatch Retries"
          value={String(model.transport.dispatch_retries)}
          detail="Retries across transport"
          tone={model.transport.dispatch_retries > 0 ? "warn" : "good"}
        />
      </div>

      <div className="execution-main-grid">
        <Panel title="In-flight orders" subtitle="Select a row to load timeline, trace, and replay.">
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
              { key: "updated", title: "Updated", render: (row) => formatDateTime(row.updated_at) },
              {
                key: "action",
                title: "Inspect",
                render: (row) => (
                  <button
                    type="button"
                    className="action-button small"
                    onClick={() => onSelectTimeline(row.internal_order_id)}
                  >
                    Load
                  </button>
                ),
              },
            ]}
          />
        </Panel>

        <div className="execution-side-stack">
          <Panel title="Selected order summary" compact>
            {timeline ? (
              <div className="metric-list compact-list">
                <div><span>Internal order</span><strong>{timeline.internal_order_id}</strong></div>
                <div><span>Broker order</span><strong>{timeline.broker_order_id ?? "—"}</strong></div>
                <div><span>Symbol</span><strong>{timeline.symbol}</strong></div>
                <div><span>Strategy</span><strong>{timeline.strategy_id}</strong></div>
                <div><span>Status</span><strong>{timeline.current_status}</strong></div>
                <div><span>Stage</span><strong>{timeline.current_stage}</strong></div>
                <div><span>Qty</span><strong>{timeline.filled_qty}/{timeline.requested_qty}</strong></div>
                <div><span>Updated</span><strong>{formatDateTime(timeline.last_updated_at)}</strong></div>
              </div>
            ) : (
              <div className="empty-state">Select an order to inspect execution detail.</div>
            )}
          </Panel>

          <Panel title="Timeline status" compact>
            <div className="metric-list compact-list">
              <div><span>Selection</span><strong>{timeline ? "Loaded" : "None"}</strong></div>
              <div><span>Timeline load</span><strong>{timelineLoading ? "Loading" : "Ready"}</strong></div>
              <div><span>Replay frames</span><strong>{model.executionReplay?.frames.length ?? 0}</strong></div>
              <div><span>Trace events</span><strong>{model.executionTrace?.timeline.length ?? 0}</strong></div>
            </div>
          </Panel>

          <ReplaceCancelChainInspector chains={model.replaceCancelChains} />
        </div>
      </div>

      <OmsStateMachineVisualizer overview={model.omsOverview} />

      <Panel title="Lifecycle stage strip" compact>
        {timeline ? (
          <TimelineStageStrip stages={timeline.stages} />
        ) : (
          <div className="empty-state">Select an order to view lifecycle stages.</div>
        )}
      </Panel>

      <div className="desk-component-grid">
        <ExecutionChartPanel
          chart={model.executionChart}
          replay={model.executionReplay}
          trace={model.executionTrace}
          selectedFrameIndex={selectedReplayFrameIndex}
          onSelectFrame={setSelectedReplayFrameIndex}
        />
        <ExecutionTraceViewer trace={model.executionTrace} />
      </div>

      <div className="desk-component-grid">
        <ExecutionReplayViewer
          replay={model.executionReplay}
          selectedFrameIndex={selectedReplayFrameIndex}
          onSelectFrame={setSelectedReplayFrameIndex}
        />
        <CausalityTraceViewer trace={model.causalityTrace} />
      </div>
    </div>
  );
}
