// core-rs/mqk-gui/src/features/system/http.ts
//
// HTTP plumbing layer: typed result wrappers and GET/POST fetch utilities.
// No business logic lives here — pure transport concerns only.

import { getDaemonUrl } from "../../config";
import { getDesktopOperatorToken, isDesktopShell } from "../../desktop/bootstrap";

export interface EndpointFetchResult<T> {
  ok: boolean;
  endpoint: string;
  data?: T;
  error?: string;
}

export interface EndpointPostResult<T> {
  ok: boolean;
  endpoint: string;
  status?: number;
  data?: T;
  error?: string;
}

export async function fetchJsonCandidate<T>(path: string): Promise<EndpointFetchResult<T>> {
  try {
    const url = new URL(path, getDaemonUrl()).toString();
    const response = await fetch(url, {
      method: "GET",
      headers: { Accept: "application/json" },
    });

    if (!response.ok) {
      return { ok: false, endpoint: path, error: `HTTP ${response.status}` };
    }

    return {
      ok: true,
      endpoint: path,
      data: (await response.json()) as T,
    };
  } catch (error) {
    return {
      ok: false,
      endpoint: path,
      error: error instanceof Error ? error.message : "unknown error",
    };
  }
}

export async function fetchJsonCandidates<T>(paths: string[]): Promise<EndpointFetchResult<T>> {
  for (const path of paths) {
    const result = await fetchJsonCandidate<T>(path);
    if (result.ok) return result;
  }
  return {
    ok: false,
    endpoint: paths[0] ?? "unknown",
    error: "all candidates failed",
  };
}

export async function tryFetchJson<T>(paths: string[]): Promise<T | null> {
  const result = await fetchJsonCandidates<T>(paths);
  return result.ok ? (result.data ?? null) : null;
}

export async function postJson<T>(
  paths: string[],
  body: Record<string, unknown>,
  options?: { privileged?: boolean },
): Promise<EndpointPostResult<T>> {
  let lastFailure: EndpointPostResult<T> = {
    ok: false,
    endpoint: paths[0] ?? "unknown",
    error: "all candidates failed",
  };

  const privileged = options?.privileged === true;
  const desktopOperatorToken = getDesktopOperatorToken();

  if (privileged && isDesktopShell() && !desktopOperatorToken) {
    return {
      ok: false,
      endpoint: paths[0] ?? "unknown",
      error: "desktop operator token missing",
    };
  }

  for (const path of paths) {
    try {
      const url = new URL(path, getDaemonUrl()).toString();
      const headers: Record<string, string> = {
        Accept: "application/json",
        "Content-Type": "application/json",
      };

      if (privileged && desktopOperatorToken) {
        headers.Authorization = `Bearer ${desktopOperatorToken}`;
      }

      const response = await fetch(url, {
        method: "POST",
        headers,
        body: JSON.stringify(body),
      });

      if (!response.ok) {
        lastFailure = {
          ok: false,
          endpoint: path,
          status: response.status,
          error: `HTTP ${response.status}`,
        };
        continue;
      }

      const contentType = response.headers.get("content-type") ?? "";
      const data = contentType.includes("application/json") ? ((await response.json()) as T) : undefined;

      return {
        ok: true,
        endpoint: path,
        status: response.status,
        data,
      };
    } catch (error) {
      lastFailure = {
        ok: false,
        endpoint: path,
        error: error instanceof Error ? error.message : "unknown error",
      };
    }
  }

  return lastFailure;
}
