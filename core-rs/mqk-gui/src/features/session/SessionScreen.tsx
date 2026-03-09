import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { formatDateTime } from "../../lib/format";
import type { SystemModel } from "../system/types";

export function SessionScreen({ model }: { model: SystemModel }) {
  const s = model.sessionState;

  return (
    <div className="screen-grid desk-screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Market Session" value={s.market_session} detail="Current exchange session" tone="good" />
        <StatCard title="Calendar State" value={s.exchange_calendar_state} detail="Exchange calendar view" tone="neutral" />
        <StatCard title="Trading Window" value={s.system_trading_window} detail="System-level trade permission" tone={s.system_trading_window === "enabled" ? "good" : "warn"} />
        <StatCard title="Strategy Allowed" value={s.strategy_allowed ? "Yes" : "No"} detail={`Next change ${formatDateTime(s.next_session_change_at)}`} tone={s.strategy_allowed ? "good" : "bad"} />
      </div>

      <Panel title="Session notes" subtitle="Operator session context and upcoming transitions.">
        <div className="list-stack">
          {s.notes.map((note) => (
            <div key={note} className="list-row">
              <strong>Note</strong>
              <span>{note}</span>
            </div>
          ))}
        </div>
      </Panel>
    </div>
  );
}
