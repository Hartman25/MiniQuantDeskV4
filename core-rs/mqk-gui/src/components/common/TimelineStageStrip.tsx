import type { TimelineStage } from "../../features/system/types";
import { formatDurationMs, formatDateTime } from "../../lib/format";

export function TimelineStageStrip({ stages }: { stages: TimelineStage[] }) {
  return (
    <div className="timeline-strip">
      {stages.map((stage) => (
        <div key={stage.stage_key} className={`timeline-node status-${stage.status}`}>
          <div className="timeline-node-seq">{stage.sequence}</div>
          <div className="timeline-node-label">{stage.stage_label}</div>
          <div className="timeline-node-detail">{stage.details}</div>
          <div className="timeline-node-metrics">
            <span>{stage.started_at ? formatDateTime(stage.started_at) : "—"}</span>
            <span>{stage.duration_ms != null ? formatDurationMs(stage.duration_ms) : "—"}</span>
          </div>
        </div>
      ))}
    </div>
  );
}
