import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { formatDateTime, formatLabel } from "../../lib/format";
import type { SystemModel } from "../system/types";

export function SessionScreen({ model }: { model: SystemModel }) {
  const s = model.sessionState;
  return (
    <div className="screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Market Session" value={formatLabel(s.market_session)} tone={s.market_session === "closed" ? "warn" : "good"} />
        <StatCard title="Exchange State" value={formatLabel(s.exchange_calendar_state)} tone={s.exchange_calendar_state === "open" ? "good" : "warn"} />
        <StatCard title="Trading Window" value={formatLabel(s.system_trading_window)} tone={s.system_trading_window === "enabled" ? "good" : "warn"} />
        <StatCard title="Strategy Allowed" value={s.strategy_allowed ? "Yes" : "No"} tone={s.strategy_allowed ? "good" : "bad"} />
      </div>
      <Panel title="Session and market-state panel" subtitle="Session, exchange calendar, and system trading-window visibility for operator safety.">
        <div className="metric-list compact-list">
          <div><span>Next session change</span><strong>{formatDateTime(s.next_session_change_at)}</strong></div>
          <div><span>Notes</span><strong>{s.notes.join(" | ")}</strong></div>
        </div>
      </Panel>
    </div>
  );
}
