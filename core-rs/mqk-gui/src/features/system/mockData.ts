import type {
  AuditActionRow,
  CausalityTrace,
  ExecutionOrderRow,
  ExecutionReplay,
  ExecutionChartModel,
  ExecutionSummary,
  ExecutionTimeline,
  ExecutionTrace,
  FeedEvent,
  FillRow,
  ConfigDiffRow,
  ConfigFingerprintSummary,
  IncidentCase,
  MarketDataQualitySummary,
  MetadataSummary,
  ReplaceCancelChainRow,
  RuntimeLeadershipSummary,
  AlertTriageRow,
  ServiceTopology,
  SessionStateSummary,
  TransportSummary,
  OmsOverview,
  OpenOrderRow,
  OperatorActionDefinition,
  OperatorAlert,
  OperatorTimelineEvent,
  PortfolioSummary,
  PositionRow,
  PreflightStatus,
  ReconcileMismatchRow,
  ReconcileSummary,
  RiskDenialRow,
  RiskSummary,
  StrategyRow,
  StrategySuppressionRow,
  SystemMetrics,
  ArtifactRegistrySummary,
  SystemModel,
  SystemStatus,
} from "./types";
import { classifyPanelSources } from "./sourceAuthority";

const now = new Date();
const iso = (minutesAgo = 0) => new Date(now.getTime() - minutesAgo * 60_000).toISOString();

export const MOCK_STATUS: SystemStatus = {
  environment: "paper",
  runtime_status: "running",
  broker_status: "ok",
  db_status: "ok",
  market_data_health: "warning",
  reconcile_status: "warning",
  integrity_status: "ok",
  audit_writer_status: "ok",
  last_heartbeat: iso(0),
  loop_latency_ms: 186,
  active_account_id: "PAPER-ALPACA-01",
  config_profile: "windows-dev.paper.main",
  has_warning: true,
  has_critical: false,
  strategy_armed: true,
  execution_armed: true,
  live_routing_enabled: false,
  kill_switch_active: false,
  risk_halt_active: false,
  integrity_halt_active: false,
  daemon_reachable: true,
};

export const MOCK_PREFLIGHT: PreflightStatus = {
  daemon_reachable: true,
  db_reachable: true,
  broker_config_present: true,
  market_data_config_present: true,
  audit_writer_ready: true,
  runtime_idle: false,
  strategy_disarmed: false,
  execution_disarmed: false,
  live_routing_disabled: true,
  warnings: ["Market data freshness is degraded for 2 symbols."],
  blockers: [],
};

export const MOCK_ALERTS: OperatorAlert[] = [
  {
    id: "a1",
    severity: "warning",
    title: "Reconcile mismatch detected",
    message: "Broker order qty differs from internal open quantity for NVDA ladder exit.",
    domain: "reconcile",
  },
  {
    id: "a2",
    severity: "warning",
    title: "Market data freshness degraded",
    message: "Primary quote feed delayed beyond configured threshold for AMD and AVGO.",
    domain: "metrics",
  },
];

export const MOCK_FEED: FeedEvent[] = [
  { id: "f1", at: iso(1), severity: "info", source: "runtime", text: "Heartbeat received. Runtime loop latency 186 ms." },
  { id: "f2", at: iso(2), severity: "warning", source: "reconcile", text: "Order drift detected on NVDA internal order O-240308-001." },
  { id: "f3", at: iso(3), severity: "info", source: "risk", text: "No active hard breaches. Loss-limit utilization 41.8%." },
  { id: "f4", at: iso(4), severity: "warning", source: "market-data", text: "Quote staleness crossed soft threshold for AMD." },
];

export const MOCK_EXECUTION_SUMMARY: ExecutionSummary = {
  active_orders: 7,
  pending_orders: 2,
  dispatching_orders: 1,
  reject_count_today: 1,
  cancel_replace_count_today: 4,
  avg_ack_latency_ms: 384,
  stuck_orders: 1,
};

export const MOCK_EXECUTION_ORDERS: ExecutionOrderRow[] = [
  {
    internal_order_id: "O-240308-001",
    broker_order_id: "ALP-9918122",
    symbol: "NVDA",
    strategy_id: "breakout_momo",
    side: "buy",
    order_type: "limit",
    requested_qty: 120,
    filled_qty: 40,
    current_status: "partially_filled",
    current_stage: "Partial Fill",
    age_ms: 198000,
    has_warning: true,
    has_critical: false,
    updated_at: iso(1),
  },
  {
    internal_order_id: "O-240308-002",
    broker_order_id: "ALP-9918123",
    symbol: "AMD",
    strategy_id: "pullback_core",
    side: "sell",
    order_type: "market",
    requested_qty: 80,
    filled_qty: 80,
    current_status: "filled",
    current_stage: "Closed",
    age_ms: 51000,
    has_warning: false,
    has_critical: false,
    updated_at: iso(2),
  },
  {
    internal_order_id: "O-240308-003",
    broker_order_id: null,
    symbol: "AVGO",
    strategy_id: "mean_revert",
    side: "buy",
    order_type: "limit",
    requested_qty: 25,
    filled_qty: 0,
    current_status: "dispatching",
    current_stage: "Dispatching",
    age_ms: 640000,
    has_warning: false,
    has_critical: true,
    updated_at: iso(6),
  },
];

export const MOCK_TIMELINE: ExecutionTimeline = {
  timeline_id: "TL-O-240308-001",
  environment: "paper",
  account_id: "PAPER-ALPACA-01",
  symbol: "NVDA",
  strategy_id: "breakout_momo",
  internal_order_id: "O-240308-001",
  broker_order_id: "ALP-9918122",
  parent_intent_id: "INT-992201",
  side: "buy",
  order_type: "limit",
  requested_qty: 120,
  filled_qty: 40,
  current_status: "partially_filled",
  current_stage: "Partial Fill",
  opened_at: iso(8),
  last_updated_at: iso(1),
  is_terminal: false,
  has_warning: true,
  has_critical: false,
  stages: [
    ["signal", "Signal Generated", "completed", 8, 7, "strategy", "Signal emitted after breakout confirmation."],
    ["intent", "Intent Created", "completed", 7, 7, "runtime", "Intent persisted into orchestrator queue."],
    ["risk", "Risk Evaluated", "completed", 7, 7, "risk", "Position sizing passed, no hard denial."],
    ["outbox", "Outbox Inserted", "completed", 7, 6, "db", "Outbox row written with claim token pending."],
    ["pending", "Pending", "completed", 6, 6, "execution", "Order waiting to be claimed."],
    ["claimed", "Claimed", "completed", 6, 6, "execution", "Claim token issued to dispatcher."],
    ["dispatching", "Dispatching", "completed", 6, 5, "execution", "Serialized broker submit attempt."],
    ["submit", "Broker Submit Attempted", "completed", 5, 5, "broker", "Submit HTTP request sent to broker adapter."],
    ["sent", "Sent", "completed", 5, 4, "broker", "Request accepted by broker edge."],
    ["ack", "Broker Acknowledged", "late", 4, 3, "broker", "ACK arrived slower than warning threshold."],
    ["partial_fill", "Partial Fill", "active", 3, 0, "broker", "40 / 120 shares filled across two executions."],
    ["closed", "Closed", "not_reached", 0, 0, "reconcile", "Terminal stage not yet reached."],
  ].map(([stage_key, stage_label, status, startedAgo, completedAgo, source_system, details], index) => ({
    stage_key: String(stage_key),
    stage_label: String(stage_label),
    sequence: index + 1,
    status: status as any,
    started_at: iso(Number(startedAgo)),
    completed_at: String(status) === "active" || String(status) === "not_reached" ? null : iso(Number(completedAgo)),
    duration_ms: String(status) === "not_reached" ? null : Math.max(500, (Number(startedAgo) - Number(completedAgo)) * 60_000),
    age_ms: String(status) === "active" ? 180_000 : null,
    expected: true,
    present: String(status) !== "missing",
    source_system: String(source_system),
    source_ref: `REF-${index + 1}`,
    details: String(details),
    severity: String(status) === "late" ? "warning" : String(status) === "failed" || String(status) === "missing" ? "critical" : "info",
    is_current: String(status) === "active",
    is_terminal_stage: String(stage_key) === "closed",
  })),
  incident_events: [
    {
      incident_id: "INC-991",
      incident_type: "delayed_ack",
      severity: "warning",
      message: "Broker ACK exceeded soft threshold by 1.7 seconds.",
      at: iso(3),
    },
    {
      incident_id: "INC-992",
      incident_type: "reconcile_mismatch",
      severity: "warning",
      message: "Internal open qty 80 disagrees with broker open qty 60 until next snapshot refresh.",
      at: iso(2),
    },
  ],
  event_rows: [
    {
      event_id: "EV-1",
      timestamp: iso(8),
      event_type: "signal_generated",
      source_system: "strategy",
      severity: "info",
      order_id: "O-240308-001",
      broker_order_id: null,
      stage_key: "signal",
      message: "Breakout threshold crossed above pre-market high.",
      payload_json: '{"symbol":"NVDA","side":"buy"}',
      is_duplicate: false,
      is_replayed: false,
      is_operator_visible: true,
    },
    {
      event_id: "EV-2",
      timestamp: iso(5),
      event_type: "broker_ack",
      source_system: "broker",
      severity: "warning",
      order_id: "O-240308-001",
      broker_order_id: "ALP-9918122",
      stage_key: "ack",
      message: "Broker ACK arrived beyond latency threshold.",
      payload_json: '{"ack_latency_ms":2120}',
      is_duplicate: false,
      is_replayed: false,
      is_operator_visible: true,
    },
    {
      event_id: "EV-3",
      timestamp: iso(2),
      event_type: "partial_fill",
      source_system: "broker",
      severity: "info",
      order_id: "O-240308-001",
      broker_order_id: "ALP-9918122",
      stage_key: "partial_fill",
      message: "Second execution applied. Total filled now 40 shares.",
      payload_json: '{"fill_qty":20,"fill_price":945.22}',
      is_duplicate: false,
      is_replayed: false,
      is_operator_visible: true,
    },
  ],
  reconcile_summary: {
    status: "warning",
    last_run_at: iso(1),
    mismatched_positions: 0,
    mismatched_orders: 1,
    mismatched_fills: 0,
    unmatched_broker_events: 1,
  },
};

