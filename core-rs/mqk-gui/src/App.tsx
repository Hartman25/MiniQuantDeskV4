import { useCallback, useEffect, useMemo, useRef, useState } from "react";

// ---------------------------------------------------------------------------
// Config — single place for the daemon base URL
// ---------------------------------------------------------------------------
const DAEMON_URL = "http://127.0.0.1:8899";

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

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

export default function App() {
  const [statusOk, setStatusOk] = useState(false);
  const [sseOk, setSseOk] = useState(false);
  const [status, setStatus] = useState<StatusSnapshot | null>(null);
  const [events, setEvents] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const logRef = useRef<HTMLDivElement>(null);

  const connected = useMemo(() => statusOk || sseOk, [statusOk, sseOk]);

  // -------------------------------------------------------------------------
  // Helpers
  // -------------------------------------------------------------------------

  const pushLog = useCallback((line: string) => {
    setEvents((prev) => [line, ...prev.slice(0, 199)]);
  }, []);

  /** Fetch the latest status snapshot immediately (post-action refresh). */
  const refreshStatus = useCallback(async () => {
    try {
      const res = await fetch(`${DAEMON_URL}/v1/status`);
      if (!res.ok) throw new Error(`status ${res.status}`);
      const json = (await res.json()) as StatusSnapshot;
      setStatus(json);
      setStatusOk(true);
    } catch (e) {
      setStatusOk(false);
      console.error("refreshStatus:", e);
    }
  }, []);

  /**
   * POST a control-plane command.
   * On success: clears error, logs the action, refreshes status.
   * On failure: shows an error banner.
   */
  const postCmd = useCallback(
    async (path: string, label: string) => {
      setBusy(true);
      setError(null);
      try {
        const res = await fetch(`${DAEMON_URL}${path}`, { method: "POST" });
        if (!res.ok) {
          const body = await res.text().catch(() => "");
          throw new Error(`${res.status}${body ? `: ${body}` : ""}`);
        }
        pushLog(`CMD: ${label} → OK`);
        await refreshStatus();
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        setError(`${label} failed — ${msg}`);
        pushLog(`CMD: ${label} → ERROR: ${msg}`);
      } finally {
        setBusy(false);
      }
    },
    [pushLog, refreshStatus]
  );

  // -------------------------------------------------------------------------
  // Button handlers
  // -------------------------------------------------------------------------

  const handleStart = () => postCmd("/v1/run/start", "Start Run");
  const handleStop = () => postCmd("/v1/run/stop", "Stop Run");
  const handleHalt = () => postCmd("/v1/run/halt", "Halt Run");
  const handleArm = () => postCmd("/v1/integrity/arm", "Arm Integrity");
  const handleDisarm = () => postCmd("/v1/integrity/disarm", "Disarm Integrity");

  // -------------------------------------------------------------------------
  // Derived disabled states
  // -------------------------------------------------------------------------

  const runState = status?.state?.toUpperCase() ?? "IDLE";
  const armed = status?.integrity_armed ?? true;

  const isRunning = runState === "RUNNING";
  const isHalted = runState === "HALTED";
  const isIdle = !isRunning && !isHalted;

  // Start: only when idle and not busy
  const startDisabled = busy || !isIdle;
  // Stop: only when running or halted (stop clears halted too)
  const stopDisabled = busy || isIdle;
  // Halt: only when running (already halted = no-op/disable)
  const haltDisabled = busy || !isRunning;
  // Arm: only when currently disarmed
  const armDisabled = busy || armed;
  // Disarm: only when currently armed
  const disarmDisabled = busy || !armed;

  // -------------------------------------------------------------------------
  // Poll status (fallback when SSE is disconnected; runs always)
  // -------------------------------------------------------------------------

  useEffect(() => {
    const interval = setInterval(async () => {
      try {
        const res = await fetch(`${DAEMON_URL}/v1/status`);
        if (!res.ok) throw new Error("status failed");
        const json = (await res.json()) as StatusSnapshot;
        setStatus(json);
        setStatusOk(true);
      } catch {
        setStatusOk(false);
      }
    }, 1500);

    return () => clearInterval(interval);
  }, []);

  // -------------------------------------------------------------------------
  // SSE stream — authoritative live updates
  // -------------------------------------------------------------------------

  useEffect(() => {
    let es: EventSource;

    const connect = () => {
      es = new EventSource(`${DAEMON_URL}/v1/stream`);

      es.onopen = () => setSseOk(true);
      es.onerror = () => setSseOk(false);

      es.addEventListener("heartbeat", () => {
        pushLog(`HB: ${new Date().toLocaleTimeString()}`);
      });

      es.addEventListener("status", (e) => {
        const raw = (e as MessageEvent).data as string;
        pushLog(`STATUS: ${raw}`);
        // Parse and apply to state so SSE is the primary live source.
        try {
          const snap = JSON.parse(raw) as StatusSnapshot;
          // The SSE status event wraps the payload under `type` + the fields,
          // so we need to handle the tagged union: { type: "status", ...snap }
          // or { type: "status", ...StatusSnapshot fields }
          // The daemon emits: BusMsg::Status(StatusSnapshot) serialised as
          // { "type": "status", "daemon_uptime_secs": ..., "state": ..., ... }
          // All StatusSnapshot fields are at the top level alongside "type".
          if (snap.state !== undefined) {
            setStatus(snap);
            setStatusOk(true);
          }
        } catch {
          // unparseable — ignore, polling will correct
        }
      });

      es.addEventListener("log", (e) => {
        const data = (e as MessageEvent).data as string;
        pushLog(`LOG: ${data}`);
      });
    };

    connect();
    return () => es?.close();
  }, [pushLog]);

  // -------------------------------------------------------------------------
  // Auto-scroll log panel to top (newest entry is at top)
  // -------------------------------------------------------------------------

  useEffect(() => {
    if (logRef.current) logRef.current.scrollTop = 0;
  }, [events]);

  // -------------------------------------------------------------------------
  // Render
  // -------------------------------------------------------------------------

  return (
    <div style={styles.app}>
      {/* Header */}
      <header style={styles.header}>
        <div>MiniQuantDesk Control</div>
        <div style={{ color: connected ? "#22c55e" : "#ef4444", fontWeight: 600 }}>
          {connected ? "● CONNECTED" : "○ DISCONNECTED"}
        </div>
      </header>

      {/* Error banner */}
      {error && (
        <div style={styles.errorBanner}>
          <span>{error}</span>
          <button style={styles.dismissBtn} onClick={() => setError(null)}>
            ✕
          </button>
        </div>
      )}

      <div style={styles.main}>
        {/* Left column: Status + Controls */}
        <div style={{ display: "flex", flexDirection: "column", gap: 16, flex: 1 }}>
          {/* Status card */}
          <div style={styles.card}>
            <h3 style={styles.cardTitle}>Status</h3>

            <p style={styles.metaRow}>
              poll ok: {statusOk ? "yes" : "no"} &nbsp;|&nbsp; sse ok: {sseOk ? "yes" : "no"}
            </p>

            {!status ? (
              <p style={{ color: "#94a3b8" }}>Waiting for daemon…</p>
            ) : (
              <table style={styles.table}>
                <tbody>
                  <tr>
                    <td style={styles.tdLabel}>State</td>
                    <td style={{ ...styles.tdValue, color: stateColor(status.state) }}>
                      {status.state.toUpperCase()}
                    </td>
                  </tr>
                  <tr>
                    <td style={styles.tdLabel}>Run ID</td>
                    <td style={styles.tdValue}>
                      {status.active_run_id ?? <span style={{ opacity: 0.5 }}>None</span>}
                    </td>
                  </tr>
                  <tr>
                    <td style={styles.tdLabel}>Integrity</td>
                    <td
                      style={{
                        ...styles.tdValue,
                        color: status.integrity_armed ? "#22c55e" : "#f59e0b",
                        fontWeight: 600,
                      }}
                    >
                      {status.integrity_armed ? "ARMED" : "DISARMED"}
                    </td>
                  </tr>
                  <tr>
                    <td style={styles.tdLabel}>Uptime</td>
                    <td style={styles.tdValue}>{status.daemon_uptime_secs}s</td>
                  </tr>
                  {status.notes && (
                    <tr>
                      <td style={styles.tdLabel}>Notes</td>
                      <td style={{ ...styles.tdValue, color: "#94a3b8", fontSize: 11 }}>
                        {status.notes}
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            )}
          </div>

          {/* Run lifecycle controls */}
          <div style={styles.card}>
            <h3 style={styles.cardTitle}>Run Lifecycle</h3>
            <div style={styles.btnRow}>
              <button
                style={{ ...styles.btn, ...styles.btnGreen }}
                disabled={startDisabled}
                onClick={handleStart}
                title="POST /v1/run/start"
              >
                ▶ Start Run
              </button>
              <button
                style={{ ...styles.btn, ...styles.btnYellow }}
                disabled={stopDisabled}
                onClick={handleStop}
                title="POST /v1/run/stop"
              >
                ■ Stop Run
              </button>
              <button
                style={{ ...styles.btn, ...styles.btnRed }}
                disabled={haltDisabled}
                onClick={handleHalt}
                title="POST /v1/run/halt"
              >
                ⛔ Halt Run
              </button>
            </div>
          </div>

          {/* Integrity controls */}
          <div style={styles.card}>
            <h3 style={styles.cardTitle}>Integrity</h3>
            <div style={styles.btnRow}>
              <button
                style={{ ...styles.btn, ...styles.btnGreen }}
                disabled={armDisabled}
                onClick={handleArm}
                title="POST /v1/integrity/arm"
              >
                🛡 Arm
              </button>
              <button
                style={{ ...styles.btn, ...styles.btnYellow }}
                disabled={disarmDisabled}
                onClick={handleDisarm}
                title="POST /v1/integrity/disarm"
              >
                ⚠ Disarm
              </button>
            </div>
          </div>
        </div>

        {/* Right column: Live stream log */}
        <div style={{ ...styles.card, flex: 1.4 }}>
          <h3 style={styles.cardTitle}>
            Live Stream{" "}
            <span style={{ fontSize: 11, fontWeight: 400, color: "#64748b" }}>
              (newest first · max 200)
            </span>
          </h3>
          <div style={styles.logBox} ref={logRef}>
            {events.length === 0 ? (
              <div style={{ color: "#475569", fontSize: 12 }}>No events yet…</div>
            ) : (
              events.map((e, i) => (
                <div key={i} style={logLineStyle(e)}>
                  {e}
                </div>
              ))
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function stateColor(state: string): string {
  switch (state.toUpperCase()) {
    case "RUNNING":
      return "#22c55e";
    case "HALTED":
      return "#ef4444";
    case "IDLE":
      return "#94a3b8";
    default:
      return "#e2e8f0";
  }
}

function logLineStyle(line: string): React.CSSProperties {
  const base: React.CSSProperties = { fontSize: 11, marginBottom: 3, fontFamily: "monospace" };
  if (line.startsWith("CMD:") && line.includes("ERROR"))
    return { ...base, color: "#f87171" };
  if (line.startsWith("CMD:"))
    return { ...base, color: "#86efac" };
  if (line.startsWith("LOG:"))
    return { ...base, color: "#fde68a" };
  if (line.startsWith("HB:"))
    return { ...base, color: "#334155" };
  return { ...base, color: "#cbd5e1" };
}

// ---------------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------------

const styles: { [key: string]: React.CSSProperties } = {
  app: {
    backgroundColor: "#0f172a",
    color: "#e2e8f0",
    minHeight: "100vh",
    fontFamily: "system-ui, sans-serif",
  },
  header: {
    display: "flex",
    justifyContent: "space-between",
    alignItems: "center",
    padding: "14px 24px",
    borderBottom: "1px solid #1e293b",
    fontSize: 16,
    fontWeight: 600,
  },
  errorBanner: {
    display: "flex",
    justifyContent: "space-between",
    alignItems: "center",
    backgroundColor: "#7f1d1d",
    color: "#fca5a5",
    padding: "10px 24px",
    fontSize: 13,
    borderBottom: "1px solid #991b1b",
  },
  dismissBtn: {
    background: "none",
    border: "none",
    color: "#fca5a5",
    cursor: "pointer",
    fontSize: 16,
    padding: "0 4px",
    lineHeight: 1,
  },
  main: {
    display: "flex",
    gap: 16,
    padding: "20px 24px",
    alignItems: "flex-start",
  },
  card: {
    backgroundColor: "#1e293b",
    padding: "18px 20px",
    borderRadius: 8,
    minWidth: 0,
  },
  cardTitle: {
    margin: "0 0 12px 0",
    fontSize: 14,
    fontWeight: 600,
    color: "#94a3b8",
    textTransform: "uppercase" as const,
    letterSpacing: "0.05em",
  },
  metaRow: {
    fontSize: 11,
    color: "#475569",
    margin: "0 0 10px 0",
  },
  table: {
    width: "100%",
    borderCollapse: "collapse" as const,
  },
  tdLabel: {
    color: "#64748b",
    fontSize: 12,
    padding: "4px 12px 4px 0",
    whiteSpace: "nowrap" as const,
    verticalAlign: "top" as const,
  },
  tdValue: {
    color: "#e2e8f0",
    fontSize: 13,
    padding: "4px 0",
    wordBreak: "break-all" as const,
  },
  btnRow: {
    display: "flex",
    flexWrap: "wrap" as const,
    gap: 10,
  },
  btn: {
    padding: "8px 16px",
    fontSize: 13,
    fontWeight: 600,
    border: "none",
    borderRadius: 6,
    cursor: "pointer",
    transition: "opacity 0.15s",
    opacity: 1,
  },
  btnGreen: {
    backgroundColor: "#166534",
    color: "#bbf7d0",
  },
  btnYellow: {
    backgroundColor: "#78350f",
    color: "#fde68a",
  },
  btnRed: {
    backgroundColor: "#7f1d1d",
    color: "#fecaca",
  },
  logBox: {
    height: 420,
    overflowY: "auto" as const,
    backgroundColor: "#0f172a",
    padding: "10px 12px",
    borderRadius: 6,
  },
};
