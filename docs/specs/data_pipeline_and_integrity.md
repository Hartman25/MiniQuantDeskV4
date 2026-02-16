# Data Pipeline and Integrity Spec (V4)

Stages:
1) raw ingest (immutable)
2) normalization to canonical bars
3) quality gates (gaps/outliers/stale/structure)
4) canonical storage
5) event emission

Rules:
- internal time UTC; calendars for session boundaries
- no-lookahead enforced (future bars hard fail)
- feed fallback with audit; feed disagreement => HALT_NEW (or DISARM) / fail promotion
- corp actions/survivorship limitations must be declared; broadened universes require proper datasets
