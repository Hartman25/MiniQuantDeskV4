import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

export function SessionScreen({ model }: { model: SystemModel }) {
  const s = model.sessionState;
  const truthState = panelTruthRenderState(model, "session");

  if (truthState === "unimplemented" || truthState === "unavailable" || truthState === "no_snapshot") {
    return <TruthStateNotice state={truthState} />;
  }

  return (
    <div className="screen-grid desk-screen-grid">
      <p className="panel-subtitle" aria-label="Source authority legend">
        Source labels: DB truth = persisted records, Runtime memory = in-process state, Broker snapshot = latest broker fetch, Placeholder = mock/fallback, Mixed = multiple sources.
      </p>
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
