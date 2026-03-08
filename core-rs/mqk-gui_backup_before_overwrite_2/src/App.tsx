import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import "./App.css";
import {
  clearSavedDaemonUrl,
  defaultDaemonUrl,
  getDaemonUrl,
  getSavedDaemonUrl,
  normalizeUrl,
  setSavedDaemonUrl,
  validateDaemonUrl,
} from "./config";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type StatusSnapshot = {
  daemon_uptime_secs: number;
  active_run_id: string | null;
  state: string;
  notes?: string | null;
  integrity_armed: boolean;
};

type UiTab = "overview" | "trading" | "backtest" | "research" | "logs" | "config";

type LogLevel = "INFO" | "WARN" | "ERROR";

type LogEntry = {
  ts_local: string; // local time string for display
  source: "SSE" | "POLL";
  level: LogLevel;
  topic: "heartbeat" | "status" | "log" | "ui";
  message: string;
  raw?: string;
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function formatUptime(secs: number | null | undefined): string {
  if (secs === null || secs === undefined || Number.isNaN(secs)) return "—";
  const s = Math.max(0, Math.floor(secs));
  const hh = Math.floor(s / 3600);
  const mm = Math.floor((s % 3600) / 60);
  const ss = s % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(hh)}:${pad(mm)}:${pad(ss)}`;
}

function parseTaggedStatus(raw: string): StatusSnapshot | null {
  try {
    const obj = JSON.parse(raw) as any;
    // daemon emits BusMsg::Status(StatusSnapshot) as:
    // { "type": "status", "daemon_uptime_secs": ..., "state": ..., ... }
    if (obj && typeof obj === "object" && obj.daemon_uptime_secs !== undefined && obj.state !== undefined) {
      return obj as StatusSnapshot;
    }
    return null;
  } catch {
    return null;
  }
}

function levelFromText(s: string): LogLevel {
  const up = s.toUpperCase();
  if (up.includes("ERROR") || up.includes("PANIC") || up.includes("FATAL")) return "ERROR";
  if (up.includes("WARN")) return "WARN";
  return "INFO";
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

export default function App() {
  const [tab, setTab] = useState<UiTab>("overview");

  // GUI-2: daemon URL is configurable (env + localStorage override)
  const [daemonUrl, setDaemonUrl] = useState<string>(() => getDaemonUrl());

  // Status + connectivity
  const [statusOk, setStatusOk] = useState(false);
  const [sseOk, setSseOk] = useState(false);
  const [status, setStatus] = useState<StatusSnapshot | null>(null);

  // Logs
  const [logPaused, setLogPaused] = useState(false);
  const [logFilter, setLogFilter] = useState("");
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const logRef = useRef<HTMLDivElement | null>(null);

  // Config UI state
  const [configDraftUrl, setConfigDraftUrl] = useState<string>(() => daemonUrl);
  const [configMsg, setConfigMsg] = useState<string>("");
  const [configMsgLevel, setConfigMsgLevel] = useState<LogLevel>("INFO");

  const connected = statusOk && sseOk;

  const pushEntry = useCallback(
    (e: Omit<LogEntry, "ts_local">) => {
      if (logPaused) return;
      const ts_local = new Date().toLocaleTimeString();
      setEntries((prev) => [{ ...e, ts_local }, ...prev].slice(0, 2000));
    },
    [logPaused]
  );

  const pushUi = useCallback(
    (message: string, level: LogLevel = "INFO") => {
      pushEntry({ source: "SSE", level, topic: "ui", message });
    },
    [pushEntry]
  );

  // -------------------------------------------------------------------------
  // Status poll — keeps the UI sane even if SSE hiccups
  // -------------------------------------------------------------------------

  useEffect(() => {
    setStatusOk(false);

    const interval = setInterval(async () => {
      try {
        const res = await fetch(`${daemonUrl}/v1/status`);
        if (!res.ok) throw new Error("status failed");
        const json = (await res.json()) as StatusSnapshot;
        setStatus(json);
        setStatusOk(true);
      } catch {
        setStatusOk(false);
      }
    }, 1500);

    return () => clearInterval(interval);
  }, [daemonUrl]);

  // -------------------------------------------------------------------------
  // SSE stream — authoritative live updates
  // -------------------------------------------------------------------------

  useEffect(() => {
    let es: EventSource | null = null;

    const connect = () => {
      setSseOk(false);

      try {
        es = new EventSource(`${daemonUrl}/v1/stream`);
      } catch {
        setSseOk(false);
        return;
      }

      es.onopen = () => setSseOk(true);
      es.onerror = () => setSseOk(false);

      es.addEventListener("heartbeat", () => {
        pushEntry({ source: "SSE", level: "INFO", topic: "heartbeat", message: "heartbeat" });
      });

      es.addEventListener("status", (e) => {
        const raw = (e as MessageEvent).data as string;
        const snap = parseTaggedStatus(raw);
        if (snap) {
          setStatus(snap);
          setStatusOk(true);
        }
        pushEntry({
          source: "SSE",
          level: "INFO",
          topic: "status",
          message: snap ? `state=${snap.state} armed=${snap.integrity_armed} run=${snap.active_run_id ?? "—"}` : raw,
          raw,
        });
      });

      es.addEventListener("log", (e) => {
        const data = (e as MessageEvent).data as string;
        pushEntry({
          source: "SSE",
          level: levelFromText(data),
          topic: "log",
          message: data,
          raw: data,
        });
      });
    };

    connect();
    return () => es?.close();
  }, [daemonUrl, pushEntry]);

  // -------------------------------------------------------------------------
  // Auto-scroll log panel to top (newest entry is at top)
  // -------------------------------------------------------------------------

  useEffect(() => {
    if (!logRef.current) return;
    logRef.current.scrollTop = 0;
  }, [entries]);

  // -------------------------------------------------------------------------
  // Actions
  // -------------------------------------------------------------------------

  const apiPost = useCallback(
    async (path: string) => {
      const res = await fetch(`${daemonUrl}${path}`, { method: "POST" });
      if (!res.ok) {
        const body = await res.text().catch(() => "");
        throw new Error(`${path} failed: ${res.status} ${body}`);
      }
      return res.text().catch(() => "");
    },
    [daemonUrl]
  );

  const startRun = useCallback(async () => {
    try {
      await apiPost("/v1/run/start");
      pushUi("run/start requested");
    } catch (e: any) {
      pushUi(String(e?.message ?? e), "ERROR");
    }
  }, [apiPost, pushUi]);

  const stopRun = useCallback(async () => {
    try {
      await apiPost("/v1/run/stop");
      pushUi("run/stop requested");
    } catch (e: any) {
      pushUi(String(e?.message ?? e), "ERROR");
    }
  }, [apiPost, pushUi]);

  const haltRun = useCallback(async () => {
    try {
      await apiPost("/v1/run/halt");
      pushUi("run/halt requested");
    } catch (e: any) {
      pushUi(String(e?.message ?? e), "ERROR");
    }
  }, [apiPost, pushUi]);

  const armIntegrity = useCallback(async () => {
    try {
      await apiPost("/v1/integrity/arm");
      pushUi("integrity/arm requested");
    } catch (e: any) {
      pushUi(String(e?.message ?? e), "ERROR");
    }
  }, [apiPost, pushUi]);

  const disarmIntegrity = useCallback(async () => {
    try {
      await apiPost("/v1/integrity/disarm");
      pushUi("integrity/disarm requested");
    } catch (e: any) {
      pushUi(String(e?.message ?? e), "ERROR");
    }
  }, [apiPost, pushUi]);

  // -------------------------------------------------------------------------
  // Derived UI state
  // -------------------------------------------------------------------------

  const headerBadge = useMemo(() => {
    const text = connected ? "CONNECTED" : "DISCONNECTED";
    const cls = connected ? "badge ok" : "badge bad";
    return { text, cls };
  }, [connected]);

  const stateBadge = useMemo(() => {
    const st = status?.state ?? "UNKNOWN";
    const normalized = st.toUpperCase();
    if (normalized.includes("HALT")) return { text: st, cls: "badge bad" };
    if (normalized.includes("RUN")) return { text: st, cls: "badge ok" };
    return { text: st, cls: "badge warn" };
  }, [status?.state]);

  const armedBadge = useMemo(() => {
    const armed = Boolean(status?.integrity_armed);
    return armed ? { text: "ARMED", cls: "badge ok" } : { text: "DISARMED", cls: "badge warn" };
  }, [status?.integrity_armed]);

  const filteredEntries = useMemo(() => {
    const q = logFilter.trim().toLowerCase();
    if (!q) return entries;
    return entries.filter((e) => {
      const hay = `${e.ts_local} ${e.source} ${e.level} ${e.topic} ${e.message}`.toLowerCase();
      return hay.includes(q);
    });
  }, [entries, logFilter]);

  const recentAlerts = useMemo(() => {
    // last 8 errors/warns only
    const out: LogEntry[] = [];
    for (const e of entries) {
      if (e.level === "ERROR" || e.level === "WARN") out.push(e);
      if (out.length >= 8) break;
    }
    return out;
  }, [entries]);

  // -------------------------------------------------------------------------
  // Config tab handlers (GUI-2)
  // -------------------------------------------------------------------------

  const setCfgMessage = useCallback((msg: string, lvl: LogLevel) => {
    setConfigMsg(msg);
    setConfigMsgLevel(lvl);
  }, []);

  const applyDaemonUrl = useCallback(() => {
    const normalized = normalizeUrl(configDraftUrl);
    const check = validateDaemonUrl(normalized);
    if (!check.ok) {
      setCfgMessage(`Invalid daemon URL: ${check.error}`, "ERROR");
      return;
    }

    const persisted = setSavedDaemonUrl(normalized);
    if (!persisted.ok) {
      setCfgMessage(`Failed to save URL: ${persisted.error ?? "unknown error"}`, "ERROR");
      return;
    }

    setDaemonUrl(normalized);
    setCfgMessage(`Daemon URL applied: ${normalized}`, "INFO");
    pushUi(`daemon url set to ${normalized}`);
  }, [configDraftUrl, pushUi, setCfgMessage]);

  const resetDaemonUrl = useCallback(() => {
    clearSavedDaemonUrl();
    const next = getDaemonUrl(); // will fall back to env or default
    setDaemonUrl(next);
    setConfigDraftUrl(next);
    setCfgMessage(`Daemon URL reset to: ${next}`, "INFO");
    pushUi(`daemon url reset to ${next}`);
  }, [pushUi, setCfgMessage]);

  useEffect(() => {
    // Keep draft synced to active URL unless operator is currently editing
    setConfigDraftUrl(daemonUrl);
  }, [daemonUrl]);

  const savedOverride = useMemo(() => getSavedDaemonUrl(), [daemonUrl]);

  // -------------------------------------------------------------------------
  // Render
  // -------------------------------------------------------------------------

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-title">MiniQuantDesk</div>
          <div className="brand-subtitle">Operator Console</div>
        </div>

        <nav className="nav">
          <NavItem label="Overview" active={tab === "overview"} onClick={() => setTab("overview")} />
          <NavItem label="Trading" active={tab === "trading"} onClick={() => setTab("trading")} />
          <NavItem label="Backtest" active={tab === "backtest"} onClick={() => setTab("backtest")} />
          <NavItem label="Research" active={tab === "research"} onClick={() => setTab("research")} />
          <NavItem label="Logs" active={tab === "logs"} onClick={() => setTab("logs")} />
          <NavItem label="Config" active={tab === "config"} onClick={() => setTab("config")} />
        </nav>

        <div className="sidebar-footer">
          <div className="kv">
            <div className="k">Daemon</div>
            <div className="v mono">{daemonUrl}</div>
          </div>
          <div className="row">
            <span className={headerBadge.cls}>{headerBadge.text}</span>
            <span className={armedBadge.cls}>{armedBadge.text}</span>
          </div>
        </div>
      </aside>

      <main className="main">
        <header className="topbar">
          <div className="topbar-left">
            <div className="page-title">{tabTitle(tab)}</div>
            <div className="muted small">
              uptime <span className="mono">{formatUptime(status?.daemon_uptime_secs)}</span> · run{" "}
              <span className="mono">{status?.active_run_id ?? "—"}</span>
            </div>
          </div>
          <div className="topbar-right">
            <span className={stateBadge.cls}>{stateBadge.text}</span>
            <span className={sseOk ? "dot ok" : "dot bad"} title={sseOk ? "SSE ok" : "SSE down"} />
            <span className={statusOk ? "dot ok" : "dot bad"} title={statusOk ? "poll ok" : "poll down"} />
          </div>
        </header>

        <section className="content">
          {tab === "overview" && (
            <div className="grid two">
              <Card title="System Status" subtitle="Daemon snapshot (poll + SSE)">
                <table className="table">
                  <tbody>
                    <tr>
                      <td className="tdLabel">Connected</td>
                      <td className="tdValue">{connected ? "YES" : "NO"}</td>
                    </tr>
                    <tr>
                      <td className="tdLabel">State</td>
                      <td className="tdValue">{status?.state ?? "—"}</td>
                    </tr>
                    <tr>
                      <td className="tdLabel">Integrity</td>
                      <td className="tdValue">{status?.integrity_armed ? "ARMED" : "DISARMED"}</td>
                    </tr>
                    <tr>
                      <td className="tdLabel">Active Run</td>
                      <td className="tdValue mono">{status?.active_run_id ?? "—"}</td>
                    </tr>
                    <tr>
                      <td className="tdLabel">Notes</td>
                      <td className="tdValue">{status?.notes ?? "—"}</td>
                    </tr>
                  </tbody>
                </table>
              </Card>

              <Card title="Controls" subtitle="Run lifecycle + integrity gate">
                <div className="btnRow">
                  <button className="btn primary" onClick={startRun} disabled={!connected}>
                    Start Run
                  </button>
                  <button className="btn" onClick={stopRun} disabled={!connected}>
                    Stop Run
                  </button>
                  <button className="btn danger" onClick={haltRun} disabled={!connected}>
                    HALT
                  </button>
                </div>

                <div className="divider" />

                <div className="btnRow">
                  <button className="btn warn" onClick={armIntegrity} disabled={!connected}>
                    Arm Integrity
                  </button>
                  <button className="btn" onClick={disarmIntegrity} disabled={!connected}>
                    Disarm Integrity
                  </button>
                </div>

                <div className="hint">
                  Option A target: GUI should call daemon APIs only. Backtest/Research tabs are placeholders until the
                  daemon exposes job endpoints.
                </div>
              </Card>

              <Card title="Recent Alerts" subtitle="Last WARN/ERROR events">
                {recentAlerts.length === 0 ? (
                  <div className="muted">No warnings/errors observed yet.</div>
                ) : (
                  <div className="logMini">
                    {recentAlerts.map((e, i) => (
                      <div key={i} className={`logLine ${e.level.toLowerCase()}`}>
                        <span className="mono muted">{e.ts_local}</span>
                        <span className="pill">{e.level}</span>
                        <span className="mono muted">{e.topic}</span>
                        <span className="logMsg">{e.message}</span>
                      </div>
                    ))}
                  </div>
                )}
                <div className="hint">
                  Tip: use the Logs tab for filtering/search/export. Overview stays high-signal only.
                </div>
              </Card>

              <Card title="Quick Links" subtitle="Jump to high-work tabs">
                <div className="btnRow">
                  <button className="btn" onClick={() => setTab("logs")}>
                    View Logs
                  </button>
                  <button className="btn" onClick={() => setTab("backtest")}>
                    Backtest
                  </button>
                  <button className="btn" onClick={() => setTab("research")}>
                    Research
                  </button>
                  <button className="btn" onClick={() => setTab("config")}>
                    Config
                  </button>
                </div>
              </Card>
            </div>
          )}

          {tab === "trading" && (
            <div className="grid two">
              <Card title="Trading" subtitle="Placeholder (daemon endpoints not implemented yet)">
                <div className="muted">
                  This panel becomes real once the daemon exposes:
                  <ul className="bullets">
                    <li>
                      <span className="mono">GET /v1/trading/positions</span>, <span className="mono">/orders</span>,{" "}
                      <span className="mono">/fills</span>
                    </li>
                    <li>
                      <span className="mono">GET /v1/risk/summary</span>
                    </li>
                  </ul>
                </div>
                <div className="hint">Patch plan is in GUI_PATCH_TRACKER.md.</div>
              </Card>

              <Card title="Risk Snapshot" subtitle="Hard gates should be visible here">
                <div className="muted">
                  Future: show hard limits (max exposure, max orders, max loss), plus “armed/disarmed” and halt reasons.
                </div>
              </Card>
            </div>
          )}

          {tab === "backtest" && <BacktestPlaceholder connected={connected} />}

          {tab === "research" && <ResearchPlaceholder connected={connected} />}

          {tab === "logs" && (
            <div className="grid one">
              <Card title="Logs" subtitle="SSE stream + UI actions">
                <div className="toolbar">
                  <input
                    className="input"
                    placeholder="filter (substring match)"
                    value={logFilter}
                    onChange={(e) => setLogFilter(e.target.value)}
                  />
                  <button className="btn" onClick={() => setEntries([])}>
                    Clear
                  </button>
                  <button className="btn" onClick={() => setLogPaused((p) => !p)}>
                    {logPaused ? "Resume" : "Pause"}
                  </button>
                  <button
                    className="btn"
                    onClick={async () => {
                      const text = filteredEntries
                        .slice(0, 500)
                        .reverse()
                        .map((e) => `${e.ts_local}\t${e.source}\t${e.level}\t${e.topic}\t${e.message}`)
                        .join("\n");
                      await navigator.clipboard.writeText(text);
                      pushUi("copied logs to clipboard");
                    }}
                  >
                    Copy
                  </button>
                </div>

                <div className="logBox" ref={logRef}>
                  {filteredEntries.length === 0 ? (
                    <div className="muted">No log entries.</div>
                  ) : (
                    filteredEntries.map((e, i) => (
                      <div key={i} className={`logLine ${e.level.toLowerCase()}`}>
                        <span className="mono muted">{e.ts_local}</span>
                        <span className="pill">{e.level}</span>
                        <span className="mono muted">{e.topic}</span>
                        <span className="logMsg">{e.message}</span>
                      </div>
                    ))
                  )}
                </div>

                <div className="hint">
                  Stream source: <span className="mono">{daemonUrl}/v1/stream</span>.
                </div>
              </Card>
            </div>
          )}

          {tab === "config" && (
            <div className="grid two">
              <Card title="Connection" subtitle="Configure which daemon this GUI targets (Option A)">
                <div className="muted small">
                  Order of precedence: saved override → <span className="mono">VITE_MQK_DAEMON_URL</span> → default.
                </div>

                <div className="divider" />

                <div className="formGrid">
                  <label className="field">
                    <div className="label">Daemon URL</div>
                    <input
                      className="input"
                      value={configDraftUrl}
                      onChange={(e) => setConfigDraftUrl(e.target.value)}
                      placeholder="http://127.0.0.1:8899"
                    />
                  </label>

                  <div className="field">
                    <div className="label">Saved Override</div>
                    <div className="kvInline">
                      <span className="mono">{savedOverride ?? "—"}</span>
                      <span className={savedOverride ? "badge warn" : "badge ok"}>
                        {savedOverride ? "OVERRIDE" : "NONE"}
                      </span>
                    </div>
                  </div>
                </div>

                <div className="btnRow">
                  <button className="btn primary" onClick={applyDaemonUrl}>
                    Apply
                  </button>
                  <button className="btn" onClick={resetDaemonUrl}>
                    Reset
                  </button>
                </div>

                {configMsg ? (
                  <div className={configMsgLevel === "ERROR" ? "banner bad" : "banner ok"}>{configMsg}</div>
                ) : null}

                <div className="hint">
                  For remote/mobile later: do NOT expose control endpoints publicly without auth. That’s DAEMON-7 in the
                  tracker.
                </div>
              </Card>

              <Card title="Runtime" subtitle="What the app is currently using">
                <table className="table">
                  <tbody>
                    <tr>
                      <td className="tdLabel">Active URL</td>
                      <td className="tdValue mono">{daemonUrl}</td>
                    </tr>
                    <tr>
                      <td className="tdLabel">Default URL</td>
                      <td className="tdValue mono">{defaultDaemonUrl()}</td>
                    </tr>
                    <tr>
                      <td className="tdLabel">Connected</td>
                      <td className="tdValue">{connected ? "YES" : "NO"}</td>
                    </tr>
                    <tr>
                      <td className="tdLabel">State</td>
                      <td className="tdValue">{status?.state ?? "—"}</td>
                    </tr>
                    <tr>
                      <td className="tdLabel">Integrity</td>
                      <td className="tdValue">{status?.integrity_armed ? "ARMED" : "DISARMED"}</td>
                    </tr>
                  </tbody>
                </table>

                <div className="divider" />

                <div className="muted small">
                  Env var support:
                  <ul className="bullets">
                    <li>
                      create <span className="mono">.env.local</span> with{" "}
                      <span className="mono">VITE_MQK_DAEMON_URL=http://yourhost:8899</span>
                    </li>
                    <li>restart Vite/Tauri dev after changing env files</li>
                  </ul>
                </div>
              </Card>
            </div>
          )}
        </section>
      </main>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

function tabTitle(tab: UiTab): string {
  switch (tab) {
    case "overview":
      return "Overview";
    case "trading":
      return "Trading";
    case "backtest":
      return "Backtest";
    case "research":
      return "Research";
    case "logs":
      return "Logs";
    case "config":
      return "Config";
    default:
      return "MiniQuantDesk";
  }
}

function NavItem(props: { label: string; active: boolean; onClick: () => void }) {
  return (
    <button className={props.active ? "navItem active" : "navItem"} onClick={props.onClick}>
      {props.label}
    </button>
  );
}

function Card(props: { title: string; subtitle?: string; children: React.ReactNode }) {
  return (
    <div className="card">
      <div className="cardHeader">
        <div className="cardTitle">{props.title}</div>
        {props.subtitle ? <div className="cardSubtitle">{props.subtitle}</div> : null}
      </div>
      <div className="cardBody">{props.children}</div>
    </div>
  );
}

function BacktestPlaceholder(props: { connected: boolean }) {
  return (
    <div className="grid two">
      <Card title="Backtest Runner" subtitle="Placeholder UI (Option A: daemon job endpoints)">
        <div className="formGrid">
          <label className="field">
            <div className="label">Strategy</div>
            <select className="input" disabled>
              <option>select…</option>
            </select>
          </label>

          <label className="field">
            <div className="label">Universe</div>
            <input className="input" disabled value="(coming soon)" />
          </label>

          <label className="field">
            <div className="label">From</div>
            <input className="input" disabled value="YYYY-MM-DD" />
          </label>

          <label className="field">
            <div className="label">To</div>
            <input className="input" disabled value="YYYY-MM-DD" />
          </label>
        </div>

        <div className="btnRow">
          <button className="btn primary" disabled>
            Run Backtest
          </button>
          <button className="btn" disabled>
            View Last Result
          </button>
        </div>

        <div className="hint">
          Needed daemon APIs (planned):
          <ul className="bullets">
            <li>
              <span className="mono">POST /v1/backtest/jobs</span> (submit)
            </li>
            <li>
              <span className="mono">GET /v1/backtest/jobs/:id</span> (status + progress)
            </li>
            <li>
              <span className="mono">GET /v1/backtest/jobs/:id/artifacts</span> (metrics/curves/trades)
            </li>
          </ul>
        </div>
        {!props.connected ? <div className="banner bad">Daemon not connected.</div> : null}
      </Card>

      <Card title="Results" subtitle="Equity curve, metrics, trades (after daemon work)">
        <div className="muted">
          Future visuals:
          <ul className="bullets">
            <li>Equity + drawdown curve</li>
            <li>Summary metrics (CAGR, max DD, Sharpe-ish)</li>
            <li>Trades table + export</li>
          </ul>
        </div>
      </Card>
    </div>
  );
}

function ResearchPlaceholder(props: { connected: boolean }) {
  return (
    <div className="grid two">
      <Card title="Research Jobs" subtitle="Placeholder UI (Option A: daemon job endpoints)">
        <div className="muted">
          Research should run as deterministic jobs producing artifacts (CSV/JSON/plots). The GUI just submits jobs and
          renders results.
        </div>

        <div className="divider" />

        <div className="btnRow">
          <button className="btn primary" disabled>
            Run Factor Scan
          </button>
          <button className="btn" disabled>
            Build Universe
          </button>
          <button className="btn" disabled>
            Export Artifacts
          </button>
        </div>

        <div className="hint">
          Needed daemon APIs (planned):
          <ul className="bullets">
            <li>
              <span className="mono">POST /v1/research/jobs</span>
            </li>
            <li>
              <span className="mono">GET /v1/research/jobs/:id</span>
            </li>
            <li>
              <span className="mono">GET /v1/research/jobs/:id/artifacts</span>
            </li>
          </ul>
        </div>

        {!props.connected ? <div className="banner bad">Daemon not connected.</div> : null}
      </Card>

      <Card title="Artifacts" subtitle="Immutable outputs with provenance">
        <div className="muted">
          Future: list artifacts with:
          <ul className="bullets">
            <li>job id + parameters</li>
            <li>hashes + timestamps</li>
            <li>download/view options</li>
          </ul>
        </div>
      </Card>
    </div>
  );
}