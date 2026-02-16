-- mqk v4: minimal spine schema (PATCH 01)

create table if not exists runs (
  run_id         uuid primary key,
  engine_id      text not null,
  mode           text not null, -- PAPER | LIVE
  started_at_utc timestamptz not null default now(),
  git_hash       text not null,
  config_hash    text not null,
  config_json    jsonb not null,
  host_fingerprint text not null
);

create index if not exists idx_runs_started_at on runs(started_at_utc);

-- Immutable audit trail (JSONL mirrored in DB); hashchain optional now, enforced later.
create table if not exists audit_events (
  event_id       uuid primary key,
  run_id         uuid not null references runs(run_id) on delete cascade,
  ts_utc         timestamptz not null,
  topic          text not null,
  event_type     text not null,
  payload        jsonb not null,
  hash_prev      text null,
  hash_self      text null
);

create index if not exists idx_audit_run_ts on audit_events(run_id, ts_utc);

-- Outbox for order submissions (idempotent)
create table if not exists oms_outbox (
  outbox_id        bigserial primary key,
  run_id           uuid not null references runs(run_id) on delete cascade,
  idempotency_key  text not null,
  order_json       jsonb not null,
  status           text not null, -- PENDING | SENT | ACKED | FAILED
  created_at_utc   timestamptz not null default now(),
  sent_at_utc      timestamptz null
);

create unique index if not exists uq_outbox_idempotency on oms_outbox(idempotency_key);
create index if not exists idx_outbox_run_status on oms_outbox(run_id, status);

-- Inbox for broker messages/fills (dedupe)
create table if not exists oms_inbox (
  inbox_id          bigserial primary key,
  run_id            uuid not null references runs(run_id) on delete cascade,
  broker_message_id text not null,
  message_json      jsonb not null,
  received_at_utc   timestamptz not null default now()
);

create unique index if not exists uq_inbox_broker_message_id on oms_inbox(broker_message_id);
create index if not exists idx_inbox_run_received on oms_inbox(run_id, received_at_utc);
