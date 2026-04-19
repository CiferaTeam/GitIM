import { AlertTriangle, Loader2 } from "lucide-react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Button } from "@/components/ui/button";
import { useVersionCheck } from "@/hooks/use-version-check";

/**
 * Header-right indicator for self-update state.
 *
 * Hidden entirely when idle and no newer version is known. Surfaces three
 * distinct presentations (idle-with-update / busy / error) through one control
 * that doubles as the popover trigger — keeps the header chrome minimal.
 *
 * UI primitive note: the plan spec calls for Radix HoverCard, but this project
 * ships Tooltip + Popover only. Wrapping both around one trigger conflicts on
 * event handling (Radix does not cleanly compose two asChild triggers on the
 * same button), so we use a native `title` for the hover hint and Popover for
 * the click panel — matching the pattern the neighboring HelpCircle button uses.
 */
export function UpdateIndicator() {
  const {
    current,
    latest,
    hasUpdate,
    isUpdating,
    isRestarting,
    error,
    triggerUpdate,
  } = useVersionCheck();

  const busy = isUpdating || isRestarting;
  // Stay mounted while busy even if hasUpdate flips (current = latest mid-update).
  const visible = hasUpdate || busy || !!error;
  if (!visible) return null;

  // Icon color follows severity: red on error, amber otherwise. A spinner
  // replaces the triangle while the restart window is in flight.
  const iconColor = error ? "text-red-500" : "text-amber-500";
  const Icon = busy ? Loader2 : AlertTriangle;
  const iconClasses = busy ? "size-4 animate-spin" : "size-4";

  let hintText: string;
  if (error) hintText = "Update failed";
  else if (isRestarting) hintText = "Restarting…";
  else if (isUpdating) hintText = "Updating…";
  else hintText = latest ? `New version v${latest} available` : "Update available";

  const buttonLabel = isRestarting
    ? "Restarting…"
    : isUpdating
      ? "Updating…"
      : "Update & Restart";

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          title={hintText}
          aria-label={hintText}
          className={`flex items-center justify-center w-7 h-7 rounded-md ${iconColor} hover:bg-surface/60 transition-colors`}
        >
          <Icon className={iconClasses} />
        </button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-64">
        <div className="space-y-2">
          <div className="text-sm font-medium text-foreground">
            {error
              ? "Update failed"
              : latest
                ? `New version v${latest} available`
                : "Update available"}
          </div>
          <div className="text-xs text-text-muted">
            You're on v{current ?? "unknown"}
          </div>
          {error ? (
            <div className="text-xs text-red-500 break-words">{error}</div>
          ) : (
            <Button
              size="sm"
              className="w-full"
              onClick={() => {
                void triggerUpdate();
              }}
              disabled={busy}
            >
              {buttonLabel}
            </Button>
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}
