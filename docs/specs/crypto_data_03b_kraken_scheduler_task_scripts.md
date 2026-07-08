# CRYPTO-DATA-03B — Kraken Scheduler Task Scripts

Patch ID: `CRYPTO-DATA-03B-KRAKEN-SCHEDULER-TASK-SCRIPTS-01`

This patch adds the optional Windows runner and registration scripts for a
**future** Kraken OHLC scheduled sync. It is **not** itself a Windows
Scheduled Task registration, **not** a daemon recurring job, **not** a live
Kraken API call, **not** a broker/execution/risk/OMS/runtime change, **not**
a DB migration, **not** a config-flag change, and **not** crypto trading
enablement. Continuing after `CRYPTO-DATA-03A-KRAKEN-SCHEDULED-NETWORK-GATE-01`
(committed on top of the readiness/registry lane closed through `ad7b9aca`).

---

## 1. What This Patch Adds

Two scripts under `scripts\windows\`:

- **`Run-KrakenOhlcSync.ps1`** — the runner a future scheduled task would
  call. Validates every prerequisite gate (repo/policy/registry/providers
  paths, `cargo` availability, `MQK_DATABASE_URL` presence and paper-DB
  targeting, `MQK_ALLOW_KRAKEN_SCHEDULED_SYNC` for real runs, symbol
  count/timeframe, scheduler-policy `not_registered` status, and a live
  `kraken-scheduler-readiness` check reporting `truth_state=active`) before
  ever calling `kraken-ohlc-sync`. Defaults to `-CheckOnly`, which performs
  every gate check and writes evidence but never calls `kraken-ohlc-sync`,
  never makes a network call, and never writes to the database.
- **`Register-KrakenOhlcSyncTask.ps1`** — the optional Windows Scheduled
  Task registration wrapper for the runner above, mirroring the existing
  `Register-LocalCryptoIngestTask.ps1` (`CRYPTO-DATA-01E`) pattern. Defaults
  to `-CheckOnly` (preview only) whenever neither `-Register` nor
  `-Unregister` is explicitly passed. `-Register` and `-Unregister` are both
  present as separate, explicit switches.

Plus a validator, `scripts\guards\validate_crypto_data_03b_kraken_scheduler_task_scripts.ps1`,
and this spec document.

---

## 2. Default State: The Task Is Not Registered

Running either script with no arguments, or with `-CheckOnly`, performs
**zero mutation**. No Windows Scheduled Task is created, updated, or
removed. The task is **not registered by default** — registration only
happens when an operator explicitly passes `-Register` to
`Register-KrakenOhlcSyncTask.ps1`, and this patch's own validation never
does so.

---

## 3. Safety Boundaries

- **No task registered by default.** `-CheckOnly` is the default mode for
  both scripts; `-Register` is required, explicit, and separate from
  `-Unregister`.
- **No live Kraken network call during validation.** `Run-KrakenOhlcSync.ps1
  -CheckOnly` never calls `kraken-ohlc-sync` — it stops after the gate
  checks, one of which (`kraken-scheduler-readiness`) is itself a read-only
  command that never makes a network or DB call (proven by
  `CRYPTO-DATA-02B`/`02C`).
- **No DB mutation during validation.** Neither script writes to any table.
  `Run-KrakenOhlcSync.ps1 -CheckOnly` never calls `kraken-ohlc-sync`, so the
  DB write path inside that command is never reached.
- **No credentials embedded in the task action.** The registered task action
  calls only `Run-KrakenOhlcSync.ps1 -CheckOnly:$false ...` with plain,
  non-secret arguments (symbols, timeframe, paths). `MQK_DATABASE_URL` and
  `MQK_ALLOW_KRAKEN_SCHEDULED_SYNC` are never set by the registration
  script and never appear in the task action string — they must already be
  persistent user/system environment variables at the time the task
  actually runs. Registration evidence explicitly records
  `env_vars_embedded: []` and `env_vars_required: ["MQK_DATABASE_URL",
  "MQK_ALLOW_KRAKEN_SCHEDULED_SYNC"]`.
- **`.env.local` is never read by either script.**
- **The task is never started by the registration script.** `-Register`
  creates or updates the task definition only; it never calls
  `Start-ScheduledTask`.
- **No trading enablement.** Neither script touches
  `config/providers/providers.json` or
  `config/instruments/instruments_v2.crypto_local_marks.example.json`.
  `kraken.enabled`, `BTC/USD`/`ETH/USD` `enabled`, `paper_trading_enabled`,
  and `live_trading_enabled` all remain `false`. Crypto trading remains
  disabled regardless of whether this task is ever registered.
- **Sequential, spaced calls only.** The runner calls `kraken-ohlc-sync`
  once per symbol, strictly sequentially, sleeping at least the policy's
  `min_seconds_between_pair_calls` between symbols — never in parallel,
  never batching multiple symbols into one call.
- **No daemon, runtime, broker, order, or risk reference.** Both scripts
  call only `cargo run -p mqk-cli --bin mqk-cli -- md kraken-scheduler-readiness`
  and `cargo run -p mqk-cli --bin mqk-cli -- md kraken-ohlc-sync` (runner),
  or only the runner script (registration wrapper). Neither references any
  daemon route, broker, order, or risk command.

---

## 4. Why a Two-Script Split (Runner + Registration Wrapper)

A single combined script would conflate "what runs on schedule" with "how
the schedule is installed," making it harder to reason about and test each
half in isolation. Splitting them mirrors the existing, already-reviewed
`Import-LocalCryptoMarks.ps1` / `Register-LocalCryptoIngestTask.ps1` pair
(`CRYPTO-DATA-01D`/`01E`): the runner is safe to invoke manually at any time
(and is exactly what the task would execute), while the registration
wrapper's only job is installing/removing/previewing that invocation as a
scheduled trigger — it contains no ingestion logic of its own.

---

## 5. Evidence Contracts

**Runner** (`Run-KrakenOhlcSync.ps1`), schema `kraken-ohlc-scheduled-runner-v1`,
written to `exports\market_data\kraken_ohlc_scheduled_runner_<epoch_seconds>.json`:
records `mode` (`check_only`/`scheduled_run`), every resolved path, the
resolved symbol list and timeframe, `network_call_made`/`db_write`/
`md_bars_write`/`scheduled_task_mutation` (all `false` in `check_only`
mode), `scheduler_registration_checked`, `readiness_truth_state`, the
effective rate-limit parameters, per-symbol results (real-run mode only),
`all_passed`, `reason_code`, `fail_reasons`, `warnings`, and a `safety`
object.

**Registration** (`Register-KrakenOhlcSyncTask.ps1`), schema
`kraken-ohlc-task-registration-v1`, written to
`exports\market_data\kraken_ohlc_task_registration.json` (and a matching
`.txt`): records `mode` (`check_only`/`register`/`unregister`), `task_name`,
`task_exists_before`/`task_exists_after`, `registered`/`unregistered`/
`check_only`, the full `task_action` string (for operator review — it
contains no secrets), `runner_path` and pass-through config paths,
`scheduled_task_mutation`, `network_call_made`/`db_write`/`md_bars_write`
(all `false` — this script never calls the runner in real mode, only
previews/registers/unregisters the task definition), `env_vars_embedded`
(always `[]`), `env_vars_required`, `all_passed`, `reason_code`,
`fail_reasons`, `warnings`, and a `safety` object.

---

## 6. What This Patch Does Not Change

This patch adds only the two scripts above, their validator, and this spec
document. It does not touch `core-rs/*`, `config/*`, `.env.local`, any DB
migration, or any broker/risk/execution/runtime/strategy code. No Windows
Scheduled Task was registered during this patch's own validation. No live
Kraken API call was made during validation. No DB was mutated during
validation.

---

## 7. Remaining Gaps (Unchanged by This Patch)

- No Windows Scheduled Task is actually registered — `-Register` exists but
  was never invoked by this patch.
- No daemon recurring job of any kind.
- No production registry-v2 cutover (`enabled` stays `false`).
- No crypto risk policy activation.
- No crypto paper or live execution.
- No crypto strategy.
- ~~No read-only status route/GUI panel for task-registration evidence yet~~
  — closed by `CRYPTO-DATA-03C-KRAKEN-SCHEDULER-TASK-STATUS-SURFACE-01`: see
  `docs/runbooks/local_crypto_marks_ingest.md` for the route/panel detail.
  Task registration itself (`-Register`) is still never invoked by any
  patch or route.
