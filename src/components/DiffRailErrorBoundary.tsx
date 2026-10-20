import { Component, type ErrorInfo, type ReactNode } from "react";
import { diffRailStore, writeDiffRailPreferences } from "../diffRail";
import { useT } from "../i18n/runtime";

type BoundaryProps = {
  children: ReactNode;
  description: string;
  retryLabel: string;
  title: string;
};

type BoundaryState = {
  failed: boolean;
};

class DiffRailBoundary extends Component<BoundaryProps, BoundaryState> {
  state: BoundaryState = { failed: false };

  static getDerivedStateFromError(): BoundaryState {
    return { failed: true };
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error("Diff rail render failed", error, info);
    writeDiffRailPreferences({
      ...diffRailStore.getSnapshot().ui,
      open: false,
    });
  }

  private retry = () => {
    this.setState({ failed: false });
    writeDiffRailPreferences(diffRailStore.getSnapshot().ui);
  };

  render() {
    if (!this.state.failed) return this.props.children;
    return (
      <aside aria-label={this.props.title} className="diff-rail-error">
        <strong>{this.props.title}</strong>
        <span>{this.props.description}</span>
        <button onClick={this.retry} type="button">
          {this.props.retryLabel}
        </button>
      </aside>
    );
  }
}

export function DiffRailErrorBoundary({ children }: { children: ReactNode }) {
  const t = useT();
  return (
    <DiffRailBoundary
      description={t(
        "Halv closed the diff rail after an error. Retry when you are ready.",
      )}
      retryLabel={t("Retry")}
      title={t("Diff rail closed")}
    >
      {children}
    </DiffRailBoundary>
  );
}