export const MOCK_OMS_OVERVIEW: OmsOverview = {
  total_active_orders: 7,
  stuck_orders: 1,
  missing_transition_orders: 1,
  state_nodes: [
    { state: "open", active_count: 2, warning_count: 1, over_sla_count: 1, avg_dwell_ms: 82000, p95_dwell_ms: 161000 },
    { state: "partially_filled", active_count: 2, warning_count: 1, over_sla_count: 1, avg_dwell_ms: 121000, p95_dwell_ms: 198000 },
    { state: "filled", active_count: 1, warning_count: 0, over_sla_count: 0, avg_dwell_ms: 52000, p95_dwell_ms: 52000 },
    { state: "cancelled", active_count: 1, warning_count: 0, over_sla_count: 0, avg_dwell_ms: 34000, p95_dwell_ms: 34000 },
    { state: "rejected", active_count: 1, warning_count: 1, over_sla_count: 0, avg_dwell_ms: 18000, p95_dwell_ms: 18000 },
  ],
  transition_edges: [
    { from_state: "open", to_state: "partially_filled", transition_count: 5, median_latency_ms: 2310, anomaly_count: 1 },
    { from_state: "partially_filled", to_state: "filled", transition_count: 4, median_latency_ms: 5410, anomaly_count: 0 },
    { from_state: "open", to_state: "cancelled", transition_count: 2, median_latency_ms: 3310, anomaly_count: 0 },
    { from_state: "open", to_state: "rejected", transition_count: 1, median_latency_ms: 980, anomaly_count: 1 },
  ],
  orders: [
    {
      internal_order_id: "O-240308-001",
      broker_order_id: "ALP-9918122",
      strategy_id: "breakout_momo",
      symbol: "NVDA",
      side: "buy",
      requested_qty: 120,
      filled_qty: 40,
      oms_state: "partially_filled",
      execution_stage: "Partial Fill",
      entered_state_at: iso(3),
      dwell_ms: 180000,
      sla_ms: 120000,
      is_stuck: true,
      severity: "warning",
    },
    {
      internal_order_id: "O-240308-003",
      broker_order_id: null,
      strategy_id: "mean_revert",
      symbol: "AVGO",
      side: "buy",
      requested_qty: 25,
      filled_qty: 0,
      oms_state: "open",
      execution_stage: "Dispatching",
      entered_state_at: iso(6),
      dwell_ms: 360000,
      sla_ms: 90000,
      is_stuck: true,
      severity: "critical",
    },
    {
      internal_order_id: "O-240308-002",
      broker_order_id: "ALP-9918123",
      strategy_id: "pullback_core",
      symbol: "AMD",
      side: "sell",
      requested_qty: 80,
      filled_qty: 80,
      oms_state: "filled",
      execution_stage: "Closed",
      entered_state_at: iso(2),
      dwell_ms: 52000,
      sla_ms: 120000,
      is_stuck: false,
      severity: "info",
    },
  ],
};

export const MOCK_EXECUTION_TRACE: ExecutionTrace = {
  internal_order_id: "O-240308-001",
  broker_order_id: "ALP-9918122",
  parent_order_id: null,
  strategy_id: "breakout_momo",
  symbol: "NVDA",
  side: "buy",
  qty: 120,
  current_execution_state: "partial_fill_active",
  current_oms_state: "partially_filled",
  submit_time: iso(8),
  terminal_time: null,
  replay_available: true,
  correlation: {
    internal_order_id: "O-240308-001",
    broker_order_id: "ALP-9918122",
    parent_order_id: null,
    outbox_id: "OUT-19021",
    claim_token: "CLM-b71e-44f1",
    dispatch_attempt_id: "DSP-9013",
    inbox_ids: ["IN-8012", "IN-8013"],
    fill_ids: ["F-1", "F-2"],
    reconcile_case_id: "REC-221",
    audit_chain_id: "AUD-CHAIN-301",
  },
  timeline: [
    { trace_event_id: "TR-1", timestamp: iso(8), subsystem: "strategy", event_type: "signal_generated", before_state: "none", after_state: "intent_created", latency_since_prev_ms: null, summary: "Breakout signal emitted.", payload_digest: "signal:NVDA:buy", anomaly_tags: [] },
    { trace_event_id: "TR-2", timestamp: iso(7), subsystem: "risk", event_type: "risk_pass", before_state: "intent_created", after_state: "risk_passed", latency_since_prev_ms: 80, summary: "Sizing accepted.", payload_digest: "risk:pass", anomaly_tags: [] },
    { trace_event_id: "TR-3", timestamp: iso(6), subsystem: "db", event_type: "outbox_inserted", before_state: "risk_passed", after_state: "pending", latency_since_prev_ms: 120, summary: "Outbox persisted.", payload_digest: "outbox:OUT-19021", anomaly_tags: [] },
    { trace_event_id: "TR-4", timestamp: iso(5), subsystem: "execution", event_type: "dispatch_claimed", before_state: "pending", after_state: "claimed", latency_since_prev_ms: 55, summary: "Dispatcher claimed token.", payload_digest: "claim:CLM-b71e-44f1", anomaly_tags: [] },
    { trace_event_id: "TR-5", timestamp: iso(5), subsystem: "broker", event_type: "submit_sent", before_state: "claimed", after_state: "sent", latency_since_prev_ms: 180, summary: "Submit posted to broker edge.", payload_digest: "submit:DSP-9013", anomaly_tags: [] },
    { trace_event_id: "TR-6", timestamp: iso(4), subsystem: "broker", event_type: "broker_ack", before_state: "sent", after_state: "acked", latency_since_prev_ms: 2120, summary: "ACK exceeded soft threshold.", payload_digest: "ack:ALP-9918122", anomaly_tags: ["delayed_ack"] },
    { trace_event_id: "TR-7", timestamp: iso(3), subsystem: "broker", event_type: "partial_fill", before_state: "acked", after_state: "partial_fill_active", latency_since_prev_ms: 1430, summary: "First 20-share execution applied.", payload_digest: "fill:X-8001", anomaly_tags: [] },
    { trace_event_id: "TR-8", timestamp: iso(2), subsystem: "broker", event_type: "partial_fill", before_state: "partial_fill_active", after_state: "partial_fill_active", latency_since_prev_ms: 980, summary: "Second execution applied.", payload_digest: "fill:X-8002", anomaly_tags: [] },
    { trace_event_id: "TR-9", timestamp: iso(2), subsystem: "reconcile", event_type: "drift_marker", before_state: "partial_fill_active", after_state: "partial_fill_active", latency_since_prev_ms: 310, summary: "Broker open qty disagreed with runtime qty until refresh.", payload_digest: "reconcile:REC-221", anomaly_tags: ["reconcile_mismatch"] },
  ],
  broker_messages: [
    { message_id: "MSG-1", timestamp: iso(5), direction: "outbound", message_type: "submit_order", normalized_summary: "BUY 120 NVDA LIMIT @ 945.00", raw_payload: '{"symbol":"NVDA","qty":120,"limit_price":945.0}' },
    { message_id: "MSG-2", timestamp: iso(4), direction: "inbound", message_type: "ack", normalized_summary: "Accepted by broker with order ALP-9918122", raw_payload: '{"status":"accepted","broker_order_id":"ALP-9918122"}' },
    { message_id: "MSG-3", timestamp: iso(3), direction: "inbound", message_type: "fill", normalized_summary: "Partial fill 20 @ 944.20", raw_payload: '{"exec_id":"X-8001","qty":20,"price":944.20}' },
  ],
  state_ladder: [
    { key: "SL-1", oms_state: "open", execution_state: "pending", broker_state: "new", reconcile_state: "clean", at: iso(6) },
    { key: "SL-2", oms_state: "open", execution_state: "sent", broker_state: "accepted", reconcile_state: "clean", at: iso(4) },
    { key: "SL-3", oms_state: "partially_filled", execution_state: "partial_fill_active", broker_state: "partially_filled", reconcile_state: "warning", at: iso(2) },
  ],
  fills: [
    { fill_id: "F-1", timestamp: iso(3), qty: 20, price: 944.2, liquidity_flag: "taker", fee_estimate: 0.42, fee_actual: 0.41, cumulative_filled_qty: 20, average_fill_price: 944.2, slippage_bps: 1.1 },
    { fill_id: "F-2", timestamp: iso(2), qty: 20, price: 945.22, liquidity_flag: "taker", fee_estimate: 0.42, fee_actual: 0.43, cumulative_filled_qty: 40, average_fill_price: 944.71, slippage_bps: 1.6 },
  ],
};


