import type { ScreenKey } from "../../features/screens/screenRegistry";

interface LeftCommandRailProps {
  activeScreen: ScreenKey;
  onSelect: (screen: ScreenKey) => void;
}

const ITEMS: Array<{ key: ScreenKey; label: string; subtitle: string }> = [
  { key: "dashboard", label: "Dashboard", subtitle: "System posture" },
  { key: "metrics", label: "Metrics", subtitle: "Time-series health" },
  { key: "topology", label: "Topology", subtitle: "Service dependency map" },
  { key: "runtime", label: "Runtime", subtitle: "Leadership + restart" },
  { key: "marketData", label: "Market Data", subtitle: "Feed quality" },
  { key: "transport", label: "Transport", subtitle: "Outbox / inbox monitor" },
  { key: "execution", label: "Execution", subtitle: "OMS + trace + replay" },
  { key: "incidents", label: "Incidents", subtitle: "Case workspace" },
  { key: "alerts", label: "Alerts", subtitle: "Triage board" },
  { key: "operatorTimeline", label: "Operator Timeline", subtitle: "Human + system chronology" },
  { key: "risk", label: "Risk", subtitle: "Exposure and halts" },
  { key: "portfolio", label: "Portfolio", subtitle: "Positions and fills" },
  { key: "reconcile", label: "Reconcile", subtitle: "Broker agreement" },
  { key: "strategy", label: "Strategy", subtitle: "Engine supervision" },
  { key: "audit", label: "Logs / Audit", subtitle: "Forensics and evidence" },
  { key: "artifacts", label: "Artifacts", subtitle: "Bundles + exports" },
  { key: "session", label: "Session", subtitle: "Market-state guardrails" },
  { key: "config", label: "Config", subtitle: "Fingerprint visibility" },
  { key: "ops", label: "Operator Actions", subtitle: "Guarded controls" },
  { key: "settings", label: "Settings / Ops", subtitle: "Metadata and daemon" },
];

export function LeftCommandRail({ activeScreen, onSelect }: LeftCommandRailProps) {
  return (
    <aside className="left-rail">
      <div className="brand-card">
        <div className="eyebrow">MiniQuantDesk</div>
        <h1>Operator Terminal</h1>
        <p>GUI is a cockpit above the daemon, not an execution bypass.</p>
      </div>

      <div className="left-rail-scroll">
        <nav className="nav-stack" aria-label="Primary navigation">
          {ITEMS.map((item) => (
            <button
              key={item.key}
              type="button"
              className={`nav-card ${item.key === activeScreen ? "is-active" : ""}`}
              onClick={() => onSelect(item.key)}
            >
              <span className="nav-label">{item.label}</span>
              <span className="nav-subtitle">{item.subtitle}</span>
            </button>
          ))}
        </nav>
      </div>

      <div className="rail-footnote">
        <div className="eyebrow">Desk Split</div>
        <p>1. Control + health</p>
        <p>2. Execution + OMS</p>
        <p>3. Risk + reconcile + audit</p>
      </div>
    </aside>
  );
}
