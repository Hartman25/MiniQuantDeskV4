import { ROLE_SCREENS, SCREEN_REGISTRY, type ScreenKey } from "../../features/screens/screenRegistry";

export function RoleCommandStrip({
  deskRole,
  activeScreen,
  onSelect,
}: {
  deskRole: "execution" | "oversight";
  activeScreen: ScreenKey;
  onSelect: (screen: ScreenKey) => void;
}) {
  const screens = ROLE_SCREENS[deskRole];
  const label = deskRole === "execution" ? "Execution" : "Oversight";

  return (
    <nav className="role-command-strip panel panel-compact" aria-label={`${label} screen navigation`}>
      <div className="role-strip-label eyebrow">{label}</div>
      <div className="role-strip-buttons">
        {screens.map((key) => (
          <button
            key={key}
            type="button"
            className={`role-strip-btn ${activeScreen === key ? "is-active" : ""}`}
            onClick={() => onSelect(key)}
            title={SCREEN_REGISTRY[key].description}
          >
            {SCREEN_REGISTRY[key].title}
          </button>
        ))}
      </div>
    </nav>
  );
}