export const MOCK_CAUSALITY_TRACE: CausalityTrace = {
  incident_id: "INC-O-240308-001",
  internal_order_id: "O-240308-001",
  broker_order_id: "ALP-9918122",
  strategy_id: "breakout_momo",
  symbol: "NVDA",
  side: "buy",
  target_qty: 120,
  filled_qty: 40,
  current_oms_state: "partially_filled",
  current_execution_state: "broker_partially_filled",
  current_reconcile_status: "warning",
  terminal_outcome: "working",
  replay_available: true,
  audit_chain_id: "AUD-CHAIN-9001",
  severity: "warning",
  correlation: {
    signal_id: "SIG-20260308-441",
    decision_id: "DEC-20260308-440",
    intent_id: "INT-20260308-122",
    internal_order_id: "O-240308-001",
    broker_order_id: "ALP-9918122",
    outbox_id: "OBX-10001",
    claim_token: "CLM-8891",
    dispatch_attempt_id: "DSP-3301",
    inbox_ids: ["INB-1001", "INB-1002"],
    fill_ids: ["F-1", "F-2"],
    portfolio_event_ids: ["PF-2001", "PF-2002"],
    reconcile_case_id: "RC-77",
    audit_chain_id: "AUD-CHAIN-9001",
    run_id: "RUN-20260308-AM-01",
  },
  nodes: [
    { node_key: "signal", node_type: "signal", title: "Breakout signal emitted", status: "ok", subsystem: "strategy", linked_id: "SIG-20260308-441", timestamp: iso(8), elapsed_from_prev_ms: null, anomaly_tags: [], summary: "Signal crossed opening-range breakout threshold on NVDA." },
    { node_key: "intent", node_type: "intent", title: "Intent created", status: "ok", subsystem: "runtime", linked_id: "INT-20260308-122", timestamp: iso(7), elapsed_from_prev_ms: 830, anomaly_tags: [], summary: "Buy 120 shares at breakout ladder settings." },
    { node_key: "risk", node_type: "risk_gate", title: "Risk gate approved", status: "ok", subsystem: "risk", linked_id: "DEC-20260308-440", timestamp: iso(7), elapsed_from_prev_ms: 290, anomaly_tags: [], summary: "Loss, concentration, and regime checks passed." },
    { node_key: "outbox", node_type: "outbox", title: "Outbox queued and claimed", status: "ok", subsystem: "execution", linked_id: "OBX-10001", timestamp: iso(6), elapsed_from_prev_ms: 410, anomaly_tags: [], summary: "Order persisted, claimed, and dispatch attempt created." },
    { node_key: "broker", node_type: "broker_event", title: "Broker ACK delayed", status: "warning", subsystem: "broker", linked_id: "ALP-9918122", timestamp: iso(5), elapsed_from_prev_ms: 2120, anomaly_tags: ["delayed_ack"], summary: "Broker accepted order but ACK exceeded warning SLA." },
    { node_key: "oms", node_type: "oms_transition", title: "OMS advanced to partially filled", status: "warning", subsystem: "execution", linked_id: "O-240308-001", timestamp: iso(3), elapsed_from_prev_ms: 118000, anomaly_tags: ["stuck_partial_fill"], summary: "OMS left open state but has remained partially filled beyond target dwell time." },
    { node_key: "portfolio", node_type: "portfolio_effect", title: "Portfolio updated from partial fills", status: "ok", subsystem: "portfolio", linked_id: "PF-2002", timestamp: iso(2), elapsed_from_prev_ms: 510, anomaly_tags: [], summary: "Position increased by 40 shares and cash decreased accordingly." },
    { node_key: "reconcile", node_type: "reconcile_outcome", title: "Reconcile flagged quantity drift", status: "warning", subsystem: "reconcile", linked_id: "RC-77", timestamp: iso(1), elapsed_from_prev_ms: 43000, anomaly_tags: ["open_qty_drift"], summary: "Broker open quantity differed from internal open quantity until next snapshot refresh." },
  ],
  timeline: [
    { row_id: "CT-1", timestamp: iso(8), subsystem: "strategy", event_type: "signal_emitted", correlation_id: "SIG-20260308-441", before_state: "idle", after_state: "signal_ready", latency_since_prev_ms: null, summary: "Signal triggered from breakout detector.", anomaly_tags: [] },
    { row_id: "CT-2", timestamp: iso(7), subsystem: "runtime", event_type: "intent_created", correlation_id: "INT-20260308-122", before_state: "signal_ready", after_state: "intent_created", latency_since_prev_ms: 830, summary: "Runtime materialized order intent for breakout strategy.", anomaly_tags: [] },
    { row_id: "CT-3", timestamp: iso(7), subsystem: "risk", event_type: "risk_pass", correlation_id: "DEC-20260308-440", before_state: "pending_risk", after_state: "approved", latency_since_prev_ms: 290, summary: "Intent cleared pre-trade risk checks.", anomaly_tags: [] },
    { row_id: "CT-4", timestamp: iso(6), subsystem: "execution", event_type: "outbox_claimed", correlation_id: "OBX-10001", before_state: "pending", after_state: "dispatching", latency_since_prev_ms: 410, summary: "Outbox row claimed with dispatch token CLM-8891.", anomaly_tags: [] },
    { row_id: "CT-5", timestamp: iso(5), subsystem: "broker", event_type: "broker_ack", correlation_id: "ALP-9918122", before_state: "sent", after_state: "acked", latency_since_prev_ms: 2120, summary: "Broker ACK arrived late relative to ACK SLA.", anomaly_tags: ["delayed_ack"] },
    { row_id: "CT-6", timestamp: iso(3), subsystem: "oms", event_type: "partial_fill_applied", correlation_id: "F-2", before_state: "open", after_state: "partially_filled", latency_since_prev_ms: 118000, summary: "Second partial fill advanced OMS into prolonged working state.", anomaly_tags: ["stuck_partial_fill"] },
    { row_id: "CT-7", timestamp: iso(2), subsystem: "portfolio", event_type: "position_update", correlation_id: "PF-2002", before_state: "qty=0", after_state: "qty=40", latency_since_prev_ms: 510, summary: "Portfolio layer applied cumulative fills.", anomaly_tags: [] },
    { row_id: "CT-8", timestamp: iso(1), subsystem: "reconcile", event_type: "drift_detected", correlation_id: "RC-77", before_state: "matched", after_state: "warning", latency_since_prev_ms: 43000, summary: "Snapshot compare found open-qty mismatch against broker state.", anomaly_tags: ["open_qty_drift"] },
  ],
  portfolio_effects: {
    pre_position_qty: 0,
    post_position_qty: 40,
    net_position_delta: 40,
    average_price_effect: 944.71,
    realized_pnl_delta: 0,
    unrealized_pnl_delta: 132.8,
    cash_delta: -37788.4,
    buying_power_delta: -37788.4,
    exposure_delta: 37925.6,
    strategy_allocation_effect: "breakout_momo allocation increased from 0.0% to 14.2% of active gross exposure.",
  },
  reconcile_outcome: {
    reconcile_status: "corrected",
    drift_detected: true,
    drift_type: "open_qty_mismatch",
    correction_applied: true,
    correction_timestamp: iso(0),
    corrected_fields: ["open_qty"],
    remaining_discrepancy: null,
    operator_escalation_status: "monitor_only",
  },
  anomalies: ["delayed_ack", "stuck_partial_fill", "open_qty_drift"],
};

