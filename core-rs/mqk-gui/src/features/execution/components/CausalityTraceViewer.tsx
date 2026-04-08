import { Panel } from "../../../components/common/Panel";
import { formatDateTime, formatDurationMs, formatLabel } from "../../../lib/format";
import type { OrderCausalityResponse } from "../../system/types";

function causalityUnavailableNotice(trace: OrderCausalityResponse): string | null {
  switch (trace.truth_state) {
    case "no_db":
      return "No database connection — causality unavailable. Connect a DB to view fill telemetry.";
    case "no_order":
      return "Order not found in any current authoritative source. It may be from a prior run or has not been submitted yet.";
    case "no_fills_yet":
      return "Order is visible in the OMS snapshot but no fill events have been received yet.";
    default:
      return null;
  }
}

export function CausalityTraceViewer({ trace }: { trace: OrderCausalityResponse | null }) {
  if (!trace) {
    return (
      <Panel title="Causality trace viewer">
        <div className="empty-state">Select an order to inspect upstream signal, intent, execution, portfolio, and reconcile causality.</div>
      </Panel>
    );
  }

  const notice = causalityUnavailableNotice(trace);

  return (
    <Panel
      title="Causality trace viewer"
      subtitle={
        trace.symbol
          ? `${trace.symbol} · ${trace.order_id} · ${trace.truth_state}`
          : `${trace.order_id} · ${trace.truth_state}`
      }
    >
      {notice && <div className="unavailable-notice">{notice}</div>}

      <div className="timeline-meta-grid">
        <div><span>Truth state</span><strong>{trace.truth_state}</strong></div>
        <div><span>Backend</span><strong>{trace.backend}</strong></div>
        <div><span>Order ID</span><strong>{trace.order_id}</strong></div>
        {trace.symbol ? <div><span>Symbol</span><strong>{trace.symbol}</strong></div> : null}
      </div>

      {/* Proven and unproven lanes */}
      <div className="two-column-grid">
        <Panel title="Proven causality lanes" compact>
          {trace.proven_lanes.length === 0 ? (
            <div className="empty-state">No lanes proven — {trace.truth_state}.</div>
          ) : (
            <div className="list-stack">
              {trace.proven_lanes.map((lane) => (
                <div key={lane} className="alert-card severity-info">
                  <div className="alert-header">
                    <strong>{formatLabel(lane)}</strong>
                    <span>proven</span>
                  </div>
                </div>
              ))}
            </div>
          )}
        </Panel>

        <Panel title="Unproven causality lanes" compact>
          <div className="list-stack">
            {trace.unproven_lanes.map((lane) => (
              <div key={lane} className="alert-card severity-warning">
                <div className="alert-header">
                  <strong>{formatLabel(lane)}</strong>
                  <span>not joinable</span>
                </div>
                <div className="alert-message">Not linked to internal_order_id in current schema.</div>
              </div>
            ))}
          </div>
        </Panel>
      </div>

      {/* Fill-derived execution nodes */}
      {trace.nodes.length > 0 && (
        <Panel title="Execution-fill causality nodes" compact>
          <div className="causality-node-grid">
            {trace.nodes.map((node) => (
              <div key={node.node_key} className="causality-node-card severity-info">
                <div className="alert-header">
                  <strong>{node.title}</strong>
                  <span>{node.timestamp ? formatDateTime(node.timestamp) : "—"}</span>
                </div>
                <div className="summary-detail">{formatLabel(node.node_type)} · {formatLabel(node.subsystem)}</div>
                <div className="summary-detail">
                  Linked: {node.linked_id ?? "—"} · Δ {formatDurationMs(node.elapsed_from_prev_ms)}
                </div>
                <div className="summary-detail">{node.summary}</div>
              </div>
            ))}
          </div>
        </Panel>
      )}

      {/* Honest comment */}
      <Panel title="Causality scope" compact>
        <div className="summary-detail">{trace.comment}</div>
      </Panel>
    </Panel>
  );
}
