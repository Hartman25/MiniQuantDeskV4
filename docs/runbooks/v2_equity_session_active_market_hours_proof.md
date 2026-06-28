# V2 Equity Session Active — Market-Hours Proof Runbook (ASSET-CORE-05F)

## Purpose

This runbook prepares for, and structures, the one proof category that
`ASSET-CORE-05D-EQUITY-SESSION-V2-CUTOVER-SCAFFOLD-01-COMBINED` and
`ASSET-CORE-05E-EQUITY-SESSION-V2-ACTIVE-CUTOVER-HOOK-01-COMBINED` both
explicitly deferred: an **operator-supervised, real wall-clock, paper-only**
session where the daemon actually runs with
`MQK_RUNTIME_SESSION_SOURCE=v2_equity_active` during a live NYSE session, so
an operator can confirm the v2 equity session source drives the same
start/stop behavior the legacy calendar already proves.

This runbook is **off-market preparation only**. It does not run the
market-hours proof itself, and it does not flip any default. Reading this
document and running the companion collector script does not, by itself,
prove anything about live wall-clock behavior — it only prepares the exact
steps and the exact evidence capture for when an operator chooses to run the
proof during a real session.

The companion script is
[`scripts/windows/Collect-V2EquitySessionActiveProof.ps1`](../../scripts/windows/Collect-V2EquitySessionActiveProof.ps1).
It is **read-only**: it calls only `GET` routes on the local daemon (plus
optional read-only `select` queries via `docker exec ... psql` when
`-IncludeDb` is passed), and writes a timestamped evidence file under
`smoke_logs/`. It never starts or stops the daemon, never arms or disarms
anything, never clears halted state, never calls `/api/v1/ops/action`, never
calls a strategy/signal/order/broker/provider route or script, and never
writes to the database.

---

## What ASSET-CORE-05E already proved (code-level, fixed timestamps only)

- `RuntimeSessionSourceMode::V2EquityActive` (`MQK_RUNTIME_SESSION_SOURCE=v2_equity_active`)
  can drive the real `AutonomousSessionSchedule::is_in_session` decision in
  `session_controller.rs` — the single decision point consumed by the
  autonomous session controller's auto-start/auto-stop tick,
  `/api/v1/system/preflight`'s `session_in_window`, and
  `/api/v1/autonomous/readiness`'s `session_in_window` /
  `session_window_state` / `overall_ready`.
- Default behavior (`MQK_RUNTIME_SESSION_SOURCE` unset, `legacy`, or
  `v2_equity_shadow`) is unchanged — zero v2 registry IO, bit-for-bit
  pre-patch behavior.
- At four **fixed, injected** timestamps (regular-open, closed-weekend,
  holiday, before/after Black Friday early close) against the real 88-row
  production registry, active mode's in-window decision matched legacy
  exactly.
- Fail-closed proof: a missing registry makes `is_in_session` return `false`
  even at a regular-open timestamp where legacy alone would say `true` — a
  real override, not agreement-by-coincidence.
- `production_cutover_enabled`, `active_source_used`, `runtime_uses_session_v2`,
  and `trading_uses_session_v2` are all `false` by default and in
  `v2_equity_shadow` mode; in `v2_equity_active` mode they flip together only
  when the registry evaluation actually proves safe.

**What it did not prove:** any of this against the real wall clock, a real
WS feed, or a real daemon process running continuously through session-open
and session-close. Every ASSET-CORE-05D/05E test used an injected fixed
timestamp, not `Utc::now()`.

## What this market-hours proof still needs to prove

1. The daemon, started with `MQK_RUNTIME_SESSION_SOURCE=v2_equity_active`,
   reports `session_source_mode="v2_equity_active"` and
   `active_source_used=true` through live operator APIs — not just in tests.
2. `runtime_uses_session_v2=true` and `trading_uses_session_v2=true` only
   while the v2 source is actually accepted (never a false positive).
3. `/api/v1/autonomous/readiness`'s `session_in_window` /
   `session_window_state` / `overall_ready` track the real NYSE session
   (in-window during regular hours, outside-window before/after) using the
   real wall clock, not an injected one.
4. `live_routing_enabled=false` and `daemon_mode="paper"` hold for the entire
   observation window — this is a paper-only proof.
5. Intraday market data is current (`/api/v1/market-data/intraday-refresh/status`
   `all_passed=true`, not stale) before any conclusion is drawn — a stale-data
   run proves nothing.
6. Any refusal is captured honestly (`fallback_reason` /
   `activation_refusal_reason` / `autonomous_blockers`), not papered over.

---

## Pre-market checklist

Run from the repo root, before market open:

