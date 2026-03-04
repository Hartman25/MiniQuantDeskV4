import React, { useEffect, useState } from "react";
import { ControlStatus, getControlStatus, postArm, postDisarm, postRestart } from "../lib/runtimeApi";

type Props = {
  baseUrl: string; // daemon base url, e.g. http://localhost:8080
  pollMs?: number;
};

export function RuntimeControlPanel({ baseUrl, pollMs = 1500 }: Props) {
  const [status, setStatus] = useState<ControlStatus | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [reason, setReason] = useState<string>("");

  async function refresh() {
    try {
      setErr(null);
      const s = await getControlStatus(baseUrl);
      setStatus(s);
    } catch (e: any) {
      setErr(e?.message ?? String(e));
    }
  }

  useEffect(() => {
    refresh();
    const t = window.setInterval(refresh, pollMs);
    return () => window.clearInterval(t);
  }, [baseUrl, pollMs]);

  return (
    <div style={{ padding: 12, border: "1px solid #333", borderRadius: 12 }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <h3 style={{ margin: 0 }}>Runtime Control</h3>
        <button onClick={refresh}>Refresh</button>
      </div>

      {err && <div style={{ marginTop: 8, color: "tomato" }}>Error: {err}</div>}

      <div style={{ marginTop: 10, fontFamily: "monospace", fontSize: 13 }}>
        <div>desired_armed: {String(status?.desired_armed ?? "—")}</div>
        <div>leader_holder_id: {status?.leader_holder_id ?? "—"}</div>
        <div>leader_epoch: {status?.leader_epoch ?? "—"}</div>
        <div>lease_expires_at_utc: {status?.lease_expires_at_utc ?? "—"}</div>
      </div>

      <div style={{ marginTop: 12, display: "flex", gap: 8, flexWrap: "wrap" }}>
        <button onClick={() => postDisarm(baseUrl).then(refresh).catch((e) => setErr(String(e)))}>
          Disarm
        </button>
        <button onClick={() => postArm(baseUrl).then(refresh).catch((e) => setErr(String(e)))}>
          Arm
        </button>
      </div>

      <div style={{ marginTop: 12 }}>
        <div style={{ fontSize: 12, opacity: 0.8 }}>Restart reason (optional)</div>
        <input
          style={{ width: "100%", padding: 8, borderRadius: 8, border: "1px solid #333", marginTop: 6 }}
          value={reason}
          onChange={(e) => setReason(e.target.value)}
          placeholder="e.g. apply config change / recover from network wedge"
        />
        <button
          style={{ marginTop: 8 }}
          onClick={() => postRestart(baseUrl, reason).then(refresh).catch((e) => setErr(String(e)))}
        >
          Request Restart
        </button>
      </div>

      <div style={{ marginTop: 10, fontSize: 12, opacity: 0.8 }}>
        NOTE: This is scaffold UI. Real builds should gate restart behind auth and ensure runtime is disarmed before restart.
      </div>
    </div>
  );
}
