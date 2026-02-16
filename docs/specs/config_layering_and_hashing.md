# Config Layering and Hashing (V4)

Defines deterministic config layering, hashing, and secrets exclusion.

## 1) Sources
1. config/defaults/base.yaml
2. config/environments/<env>.yaml
3. config/engines/<engine>.yaml
4. config/risk_profiles/<profile>.yaml
5. optional config/stress_profiles/<profile>.yaml
6. secrets via environment variables only

## 2) Merge order
effective = merge(base, env, engine, risk_profile, stress_profiles...)
Precedence: later overrides earlier.
Deep merge for maps. Arrays replaced (default).
No hidden runtime defaults outside YAML.

## 3) Canonicalization
Before hashing:
- stable key ordering
- normalize numeric formatting
- remove comments
- validate required keys
Validation failure aborts startup.

## 4) Hashing
SHA-256 over canonicalized effective config (non-secret).
Store runs.config_hash and runs.config_json (non-secret).
Export manifest.json includes config_hash and config_json.

## 5) Secrets exclusion
Secrets must never appear in YAML/DB/manifests/logs.
Config may reference env var NAMES only.
If secret-like value detected in YAML/effective config:
- abort
- emit CONFIG_SECRET_DETECTED (redacted)

## 6) Change control
Risk/execution/data/arming/promotion changes:
- require new run_id + new config_hash
- no hot edits in LIVE

## 7) Required tests
- merge precedence
- canonicalization stability
- config hash stability
- secrets excluded from manifest and DB
- abort on secret present in YAML
