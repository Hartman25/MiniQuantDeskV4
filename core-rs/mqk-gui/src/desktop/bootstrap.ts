export interface DesktopBootstrapState {
  isDesktopShell: boolean;
  daemonUrl: string | null;
  operatorToken: string | null;
  productName: string | null;
}

const DEFAULT_BOOTSTRAP: DesktopBootstrapState = {
  isDesktopShell: false,
  daemonUrl: null,
  operatorToken: null,
  productName: null,
};

let cachedBootstrap: DesktopBootstrapState = { ...DEFAULT_BOOTSTRAP };
let initPromise: Promise<DesktopBootstrapState> | null = null;

function normalizeOptionalString(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return trimmed ? trimmed : null;
}

function coerceBootstrapPayload(value: unknown): DesktopBootstrapState {
  if (!value || typeof value !== "object") {
    return { ...DEFAULT_BOOTSTRAP };
  }

  const payload = value as Record<string, unknown>;
  return {
    isDesktopShell: payload.isDesktopShell === true,
    daemonUrl: normalizeOptionalString(payload.daemonUrl),
    operatorToken: normalizeOptionalString(payload.operatorToken),
    productName: normalizeOptionalString(payload.productName),
  };
}

export async function initDesktopBootstrap(): Promise<DesktopBootstrapState> {
  if (initPromise) return initPromise;

  initPromise = (async () => {
    try {
      const mod = await import("@tauri-apps/api/core");
      const invoke = mod.invoke as <T>(command: string) => Promise<T>;
      const payload = await invoke<unknown>("get_desktop_bootstrap");
      cachedBootstrap = coerceBootstrapPayload(payload);
      return cachedBootstrap;
    } catch {
      cachedBootstrap = { ...DEFAULT_BOOTSTRAP };
      return cachedBootstrap;
    }
  })();

  return initPromise;
}

export function getDesktopBootstrap(): DesktopBootstrapState {
  return cachedBootstrap;
}

export function getDesktopDaemonUrl(): string | null {
  return cachedBootstrap.daemonUrl;
}

export function getDesktopOperatorToken(): string | null {
  return cachedBootstrap.operatorToken;
}

export function isDesktopShell(): boolean {
  return cachedBootstrap.isDesktopShell;
}
