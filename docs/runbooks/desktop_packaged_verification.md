# Desktop Packaged Verification Runbook
## DESKTOP-VERIFY-01

Scope: packaged binary build, desktop shortcut install, and step-by-step
Observe / TradeReady / single / two / three desk-mode proof for the
`Veritas Ledger.veritas` desktop-shortcut path.

This runbook is the authoritative pass/fail record for the packaged
desktop shortcut path. Do not mark DESKTOP-VERIFY-01 CLOSED until each
section below has been walked and the operator has recorded a verdict.

---

## Prerequisites

All items must be satisfied before running any verification step.

### Environment variables

Set these in `<repo_root>/.env.local` (or in your user environment).
The launcher imports this file automatically; it does NOT fall through to
system environment if the variable is already set in `.env.local`.

```
MQK_OPERATOR_TOKEN=<your-operator-token>
ALPACA_API_KEY_PAPER=<paper-key>
ALPACA_API_SECRET_PAPER=<paper-secret>
ALPACA_PAPER_BASE_URL=https://paper-api.alpaca.markets
MQK_DATABASE_URL=<postgres-url>
```

**Fail-closed checks by the launcher:**
- `MQK_OPERATOR_TOKEN` absent → launcher prints error and waits for Enter; window
  does NOT open silently.
- `broker_config_present=false` → launcher fails closed with explicit message.

### Toolchain

| Tool | Minimum version | Check |
|---|---|---|
| Rust / cargo | stable | `cargo --version` |
| Node.js | 18 LTS | `node --version` |
| npm | bundled with Node | `npm --version` |
| PowerShell | 5.1 (Windows built-in) | `$PSVersionTable.PSVersion` |
| WebView2 runtime | any current | Control Panel → Installed Programs |

---

## Step 1 — Build the packaged binary

Run from the repo root in PowerShell:

```powershell
cd core-rs\mqk-gui
npm ci
npm run tauri build -- --no-bundle
```

Expected output (last lines):
```
    Finished release [optimized] target(s) in …
    Built application at: …\core-rs\target\release\mqk-gui.exe
```

**PASS:** `core-rs\target\release\mqk-gui.exe` exists after the command.

**FAIL:** Build error printed. Diagnose and fix before continuing — do not
proceed with a broken binary.

The launcher (`Launch-VeritasLedger.ps1`) performs this build automatically
if neither candidate binary is found, so this step can be skipped if you
want the launcher to build on first run. Building here first isolates build
failures from launcher failures.

---

## Step 2 — Install the desktop shortcut

Run once per machine in PowerShell (no admin required):

```powershell
.\scripts\windows\Install-VeritasLedgerDesktopShortcut.ps1
```

Expected output:
```
Desktop launcher created: C:\Users\<user>\Desktop\Veritas Ledger.veritas
  Double-click  =>  Observe mode (idle-only; no privileged POSTs)
  Right-click   =>  'Open in Trade Ready mode' (Bearer auth round-trip)
  Icon:            …\assets\logo\veritas_ledger_shield.ico,0
  File type:       VeritasLedger.launcher.1 (HKCU; no admin required)
```

**PASS criteria:**
- `Veritas Ledger.veritas` appears on the desktop.
- Icon is the shield icon (not a blank-page icon).
- No old `.lnk` shortcuts remain on the desktop.

**FAIL:** Error printed. Check that `scripts\windows\Launch-VeritasLedger.ps1`
is present and `assets\logo\veritas_ledger_shield.ico` exists.

If the icon still shows a blank page after install, sign out and sign back
in. The installer calls `SHChangeNotify` but Windows may delay the refresh.

---

## Step 3 — Observe mode via double-click

**What the operator does:**
Double-click `Veritas Ledger.veritas` on the desktop.

A PowerShell console window opens. The launcher:
1. Loads `.env.local` if present.
2. Reads `MQK_OPERATOR_TOKEN`.
3. Sets env vars: `MQK_DAEMON_DEPLOYMENT_MODE=paper`, `MQK_DAEMON_ADAPTER_ID=alpaca`,
   `MQK_DAEMON_ADDR=127.0.0.1:8899`, `MQK_GUI_DAEMON_URL=http://127.0.0.1:8899`.
4. Resolves or builds `mqk-daemon.exe`.
5. Probes the daemon at five routes:
   - `/api/v1/system/metadata`
   - `/api/v1/system/status`
   - `/api/v1/system/session`
   - `/api/v1/system/preflight`
   - `/api/v1/autonomous/readiness`
6. Verifies `daemon_mode=paper`, `adapter_id=alpaca`, `operator_auth_mode=token_required`.
7. Performs a Bearer auth round-trip: `POST /api/v1/ops/action` with
   `action_key=__veritas_launcher_auth_probe__` → expects `400 unknown_action accepted=false`.
8. Resolves or builds `mqk-gui.exe`.
9. Launches `mqk-gui.exe`; window starts hidden (Tauri `visible: false`).
10. React mounts → `requestAnimationFrame` triggers `getCurrentWebviewWindow().show()`.

