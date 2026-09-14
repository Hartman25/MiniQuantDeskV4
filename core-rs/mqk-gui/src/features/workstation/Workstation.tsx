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
  useMemo,
  useRef,
  useState,
} from "react";
import {
  DockviewReact,
  themeAbyss,
  type DockviewApi,
  type DockviewReadyEvent,
  type IDockviewPanelProps,
} from "dockview-react";
import "dockview-react/dist/styles/dockview.css";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { SCREEN_REGISTRY, type ScreenKey, type ScreenRenderContext } from "../screens/screenRegistry";
import { WorkspaceFrame } from "../../components/layout/WorkspaceFrame";
import { ScreenErrorBoundary } from "../../components/common/ScreenErrorBoundary";
import { useWorkspaceContext, WorkspaceScopeProvider } from "../workspace/WorkspaceContext.tsx";
import { initialPanelLinkState, pinPanel, resolvePanelIdentity, unpinPanel, type PanelLinkState } from "../workspace/workspaceModel.ts";
import { getPanelMetadata, isKnownPanelId } from "./panelRegistry";
import { detachPanel, REATTACH_EVENT } from "./detachedWindow";
import { APPLY_PRESET_EVENT, pendingPresetStorageKey, readPendingPresetPanelIds } from "./presets";
import type { DeskRole } from "../../app/shellTypes";
import {
  allDockviewPanelIdsKnown,
  extractDockviewPanelIds,
  parsePersistedLayout,
  sameIdSet,
  sanitizeLayoutSummary,
  serializeLayout,
  type PersistedWorkstationLayoutV1,
} from "./layoutModel";

interface PanelHostParams {
  panelId: ScreenKey;
}

const ScreenRenderCtx = createContext<ScreenRenderContext | null>(null);

interface WorkstationActions {
  detach: (id: ScreenKey, pinState: PanelLinkState) => void;
}

const WorkstationActionsCtx = createContext<WorkstationActions | null>(null);

function PanelHost({ params }: IDockviewPanelProps<PanelHostParams>) {
  const ctx = useContext(ScreenRenderCtx);
  const actions = useContext(WorkstationActionsCtx);
  const workspace = useWorkspaceContext();
  const [linkState, setLinkState] = useState<PanelLinkState>(initialPanelLinkState);
  const panelId = params.panelId;
  const screen = isKnownPanelId(panelId) ? SCREEN_REGISTRY[panelId] : null;
  const meta = isKnownPanelId(panelId) ? getPanelMetadata(panelId) : null;

  if (!ctx || !screen) {
    // Fail closed: never fabricate content for a panel id whose definition
    // is unavailable (e.g. removed from SCREEN_REGISTRY since this layout
    // was saved). filterKnownPanelIds should have already dropped these
    // before dockview ever sees them; this is the last-resort guard.
    return <div className="workstation-panel-unavailable">Panel unavailable.</div>;
  }

  // GUI-LAYOUT-05: only a contextAware panel (marketData, backtests — see
  // panelRegistry.ts) participates in linked/pinned identity at all. Every
  // other panel is rendered under the ambient global context unchanged.
  const contextAware = meta?.contextAware ?? false;
  const resolved = contextAware ? resolvePanelIdentity(workspace.linked, linkState) : workspace.linked;
  const togglePin = () => setLinkState((prev) => (prev.pinned ? unpinPanel() : pinPanel(resolved)));

  const body = (
    <ScreenErrorBoundary key={panelId} screenKey={panelId}>
      {screen.render(ctx)}
    </ScreenErrorBoundary>
  );

  return (
    <WorkspaceFrame
      title={screen.title}
      description={screen.description}
      panelKey={panelId}
      authority={ctx.model.panelSources[panelId]}
      onDetach={meta?.detachable && actions ? () => actions.detach(panelId, linkState) : undefined}
      pinState={contextAware ? { pinned: linkState.pinned, onToggle: togglePin } : undefined}
    >
      {contextAware ? (
        <WorkspaceScopeProvider value={{ linked: resolved, setLinked: workspace.setLinked, resetLinked: workspace.resetLinked }}>
          {body}
        </WorkspaceScopeProvider>
      ) : (
        body
      )}
    </WorkspaceFrame>
  );
}

