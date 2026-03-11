import shieldLogo from "../../../../../assets/logo/miniquantdesk_logo_shield_terminal_transparent.png";
import { SCREEN_REGISTRY, type ScreenKey } from "../../features/screens/screenRegistry";

export function LeftCommandRail({
  activeScreen,
  onSelect,
}: {
  activeScreen: ScreenKey;
  onSelect: (screen: ScreenKey) => void;
}) {
  const primary: ScreenKey[] = ["dashboard", "execution", "risk", "portfolio", "reconcile", "ops", "runtime"];
  const secondary: ScreenKey[] = [
    "metrics",
    "transport",
    "topology",
    "alerts",
    "incidents",
    "operatorTimeline",
    "session",
    "config",
    "marketData",
    "strategy",
    "audit",
    "artifacts",
    "settings",
  ];

  const renderButton = (screen: ScreenKey) => (
    <button
      key={screen}
      type="button"
      className={`rail-nav-button ${activeScreen === screen ? "is-active" : ""}`}
      onClick={() => onSelect(screen)}
      title={SCREEN_REGISTRY[screen].title}
    >
      <span className="rail-nav-label">{SCREEN_REGISTRY[screen].title}</span>
      <small className="rail-nav-key">{screen}</small>
    </button>
  );

  return (
    <aside className="left-rail">
      <div className="brand-block panel panel-compact">
        <div style={{ display: "flex", justifyContent: "center", width: "100%" }}>
          <img
            src={shieldLogo}
            alt="MiniQuantDesk shield logo"
          />
        </div>

        <div className="eyebrow">MiniQuantDesk</div>
        <h1 className="brand-title">Operator Console</h1>
        <p className="brand-subtitle">Institution-grade trading control</p>
      </div>

      <div className="rail-section">
        <div className="rail-section-title">Primary</div>
        <div className="rail-nav-list">{primary.map(renderButton)}</div>
      </div>

      <div className="rail-section">
        <div className="rail-section-title">Secondary</div>
        <div className="rail-nav-list">{secondary.map(renderButton)}</div>
      </div>
    </aside>
  );
}