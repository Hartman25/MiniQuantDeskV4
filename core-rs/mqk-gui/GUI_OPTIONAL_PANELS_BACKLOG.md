# MiniQuantDesk GUI Optional Panels Backlog

This file separates **required monitoring surfaces already added to the scaffold** from items that are useful later if you want more screen space or more specialized workflows.

## Added as required monitoring panels

These are now in the scaffold because they are needed to correctly monitor the system end to end:

- Topology
- Runtime
- Market Data
- Transport
- Execution
- Incidents
- Alerts
- Risk
- Portfolio
- Reconcile
- Strategy
- Logs / Audit
- Artifacts
- Session
- Config
- Operator Actions
- Settings / Ops

These cover the minimum institutional monitoring loop:

- service dependency health
- leadership / restart boundaries
- market-data quality and strategy blocking conditions
- outbox / inbox transport health
- OMS state supervision
- deep execution trace / replay / causality
- incident grouping and alert triage
- risk posture and halts
- portfolio and fill effects
- reconciliation drift
- audit evidence and exported artifacts
- market session and policy fingerprint visibility

## Optional / nice-to-have later

These are worth adding later only if you have space, or if the live system proves you need more specialization.

### 1. Dedicated policy diff explorer
Use when you want more than the compact config-diff table already in Config.

Would add:
- full before/after policy blobs
- hash lineage
- changed keys only view
- operator approval linkage

### 2. Cross-incident comparison board
Useful once you have enough incident history to compare recurring patterns.

Would add:
- incident clustering
- repeated subsystem failure fingerprints
- recurring order symbols / strategies
- MTTR comparison

### 3. Strategy decision trace viewer
Only needed once strategy logic grows more complex and you want full upstream decision introspection.

Would add:
- feature snapshot lineage
- score/rank decomposition
- suppression decision path
- reason codes for skipped intents

### 4. Capacity / saturation dashboard
Useful later if runtime load or horizontal scaling becomes real.

Would add:
- queue growth versus throughput
- CPU / memory by service
- DB saturation proxies
- backlog growth rate

### 5. Operator handoff board
Useful when more than one operator is using the console.

Would add:
- shift notes
- active owner by incident
- pending reviews
- unresolved alerts by assignee

## Rule for future additions

Do not add a panel just because it sounds institutional.

Add a panel only if one of these is true:

1. it closes a real blind spot in monitoring,
2. it shortens incident diagnosis materially,
3. it proves a state transition or recovery boundary that currently cannot be seen,
4. or it prevents operators from misreading system health.