export const MOCK_EXECUTION_REPLAY: ExecutionReplay = {
  replay_id: "RPL-001",
  replay_scope: "single_order",
  source: "full_correlated",
  title: "NVDA order lifecycle replay",
  current_frame_index: 5,
  frames: [
    { frame_id: "RF-1", timestamp: iso(8), subsystem: "strategy", event_type: "signal_generated", state_delta: "none -> intent_created", message_digest: "Breakout signal emitted", order_execution_state: "intent_created", oms_state: "open", filled_qty: 0, open_qty: 120, risk_state: "pending", reconcile_state: "clean", queue_status: "intent_queue=1", anomaly_tags: [], boundary_tags: [] },
    { frame_id: "RF-2", timestamp: iso(7), subsystem: "risk", event_type: "risk_pass", state_delta: "intent_created -> risk_passed", message_digest: "Sizing accepted", order_execution_state: "risk_passed", oms_state: "open", filled_qty: 0, open_qty: 120, risk_state: "pass", reconcile_state: "clean", queue_status: "outbox_pending=0", anomaly_tags: [], boundary_tags: [] },
    { frame_id: "RF-3", timestamp: iso(6), subsystem: "execution", event_type: "outbox_inserted", state_delta: "risk_passed -> pending", message_digest: "Outbox row written", order_execution_state: "pending", oms_state: "open", filled_qty: 0, open_qty: 120, risk_state: "pass", reconcile_state: "clean", queue_status: "outbox_pending=1", anomaly_tags: [], boundary_tags: [] },
    { frame_id: "RF-4", timestamp: iso(5), subsystem: "execution", event_type: "dispatch_claimed", state_delta: "pending -> claimed", message_digest: "Claim token issued", order_execution_state: "claimed", oms_state: "open", filled_qty: 0, open_qty: 120, risk_state: "pass", reconcile_state: "clean", queue_status: "dispatcher_busy", anomaly_tags: [], boundary_tags: [] },
    { frame_id: "RF-5", timestamp: iso(4), subsystem: "broker", event_type: "broker_ack", state_delta: "sent -> acked", message_digest: "ACK at 2120 ms", order_execution_state: "acked", oms_state: "open", filled_qty: 0, open_qty: 120, risk_state: "pass", reconcile_state: "clean", queue_status: "inbox_backlog=1", anomaly_tags: ["delayed_ack"], boundary_tags: [] },
    { frame_id: "RF-6", timestamp: iso(3), subsystem: "broker", event_type: "partial_fill", state_delta: "acked -> partial_fill_active", message_digest: "20 shares applied", order_execution_state: "partial_fill_active", oms_state: "partially_filled", filled_qty: 20, open_qty: 100, risk_state: "pass", reconcile_state: "clean", queue_status: "fills_pending=0", anomaly_tags: [], boundary_tags: [] },
    { frame_id: "RF-7", timestamp: iso(2), subsystem: "reconcile", event_type: "drift_marker", state_delta: "partial_fill_active -> partial_fill_active", message_digest: "Broker/runtime qty mismatch", order_execution_state: "partial_fill_active", oms_state: "partially_filled", filled_qty: 40, open_qty: 80, risk_state: "pass", reconcile_state: "warning", queue_status: "snapshot_refresh_pending", anomaly_tags: ["reconcile_mismatch"], boundary_tags: ["post_restart_boundary"] },
  ],
};

function metricSeries(key: string, label: string, unit: "count" | "ms" | "pct" | "rate" | "usd", values: number[], warn: number | null, crit: number | null) {
  return {
    key,
    label,
    unit,
    window: "15m" as const,
    current_value: values[values.length - 1],
    threshold_warning: warn,
    threshold_critical: crit,
    points: values.map((value, index) => ({ ts: iso(15 - index * 2), value })),
  };
}

export const MOCK_METRICS: SystemMetrics = {
  runtime: {
    key: "runtime",
    title: "Runtime Health",
    description: "Event queues, loop cadence, and service health.",
    series: [
      metricSeries("queue_depth", "Event Queue Depth", "count", [4, 6, 7, 5, 8, 9, 7, 6], 12, 20),
      metricSeries("outbox_backlog", "Outbox Backlog", "count", [1, 1, 2, 2, 1, 3, 2, 1], 5, 8),
      metricSeries("loop_latency", "Loop Latency", "ms", [122, 140, 166, 180, 175, 186, 190, 186], 250, 500),
    ],
  },
  execution: {
    key: "execution",
    title: "Execution Performance",
    description: "Transport and broker interaction telemetry.",
    series: [
      metricSeries("submit_to_ack", "Submit → Ack", "ms", [420, 390, 510, 620, 710, 480, 384, 2120], 1000, 2000),
      metricSeries("throughput", "Execution Throughput", "rate", [3, 4, 5, 4, 6, 7, 5, 4], 9, 12),
      metricSeries("active_orders", "Active Orders", "count", [4, 5, 5, 6, 7, 6, 7, 7], 10, 15),
    ],
  },
  fillQuality: {
    key: "fill_quality",
    title: "Fill / Outcome Quality",
    description: "Fill rates, partial fills, and rejections.",
    series: [
      metricSeries("fill_rate", "Fill Rate", "pct", [61, 64, 66, 63, 68, 71, 69, 67], 55, 45),
      metricSeries("partial_fill_ratio", "Partial Fill Ratio", "pct", [18, 17, 19, 21, 18, 19, 20, 22], 30, 40),
      metricSeries("rejection_rate", "Reject Rate", "pct", [1, 2, 1, 1, 2, 3, 2, 2], 4, 8),
    ],
  },
  reconciliation: {
    key: "reconciliation",
    title: "Reconciliation Pressure",
    description: "Drift counts and correction pressure.",
    series: [
      metricSeries("drift_cases", "Drift Cases", "count", [0, 1, 1, 2, 2, 1, 1, 1], 3, 6),
      metricSeries("unknown_orders", "Unknown Broker Orders", "count", [0, 0, 1, 1, 1, 0, 0, 0], 2, 4),
      metricSeries("corrections", "Corrections / Hour", "count", [0, 1, 0, 2, 1, 1, 0, 1], 3, 6),
    ],
  },
  riskSafety: {
    key: "risk_safety",
    title: "Risk / Safety",
    description: "Halts, suppressions, and risk rejects.",
    series: [
      metricSeries("risk_rejects", "Risk Rejects", "count", [0, 0, 1, 0, 0, 1, 0, 0], 2, 5),
      metricSeries("limit_utilization", "Loss Limit Utilization", "pct", [31, 33, 35, 36, 39, 41, 42, 41.8], 70, 90),
      metricSeries("operator_interventions", "Operator Interventions", "count", [0, 0, 0, 1, 1, 0, 0, 1], 3, 5),
    ],
  },
};

export const MOCK_PORTFOLIO_SUMMARY: PortfolioSummary = {
  account_equity: 151240.81,
  cash: 63220.12,
  long_market_value: 88020.69,
  short_market_value: 0,
  daily_pnl: 1840.22,
  buying_power: 210481.62,
};

