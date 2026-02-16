# Kill Switches and Risk Limits Spec (V4)

Hard safety boundaries:
- STALE_DATA, REJECT_STORM, DESYNC, DRAWDOWN, MISSING_PROTECTIVE_STOP, POSITION/LEVERAGE LIMIT, PDT_PREVENTED

Each kill switch emits KILL_SWITCH_<TYPE> with evidence.

Missing protective stop is CRITICAL:
attempt repair; else FLATTEN (configurable) + DISARM.
