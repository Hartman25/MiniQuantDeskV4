# MiniQuantDesk Future Backlog (Brainstorm)

**Purpose:**
This document captures future feature ideas, workflow upgrades, and research concepts discussed during brainstorming. These are **NOT current execution priorities**.

**Current priority remains:**
1. Fully harden autonomous paper execution
2. Prove paper execution end-to-end
3. Preserve fail-closed behavior
4. Transition to live only after proof is complete

---

# 1. Trade Thesis Journal Layer

## Goal
Store *why* every trade was entered/exited.

Example:

```json
{
  "trade_id": "...",
  "strategy": "ABCD_breakout",
  "entry_reason": "VWAP reclaim + volume expansion",
  "exit_reason": "target hit",
  "regime": "high_vol_trend",
  "confidence_score": 0.81
}
```

## Why it matters
Current system tracks execution truth well.
This adds decision attribution.

## Future Patch
`JOURNAL-THESIS-01`

---

# 2. Weekly Automated Trade Reviews

## Goal
Automate post-trade review.

Metrics:
- win rate by strategy
- win rate by time of day
- win rate by regime
- expectancy
- MAE
- MFE
- top losses
- stop-out reasons

## Future Patch
`TRADE-REVIEW-01`

---

# 3. Time-of-Day Filters

## Goal
Reduce low-quality opening trades.

Examples:
- no trades first 15–30 minutes
- reduced sizing during opening volatility

## Future Patch
`STRAT-TIME-FILTER-01`

---

# 4. Stop-Loss Immutability

## Goal
Prevent widening risk after entry.

Allowed:
- unchanged stop
- tighter stop
- reduced exposure

Forbidden:
- wider stop

## Future Patch
`RISK-STOP-IMMUTABILITY-01`

---

# 5. MAE / MFE Analytics

Track:
- Maximum Adverse Excursion
- Maximum Favorable Excursion

## Future Patch
`ANALYTICS-MAE-MFE-01`

---

# 6. Regime Attribution

Track performance by:
- trending
- ranging
- high volatility
- low volatility
- panic
- accumulation

## Future Patch
`REGIME-01`

---

# 7. Strategy Decay Detection

Detect when strategy edge deteriorates.

Actions:
- reduce size
- disable strategy
- alert operator

## Future Patch
`DECAY-01`

---

# 8. AI Trade Forensics

Use AI AFTER trades.

Examples:
- why did this lose?
- what changed?
- pattern breakdown

**AI should not authorize live trades.**

## Future Patch
`FORENSICS-AI-01`

---

# 9. Multi-Agent Signal Council (Research Only)

Inspired by BTC council concept.

Potential components:
- regime classifier
- specialist signal scoring
- weighted verdict engine
- regime-specific exits

Do NOT allow LLMs to directly execute trades.

## Future Patches
- `ML-REGIME-01`
- `ML-SIGNAL-01`
- `ML-COUNCIL-01`

---

# 10. LLM Trade Explanation Layer

Optional layer explaining:
- why trade happened
- signal alignment
- regime state
- risk posture

Explanation only.

---

# 11. Codex as Independent Verifier

Current workflow:

```
ChatGPT → PM
Claude → coder
ChatGPT → reviewer
```

Future workflow:

```
ChatGPT → architect
Claude → implementation
Codex → verification
```

Use Codex for:
- repo audits
- diff validation
- test verification
- failure analysis

---

# 12. Agent Flow Observability

GitHub:
https://github.com/patoles/agent-flow

Purpose:
Visualize:
- Claude actions
- Codex actions
- tool calls
- reasoning flow
- branching
- timing

---

# 13. Full AI Development Stack

Potential workflow:

```
ChatGPT = architecture
Claude = implementation
Codex = verification
Agent Flow = observability
```

---

# 14. MiniQuantDesk Internal Flow Visualization

Build an internal version of Agent Flow for trading operations.

Visualize:

```
market data
→ signal
→ risk
→ order intent
→ broker
→ fill
→ OMS
→ portfolio
→ reconcile
→ alerts
```

## Future Patches
- `FLOW-01`
- `FLOW-02`
- `FLOW-03`
- `FLOW-04`
- `FLOW-05`
- `FLOW-06`

---

# 15. Multi-Asset Expansion

Current state:
- single strategy
- single timeframe
- multi-symbol capable
- not fully multi-asset yet

Future needs:
- instrument registry
- multi-feed manager
- per-asset risk models
- concurrent scheduling
- portfolio aggregation

---

# What NOT To Do Right Now

Do NOT prioritize:
- advanced AI execution systems
- crypto expansion
- complex visual dashboards
- heavy strategy experimentation

---

# Immediate Priority

Remain focused on:

1. Paper execution hardening
2. End-to-end proof
3. Live safety
4. Operational stability

Everything above comes **after** execution truth is fully proven.