export const MOCK_POSITIONS: PositionRow[] = [
  { symbol: "NVDA", strategy_id: "breakout_momo", qty: 40, avg_price: 944.82, mark_price: 948.14, unrealized_pnl: 132.8, realized_pnl_today: 0, broker_qty: 40, drift: false },
  { symbol: "AMD", strategy_id: "pullback_core", qty: 300, avg_price: 168.12, mark_price: 170.4, unrealized_pnl: 684, realized_pnl_today: 921.4, broker_qty: 300, drift: false },
  { symbol: "MSFT", strategy_id: "mean_revert", qty: 50, avg_price: 412.3, mark_price: 409.14, unrealized_pnl: -158, realized_pnl_today: -64.2, broker_qty: 50, drift: false },
];

export const MOCK_OPEN_ORDERS: OpenOrderRow[] = MOCK_EXECUTION_ORDERS.map((row) => ({
  internal_order_id: row.internal_order_id,
  symbol: row.symbol,
  strategy_id: row.strategy_id,
  side: row.side,
  status: row.current_status,
  broker_order_id: row.broker_order_id,
  requested_qty: row.requested_qty,
  filled_qty: row.filled_qty,
  entered_at: row.updated_at,
}));

export const MOCK_FILLS: FillRow[] = [
  { fill_id: "F-1", internal_order_id: "O-240308-001", symbol: "NVDA", strategy_id: "breakout_momo", side: "buy", qty: 20, price: 944.2, broker_exec_id: "X-8001", applied: true, at: iso(3) },
  { fill_id: "F-2", internal_order_id: "O-240308-001", symbol: "NVDA", strategy_id: "breakout_momo", side: "buy", qty: 20, price: 945.22, broker_exec_id: "X-8002", applied: true, at: iso(2) },
  { fill_id: "F-3", internal_order_id: "O-240308-002", symbol: "AMD", strategy_id: "pullback_core", side: "sell", qty: 80, price: 170.1, broker_exec_id: "X-8003", applied: true, at: iso(5) },
];

export const MOCK_RISK_SUMMARY: RiskSummary = {
  gross_exposure: 88020.69,
  net_exposure: 88020.69,
  concentration_pct: 43.4,
  daily_pnl: 1840.22,
  drawdown_pct: 1.82,
  loss_limit_utilization_pct: 41.8,
  kill_switch_active: false,
  active_breaches: 0,
};

export const MOCK_RISK_DENIALS: RiskDenialRow[] = [
  { id: "RD-1", at: iso(22), strategy_id: "mean_revert", symbol: "TSLA", rule: "max_symbol_exposure", message: "Order denied due to symbol concentration threshold.", severity: "warning" },
  { id: "RD-2", at: iso(41), strategy_id: "breakout_momo", symbol: "NVDA", rule: "opening_range_filter", message: "No entry after volatility guard trigger.", severity: "info" },
];

export const MOCK_RECONCILE_SUMMARY: ReconcileSummary = {
  status: "warning",
  last_run_at: iso(1),
  mismatched_positions: 0,
  mismatched_orders: 1,
  mismatched_fills: 0,
  unmatched_broker_events: 1,
};

export const MOCK_MISMATCHES: ReconcileMismatchRow[] = [
  { id: "MM-1", domain: "order", symbol: "NVDA", internal_value: "open_qty=80", broker_value: "open_qty=60", status: "warning", note: "Expected to resolve after snapshot refresh." },
  { id: "MM-2", domain: "event", symbol: "AVGO", internal_value: "no broker id", broker_value: "pending edge submission", status: "critical", note: "Dispatching order has not received broker acknowledgement." },
];

export const MOCK_STRATEGIES: StrategyRow[] = [
  { strategy_id: "breakout_momo", enabled: true, armed: true, health: "ok", universe: "large-cap momentum", pending_intents: 1, open_positions: 1, today_pnl: 801.1, drawdown_pct: 0.8, regime: "trend", throttle_state: "normal", last_decision_time: iso(8) },
  { strategy_id: "pullback_core", enabled: true, armed: true, health: "ok", universe: "semis", pending_intents: 0, open_positions: 1, today_pnl: 1120.3, drawdown_pct: 0.4, regime: "mean reversion", throttle_state: "normal", last_decision_time: iso(4) },
  { strategy_id: "mean_revert", enabled: true, armed: true, health: "warning", universe: "broad tech", pending_intents: 1, open_positions: 1, today_pnl: -81.2, drawdown_pct: 1.9, regime: "chop", throttle_state: "throttled", last_decision_time: iso(12) },
];

export const MOCK_AUDIT_ACTIONS: AuditActionRow[] = [
  { audit_ref: "AUD-001", at: iso(55), actor: "operator@desk", action_key: "refresh-broker-snapshot", environment: "paper", target_scope: "reconcile", result_state: "accepted", warnings: [] },
  { audit_ref: "AUD-002", at: iso(71), actor: "operator@desk", action_key: "reconcile-now", environment: "paper", target_scope: "ops", result_state: "accepted", warnings: [] },
];

export const MOCK_METADATA: MetadataSummary = {
  build_version: "mqd-gui-v2.1-scaffold",
  api_version: "v1",
  broker_adapter: "alpaca-paper",
  endpoint_status: "ok",
};

export const MOCK_ACTION_CATALOG: OperatorActionDefinition[] = [
  { action_key: "start-system", label: "Start runtime", level: 1, description: "Start the daemon-managed runtime loop.", requiresReason: false, confirmText: "Start the runtime?", disabled: false },
  { action_key: "stop-system", label: "Stop runtime", level: 2, description: "Stop new processing and drain safely.", requiresReason: true, confirmText: "Stop the runtime?", disabled: false },
  { action_key: "arm-strategy", label: "Arm strategies", level: 1, description: "Permit strategies to emit intents.", requiresReason: false, confirmText: "Arm strategies?", disabled: false },
  { action_key: "disarm-strategy", label: "Disarm strategies", level: 2, description: "Stop new strategy intents immediately.", requiresReason: true, confirmText: "Disarm strategies?", disabled: false },
  { action_key: "arm-execution", label: "Arm execution", level: 2, description: "Permit daemon to submit approved orders.", requiresReason: true, confirmText: "Arm execution?", disabled: false },
  { action_key: "disarm-execution", label: "Disarm execution", level: 2, description: "Block further outbound execution actions.", requiresReason: true, confirmText: "Disarm execution?", disabled: false },
  { action_key: "enable-live-routing", label: "Enable live routing", level: 3, description: "Allow live-capital routing only when environment is live.", requiresReason: true, confirmText: "Enable live routing?", disabled: false },
  { action_key: "disable-live-routing", label: "Disable live routing", level: 3, description: "Immediately prevent new live routing.", requiresReason: true, confirmText: "Disable live routing?", disabled: false },
  { action_key: "pause-new-entries", label: "Pause new entries", level: 1, description: "Hold new entries while keeping exits available.", requiresReason: false, confirmText: "Pause new entries?", disabled: false },
  { action_key: "resume-new-entries", label: "Resume new entries", level: 1, description: "Re-enable entries after operator review.", requiresReason: false, confirmText: "Resume new entries?", disabled: false },
  { action_key: "reconcile-now", label: "Run reconcile now", level: 1, description: "Trigger an immediate reconcile cycle.", requiresReason: false, confirmText: "Run reconcile now?", disabled: false },
  { action_key: "refresh-broker-snapshot", label: "Refresh broker snapshot", level: 1, description: "Pull latest broker state for diagnostics.", requiresReason: false, confirmText: "Refresh broker snapshot?", disabled: false },
  { action_key: "cancel-all-open-orders", label: "Cancel all open orders", level: 3, description: "Issue cancel-all through daemon safeguards.", requiresReason: true, confirmText: "Cancel all open orders?", disabled: false },
  { action_key: "flatten-all", label: "Flatten all positions", level: 3, description: "Emergency flatten through guarded daemon path.", requiresReason: true, confirmText: "Flatten all positions?", disabled: false },
  { action_key: "kill-switch", label: "Kill switch", level: 3, description: "Global hard stop.", requiresReason: true, confirmText: "Trigger kill switch?", disabled: false },
  { action_key: "resume-after-halt", label: "Resume after halt", level: 2, description: "Resume after operator verification.", requiresReason: true, confirmText: "Resume after halt?", disabled: false },
  { action_key: "ack-alert", label: "Acknowledge alert", level: 0, description: "Mark alert acknowledged.", requiresReason: false, confirmText: "Acknowledge alert?", disabled: false },
  { action_key: "change-system-mode", label: "Change system mode", level: 3, description: "Perform a controlled mode transition with daemon restart and config reload.", requiresReason: true, confirmText: "Change system mode?", disabled: false },
];



