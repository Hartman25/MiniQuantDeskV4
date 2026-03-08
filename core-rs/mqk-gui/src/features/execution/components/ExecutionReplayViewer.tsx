import { Panel } from "../../../components/common/Panel";
import { formatDateTime } from "../../../lib/format";
import type { ExecutionReplay } from "../../system/types";

export function ExecutionReplayViewer({ replay, selectedFrameIndex, onSelectFrame }: { replay: ExecutionReplay | null; selectedFrameIndex: number; onSelectFrame: (index: number) => void }) {
  if (!replay) {
    return <Panel title="Execution replay viewer"><div className="empty-state">Replay is unavailable until a traceable order is selected.</div></Panel>;
  }

  const activeFrame = replay.frames[selectedFrameIndex] ?? replay.frames[replay.current_frame_index] ?? replay.frames[0];

  return (
    <Panel title="Execution replay viewer" subtitle={`${replay.title} · ${replay.source}`}>
      <div className="replay-toolbar">
        <button className="action-button small ghost" type="button">◀ Step</button>
        <button className="action-button small" type="button">Play</button>
        <button className="action-button small ghost" type="button">Pause</button>
        <button className="action-button small ghost" type="button">Jump to anomaly</button>
      </div>

      <div className="timeline-meta-grid">
        <div><span>Frame</span><strong>{selectedFrameIndex + 1} / {replay.frames.length}</strong></div>
        <div><span>Timestamp</span><strong>{formatDateTime(activeFrame?.timestamp ?? null)}</strong></div>
        <div><span>OMS</span><strong>{activeFrame?.oms_state ?? "—"}</strong></div>
        <div><span>Execution</span><strong>{activeFrame?.order_execution_state ?? "—"}</strong></div>
        <div><span>Risk</span><strong>{activeFrame?.risk_state ?? "—"}</strong></div>
        <div><span>Reconcile</span><strong>{activeFrame?.reconcile_state ?? "—"}</strong></div>
      </div>

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
            <div className="summary-detail">Qty {frame.filled_qty} filled / {frame.open_qty} open · Queue {frame.queue_status}</div>
            {frame.anomaly_tags.length > 0 ? <div className="summary-detail">Anomalies: {frame.anomaly_tags.join(", ")}</div> : null}
            {frame.boundary_tags.length > 0 ? <div className="summary-detail">Boundaries: {frame.boundary_tags.join(", ")}</div> : null}
          </button>
        ))}
      </div>
    </Panel>
  );
}