**Expected console output (Observe mode, daemon already running):**
```
[Veritas Ledger] Launcher mode: observe/attach
[Veritas Ledger] Resolving daemon binary
[Veritas Ledger] Reusing verified local mqk-daemon for observe/attach mode
[Veritas Ledger] Verified canonical backend for observe/attach mode: service=mqk-daemon mode=paper adapter=alpaca auth=token_required runtime=idle db=connected ws=cold_start_unproven reconcile=… arm=… session=…
[Veritas Ledger] Backend is NOT trade-ready: …   ← expected in Observe; readiness reasons listed
[Veritas Ledger] Resolving desktop GUI binary
[Veritas Ledger] Launching desktop GUI against verified local daemon
[Veritas Ledger] GUI opened in observe/attach mode against the verified canonical backend. No runtime auto-start was performed.
```

**PASS criteria — Observe mode:**
- Console prints no `LAUNCH FAILED` line.
- The `Veritas Ledger` window opens without a blank-screen flash (hidden until React painted).
- The window title bar reads `Veritas Ledger`.
- The left rail is visible: Veritas Ledger shield logo + "Operator Console" heading.
- Screen defaults to `Dashboard`.
- GlobalStatusBar (top bar) shows daemon status pills.
- No `Startup Failed` error overlay is displayed.
- Console does NOT block waiting for Enter (which only happens on failure).

**FAIL criteria:**
- Console prints `LAUNCH FAILED` and waits for Enter — read the error.
- Window is blank white and stays blank (bootstrap failure or WebView2 issue).
- Window shows `Veritas Ledger — Startup Failed` overlay — read the error in the overlay.
- Any `LAUNCH FAILED` or throw visible in console.

---

## Step 4 — Trade Ready mode via right-click

**Prerequisites for TradeReady to pass the readiness check:**
- Alpaca paper WS must reach `Live` state: the daemon must have a confirmed
  WebSocket connection to Alpaca (waits up to 30s on daemon start).
- `arm_ready=true`: system must be armed.
- `session_in_window=true`: UTC time must be within the configured trading window.
- `reconcile_ready=true`: reconcile status must be clean/idle.

**What the operator does:**
Right-click `Veritas Ledger.veritas` → "Open in Trade Ready mode".

The launcher performs all the same probes as Observe mode **plus** the
autonomous readiness gate. If any readiness check fails, the launcher
terminates with a descriptive `LAUNCH FAILED` message listing the specific
blocking conditions.

**Expected console output (TradeReady, all conditions met):**
```
[Veritas Ledger] Launcher mode: trade-ready
…
[Veritas Ledger] Backend is trade-ready under mounted daemon truth.
[Veritas Ledger] Resolving desktop GUI binary
[Veritas Ledger] Launching desktop GUI against verified local daemon
[Veritas Ledger] Started verified trade-ready local paper daemon (PID …)
[Veritas Ledger] GUI opened in trade-ready mode against the verified canonical backend. Trading runtime remains idle until you explicitly start it.
```

**PASS criteria — TradeReady:**
- All Observe mode pass criteria apply.
- Console prints `Backend is trade-ready under mounted daemon truth.`
- Console prints `GUI opened in trade-ready mode…`

**FAIL — expected when preconditions are not met:**
The launcher prints `LAUNCH FAILED` with a structured reasons list, e.g.:
```
[Veritas Ledger] LAUNCH FAILED
  Verified canonical backend is not trade-ready.
  autonomous readiness reports ws_continuity_ready=False (ws_continuity=cold_start_unproven);
  autonomous readiness reports arm_ready=False (arm_state=disarmed);
  autonomous readiness reports session_in_window=False (session_window_state=pre_market)
```
This is **correct fail-closed behavior** — not a bug. The operator must resolve
each blocking condition before TradeReady mode becomes openable.

**Common TradeReady blockers and resolutions:**

| Blocker | Resolution |
|---|---|
| `ws_continuity_ready=False` / `cold_start_unproven` | Wait for daemon WS to establish; check Alpaca paper credentials. |
| `arm_ready=False` / `disarmed` | Arm the system via Ops → Arm action or API. |
| `session_in_window=False` / `pre_market` | Launch during market hours (or reconfigure session window). |
| `reconcile_ready=False` | Review and clear reconcile dirty state. |
| `broker_config_present=False` | Check `ALPACA_API_KEY_PAPER` / `ALPACA_API_SECRET_PAPER` in env. |
| `operator token was rejected` | Check `MQK_OPERATOR_TOKEN` matches the daemon's configured token. |

---

## Step 5 — Single-screen desk mode

**Within the running GUI window (control role, label="main"):**

Verify the toolbar shows three buttons: `1 window`, `2 monitors`, `3 monitors`.
Click `1 window`.

**PASS criteria:**
- Only the main `Veritas Ledger` window exists.
- No secondary windows titled `Veritas Ledger — Execution` or `Veritas Ledger — Oversight`.
- `deskMode` in localStorage (`mqd.desktop.deskMode`) = `"single"`.
- After reload, the window reopens in single mode (persisted).

