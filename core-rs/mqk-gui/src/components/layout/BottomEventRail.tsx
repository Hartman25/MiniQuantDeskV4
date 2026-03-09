import { formatDateTime } from "../../lib/format";
import type { FeedEvent } from "../../features/system/types";

interface BottomEventRailProps {
  events: FeedEvent[];
}

export function BottomEventRail({ events }: BottomEventRailProps) {
  return (
    <section className="bottom-rail panel">
      <div className="panel-header">
        <div>
          <div className="eyebrow">Event Stream</div>
          <h2>Operator-visible feed</h2>
        </div>
      </div>
      <div className="bottom-rail-scroll">
        <div className="event-table">
          <div className="event-table-head">
            <span>Time</span>
            <span>Severity</span>
            <span>Source</span>
            <span>Message</span>
          </div>
          {events.map((event) => (
            <div key={event.id} className={`event-row tone-${event.severity}`}>
              <span>{formatDateTime(event.at)}</span>
              <span>{event.severity}</span>
              <span>{event.source}</span>
              <span>{event.text}</span>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
