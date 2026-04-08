import { Panel } from "../../../components/common/Panel";
import { formatDateTime } from "../../../lib/format";
import type { OrderReplayResponse } from "../../system/types";

function replayUnavailableNotice(replay: OrderReplayResponse): string | null {
  switch (replay.truth_state) {
    case "no_db":
      return "No database connection — replay unavailable. Connect a DB to view fill telemetry.";
    case "no_order":
      return "Order not found in any current authoritative source. It may be from a prior run or has not been submitted yet.";
    case "no_fills_yet":
      return "Order is visible in the OMS snapshot but no fill events have been received yet.";
    default:
      return null;
  }
}

export function ExecutionReplayViewer({ replay, selectedFrameIndex, onSelectFrame }: { replay: OrderReplayResponse | null; selectedFrameIndex: number; onSelectFrame: (index: number) => void }) {
  if (!replay) {
    return <Panel title="Execution replay viewer"><div className="empty-state">Replay is unavailable until a traceable order is selected.</div></Panel>;
  }

  const notice = replayUnavailableNotice(replay);
  const activeFrame = replay.frames[selectedFrameIndex] ?? replay.frames[replay.current_frame_index] ?? replay.frames[0];

  return (
    <Panel title="Execution replay viewer" subtitle={`${replay.title} · ${replay.source}`}>
      {notice && (
        <div className="unavailable-notice">{notice}</div>
      )}

      <div className="replay-toolbar">
        <button className="action-button small ghost" type="button">◀ Step</button>
        <button className="action-button small" type="button">Play</button>
        <button className="action-button small ghost" type="button">Pause</button>
        <button className="action-button small ghost" type="button">Jump to anomaly</button>
      </div>

      <div className="timeline-meta-grid">
        <div><span>Truth state</span><strong>{replay.truth_state}</strong></div>
        <div><span>Frame</span><strong>{replay.frames.length > 0 ? `${selectedFrameIndex + 1} / ${replay.frames.length}` : "—"}</strong></div>
        <div><span>Timestamp</span><strong>{formatDateTime(activeFrame?.timestamp ?? null)}</strong></div>
        <div><span>OMS</span><strong>{activeFrame?.oms_state ?? "—"}</strong></div>
        <div><span>Execution</span><strong>{activeFrame?.order_execution_state ?? "—"}</strong></div>
        <div><span>Risk</span><strong>{activeFrame?.risk_state ?? "—"}</strong></div>
        <div><span>Reconcile</span><strong>{activeFrame?.reconcile_state ?? "—"}</strong></div>
      </div>

      {replay.frames.length > 0 && (
        <>
          <div className="replay-scrubber" aria-hidden="true">
            {replay.frames.map((frame, index) => (
              <span key={frame.frame_id} className={`replay-dot ${index === selectedFrameIndex ? "is-active" : ""} ${frame.anomaly_tags.length > 0 ? "is-alert" : ""}`} />
            ))}
          </div>

          <div className="list-stack">
            {replay.frames.map((frame, index) => (
              <button type="button" onClick={() => onSelectFrame(index)} key={frame.frame_id} className={`replay-frame-card ${index === selectedFrameIndex ? "is-selected" : ""}`}>
                <div className="alert-header">
                  <strong>{frame.event_type}</strong>
                  <span>{formatDateTime(frame.timestamp)}</span>
                </div>
                <div className="summary-detail">{frame.state_delta} · {frame.message_digest}</div>
                <div className="summary-detail">Qty {frame.filled_qty} filled / {frame.open_qty ?? "—"} open · Queue {frame.queue_status}</div>
                {frame.anomaly_tags.length > 0 ? <div className="summary-detail">Anomalies: {frame.anomaly_tags.join(", ")}</div> : null}
                {frame.boundary_tags.length > 0 ? <div className="summary-detail">Boundaries: {frame.boundary_tags.join(", ")}</div> : null}
              </button>
            ))}
          </div>
        </>
      )}
    </Panel>
  );
}
