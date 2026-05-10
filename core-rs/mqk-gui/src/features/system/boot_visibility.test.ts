// boot_visibility.test.ts
//
// GUI-OPERATOR-BOOT-VISIBILITY-01 proof tests.
//
// G01: Daemon unreachable (network error) — dashboard renders unavailable state, not blank.
// G02: HTTP 401/403 — model surfaces auth-required message; no crash; no false green.
// G03: HTTP 500 — model is disconnected; no crash; unavailable state rendered.
// G04: Null/partial status response — model uses safe fallback; no crash.
// G05: Connected model with full status — dashboard renders data (not truth-blocked).
// G06: Disconnected model — no token, key, or DB-URL strings rendered.

import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { fetchOperatorModel } from "./api.ts";
import { DashboardScreen } from "../dashboard/DashboardScreen.tsx";
import { DEFAULT_STATUS, DEFAULT_PREFLIGHT } from "./types.ts";
import { panelTruthRenderState } from "./truthRendering.ts";

// ---------------------------------------------------------------------------
// Shared fetch mocks
// ---------------------------------------------------------------------------

function networkErrorFetch(): typeof fetch {
  return (async () => {
    throw new Error("Failed to fetch");
  }) as unknown as typeof fetch;
}

function httpStatusFetch(statusForStatus: number): typeof fetch {
  return (async (input: string | URL | Request) => {
    const raw =
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.toString()
          : input.url;
    const path = new URL(raw).pathname;
    if (path === "/api/v1/system/status") {
      return {
        ok: false,
        status: statusForStatus,
        async json() {
          return { error: "server error" };
        },
      } as Response;
    }
    return {
      ok: false,
      status: 404,
      async json() {
        return {};
      },
    } as Response;
  }) as typeof fetch;
}

// ---------------------------------------------------------------------------
// G01: Daemon unreachable — renders unavailable, not blank
// ---------------------------------------------------------------------------

test("G01: daemon unreachable — dashboard renders unavailable state, not blank HTML", async () => {
  const orig = globalThis.fetch;
  globalThis.fetch = networkErrorFetch();
  try {
    const model = await fetchOperatorModel();
    assert.equal(model.connected, false, "G01: model must be disconnected");
    assert.equal(
      model.dataSource.state,
      "disconnected",
      "G01: dataSource must be disconnected",
    );

    const html = renderToStaticMarkup(React.createElement(DashboardScreen, { model }));
    assert.ok(html.length > 0, "G01: dashboard must render non-empty HTML");
    assert.match(html, /Unavailable/i, "G01: must render unavailable state, not blank");
    // Must NOT render live runtime data as if connected
    assert.doesNotMatch(
      html,
      /truth-state-notice.*Running|Running.*truth-state-notice/s,
      "G01: must not show running status through truth notice",
    );
  } finally {
    globalThis.fetch = orig;
  }
});

// ---------------------------------------------------------------------------
// G02: HTTP 401 — auth-specific message surfaced; no crash; no false green
// ---------------------------------------------------------------------------

test("G02: HTTP 401 — model surfaces auth-required message; no crash; no false-green render", async () => {
  const orig = globalThis.fetch;
  globalThis.fetch = httpStatusFetch(401);
  try {
    const model = await fetchOperatorModel();
    assert.equal(model.connected, false, "G02: 401 must result in disconnected model");
    assert.ok(
      model.dataSource.message?.toLowerCase().includes("auth"),
      `G02: dataSource.message must mention auth on 401; got: "${model.dataSource.message}"`,
    );
    const html = renderToStaticMarkup(React.createElement(DashboardScreen, { model }));
    assert.ok(html.length > 0, "G02: dashboard must render without crash on 401");
    assert.match(html, /Unavailable/i, "G02: must render unavailable, not false-green");
  } finally {
    globalThis.fetch = orig;
  }
});

test("G02b: HTTP 403 — model surfaces auth-required message", async () => {
  const orig = globalThis.fetch;
  globalThis.fetch = httpStatusFetch(403);
  try {
    const model = await fetchOperatorModel();
    assert.equal(model.connected, false, "G02b: 403 must result in disconnected model");
    assert.ok(
      model.dataSource.message?.toLowerCase().includes("auth"),
      `G02b: dataSource.message must mention auth on 403; got: "${model.dataSource.message}"`,
    );
  } finally {
    globalThis.fetch = orig;
  }
});

// ---------------------------------------------------------------------------
// G03: HTTP 500 — model disconnected; no crash; unavailable rendered
// ---------------------------------------------------------------------------

test("G03: HTTP 500 from status — model is disconnected; no crash; unavailable rendered", async () => {
  const orig = globalThis.fetch;
  globalThis.fetch = httpStatusFetch(500);
  try {
    const model = await fetchOperatorModel();
    assert.equal(model.connected, false, "G03: 500 must result in disconnected model");
    // 500 must NOT surface auth message (it is a server error, not auth rejection)
    assert.ok(
      !model.dataSource.message?.toLowerCase().includes("auth"),
      "G03: 500 must not produce auth message",
    );
    const html = renderToStaticMarkup(React.createElement(DashboardScreen, { model }));
    assert.ok(html.length > 0, "G03: dashboard must render non-empty HTML on 5xx");
    assert.match(html, /Unavailable/i, "G03: must render unavailable, not blank");
  } finally {
    globalThis.fetch = orig;
  }
});

