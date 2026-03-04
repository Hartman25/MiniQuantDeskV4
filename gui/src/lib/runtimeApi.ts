export type ControlStatus = {
  desired_armed: boolean;
  leader_holder_id: string | null;
  leader_epoch: number | null;
  lease_expires_at_utc: string | null;
};

export type RestartResponse = { restart_id: string };

export async function getControlStatus(baseUrl: string): Promise<ControlStatus> {
  const r = await fetch(`${baseUrl}/control/status`);
  if (!r.ok) throw new Error(`status ${r.status}`);
  return await r.json();
}

export async function postDisarm(baseUrl: string): Promise<void> {
  const r = await fetch(`${baseUrl}/control/disarm`, { method: "POST" });
  if (!r.ok && r.status !== 204) throw new Error(`disarm ${r.status}`);
}

export async function postArm(baseUrl: string): Promise<void> {
  const r = await fetch(`${baseUrl}/control/arm`, { method: "POST" });
  if (!r.ok && r.status !== 204) throw new Error(`arm ${r.status}`);
}

export async function postRestart(baseUrl: string, reason?: string): Promise<RestartResponse> {
  const r = await fetch(`${baseUrl}/control/restart`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ reason: reason ?? null }),
  });
  if (!r.ok) throw new Error(`restart ${r.status}`);
  return await r.json();
}