```powershell
cd C:\Users\Zacha\Desktop\MiniQuantDeskV4

git branch --show-current
git log --oneline -5
git status --short --untracked-files=no
```

- Tracked working tree must be clean. Untracked files (`smoke_logs/`, the
  stray ledger draft) do not block.
- Confirm the daemon binary you are about to run actually contains the
  ASSET-CORE-05E hook (`git log` shows the `v2 equity session active hook`
  commit in `HEAD`'s ancestry) — do not trust a stale build.
- Confirm intraday market data is fresh **before** attempting the proof.
  Use the existing refresher (see `docs/runbooks/operator_workflows.md` and
  `INTRADAY-MD-PROVIDER-FRESHNESS-TRUTH-01-COMBINED`):

  ```powershell
  powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Refresh-IntradayMarketData.ps1
  ```

  Do not proceed to the manual paper proof if this reports stale or
  incomplete bars. A stale-data run cannot be used as market-hours proof.

---

## Daemon launch env var

The default repo behavior is, and remains, **`legacy`** — nothing about this
runbook or the collector script changes that default. To exercise the v2
active path, the operator must explicitly export this environment variable
in the same terminal that starts the daemon, **in addition to** the existing
documented paper startup sequence (`docs/runbooks/operator_workflows.md` /
the ledger's "Normal Startup Commands for Paper Trading" §5):

```powershell
$env:MQK_RUNTIME_SESSION_SOURCE = "v2_equity_active"
```

Set this before `cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --bin mqk-daemon`
in the same terminal/session — an unset or misspelled value silently falls
back to `legacy` (fail-soft by design; the daemon will not abort, but the
proof will not be exercising the path under test). Verify it actually took
effect using the collector script below before treating any further
observation as meaningful.

This runbook does not itself start, stop, arm, disarm, or otherwise mutate
the daemon. Daemon startup, arming, and shutdown remain governed by the
existing runbooks (`docs/runbooks/operator_workflows.md`,
`docs/runbooks/autonomous_paper_ops.md`,
`docs/runbooks/operator_control_surface.md`) — this runbook only adds the one
env var above to that existing, already-proven sequence.

---

## Read-only collector command

Once the daemon is running (with or without the env var above — the script
detects and reports either case) and you are inside the intended observation
window:

```powershell
cd C:\Users\Zacha\Desktop\MiniQuantDeskV4

powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Collect-V2EquitySessionActiveProof.ps1
```

Optional flags:

- `-IncludeDb` — additionally collect read-only evidence from `runs`,
  `sys_arm_state`, and `sys_autonomous_session_events` via
  `docker exec mqk-paper-postgres psql ...` (`select` only, no writes).
- `-AllowOutsideMarketWindow` — use only for **off-market dry validation**
  of the script/wiring itself (e.g. confirming the env var is visible before
  the session opens). Do not use this flag when actually trying to prove the
  in-window behavior — outside-window is expected to block in that case.
- `-SkipIntradayRefreshCheck` — use only if intraday freshness was already
  independently confirmed through some other path this run. Do not use this
  to paper over a real stale-data blocker.
- `-BaseUrl` / `-OutDir` / `-Depth` — override daemon URL, evidence output
  directory, and JSON serialization depth if needed.

Run it more than once across the session if useful (e.g. once shortly after
open, once mid-session, once near close) — each run produces its own
timestamped evidence pair under `smoke_logs/`:

```text
smoke_logs/v2_equity_active_session_proof_YYYYMMDD_HHMMSS.json
smoke_logs/v2_equity_active_session_proof_YYYYMMDD_HHMMSS.txt
```

---

## What "pass" looks like

The script prints a `VERDICT` section and exits `0` only when every one of
the following holds simultaneously (mirrored in the JSON's `verdict` object):

- `daemon_reachable=true`
- `paper_mode_confirmed=true` (`daemon_mode="paper"`, `adapter_id="alpaca"`)
- `live_routing_disabled=true`
- `v2_active_configured=true` (`session_source_mode="v2_equity_active"`)
- `v2_active_source_used=true`
- `runtime_uses_session_v2=true`
- `trading_uses_session_v2=true`
- `session_in_window` matches the real session state at the moment checked
- `runtime_start_allowed=true` (no conflicting run already active)
- `intraday_refresh_passed=true` (unless explicitly skipped)
- `blockers` is empty, so `safe_to_continue_to_manual_paper_proof=true`

A `PASS` from this script means the read-only evidence is consistent with v2
active mode correctly driving session truth **at the moment it was checked**.
It is not, by itself, the full market-hours proof — see "How to classify the
final verdict" below.

## What "blocked" looks like

Exit code `2` and `safe_to_continue_to_manual_paper_proof=false` mean at
least one condition above failed. The `blockers` array (console + JSON +
`.txt`) names exactly which one(s), for example:

- `session_source_mode='legacy' -- daemon was not started with MQK_RUNTIME_SESSION_SOURCE=v2_equity_active`
- `active_source_used=False -- v2 candidate was not accepted as authoritative (fallback_reason=...)`
- `session_in_window=False ... -- not currently inside the trading session window`
- `intraday refresh all_passed=False stale_or_missing_evidence=True -- run Refresh-IntradayMarketData.ps1 before proceeding`

Exit code `1` means the collector could not even reach the daemon
(`daemon_unreachable`) or failed before evidence could be written — this is a
collector/connectivity problem, not a session-source verdict.

A `BLOCKED` result is not a failure of this patch — it is the correct,
honest outcome. Record it (timestamp, exact blockers, evidence file path)
and stop. Do not relax a gate, do not re-run with a skip flag to make a real
blocker disappear, and do not edit the evidence file.

## What NOT to do

- Do not enable live routing, under any circumstance, to "test" this proof.
- Do not submit live orders.
- Do not submit paper orders as part of this proof — this is observation
  only; if you want to separately run a paper trading session, use the
  existing, unrelated runbooks for that.
- Do not use `-AllowOutsideMarketWindow` or `-SkipIntradayRefreshCheck` to
  manufacture a passing result outside their stated dry-validation purpose.
- Do not claim market-hours proof from off-market data, a closed-market run,
  or a holiday.
- Do not claim a default production cutover just because active mode works
  when explicitly configured — `MQK_RUNTIME_SESSION_SOURCE` still defaults to
  `legacy`, and this runbook does not change that.
- Do not stage generated `smoke_logs/` evidence files or the untracked
  `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` draft.
- Do not run this script, or any market-hours proof, against the live broker
  adapter — paper + Alpaca only.

## Manual push/commit note

This runbook and the collector script are local documentation/tooling only.
Commit them locally as their own patch if asked to. Do not push unless the
operator explicitly asks. The market-hours proof itself produces evidence
files under `smoke_logs/` — those stay untracked/local; do not commit or
push them as part of any patch.

---

## How to classify the final verdict

After running the collector script during an actual NYSE regular session
with `MQK_RUNTIME_SESSION_SOURCE=v2_equity_active` configured:

- **`ASSET-CORE-05-MARKET-HOURS-V2-ACTIVE-PROOF CLOSED`** — the collector
  reported `PASS` (`safe_to_continue_to_manual_paper_proof=true`) at least
  once clearly inside the regular session window, with fresh intraday data,
  `live_routing_enabled=false`, and `active_source_used=true` /
  `runtime_uses_session_v2=true` / `trading_uses_session_v2=true` all
  observed together. Record the evidence file path and timestamp.
- **`PARTIAL`** — some evidence was collected (e.g. confirmed the env var
  takes effect, confirmed outside-window refusal behaves correctly) but a
  full in-window `PASS` was not observed this session — for example, the
  session closed before a clean run, or intraday data went stale mid-session.
  Record exactly what was and was not observed.
- **`BLOCKED`** — the collector could not get past a real blocker
  (daemon unreachable, stale intraday data, v2 active mode refused, live
  routing somehow enabled) during the intended window. Record the exact
  blocker(s) and the evidence file path; do not retry by relaxing a gate.

In every case, the verdict must be supported by a specific evidence file
under `smoke_logs/` from this runbook's collector script — not by session
memory, a prior conversation's claim, or this runbook's own text.

## Recommended exact market-hours proof command sequence

```powershell
# 1. Pre-flight (see "Pre-market checklist" above)
cd C:\Users\Zacha\Desktop\MiniQuantDeskV4
git status --short --untracked-files=no
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Refresh-IntradayMarketData.ps1

# 2. Start the daemon with the v2 active env var (Terminal 1; see
#    docs/runbooks/operator_workflows.md for the rest of the normal startup
#    sequence -- DB start/migrate, strategy registry seed, etc.)
$env:MQK_RUNTIME_SESSION_SOURCE = "v2_equity_active"
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --bin mqk-daemon

# 3. From a second terminal, once the daemon is up and the normal
#    arm/start sequence (operator_workflows.md) has been followed:
cd C:\Users\Zacha\Desktop\MiniQuantDeskV4
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Collect-V2EquitySessionActiveProof.ps1 -IncludeDb

# 4. Re-run the collector at additional points across the session as desired:
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Collect-V2EquitySessionActiveProof.ps1

# 5. Classify the result per "How to classify the final verdict" above,
#    referencing the smoke_logs/v2_equity_active_session_proof_*.json files
#    produced by each run.
```
