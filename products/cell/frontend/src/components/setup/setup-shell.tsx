import { ArrowLeft, Loader2 } from "lucide-react";

interface Step {
  number: number;
  label: string;
}

const STEPS: Step[] = [
  { number: 1, label: "Install" },
  { number: 2, label: "Connect" },
];

interface SetupShellProps {
  step: number;
  title: string;
  description: string;
  error?: string | null;
  onBack?: () => void;
  children?: React.ReactNode;
  footer?: React.ReactNode;
  loading?: boolean;
  showSteps?: boolean;
}

export function SetupShell({
  step,
  title,
  description,
  error,
  onBack,
  children,
  footer,
  loading = false,
  showSteps = true,
}: SetupShellProps) {
  return (
    <div className="min-h-screen flex flex-col items-center justify-center bg-background p-6">
      <div className="w-full max-w-md">
        {/* Logo */}
        <div className="text-center mb-8">
          <h1 className="text-2xl font-bold tracking-tight text-foreground">
            GitIM<span className="text-primary">·</span>Cell
          </h1>
          <p className="text-sm text-text-muted mt-1">AI-native IM over Git</p>
        </div>

        {showSteps && (
          <div className="mb-8">
            <div className="flex items-center justify-between relative">
              <div className="absolute left-0 right-0 top-4 h-0.5 bg-border rounded-full" />
              <div
                className="absolute left-0 top-4 h-0.5 bg-primary rounded-full transition-all duration-500"
                style={{
                  width:
                    STEPS.length > 1
                      ? `${((step - 1) / (STEPS.length - 1)) * 100}%`
                      : "0%",
                }}
              />
              {STEPS.map((s) => {
                const isActive = s.number === step;
                const isCompleted = s.number < step;
                return (
                  <div key={s.number} className="relative z-10 flex flex-col items-center gap-2">
                    <div
                      className={[
                        "w-8 h-8 rounded-full flex items-center justify-center text-sm font-semibold border-2 transition-all duration-300",
                        isActive
                          ? "bg-primary border-primary text-white shadow-[0_0_12px_var(--color-glow-primary)]"
                          : isCompleted
                            ? "bg-primary border-primary text-white"
                            : "bg-card border-border text-text-muted",
                      ].join(" ")}
                    >
                      {isCompleted ? "✓" : s.number}
                    </div>
                    <span
                      className={[
                        "text-xs font-medium transition-colors",
                        isActive
                          ? "text-primary"
                          : isCompleted
                            ? "text-text-secondary"
                            : "text-text-muted",
                      ].join(" ")}
                    >
                      {s.label}
                    </span>
                  </div>
                );
              })}
            </div>
          </div>
        )}

        {/* Card */}
        <div className="rounded-2xl border border-border bg-card/90 shadow-xl shadow-[var(--color-shadow)] p-6">
          {onBack && !loading && (
            <button
              onClick={onBack}
              className="flex items-center gap-1 text-xs text-text-muted hover:text-foreground transition-colors mb-4 -ml-1"
            >
              <ArrowLeft className="size-3.5" />
              Back
            </button>
          )}

          <div className="mb-6">
            <h2 className="text-lg font-semibold text-foreground">{title}</h2>
            <p className="text-sm text-text-muted mt-1">{description}</p>
          </div>

          {loading ? (
            <div className="flex flex-col items-center justify-center py-8 gap-3">
              <Loader2 className="size-6 text-primary animate-spin" />
              <p className="text-sm text-text-muted">Connecting to runtime...</p>
            </div>
          ) : (
            children
          )}

          {error && !loading && (
            <div className="mt-4 p-3 rounded-lg bg-destructive/10 border border-destructive/20">
              <p className="text-xs text-destructive flex items-start gap-2">
                <span className="inline-block w-1 h-1 rounded-full bg-destructive shrink-0 mt-1.5" />
                {error}
              </p>
            </div>
          )}
        </div>

        {footer && (
          <div className="mt-6 text-center text-xs text-text-muted leading-relaxed">
            {footer}
          </div>
        )}
      </div>
    </div>
  );
}
