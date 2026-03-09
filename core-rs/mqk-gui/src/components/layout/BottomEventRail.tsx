import { formatDateTime } from "../../lib/format";
import type { FeedEvent } from "../../features/system/types";

export function BottomEventRail({ events }: { events: FeedEvent[] }) {
  return (
    <section className="bottom-rail panel">
      <div className="panel-head">
        <div>
          <div className="eyebrow">Event Rail</div>
          <h3>Recent system events</h3>
        </div>
      </div>

      <div className="event-table">
        {events.length > 0 ? (
          events.map((event) => (
            <div key={event.id} className="table-row event-row">
              <div>{formatDateTime(event.at)}</div>
              <div>{event.severity}</div>
              <div>{event.source}</div>
              <div>{event.text}</div>
            </div>
          ))
        ) : (
          <div className="empty-state">No events available.</div>
        )}
      </div>
    </section>
  );
}
