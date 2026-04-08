import { DataTable } from "../../../components/common/DataTable";
import { Panel } from "../../../components/common/Panel";
import { formatDateTime, formatDurationMs } from "../../../lib/format";
import type { OrderTraceResponse } from "../../system/types";

function traceUnavailableNotice(trace: OrderTraceResponse): string | null {
  switch (trace.truth_state) {
    case "no_db":
      return "No database connection — trace unavailable. Connect a DB to view fill telemetry.";
    case "no_order":
      return "Order not found in any current authoritative source. It may have been from a prior run or has not been submitted yet.";
    case "no_fills_yet":
      return "Order is visible in the OMS snapshot but no fill events have been received yet.";
    default:
      return null;
  }
}

export function ExecutionTraceViewer({ trace }: { trace: OrderTraceResponse | null }) {
  if (!trace) {
    return (
      <Panel title="Execution trace viewer">
        <div className="empty-state">Select an order to inspect fill telemetry and execution trace.</div>
      </Panel>
    );
  }

  const notice = traceUnavailableNotice(trace);

  return (
    <Panel
      title="Execution trace viewer"
      subtitle={
        trace.symbol && trace.order_id
          ? `${trace.symbol} · ${trace.order_id}`
          : trace.order_id
      }
    >
      {notice && (
        <div className="unavailable-notice">{notice}</div>
      )}

      <div className="timeline-meta-grid">
        <div><span>Truth state</span><strong>{trace.truth_state}</strong></div>
        <div><span>Order ID</span><strong>{trace.order_id}</strong></div>
        <div><span>Broker order</span><strong>{trace.broker_order_id ?? "—"}</strong></div>
        <div><span>OMS status</span><strong>{trace.current_status ?? "—"}</strong></div>
        <div><span>Stage</span><strong>{trace.current_stage ?? "—"}</strong></div>
        <div><span>Outbox</span><strong>{trace.outbox_status ?? "—"} {trace.outbox_lifecycle_stage ? `(${trace.outbox_lifecycle_stage})` : ""}</strong></div>
        <div><span>Requested qty</span><strong>{trace.requested_qty ?? "—"}</strong></div>
        <div><span>Filled qty</span><strong>{trace.filled_qty ?? "—"}</strong></div>
        <div><span>Last event</span><strong>{trace.last_event_at ? formatDateTime(trace.last_event_at) : "—"}</strong></div>
        <div><span>Backend</span><strong>{trace.backend}</strong></div>
      </div>

      <Panel title="Fill events" compact>
        {trace.rows.length === 0 ? (
          <div className="empty-state">
            {trace.truth_state === "no_fills_yet"
              ? "No fill events yet — order is pending execution."
              : "No fill events."}
          </div>
        ) : (
          <DataTable
            rows={trace.rows}
            rowKey={(row) => row.event_id}
            columns={[
              { key: "ts_utc", title: "Timestamp", render: (row) => formatDateTime(row.ts_utc) },
              { key: "stage", title: "Stage", render: (row) => row.stage },
              { key: "side", title: "Side", render: (row) => row.side ?? "—" },
              { key: "fill_qty", title: "Fill Qty", render: (row) => row.fill_qty ?? "—" },
              { key: "fill_price_micros", title: "Price (µ$)", render: (row) => row.fill_price_micros ?? "—" },
              { key: "slippage_bps", title: "Slippage bps", render: (row) => row.slippage_bps ?? "—" },
              { key: "submit_to_fill_ms", title: "Submit→Fill", render: (row) => row.submit_to_fill_ms != null ? formatDurationMs(row.submit_to_fill_ms) : "—" },
              { key: "detail", title: "Detail", render: (row) => row.detail ?? "—" },
            ]}
          />
        )}
      </Panel>
    </Panel>
  );
}
