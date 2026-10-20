import { Component, type ErrorInfo, type ReactNode } from "react";
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

class AppBoundary extends Component<BoundaryProps, BoundaryState> {
  state: BoundaryState = { failed: false };

  static getDerivedStateFromError(): BoundaryState {
    return { failed: true };
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error("App render failed", error, info);
  }

  private retry = () => {
    window.location.reload();
  };

  render() {
    if (!this.state.failed) return this.props.children;

    return (
      <main className="flex min-h-screen items-center justify-center bg-surface-base px-6 text-content-primary">
        <section
          className="w-full max-w-md rounded-2xl border border-surface-border/40 bg-surface-panel p-6 shadow-xl shadow-black/20"
          role="alert"
        >
          <span className="text-xs font-bold tracking-[0.2em] text-accent-strong">
            HALV
          </span>
          <h1 className="mt-3 text-xl font-semibold">{this.props.title}</h1>
          <p className="mt-2 text-sm text-content-secondary">
            {this.props.description}
          </p>
          <button
            className="mt-5 rounded-lg bg-accent px-4 py-2 text-sm font-semibold text-accent-contrast"
            onClick={this.retry}
            type="button"
          >
            {this.props.retryLabel}
          </button>
        </section>
      </main>
    );
  }
}

export function AppErrorBoundary({ children }: { children: ReactNode }) {
  const t = useT();
  return (
    <AppBoundary
      description={t("Reload to try again.")}
      retryLabel={t("Retry")}
      title={t("Halv could not open this window.")}
    >
      {children}
    </AppBoundary>
  );
}
