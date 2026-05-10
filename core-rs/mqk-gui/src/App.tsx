import { Component, type ErrorInfo, type ReactNode } from "react";
import { AppShell } from "./app/AppShell";

class AppErrorBoundary extends Component<{ children: ReactNode }, { caught: Error | null }> {
  constructor(props: { children: ReactNode }) {
    super(props);
    this.state = { caught: null };
  }

  static getDerivedStateFromError(error: Error): { caught: Error } {
    return { caught: error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("[AppErrorBoundary] Unhandled render error:", error, info.componentStack);
  }

  override render() {
    if (this.state.caught !== null) {
      return (
        <div
          style={{
            fontFamily: "monospace",
            padding: "2rem",
            background: "#050505",
            color: "#aaa",
            minHeight: "100vh",
            boxSizing: "border-box",
          }}
        >
          <div
            style={{
              padding: "1.5rem 2rem",
              border: "1px solid rgba(239,68,68,0.4)",
              borderRadius: 4,
              background: "rgba(239,68,68,0.05)",
              maxWidth: 600,
            }}
          >
            <strong style={{ display: "block", marginBottom: "0.5rem", color: "#ef4444" }}>
              Operator session unavailable — render error
            </strong>
            <p style={{ margin: "0 0 0.75rem", color: "#6B7A8A", fontSize: "0.85rem" }}>
              The interface encountered an unhandled error. Check the browser console for the full
              stack trace, then restart the session.
            </p>
            <pre
              style={{
                margin: 0,
                fontSize: "0.8rem",
                color: "#888",
                whiteSpace: "pre-wrap",
                wordBreak: "break-all",
              }}
            >
              {this.state.caught.message}
            </pre>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}

export default function App() {
  return (
    <AppErrorBoundary>
      <AppShell />
    </AppErrorBoundary>
  );
}
