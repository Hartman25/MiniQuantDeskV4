import { formatDateTime } from "../../lib/format";
import type { FeedEvent } from "../../features/system/types";

export function BottomEventRail({ events, available = false }: { events: FeedEvent[]; available?: boolean }) {
  return (
    <section className="bottom-rail">
      <div className="bottom-rail-header">
        <span className="eyebrow">Recent events · bounded durable feed</span>
      </div>

      <div className="event-table">
        {!available ? <div className="unavailable-notice">Durable event feed unavailable; no-event status is unknown.</div> : events.length > 0 ? (
          events.map((event) => (
            <div key={event.id} className="table-row event-row">
              <div>{formatDateTime(event.at)}</div>
              <div>{event.severity}</div>
              <div>{event.source}</div>
              <div>{event.text}</div>
            </div>
          ))
        ) : (
          <div className="empty-state">No events returned by the bounded source.</div>
        )}
      </div>
    </section>
  );
}
