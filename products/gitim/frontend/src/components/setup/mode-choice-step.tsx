import { Globe2, Monitor } from "lucide-react";
import { SetupShell } from "./setup-shell";

interface ModeChoiceStepProps {
  onUseRuntime: () => void;
  onUseBrowserMode: () => void;
}

export function ModeChoiceStep({
  onUseRuntime,
  onUseBrowserMode,
}: ModeChoiceStepProps) {
  return (
    <SetupShell
      step={1}
      title="Choose Mode"
      description="Pick how this browser connects to gitim"
      showSteps={false}
    >
      <div className="space-y-3">
        <button
          type="button"
          onClick={onUseRuntime}
          className="w-full rounded-lg border border-border bg-surface/40 px-4 py-3 text-left hover:border-primary/50 hover:bg-surface transition-colors"
        >
          <div className="flex items-start gap-3">
            <Monitor className="mt-0.5 size-5 text-primary shrink-0" />
            <div>
              <p className="text-sm font-semibold text-foreground">Desktop Runtime</p>
              <p className="mt-1 text-xs leading-relaxed text-text-muted">
                Full desktop workspace with Chat, Cards, Agents, and runtime management.
              </p>
            </div>
          </div>
        </button>

        <button
          type="button"
          onClick={onUseBrowserMode}
          className="w-full rounded-lg border border-border bg-surface/40 px-4 py-3 text-left hover:border-primary/50 hover:bg-surface transition-colors"
        >
          <div className="flex items-start gap-3">
            <Globe2 className="mt-0.5 size-5 text-primary shrink-0" />
            <div>
              <p className="text-sm font-semibold text-foreground">Browser Mode</p>
              <p className="mt-1 text-xs leading-relaxed text-text-muted">
                Chat-focused browser setup using a Git remote, token, and IndexedDB.
              </p>
            </div>
          </div>
        </button>
      </div>
    </SetupShell>
  );
}
