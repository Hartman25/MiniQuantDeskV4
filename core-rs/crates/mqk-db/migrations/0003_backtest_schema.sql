-- PATCH A: Backtest / Replay schema additions (authoritative spec: MQD_Backtest_Spec_v1)
-- PATCH A.1: Fix FK target to match existing runs PK, remove redundant md_bars index,
--            and add default now() to run_events.ts.

-- Canonical market data (bars) as source of truth.
create table if not exists md_bars (
  symbol       text not null,
  timeframe    text not null,
  end_ts       bigint not null,
  open_micros  bigint not null,
  high_micros  bigint not null,
  low_micros   bigint not null,
  close_micros bigint not null,
  volume       bigint not null,
  is_complete  boolean not null,
  ingested_at  timestamptz not null default now(),
  primary key (symbol, timeframe, end_ts)
);

-- Query acceleration by time range across symbols/universe.
create index if not exists idx_md_bars_timeframe_end_ts
  on md_bars(timeframe, end_ts);

-- Append-only per-run event journal for backtest/replay.
-- Optional hash-chain fields included to align with existing audit_events conventions.
create table if not exists run_events (
  run_id       uuid not null references runs(run_id) on delete cascade,
  seq          bigint not null,
  ts           timestamptz not null default now(),
  end_ts       bigint null,
  event_type   text not null,
  payload_json jsonb not null,
  prev_hash    text null,
  event_hash   text null,
  primary key (run_id, seq)
);

-- Earnings calendar + other corporate events (v1 minimum: EARNINGS).
create table if not exists corporate_events (
  symbol      text not null,
  event_type  text not null,
  event_date  date not null,
  event_time  text null,
  source      text null,
  ingested_at timestamptz not null default now()
);

create index if not exists idx_corporate_events_symbol_date
  on corporate_events(symbol, event_date);

create index if not exists idx_corporate_events_type_date
  on corporate_events(event_type, event_date);

-- Symbol -> GICS sector mapping for sector concentration constraints.
create table if not exists symbol_gics (
  symbol          text not null,
  gics_sector     text not null,
  effective_start date null,
  effective_end   date null
);

create index if not exists idx_symbol_gics_symbol
  on symbol_gics(symbol);

create index if not exists idx_symbol_gics_sector
  on symbol_gics(gics_sector);