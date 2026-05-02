import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  children: ReactNode;
  screenKey?: string;
}

interface State {
  caught: Error | null;
}

export class ScreenErrorBoundary extends Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { caught: null };
  }

  static getDerivedStateFromError(error: Error): State {
    return { caught: error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("[ScreenErrorBoundary]", this.props.screenKey, error, info.componentStack);
  }

  componentDidUpdate(prevProps: Props) {
    if (prevProps.screenKey !== this.props.screenKey && this.state.caught !== null) {
      this.setState({ caught: null });
    }
  }

  override render() {
    if (this.state.caught !== null) {
      return (
        <div className="empty-state truth-state-notice truth-state-degraded" role="status">
          <strong>Screen unavailable</strong>
          <span>{this.state.caught.message}</span>
        </div>
      );
    }
    return this.props.children;
  }
}