export const MOCK_TOPOLOGY: ServiceTopology = {
  updated_at: iso(0),
  services: [
    { service_key: "daemon", label: "Daemon API", layer: "runtime", health: "ok", role: "Command and read-model surface", dependency_keys: ["postgres", "runtime"], failure_impact: "GUI becomes read-only/disconnected.", last_heartbeat: iso(0), latency_ms: 42, notes: "Primary entrypoint for operator console." },
    { service_key: "runtime", label: "Runtime Loop", layer: "runtime", health: "ok", role: "Deterministic event loop", dependency_keys: ["postgres", "broker_adapter", "risk"], failure_impact: "No new processing or heartbeat progression.", last_heartbeat: iso(0), latency_ms: 186, notes: "Leader lease held and progressing normally." },
    { service_key: "broker_adapter", label: "Broker Adapter", layer: "broker", health: "warning", role: "Submit/ack/fill bridge", dependency_keys: ["runtime"], failure_impact: "Orders can stall pre/post ACK.", last_heartbeat: iso(1), latency_ms: 2120, notes: "ACK latency elevated but service responsive." },
    { service_key: "postgres", label: "Postgres", layer: "data", health: "ok", role: "Canonical persistence", dependency_keys: [], failure_impact: "System cannot persist or prove state.", last_heartbeat: iso(0), latency_ms: 18, notes: "Primary DB healthy." },
    { service_key: "reconcile", label: "Reconcile Worker", layer: "reconcile", health: "warning", role: "Broker/internal drift detection", dependency_keys: ["postgres", "broker_adapter"], failure_impact: "Drift can accumulate unresolved.", last_heartbeat: iso(1), latency_ms: 311, notes: "One active mismatch case." },
    { service_key: "audit", label: "Audit Writer", layer: "audit", health: "ok", role: "Immutable event evidence", dependency_keys: ["postgres"], failure_impact: "Operator actions lose forensic traceability.", last_heartbeat: iso(0), latency_ms: 26, notes: "Writes clean." },
    { service_key: "strategy", label: "Strategy Runtime", layer: "strategy", health: "ok", role: "Signal and intent generation", dependency_keys: ["runtime", "risk"], failure_impact: "No new intents or bad signal silence.", last_heartbeat: iso(0), latency_ms: 71, notes: "3 strategy engines armed." },
    { service_key: "risk", label: "Risk Gate", layer: "risk", health: "ok", role: "Pre-trade and safety checks", dependency_keys: ["postgres", "runtime"], failure_impact: "Unsafe orders could bypass checks or all orders block.", last_heartbeat: iso(0), latency_ms: 33, notes: "No active breaches." },
  ],
};

export const MOCK_TRANSPORT: TransportSummary = {
  outbox_depth: 3,
  inbox_depth: 4,
  max_claim_age_ms: 361000,
  dispatch_retries: 2,
  orphaned_claims: 1,
  duplicate_inbox_events: 1,
  queues: [
    { queue_id: "outbox-main", direction: "outbox", status: "degraded", depth: 3, oldest_age_ms: 361000, retry_count: 2, duplicate_events: 0, orphaned_claims: 1, lag_ms: 780, last_activity_at: iso(0), notes: "One claim token older than target window." },
    { queue_id: "inbox-broker", direction: "inbox", status: "warning", depth: 4, oldest_age_ms: 128000, retry_count: 0, duplicate_events: 1, orphaned_claims: 0, lag_ms: 640, last_activity_at: iso(0), notes: "Duplicate fill event deduped once." },
  ],
};

export const MOCK_INCIDENTS: IncidentCase[] = [
  { incident_id: "INC-240308-OMS-01", severity: "warning", title: "NVDA partial fill linger", status: "investigating", opened_at: iso(4), updated_at: iso(1), impacted_orders: ["O-240308-001"], impacted_strategies: ["breakout_momo"], impacted_subsystems: ["broker_adapter", "reconcile"], alerts: ["a1"], reconcile_case_ids: ["RC-77"], operator_actions_taken: ["refresh-broker-snapshot", "reconcile-now"], final_disposition: "Pending broker snapshot convergence." },
  { incident_id: "INC-240308-DSP-02", severity: "critical", title: "AVGO dispatch claim aging", status: "contained", opened_at: iso(9), updated_at: iso(2), impacted_orders: ["O-240308-003"], impacted_strategies: ["mean_revert"], impacted_subsystems: ["runtime", "broker_adapter"], alerts: ["transport-claim"], reconcile_case_ids: [], operator_actions_taken: ["pause-new-entries"], final_disposition: "Entries paused while claim token investigated." },
];

export const MOCK_REPLACE_CANCEL_CHAINS: ReplaceCancelChainRow[] = [
  { chain_id: "CHAIN-901", root_order_id: "O-240308-001", current_order_id: "O-240308-001-R1", broker_order_id: "ALP-9918122", symbol: "NVDA", strategy_id: "breakout_momo", action_type: "replace", status: "awaiting_broker_ack", request_at: iso(2), ack_at: null, target_order_id: "O-240308-001", notes: "Price ladder tightened after first fill; ack still pending." },
  { chain_id: "CHAIN-777", root_order_id: "O-240308-004", current_order_id: "O-240308-004", broker_order_id: "ALP-9918201", symbol: "META", strategy_id: "pullback_core", action_type: "cancel", status: "broker_cancelled", request_at: iso(14), ack_at: iso(13), target_order_id: "O-240308-004", notes: "Cancel completed within SLA." },
];

export const MOCK_ALERT_TRIAGE: AlertTriageRow[] = [
  { alert_id: "a1", severity: "warning", status: "unacked", title: "Reconcile mismatch detected", domain: "reconcile", linked_incident_id: "INC-240308-OMS-01", linked_order_id: "O-240308-001", linked_strategy_id: "breakout_momo", created_at: iso(3), assigned_to: null },
  { alert_id: "a2", severity: "warning", status: "acked", title: "Broker ACK latency elevated", domain: "execution", linked_incident_id: "INC-240308-OMS-01", linked_order_id: "O-240308-001", linked_strategy_id: "breakout_momo", created_at: iso(5), assigned_to: "operator@desk" },
  { alert_id: "a3", severity: "critical", status: "escalated", title: "Dispatch claim age exceeded hard limit", domain: "transport", linked_incident_id: "INC-240308-DSP-02", linked_order_id: "O-240308-003", linked_strategy_id: "mean_revert", created_at: iso(7), assigned_to: "ops_lead" },
];

export const MOCK_SESSION_STATE: SessionStateSummary = {
  market_session: "regular",
  exchange_calendar_state: "open",
  system_trading_window: "enabled",
  strategy_allowed: true,
  next_session_change_at: iso(-383),
  notes: ["Regular session active.", "Live routing disabled at environment layer.", "Exit-only override not engaged."],
};

export const MOCK_CONFIG_FINGERPRINT: ConfigFingerprintSummary = {
  config_hash: "cfg_8d1e9f33",
  risk_policy_version: "risk-policy-2026.03.08-a",
  strategy_bundle_version: "bundle-17",
  build_version: "mqd-gui-v4-scaffold",
  environment_profile: "windows-dev.paper.main",
  runtime_generation_id: "rtgen-20260308-01",
  last_restart_at: iso(62),
};


