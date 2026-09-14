import shieldLogo from "../../../../../assets/logo/veritas_ledger_shield.png";
import { SCREEN_REGISTRY, type ScreenKey } from "../../features/screens/screenRegistry";
import { LEFT_RAIL_PRIMARY, LEFT_RAIL_SECONDARY } from "./leftRailNav";
import { RAIL_NAV_ICONS } from "./railNavIcons";

export { LEFT_RAIL_PRIMARY, LEFT_RAIL_SECONDARY };

export function LeftCommandRail({
  activeScreen,
  onSelect,
  collapsed = false,
  onToggleCollapsed,
}: {
  activeScreen: ScreenKey;
  onSelect: (screen: ScreenKey) => void;
  collapsed?: boolean;
  onToggleCollapsed?: () => void;
}) {
  const primary = LEFT_RAIL_PRIMARY;
  const secondary = LEFT_RAIL_SECONDARY;

  const renderButton = (screen: ScreenKey) => {
    const Icon = RAIL_NAV_ICONS[screen];
    return (
      <button
        key={screen}
        type="button"
        className={`rail-nav-button ${activeScreen === screen ? "is-active" : ""}`}
        onClick={() => onSelect(screen)}
        title={SCREEN_REGISTRY[screen].title}
        aria-label={SCREEN_REGISTRY[screen].title}
      >
        {collapsed ? (
          <Icon className="rail-nav-icon" aria-hidden="true" size={18} strokeWidth={2} />
        ) : (
          <>
            <span className="rail-nav-label-group">
              <Icon className="rail-nav-icon" aria-hidden="true" size={16} strokeWidth={2} />
              <span className="rail-nav-label">{SCREEN_REGISTRY[screen].title}</span>
            </span>
            <small className="rail-nav-key">{screen}</small>
          </>
        )}
      </button>
    );
  };

  return (
    <aside className="left-rail">
      <div className="brand-block panel panel-compact">
        <img
          src={shieldLogo}
          alt="Veritas Ledger shield"
        />
        {collapsed ? null : (
          <div>
            <div className="eyebrow">Veritas Ledger</div>
            <h1 className="brand-title">Operator Console</h1>
            <p className="brand-subtitle">Institution-grade trading control</p>
          </div>
        )}
      </div>

      {onToggleCollapsed ? (
        <button
          type="button"
          className="rail-collapse-toggle"
          onClick={onToggleCollapsed}
          aria-pressed={collapsed}
          title={collapsed ? "Expand navigation" : "Collapse navigation"}
        >
          {collapsed ? "»" : "« Collapse"}
        </button>
      ) : null}

      <div className="rail-nav-scroll">
        <div className="rail-section">
          {collapsed ? null : <div className="rail-section-title">Primary</div>}
          <div className="rail-nav-list">{primary.map(renderButton)}</div>
        </div>

        <div className="rail-section">
          {collapsed ? null : <div className="rail-section-title">Secondary</div>}
          <div className="rail-nav-list">{secondary.map(renderButton)}</div>
        </div>
      </div>
    </aside>
  );
}