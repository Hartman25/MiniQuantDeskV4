import { initDesktopBootstrap } from "./desktop/bootstrap";
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

// Extracted helper so both the success path and the failure catch path can
// show the hidden Tauri window without duplicating the import/try logic.
// DESKTOP-LAUNCH-03: Show the native OS window, not the webview handle.
// getCurrentWindow() targets the Window (native OS layer); show() sends
// plugin:window|show over IPC, which requires core:window:allow-show in the
// capability (added to desktop.json). Without that permission the IPC call
// was rejected and silently swallowed by the catch, leaving the process
// alive-but-invisible.  Fallback to WebviewWindow.show() covers the case
// where the window plugin is unavailable (non-Tauri browser context).
async function showDesktopWindow(): Promise<void> {
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().show();
  } catch {
    try {
      const { getCurrentWebviewWindow } = await import(
        "@tauri-apps/api/webviewWindow"
      );
      await getCurrentWebviewWindow().show();
    } catch {
      // Not in a Tauri context — no-op.
    }
  }
}

// Escape HTML entities before injecting error text into innerHTML.
function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

// DESKTOP-LAUNCH-01A: Render a plain fail-closed startup-error surface using
// raw DOM manipulation so this path does not depend on React being functional.
function renderBootstrapFailure(err: unknown): void {
  const message = err instanceof Error ? err.message : String(err);
  const root = document.getElementById("root") ?? document.body;
  root.innerHTML = `
    <div style="font-family:monospace;padding:2rem;color:#c00;background:#111;min-height:100vh;box-sizing:border-box;">
      <h2 style="margin:0 0 1rem;color:#fff;">Veritas Ledger — Startup Failed</h2>
      <p style="margin:0 0 0.5rem;color:#aaa;">The desktop process failed to initialize. Check the launcher log for details.</p>
      <pre style="white-space:pre-wrap;word-break:break-all;color:#f88;">${escapeHtml(message)}</pre>
    </div>`;
}

async function bootstrap(): Promise<void> {
  await initDesktopBootstrap();

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );

  // DESKTOP-LAUNCH-02: Show is called unconditionally after React mounts.
  // showDesktopWindow() already no-ops when not in a Tauri context (the
  // @tauri-apps/api/webviewWindow import fails and the catch swallows it).
  // The previous isDesktopShell() gate was the suppression seam: if
  // initDesktopBootstrap() silently fell back to DEFAULT_BOOTSTRAP (its own
  // catch at bootstrap.ts), isDesktopShell() returned false and the hidden
  // window was never shown despite the process staying alive. Removing the
  // gate eliminates that dependency while preserving the no-op in browser
  // context and the failure-path show via bootstrap().catch() below.
  requestAnimationFrame(() => {
    void showDesktopWindow();
  });
}

// DESKTOP-LAUNCH-01A: Top-level bootstrap rejection is explicitly caught.
// On failure the hidden window is shown so the operator sees the error surface
// instead of the process disappearing silently.
bootstrap().catch((err: unknown) => {
  void showDesktopWindow();
  renderBootstrapFailure(err);
});