// ---------------------------------------------------------------------------
// G04: Null/partial status body — safe fallback; no crash
// ---------------------------------------------------------------------------

test("G04: null status response body — model falls back safely; dashboard renders without crash", async () => {
  const orig = globalThis.fetch;
  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw =
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.toString()
          : input.url;
    const path = new URL(raw).pathname;
    if (path === "/api/v1/system/status") {
      return {
        ok: true,
        status: 200,
        async json() {
          return null;
        },
      } as Response;
    }
    return {
      ok: false,
      status: 404,
      async json() {
        return {};
      },
    } as Response;
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();
    // Null body must fall through to unavailableStatus — no crash
    const html = renderToStaticMarkup(React.createElement(DashboardScreen, { model }));
    assert.ok(html.length > 0, "G04: dashboard must render without crash on null status body");
  } finally {
    globalThis.fetch = orig;
  }
});

test("G04b: partial status body missing required fields — model uses safe fallback; no crash", async () => {
  const orig = globalThis.fetch;
  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw =
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.toString()
          : input.url;
    const path = new URL(raw).pathname;
    if (path === "/api/v1/system/status") {
      // Partial body: only has runtime_status, missing daemon_reachable + other fields
      return {
        ok: true,
        status: 200,
        async json() {
          return { runtime_status: "running" };
        },
      } as Response;
    }
    return {
      ok: false,
      status: 404,
      async json() {
        return {};
      },
    } as Response;
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();
    const html = renderToStaticMarkup(React.createElement(DashboardScreen, { model }));
    assert.ok(html.length > 0, "G04b: dashboard must render without crash on partial status body");
  } finally {
    globalThis.fetch = orig;
  }
});

// ---------------------------------------------------------------------------
// G05: Connected model — dashboard truth state is null (not blocked)
// ---------------------------------------------------------------------------

test("G05: connected model with full status and risk truth — dashboard truth state is null", async () => {
  const orig = globalThis.fetch;
  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw =
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.toString()
          : input.url;
    const path = new URL(raw).pathname;
    switch (path) {
      case "/api/v1/system/status":
        return {
          ok: true,
          status: 200,
          async json() {
            return {
              ...DEFAULT_STATUS,
              daemon_reachable: true,
              runtime_status: "running",
              last_heartbeat: new Date().toISOString(),
            };
          },
        } as Response;
      case "/api/v1/system/preflight":
        return {
          ok: true,
          status: 200,
          async json() {
            return {
              ...DEFAULT_PREFLIGHT,
              daemon_reachable: true,
              runtime_idle: true,
              strategy_disarmed: true,
              execution_disarmed: true,
              live_routing_disabled: true,
              blockers: [],
              warnings: [],
            };
          },
        } as Response;
      case "/api/v1/risk/denials":
        return {
          ok: true,
          status: 200,
          async json() {
            return {
              truth_state: "active",
              snapshot_at_utc: new Date().toISOString(),
              denials: [],
            };
          },
        } as Response;
      default:
        return {
          ok: false,
          status: 404,
          async json() {
            return {};
          },
        } as Response;
    }
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();
    assert.equal(model.connected, true, "G05: model must be connected");
    // Dashboard must not be truth-blocked when risk/denials and status are real
    const truthState = panelTruthRenderState(model, "dashboard");
    assert.equal(truthState, null, "G05: dashboard truth state must be null (not blocked)");
    // Dashboard renders content, not just an unavailable notice
    const html = renderToStaticMarkup(React.createElement(DashboardScreen, { model }));
    assert.doesNotMatch(
      html,
      /truth-state-unavailable/,
      "G05: must not render unavailable class when connected",
    );
  } finally {
    globalThis.fetch = orig;
  }
});

// ---------------------------------------------------------------------------
// G06: Disconnected model — no secret-like strings rendered
// ---------------------------------------------------------------------------

test("G06: disconnected model — dashboard renders no token, key, or DB-URL strings", async () => {
  const orig = globalThis.fetch;
  globalThis.fetch = networkErrorFetch();
  try {
    const model = await fetchOperatorModel();
    const html = renderToStaticMarkup(React.createElement(DashboardScreen, { model }));
    assert.doesNotMatch(html, /Bearer\s+\S+/i, "G06: must not render bearer token");
    assert.doesNotMatch(html, /postgres:\/\//i, "G06: must not render DB URL");
    assert.doesNotMatch(html, /API[_\-]?KEY/i, "G06: must not render API key label");
    assert.doesNotMatch(html, /(?<!\w)SECRET(?!\w)/i, "G06: must not render SECRET string");
    assert.doesNotMatch(html, /sk-[a-zA-Z0-9]{10,}/i, "G06: must not render secret key pattern");
  } finally {
    globalThis.fetch = orig;
  }
});
