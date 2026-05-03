import shieldLogo from "../../../../../assets/logo/veritas_ledger_shield.png";
import { SCREEN_REGISTRY, type ScreenKey } from "../../features/screens/screenRegistry";
import { LEFT_RAIL_PRIMARY, LEFT_RAIL_SECONDARY } from "./leftRailNav";

export { LEFT_RAIL_PRIMARY, LEFT_RAIL_SECONDARY };

export function LeftCommandRail({
  activeScreen,
  onSelect,
}: {
  activeScreen: ScreenKey;
  onSelect: (screen: ScreenKey) => void;
}) {
  const primary = LEFT_RAIL_PRIMARY;
  const secondary = LEFT_RAIL_SECONDARY;

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
        <img
          src={shieldLogo}
          alt="Veritas Ledger shield"
        />
        <div>
          <div className="eyebrow">Veritas Ledger</div>
          <h1 className="brand-title">Operator Console</h1>
          <p className="brand-subtitle">Institution-grade trading control</p>
        </div>
      </div>

      <div className="rail-nav-scroll">
        <div className="rail-section">
          <div className="rail-section-title">Primary</div>
          <div className="rail-nav-list">{primary.map(renderButton)}</div>
        </div>

        <div className="rail-section">
          <div className="rail-section-title">Secondary</div>
          <div className="rail-nav-list">{secondary.map(renderButton)}</div>
        </div>
      </div>
    </aside>
  );
}