---

## Step 6 — Two-screen desk mode

Click `2 monitors` in the toolbar.

**PASS criteria:**
- A second window titled `Veritas Ledger — Execution` opens (1600×1000).
- The second window shows a `RoleCommandStrip` (not a `LeftCommandRail`) with
  buttons: `Execution`, `Transport`, `Runtime`.
- Clicking `Execution` in the second window switches to the Execution screen.
- No third `Veritas Ledger — Oversight` window exists.
- Switching back to `1 window` closes the execution window.

**FAIL indicators:**
- Second window fails to open (Tauri capability error in browser devtools console).
- Second window shows the full left rail instead of the role strip — label
  detection is broken.
- Second window shows `Startup Failed` overlay.

---

## Step 7 — Three-screen desk mode

Click `3 monitors` in the toolbar.

**PASS criteria:**
- `Veritas Ledger — Execution` window opens (1600×1000) or is already present.
- `Veritas Ledger — Oversight` window opens (1500×960).
- The oversight window shows a `RoleCommandStrip` with the full diagnostics
  group: `Logs / Audit`, `Incidents`, `Alerts`, `Operator Timeline`, `Runtime`,
  `Metrics`, `Topology`, `Transport`, `Artifacts`, `Risk`.
- The oversight window does NOT show the `BottomEventRail` (`showBottomRail=false`
  for oversight role per AppShell.tsx:194).
- Switching back to `2 monitors` closes the oversight window and keeps execution.
- Switching to `1 window` closes both.

**FAIL indicators:**
- Oversight window fails to open.
- Oversight window shows the bottom event rail (role detection broken).
- Any window shows `Startup Failed` overlay.

---

## Step 8 — Persistence check

After setting desk mode to `two` or `three`, close all windows and re-launch
via double-click on `Veritas Ledger.veritas`.

**PASS:**
- The main window opens.
- Desk mode toggle shows the previously-selected mode as active.
- Secondary windows re-open only if the operator clicks the mode button
  again — they do NOT reopen automatically on launch (windows are managed
  by AppShell at runtime, not persisted across process restarts).

**Note:** deskMode preference persists in localStorage; window state does not.
This is correct behavior.

---

## Step 9 — Manual launcher invocation (alternative to shortcut)

For diagnostics without the shortcut, invoke the launcher directly:

```powershell
# Observe mode (same as double-click):
powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\scripts\windows\Launch-VeritasLedger.ps1 -Mode Observe

# TradeReady mode (same as right-click):
powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\scripts\windows\Launch-VeritasLedger.ps1 -Mode TradeReady

# Force rebuild before launch:
powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\scripts\windows\Launch-VeritasLedger.ps1 -Mode Observe -Rebuild
```

These are identical to the shortcut commands — the installer embeds these
exact strings in the HKCU verb handler.

---

## Verification inventory and status

| # | Item | Code-verified | Runtime-proven |
|---|---|---|---|
| 1 | Packaged build path (`npm run tauri build -- --no-bundle`) | Yes | Pending |
| 2 | Desktop shortcut install (`.veritas` HKCU handler, double-click/right-click verbs) | Yes | Pending |
| 3 | Observe mode — daemon probe + bearer auth round-trip | Yes | Pending |
| 4 | TradeReady mode — preconditions pass | Yes | Pending |
| 5 | TradeReady mode — preconditions fail (correct LAUNCH FAILED) | Yes | Pending |
| 6 | Single-screen mode | Yes | Pending |
| 7 | Two-screen mode (execution window, role detection) | Yes | Pending |
| 8 | Three-screen mode (execution + oversight windows, role detection) | Yes | Pending |
| 9 | deskMode localStorage persistence across launch | Yes | Pending |

Update this table with `PASS` or `FAIL` + date as each item is walked.

---

## Known honest gaps (not blockers to this runbook being complete)

1. **TradeReady proof requires real Alpaca paper credentials and market hours.**
   The `ws_continuity_ready` check will always block if Alpaca WS has not
   reached `Live` state. This is correct fail-closed behavior.

2. **Secondary windows are managed at runtime, not persisted across process
   restart.** The operator must click the desk mode button to re-open them
   after relaunching the GUI.

3. **WebView2 must be installed.** If WebView2 is absent, the GUI window
   will either fail to open or show a blank screen. This is a system
   prerequisite, not a code issue.

4. **Icon refresh** may require sign-out/sign-in if `SHChangeNotify` does
   not trigger immediately after shortcut install.

5. **`mqd.desktop.deskMode` localStorage key** is shared across all windows
   of the same Tauri app origin. Secondary windows do not independently
   re-open because window management is fully in the control window. This
   is intentional.

---

## Verdict guide

**CLOSED:** Every row in the Verification inventory table above is marked PASS
(or FAIL with a documented known-acceptable reason), and the operator has
walked the shortcut path from a real desktop entry on the target machine.

**NOT CLOSED:** Any row in the table is still Pending, or any step produced
an unexpected FAIL that does not have a documented resolution.

Current status: **NOT CLOSED — runtime walk pending.**