export const MOCK_MARKET_DATA_QUALITY: MarketDataQualitySummary = {
  overall_health: "warning",
  freshness_sla_ms: 1500,
  stale_symbol_count: 2,
  missing_bar_count: 1,
  venue_disagreement_count: 1,
  strategy_blocks: 1,
  venues: [
    { venue_key: "primary_quotes", label: "Primary Quotes", health: "warning", freshness_lag_ms: 2310, symbols_degraded: 2, missing_updates: 0, disagreement_count: 1, last_good_at: iso(1), note: "AMD and AVGO exceeded quote freshness soft limit." },
    { venue_key: "bars_1m", label: "1m Bars", health: "warning", freshness_lag_ms: 4120, symbols_degraded: 1, missing_updates: 1, disagreement_count: 0, last_good_at: iso(2), note: "One missing 1m bar pending backfill." },
    { venue_key: "broker_snapshots", label: "Broker Snapshots", health: "ok", freshness_lag_ms: 420, symbols_degraded: 0, missing_updates: 0, disagreement_count: 0, last_good_at: iso(0), note: "Snapshot cadence normal." },
  ],
  issues: [
    { issue_id: "MD-1", severity: "warning", scope: "symbol", symbol: "AMD", venue: "primary_quotes", issue_type: "stale_quote", freshness_lag_ms: 2310, affected_strategies: ["pullback_core"], status: "open", note: "Freshness above soft threshold; strategy remains allowed.", detected_at: iso(4) },
    { issue_id: "MD-2", severity: "critical", scope: "symbol", symbol: "AVGO", venue: "bars_1m", issue_type: "missing_bar", freshness_lag_ms: 4120, affected_strategies: ["mean_revert"], status: "blocked", note: "Mean reversion entries suppressed until bar continuity restored.", detected_at: iso(6) },
    { issue_id: "MD-3", severity: "warning", scope: "venue", symbol: null, venue: "primary_quotes", issue_type: "venue_disagreement", freshness_lag_ms: null, affected_strategies: ["breakout_momo"], status: "monitoring", note: "Quote source divergence observed on NVDA spread snapshot.", detected_at: iso(5) },
  ],
};

export const MOCK_RUNTIME_LEADERSHIP: RuntimeLeadershipSummary = {
  leader_node: "runtime-node-a",
  leader_lease_state: "held",
  generation_id: "rtgen-20260308-01",
  restart_count_24h: 1,
  last_restart_at: iso(62),
  post_restart_recovery_state: "complete",
  recovery_checkpoint: "Inbox catchup complete; reconcile watermark aligned.",
  checkpoints: [
    { checkpoint_id: "RT-1", checkpoint_type: "restart", timestamp: iso(62), generation_id: "rtgen-20260308-01", leader_node: "runtime-node-a", status: "warning", note: "Operator-triggered controlled restart for config reload." },
    { checkpoint_id: "RT-2", checkpoint_type: "leader_acquired", timestamp: iso(61), generation_id: "rtgen-20260308-01", leader_node: "runtime-node-a", status: "ok", note: "Leader lease acquired cleanly after restart." },
    { checkpoint_id: "RT-3", checkpoint_type: "snapshot_refresh", timestamp: iso(59), generation_id: "rtgen-20260308-01", leader_node: "runtime-node-a", status: "ok", note: "Broker snapshot refresh completed before execution resumed." },
    { checkpoint_id: "RT-4", checkpoint_type: "recovery_complete", timestamp: iso(57), generation_id: "rtgen-20260308-01", leader_node: "runtime-node-a", status: "ok", note: "Replay catchup and reconcile watermark recovered." },
  ],
};

export const MOCK_ARTIFACT_REGISTRY: ArtifactRegistrySummary = {
  last_updated_at: iso(0),
  ready_count: 4,
  pending_count: 1,
  failed_count: 0,
  artifacts: [
    { artifact_id: "ART-001", artifact_type: "incident_bundle", created_at: iso(8), status: "ready", linked_order_id: "O-240308-001", linked_incident_id: "INC-240308-OMS-01", linked_run_id: "RUN-20260308-01", storage_path: "artifacts/incidents/INC-240308-OMS-01.zip", note: "Bundle includes trace, replay, and reconcile notes." },
    { artifact_id: "ART-002", artifact_type: "trace_export", created_at: iso(7), status: "ready", linked_order_id: "O-240308-001", linked_incident_id: null, linked_run_id: "RUN-20260308-01", storage_path: "artifacts/traces/O-240308-001.json", note: "Execution trace export generated from audit surface." },
    { artifact_id: "ART-003", artifact_type: "reconcile_report", created_at: iso(5), status: "ready", linked_order_id: null, linked_incident_id: "INC-240308-OMS-01", linked_run_id: "RUN-20260308-01", storage_path: "artifacts/reconcile/RC-77.md", note: "Drift review report for partial fill mismatch." },
    { artifact_id: "ART-004", artifact_type: "operator_receipt", created_at: iso(55), status: "ready", linked_order_id: null, linked_incident_id: null, linked_run_id: "RUN-20260308-01", storage_path: "artifacts/ops/AUD-001.json", note: "Operator receipt for broker snapshot refresh." },
    { artifact_id: "ART-005", artifact_type: "replay_export", created_at: iso(3), status: "pending", linked_order_id: "O-240308-003", linked_incident_id: "INC-240308-DSP-02", linked_run_id: "RUN-20260308-01", storage_path: "artifacts/replay/O-240308-003.rpl", note: "Replay export queued after dispatch-aging incident." },
  ],
};

export const MOCK_STRATEGY_SUPPRESSIONS: StrategySuppressionRow[] = [
  { suppression_id: "SUP-001", strategy_id: "mean_revert", state: "active", trigger_domain: "market_data", trigger_reason: "AVGO 1m bar continuity broken", started_at: iso(6), cleared_at: null, note: "New entries blocked until missing bar repaired." },
  { suppression_id: "SUP-002", strategy_id: "breakout_momo", state: "cleared", trigger_domain: "operator", trigger_reason: "Operator paused entries during broker latency spike", started_at: iso(92), cleared_at: iso(70), note: "Cleared after transport latency normalized." },
];

export const MOCK_CONFIG_DIFFS: ConfigDiffRow[] = [
  { diff_id: "CFG-1", changed_at: iso(62), changed_domain: "runtime", before_version: "rtgen-20260307-03", after_version: "rtgen-20260308-01", summary: "Controlled restart generated new runtime generation id." },
  { diff_id: "CFG-2", changed_at: iso(63), changed_domain: "risk", before_version: "risk-policy-2026.03.07-b", after_version: "risk-policy-2026.03.08-a", summary: "Updated stale-data block policy for mean reversion entries." },
];



export const MOCK_OPERATOR_TIMELINE: OperatorTimelineEvent[] = [
  { timeline_event_id: "OTL-001", at: iso(68), category: "config_change", severity: "warning", title: "Risk policy updated", summary: "Stale-data block policy changed before controlled runtime restart.", actor: "operator@desk", linked_incident_id: null, linked_order_id: null, linked_strategy_id: "mean_revert", linked_action_key: null, linked_config_diff_id: "CFG-2", linked_runtime_generation_id: "rtgen-20260307-03" },
  { timeline_event_id: "OTL-002", at: iso(62), category: "operator_action", severity: "warning", title: "Controlled restart requested", summary: "Operator initiated restart to reload runtime and policy configuration.", actor: "operator@desk", linked_incident_id: null, linked_order_id: null, linked_strategy_id: null, linked_action_key: "stop-system", linked_config_diff_id: null, linked_runtime_generation_id: "rtgen-20260307-03" },
  { timeline_event_id: "OTL-003", at: iso(61), category: "runtime_restart", severity: "warning", title: "Runtime generation advanced", summary: "Runtime restarted cleanly and leader lease reacquired on runtime-node-a.", actor: "mqk-runtime", linked_incident_id: null, linked_order_id: null, linked_strategy_id: null, linked_action_key: null, linked_config_diff_id: "CFG-1", linked_runtime_generation_id: "rtgen-20260308-01" },
  { timeline_event_id: "OTL-004", at: iso(7), category: "alert", severity: "critical", title: "Dispatch claim age exceeded", summary: "Transport monitor raised a hard alert for outbox claim aging on AVGO.", actor: "mqk-transport", linked_incident_id: "INC-240308-DSP-02", linked_order_id: "O-240308-003", linked_strategy_id: "mean_revert", linked_action_key: null, linked_config_diff_id: null, linked_runtime_generation_id: "rtgen-20260308-01" },
  { timeline_event_id: "OTL-005", at: iso(6), category: "incident", severity: "critical", title: "Incident workspace opened", summary: "Incident INC-240308-DSP-02 opened and linked to transport, execution, and mean_revert strategy context.", actor: "operator@desk", linked_incident_id: "INC-240308-DSP-02", linked_order_id: "O-240308-003", linked_strategy_id: "mean_revert", linked_action_key: null, linked_config_diff_id: null, linked_runtime_generation_id: "rtgen-20260308-01" },
  { timeline_event_id: "OTL-006", at: iso(5), category: "operator_action", severity: "warning", title: "Entries paused", summary: "Operator paused new entries to contain transport risk while claim token aging was investigated.", actor: "operator@desk", linked_incident_id: "INC-240308-DSP-02", linked_order_id: null, linked_strategy_id: "mean_revert", linked_action_key: "pause-new-entries", linked_config_diff_id: null, linked_runtime_generation_id: "rtgen-20260308-01" },
  { timeline_event_id: "OTL-007", at: iso(3), category: "reconcile", severity: "warning", title: "Reconcile case linked", summary: "Reconcile case RC-77 attached to NVDA partial-fill linger for downstream review.", actor: "mqk-reconcile", linked_incident_id: "INC-240308-OMS-01", linked_order_id: "O-240308-001", linked_strategy_id: "breakout_momo", linked_action_key: "reconcile-now", linked_config_diff_id: null, linked_runtime_generation_id: "rtgen-20260308-01" },
  { timeline_event_id: "OTL-008", at: iso(1), category: "operator_action", severity: "info", title: "Broker snapshot refresh", summary: "Operator requested fresh broker state before deciding whether to resume entries.", actor: "operator@desk", linked_incident_id: "INC-240308-OMS-01", linked_order_id: "O-240308-001", linked_strategy_id: "breakout_momo", linked_action_key: "refresh-broker-snapshot", linked_config_diff_id: null, linked_runtime_generation_id: "rtgen-20260308-01" },
];



