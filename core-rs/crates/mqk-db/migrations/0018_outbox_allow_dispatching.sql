-- Runtime leader lease + epoch table (scaffold)

CREATE TABLE IF NOT EXISTS runtime_leader_lease (
  id                SMALLINT PRIMARY KEY DEFAULT 1,
  holder_id         TEXT NOT NULL,
  epoch             BIGINT NOT NULL,
  lease_expires_at  TIMESTAMPTZ NOT NULL,
  -- updated_at injected by caller; no DEFAULT now() per [N] guard (>= 0012).
  updated_at        TIMESTAMPTZ NOT NULL,
  CHECK (id = 1)
);

-- Optional: store disarm/arm desired state in DB for operator control plane.
CREATE TABLE IF NOT EXISTS runtime_control_state (
  id            SMALLINT PRIMARY KEY DEFAULT 1,
  desired_armed BOOLEAN NOT NULL DEFAULT FALSE,
  -- updated_at injected by caller; no DEFAULT now() per [N] guard (>= 0012).
  updated_at    TIMESTAMPTZ NOT NULL,
  CHECK (id = 1)
);

-- Optional: restart requests (the daemon can write these; a supervisor can act on them)
CREATE TABLE IF NOT EXISTS runtime_restart_requests (
  restart_id    TEXT PRIMARY KEY,
  requested_by  TEXT NOT NULL,
  -- requested_at injected by caller; no DEFAULT now() per [N] guard (>= 0012).
  requested_at  TIMESTAMPTZ NOT NULL,
  reason        TEXT NULL
);