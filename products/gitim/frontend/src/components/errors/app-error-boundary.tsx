import { Component, type ErrorInfo, type ReactNode } from "react";

interface AppErrorBoundaryProps {
  children: ReactNode;
}

interface AppErrorBoundaryState {
  error: Error | null;
}

export class AppErrorBoundary extends Component<
  AppErrorBoundaryProps,
  AppErrorBoundaryState
> {
  state: AppErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): AppErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(
      "[gitim] Unhandled UI error",
      error,
      info.componentStack ?? "",
    );
  }

  private retry = () => {
    this.setState({ error: null });
  };

  render() {
    if (!this.state.error) {
      return this.props.children;
    }

    return (
      <main className="flex min-h-screen items-center justify-center bg-background p-6">
        <section
          role="alert"
          className="w-full max-w-md rounded-xl border border-border bg-surface p-6 text-center shadow-xl shadow-[var(--color-shadow)]"
        >
          <h1 className="text-xl font-semibold text-foreground">
            Something went wrong
          </h1>
          <p className="mt-2 text-sm leading-relaxed text-text-muted">
            The workspace hit an unexpected UI error. Retry the current view.
          </p>
          <button
            type="button"
            data-action="retry"
            className="mt-5 rounded-lg bg-primary px-4 py-2 text-sm font-medium text-primary-foreground transition-colors duration-75 hover:bg-primary/90"
            onClick={this.retry}
          >
            Try again
          </button>
        </section>
      </main>
    );
  }
}