const DOCKVIEW_COMPONENTS = { panelHost: PanelHost };

export interface WorkstationHandle {
  /** Focuses the panel if already open; otherwise opens it. Never creates a second instance of an already-open panel (this is what keeps an operator-singleton panel from duplicating). */
  openPanel: (id: ScreenKey) => void;
  resetLayout: () => void;
  /** GUI-LAYOUT-04: replaces the entire layout with exactly this panel set (a preset's starting arrangement). */
  applyPanelSet: (ids: readonly ScreenKey[]) => void;
  /** GUI-LAYOUT-04: snapshot of the current layout for "Save layout as...". Null before dockview has finished initializing. */
  getCurrentLayoutDoc: () => PersistedWorkstationLayoutV1 | null;
  /** GUI-LAYOUT-04: applies a previously-saved custom layout document, through the same validation as restoring a persisted layout. Returns false (leaving the current layout untouched) if the document is malformed or resolves to zero known panels. */
  applyLayoutDoc: (doc: PersistedWorkstationLayoutV1) => boolean;
}

function addOrFocusPanel(api: DockviewApi, id: ScreenKey) {
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
}

function currentWindowLabelOrDefault(): string {
  try {
    return getCurrentWebviewWindow().label;
  } catch {
    return "control";
  }
}

export interface WorkstationProps {
  /** Unique per desk-role/window persistence key, e.g. "mqd.workstation.layout.control". */
  storageKey: string;
  /** This window's desk role — used to look up a pending-preset seed and to scope the live apply-preset event. */
  role: DeskRole;
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

/** Replaces the whole layout with exactly these panels, in order. Used by presets and by the pending-preset seed. */
function buildPanelSet(api: DockviewApi, ids: readonly ScreenKey[]) {
  api.clear();
  for (const id of ids) addOrFocusPanel(api, id);
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

/** Attempts to apply a layout document (persisted-auto or user-saved custom); returns true only if fully applied and every resulting panel id is known. Any disagreement/corruption/exception falls through to false and leaves `api` untouched (never partially applied) so the caller can fall back to a known-good layout. */
function tryApplyLayoutDocument(api: DockviewApi, persisted: PersistedWorkstationLayoutV1): boolean {
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

/** Attempts to restore this window's own auto-persisted layout from storage. */
function tryRestoreLayout(api: DockviewApi, storageKey: string): boolean {
  const persisted = parsePersistedLayout(window.localStorage.getItem(storageKey));
  return persisted !== null && tryApplyLayoutDocument(api, persisted);
}

/** Consumes (reads-then-clears) a one-shot pending-preset seed written by the control window for a not-yet-open target window. Returns true only if a valid seed was found and applied. */
function tryApplyPendingPreset(api: DockviewApi, role: DeskRole): boolean {
  const key = pendingPresetStorageKey(role);
  const raw = window.localStorage.getItem(key);
  if (!raw) return false;
  try {
    window.localStorage.removeItem(key);
  } catch {
    // best-effort cleanup only
  }
  const panelIds = readPendingPresetPanelIds(raw);
  if (!panelIds) return false;
  buildPanelSet(api, panelIds);
  return true;
}

export const Workstation = forwardRef<WorkstationHandle, WorkstationProps>(function Workstation(
  { storageKey, role, initialPanelId, ctx, onActivePanelChange },
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

      // A pending preset seed (written by the control window for a window it
      // just created) always wins over the window's own auto-persisted
      // layout — it is the explicit reason this window was opened.
      const appliedPreset = tryApplyPendingPreset(api, role);
      if (!appliedPreset) {
        const restored = tryRestoreLayout(api, storageKey);
        if (!restored) {
          buildDefaultLayout(api, initialPanelId);
        }
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
    [storageKey, role, initialPanelId],
  );

  useEffect(
    () => () => {
      for (const d of disposablesRef.current) d.dispose();
      disposablesRef.current = [];
    },
    [],
  );

  // GUI-LAYOUT-04: live apply-preset for an already-open window (the
  // pending-preset seed above only covers a window that didn't exist yet).
  // Scoped to this window's own webview, same as the reattach listener.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    (async () => {
      try {
        const stop = await getCurrentWebviewWindow().listen<{ panelIds: string[] }>(APPLY_PRESET_EVENT, (event) => {
          const api = apiRef.current;
          const ids = event.payload?.panelIds;
          if (api && Array.isArray(ids)) {
            const known = ids.filter(isKnownPanelId);
            if (known.length > 0) buildPanelSet(api, known);
          }
        });
        if (cancelled) stop();
        else unlisten = stop;
      } catch {
        // Not in a Tauri context — no cross-window preset application is possible.
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // GUI-LAYOUT-03: listens for a detached window asking to be reattached.
  // Scoped to THIS window's own webview (getCurrentWebviewWindow().listen),
  // never a global broadcast — only the window that owns the target
  // panel-window's `opener` param ever receives its reattach request, so a
  // panel can never be re-added into more than one window from a single
  // reattach action. No-ops outside Tauri (browser-only preview).
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    (async () => {
      try {
        const stop = await getCurrentWebviewWindow().listen<{ panelId: string }>(REATTACH_EVENT, (event) => {
          const id = event.payload?.panelId;
          const api = apiRef.current;
          if (api && id && isKnownPanelId(id)) addOrFocusPanel(api, id);
        });
        if (cancelled) stop();
        else unlisten = stop;
      } catch {
        // Not in a Tauri context — no cross-window reattach is possible, nothing to listen for.
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const actions = useMemo<WorkstationActions>(
    () => ({
      detach: (id: ScreenKey, pinState: PanelLinkState) => {
        const api = apiRef.current;
        if (!api) return;
        const panel = api.getPanel(id);
        if (panel) api.removePanel(panel);
        const openerLabel = currentWindowLabelOrDefault();
        // GUI-LAYOUT-05: a pinned panel carries its frozen identity across
        // the detach boundary so the detached window shows the same
        // context, not a reset-to-global one.
        void detachPanel(id, openerLabel, pinState.pinned ? pinState.pinnedIdentity : null).then((result) => {
          // Detach genuinely failed (e.g. browser-only preview, no monitors)
          // — restore the panel locally rather than silently lose it.
          if (!result.ok) {
            const liveApi = apiRef.current;
            if (liveApi) addOrFocusPanel(liveApi, id);
          }
        });
      },
    }),
    [],
  );

  useImperativeHandle(
    ref,
    () => ({
      openPanel: (id: ScreenKey) => {
        const api = apiRef.current;
        if (api) addOrFocusPanel(api, id);
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
      applyPanelSet: (ids: readonly ScreenKey[]) => {
        const api = apiRef.current;
        if (api) buildPanelSet(api, ids);
      },
      getCurrentLayoutDoc: () => {
        const api = apiRef.current;
        if (!api) return null;
        const panelIds = api.panels.map((p) => p.id).filter(isKnownPanelId);
        const activeId = api.activePanel?.id;
        const activePanelId = activeId && isKnownPanelId(activeId) ? activeId : null;
        return serializeLayout(panelIds, activePanelId, api.toJSON());
      },
      applyLayoutDoc: (doc: PersistedWorkstationLayoutV1) => {
        const api = apiRef.current;
        return api !== null && tryApplyLayoutDocument(api, doc);
      },
    }),
    [storageKey, initialPanelId],
  );

  return (
    <div className="workstation-root">
      <ScreenRenderCtx.Provider value={ctx}>
        <WorkstationActionsCtx.Provider value={actions}>
          <DockviewReact
            className="dockview-theme-abyss workstation-dockview"
            theme={themeAbyss}
            components={DOCKVIEW_COMPONENTS}
            onReady={handleReady}
          />
        </WorkstationActionsCtx.Provider>
      </ScreenRenderCtx.Provider>
    </div>
  );
});