export const MOCK_EXECUTION_CHART: ExecutionChartModel = {
  order_id: "O-240308-001",
  symbol: "NVDA",
  timeframe: "1m",
  reference_price: 944.05,
  bars: [
    { ts: iso(12), open: 942.8, high: 943.6, low: 942.3, close: 943.2, volume: 21000 },
    { ts: iso(11), open: 943.2, high: 943.9, low: 942.9, close: 943.6, volume: 18200 },
    { ts: iso(10), open: 943.6, high: 944.1, low: 943.1, close: 943.9, volume: 19600 },
    { ts: iso(9), open: 943.9, high: 944.4, low: 943.4, close: 944.2, volume: 22500 },
    { ts: iso(8), open: 944.2, high: 944.9, low: 943.8, close: 944.7, volume: 30800 },
    { ts: iso(7), open: 944.7, high: 945.1, low: 944.2, close: 944.9, volume: 27400 },
    { ts: iso(6), open: 944.9, high: 945.3, low: 944.4, close: 945.0, volume: 29100 },
    { ts: iso(5), open: 945.0, high: 945.2, low: 944.5, close: 944.8, volume: 33200 },
    { ts: iso(4), open: 944.8, high: 945.0, low: 944.1, close: 944.3, volume: 36100 },
    { ts: iso(3), open: 944.3, high: 944.6, low: 943.9, close: 944.2, volume: 34700 },
    { ts: iso(2), open: 944.2, high: 945.4, low: 944.0, close: 945.2, volume: 40200 },
    { ts: iso(1), open: 945.2, high: 945.8, low: 944.9, close: 945.4, volume: 31800 },
    { ts: iso(0), open: 945.4, high: 945.9, low: 945.0, close: 945.6, volume: 30100 },
  ],
  overlays: [
    { overlay_id: "OV-1", kind: "signal", ts: iso(8), price: 944.62, label: "Signal", detail: "Breakout signal fired above opening range.", severity: "info", linked_frame_id: "RF-1" },
    { overlay_id: "OV-2", kind: "intent", ts: iso(7), price: 944.88, label: "Intent", detail: "Intent sized for 120 shares.", severity: "info", linked_frame_id: "RF-1" },
    { overlay_id: "OV-3", kind: "risk_pass", ts: iso(7), price: 944.95, label: "Risk pass", detail: "Loss and concentration checks passed.", severity: "info", linked_frame_id: "RF-1" },
    { overlay_id: "OV-4", kind: "expected_price", ts: iso(6), price: 945.00, label: "Ref", detail: "Expected decision price at dispatch.", severity: "info", linked_frame_id: "RF-2" },
    { overlay_id: "OV-5", kind: "order_sent", ts: iso(5), price: 944.92, label: "Sent", detail: "Order left outbox and was posted to broker edge.", severity: "info", linked_frame_id: "RF-2" },
    { overlay_id: "OV-6", kind: "broker_ack", ts: iso(4), price: 944.58, label: "ACK", detail: "Broker ACK arrived beyond soft threshold.", severity: "warning", linked_frame_id: "RF-3" },
    { overlay_id: "OV-7", kind: "partial_fill", ts: iso(3), price: 944.20, label: "Fill 20", detail: "First execution. Taker fill 20 @ 944.20.", severity: "info", linked_frame_id: "RF-4" },
    { overlay_id: "OV-8", kind: "partial_fill", ts: iso(2), price: 945.22, label: "Fill 20", detail: "Second execution. Cumulative filled 40.", severity: "info", linked_frame_id: "RF-5" },
    { overlay_id: "OV-9", kind: "portfolio", ts: iso(2), price: 945.22, label: "Portfolio", detail: "Position advanced to +40 shares.", severity: "info", linked_frame_id: "RF-5" },
    { overlay_id: "OV-10", kind: "reconcile", ts: iso(1), price: 945.35, label: "Reconcile", detail: "Open-qty drift flagged pending broker refresh.", severity: "warning", linked_frame_id: "RF-6" },
  ],
};

export const MOCK_MODEL: SystemModel = {
  status: MOCK_STATUS,
  preflight: MOCK_PREFLIGHT,
  alerts: MOCK_ALERTS,
  feed: MOCK_FEED,
  executionSummary: MOCK_EXECUTION_SUMMARY,
  executionOrders: MOCK_EXECUTION_ORDERS,
  selectedTimeline: MOCK_TIMELINE,
  omsOverview: MOCK_OMS_OVERVIEW,
  executionTrace: MOCK_EXECUTION_TRACE,
  executionReplay: MOCK_EXECUTION_REPLAY,
  executionChart: MOCK_EXECUTION_CHART,
  causalityTrace: MOCK_CAUSALITY_TRACE,
  metrics: MOCK_METRICS,
  portfolioSummary: MOCK_PORTFOLIO_SUMMARY,
  positions: MOCK_POSITIONS,
  openOrders: MOCK_OPEN_ORDERS,
  fills: MOCK_FILLS,
  riskSummary: MOCK_RISK_SUMMARY,
  riskDenials: MOCK_RISK_DENIALS,
  reconcileSummary: MOCK_RECONCILE_SUMMARY,
  mismatches: MOCK_MISMATCHES,
  strategies: MOCK_STRATEGIES,
  auditActions: MOCK_AUDIT_ACTIONS,
  metadata: MOCK_METADATA,
  topology: MOCK_TOPOLOGY,
  transport: MOCK_TRANSPORT,
  incidents: MOCK_INCIDENTS,
  replaceCancelChains: MOCK_REPLACE_CANCEL_CHAINS,
  alertTriage: MOCK_ALERT_TRIAGE,
  sessionState: MOCK_SESSION_STATE,
  configFingerprint: MOCK_CONFIG_FINGERPRINT,
  marketDataQuality: MOCK_MARKET_DATA_QUALITY,
  runtimeLeadership: MOCK_RUNTIME_LEADERSHIP,
  artifactRegistry: MOCK_ARTIFACT_REGISTRY,
  strategySuppressions: MOCK_STRATEGY_SUPPRESSIONS,
  configDiffs: MOCK_CONFIG_DIFFS,
  operatorTimeline: MOCK_OPERATOR_TIMELINE,
  actionCatalog: MOCK_ACTION_CATALOG,
  dataSource: {
    state: "mock",
    reachable: true,
    realEndpoints: [],
    missingEndpoints: [],
    mockSections: ["all"],
    message: "Mock fallback model active",
  },
  panelSources: classifyPanelSources({
    state: "mock",
    reachable: true,
    realEndpoints: [],
    missingEndpoints: [],
    mockSections: ["all"],
    message: "Mock fallback model active",
  }, true),
  connected: true,
  lastUpdatedAt: new Date().toISOString(),
};
