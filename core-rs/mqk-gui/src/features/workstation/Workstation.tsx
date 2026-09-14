// GUI-LAYOUT-02: dockable Tier-2 workstation surface.
//
// Wraps dockview-react. Every dockview panel id is a ScreenKey; content is
// still rendered through SCREEN_REGISTRY[key].render(ctx) — dockview owns
// only placement (dock/resize/tab/hide), never truth or rendering logic.
//
// Persistence is UI-only (see layoutModel.ts): a corrupted or foreign
// localStorage value, or one referencing a removed panel id, falls back to a
// fresh single-panel default rather than crashing or partially applying.

import {
  createContext,
  forwardRef,
  useCallback,
  useContext,
  useEffect,
  useImperativeHandle,
  useRef,
} from "react";
import {
  DockviewReact,
  themeAbyss,
  type DockviewApi,
  type DockviewReadyEvent,
  type IDockviewPanelProps,
} from "dockview-react";
import "dockview-react/dist/styles/dockview.css";
import { SCREEN_REGISTRY, type ScreenKey, type ScreenRenderContext } from "../screens/screenRegistry";
import { WorkspaceFrame } from "../../components/layout/WorkspaceFrame";
import { ScreenErrorBoundary } from "../../components/common/ScreenErrorBoundary";
import { getPanelMetadata, isKnownPanelId } from "./panelRegistry";
import {
  allDockviewPanelIdsKnown,
  extractDockviewPanelIds,
  parsePersistedLayout,
  sameIdSet,
  sanitizeLayoutSummary,
  serializeLayout,
} from "./layoutModel";

interface PanelHostParams {
  panelId: ScreenKey;
}

const ScreenRenderCtx = createContext<ScreenRenderContext | null>(null);

function PanelHost({ params }: IDockviewPanelProps<PanelHostParams>) {
  const ctx = useContext(ScreenRenderCtx);
  const panelId = params.panelId;
  const screen = isKnownPanelId(panelId) ? SCREEN_REGISTRY[panelId] : null;

  if (!ctx || !screen) {
    // Fail closed: never fabricate content for a panel id whose definition
    // is unavailable (e.g. removed from SCREEN_REGISTRY since this layout
    // was saved). filterKnownPanelIds should have already dropped these
    // before dockview ever sees them; this is the last-resort guard.
    return <div className="workstation-panel-unavailable">Panel unavailable.</div>;
  }

  return (
    <WorkspaceFrame
      title={screen.title}
      description={screen.description}
      panelKey={panelId}
      authority={ctx.model.panelSources[panelId]}
    >
      <ScreenErrorBoundary key={panelId} screenKey={panelId}>
        {screen.render(ctx)}
      </ScreenErrorBoundary>
    </WorkspaceFrame>
  );
}

const DOCKVIEW_COMPONENTS = { panelHost: PanelHost };

export interface WorkstationHandle {
  /** Focuses the panel if already open; otherwise opens it. Never creates a second instance of an already-open panel (this is what keeps an operator-singleton panel from duplicating). */
  openPanel: (id: ScreenKey) => void;
  resetLayout: () => void;
}

export interface WorkstationProps {
  /** Unique per desk-role/window persistence key, e.g. "mqd.workstation.layout.control". */
  storageKey: string;
  initialPanelId: ScreenKey;
  ctx: ScreenRenderContext;
  onActivePanelChange?: (id: ScreenKey | null) => void;
}

function buildDefaultLayout(api: DockviewApi, initialPanelId: ScreenKey) {
  api.clear();
  const meta = getPanelMetadata(initialPanelId);
  api.addPanel({
    id: initialPanelId,
    component: "panelHost",
    title: meta?.title ?? initialPanelId,
    params: { panelId: initialPanelId } satisfies PanelHostParams,
  });
}

