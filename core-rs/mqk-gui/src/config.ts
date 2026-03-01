// GUI-2: Daemon URL configuration
// Priority order:
// 1) localStorage override (operator-selected)
// 2) Vite env var (VITE_MQK_DAEMON_URL)
// 3) default localhost

const LS_KEY = "mqk.daemon_url";

export function defaultDaemonUrl(): string {
  return "http://127.0.0.1:8899";
}

export function getDaemonUrl(): string {
  const saved = getSavedDaemonUrl();
  if (saved) return saved;

  const env = (import.meta as any)?.env?.VITE_MQK_DAEMON_URL as string | undefined;
  if (env && typeof env === "string") {
    const trimmed = env.trim();
    if (trimmed) return normalizeUrl(trimmed);
  }

  return defaultDaemonUrl();
}

export function getSavedDaemonUrl(): string | null {
  try {
    const v = localStorage.getItem(LS_KEY);
    if (!v) return null;
    const trimmed = v.trim();
    if (!trimmed) return null;
    return normalizeUrl(trimmed);
  } catch {
    return null;
  }
}

export function setSavedDaemonUrl(url: string): { ok: boolean; error?: string; value?: string } {
  const normalized = normalizeUrl(url);
  const check = validateDaemonUrl(normalized);
  if (!check.ok) return check;

  try {
    localStorage.setItem(LS_KEY, normalized);
  } catch (e: any) {
    return { ok: false, error: `failed to persist: ${String(e?.message ?? e)}` };
  }

  return { ok: true, value: normalized };
}

export function clearSavedDaemonUrl(): void {
  try {
    localStorage.removeItem(LS_KEY);
  } catch {
    // ignore
  }
}

export function validateDaemonUrl(url: string): { ok: boolean; error?: string } {
  try {
    const u = new URL(url);
    if (u.protocol !== "http:" && u.protocol !== "https:") {
      return { ok: false, error: "must be http or https" };
    }
    if (!u.hostname) return { ok: false, error: "missing host" };
    // allow default port (80/443) or explicit port
    return { ok: true };
  } catch {
    return { ok: false, error: "invalid URL" };
  }
}

export function normalizeUrl(url: string): string {
  const s = (url ?? "").trim();
  if (!s) return s;

  // Allow operator to paste "127.0.0.1:8899"
  if (!s.startsWith("http://") && !s.startsWith("https://")) {
    return `http://${s}`;
  }

  return s;
}