function persistLayout(api: DockviewApi, storageKey: string) {
  const panelIds = api.panels.map((p) => p.id).filter(isKnownPanelId);
  const activeId = api.activePanel?.id;
  const activePanelId = activeId && isKnownPanelId(activeId) ? activeId : null;
  const doc = serializeLayout(panelIds, activePanelId, api.toJSON());
  try {
    window.localStorage.setItem(storageKey, JSON.stringify(doc));
  } catch {
    // Best-effort persistence only (e.g. storage quota, private browsing) —
    // never block the workstation on a failed write.
  }
}

/** Attempts to restore a persisted layout; returns true only if fully restored and every resulting panel id is known. Any disagreement/corruption/exception falls through to false so the caller rebuilds the default. */
function tryRestoreLayout(api: DockviewApi, storageKey: string): boolean {
  const persisted = parsePersistedLayout(window.localStorage.getItem(storageKey));
  if (!persisted) return false;

  const summary = sanitizeLayoutSummary(persisted);
  if (summary.panelIds.length === 0) return false;

  const dockviewIds = extractDockviewPanelIds(persisted.dockviewLayout);
  if (dockviewIds === null) return false;
  if (!allDockviewPanelIdsKnown(dockviewIds)) return false;
  if (!sameIdSet(summary.panelIds, dockviewIds)) return false;

  try {
    api.fromJSON(persisted.dockviewLayout as Parameters<DockviewApi["fromJSON"]>[0]);
  } catch {
    return false;
  }

  if (!api.panels.every((p) => isKnownPanelId(p.id))) {
    api.clear();
    return false;
  }
  return true;
}

export const Workstation = forwardRef<WorkstationHandle, WorkstationProps>(function Workstation(
  { storageKey, initialPanelId, ctx, onActivePanelChange },
  ref,
) {
  const apiRef = useRef<DockviewApi | null>(null);
  const disposablesRef = useRef<Array<{ dispose(): void }>>([]);
  const onActivePanelChangeRef = useRef(onActivePanelChange);
  onActivePanelChangeRef.current = onActivePanelChange;

  const handleReady = useCallback(
    (event: DockviewReadyEvent) => {
      const api = event.api;
      apiRef.current = api;

      const restored = tryRestoreLayout(api, storageKey);
      if (!restored) {
        buildDefaultLayout(api, initialPanelId);
      }

      const layoutSub = api.onDidLayoutChange(() => persistLayout(api, storageKey));
      const activeSub = api.onDidActivePanelChange((e) => {
        const id = e.panel?.id;
        onActivePanelChangeRef.current?.(id && isKnownPanelId(id) ? id : null);
      });
      disposablesRef.current = [layoutSub, activeSub];

      const activeId = api.activePanel?.id;
      onActivePanelChangeRef.current?.(activeId && isKnownPanelId(activeId) ? activeId : null);
    },
    [storageKey, initialPanelId],
  );

  useEffect(
    () => () => {
      for (const d of disposablesRef.current) d.dispose();
      disposablesRef.current = [];
    },
    [],
  );

  useImperativeHandle(
    ref,
    () => ({
      openPanel: (id: ScreenKey) => {
        const api = apiRef.current;
        if (!api) return;
        const existing = api.getPanel(id);
        if (existing) {
          existing.api.setActive();
          return;
        }
        const meta = getPanelMetadata(id);
        api.addPanel({
          id,
          component: "panelHost",
          title: meta?.title ?? id,
          params: { panelId: id } satisfies PanelHostParams,
        });
      },
      resetLayout: () => {
        const api = apiRef.current;
        if (!api) return;
        try {
          window.localStorage.removeItem(storageKey);
        } catch {
          // ignore storage failures on reset — clearing the in-memory layout still succeeds below.
        }
        buildDefaultLayout(api, initialPanelId);
      },
    }),
    [storageKey, initialPanelId],
  );

  return (
    <div className="workstation-root">
      <ScreenRenderCtx.Provider value={ctx}>
        <DockviewReact
          className="dockview-theme-abyss workstation-dockview"
          theme={themeAbyss}
          components={DOCKVIEW_COMPONENTS}
          onReady={handleReady}
        />
      </ScreenRenderCtx.Provider>
    </div>
  );
});
